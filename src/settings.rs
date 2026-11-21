//! User preferences in `HKCU\Software\Trayward`, and "Start with Windows".

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::{HSTRING, PCWSTR, w};

const KEY: PCWSTR = w!("Software\\Trayward");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Trayward");

/// Refresh interval choices: seconds and menu label.
pub const INTERVALS: [(u32, &str); 5] =
    [(60, "1 minute"), (120, "2 minutes"), (300, "5 minutes"), (600, "10 minutes"), (900, "15 minutes")];
pub const DEFAULT_INTERVAL: u32 = 120;

/// A setting the demo changed.
enum Stored {
    Dword(u32),
    Text(String),
}

/// The demo starts with the user's settings and keeps its changes here, in
/// memory: it changes nothing of the user's, not even "Start with Windows".
static DEMO: std::sync::Mutex<std::collections::BTreeMap<String, Stored>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// What the demo changed of a setting: None if it didn't (or it isn't the demo).
fn demo_value<T>(name: &str, get: impl FnOnce(&Stored) -> Option<T>) -> Option<Option<T>> {
    if !crate::demo::active() {
        return None;
    }
    let store = DEMO.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    store.get(name).map(get)
}

/// Keeps a change in the demo's memory; false if it isn't the demo.
fn demo_store(name: &str, value: Stored) -> bool {
    if crate::demo::active() {
        DEMO.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name.to_string(), value);
    }
    crate::demo::active()
}

/// A value that says something about the user (an address of theirs): the
/// demo has only what was set in it, never the real one (screenshots).
fn private_string(name: &str) -> Option<String> {
    if crate::demo::active() {
        return demo_value(name, |s| if let Stored::Text(t) = s { Some(t.clone()) } else { None })
            .flatten()
            .filter(|s| !s.is_empty());
    }
    read_string(name)
}

fn read_dword(name: &str) -> Option<u32> {
    if let Some(v) = demo_value(name, |s| if let Stored::Dword(v) = s { Some(*v) } else { None }) {
        return v;
    }
    let mut v = 0u32;
    let mut size = 4u32;
    // SAFETY: out-pointers to a u32 and its size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEY,
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(v)
}

fn write_dword(name: &str, v: u32) {
    if demo_store(name, Stored::Dword(v)) {
        return;
    }
    // SAFETY: pointer to a u32 of the stated size.
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            KEY,
            &HSTRING::from(name),
            REG_DWORD.0,
            Some(&v as *const u32 as *const _),
            4,
        );
    }
}

pub fn interval() -> u32 {
    read_dword("IntervalSeconds").filter(|v| INTERVALS.iter().any(|(s, _)| s == v)).unwrap_or(DEFAULT_INTERVAL)
}

pub fn set_interval(secs: u32) {
    write_dword("IntervalSeconds", secs);
}

pub fn autostart() -> bool {
    if let Some(on) = demo_value("Autostart", |s| if let Stored::Dword(v) = s { Some(*v == 1) } else { None }) {
        return on == Some(true);
    }
    let mut size = 0u32;
    // SAFETY: size-only query.
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)) == ERROR_SUCCESS
    }
}

/// "Start with Windows" names the exe by its path: after the exe was moved
/// (the file it names is gone), the entry is pointed at where it is now, as
/// the hooks are. Called at start. Another copy that's merely tried out (a
/// download, a build) doesn't take the entry over while the one in it exists.
pub fn repair_autostart() {
    if crate::demo::active() {
        return;
    }
    let Some(entry) = reg_string(RUN_KEY, &HSTRING::from("Trayward")) else { return };
    if !run_entry_exe(&entry).is_some_and(|exe| exe.is_file()) {
        let _ = set_autostart(true);
    }
}

/// The exe a Run entry starts: the quoted path, or the text up to its first space.
fn run_entry_exe(entry: &str) -> Option<std::path::PathBuf> {
    let entry = entry.trim();
    let path = match entry.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => entry.split(' ').next()?,
    };
    (!path.is_empty()).then(|| std::path::PathBuf::from(path))
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    if demo_store("Autostart", Stored::Dword(u32::from(on))) {
        return Ok(());
    }
    // SAFETY: the value buffer is a NUL-terminated UTF-16 string of the stated byte length.
    let r = unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let cmd: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE)
        }
    };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(windows::core::Error::from(r.to_hresult()).message()) }
}

/// When the widget shows the small usage bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bars {
    /// Only when there is room before the taskbar buttons.
    Auto,
    Always,
    Never,
}

pub fn bars() -> Bars {
    match read_dword("Bars") {
        Some(1) => Bars::Always,
        Some(2) => Bars::Never,
        _ => Bars::Auto,
    }
}

pub fn set_bars(b: Bars) {
    write_dword(
        "Bars",
        match b {
            Bars::Auto => 0,
            Bars::Always => 1,
            Bars::Never => 2,
        },
    );
}

/// Where the widget sits on a taskbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Left of the tray (or of the clock, on other displays' taskbars).
    Tray,
    /// Over Windows' Widgets button, wherever Windows puts it: Windows keeps
    /// the app buttons away from it. Next to the tray while there is none.
    Widgets,
}

pub fn place() -> Place {
    match read_dword("Place") {
        Some(1) => Place::Widgets,
        _ => Place::Tray,
    }
}

pub fn set_place(p: Place) {
    write_dword("Place", u32::from(p == Place::Widgets));
}

/// Which taskbars show the widget, with several displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taskbars {
    /// The main taskbar only.
    Main,
    /// Every taskbar, including those of displays connected later.
    All,
    /// The taskbars of the displays in [`chosen_displays`].
    Chosen,
}

pub fn taskbars() -> Taskbars {
    match read_dword("Taskbars") {
        Some(1) => Taskbars::All,
        Some(2) => Taskbars::Chosen,
        _ => Taskbars::Main,
    }
}

pub fn set_taskbars(t: Taskbars) {
    write_dword(
        "Taskbars",
        match t {
            Taskbars::Main => 0,
            Taskbars::All => 1,
            Taskbars::Chosen => 2,
        },
    );
}

/// Ids of the displays chosen for [`Taskbars::Chosen`] (see `taskbars::Display::id`).
pub fn chosen_displays() -> Vec<String> {
    read_string("TaskbarDisplays")
        .map(|s| s.split('|').filter(|id| !id.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn set_chosen_displays(ids: &[String]) {
    write_string("TaskbarDisplays", &ids.join("|"));
}

/// Whether the widget draws a solid background that covers taskbar buttons under it.
pub fn solid_background() -> bool {
    read_dword("SolidBackground") == Some(1)
}

pub fn set_solid_background(on: bool) {
    write_dword("SolidBackground", u32::from(on));
}

fn read_string(name: &str) -> Option<String> {
    if let Some(v) = demo_value(name, |s| if let Stored::Text(t) = s { Some(t.clone()) } else { None }) {
        return v.filter(|s| !s.is_empty());
    }
    reg_string(KEY, &HSTRING::from(name))
}

/// A string value of a key under HKEY_CURRENT_USER.
fn reg_string(key: PCWSTR, name: &HSTRING) -> Option<String> {
    let mut size = 0u32;
    // SAFETY: first call asks for the size, the second fills a buffer of that size.
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, key, name, RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        size = (buf.len() * 2) as u32;
        if RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(buf.get(..len).unwrap_or_default())).filter(|s| !s.is_empty())
    }
}

fn write_string(name: &str, value: &str) {
    if demo_store(name, Stored::Text(value.to_string())) {
        return;
    }
    let v: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    // SAFETY: NUL-terminated UTF-16 buffer of the stated byte length.
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            KEY,
            &HSTRING::from(name),
            REG_SZ.0,
            Some(v.as_ptr() as *const _),
            (v.len() * 2) as u32,
        );
    }
}

/// Encrypts with DPAPI for the current Windows account.
fn protect(data: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points into `data` for the call; the output buffer is
    // allocated by Windows, copied, and freed with LocalFree.
    unsafe {
        CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Some(v)
    }
}

/// Decrypts what `protect` encrypted.
fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Some(v)
    }
}

/// A secret kept encrypted for this Windows account (as hex text).
fn read_secret(name: &str) -> Option<String> {
    let hex = private_string(name)?;
    let bytes: Option<Vec<u8>> =
        (0..hex.len()).step_by(2).map(|i| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    String::from_utf8(unprotect(&bytes?)?).ok()
}

fn write_secret(name: &str, secret: Option<&str>) {
    let hex = secret
        .and_then(|p| protect(p.as_bytes()))
        .map(|b| {
            use std::fmt::Write;
            b.iter().fold(String::new(), |mut hex, x| {
                let _ = write!(hex, "{x:02x}");
                hex
            })
        })
        .unwrap_or_default();
    write_string(name, &hex);
}

/// The MQTT broker's password, kept encrypted for this Windows account.
pub fn mqtt_password() -> Option<String> {
    read_secret("MqttPassword")
}

/// Whether a broker password is saved (without decrypting it).
pub fn mqtt_password_set() -> bool {
    private_string("MqttPassword").is_some_and(|s| !s.is_empty())
}

pub fn set_mqtt_password(password: Option<&str>) {
    write_secret("MqttPassword", password);
}

/// Away mode's push setup, kept encrypted like the password: tokens and a
/// webhook's address are secrets. As JSON (`push::Config::stored`); an address
/// saved by an earlier version, encrypted or as plain text, is taken over.
fn push_config() -> Option<crate::push::Config> {
    let config =
        read_secret("PushAddress").or_else(|| private_string("PushUrl")).and_then(|s| crate::push::Config::load(&s))?;
    // Kept in today's form from now on.
    if read_secret("PushAddress").as_deref() != Some(config.stored().as_str()) && !crate::demo::active() {
        set_push_config(Some(&config));
    }
    Some(config)
}

fn set_push_config(config: Option<&crate::push::Config>) {
    let stored = config.map(crate::push::Config::stored);
    write_secret("PushAddress", stored.as_deref());
    // Should encrypting ever fail, the setup stays as plain text rather than being lost.
    let kept = stored.is_none() || read_secret("PushAddress") == stored;
    write_string("PushUrl", if kept { "" } else { stored.as_deref().unwrap_or_default() });
}

/// A sound choice: off, one of the built-in sounds (1 to 3), or a WAV file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundChoice {
    Off,
    Builtin(u8),
    File(String),
}

/// How loud the built-in sounds are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Volume {
    Quiet,
    Medium,
    Loud,
}

pub const VOLUMES: [(Volume, &str); 3] = [(Volume::Quiet, "Quiet"), (Volume::Medium, "Medium"), (Volume::Loud, "Loud")];

/// How often Claude Code's session lists are re-read, in seconds.
pub const SESSION_POLLS: [(u32, &str); 3] = [(2, "2 seconds"), (5, "5 seconds"), (10, "10 seconds")];

/// Preferences that switch features on and off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// Glow, shake and spinner.
    pub animations: bool,
    /// The slim widget: sessions on top, one limit at a time below.
    pub slim: bool,
    /// Seconds each limit is shown before the next one's turn (0: no turns):
    /// today's limit with the 5-hour one, in slim mode all of them.
    pub turn_secs: u8,
    /// How each day's share of the weekly limit is set.
    pub daily_budget: crate::forecast::Budget,
    /// How the flyout's weekly bar shows the days.
    pub week_bar: crate::flyout::WeekBar,
    /// Read transcripts for titles, tokens and cache timers.
    pub transcripts: bool,
    /// Look for sessions in running WSL distributions.
    pub wsl: bool,
    pub session_poll_secs: u32,
    pub needs_sound: SoundChoice,
    pub done_sound: SoundChoice,
    pub volume: Volume,
    /// Show a bubble when a session is done or needs you.
    pub bubbles: bool,
    /// All sounds off for now; the chosen sounds are kept.
    pub muted: bool,
    /// Leave out sessions archived in the Claude Code extension, unless they need the user.
    pub hide_archived: bool,
    /// Show how long the GPG key for signing commits stays unlocked.
    pub gpg_timer: bool,
    /// On plans without a 5-hour limit, simulate one (a day's share of the week).
    pub simulate_five_hour: bool,
    /// Warn when the weekly limit reaches this percentage (and 95 %); 0: never.
    pub warn_weekly: u8,
    /// Warn when the 5-hour limit reaches this percentage; 0: never.
    pub warn_five_hour: u8,
    /// Warn when the pace would use up the weekly limit before it resets.
    pub warn_pace: bool,
    /// From this percentage on, the widget counts down to the reset instead; 0: never.
    pub countdown_at: u8,
    /// Say so when a limit that was nearly used up resets.
    pub notify_free: bool,
    /// Show Fable's own limit (on plans that have one).
    pub fable: bool,
    /// Read Codex's limits and sessions (when Codex is used on this PC).
    pub codex: bool,
    /// Read Copilot's sessions (when Copilot's agent is used on this PC).
    pub copilot: bool,
    /// Ask GitHub for Copilot's monthly limit (via gh).
    pub copilot_limit: bool,
    /// Whose limits the taskbar shows.
    pub taskbar_shows: Shown,
    /// The language of the app.
    pub language: crate::i18n::Choice,
    /// Merge and pull requests and pipelines from GitLab and GitHub (via glab, gh).
    pub forge: bool,
    /// Bubbles when a pipeline ends or a merge or pull request is merged.
    pub forge_bubbles: bool,
    /// Merge and pull requests and pipelines in the flyout: under each session, and the user's own list.
    pub forge_flyout: bool,
    /// A bubble on Monday morning about last week.
    pub week_summary: bool,
    /// Away mode: notifications go to the phone too while the user is away.
    pub away: bool,
    /// Minutes without input that count as away (see [`AWAY_MINUTES`]).
    pub away_minutes: u32,
    /// Push "done" too, not only "needs you".
    pub away_done: bool,
    /// Where pushes go: the service and what it needs (see `push`).
    pub push: Option<crate::push::Config>,
    /// Once a day, ask GitHub for a newer release.
    pub update_check: bool,
    /// Keep sessions' statistics after Claude Code deletes their transcripts (`keep`).
    pub keep_stats: bool,
    /// Publish to an MQTT broker (`mqtt`).
    pub mqtt: bool,
    /// The broker, without its password (`mqtt://user@host:port`).
    pub mqtt_broker: Option<String>,
    /// The session mailbox (`mailbox.rs`, `docs/mailbox.md`): off by default.
    pub mailbox: bool,
    /// Its folder, if set; otherwise it's looked for.
    pub mailbox_dir: Option<String>,
    /// The user's own extensions (`extensions.rs`, `docs/extensions.md`): off by default.
    pub own_extensions: bool,
    /// Their ids switched on; a new one is off until switched on.
    pub extensions_on: Vec<String>,
    /// The system monitor in the widget (`sysmon`): which values, and graphs.
    pub monitor: Monitor,
}

/// The system monitor's switches: on or off as a whole, each value, how it
/// shows, and graphs behind them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The monitor as a whole; off keeps which values were chosen.
    pub on: bool,
    pub cpu: bool,
    pub ram: bool,
    pub gpu: bool,
    pub net: bool,
    pub disk: bool,
    pub graphs: bool,
    /// Memory in GB instead of percent.
    pub ram_gb: bool,
    /// The GPU's video memory in GB instead of how busy it is.
    pub gpu_memory: bool,
    /// The disks' read and write rates instead of how busy they are.
    pub disk_rates: bool,
}

impl Monitor {
    /// The values shown, as `sysmon` bits: none while the monitor is off.
    pub fn bits(&self) -> u8 {
        if self.on { self.chosen() } else { 0 }
    }

    /// The values chosen, as `sysmon` bits, whether the monitor is on or not.
    pub fn chosen(&self) -> u8 {
        use crate::sysmon::{CPU, DISK, GPU, NET, RAM};
        [(self.cpu, CPU), (self.ram, RAM), (self.gpu, GPU), (self.net, NET), (self.disk, DISK)]
            .iter()
            .filter(|(on, _)| *on)
            .fold(0, |bits, (_, bit)| bits | bit)
    }
}

/// The choices for how long without input counts as away (minutes).
pub const AWAY_MINUTES: [u32; 4] = [5, 10, 15, 30];

/// Whose limits the taskbar shows, with Codex on (`Both`: everyone's, Copilot's too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Both,
    Claude,
    Codex,
}

/// Choices for the countdown (0: off).
pub const COUNTDOWN_AT: [u8; 4] = [0, 90, 95, 100];

/// Choices for how long each limit is shown when they take turns (0: off).
pub const TURN_SECS: [u8; 5] = [0, 5, 10, 20, 30];

/// Choices for the weekly and 5-hour warnings (0: off).
pub const WARN_WEEKLY: [u8; 4] = [0, 70, 80, 90];
pub const WARN_FIVE_HOUR: [u8; 4] = [0, 75, 90, 95];

/// A stored warning level, if it's one of the choices.
fn warn_level(name: &str, choices: &[u8], default: u8) -> u8 {
    read_dword(name).and_then(|v| u8::try_from(v).ok()).filter(|v| choices.contains(v)).unwrap_or(default)
}

fn read_sound(name: &str, default: u8) -> SoundChoice {
    match read_dword(name).unwrap_or(u32::from(default)) {
        0 => SoundChoice::Off,
        n @ 1..=3 => SoundChoice::Builtin(n as u8),
        // The user's own file: not the demo's to show or play.
        _ => match private_string(&format!("{name}File")) {
            Some(path) => SoundChoice::File(path),
            None => SoundChoice::Builtin(default),
        },
    }
}

fn write_sound(name: &str, s: &SoundChoice) {
    match s {
        SoundChoice::Off => write_dword(name, 0),
        SoundChoice::Builtin(n) => write_dword(name, u32::from(*n)),
        SoundChoice::File(path) => {
            write_string(&format!("{name}File"), path);
            write_dword(name, 4);
        }
    }
}

impl Prefs {
    /// The preferences of a fresh install (what [`prefs`] reads with nothing stored).
    #[cfg(test)]
    pub fn defaults() -> Prefs {
        Prefs {
            animations: true,
            slim: false,
            turn_secs: 10,
            daily_budget: crate::forecast::Budget::Adaptive,
            week_bar: crate::flyout::WeekBar::Days,
            transcripts: true,
            wsl: true,
            session_poll_secs: 2,
            needs_sound: SoundChoice::Builtin(1),
            done_sound: SoundChoice::Builtin(1),
            volume: Volume::Medium,
            bubbles: true,
            muted: false,
            hide_archived: true,
            gpg_timer: false,
            simulate_five_hour: false,
            warn_weekly: 80,
            warn_five_hour: 90,
            warn_pace: true,
            countdown_at: 95,
            notify_free: true,
            fable: true,
            codex: true,
            copilot: true,
            copilot_limit: false,
            taskbar_shows: Shown::Both,
            language: crate::i18n::Choice::System,
            forge: false,
            forge_bubbles: true,
            forge_flyout: true,
            week_summary: true,
            away: false,
            away_minutes: 10,
            away_done: true,
            push: None,
            update_check: true,
            keep_stats: true,
            mqtt: false,
            mqtt_broker: None,
            mailbox: false,
            mailbox_dir: None,
            own_extensions: false,
            extensions_on: Vec::new(),
            monitor: Monitor {
                on: false,
                cpu: false,
                ram: false,
                gpu: false,
                net: false,
                disk: false,
                graphs: true,
                ram_gb: false,
                gpu_memory: false,
                disk_rates: false,
            },
        }
    }
}

pub fn prefs() -> Prefs {
    let on = |name: &str| read_dword(name) != Some(0);
    // The demo shows its own sessions only: no WSL, no GPG key of yours.
    let demo = crate::demo::active();
    Prefs {
        animations: on("Animations"),
        slim: read_dword("Slim") == Some(1),
        turn_secs: warn_level("TurnSeconds", &TURN_SECS, 10),
        daily_budget: match read_dword("DailyBudget") {
            Some(1) => crate::forecast::Budget::Even,
            _ => crate::forecast::Budget::Adaptive,
        },
        week_bar: match read_dword("WeekBar") {
            Some(1) => crate::flyout::WeekBar::Budgets,
            _ => crate::flyout::WeekBar::Days,
        },
        transcripts: demo || on("Transcripts"),
        wsl: !demo && on("WslSessions"),
        session_poll_secs: read_dword("SessionPollSeconds")
            .filter(|v| SESSION_POLLS.iter().any(|(s, _)| s == v))
            .unwrap_or(2),
        needs_sound: read_sound("NeedsSound", 1),
        done_sound: read_sound("DoneSound", 1),
        volume: match read_dword("SoundVolume") {
            Some(0) => Volume::Quiet,
            Some(2) => Volume::Loud,
            _ => Volume::Medium,
        },
        bubbles: on("Bubbles"),
        muted: read_dword("Muted") == Some(1),
        hide_archived: on("HideArchived"),
        // An extension: off until switched on.
        gpg_timer: !demo && read_dword("GpgTimer") == Some(1),
        simulate_five_hour: read_dword("SimulateFiveHour") == Some(1),
        warn_weekly: warn_level("WarnWeekly", &WARN_WEEKLY, 80),
        warn_five_hour: warn_level("WarnFiveHour", &WARN_FIVE_HOUR, 90),
        warn_pace: on("WarnPace"),
        countdown_at: warn_level("CountdownAt", &COUNTDOWN_AT, 95),
        notify_free: on("NotifyFree"),
        fable: on("FableLimit"),
        codex: on("Codex"),
        copilot: on("Copilot"),
        // Off unless switched on: it contacts GitHub.
        copilot_limit: read_dword("CopilotLimit") == Some(1),
        // Off unless switched on: it contacts GitLab and GitHub.
        forge: read_dword("Forge") == Some(1),
        forge_bubbles: on("ForgeBubbles"),
        // Never neither: merge requests would show nowhere while still asked for.
        forge_flyout: on("ForgeFlyout") || !on("ForgeBubbles"),
        week_summary: on("WeekSummary"),
        // Off unless switched on: it sends to the push service.
        away: read_dword("Away") == Some(1),
        away_minutes: read_dword("AwayMinutes").filter(|m| AWAY_MINUTES.contains(m)).unwrap_or(10),
        away_done: on("AwayDone"),
        push: push_config(),
        update_check: on("UpdateCheck"),
        keep_stats: on("KeepStats"),
        // Off unless switched on: it connects to the broker.
        mqtt: read_dword("Mqtt") == Some(1),
        mqtt_broker: private_string("MqttBroker"),
        // Off unless switched on: a use of the user's own.
        mailbox: read_dword("Mailbox") == Some(1),
        mailbox_dir: private_string("MailboxDir"),
        // Off unless switched on, and each extension too.
        own_extensions: read_dword("OwnExtensions") == Some(1),
        extensions_on: private_string("ExtensionsOn")
            .map(|s| s.split(',').filter(|id| !id.is_empty()).map(str::to_string).collect())
            .unwrap_or_default(),
        // Off unless switched on: an extra next to what Trayward is about.
        monitor: Monitor {
            // Before the switch existed, the values alone decided.
            on: read_dword("Monitor").map_or_else(
                || {
                    ["MonitorCpu", "MonitorRam", "MonitorGpu", "MonitorNet", "MonitorDisk"]
                        .iter()
                        .any(|v| read_dword(v) == Some(1))
                },
                |v| v == 1,
            ),
            cpu: read_dword("MonitorCpu") == Some(1),
            ram: read_dword("MonitorRam") == Some(1),
            gpu: read_dword("MonitorGpu") == Some(1),
            net: read_dword("MonitorNet") == Some(1),
            disk: read_dword("MonitorDisk") == Some(1),
            graphs: on("MonitorGraphs"),
            ram_gb: read_dword("MonitorRamGb") == Some(1),
            gpu_memory: read_dword("MonitorGpuMemory") == Some(1),
            disk_rates: read_dword("MonitorDiskRates") == Some(1),
        },
        language: match read_dword("Language") {
            Some(1) => crate::i18n::Choice::English,
            Some(2) => crate::i18n::Choice::German,
            _ => crate::i18n::Choice::System,
        },
        taskbar_shows: match read_dword("TaskbarShows") {
            Some(1) => Shown::Claude,
            Some(2) => Shown::Codex,
            _ => Shown::Both,
        },
    }
}

pub fn set_prefs(p: &Prefs) {
    write_dword("Animations", u32::from(p.animations));
    write_dword("Slim", u32::from(p.slim));
    write_dword("TurnSeconds", u32::from(p.turn_secs));
    write_dword("DailyBudget", u32::from(p.daily_budget == crate::forecast::Budget::Even));
    write_dword("WeekBar", u32::from(p.week_bar == crate::flyout::WeekBar::Budgets));
    write_dword("Transcripts", u32::from(p.transcripts));
    write_dword("WslSessions", u32::from(p.wsl));
    write_dword("SessionPollSeconds", p.session_poll_secs);
    write_sound("NeedsSound", &p.needs_sound);
    write_sound("DoneSound", &p.done_sound);
    write_dword("Bubbles", u32::from(p.bubbles));
    write_dword("Muted", u32::from(p.muted));
    write_dword("HideArchived", u32::from(p.hide_archived));
    write_dword("GpgTimer", u32::from(p.gpg_timer));
    write_dword("SimulateFiveHour", u32::from(p.simulate_five_hour));
    write_dword("WarnWeekly", u32::from(p.warn_weekly));
    write_dword("WarnFiveHour", u32::from(p.warn_five_hour));
    write_dword("WarnPace", u32::from(p.warn_pace));
    write_dword("CountdownAt", u32::from(p.countdown_at));
    write_dword("NotifyFree", u32::from(p.notify_free));
    write_dword("FableLimit", u32::from(p.fable));
    write_dword("Codex", u32::from(p.codex));
    write_dword("Copilot", u32::from(p.copilot));
    write_dword("CopilotLimit", u32::from(p.copilot_limit));
    write_dword("Forge", u32::from(p.forge));
    write_dword("ForgeBubbles", u32::from(p.forge_bubbles));
    write_dword("ForgeFlyout", u32::from(p.forge_flyout));
    write_dword("WeekSummary", u32::from(p.week_summary));
    write_dword("Away", u32::from(p.away));
    write_dword("AwayMinutes", p.away_minutes);
    write_dword("AwayDone", u32::from(p.away_done));
    set_push_config(p.push.as_ref());
    write_dword("UpdateCheck", u32::from(p.update_check));
    write_dword("KeepStats", u32::from(p.keep_stats));
    write_dword("Mqtt", u32::from(p.mqtt));
    let m = &p.monitor;
    for (name, on) in [
        ("Monitor", m.on),
        ("MonitorCpu", m.cpu),
        ("MonitorRam", m.ram),
        ("MonitorGpu", m.gpu),
        ("MonitorNet", m.net),
        ("MonitorDisk", m.disk),
        ("MonitorGraphs", m.graphs),
        ("MonitorRamGb", m.ram_gb),
        ("MonitorGpuMemory", m.gpu_memory),
        ("MonitorDiskRates", m.disk_rates),
    ] {
        write_dword(name, u32::from(on));
    }
    write_string("MqttBroker", p.mqtt_broker.as_deref().unwrap_or_default());
    write_dword("Mailbox", u32::from(p.mailbox));
    write_string("MailboxDir", p.mailbox_dir.as_deref().unwrap_or_default());
    write_dword("OwnExtensions", u32::from(p.own_extensions));
    // Ids have no commas (`extensions::parse`).
    write_string("ExtensionsOn", &p.extensions_on.join(","));
    write_dword(
        "Language",
        match p.language {
            crate::i18n::Choice::System => 0,
            crate::i18n::Choice::English => 1,
            crate::i18n::Choice::German => 2,
        },
    );
    write_dword(
        "TaskbarShows",
        match p.taskbar_shows {
            Shown::Both => 0,
            Shown::Claude => 1,
            Shown::Codex => 2,
        },
    );
    write_dword(
        "SoundVolume",
        match p.volume {
            Volume::Quiet => 0,
            Volume::Medium => 1,
            Volume::Loud => 2,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_exe_of_a_run_entry() {
        let exe = |e: &str| run_entry_exe(e).map(|p| p.display().to_string());
        assert_eq!(exe(r#""C:\Apps\Tray ward\trayward.exe""#).as_deref(), Some(r"C:\Apps\Tray ward\trayward.exe"));
        assert_eq!(exe(r#""C:\a.exe" --flag"#).as_deref(), Some(r"C:\a.exe"));
        assert_eq!(exe(r"C:\a.exe --flag").as_deref(), Some(r"C:\a.exe"));
        assert_eq!(exe("  "), None);
    }

    #[test]
    fn secrets_come_back_as_they_went_in() {
        let secret = "https://hooks.example.com/services/T0/B0/xyz ü";
        let kept = protect(secret.as_bytes()).unwrap();
        assert!(!kept.windows(8).any(|w| w == b"example."));
        assert_eq!(unprotect(&kept).unwrap(), secret.as_bytes());
        assert_eq!(unprotect(b"not encrypted"), None);
    }

    /// A value name nothing stores: reading it reads nothing of the user's settings.
    const NOT_STORED: &str = "TestValueNothingStores";

    /// Values that aren't stored read as their defaults (read only: tests never
    /// write the registry, and outside the demo nothing is kept in memory).
    #[test]
    fn what_is_not_stored_reads_as_its_default() {
        assert!(!crate::demo::active());
        assert_eq!(read_dword(NOT_STORED), None);
        assert_eq!(read_string(NOT_STORED), None);
        assert_eq!(private_string(NOT_STORED), None);
        assert_eq!(read_secret(NOT_STORED), None);
        assert_eq!(reg_string(KEY, &HSTRING::from(NOT_STORED)), None);
        assert_eq!(warn_level(NOT_STORED, &WARN_WEEKLY, 80), 80);
        assert_eq!(read_sound(NOT_STORED, 2), SoundChoice::Builtin(2));
        assert_eq!(read_sound(NOT_STORED, 0), SoundChoice::Off);
        // Outside the demo, nothing is kept in memory: a write would go to the registry.
        assert_eq!(demo_value(NOT_STORED, |_| Some(1)), None);
    }

    /// What's stored, whatever it is, reads as one of the choices.
    #[test]
    fn stored_settings_read_as_choices() {
        assert!(INTERVALS.iter().any(|(s, _)| *s == interval()));
        let _ = (bars(), place(), taskbars(), solid_background(), autostart());
        assert!(chosen_displays().iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn the_monitor_shows_what_is_chosen_while_on() {
        use crate::sysmon::{CPU, DISK, GPU, NET, RAM};
        let none = Prefs::defaults().monitor;
        assert_eq!((none.chosen(), none.bits()), (0, 0));
        let some = Monitor { cpu: true, net: true, ..none };
        assert_eq!((some.chosen(), some.bits()), (CPU | NET, 0));
        let all = Monitor { on: true, cpu: true, ram: true, gpu: true, net: true, disk: true, ..none };
        assert_eq!(all.bits(), CPU | RAM | GPU | NET | DISK);
    }

    /// A fresh install: everything that contacts anything is off.
    #[test]
    fn defaults_keep_to_themselves() {
        let p = Prefs::defaults();
        assert!(!p.forge && !p.away && !p.mqtt && !p.mailbox && !p.own_extensions && !p.copilot_limit);
        assert!(!p.monitor.on && p.push.is_none() && p.mqtt_broker.is_none());
        // Merge requests show somewhere when they're switched on.
        assert!(p.forge_flyout || p.forge_bubbles);
        assert!(WARN_WEEKLY.contains(&p.warn_weekly) && WARN_FIVE_HOUR.contains(&p.warn_five_hour));
        assert!(COUNTDOWN_AT.contains(&p.countdown_at) && TURN_SECS.contains(&p.turn_secs));
        assert!(AWAY_MINUTES.contains(&p.away_minutes));
        assert!(SESSION_POLLS.iter().any(|(s, _)| *s == p.session_poll_secs));
        assert!(INTERVALS.iter().any(|(s, _)| *s == DEFAULT_INTERVAL));
        assert_eq!(VOLUMES.map(|(v, _)| v), [Volume::Quiet, Volume::Medium, Volume::Loud]);
    }

    #[test]
    fn encrypts_for_this_account() {
        let secret = b"s3cr@t password";
        let sealed = protect(secret).unwrap();
        assert!(!sealed.windows(secret.len()).any(|w| w == secret));
        assert_eq!(unprotect(&sealed).unwrap(), secret);
        assert_eq!(unprotect(b"not sealed"), None);
    }
}
