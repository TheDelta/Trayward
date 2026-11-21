//! MQTT: Trayward's state and events on a broker, for Home Assistant and
//! the like (Settings → Extensions → MQTT, off by default).
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
    /// The host has characters a host name can't have, or is empty.
    Host,
    /// The port isn't a number from 1 to 65535.
    Port,
}

impl BadAddress {
    pub fn text(self) -> &'static str {
        match self {
            BadAddress::NotMqtt => i18n::t("No mqtt:// address on the clipboard"),
            BadAddress::Host => i18n::t("The address isn't a host name or IP address"),
            BadAddress::Port => i18n::t("The port is a number from 1 to 65535"),
        }
    }
}

impl Broker {
    /// A broker from its parts, as typed into the settings: the port empty for
    /// the default (1883, or 8883 over TLS), the user name empty for none.
    pub fn new(tls: bool, host: &str, port: &str, user: &str, password: Option<&str>) -> Result<Broker, BadAddress> {
        let host = host.trim();
        // "mqtt://host:1883" typed into the address: the parts of it, and from the
        // other fields what it doesn't say (the port, the user, the password).
        if let Some((_, rest)) = host.split_once("://") {
            let mut b = parse(host)?;
            let names_port = rest.rsplit('@').next().is_some_and(|h| h.contains(':'));
            if !names_port && !port.trim().is_empty() {
                b.port = port.trim().parse().ok().filter(|p| *p != 0).ok_or(BadAddress::Port)?;
            }
            if b.user.is_none() {
                b.user = Some(user.trim().to_string()).filter(|u| !u.is_empty());
            }
            if b.password.is_none() && b.user.is_some() {
                b.password = password.map(str::to_string);
            }
            return Ok(b);
        }
        let port = match port.trim() {
            "" => default_port(tls),
            p => p.parse().ok().filter(|p| *p != 0).ok_or(BadAddress::Port)?,
        };
        if !host_ok(host) {
            return Err(BadAddress::Host);
        }
        let user = Some(user.trim().to_string()).filter(|u| !u.is_empty());
        let password = password.filter(|_| user.is_some()).map(str::to_string);
        Ok(Broker { tls, host: host.to_ascii_lowercase(), port, user, password })
    }
}

fn default_port(tls: bool) -> u16 {
    if tls { 8883 } else { 1883 }
}

/// A host name or IPv4 address: letters, digits, `.`, `-` and `_`.
fn host_ok(host: &str) -> bool {
    !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

/// Reads a broker address. `tcp://` and `ssl://` work too; the port
/// defaults to 1883, or 8883 over TLS.
pub fn parse(url: &str) -> Result<Broker, BadAddress> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let (rest, tls) = [("mqtt://", false), ("tcp://", false), ("mqtts://", true), ("ssl://", true)]
        .iter()
        .find_map(|(p, tls)| url.get(p.len()..).filter(|_| lower.starts_with(p)).map(|rest| (rest, *tls)))
        .ok_or(BadAddress::NotMqtt)?;
    let rest = rest.trim_end_matches('/');
    let (userinfo, hostport) = match rest.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| BadAddress::NotMqtt)?),
        None => (hostport, default_port(tls)),
    };
    if !host_ok(host) || port == 0 {
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
    out.extend_from_slice(s.get(..usize::from(u16::MAX)).unwrap_or(s));
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
        // Mosquitto answers 5 for a wrong user name or password, not only for missing rights.
        5 => Some(i18n::t("not authorised: the broker refused this user name and password").into()),
        c => Some(i18n::f("the broker answered {0}", &[&c])),
    }
}

// ---------- Home Assistant discovery ----------

/// Whose limits Home Assistant gets entities for: Claude's while Claude Code is
/// used on the PC, Codex's and Copilot's while Trayward shows them. Of those,
/// only the limits the plan has: a limit it doesn't have (Claude's 5-hour limit
/// on Ultra, unless it's simulated) gets no entity rather than one that's
/// always unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Providers {
    pub claude: bool,
    pub codex: bool,
    pub copilot: bool,
    /// Claude has a 5-hour limit (or a simulated one).
    pub claude_five_hour: bool,
    /// Claude's Fable limit is there and shown.
    pub claude_fable: bool,
    /// Codex has a 5-hour limit.
    pub codex_five_hour: bool,
}

/// Entities of earlier versions, before the keys named their provider: an empty
/// config removes them from Home Assistant.
const RETIRED: [&str; 6] = ["weekly", "five_hour", "fable", "weekly_resets", "five_hour_resets", "plan"];

/// The discovery messages: topic and retained config, one per entity. Entities
/// of a provider that isn't shown (and of earlier versions) get an empty
/// config, which removes them. `version` is the app's.
pub fn discovery(node: &str, providers: Providers, version: &str) -> Vec<(String, String)> {
    let base = base(node);
    let device = json!({
        "identifiers": [format!("trayward_{node}")],
        "name": format!("Trayward {}", node.to_uppercase()),
        "manufacturer": "Trayward",
        "model": "AI coding agents' limits and sessions",
        "sw_version": version,
        "configuration_url": crate::update::REPO_URL,
    });
    // Who sends the discovery messages (Home Assistant shows it in the MQTT info).
    let origin = json!({
        "name": "Trayward",
        "sw_version": version,
        "support_url": crate::update::REPO_URL,
    });
    let topic = |component: &str, key: &str| format!("homeassistant/{component}/trayward_{node}/{key}/config");
    let entity = |component: &str, key: &str, name: &str, extra: Value| {
        // Entity ids come from the device and entity names (e.g. sensor.trayward_my_pc_claude_weekly_limit).
        let mut config = json!({
            "name": name,
            "unique_id": format!("trayward_{node}_{key}"),
            "availability_topic": format!("{base}/availability"),
            "device": device,
            "origin": origin,
        });
        if let (Some(c), Some(e)) = (config.as_object_mut(), extra.as_object()) {
            c.extend(e.clone());
        }
        (topic(component, key), config.to_string())
    };
    let state = format!("{base}/state");
    let value = |key: &str| format!("{{{{ value_json.{key} }}}}");
    let percent = |key: &str, name: &str| {
        let extra = json!({
            "state_topic": state,
            "value_template": value(key),
            "unit_of_measurement": "%",
            "state_class": "measurement",
            "icon": "mdi:gauge",
        });
        entity("sensor", key, name, extra)
    };
    let resets = |key: &str, name: &str| {
        let extra = json!({ "state_topic": state, "value_template": value(key), "device_class": "timestamp" });
        entity("sensor", key, name, extra)
    };
    let plan = |key: &str, name: &str| {
        entity("sensor", key, name, json!({ "state_topic": state, "value_template": value(key), "icon": "mdi:star" }))
    };
    let count = |key: &str, name: &str, icon: &str| {
        let extra = json!({
            "state_topic": state,
            "value_template": value(key),
            "state_class": "measurement",
            "icon": icon,
            "json_attributes_topic": state,
            "json_attributes_template": "{{ {'sessions': value_json.sessions} | tojson }}",
        });
        entity("sensor", key, name, extra)
    };
    let mut out = Vec::new();
    let (p, claude, codex) = (providers, providers.claude, providers.codex);
    // Each entity, and whether it's there: a provider that isn't shown, or a
    // limit its plan doesn't have, gets an empty config, which removes it.
    let entities = [
        (claude, percent("claude_weekly", "Claude weekly limit")),
        (claude && p.claude_five_hour, percent("claude_five_hour", "Claude 5-hour limit")),
        (claude && p.claude_fable, percent("claude_fable", "Claude Fable limit")),
        (claude, resets("claude_weekly_resets", "Claude weekly limit resets")),
        (claude && p.claude_five_hour, resets("claude_five_hour_resets", "Claude 5-hour limit resets")),
        (claude && p.claude_fable, resets("claude_fable_resets", "Claude Fable limit resets")),
        (claude, plan("claude_plan", "Claude plan")),
        (codex, percent("codex_weekly", "Codex weekly limit")),
        (codex && p.codex_five_hour, percent("codex_five_hour", "Codex 5-hour limit")),
        (codex, resets("codex_weekly_resets", "Codex weekly limit resets")),
        (codex && p.codex_five_hour, resets("codex_five_hour_resets", "Codex 5-hour limit resets")),
        (codex, plan("codex_plan", "Codex plan")),
        (p.copilot, percent("copilot_monthly", "Copilot monthly limit")),
        (p.copilot, resets("copilot_monthly_resets", "Copilot monthly limit resets")),
        (p.copilot, plan("copilot_plan", "Copilot plan")),
    ];
    out.extend(entities.into_iter().map(|(there, (t, config))| (t, if there { config } else { String::new() })));
    out.extend([
        count("needs_you", "Sessions needing you", "mdi:account-alert"),
        count("working", "Sessions working", "mdi:cog"),
        count("done", "Sessions done", "mdi:check-circle"),
        // Sessions waiting for the user whose prompt cache runs out soon; which, and when, as attributes.
        entity(
            "sensor",
            "caches_expiring",
            "Caches expiring soon",
            json!({
                "state_topic": state,
                "value_template": value("caches_expiring"),
                "state_class": "measurement",
                "icon": "mdi:timer-alert",
                "json_attributes_topic": state,
                "json_attributes_template": "{{ {'sessions': value_json.caches_expiring_sessions} | tojson }}",
            }),
        ),
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
            "event",
            "session",
            "Session",
            json!({ "state_topic": format!("{base}/event"), "event_types": EVENT_TYPES, "icon": "mdi:robot" }),
        ),
    ]);
    out.extend(RETIRED.iter().map(|key| (topic("sensor", key), String::new())));
    out
}

// ---------- the connection ----------

/// What the app tells the MQTT thread.
pub enum Command {
    /// Connect to this broker (None: disconnect).
    Config(Option<Broker>),
    /// The state changed (JSON).
    State(String),
    /// The Home Assistant discovery messages changed (see [`discovery`]).
    Discovery(Vec<(String, String)>),
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
            let tls = crate::os::tls::TlsStream::connect(tcp, &b.host).map_err(|e| i18n::f("TLS: {0}", &[&e]))?;
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

/// The connection between commands: what to connect to, what was last sent,
/// and when to try again.
struct Link<R: Fn(Status)> {
    node: String,
    base: String,
    broker: Option<Broker>,
    conn: Option<Conn>,
    /// The state and the discovery messages last given, sent again after a reconnect.
    state: Option<String>,
    configs: Vec<(String, String)>,
    next_try: Instant,
    failures: usize,
    last_status: Option<Status>,
    report: R,
}

impl<R: Fn(Status)> Link<R> {
    fn new(report: R) -> Self {
        let node = node();
        Link {
            base: base(&node),
            node,
            broker: None,
            conn: None,
            state: None,
            configs: Vec::new(),
            next_try: Instant::now(),
            failures: 0,
            last_status: None,
            report,
        }
    }

    /// Reports a status when it differs from the last one reported.
    fn status(&mut self, s: Status) {
        if self.last_status.as_ref() != Some(&s) {
            (self.report)(s.clone());
            self.last_status = Some(s);
        }
    }

    /// How long to wait for a command: until the next ping, or the next try to connect.
    fn wait(&self) -> Duration {
        match (&self.conn, &self.broker) {
            (Some(c), _) => PING_EVERY.saturating_sub(c.last_sent.elapsed()),
            (None, Some(_)) => self.next_try.saturating_duration_since(Instant::now()),
            (None, None) => Duration::from_secs(3600),
        }
    }

    /// Publishes, if connected. Returns whether the connection was lost over it.
    fn publish(&mut self, topic: &str, payload: &str, retain: bool) -> bool {
        self.conn.as_mut().is_some_and(|c| c.publish(topic, payload, retain).is_err())
    }

    fn close(&mut self) {
        if let Some(c) = self.conn.take() {
            c.close(&self.node);
        }
    }

    /// Takes a command in. Returns whether the connection was lost over it.
    fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Config(b) => {
                if b != self.broker {
                    self.close();
                    self.broker = b;
                    self.failures = 0;
                    self.next_try = Instant::now();
                    self.last_status = None;
                }
                false
            }
            Command::State(s) => {
                let lost = self.publish(&format!("{}/state", self.base), &s, true);
                self.state = Some(s);
                lost
            }
            Command::Discovery(d) => {
                // Only what changed: an entity added, changed or removed.
                let lost = match &mut self.conn {
                    Some(c) => d
                        .iter()
                        .filter(|x| !self.configs.contains(x))
                        .any(|(topic, config)| c.publish(topic, config, true).is_err()),
                    None => false,
                };
                self.configs = d;
                lost
            }
            Command::Event(e) => self.publish(&format!("{}/event", self.base), &e, false),
            Command::Test => self.test(),
            Command::Stop => false,
        }
    }

    /// The settings' "Test": the answer is reported in any case, they wait for it.
    fn test(&mut self) -> bool {
        self.last_status = None;
        let Some(c) = &mut self.conn else {
            self.next_try = Instant::now();
            return false;
        };
        let lost = c.ping().is_err();
        if !lost {
            self.status(Ok(()));
        }
        lost
    }

    /// Nothing to send for a while: a ping keeps the connection, and tells whether it's still there.
    fn keep_alive(&mut self) -> bool {
        self.conn.as_mut().is_some_and(|c| c.last_sent.elapsed() >= PING_EVERY && c.ping().is_err())
    }

    fn lost(&mut self) {
        self.conn = None;
        self.status(Err(i18n::t("connection lost").into()));
        self.next_try = Instant::now() + Duration::from_secs(BACKOFF.first().copied().unwrap_or(1));
    }

    /// Connects, when there's a broker, no connection and it's time to try.
    fn connect(&mut self) {
        let (None, Some(broker)) = (&self.conn, &self.broker) else { return };
        if Instant::now() < self.next_try {
            return;
        }
        match Conn::open(broker, &self.node).and_then(|c| self.greet(c)) {
            Ok(c) => {
                self.conn = Some(c);
                self.failures = 0;
                self.status(Ok(()));
            }
            Err(e) => {
                let wait = BACKOFF.get(self.failures).or(BACKOFF.last()).copied().unwrap_or(1);
                self.next_try = Instant::now() + Duration::from_secs(wait);
                self.failures += 1;
                self.status(Err(e));
            }
        }
    }

    /// What a new connection is told first: that Trayward is there, every
    /// entity, and the state.
    fn greet(&self, mut c: Conn) -> Result<Conn, String> {
        let mut send = |topic: &str, payload: &str| c.publish(topic, payload, true).map_err(|e| e.to_string());
        send(&format!("{}/availability", self.base), "online")?;
        for (topic, config) in &self.configs {
            send(topic, config)?;
        }
        if let Some(s) = &self.state {
            send(&format!("{}/state", self.base), s)?;
        }
        Ok(c)
    }
}

/// Runs the connection until `Stop` or the app goes away. `report` gets the
/// status whenever it changes.
pub fn run(rx: Receiver<Command>, report: impl Fn(Status)) {
    let mut link = Link::new(report);
    loop {
        let lost = match rx.recv_timeout(link.wait().max(Duration::from_millis(10))) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => {
                link.close();
                return;
            }
            Ok(command) => link.command(command),
            Err(RecvTimeoutError::Timeout) => link.keep_alive(),
        };
        if lost {
            link.lost();
        }
        link.connect();
    }
}

/// Starts the MQTT thread; it waits for a `Config`.
pub fn spawn(report: impl Fn(Status) + Send + 'static) -> Sender<Command> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || run(rx, report));
    tx
}

/// German translations of this file's text (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("No mqtt:// address on the clipboard", "Keine mqtt://-Adresse in der Zwischenablage"),
    ("The address isn't a host name or IP address", "Die Adresse ist kein Hostname und keine IP-Adresse"),
    ("The port is a number from 1 to 65535", "Der Port ist eine Zahl von 1 bis 65535"),
    ("the broker doesn't speak MQTT 3.1.1", "der Broker spricht kein MQTT 3.1.1"),
    ("the broker refused the client id", "der Broker lehnt die Client-ID ab"),
    ("the broker is unavailable", "der Broker ist nicht verfügbar"),
    ("wrong user name or password", "Benutzer oder Passwort falsch"),
    ("not authorised: the broker refused this user name and password", "nicht berechtigt: der Broker lehnt diesen Benutzernamen und dieses Passwort ab"),
    ("the broker answered {0}", "der Broker antwortete {0}"),
    ("{0} not found ({1})", "{0} nicht gefunden ({1})"),
    ("{0} not found", "{0} nicht gefunden"),
    ("TLS: {0}", "TLS: {0}"),
    ("no answer from the broker", "keine Antwort vom Broker"),
    ("connection lost", "Verbindung verloren"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broker_from_its_parts() {
        let b = Broker::new(false, " Core-Mosquitto ", "", "homeassistant", Some("p@ss:word")).unwrap();
        assert_eq!((b.host.as_str(), b.port, b.tls), ("core-mosquitto", 1883, false));
        assert_eq!((b.user.as_deref(), b.password.as_deref()), (Some("homeassistant"), Some("p@ss:word")));
        // Stored without the password, and read back the same.
        assert_eq!(b.shown(), "mqtt://homeassistant@core-mosquitto:1883");
        assert_eq!(parse(&b.shown()).unwrap().user.as_deref(), Some("homeassistant"));
        assert_eq!(Broker::new(true, "192.168.1.5", "", "", None).unwrap().port, 8883);
        // No user: no password either.
        assert_eq!(Broker::new(false, "ha", "1884", "", Some("x")).unwrap().password, None);
        assert_eq!(Broker::new(false, "ha", "0", "", None), Err(BadAddress::Port));
        assert_eq!(Broker::new(false, "ha", "70000", "", None), Err(BadAddress::Port));
        assert_eq!(Broker::new(false, "home assistant", "", "", None), Err(BadAddress::Host));
        assert_eq!(Broker::new(false, "", "", "", None), Err(BadAddress::Host));
        // A whole address typed into the host field.
        assert_eq!(Broker::new(false, "mqtts://ha:8884", "", "", None).unwrap().port, 8884);
        // What the address doesn't say comes from the other fields: the password isn't lost.
        let b = Broker::new(false, "mqtt://homeassistant.local", "1884", "ha", Some("pw")).unwrap();
        assert_eq!((b.port, b.user.as_deref(), b.password.as_deref()), (1884, Some("ha"), Some("pw")));
        let b = Broker::new(false, "mqtt://me@ha:1885", "1884", "other", Some("pw")).unwrap();
        assert_eq!((b.port, b.user.as_deref(), b.password.as_deref()), (1885, Some("me"), Some("pw")));
        let b = Broker::new(false, "mqtt://me:mine@ha", "", "", Some("pw")).unwrap();
        assert_eq!(b.password.as_deref(), Some("mine"));
        assert_eq!(Broker::new(false, "mqtt://ha", "", "", Some("pw")).unwrap().password, None);
    }

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
        let d = discovery(
            "dev-pc",
            Providers {
                claude: true,
                codex: true,
                copilot: false,
                claude_five_hour: false,
                claude_fable: true,
                codex_five_hour: true,
            },
            "1.2.3",
        );
        assert!(d.iter().all(|(t, _)| t.starts_with("homeassistant/") && t.contains("/trayward_dev-pc/")));
        let config = |key: &str| {
            let (_, c) = d.iter().find(|(t, _)| t.ends_with(&format!("/{key}/config"))).unwrap();
            c.clone()
        };
        let v: Value = serde_json::from_str(&config("claude_weekly")).unwrap();
        assert_eq!(v["name"], "Claude weekly limit");
        assert_eq!(v["state_topic"], "trayward/dev-pc/state");
        assert_eq!(v["value_template"], "{{ value_json.claude_weekly }}");
        assert_eq!(v["availability_topic"], "trayward/dev-pc/availability");
        assert_eq!(v["device"]["identifiers"][0], "trayward_dev-pc");
        assert_eq!(v["device"]["configuration_url"], "https://github.com/TheDelta/trayward");
        assert_eq!(v["device"]["sw_version"], "1.2.3");
        assert_eq!(v["origin"]["name"], "Trayward");
        // Codex is shown, Copilot isn't: its entities are removed, like those of earlier versions.
        assert!(!config("codex_plan").is_empty());
        assert!(config("copilot_monthly").is_empty() && config("copilot_plan").is_empty());
        assert!(config("weekly").is_empty() && config("plan").is_empty());
        // A limit the plan doesn't have (Claude's 5-hour limit on Ultra): no entity, rather than "unknown".
        assert!(config("claude_five_hour").is_empty() && config("claude_five_hour_resets").is_empty());
        assert!(!config("claude_fable_resets").is_empty() && !config("codex_five_hour").is_empty());
        // Done sessions and expiring caches, with the sessions as attributes.
        assert_eq!(serde_json::from_str::<Value>(&config("done")).unwrap()["value_template"], "{{ value_json.done }}");
        let caches: Value = serde_json::from_str(&config("caches_expiring")).unwrap();
        assert_eq!(caches["name"], "Caches expiring soon");
        assert!(caches["json_attributes_template"].as_str().unwrap().contains("caches_expiring_sessions"));
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
        tx.send(Command::Discovery(discovery("dev-pc", Providers::default(), "1.2.3"))).unwrap();
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

    #[test]
    fn says_what_is_wrong() {
        i18n::set(i18n::Choice::English);
        assert!(BadAddress::NotMqtt.text().contains("mqtt://"));
        assert!(BadAddress::Host.text().contains("host name"));
        assert!(BadAddress::Port.text().contains("65535"));
        for (code, says) in
            [(1, "3.1.1"), (2, "client id"), (3, "unavailable"), (5, "not authorised"), (9, "answered 9")]
        {
            assert!(connack_error(code).unwrap().contains(says), "{code}");
        }
        // A whole address, with a port in the port field that isn't one.
        assert_eq!(Broker::new(false, "mqtt://ha", "x", "", None), Err(BadAddress::Port));
        assert_eq!(Broker::new(false, "mqtt://bro ker", "", "", None), Err(BadAddress::NotMqtt));
        let n = node();
        assert!(!n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c)));
    }

    /// One MQTT packet: its header byte and its body.
    fn read_packet(s: &mut impl Read) -> Option<(u8, Vec<u8>)> {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).ok()?;
        let header = b[0];
        let (mut len, mut shift) = (0usize, 0);
        loop {
            s.read_exact(&mut b).ok()?;
            len |= usize::from(b[0] & 0x7F) << shift;
            shift += 7;
            if b[0] & 0x80 == 0 {
                break;
            }
        }
        let mut body = vec![0; len];
        s.read_exact(&mut body).ok()?;
        Some((header, body))
    }

    /// A stand-in broker on this PC for one connection: answers CONNECT with `ack`
    /// (none: hangs up), each PINGREQ with the next of `pongs` (none left: hangs
    /// up), and returns the headers of the packets it got.
    fn stand_in(ack: Option<[u8; 4]>, pongs: Vec<[u8; 2]>) -> (Broker, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let broker = parse(&format!("mqtt://127.0.0.1:{}", listener.local_addr().unwrap().port())).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut pongs = pongs.into_iter();
            while let Some((header, _)) = read_packet(&mut s) {
                got.push(header);
                let answer = match header {
                    0x10 => ack.map(Vec::from),
                    0xC0 => pongs.next().map(Vec::from),
                    _ => continue,
                };
                let Some(answer) = answer else { break };
                if s.write_all(&answer).is_err() {
                    break;
                }
            }
            got
        });
        (broker, handle)
    }

    fn new_link() -> (Link<impl Fn(Status)>, std::sync::mpsc::Receiver<Status>) {
        i18n::set(i18n::Choice::English);
        let (tx, rx) = std::sync::mpsc::channel();
        (Link::new(move |s| drop(tx.send(s))), rx)
    }

    #[test]
    fn tells_why_a_broker_turned_it_away() {
        for (ack, says) in
            [(Some([0x20, 2, 0, 5]), "not authorised"), (Some([0x30, 2, 0, 0]), "no answer"), (None, "no answer")]
        {
            let (broker, handle) = stand_in(ack, Vec::new());
            let (mut link, status) = new_link();
            assert_eq!(link.wait(), Duration::from_secs(3600));
            link.command(Command::Config(Some(broker.clone())));
            link.connect();
            let err = status.try_recv().unwrap().unwrap_err();
            assert!(err.contains(says), "{err}");
            assert_eq!(link.failures, 1);
            // Not again before its time, and the same broker once more changes nothing.
            assert!(link.wait() > Duration::from_secs(1));
            link.connect();
            link.command(Command::Config(Some(broker)));
            assert_eq!(link.failures, 1);
            assert!(status.try_recv().is_err());
            // Test: try now.
            assert!(!link.command(Command::Test));
            assert!(link.wait() <= Duration::from_millis(10));
            drop(link);
            let _ = handle.join();
        }
    }

    #[test]
    fn a_broker_that_is_not_there() {
        // A port nobody listens on any more.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let broker = parse(&format!("mqtt://127.0.0.1:{}", listener.local_addr().unwrap().port())).unwrap();
        drop(listener);
        let (mut link, status) = new_link();
        link.command(Command::Config(Some(broker)));
        link.connect();
        assert!(status.try_recv().unwrap().is_err());
        // Over TLS, to something that hangs up.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut tls = parse(&format!("mqtts://127.0.0.1:{}", listener.local_addr().unwrap().port())).unwrap();
        tls.tls = true;
        let handle = std::thread::spawn(move || drop(listener.accept()));
        let (mut link, status) = new_link();
        link.command(Command::Config(Some(tls)));
        link.connect();
        assert!(status.try_recv().unwrap().unwrap_err().starts_with("TLS: "));
        drop(link);
        let _ = handle.join();
    }

    #[test]
    fn keeps_the_connection_and_notices_it_lost() {
        let (broker, handle) = stand_in(Some([0x20, 2, 0, 0]), vec![[0xD0, 0x00], [0x00, 0x00]]);
        let (mut link, status) = new_link();
        link.command(Command::Discovery(vec![("t/a".into(), "1".into())]));
        link.command(Command::Config(Some(broker)));
        link.connect();
        assert_eq!(status.try_recv().unwrap(), Ok(()));
        assert!(link.wait() <= PING_EVERY);
        // The same discovery again sends nothing; a changed one only what changed.
        assert!(!link.command(Command::Discovery(vec![("t/a".into(), "1".into())])));
        assert!(!link.command(Command::Discovery(vec![("t/a".into(), "1".into()), ("t/b".into(), "2".into())])));
        // Test: a ping, and its answer reported though nothing changed.
        assert!(!link.command(Command::Test));
        assert_eq!(status.try_recv().unwrap(), Ok(()));
        // Nothing sent for a while: a ping, and a wrong answer means the connection is gone.
        assert!(!link.keep_alive());
        if let Some(c) = link.conn.as_mut() {
            c.last_sent = Instant::now().checked_sub(PING_EVERY).unwrap_or(c.last_sent);
        }
        assert!(link.keep_alive());
        link.lost();
        assert_eq!(status.try_recv().unwrap(), Err("connection lost".into()));
        assert!(link.conn.is_none() && link.wait() > Duration::from_secs(1));
        // Publishing while not connected loses nothing.
        assert!(!link.command(Command::State("{}".into())) && !link.command(Command::Event("{}".into())));
        assert!(!link.command(Command::Stop));
        drop(link);
        let got = handle.join().unwrap();
        // CONNECT, online, the two configs, two pings.
        assert_eq!(got, [0x10, 0x31, 0x31, 0x31, 0xC0, 0xC0]);
    }

    #[test]
    fn stops_when_the_app_goes_away() {
        let (status, _rx) = std::sync::mpsc::channel::<Status>();
        let tx = spawn(move |s| drop(status.send(s)));
        tx.send(Command::Test).unwrap();
        // The app gone: the thread ends.
        drop(tx);
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        drop(tx);
        run(rx, |_| {});
    }
}
