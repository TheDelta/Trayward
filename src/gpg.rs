//! How long the GPG key that signs git commits stays unlocked.
//!
//! gpg-agent caches the passphrase: each use restarts a timer of
//! `default-cache-ttl` seconds, and after `max-cache-ttl` seconds from
//! unlocking it is forgotten regardless. The agent tells whether the key is
//! cached, but not how long is left, so the app estimates it from when it saw
//! the key get unlocked and when commits were last signed (see `app.rs`).
//!
//! Everything runs GnuPG's own tools (the ones git is set up to use) with
//! hidden windows, on the collector thread, each with a time limit. Trayward
//! only asks: it never signs, never starts the agent, never asks for the
//! passphrase.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// GnuPG's defaults, used when the agent doesn't report its settings.
const DEFAULT_CACHE_TTL: u64 = 600;
const MAX_CACHE_TTL: u64 = 7200;

/// How long one of GnuPG's tools may take: a hung agent mustn't stop the collector.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How much of the end of a reflog is read for the last signed commit.
const REFLOG_TAIL: u64 = 16 * 1024;

/// The signing setup, found once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// Folder with gpg.exe, gpg-connect-agent.exe and gpgconf.exe.
    bin: PathBuf,
    /// Keygrip of the key that signs.
    grip: String,
    /// Seconds a passphrase stays cached after its last use.
    pub default_ttl: u64,
    /// Seconds a passphrase stays cached at most.
    pub max_ttl: u64,
}

/// What gpg-agent says about the signing key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Agent {
    /// The agent's process id (a new one means it was restarted and forgot everything).
    pub pid: Option<u32>,
    pub cached: bool,
}

/// Runs a tool and returns its output, if it ended in time and succeeded.
/// One that takes too long is stopped.
fn run(exe: &Path, args: &[&str]) -> Option<String> {
    let out = crate::win::run(exe.as_os_str(), args, TIMEOUT).ok()?;
    (out.success && out.complete).then(|| out.stdout_text())
}

fn git_config(key: &str) -> Option<String> {
    let git = crate::win::find_program("git")?;
    run(&git, &["config", "--global", "--get", key]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Whether git signs commits with GPG.
pub fn signs() -> bool {
    git_config("commit.gpgsign").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        && git_config("gpg.format").is_none_or(|f| f == "openpgp")
}

/// Where GnuPG's tools are: next to git's `gpg.program`, or the usual install folders.
fn find_bin() -> Option<PathBuf> {
    let configured = git_config("gpg.program").map(PathBuf::from).and_then(|p| p.parent().map(Path::to_path_buf));
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    configured
        .into_iter()
        .chain(program_files.map(|p| p.join(r"GnuPG\bin")))
        .chain(program_files_x86.map(|p| p.join(r"GnuPG\bin")))
        .find(|dir| dir.join("gpg-connect-agent.exe").is_file())
}

/// The current value of a gpg-agent option from `gpgconf --list-options`
/// (field 10), else its default (field 8).
fn agent_option(listing: &str, name: &str) -> Option<u64> {
    let line = listing.lines().find(|l| l.split(':').next() == Some(name))?;
    let fields: Vec<&str> = line.split(':').collect();
    [fields.get(9), fields.get(7)].into_iter().flatten().find_map(|v| v.parse().ok())
}

/// Keygrip of the signing key in `gpg --with-colons --with-keygrip` output:
/// the first key or subkey whose own capabilities include signing.
fn signing_grip(listing: &str) -> Option<String> {
    let mut can_sign = false;
    for line in listing.lines() {
        let f: Vec<&str> = line.split(':').collect();
        match f.first() {
            Some(&"sec" | &"ssb" | &"pub" | &"sub") => can_sign = f.get(11).is_some_and(|caps| caps.contains('s')),
            Some(&"grp") if can_sign => return f.get(9).map(|g| g.to_string()).filter(|g| !g.is_empty()),
            _ => {}
        }
    }
    None
}

/// Finds the signing setup, if git signs commits with GPG.
pub fn detect() -> Option<Setup> {
    if !signs() {
        return None;
    }
    let bin = find_bin()?;
    // A configured key is looked up among the public keys, which needs no
    // agent; otherwise git signs with the first secret key.
    let listing = match git_config("user.signingkey") {
        // After "--": a value starting with "-" is a key, not an option.
        Some(k) => run(&bin.join("gpg.exe"), &["--list-keys", "--with-keygrip", "--with-colons", "--", &k])?,
        None => run(&bin.join("gpg.exe"), &["--list-secret-keys", "--with-keygrip", "--with-colons"])?,
    };
    let grip = signing_grip(&listing)?;
    let options = run(&bin.join("gpgconf.exe"), &["--list-options", "gpg-agent"]).unwrap_or_default();
    Some(Setup {
        default_ttl: agent_option(&options, "default-cache-ttl").unwrap_or(DEFAULT_CACHE_TTL),
        max_ttl: agent_option(&options, "max-cache-ttl").unwrap_or(MAX_CACHE_TTL),
        bin,
        grip,
    })
}

/// Reads `gpg-connect-agent "GETINFO pid" "KEYINFO <grip>"` output. Nothing
/// at all: no agent runs, so nothing is cached. None: the agent doesn't know the key.
fn agent_in(output: &str) -> Option<Agent> {
    if !output.lines().any(|l| l.starts_with("OK") || l.starts_with("ERR")) {
        return Some(Agent { pid: None, cached: false });
    }
    let pid = output.lines().find_map(|l| l.strip_prefix("D ")).and_then(|p| p.trim().parse().ok());
    let keyinfo = output.lines().find(|l| l.starts_with("S KEYINFO "))?;
    Some(Agent { pid, cached: keyinfo.split_whitespace().nth(6) == Some("1") })
}

/// What the agent says about the signing key now. It isn't started if it
/// isn't running.
pub fn agent(setup: &Setup) -> Option<Agent> {
    let keyinfo = format!("KEYINFO {}", setup.grip);
    let exe = setup.bin.join("gpg-connect-agent.exe");
    agent_in(&run(&exe, &["--no-autostart", "GETINFO pid", &keyinfo, "/bye"])?)
}

/// Whether a reflog message is for something git signs: a commit, a merge,
/// a cherry-pick or a rebased commit (not a checkout, reset or fast-forward).
fn signs_entry(msg: &str) -> bool {
    let kind = msg.split(':').next().unwrap_or_default();
    if msg.contains("Fast-forward") {
        return false;
    }
    kind.starts_with("commit")
        || kind.starts_with("cherry-pick")
        || kind.starts_with("merge")
        || (kind.contains("rebase")
            && ["(pick)", "(reword)", "(squash)", "(fixup)", "(edit)"].iter().any(|k| kind.contains(k)))
        || (kind.starts_with("pull") && msg.contains("Merge made"))
}

/// The time of the last signed entry in a reflog
/// (`<old> <new> <name> <<email>> <unix time> <zone>\t<message>`).
fn last_signed_in(reflog: &str) -> Option<i64> {
    reflog.lines().rev().find_map(|line| {
        let (head, msg) = line.split_once('\t')?;
        if !signs_entry(msg) {
            return None;
        }
        let mut words = head.rsplit(' ');
        words.next()?; // the time zone
        words.next()?.parse().ok()
    })
}

/// When git last signed something in the repository containing `dir`, from
/// its HEAD reflog (`.git/logs/HEAD`). Unix seconds.
pub fn last_commit(dir: &Path) -> Option<i64> {
    use std::io::{Seek, SeekFrom};
    let mut d = Some(dir);
    while let Some(p) = d {
        let git = p.join(".git");
        if git.exists() {
            // A worktree's .git is a file pointing at its git dir.
            let git_dir = if git.is_file() {
                let text = std::fs::read_to_string(&git).ok()?;
                // Relative to the folder it's in (submodules, some worktrees); an absolute one stays.
                p.join(text.trim().strip_prefix("gitdir:")?.trim())
            } else {
                git
            };
            let mut file = std::fs::File::open(git_dir.join("logs").join("HEAD")).ok()?;
            let len = file.metadata().ok()?.len();
            file.seek(SeekFrom::Start(len.saturating_sub(REFLOG_TAIL))).ok()?;
            let mut tail = Vec::new();
            file.read_to_end(&mut tail).ok()?;
            return last_signed_in(&String::from_utf8_lossy(&tail));
        }
        d = p.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_agent_options() {
        let listing = "default-cache-ttl:24:0:expire cached PINs after N seconds:3:3:N:600::1800\n\
                       max-cache-ttl:24:2:set maximum PIN cache lifetime to N seconds:3:3:N:7200::\n";
        assert_eq!(agent_option(listing, "default-cache-ttl"), Some(1800));
        assert_eq!(agent_option(listing, "max-cache-ttl"), Some(7200));
        assert_eq!(agent_option(listing, "nope"), None);
    }

    #[test]
    fn finds_the_signing_keygrip() {
        let listing = "sec:u:255:22:DCA0:1:::u:::scESC:::+:::ed25519:::0:\n\
                       fpr:::::::::50C6:\n\
                       grp:::::::::65015B:\n\
                       ssb:u:255:18:7764:1::::::e:::+:::cv25519::\n\
                       fpr:::::::::03D5:\n\
                       grp:::::::::3FD6CB:\n";
        assert_eq!(signing_grip(listing).as_deref(), Some("65015B"));
        // A signing subkey wins when the primary key can't sign.
        let sub = listing.replace("scESC", "cC").replace(":e:::+", ":s:::+");
        assert_eq!(signing_grip(&sub).as_deref(), Some("3FD6CB"));
        // Public key listings have the same shape.
        let public = listing.replace("sec:", "pub:").replace("ssb:", "sub:");
        assert_eq!(signing_grip(&public).as_deref(), Some("65015B"));
        assert_eq!(signing_grip(""), None);
    }

    #[test]
    fn reads_the_agent() {
        let cached = "D 93580\nOK\nS KEYINFO 65015B D - - 1 P - - -\nOK\n";
        assert_eq!(agent_in(cached), Some(Agent { pid: Some(93580), cached: true }));
        let locked = "D 93580\nOK\nS KEYINFO 65015B D - - - P - - -\nOK\n";
        assert_eq!(agent_in(locked), Some(Agent { pid: Some(93580), cached: false }));
        // No agent running (the notice goes to stderr): nothing is cached.
        assert_eq!(agent_in(""), Some(Agent { pid: None, cached: false }));
        // The agent doesn't know the key.
        assert_eq!(agent_in("D 93580\nOK\nERR 67108891 Not found <GPG Agent>\n"), None);
    }

    #[test]
    fn finds_the_last_signature_in_the_reflog() {
        let line = |time: i64, msg: &str| format!("aaa bbb Alex <a@example.com> {time} +0100\t{msg}\n");
        let log = [
            line(100, "commit (initial): first"),
            line(200, "commit: second"),
            line(300, "checkout: moving from main to fix"),
            line(400, "pull: Fast-forward"),
            line(500, "reset: moving to HEAD~1"),
        ]
        .concat();
        assert_eq!(last_signed_in(&log), Some(200));
        for (msg, signed) in [
            ("commit (amend): second", true),
            ("rebase (pick): third", true),
            ("rebase -i (reword): fourth", true),
            ("pull --rebase (pick): fifth", true),
            ("rebase (finish): returning to refs/heads/fix", false),
            ("rebase (start): checkout main", false),
            ("cherry-pick: sixth", true),
            ("merge fix: Merge made by the 'ort' strategy.", true),
            ("merge fix: Fast-forward", false),
            ("pull: Merge made by the 'ort' strategy.", true),
        ] {
            let expected = if signed { Some(600) } else { Some(200) };
            assert_eq!(last_signed_in(&format!("{log}{}", line(600, msg))), expected, "{msg}");
        }
        // A line cut off while written is skipped.
        assert_eq!(last_signed_in(&format!("{log}aaa bbb Alex <a@exa")), Some(200));
        assert_eq!(last_signed_in(""), None);
    }

    /// The real setup on this PC: the agent, the key's state and this repository's last signed commit.
    #[test]
    #[ignore = "needs GnuPG and git set up to sign"]
    fn reads_the_real_setup() {
        let setup = detect().expect("git signs with GPG");
        let started = std::time::Instant::now();
        let agent = agent(&setup);
        let signed = last_commit(Path::new(env!("CARGO_MANIFEST_DIR")));
        println!(
            "ttl {}/{} s, agent {agent:?} in {:?}, last signed {signed:?}",
            setup.default_ttl,
            setup.max_ttl,
            started.elapsed()
        );
        assert!(agent.is_some() && signed.is_some());
    }
}
