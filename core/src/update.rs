//! The update check: once a day (Settings → About), the newest release of
//! Trayward on GitHub. It only reads: nothing is downloaded or installed.

use serde_json::Value;

/// The project on GitHub. The same as `repository` in the app's `Cargo.toml` (a
/// test of the app checks it): this crate's own `Cargo.toml` has neither that nor
/// the app's version, so neither is read from it.
pub const REPO_URL: &str = "https://github.com/TheDelta/trayward";

/// The licenses of the crates a version is built from: a file of its release
/// (`scripts/licenses.py` makes it, the Release workflow attaches it).
pub fn licenses_url(version: &str) -> String {
    format!("{REPO_URL}/releases/download/v{version}/THIRD-PARTY-LICENSES.txt")
}

/// A release newer than this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Its page on GitHub: what's new and the download.
    pub url: String,
}

/// `1.2.3` or `v1.2.3`; pre-releases (`1.2.3-rc.1`) don't count.
pub fn version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// The release in GitHub's answer for the latest release, if it's newer than
/// `current`. Drafts, pre-releases and links elsewhere are ignored.
pub fn newer(body: &str, current: &str) -> Option<Release> {
    let v: Value = serde_json::from_str(body).ok()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    let url = v["html_url"].as_str()?;
    if !url.starts_with(&format!("{REPO_URL}/")) {
        return None;
    }
    (version(tag)? > version(current)?)
        .then(|| Release { version: tag.trim_start_matches('v').to_string(), url: url.to_string() })
}

/// Asks GitHub for a release newer than `current`, the app's version (blocking:
/// call it on a background thread). `Ok(None)`: this is the newest, or nothing
/// has been released yet.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let repo = REPO_URL.strip_prefix("https://github.com/").ok_or("not a GitHub project")?;
    let r = crate::os::http::get(
        "api.github.com",
        &format!("/repos/{repo}/releases/latest"),
        &[("Accept", "application/vnd.github+json"), ("X-GitHub-Api-Version", "2022-11-28")],
    )?;
    match r.status {
        200 => Ok(newer(&String::from_utf8_lossy(&r.body), current)),
        // Nothing released yet; or the project moved (a redirect, which isn't
        // followed): nothing to tell rather than an error every day.
        404 | 301 | 302 | 307 | 308 => Ok(None),
        s => Err(crate::i18n::f("GitHub answered {0}", &[&s])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions() {
        assert_eq!(version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(version("12.3.45"), Some((12, 3, 45)));
        for bad in ["0.1", "1.2.3-rc.1", "v1.2.3.4", "latest", ""] {
            assert_eq!(version(bad), None, "{bad}");
        }
        assert!(version("0.10.0") > version("0.9.9"));
    }

    #[test]
    fn links_a_versions_licenses() {
        assert_eq!(
            licenses_url("1.2.3"),
            "https://github.com/TheDelta/trayward/releases/download/v1.2.3/THIRD-PARTY-LICENSES.txt"
        );
    }

    #[test]
    fn finds_a_newer_release() {
        let body = |tag: &str, extra: &str| {
            format!(r#"{{"tag_name":"{tag}","html_url":"{REPO_URL}/releases/tag/{tag}","draft":false{extra}}}"#)
        };
        let r = newer(&body("v0.2.0", ""), "0.1.0").unwrap();
        assert_eq!(r, Release { version: "0.2.0".into(), url: format!("{REPO_URL}/releases/tag/v0.2.0") });
        assert!(newer(&body("v0.1.0", ""), "0.1.0").is_none());
        assert!(newer(&body("v0.0.9", ""), "0.1.0").is_none());
        assert!(newer(&body("v0.2.0", r#","prerelease":true"#), "0.1.0").is_none());
        assert!(newer("not json", "0.1.0").is_none());
        // A link to somewhere else isn't opened.
        let elsewhere = r#"{"tag_name":"v9.0.0","html_url":"https://example.com/x"}"#;
        assert!(newer(elsewhere, "0.1.0").is_none());
    }
}
