//! Reads the 5-hour and weekly limits from the endpoint Claude Code's `/usage`
//! command uses, authenticated with Claude Code's saved login.
//!
//! The login is only ever *read*. Claude Code renews it while it runs; if the
//! token has expired we report that instead of renewing it ourselves, so this
//! app can never interfere with Claude Code's own credentials.

use crate::os::{self, http};
use crate::timefmt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

const API_HOST: &str = "api.anthropic.com";
const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    /// Percent of the limit used, 0..=100.
    pub used: f64,
    /// When the window resets (unix seconds).
    pub resets_at: Option<i64>,
}

impl Limit {
    /// Used percent as of `now`: once the window has reset, nothing is used
    /// until Claude is used again (which also refreshes our numbers).
    pub fn used_at(&self, now: i64) -> f64 {
        match self.resets_at {
            Some(t) if t <= now => 0.0,
            _ => self.used.clamp(0.0, 100.0),
        }
    }
}

/// A model's own limit within the weekly one, such as Fable's ("up to 50 % of
/// your weekly limit"): its use as a percentage of its own cap.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelLimit {
    pub limit: Limit,
    /// The most of the weekly limit the model may use (percent), when the response says.
    pub share: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub five_hour: Option<Limit>,
    pub weekly: Option<Limit>,
    /// Fable's limit, on plans that have one.
    #[serde(default)]
    pub fable: Option<ModelLimit>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// Claude Code isn't used on this PC (no config folder): nothing to show, no error.
    NotUsed,
    /// No Claude Code login on this PC.
    NoLogin,
    /// The saved login has expired or was rejected.
    LoginExpired,
    /// Too many requests; how long the server asks to wait (seconds), if it said.
    RateLimited(Option<u32>),
    Network(String),
    Server(u32),
    BadResponse,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLogin => f.write_str(crate::i18n::t("No Claude Code login found. Run `claude` and sign in.")),
            Self::NotUsed => f.write_str(crate::i18n::t("Claude Code isn't used on this PC")),
            Self::LoginExpired => f.write_str(crate::i18n::t("Login expired. Using Claude Code renews it.")),
            Self::RateLimited(_) => f.write_str(crate::i18n::t("Rate limited, retrying later.")),
            Self::Network(e) => f.write_str(e),
            Self::Server(s) => f.write_str(&crate::i18n::f("Usage request failed (HTTP {0}).", &[s])),
            Self::BadResponse => f.write_str(crate::i18n::t("Unexpected response from the usage API.")),
        }
    }
}

/// Claude Code's config folder: `CLAUDE_CONFIG_DIR`, or `%USERPROFILE%\.claude`.
pub fn config_dir() -> PathBuf {
    if let Some(d) = crate::demo::dir() {
        return d.clone();
    }
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("USERPROFILE").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".claude")
}

/// Extracts a usable access token from the contents of `.credentials.json`.
pub(crate) fn access_token(credentials: &str, now_ms: i64) -> Result<String, FetchError> {
    let root: Value = serde_json::from_str(credentials).map_err(|_| FetchError::NoLogin)?;
    let oauth = &root["claudeAiOauth"];
    let token = oauth["accessToken"].as_str().filter(|t| !t.is_empty()).ok_or(FetchError::NoLogin)?;
    if oauth["expiresAt"].as_i64().is_some_and(|exp| exp <= now_ms) {
        return Err(FetchError::LoginExpired);
    }
    Ok(token.to_string())
}

/// `GET` an OAuth API path with the login stored in `dir`; returns the body.
fn api_get(dir: &Path, path: &str) -> Result<Vec<u8>, FetchError> {
    let creds = std::fs::read_to_string(dir.join(".credentials.json")).map_err(|_| FetchError::NoLogin)?;
    let token = access_token(&creds, os::now_ms())?;
    let auth = format!("Bearer {token}");
    let resp = http::get(
        API_HOST,
        path,
        &[("Authorization", &auth), ("anthropic-beta", "oauth-2025-04-20"), ("Accept", "application/json")],
    )
    .map_err(FetchError::Network)?;
    match resp.status {
        200 => Ok(resp.body),
        401 | 403 => Err(FetchError::LoginExpired),
        429 => Err(FetchError::RateLimited(resp.retry_after)),
        s => Err(FetchError::Server(s)),
    }
}

/// Fetches the current limits for the login stored in `dir`; `NotUsed` without
/// asking anyone when the folder doesn't exist (Claude Code isn't used here).
pub fn fetch(dir: &Path) -> Result<Usage, FetchError> {
    if !dir.is_dir() {
        return Err(FetchError::NotUsed);
    }
    parse_usage(&api_get(dir, USAGE_PATH)?)
}

/// Fetches the name of the account's plan, e.g. "Pro", "Max 20x" or "Ultra".
/// The login file has a plan too, but it is only updated when signing in.
pub fn fetch_plan(dir: &Path) -> Result<String, FetchError> {
    parse_plan(&api_get(dir, PROFILE_PATH)?).ok_or(FetchError::BadResponse)
}

pub(crate) fn parse_plan(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let org = &v["organization"];
    if let Some(name) = org["plan_display_name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
        return Some(name.to_string());
    }
    let tier = org["rate_limit_tier"].as_str().unwrap_or_default();
    let multiplier = ["20x", "5x"].into_iter().find(|m| tier.ends_with(m));
    let name = match org["organization_type"].as_str()? {
        "claude_max" => match multiplier {
            Some(m) => format!("Max {m}"),
            None => "Max".into(),
        },
        "claude_pro" => "Pro".into(),
        "claude_team" => "Team".into(),
        "claude_enterprise" => "Enterprise".into(),
        _ if v["account"]["has_claude_max"] == false && v["account"]["has_claude_pro"] == false => "Free".into(),
        _ => return None,
    };
    Some(name)
}

pub(crate) fn parse_usage(body: &[u8]) -> Result<Usage, FetchError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| FetchError::BadResponse)?;
    if !v.is_object() {
        return Err(FetchError::BadResponse);
    }
    let limit = |key: &str| v.get(key).and_then(limit_of);
    Ok(Usage { five_hour: limit("five_hour"), weekly: limit("seven_day"), fable: model_limit(&v, "fable") })
}

/// A limit object: `{"utilization": 17.0, "resets_at": "…"}`.
fn limit_of(o: &Value) -> Option<Limit> {
    Some(Limit {
        used: o.get("utilization")?.as_f64()?,
        resets_at: o.get("resets_at").and_then(Value::as_str).and_then(timefmt::parse_rfc3339),
    })
}

/// A model's own weekly limit: the entry of `limits` with the kind
/// `weekly_scoped` whose scope is that model (by its name or id), and its share
/// of the weekly limit from `weekly_scoped_shares` (by the entry's index):
///
/// ```json
/// "limits": [{"kind": "weekly_all", "percent": 31, …},
///            {"kind": "weekly_scoped", "percent": 4, "resets_at": "…",
///             "scope": {"model": {"id": null, "display_name": "Fable"}}}],
/// "weekly_scoped_shares": [{"limit_index": 1, "allowance_percent_of_weekly": 50}]
/// ```
fn model_limit(v: &Value, model: &str) -> Option<ModelLimit> {
    let names = |s: &str| s.to_ascii_lowercase().contains(model);
    let (index, entry) = v.get("limits")?.as_array()?.iter().enumerate().find(|(_, l)| {
        let of_model = &l["scope"]["model"];
        l["kind"].as_str() == Some("weekly_scoped")
            && ["display_name", "id"].iter().any(|f| of_model[*f].as_str().is_some_and(names))
    })?;
    let limit = Limit {
        used: entry.get("percent")?.as_f64()?,
        resets_at: entry.get("resets_at").and_then(Value::as_str).and_then(timefmt::parse_rfc3339),
    };
    let share = v
        .get("weekly_scoped_shares")
        .and_then(Value::as_array)
        .and_then(|shares| shares.iter().find(|s| s["limit_index"].as_u64() == Some(index as u64)))
        .and_then(|s| s["allowance_percent_of_weekly"].as_f64());
    Some(ModelLimit { limit, share })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usage_response() {
        let body = br#"{
            "five_hour": {"utilization": 17.0, "resets_at": "2026-11-11T14:10:00+00:00"},
            "seven_day": {"utilization": 42.5, "resets_at": null},
            "seven_day_opus": null
        }"#;
        let u = parse_usage(body).unwrap();
        assert_eq!(u.five_hour, Some(Limit { used: 17.0, resets_at: Some(1_794_406_200) }));
        assert_eq!(u.weekly, Some(Limit { used: 42.5, resets_at: None }));
    }

    #[test]
    fn asks_nobody_without_claude_code() {
        let missing = std::env::temp_dir().join("trayward-test-no-claude-here");
        assert_eq!(fetch(&missing), Err(FetchError::NotUsed));
    }

    #[test]
    fn reads_fables_own_limit() {
        // As the endpoint answered on 2026-11-16 (other fields left out).
        let body = br#"{
            "five_hour": null,
            "seven_day": {"utilization": 31.0, "resets_at": "2026-11-21T00:59:59.505446+00:00"},
            "limits": [
                {"kind": "weekly_all", "group": "weekly", "percent": 31, "severity": "normal",
                 "resets_at": "2026-11-21T00:59:59.505446+00:00", "scope": null, "is_active": true},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 4, "severity": "normal",
                 "resets_at": "2026-11-21T00:59:59.505628+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": false}
            ],
            "weekly_scoped_shares": [{"limit_index": 1, "used_percent_of_weekly": 2, "allowance_percent_of_weekly": 50}]
        }"#;
        let u = parse_usage(body).unwrap();
        let fable = u.fable.unwrap();
        assert_eq!(fable.limit.used, 4.0);
        assert_eq!(fable.limit.resets_at, timefmt::parse_rfc3339("2026-11-21T00:59:59Z"));
        assert_eq!(fable.share, Some(50.0));
        assert_eq!(u.weekly.unwrap().used, 31.0);
        // Without the share, still the limit; without a scoped limit for Fable, none.
        let no_share = String::from_utf8_lossy(body).replace("\"limit_index\": 1", "\"limit_index\": 7");
        assert_eq!(parse_usage(no_share.as_bytes()).unwrap().fable.unwrap().share, None);
        let other = String::from_utf8_lossy(body).replace("Fable", "Opus");
        assert_eq!(parse_usage(other.as_bytes()).unwrap().fable, None);
        assert_eq!(parse_usage(br#"{"seven_day": null}"#).unwrap().fable, None);
    }

    #[test]
    fn parses_plan() {
        let named = br#"{"organization":{"organization_type":"claude_max","plan_display_name":"Ultra"}}"#;
        assert_eq!(parse_plan(named).as_deref(), Some("Ultra"));
        let max = br#"{"organization":{"organization_type":"claude_max","rate_limit_tier":"default_claude_max_5x"}}"#;
        assert_eq!(parse_plan(max).as_deref(), Some("Max 5x"));
        let pro = br#"{"organization":{"organization_type":"claude_pro","plan_display_name":" "}}"#;
        assert_eq!(parse_plan(pro).as_deref(), Some("Pro"));
        let free =
            br#"{"account":{"has_claude_max":false,"has_claude_pro":false},"organization":{"organization_type":"x"}}"#;
        assert_eq!(parse_plan(free).as_deref(), Some("Free"));
        assert_eq!(parse_plan(b"{}"), None);
    }

    #[test]
    fn missing_limits_are_none() {
        let u = parse_usage(br#"{"five_hour": null}"#).unwrap();
        assert_eq!(u, Usage::default());
        assert_eq!(parse_usage(b"[]"), Err(FetchError::BadResponse));
        assert_eq!(parse_usage(b"nope"), Err(FetchError::BadResponse));
    }

    #[test]
    fn reset_window_reads_as_unused() {
        let l = Limit { used: 80.0, resets_at: Some(1000) };
        assert_eq!(l.used_at(999), 80.0);
        assert_eq!(l.used_at(1000), 0.0);
        assert_eq!(Limit { used: 140.0, resets_at: None }.used_at(0), 100.0);
    }

    #[test]
    fn reads_token_and_expiry() {
        let ok = r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":2000}}"#;
        assert_eq!(access_token(ok, 1000), Ok("abc".into()));
        assert_eq!(access_token(ok, 2000), Err(FetchError::LoginExpired));
        assert_eq!(access_token(r#"{"claudeAiOauth":{}}"#, 0), Err(FetchError::NoLogin));
        assert_eq!(access_token("garbage", 0), Err(FetchError::NoLogin));
        let no_expiry = r#"{"claudeAiOauth":{"accessToken":"abc"}}"#;
        assert_eq!(access_token(no_expiry, 0), Ok("abc".into()));
    }

    #[test]
    fn asks_nobody_without_a_usable_login() {
        let dir = std::env::temp_dir().join(format!("trayward-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A config folder without a login, then with one that has expired: no request is made.
        assert_eq!(fetch(&dir), Err(FetchError::NoLogin));
        assert_eq!(fetch_plan(&dir), Err(FetchError::NoLogin));
        let expired = r#"{"claudeAiOauth":{"accessToken":"made-up","expiresAt":1000}}"#;
        std::fs::write(dir.join(".credentials.json"), expired).unwrap();
        assert_eq!(fetch(&dir), Err(FetchError::LoginExpired));
        assert_eq!(fetch_plan(&dir), Err(FetchError::LoginExpired));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn says_what_went_wrong() {
        crate::i18n::set(crate::i18n::Choice::English);
        let said = |e: FetchError| e.to_string();
        assert!(said(FetchError::NoLogin).contains("sign in"));
        assert!(said(FetchError::NotUsed).contains("isn't used"));
        assert!(said(FetchError::LoginExpired).starts_with("Login expired"));
        assert!(said(FetchError::RateLimited(Some(30))).starts_with("Rate limited"));
        assert_eq!(said(FetchError::Network("offline".into())), "offline");
        assert_eq!(said(FetchError::Server(503)), "Usage request failed (HTTP 503).");
        assert!(said(FetchError::BadResponse).contains("usage API"));
        crate::i18n::set(crate::i18n::Choice::German);
        assert_eq!(said(FetchError::Server(503)), "Abfrage der Nutzung fehlgeschlagen (HTTP 503).");
    }

    #[test]
    fn finds_claude_codes_folder() {
        // Only where it would be: nothing in it is read.
        let dir = config_dir();
        let set = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty());
        assert!(set.is_some_and(|d| dir == Path::new(&d)) || dir.ends_with(".claude"));
    }

    #[test]
    fn names_more_plans() {
        let plan = |org: &str| parse_plan(format!(r#"{{"organization":{{"organization_type":"{org}"}}}}"#).as_bytes());
        assert_eq!(plan("claude_max").as_deref(), Some("Max"));
        assert_eq!(plan("claude_team").as_deref(), Some("Team"));
        assert_eq!(plan("claude_enterprise").as_deref(), Some("Enterprise"));
        // Something new, and the account doesn't say it's free: no name rather than a wrong one.
        assert_eq!(plan("claude_new"), None);
        let max20 =
            br#"{"organization":{"organization_type":"claude_max","rate_limit_tier":"default_claude_max_20x"}}"#;
        assert_eq!(parse_plan(max20).as_deref(), Some("Max 20x"));
        assert_eq!(parse_plan(b"not json"), None);
        // A scoped limit without its percentage isn't one.
        let body = br#"{"limits": [{"kind": "weekly_scoped", "scope": {"model": {"id": "fable-1"}}}]}"#;
        assert_eq!(parse_usage(body).unwrap().fable, None);
        let by_id = br#"{"limits": [{"kind": "weekly_scoped", "percent": 9, "scope": {"model": {"id": "fable-1"}}}]}"#;
        assert_eq!(parse_usage(by_id).unwrap().fable.map(|f| f.limit.used), Some(9.0));
    }
}
