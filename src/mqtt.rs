//! MQTT: Trayward's state and events on a broker, for Home Assistant and
//! the like (Settings → MQTT, off by default).
//!
//! A small MQTT 3.1.1 client over TCP, or TLS with `mqtts://` (`tls`), on its own thread:
//! it connects, says it's online (retained, with a last will that says
//! offline), announces its entities to Home Assistant (MQTT discovery,
//! retained), and publishes the state (retained) whenever it changes and
//! events as they happen. It pings every 30 s and reconnects after a failure,
//! waiting longer each time. Only names and states go out: session names,
//! limits, counts; no prompts, replies or paths.
//!
//! Topics, with `<node>` this PC's name:
//! - `trayward/<node>/availability`: `online` / `offline`
//! - `trayward/<node>/state`: JSON (see `App::mqtt_state`)
//! - `trayward/<node>/event`: JSON `{"event_type": …, "session": …, "text": …}`
//! - `homeassistant/<component>/trayward_<node>/<key>/config`: discovery

use crate::i18n;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Seconds the broker waits without hearing from us before it says we're offline.
const KEEP_ALIVE: u16 = 60;
/// How often we ping when there's nothing else to send.
const PING_EVERY: Duration = Duration::from_secs(30);
/// Connecting, reading an answer, writing.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Waits between attempts after a failure, longest last.
const BACKOFF: [u64; 5] = [5, 10, 30, 60, 300];

/// The event types the state machine knows (Home Assistant's `event_types`).
pub const EVENT_TYPES: [&str; 4] = ["needs_you", "done", "limit_warning", "free_again"];

/// A broker address: `mqtt[s]://[user[:password]@]host[:port]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broker {
    /// `mqtts://`: over TLS.
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub password: Option<String>,
}

impl Broker {
    /// The address as shown and stored: without the password.
    pub fn shown(&self) -> String {
        // Escaped where the address would be read differently (see `decode`).
        let escape = |u: &str| u.replace('%', "%25").replace(':', "%3A").replace('@', "%40").replace('/', "%2F");
        let user = self.user.as_deref().map(|u| format!("{}@", escape(u))).unwrap_or_default();
        let scheme = if self.tls { "mqtts" } else { "mqtt" };
        format!("{scheme}://{user}{}:{}", self.host, self.port)
    }
}

/// Why an address isn't usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadAddress {
    NotMqtt,
}

impl BadAddress {
    pub fn text(self) -> &'static str {
        match self {
            BadAddress::NotMqtt => i18n::t("No mqtt:// address on the clipboard"),
        }
    }
}

/// Reads a broker address. `tcp://` and `ssl://` work too; the port
/// defaults to 1883, or 8883 over TLS.
pub fn parse(url: &str) -> Result<Broker, BadAddress> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let (rest, tls) = [("mqtt://", false), ("tcp://", false), ("mqtts://", true), ("ssl://", true)]
        .iter()
        .find_map(|(p, tls)| lower.starts_with(p).then(|| (&url[p.len()..], *tls)))
        .ok_or(BadAddress::NotMqtt)?;
    let rest = rest.trim_end_matches('/');
    let (userinfo, hostport) = match rest.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| BadAddress::NotMqtt)?),
        None => (hostport, if tls { 8883 } else { 1883 }),
    };
    let ok_host = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c));
    if !ok_host || port == 0 {
        return Err(BadAddress::NotMqtt);
    }
    let (user, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((name, pw)) => (Some(decode(name)), Some(decode(pw))),
            None => (Some(decode(u)), None),
        },
        None => (None, None),
    };
    Ok(Broker { tls, host: host.to_ascii_lowercase(), port, user: user.filter(|u| !u.is_empty()), password })
}

/// Undoes `%xx` escapes in a user name or password.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%').then(|| s.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// This PC's name as it appears in topics: lowercase letters, digits, `-` and `_`.
pub fn node() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into());
    let node: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    if node.is_empty() { "pc".into() } else { node }
}

fn base(node: &str) -> String {
    format!("trayward/{node}")
}

// ---------- packets ----------

fn remaining_length(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
}

fn string(s: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len().min(u16::MAX as usize) as u16).to_be_bytes());
    out.extend_from_slice(&s[..s.len().min(u16::MAX as usize)]);
}

fn packet(header: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![header];
    remaining_length(body.len(), &mut out);
    out.extend_from_slice(body);
    out
}

/// CONNECT with a clean session and a retained last will saying `offline`.
fn connect_packet(client_id: &str, will_topic: &str, user: Option<&str>, password: Option<&str>) -> Vec<u8> {
    let mut flags = 0x02 | 0x04 | 0x08 | 0x20; // clean session, will, will QoS 1, will retain
    if user.is_some() {
        flags |= 0x80;
        if password.is_some() {
            flags |= 0x40;
        }
    }
    let mut body = Vec::new();
    string(b"MQTT", &mut body);
    body.push(4); // protocol level 3.1.1
    body.push(flags);
    body.extend_from_slice(&KEEP_ALIVE.to_be_bytes());
    string(client_id.as_bytes(), &mut body);
    string(will_topic.as_bytes(), &mut body);
    string(b"offline", &mut body);
    if let Some(u) = user {
        string(u.as_bytes(), &mut body);
        if let Some(p) = password {
            string(p.as_bytes(), &mut body);
        }
    }
    packet(0x10, &body)
}

/// PUBLISH at QoS 0.
fn publish_packet(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let mut body = Vec::new();
    string(topic.as_bytes(), &mut body);
    body.extend_from_slice(payload);
    packet(0x30 | u8::from(retain), &body)
}

/// What a CONNACK's return code means.
fn connack_error(code: u8) -> Option<String> {
    match code {
        0 => None,
        1 => Some(i18n::t("the broker doesn't speak MQTT 3.1.1").into()),
        2 => Some(i18n::t("the broker refused the client id").into()),
        3 => Some(i18n::t("the broker is unavailable").into()),
        4 => Some(i18n::t("wrong user name or password").into()),
        5 => Some(i18n::t("not authorised").into()),
        c => Some(i18n::f("the broker answered {0}", &[&c])),
    }
}

// ---------- Home Assistant discovery ----------

/// The discovery messages: topic and retained config, one per entity.
pub fn discovery(node: &str) -> Vec<(String, String)> {
    let base = base(node);
    let device = json!({
        "identifiers": [format!("trayward_{node}")],
        "name": format!("Trayward {}", node.to_uppercase()),
        "manufacturer": "Trayward",
        "model": "Claude Code limits and sessions",
        "sw_version": env!("CARGO_PKG_VERSION"),
    });
    let entity = |component: &str, key: &str, name: &str, extra: Value| {
        // Entity ids come from the device and entity names (e.g. sensor.trayward_my_pc_weekly_limit).
        let mut config = json!({
            "name": name,
            "unique_id": format!("trayward_{node}_{key}"),
            "availability_topic": format!("{base}/availability"),
            "device": device,
        });
        if let (Some(c), Some(e)) = (config.as_object_mut(), extra.as_object()) {
            c.extend(e.clone());
        }
        (format!("homeassistant/{component}/trayward_{node}/{key}/config"), config.to_string())
    };
    let state = format!("{base}/state");
    let percent = |key: &str, name: &str| {
        entity(
            "sensor",
            key,
            name,
            json!({
                "state_topic": state,
                "value_template": format!("{{{{ value_json.{key} }}}}"),
                "unit_of_measurement": "%",
                "state_class": "measurement",
                "icon": "mdi:gauge",
            }),
        )
    };
    let resets = |key: &str, name: &str| {
        entity(
            "sensor",
            key,
            name,
            json!({
                "state_topic": state,
                "value_template": format!("{{{{ value_json.{key} }}}}"),
                "device_class": "timestamp",
            }),
        )
    };
    let count = |key: &str, name: &str, icon: &str| {
        entity(
            "sensor",
            key,
            name,
            json!({
                "state_topic": state,
                "value_template": format!("{{{{ value_json.{key} }}}}"),
                "state_class": "measurement",
                "icon": icon,
                "json_attributes_topic": state,
                "json_attributes_template": "{{ {'sessions': value_json.sessions} | tojson }}",
            }),
        )
    };
    vec![
        percent("weekly", "Weekly limit"),
        percent("five_hour", "5-hour limit"),
        resets("weekly_resets", "Weekly limit resets"),
        resets("five_hour_resets", "5-hour limit resets"),
        count("needs_you", "Sessions needing you", "mdi:account-alert"),
        count("working", "Sessions working", "mdi:cog"),
        entity(
            "binary_sensor",
            "attention",
            "Needs you",
            json!({
                "state_topic": state,
                "value_template": "{{ 'ON' if value_json.needs_you > 0 else 'OFF' }}",
                "icon": "mdi:bell-ring",
            }),
        ),
        entity(
            "sensor",
            "plan",
            "Plan",
            json!({ "state_topic": state, "value_template": "{{ value_json.plan }}", "icon": "mdi:star" }),
        ),
        entity(
            "event",
            "session",
            "Session",
            json!({ "state_topic": format!("{base}/event"), "event_types": EVENT_TYPES, "icon": "mdi:robot" }),
        ),
    ]
}

// ---------- the connection ----------

/// What the app tells the MQTT thread.
pub enum Command {
    /// Connect to this broker (None: disconnect).
    Config(Option<Broker>),
    /// The state changed (JSON).
    State(String),
    /// Something happened (JSON with `event_type`).
    Event(String),
    /// Try to connect now (the settings' Test).
    Test,
    /// Trayward exits: say offline and disconnect.
    Stop,
}

/// What the MQTT thread tells the app: connected, or why not.
pub type Status = Result<(), String>;

/// The connection to the broker: TCP, or TLS over it.
trait Wire: Read + Write {}
impl<T: Read + Write> Wire for T {}

struct Conn {
    stream: Box<dyn Wire>,
    last_sent: Instant,
}

impl Conn {
    fn open(b: &Broker, node: &str) -> Result<Conn, String> {
        // Every address the name has, in order: `localhost` is often ::1
        // first while the broker listens on IPv4 only.
        let addrs =
            (b.host.as_str(), b.port).to_socket_addrs().map_err(|e| i18n::f("{0} not found ({1})", &[&b.host, &e]))?;
        let mut tcp = Err(i18n::f("{0} not found", &[&b.host]));
        for addr in addrs {
            tcp = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| e.to_string());
            if tcp.is_ok() {
                break;
            }
        }
        let tcp = tcp?;
        let _ = tcp.set_read_timeout(Some(TIMEOUT));
        let _ = tcp.set_write_timeout(Some(TIMEOUT));
        let _ = tcp.set_nodelay(true);
        let mut stream: Box<dyn Wire> = if b.tls {
            let tls = crate::tls::TlsStream::connect(tcp, &b.host).map_err(|e| i18n::f("TLS: {0}", &[&e]))?;
            Box::new(tls)
        } else {
            Box::new(tcp)
        };
        let will = format!("{}/availability", base(node));
        let client_id = format!("trayward-{node}");
        stream
            .write_all(&connect_packet(&client_id, &will, b.user.as_deref(), b.password.as_deref()))
            .map_err(|e| e.to_string())?;
        let mut ack = [0u8; 4];
        stream.read_exact(&mut ack).map_err(|_| i18n::t("no answer from the broker").to_string())?;
        if ack[0] != 0x20 {
            return Err(i18n::t("no answer from the broker").into());
        }
        if let Some(e) = connack_error(ack[3]) {
            return Err(e);
        }
        Ok(Conn { stream, last_sent: Instant::now() })
    }

    fn publish(&mut self, topic: &str, payload: &str, retain: bool) -> std::io::Result<()> {
        self.stream.write_all(&publish_packet(topic, payload.as_bytes(), retain))?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// PINGREQ, and the PINGRESP that must come back.
    fn ping(&mut self) -> std::io::Result<()> {
        self.stream.write_all(&[0xC0, 0x00])?;
        self.last_sent = Instant::now();
        let mut resp = [0u8; 2];
        self.stream.read_exact(&mut resp)?;
        if resp == [0xD0, 0x00] { Ok(()) } else { Err(std::io::ErrorKind::InvalidData.into()) }
    }

    fn close(mut self, node: &str) {
        let _ = self.publish(&format!("{}/availability", base(node)), "offline", true);
        let _ = self.stream.write_all(&[0xE0, 0x00]);
    }
}

/// Runs the connection until `Stop` or the app goes away. `report` gets the
/// status whenever it changes.
pub fn run(rx: Receiver<Command>, report: impl Fn(Status)) {
    let node = node();
    let base = base(&node);
    let mut broker: Option<Broker> = None;
    let mut conn: Option<Conn> = None;
    let mut state: Option<String> = None;
    let mut next_try = Instant::now();
    let mut failures = 0usize;
    let mut last_status: Option<Status> = None;
    // Reports a status when it differs from the last one reported.
    let status = |last: &mut Option<Status>, s: Status| {
        if last.as_ref() != Some(&s) {
            report(s.clone());
            *last = Some(s);
        }
    };
    loop {
        let wait = match (&conn, &broker) {
            (Some(c), _) => PING_EVERY.saturating_sub(c.last_sent.elapsed()),
            (None, Some(_)) => next_try.saturating_duration_since(Instant::now()),
            (None, None) => Duration::from_secs(3600),
        };
        let mut lost = false;
        match rx.recv_timeout(wait.max(Duration::from_millis(10))) {
            Ok(Command::Config(b)) => {
                if b != broker {
                    if let Some(c) = conn.take() {
                        c.close(&node);
                    }
                    broker = b;
                    failures = 0;
                    next_try = Instant::now();
                    last_status = None;
                }
            }
            Ok(Command::State(s)) => {
                if let Some(c) = &mut conn {
                    lost = c.publish(&format!("{base}/state"), &s, true).is_err();
                }
                state = Some(s);
            }
            Ok(Command::Event(e)) => {
                if let Some(c) = &mut conn {
                    lost = c.publish(&format!("{base}/event"), &e, false).is_err();
                }
            }
            Ok(Command::Test) => {
                // The answer is reported in any case: the settings wait for it.
                last_status = None;
                match &mut conn {
                    None => next_try = Instant::now(),
                    Some(c) => {
                        lost = c.ping().is_err();
                        if !lost {
                            status(&mut last_status, Ok(()));
                        }
                    }
                }
            }
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(c) = conn.take() {
                    c.close(&node);
                }
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(c) = &mut conn {
                    if c.last_sent.elapsed() >= PING_EVERY {
                        lost = c.ping().is_err();
                    }
                }
            }
        }
        if lost {
            conn = None;
            status(&mut last_status, Err(i18n::t("connection lost").into()));
            next_try = Instant::now() + Duration::from_secs(BACKOFF[0]);
        }
        if let (None, Some(b)) = (&conn, &broker) {
            if Instant::now() >= next_try {
                match Conn::open(b, &node).and_then(|mut c| {
                    let mut send =
                        |topic: &str, payload: &str| c.publish(topic, payload, true).map_err(|e| e.to_string());
                    send(&format!("{base}/availability"), "online")?;
                    for (topic, config) in discovery(&node) {
                        send(&topic, &config)?;
                    }
                    if let Some(s) = &state {
                        send(&format!("{base}/state"), s)?;
                    }
                    Ok(c)
                }) {
                    Ok(c) => {
                        conn = Some(c);
                        failures = 0;
                        status(&mut last_status, Ok(()));
                    }
                    Err(e) => {
                        next_try = Instant::now() + Duration::from_secs(BACKOFF[failures.min(BACKOFF.len() - 1)]);
                        failures += 1;
                        status(&mut last_status, Err(e));
                    }
                }
            }
        }
    }
}

/// Starts the MQTT thread; it waits for a `Config`.
pub fn spawn(report: impl Fn(Status) + Send + 'static) -> Sender<Command> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || run(rx, report));
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_broker_addresses() {
        let b = parse(" mqtt://ha:s3cr%40t@homeassistant.local:1884 ").unwrap();
        assert_eq!(b.host, "homeassistant.local");
        assert_eq!((b.port, b.user.as_deref(), b.password.as_deref()), (1884, Some("ha"), Some("s3cr@t")));
        assert_eq!(b.shown(), "mqtt://ha@homeassistant.local:1884");
        assert!(!b.tls);
        let b = parse("tcp://192.168.1.10").unwrap();
        assert_eq!((b.port, b.user, b.password), (1883, None, None));
        assert_eq!(parse("mqtt://user@broker/").unwrap().shown(), "mqtt://user@broker:1883");
        // A user name with characters of the address reads back the same from what's stored.
        let odd = parse("mqtt://a%3Ab%40c%25:pw@broker").unwrap();
        assert_eq!(odd.user.as_deref(), Some("a:b@c%"));
        assert_eq!(parse(&odd.shown()).unwrap().user, odd.user);
        let b = parse("mqtts://ha:pw@broker.example.org").unwrap();
        assert_eq!((b.tls, b.port), (true, 8883));
        assert_eq!(b.shown(), "mqtts://ha@broker.example.org:8883");
        assert!(parse("ssl://broker:9883").unwrap().tls);
        for bad in ["https://broker", "mqtt://", "mqtt://bro ker", "mqtt://broker:x", "broker:1883"] {
            assert_eq!(parse(bad), Err(BadAddress::NotMqtt), "{bad}");
        }
    }

    #[test]
    fn encodes_packets() {
        let mut out = Vec::new();
        remaining_length(321, &mut out);
        assert_eq!(out, [0xC1, 0x02]);
        let p = publish_packet("a/b", b"on", true);
        assert_eq!(p, [0x31, 7, 0, 3, b'a', b'/', b'b', b'o', b'n']);
        let c = connect_packet("trayward-pc", "trayward/pc/availability", Some("ha"), Some("pw"));
        assert_eq!(c[0], 0x10);
        // Protocol "MQTT", level 4, flags: user, password, will retain, will QoS 1, will, clean session.
        assert_eq!(&c[2..10], &[0, 4, b'M', b'Q', b'T', b'T', 4, 0xEE]);
        assert_eq!(&c[10..12], &60u16.to_be_bytes());
        assert!(c.ends_with(b"\x00\x02ha\x00\x02pw"));
        let anonymous = connect_packet("x", "w", None, None);
        assert_eq!(anonymous[9], 0x2E);
        assert_eq!(connack_error(0), None);
        assert!(connack_error(4).is_some());
    }

    #[test]
    fn announces_entities_to_home_assistant() {
        let d = discovery("dev-pc");
        assert!(d.iter().all(|(t, _)| t.starts_with("homeassistant/") && t.contains("/trayward_dev-pc/")));
        let (_, weekly) = d.iter().find(|(t, _)| t.contains("/weekly/")).unwrap();
        let v: Value = serde_json::from_str(weekly).unwrap();
        assert_eq!(v["state_topic"], "trayward/dev-pc/state");
        assert_eq!(v["value_template"], "{{ value_json.weekly }}");
        assert_eq!(v["availability_topic"], "trayward/dev-pc/availability");
        assert_eq!(v["device"]["identifiers"][0], "trayward_dev-pc");
        let (topic, event) = d.iter().find(|(t, _)| t.starts_with("homeassistant/event/")).unwrap();
        assert!(topic.ends_with("/session/config"));
        assert_eq!(serde_json::from_str::<Value>(event).unwrap()["event_types"][0], "needs_you");
    }

    /// Talks to a stand-in broker on this PC: connect, online, discovery, state, event, offline.
    #[test]
    fn talks_to_a_broker() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let broker = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut header = [0u8; 2];
            s.read_exact(&mut header).unwrap();
            let mut connect = vec![0u8; header[1] as usize];
            s.read_exact(&mut connect).unwrap();
            s.write_all(&[0x20, 0x02, 0x00, 0x00]).unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest);
            (header[0], rest)
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let client = std::thread::spawn(move || run(rx, move |s| status_tx.send(s).unwrap()));
        tx.send(Command::State(r#"{"weekly":46}"#.into())).unwrap();
        tx.send(Command::Config(Some(parse(&format!("mqtt://127.0.0.1:{port}")).unwrap()))).unwrap();
        assert_eq!(status_rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        tx.send(Command::Event(r#"{"event_type":"done"}"#.into())).unwrap();
        tx.send(Command::Stop).unwrap();
        client.join().unwrap();
        let (connect, rest) = broker.join().unwrap();
        assert_eq!(connect, 0x10);
        let text = String::from_utf8_lossy(&rest);
        let online = text.find("availabilityonline").unwrap();
        let discovery = text.find("homeassistant/sensor/").unwrap();
        let state = text.find(r#"{"weekly":46}"#).unwrap();
        let event = text.find(r#"{"event_type":"done"}"#).unwrap();
        let offline = text.rfind("availabilityoffline").unwrap();
        assert!(online < discovery && discovery < state && state < event && event < offline);
        assert_eq!(rest.last(), Some(&0x00), "ends with DISCONNECT");
    }
}
