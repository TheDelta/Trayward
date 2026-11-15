//! Away mode's pushes to the phone: a POST to an address the user pastes in
//! the settings (an ntfy topic, a Discord or Slack webhook, or any https URL
//! that takes JSON). Only a short text goes out: a session's name and state.

use crate::i18n;
use serde_json::json;

/// Which service an address belongs to; it decides the body's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// ntfy.sh, or a server with "ntfy" in its name: JSON publishing.
    Ntfy,
    Discord,
    Slack,
    /// Anything else: `{"title", "message", "source"}`.
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
    pub service: Service,
}

/// Reads a push address: https only, no user name or password in it.
pub fn parse(url: &str) -> Option<Target> {
    let url = url.trim();
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest[i..].starts_with('?') => (&rest[..i], format!("/{}", &rest[i..])),
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, 443),
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let service =
        if matches!(host.as_str(), "discord.com" | "discordapp.com" | "ptb.discord.com" | "canary.discord.com")
            && path.starts_with("/api/webhooks/")
        {
            Service::Discord
        } else if host == "hooks.slack.com" {
            Service::Slack
        } else if host.contains("ntfy") && topic(&path).is_some() {
            Service::Ntfy
        } else {
            Service::Json
        };
    Some(Target { host, port, path, service })
}

/// ntfy: the topic is the path's last part; JSON is published to the part before it.
fn topic(path: &str) -> Option<(&str, &str)> {
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let (base, topic) = path.rsplit_once('/')?;
    (!topic.is_empty()).then_some((base, topic))
}

/// What the settings show for an address: the service and host, never the
/// whole address (webhook addresses carry their secret).
pub fn describe(t: &Target) -> String {
    match t.service {
        Service::Ntfy => format!("ntfy · {}", t.host),
        Service::Discord => "Discord".into(),
        Service::Slack => "Slack".into(),
        Service::Json => i18n::f("JSON to {0}", &[&t.host]),
    }
}

/// The request for a text: path and JSON body.
pub fn request(t: &Target, text: &str) -> (String, String) {
    match t.service {
        Service::Ntfy => {
            let (base, topic) = topic(&t.path).unwrap_or(("", "trayward"));
            (format!("{base}/"), json!({ "topic": topic, "title": "Trayward", "message": text }).to_string())
        }
        Service::Discord => (t.path.clone(), json!({ "username": "Trayward", "content": text }).to_string()),
        Service::Slack => (t.path.clone(), json!({ "text": text }).to_string()),
        Service::Json => {
            (t.path.clone(), json!({ "title": "Trayward", "message": text, "source": "trayward" }).to_string())
        }
    }
}

/// Sends a text (blocking: call it on a background thread).
pub fn send(url: &str, text: &str) -> Result<(), String> {
    let t = parse(url).ok_or_else(|| i18n::t("Not an https:// address").to_string())?;
    if crate::demo::active() {
        return Err(i18n::t("The demo sends nothing").into());
    }
    let (path, body) = request(&t, text);
    let r = crate::http::post(&t.host, t.port, &path, &[("Content-Type", "application/json")], body.as_bytes())?;
    if (200..300).contains(&r.status) { Ok(()) } else { Err(i18n::f("The server answered {0}", &[&r.status])) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addresses() {
        let t = parse(" https://ntfy.sh/my-phone-7f3a ").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str(), t.service),
            ("ntfy.sh", 443, "/my-phone-7f3a", Service::Ntfy)
        );
        let t = parse("https://ntfy.example.org:8443/sub/alerts").unwrap();
        assert_eq!((t.port, t.service), (8443, Service::Ntfy));
        assert_eq!(request(&t, "x").0, "/sub/");
        let t = parse("https://discord.com/api/webhooks/123/abc?wait=true").unwrap();
        assert_eq!((t.service, t.path.as_str()), (Service::Discord, "/api/webhooks/123/abc?wait=true"));
        assert_eq!(parse("https://hooks.slack.com/services/T0/B0/xyz").unwrap().service, Service::Slack);
        assert_eq!(
            parse("HTTPS://Example.com").unwrap(),
            Target { host: "example.com".into(), port: 443, path: "/".into(), service: Service::Json }
        );
        assert_eq!(parse("https://example.com?key=1").unwrap().path, "/?key=1");
        // A server named ntfy without a topic is plain JSON.
        assert_eq!(parse("https://ntfy.sh/").unwrap().service, Service::Json);
        for bad in [
            "http://ntfy.sh/x",
            "ntfy.sh/x",
            "https://user:pw@example.com/",
            "https://my host.com/",
            "https://",
            "https://a:b/",
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn shapes_the_body_per_service() {
        let body = |url: &str| {
            serde_json::from_str::<serde_json::Value>(&request(&parse(url).unwrap(), "shop needs you").1).unwrap()
        };
        let ntfy = body("https://ntfy.sh/my-phone");
        assert_eq!((ntfy["topic"].as_str(), ntfy["message"].as_str()), (Some("my-phone"), Some("shop needs you")));
        assert_eq!(body("https://discord.com/api/webhooks/1/a")["content"], "shop needs you");
        assert_eq!(body("https://hooks.slack.com/services/a")["text"], "shop needs you");
        assert_eq!(body("https://example.com/hook")["message"], "shop needs you");
    }

    #[test]
    fn hides_the_secret_part() {
        assert_eq!(describe(&parse("https://discord.com/api/webhooks/1/secret").unwrap()), "Discord");
        assert_eq!(describe(&parse("https://ntfy.sh/my-topic").unwrap()), "ntfy · ntfy.sh");
        assert_eq!(describe(&parse("https://example.com/secret").unwrap()), "JSON to example.com");
    }

    /// A real POST, to the one host Trayward talks to anyway: without a key
    /// it must come back 401, which shows the request and its body went out.
    #[test]
    #[ignore = "needs the network"]
    fn posts_for_real() {
        let r =
            crate::http::post("api.anthropic.com", 443, "/v1/messages", &[("Content-Type", "application/json")], b"{}")
                .unwrap();
        assert_eq!(r.status, 401, "{}", String::from_utf8_lossy(&r.body));
    }
}
