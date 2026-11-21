//! Away mode's pushes to the phone: a POST to the service the user set up in
//! the settings: an ntfy topic, a Gotify server, a Discord or Slack webhook, or
//! any address that takes JSON. Only a short text goes out: a session's name
//! and state.
//!
//! The settings keep a [`Config`] (encrypted, as JSON). An address pasted, or
//! saved by an earlier version, becomes one through [`Config::from_address`].

use crate::i18n;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The services pushes can go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// ntfy.sh or the user's own ntfy server: a topic, maybe an access token.
    Ntfy,
    /// The user's own Gotify server, with an application's token.
    Gotify,
    Discord,
    Slack,
    /// Any address that takes `{"title", "message", "source"}`.
    Webhook,
}

impl Service {
    /// In the order the settings offer them.
    pub const ALL: [Service; 5] = [Service::Ntfy, Service::Gotify, Service::Discord, Service::Slack, Service::Webhook];

    /// Its name, the same in every language.
    pub fn name(self) -> &'static str {
        match self {
            Service::Ntfy => "ntfy",
            Service::Gotify => "Gotify",
            Service::Discord => "Discord",
            Service::Slack => "Slack",
            Service::Webhook => "Webhook",
        }
    }
}

/// Where pushes go: the service and what it needs. Tokens, topics and webhook
/// addresses are secrets: the settings never show them (see [`describe`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "service", rename_all = "lowercase")]
pub enum Config {
    Ntfy {
        /// `https://ntfy.sh`, or the user's server (maybe under a path).
        server: String,
        topic: String,
        /// An access token (`tk_…`), for a protected topic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    Gotify {
        /// The server's address (maybe under a path).
        server: String,
        /// An application's token (Gotify → Apps).
        token: String,
    },
    Discord {
        address: String,
    },
    Slack {
        address: String,
    },
    Webhook {
        address: String,
    },
}

/// What the settings' form has typed in. Empty fields of a secret keep the
/// saved value (the form doesn't show secrets).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    pub server: String,
    pub topic: String,
    pub token: String,
    pub address: String,
}

/// ntfy's public server, when none is given.
pub const NTFY_SERVER: &str = "https://ntfy.sh";

/// An address taken apart: `https://`, or `http://` for a server on the user's
/// own network (not encrypted: the settings say so); no user name or password in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Address {
    host: String,
    port: u16,
    secure: bool,
    /// Path and query, starting with `/`.
    path: String,
}

impl Address {
    /// `https://host[:port]`, the port only when it isn't the usual one.
    fn origin(&self) -> String {
        let (scheme, usual) = if self.secure { ("https", 443) } else { ("http", 80) };
        if self.port == usual {
            format!("{scheme}://{}", self.host)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }
}

fn parse(url: &str) -> Option<Address> {
    let url = url.trim();
    let scheme = |s: &str| url.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s));
    let (rest, secure) = if scheme("https://") {
        (url.get(8..)?, true)
    } else if scheme("http://") {
        (url.get(7..)?, false)
    } else {
        return None;
    };
    let (authority, path) = match rest.find(['/', '?']).and_then(|i| rest.split_at_checked(i)) {
        Some((authority, query)) if query.starts_with('?') => (authority, format!("/{query}")),
        Some((authority, path)) => (authority, path.to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    Some(Address { host, port, secure, path })
}

/// A server's address as kept: its origin and path, without a query or a slash at the end.
fn server_address(url: &str) -> Option<String> {
    let a = parse(url)?;
    if a.path.contains('?') {
        return None;
    }
    Some(format!("{}{}", a.origin(), a.path.trim_end_matches('/')))
}

/// ntfy: the topic is the path's last part; JSON is published to the part before it.
fn topic(path: &str) -> Option<(&str, &str)> {
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let (base, topic) = path.rsplit_once('/')?;
    (!topic.is_empty()).then_some((base, topic))
}

/// A topic: letters, digits, `-` and `_`, as ntfy allows.
fn topic_ok(topic: &str) -> bool {
    !topic.is_empty() && topic.len() <= 64 && topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A token: printable, no spaces (it goes into a header).
fn token_ok(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_graphic())
}

impl Config {
    pub fn service(&self) -> Service {
        match self {
            Config::Ntfy { .. } => Service::Ntfy,
            Config::Gotify { .. } => Service::Gotify,
            Config::Discord { .. } => Service::Discord,
            Config::Slack { .. } => Service::Slack,
            Config::Webhook { .. } => Service::Webhook,
        }
    }

    /// What the settings keep (JSON); see [`Config::load`].
    pub fn stored(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// What the settings kept: JSON, or an address saved by an earlier version.
    pub fn load(stored: &str) -> Option<Config> {
        serde_json::from_str(stored).ok().or_else(|| Config::from_address(stored))
    }

    /// The service and what it needs, from one address: an ntfy topic's
    /// (`https://ntfy.sh/topic`), Gotify's message address with its token in the
    /// query (`…/message?token=…`), a Discord or Slack webhook, or any other.
    pub fn from_address(url: &str) -> Option<Config> {
        let a = parse(url)?;
        let (path, query) = a.path.split_once('?').unwrap_or((&a.path, ""));
        let discord =
            matches!(a.host.as_str(), "discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com")
                && path.starts_with("/api/webhooks/");
        let gotify_token = query.split('&').find_map(|p| p.strip_prefix("token=")).filter(|t| token_ok(t));
        let gotify_base = path.trim_end_matches('/').strip_suffix("/message");
        Some(if discord {
            Config::Discord { address: url.trim().to_string() }
        } else if a.host == "hooks.slack.com" {
            Config::Slack { address: url.trim().to_string() }
        } else if let (Some(base), Some(token)) = (gotify_base, gotify_token) {
            Config::Gotify { server: format!("{}{base}", a.origin()), token: token.to_string() }
        } else if let Some((base, topic)) = topic(path).filter(|(_, t)| a.host.contains("ntfy") && topic_ok(t)) {
            Config::Ntfy { server: format!("{}{base}", a.origin()), topic: topic.to_string(), token: None }
        } else {
            Config::Webhook { address: url.trim().to_string() }
        })
    }

    /// The setup the form describes, or what's missing or wrong. A secret left
    /// empty keeps the one saved for the same service.
    pub fn build(service: Service, f: &Fields, saved: Option<&Config>) -> Result<Config, String> {
        let typed = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let address = |f: &Fields, saved: Option<&str>| {
            let address = typed(&f.address).or(saved.map(str::to_string)).ok_or(t("Enter the address"))?;
            parse(&address).map(|_| address).ok_or(t("That isn't an http:// or https:// address"))
        };
        let saved_address = match saved {
            Some(Config::Discord { address } | Config::Slack { address } | Config::Webhook { address })
                if saved.map(Config::service) == Some(service) =>
            {
                Some(address.as_str())
            }
            _ => None,
        };
        match service {
            Service::Ntfy => {
                let (saved_topic, saved_token) = match saved {
                    Some(Config::Ntfy { topic, token, .. }) => (Some(topic.clone()), token.clone()),
                    _ => (None, None),
                };
                let server = server_address(&typed(&f.server).unwrap_or_else(|| NTFY_SERVER.to_string()))
                    .ok_or(t("The server isn't an http:// or https:// address"))?;
                let topic = typed(&f.topic).or(saved_topic).ok_or(t("Enter the topic"))?;
                if !topic_ok(&topic) {
                    return Err(t("A topic has letters, digits, - and _ only"));
                }
                let token = typed(&f.token).or(saved_token);
                if token.as_deref().is_some_and(|t| !token_ok(t)) {
                    return Err(t("The token has a space or another character it can't have"));
                }
                Ok(Config::Ntfy { server, topic, token })
            }
            Service::Gotify => {
                let saved_token = match saved {
                    Some(Config::Gotify { token, .. }) => Some(token.clone()),
                    _ => None,
                };
                let server = typed(&f.server).ok_or(t("Enter the server's address"))?;
                let server = server_address(&server).ok_or(t("The server isn't an http:// or https:// address"))?;
                let token = typed(&f.token).or(saved_token).ok_or(t("Enter the app's token (Gotify → Apps)"))?;
                if !token_ok(&token) {
                    return Err(t("The token has a space or another character it can't have"));
                }
                Ok(Config::Gotify { server, token })
            }
            Service::Discord => Ok(Config::Discord { address: address(f, saved_address)? }),
            Service::Slack => Ok(Config::Slack { address: address(f, saved_address)? }),
            Service::Webhook => Ok(Config::Webhook { address: address(f, saved_address)? }),
        }
    }

    /// Where it goes, taken apart (always valid for a config that was built or loaded).
    fn address(&self) -> Option<Address> {
        match self {
            Config::Ntfy { server, .. } | Config::Gotify { server, .. } => parse(server),
            Config::Discord { address } | Config::Slack { address } | Config::Webhook { address } => parse(address),
        }
    }

    /// The server or webhook isn't encrypted (`http://`): only for a trusted local network.
    pub fn insecure(&self) -> bool {
        self.address().is_some_and(|a| !a.secure)
    }

    /// The server's address, for the form (not a secret: it doesn't say where to send).
    pub fn server(&self) -> Option<&str> {
        match self {
            Config::Ntfy { server, .. } | Config::Gotify { server, .. } => Some(server),
            _ => None,
        }
    }
}

fn t(s: &'static str) -> String {
    i18n::t(s).to_string()
}

/// What the settings show for a setup: the service and host, never a secret.
pub fn describe(c: &Config) -> String {
    let host = c.address().map(|a| a.host).unwrap_or_default();
    match c.service() {
        Service::Ntfy | Service::Gotify | Service::Webhook => format!("{} · {host}", c.service().name()),
        s => s.name().to_string(),
    }
}

/// One push: where it goes, its headers and JSON body.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub host: String,
    pub port: u16,
    pub secure: bool,
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

/// The request for a text.
pub fn request(c: &Config, text: &str) -> Option<Request> {
    let a = c.address()?;
    let base = a.path.trim_end_matches('/');
    let mut headers = vec![("Content-Type", "application/json".to_string())];
    let (path, body) = match c {
        Config::Ntfy { topic, token, .. } => {
            if let Some(token) = token {
                headers.push(("Authorization", format!("Bearer {token}")));
            }
            (format!("{base}/"), json!({ "topic": topic, "title": "Trayward", "message": text }))
        }
        Config::Gotify { token, .. } => {
            // In a header, not the address: addresses end up in servers' logs.
            headers.push(("X-Gotify-Key", token.clone()));
            // Priority 5: a notification on the phone (Gotify's default for apps is 0, silent on Android).
            (format!("{base}/message"), json!({ "title": "Trayward", "message": text, "priority": 5 }))
        }
        Config::Discord { .. } => (a.path.clone(), json!({ "username": "Trayward", "content": text })),
        Config::Slack { .. } => (a.path.clone(), json!({ "text": text })),
        Config::Webhook { .. } => {
            (a.path.clone(), json!({ "title": "Trayward", "message": text, "source": "trayward" }))
        }
    };
    Some(Request { host: a.host, port: a.port, secure: a.secure, path, headers, body: body.to_string() })
}

/// Sends a text (blocking: call it on a background thread).
pub fn send(c: &Config, text: &str) -> Result<(), String> {
    let r = request(c, text).ok_or_else(|| t("That isn't an http:// or https:// address"))?;
    if crate::demo::active() {
        return Err(t("The demo sends nothing"));
    }
    let headers: Vec<(&str, &str)> = r.headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let answer = crate::os::http::post((&r.host, r.port, r.secure), &r.path, &headers, r.body.as_bytes())?;
    match answer.status {
        200..300 => Ok(()),
        401 | 403 => Err(i18n::f("The server answered {0}: check the token", &[&answer.status])),
        status => Err(i18n::f("The server answered {0}", &[&status])),
    }
}

/// German translations of this file's text (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("The demo sends nothing", "Die Demo sendet nichts"),
    ("The server answered {0}", "Der Server antwortete {0}"),
    ("The server answered {0}: check the token", "Der Server antwortete {0}: prüfe das Token"),
    ("That isn't an http:// or https:// address", "Das ist keine http://- oder https://-Adresse"),
    ("Enter the address", "Gib die Adresse ein"),
    ("Enter the topic", "Gib das Thema ein"),
    ("A topic has letters, digits, - and _ only", "Ein Thema hat nur Buchstaben, Ziffern, - und _"),
    ("The token has a space or another character it can't have", "Das Token hat ein Leerzeichen oder ein anderes unzulässiges Zeichen"),
    ("Enter the server's address", "Gib die Adresse des Servers ein"),
    ("The server isn't an http:// or https:// address", "Der Server ist keine http://- oder https://-Adresse"),
    ("Enter the app's token (Gotify → Apps)", "Gib das Token der App ein (Gotify → Apps)"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(server: &str, topic: &str, token: &str, address: &str) -> Fields {
        Fields { server: server.into(), topic: topic.into(), token: token.into(), address: address.into() }
    }

    #[test]
    fn takes_an_address_apart() {
        assert_eq!(
            Config::from_address(" https://ntfy.sh/my-phone-7f3a ").unwrap(),
            Config::Ntfy { server: "https://ntfy.sh".into(), topic: "my-phone-7f3a".into(), token: None }
        );
        assert_eq!(
            Config::from_address("https://ntfy.example.org:8443/sub/alerts").unwrap(),
            Config::Ntfy { server: "https://ntfy.example.org:8443/sub".into(), topic: "alerts".into(), token: None }
        );
        // Gotify, on any host: /message with an application token (as earlier versions kept it).
        assert_eq!(
            Config::from_address("https://push.example.org/message?token=AbC123").unwrap(),
            Config::Gotify { server: "https://push.example.org".into(), token: "AbC123".into() }
        );
        assert_eq!(
            Config::from_address("http://192.168.1.20:8080/gotify/message?token=x").unwrap(),
            Config::Gotify { server: "http://192.168.1.20:8080/gotify".into(), token: "x".into() }
        );
        let discord = "https://discord.com/api/webhooks/123/abc?wait=true";
        assert_eq!(Config::from_address(discord).unwrap(), Config::Discord { address: discord.into() });
        let slack = "https://hooks.slack.com/services/T0/B0/xyz";
        assert_eq!(Config::from_address(slack).unwrap().service(), Service::Slack);
        // Without a token, /message is any webhook; so is an ntfy server without a topic.
        assert_eq!(Config::from_address("https://example.org/message").unwrap().service(), Service::Webhook);
        assert_eq!(Config::from_address("https://ntfy.sh/").unwrap().service(), Service::Webhook);
        for bad in
            ["ntfy.sh/x", "http://", "https://user:pw@example.com/", "https://my host.com/", "https://", "https://a:b/"]
        {
            assert!(Config::from_address(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn keeps_a_setup_and_reads_an_old_address() {
        let c = Config::Gotify { server: "https://push.example.org".into(), token: "AbC".into() };
        assert_eq!(c.stored(), r#"{"service":"gotify","server":"https://push.example.org","token":"AbC"}"#);
        assert_eq!(Config::load(&c.stored()), Some(c.clone()));
        // What an earlier version saved: the address.
        assert_eq!(Config::load("https://push.example.org/message?token=AbC"), Some(c));
        assert_eq!(Config::load("not an address"), None);
    }

    #[test]
    fn builds_a_setup_from_the_form() {
        // ntfy: the public server when none is given; a topic is needed.
        let c = Config::build(Service::Ntfy, &fields("", "my-phone", "", ""), None).unwrap();
        assert_eq!(c, Config::Ntfy { server: NTFY_SERVER.into(), topic: "my-phone".into(), token: None });
        assert!(Config::build(Service::Ntfy, &fields("", "", "", ""), None).is_err());
        assert!(Config::build(Service::Ntfy, &fields("", "my phone", "", ""), None).is_err());
        let c = Config::build(Service::Ntfy, &fields("https://ntfy.home.lan/", "alerts", "tk_1", ""), None).unwrap();
        assert_eq!(
            c,
            Config::Ntfy { server: "https://ntfy.home.lan".into(), topic: "alerts".into(), token: Some("tk_1".into()) }
        );
        // Left empty, the saved topic and token stay; the server can change.
        let again = Config::build(Service::Ntfy, &fields("https://ntfy.sh", "", "", ""), Some(&c)).unwrap();
        assert_eq!(
            again,
            Config::Ntfy { server: NTFY_SERVER.into(), topic: "alerts".into(), token: Some("tk_1".into()) }
        );
        // Gotify: the server and the app's token.
        assert!(Config::build(Service::Gotify, &fields("https://push.example.org", "", "", ""), None).is_err());
        assert!(Config::build(Service::Gotify, &fields("push.example.org", "", "AbC", ""), None).is_err());
        let g = Config::build(Service::Gotify, &fields("https://push.example.org/", "", "AbC", ""), None).unwrap();
        assert_eq!(g, Config::Gotify { server: "https://push.example.org".into(), token: "AbC".into() });
        // Another service's secrets aren't taken over.
        assert!(Config::build(Service::Gotify, &fields("https://push.example.org", "", "", ""), Some(&c)).is_err());
        // Webhooks: the address, kept when left empty.
        let d =
            Config::build(Service::Discord, &fields("", "", "", "https://discord.com/api/webhooks/1/a"), None).unwrap();
        assert_eq!(Config::build(Service::Discord, &Fields::default(), Some(&d)).unwrap(), d);
        assert!(Config::build(Service::Slack, &Fields::default(), Some(&d)).is_err());
        assert!(Config::build(Service::Webhook, &fields("", "", "", "ftp://x"), None).is_err());
    }

    #[test]
    fn shapes_the_request_per_service() {
        let body = |c: &Config| {
            serde_json::from_str::<serde_json::Value>(&request(c, "shop needs you").unwrap().body).unwrap()
        };
        let ntfy =
            Config::Ntfy { server: "https://ntfy.sh".into(), topic: "my-phone".into(), token: Some("tk_x".into()) };
        let r = request(&ntfy, "x").unwrap();
        assert_eq!((r.host.as_str(), r.port, r.path.as_str()), ("ntfy.sh", 443, "/"));
        assert!(r.headers.contains(&("Authorization", "Bearer tk_x".into())));
        assert_eq!(
            (body(&ntfy)["topic"].as_str(), body(&ntfy)["message"].as_str()),
            (Some("my-phone"), Some("shop needs you"))
        );
        // Gotify: the token in a header, not the address.
        let gotify = Config::Gotify { server: "http://192.168.1.20:8080/gotify".into(), token: "AbC".into() };
        let r = request(&gotify, "x").unwrap();
        assert_eq!((r.secure, r.port, r.path.as_str()), (false, 8080, "/gotify/message"));
        assert!(r.headers.contains(&("X-Gotify-Key", "AbC".into())) && !r.path.contains("AbC"));
        assert_eq!(body(&gotify)["priority"], 5);
        assert_eq!(
            body(&Config::Discord { address: "https://discord.com/api/webhooks/1/a".into() })["content"],
            "shop needs you"
        );
        assert_eq!(
            body(&Config::Slack { address: "https://hooks.slack.com/services/a".into() })["text"],
            "shop needs you"
        );
        assert_eq!(body(&Config::Webhook { address: "https://example.com/hook".into() })["message"], "shop needs you");
    }

    #[test]
    fn hides_the_secrets() {
        let shown = |url: &str| describe(&Config::from_address(url).unwrap());
        assert_eq!(shown("https://discord.com/api/webhooks/1/secret"), "Discord");
        assert_eq!(shown("https://ntfy.sh/my-topic"), "ntfy · ntfy.sh");
        assert_eq!(shown("https://example.com/secret"), "Webhook · example.com");
        assert_eq!(shown("https://push.example.org/message?token=secret"), "Gotify · push.example.org");
        assert!(Config::from_address("http://192.168.1.20/message?token=x").unwrap().insecure());
    }

    /// A real POST, to the one host Trayward talks to anyway: without a key
    /// it must come back 401, which shows the request and its body went out.
    #[test]
    #[ignore = "needs the network"]
    fn posts_for_real() {
        let r = crate::os::http::post(
            ("api.anthropic.com", 443, true),
            "/v1/messages",
            &[("Content-Type", "application/json")],
            b"{}",
        )
        .unwrap();
        assert_eq!(r.status, 401, "{}", String::from_utf8_lossy(&r.body));
    }
}
