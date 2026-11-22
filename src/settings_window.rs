//! The settings window, in the style of Windows 11 Settings: pages on the
//! left, each setting a card on the right with a switch or a set of choices.

use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::i18n::{f, t, tr};
use crate::settings::{self, Bars, Prefs, SoundChoice, Taskbars, Volume};
use crate::sound::{self, Kind};
use crate::theme::{self, Color, Palette};
use crate::tracking;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, IsWindowVisible, SW_HIDE, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CAPTION, WS_CLIPCHILDREN,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{BOOL, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Settings");

const WIDTH: f32 = 700.0;
const HEIGHT: f32 = 500.0;
const NAV_W: f32 = 190.0;
const PAD: f32 = 24.0;
const CARD_H: f32 = 66.0;
const SOUND_CARD_H: f32 = 108.0;
/// A card with its buttons under the description.
const BUTTONS_CARD_H: f32 = 100.0;
const GAP: f32 = 6.0;
const DISPLAY_CARD_H: f32 = 52.0;
/// Cards on the page with the most of them (Limits): eight. 56 with seven (the
/// "Weekly bar" card is there to try its two styles).
const COMPACT_CARD_H: f32 = 49.0;
/// The Providers page's cards: up to seven.
const PROVIDERS_CARD_H: f32 = 56.0;
/// The Taskbar page's cards: six of them, and the displays below.
const TASKBAR_CARD_H: f32 = 48.0;

// Clip children: the text fields are child windows over what's drawn.
const STYLE: WINDOW_STYLE =
    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_CLIPCHILDREN.0);
/// A text field's box.
const FIELD_H: f32 = 30.0;
/// A field's label above its box, and the gap between them.
const FIELD_LABEL_H: f32 = 17.0;
/// The broker's card while it's edited.
const BROKER_FORM_H: f32 = 252.0;
/// The push service's card while it's edited.
const PUSH_FORM_H: f32 = 198.0;

/// Pages: name and Segoe Fluent Icons glyph.
pub const PAGES: [(&str, char); 14] = [
    ("General", '\u{E713}'),
    ("Taskbar", '\u{E770}'),
    ("Sessions", '\u{E8F2}'),
    ("Notifications", '\u{EA8F}'),
    ("Limits", '\u{E7BA}'),
    ("Providers", '\u{E774}'),
    ("Extensions", '\u{EA86}'),
    ("System monitor", '\u{E9D9}'),
    ("GitLab and GitHub", '\u{E71B}'),
    ("Away", '\u{E8EA}'),
    ("MQTT", '\u{E80F}'),
    ("Mailbox", '\u{E715}'),
    ("Custom extensions", '\u{E943}'),
    ("About", '\u{E946}'),
];

/// The pages by name: indexes into [`PAGES`].
pub mod page {
    pub const GENERAL: usize = 0;
    pub const TASKBAR: usize = 1;
    pub const SESSIONS: usize = 2;
    pub const NOTIFICATIONS: usize = 3;
    pub const LIMITS: usize = 4;
    pub const PROVIDERS: usize = 5;
    pub const EXTENSIONS: usize = 6;
    pub const SYSTEM_MONITOR: usize = 7;
    pub const FORGE: usize = 8;
    pub const AWAY: usize = 9;
    pub const MQTT: usize = 10;
    pub const MAILBOX: usize = 11;
    pub const OWN_EXTENSIONS: usize = 12;
    pub const ABOUT: usize = 13;
}

/// Whether the sidebar lists `page`: an extension's page only while it's on
/// (switched on on the Extensions page), to keep the settings short.
pub fn page_shown(page: usize, p: &Prefs) -> bool {
    match page {
        page::SYSTEM_MONITOR => p.monitor.on,
        page::FORGE => p.forge,
        page::AWAY => p.away,
        page::MQTT => p.mqtt,
        page::MAILBOX => p.mailbox,
        page::OWN_EXTENSIONS => p.own_extensions,
        _ => page < PAGES.len(),
    }
}

/// Who made Trayward, and where.
const AUTHOR: &str = "Alex 'Delta' Diller";
const AUTHOR_URL: &str = "https://github.com/TheDelta";

const PLAY: char = '\u{E768}';
/// The "?" after a card's title that leads to its help.
const HELP: char = '\u{E9CE}';
/// How wide the help's hover box is.
const HELP_TIP_W: f32 = 300.0;
const FOLDER: char = '\u{E8B7}';
/// The icons of the Edit and Paste buttons.
const PENCIL: char = '\u{E70F}';
const CLIPBOARD: char = '\u{E77F}';
/// Segoe MDL2's waste bin, on Remove.
const BIN: char = '\u{E74D}';
/// The arrow of a drop-down list.
const CHEVRON: char = '\u{E70D}';
/// The space between buttons side by side.
const BUTTON_SPACE: f32 = 8.0;

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub prefs: Prefs,
    pub interval: u32,
    pub bars: Bars,
    pub solid: bool,
    pub taskbars: Taskbars,
    pub chosen: Vec<String>,
    /// Displays with a taskbar: id, name and where it is.
    pub displays: Vec<(String, String, String)>,
    /// More monitors than taskbars: Windows shows the taskbar on the main display only.
    pub more_monitors: bool,
    /// Where the widget sits.
    pub place: settings::Place,
    /// Windows shows its Widgets button (UI Automation found it).
    pub widgets_button: bool,
    /// What the widget left out to fit onto the Widgets button, if anything.
    pub fit: Option<crate::widget::Fit>,
    /// The folder trayward.exe is in (the demo shows a made-up one: nothing of the user's).
    pub exe_dir: String,
    /// The plan has a limit of its own for Fable (the usage endpoint reports one).
    pub fable_found: bool,
    /// Claude Code is used on this PC (its folder exists).
    pub claude_found: bool,
    /// Codex is used on this PC (its folder exists).
    pub codex_found: bool,
    /// Copilot's agent is used on this PC (its folder exists).
    pub copilot_found: bool,
    /// Why Copilot's limit couldn't be read, if so.
    pub copilot_error: Option<String>,
    /// How `glab` and `gh` stand: found, and (while merge and pull requests are on) logged in.
    pub forge_tools: crate::forge::Tools,
    /// What happened to a token just pasted or removed: the host, and what to say.
    pub forge_note: Option<(String, Status)>,
    /// How the GPG key timer's setup stands (none until it was looked at).
    pub gpg_check: Option<crate::gpg::Check>,
    pub tracking: tracking::State,
    pub autostart: bool,
    /// How the last test push went, or why a paste didn't take.
    pub push_status: Option<Status>,
    /// The broker and whether it's connected, or why a paste didn't take.
    pub mqtt_line: Option<Status>,
    /// A broker password is saved.
    pub mqtt_password_set: bool,
    /// A newer release, if there is one.
    pub update: Option<crate::update::Release>,
    /// What the update check says: "Up to date", "Checking…", an error.
    pub update_status: Status,
    /// How the mailbox's folder stands.
    pub mailbox_folder: MailboxFolder,
    /// Why a pasted mailbox folder wasn't taken, if so.
    pub mailbox_note: Option<String>,
    /// The mailbox's inboxes.
    pub mailbox_inboxes: Vec<InboxRow>,
    /// The user's own extensions' files.
    pub extensions: crate::extensions::Snapshot,
}

/// How the mailbox's folder stands, for its card.
#[derive(Clone, Debug, PartialEq)]
pub enum MailboxFolder {
    /// Being looked for; the spinner's angle.
    Searching(u16),
    /// In use: set by hand, or found.
    Found { path: String, set: bool },
    /// Not there: the folder set by hand, or none was found.
    Missing(Option<String>),
}

/// One inbox of the mailbox, as the settings list it.
#[derive(Clone, Debug, PartialEq)]
pub struct InboxRow {
    pub name: String,
    pub unread: usize,
    /// How long the oldest unread message has waited, e.g. "14h".
    pub waiting: Option<String>,
    pub done: usize,
    /// How long ago the last one was handled, e.g. "2h".
    pub last_done: Option<String>,
    /// A session runs in the project folder of this name.
    pub session: bool,
}

/// How a status line stands: its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Nothing to say about it (grey, like any description).
    Plain,
    /// Under way: connecting, sending, checking (blue).
    Busy,
    /// Worth knowing, nothing wrong: what a choice leaves out (blue, like Busy).
    Info,
    /// Works (green).
    Good,
    /// Needs a look (yellow).
    Warn,
    /// Failed (red).
    Bad,
}

/// A status line in a card's description, in its tone's colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub tone: Tone,
    pub text: String,
}

impl Status {
    pub fn new(tone: Tone, text: impl Into<String>) -> Status {
        Status { tone, text: text.into() }
    }

    pub fn bad(text: impl Into<String>) -> Status {
        Status::new(Tone::Bad, text)
    }
}

/// What didn't fit onto the Widgets button, in blue (nothing's wrong): its width, what all
/// of it would need, what's left out (or that the widget went slim), and where all of it shows.
fn fit_status(fit: &crate::widget::Fit) -> Status {
    if fit.slim {
        let text = f(
            "Room for {0} of {1} px on the Widgets button: slim instead · Tray shows all of it",
            &[&fit.room, &fit.full],
        );
        return Status::new(Tone::Info, text);
    }
    let mut out: Vec<&str> = Vec::new();
    if fit.bars {
        out.push(t("usage bars"));
    }
    for label in &fit.values {
        let name = match *label {
            "↑" | "↓" => t("network"),
            "Disk" | "R" | "W" => t("disk"),
            other => other,
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    let text = f(
        "Room for {0} of {1} px on the Widgets button: without {2} · Tray shows all of it",
        &[&fit.room, &fit.full, &out.join(", ")],
    );
    Status::new(Tone::Info, text)
}

/// What the Position card says about the Widgets button's room, when something didn't fit.
fn position_fit(m: &Model) -> Option<Status> {
    m.fit.as_ref().filter(|_| m.place == settings::Place::Widgets).map(fit_status)
}

/// The same on the system monitor's card, where its values are switched: when some of
/// them were left out, or all of it for the slim widget.
fn monitor_fit(m: &Model) -> Option<Status> {
    let on_button = m.prefs.monitor.on && m.place == settings::Place::Widgets;
    m.fit.as_ref().filter(|fit| on_button && (fit.slim || !fit.values.is_empty())).map(fit_status)
}

/// How `glab` and `gh` stand, for the card of merge and pull requests. Switched off,
/// neither is run: only whether each is found. Switched on: for each, the hosts
/// it's logged in to (the ones the sessions' projects are on, and github.com), or
/// what's missing. Yellow when a tool that's needed isn't there or isn't logged in.
fn forge_status(on: bool, tools: &crate::forge::Tools) -> Status {
    if on && !tools.asked {
        // (In English: the card translates what it's given.)
        return Status::new(Tone::Busy, "Checking glab and gh…");
    }
    // What to say of a tool, whether something is wrong with it, whether a login works.
    let of = |name: &str, tool: &crate::forge::Tool| {
        let hosts = |ok: bool| {
            let hosts: Vec<&str> = tool.hosts.iter().filter(|h| h.ok == ok).map(|h| h.name.as_str()).collect();
            hosts.join(", ")
        };
        let (logged_in, not) = (hosts(true), hosts(false));
        if !tool.found {
            (f("{0} not found", &[&name]), !not.is_empty(), false)
        } else if !not.is_empty() {
            (f("{0}: not logged in to {1}", &[&name, &not]), true, false)
        } else if !logged_in.is_empty() {
            (f("✓ {0}: {1}", &[&name, &logged_in]), false, true)
        } else {
            (f("{0} found", &[&name]), false, false)
        }
    };
    let (glab, gh) = (of("glab", &tools.glab), of("gh", &tools.gh));
    let text = format!("{} · {}", glab.0, gh.0);
    match (on, glab.1 || gh.1, glab.2 || gh.2) {
        (false, _, _) => Status::new(Tone::Plain, format!("{} · {text}", t("GitLab and GitHub"))),
        (true, true, _) => Status::new(Tone::Warn, text),
        (true, false, true) => Status::new(Tone::Good, text),
        (true, false, false) => Status::new(Tone::Plain, text),
    }
}

/// How the GPG key timer's setup stands, for its card. Switched off, nothing is
/// run: whether GnuPG (and git) are installed. Switched on: what's missing in
/// yellow (git, its signing with GPG, GnuPG, the key, an answer from gpg-agent), or
/// in green that it watches the key, and whether the key is unlocked. Whole texts
/// are given in English: the card translates them.
fn gpg_status(on: bool, check: Option<crate::gpg::Check>) -> Status {
    use crate::gpg::{Check, Missing};
    const ABOUT: &str = "How long your key stays unlocked";
    match (on, check) {
        (false, Some(Check::Found { git, gpg })) => {
            let mut parts = vec![t(ABOUT), if gpg { t("GnuPG found") } else { t("GnuPG not found") }];
            if !git {
                parts.push(t("git not found"));
            }
            Status::new(Tone::Plain, parts.join(" · "))
        }
        (false, _) => Status::new(Tone::Plain, ABOUT),
        (true, None | Some(Check::Found { .. })) => Status::new(Tone::Busy, "Checking git and GnuPG…"),
        (true, Some(Check::Missing(why))) => Status::new(
            Tone::Warn,
            match why {
                Missing::Git => "git not found",
                Missing::NotSigning => "git doesn't sign commits (commit.gpgsign is off)",
                Missing::OtherFormat => "git signs with SSH or X.509, not GnuPG (gpg.format)",
                Missing::GnuPg => "GnuPG not found",
                Missing::Key => "GnuPG has no key that git signs with",
                Missing::Agent => "gpg-agent didn't answer about the key",
            },
        ),
        (true, Some(Check::Ready { running, cached })) => Status::new(
            Tone::Good,
            match (running, cached) {
                (false, _) => "✓ git signs with GnuPG · gpg-agent not running",
                (true, true) => "✓ git signs with GnuPG · key unlocked",
                (true, false) => "✓ git signs with GnuPG · key locked",
            },
        ),
    }
}

/// How many hosts the page of GitLab and GitHub lists: as many as fit.
const HOST_ROWS: usize = 3;

/// The hosts the page of GitLab and GitHub lists: those the tools were asked
/// about, and github.com (for a token before gh is there). At most as many as
/// fit: first the ones with Trayward's token (it must stay removable), then
/// github.com, then the others; shown in their order.
fn host_rows(tools: &crate::forge::Tools) -> Vec<(crate::forge::Forge, crate::forge::Host)> {
    use crate::forge::{Forge, Host};
    let mut rows: Vec<(Forge, Host)> = tools.glab.hosts.iter().map(|h| (Forge::GitLab, h.clone())).collect();
    rows.extend(tools.gh.hosts.iter().map(|h| (Forge::GitHub, h.clone())));
    let github = |f: &Forge, h: &Host| *f == Forge::GitHub && h.name.eq_ignore_ascii_case("github.com");
    if !rows.iter().any(|(f, h)| github(f, h)) {
        rows.push((Forge::GitHub, Host { name: "github.com".into(), ..Default::default() }));
    }
    // Which rows stay: by rank, then shown in their order.
    let rank = |(f, h): &(Forge, Host)| {
        if h.token {
            0
        } else if github(f, h) {
            1
        } else {
            2
        }
    };
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&i| rows.get(i).map_or(3, rank));
    order.truncate(HOST_ROWS);
    order.sort_unstable();
    order.iter().filter_map(|&i| rows.get(i).cloned()).collect()
}

/// What a host's card says: which login its tool uses there and whether it works,
/// and when Trayward's token runs out (in yellow from 14 days before). Translated:
/// the card shows the host's name as it is, and this as given.
fn host_status(forge: crate::forge::Forge, host: &crate::forge::Host, tools: &crate::forge::Tools, now: i64) -> Status {
    let name = crate::forge::program(forge);
    let found = match forge {
        crate::forge::Forge::GitLab => tools.glab.found,
        crate::forge::Forge::GitHub => tools.gh.found,
    };
    if !found {
        return Status::new(Tone::Warn, f("{0} not found", &[&name]));
    }
    if !tools.asked {
        return Status::new(Tone::Busy, t("Checking…"));
    }
    match (host.token, host.ok, host.expires) {
        (true, true, Some(at)) if at - now < 14 * 86_400 => {
            Status::new(Tone::Warn, f("Trayward's token expires {0}: paste a new one", &[&day_with_year(at)]))
        }
        (true, true, Some(at)) => Status::new(Tone::Good, f("✓ Trayward's token, until {0}", &[&day_with_year(at)])),
        (true, true, None) => Status::new(Tone::Good, t("✓ Trayward's token")),
        (true, false, _) => {
            Status::new(Tone::Warn, t("Trayward's token doesn't work: expired, or a permission is missing"))
        }
        (false, true, _) => Status::new(Tone::Good, f("✓ {0}'s own login", &[&name])),
        (false, false, _) => Status::new(Tone::Warn, t("Not logged in: paste a token")),
    }
}

/// A day with its year, `9 Jun 2027`: a token runs for up to a year.
fn day_with_year(at: i64) -> String {
    let year = crate::timefmt::rfc3339(at).get(..4).unwrap_or_default().to_string();
    format!("{} {year}", trayward_core::os::local_date(at))
}

/// What a tool's card says: its version and where it is, or how to install it
/// (in yellow when a host needs it).
fn tool_status(forge: crate::forge::Forge, tool: &crate::forge::Tool) -> Status {
    let id = match forge {
        crate::forge::Forge::GitLab => "GLab.GLab",
        crate::forge::Forge::GitHub => "GitHub.cli",
    };
    if !tool.found {
        let tone = if tool.hosts.is_empty() { Tone::Plain } else { Tone::Warn };
        return Status::new(tone, f("Not found: winget install --id {0}", &[&id]));
    }
    let parts: Vec<&str> = [tool.version.as_deref(), tool.path.as_deref()].into_iter().flatten().collect();
    Status::new(Tone::Good, format!("✓ {}", parts.join(" · ")))
}

fn tone_color(pal: &Palette, tone: Tone) -> Color {
    match tone {
        Tone::Plain => pal.text_dim,
        Tone::Busy | Tone::Info => pal.accent,
        Tone::Good => pal.done,
        Tone::Warn => pal.warn,
        Tone::Bad => pal.crit,
    }
}

/// A text field of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    MqttHost,
    MqttPort,
    MqttUser,
    MqttPassword,
    /// Away mode's push setup (`push::Fields`).
    PushServer,
    PushTopic,
    PushToken,
    PushAddress,
}

/// A drop-down list of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    Language,
}

/// A setting being edited in fields, instead of its card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The MQTT broker; `tls`: mqtts://.
    Broker { tls: bool },
    /// Away mode's push setup, for this service.
    Push { service: crate::push::Service },
}

/// An action that forgets something saved: its button is red.
fn destructive(action: &Action) -> bool {
    matches!(action, Action::RemovePush | Action::RemoveBroker | Action::RemoveToken(..))
}

/// The icon in front of a button's label, for the actions that have one.
fn button_icon(action: &Action) -> Option<char> {
    match action {
        Action::Edit(_) => Some(PENCIL),
        Action::PasteMailbox | Action::PasteForm | Action::PasteToken(..) => Some(CLIPBOARD),
        Action::RemovePush | Action::RemoveBroker | Action::RemoveToken(..) => Some(BIN),
        _ => None,
    }
}

/// What a layout gives: the clickable areas, the text fields, and the
/// descriptions cut short, which show all of themselves on hover (DIPs).
#[derive(Default)]
struct Drawn {
    hits: Vec<Hit>,
    fields: Vec<crate::edit::Want<Field>>,
    tips: Vec<Rect>,
    /// Where the open drop-down list's options start in `hits`: over
    /// everything else, the only ones a click reaches while it's open.
    top: Option<usize>,
}

/// x, y, width, height (DIPs).
type Rect = (f32, f32, f32, f32);

/// An option of a set of choices or a list: its label, what it does, whether it's chosen.
type Choice = (String, Action, bool);

fn inside((x, y, w, h): Rect, (px, py): (f32, f32)) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// How a card's description is drawn when it says how something stands.
#[derive(Clone, Copy)]
struct DescStyle {
    color: Color,
    /// A turning arrow in front of it (the angle): something is being looked for.
    spin: Option<u16>,
}

/// What a click does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Page(usize),
    /// Opens (or closes) a drop-down list.
    Select(Select),
    Autostart,
    Interval(u32),
    Animations,
    Mailbox,
    /// The user's own extensions, as a whole.
    OwnExtensions,
    /// One of them, by its id.
    Extension(String),
    /// Open their folder.
    ExtensionsFolder,
    /// Show trayward.exe in its folder (About → Location).
    ShowExe,
    PasteMailbox,
    FindMailbox,
    /// Display mode: slim or normal.
    Slim(bool),
    /// How long each limit is shown when they take turns (0: no turns).
    TurnSecs(u8),
    DailyBudget(crate::forecast::Budget),
    WeekBar(crate::flyout::WeekBar),
    Bars(Bars),
    Solid,
    Place(settings::Place),
    Taskbars(Taskbars),
    /// Show or hide the widget on a display's taskbar (by id).
    Display(String),
    Hooks,
    Transcripts,
    Wsl,
    HideArchived,
    GpgTimer,
    SimulateFiveHour,
    Poll(u32),
    Bubbles,
    /// Sound for a kind: 0 off, 1 to 3 built-in, 4 custom file.
    Sound(Kind, u8),
    Play(Kind),
    ChooseFile(Kind),
    Volume(Volume),
    /// Warning levels in percent (0: off).
    WarnWeekly(u8),
    WarnFiveHour(u8),
    WarnPace,
    CountdownAt(u8),
    NotifyFree,
    Fable,
    Codex,
    /// Copilot: its sessions, and its monthly limit.
    Copilot(bool, bool),
    Shows(settings::Shown),
    Language(crate::i18n::Choice),
    Forge,
    ForgeBubbles,
    /// Where merge and pull requests show: in the flyout, as bubbles, or both.
    ForgeShows {
        flyout: bool,
        bubbles: bool,
    },
    /// Take a token for a host from the clipboard, as Trayward's own.
    PasteToken(crate::forge::Forge, String),
    /// Forget Trayward's own token for a host (asks first).
    RemoveToken(crate::forge::Forge, String),
    WeekSummary,
    Away,
    AwayAfter(u32),
    AwayDone,
    TestPush,
    /// Forget the push setup (asks first).
    RemovePush,
    /// Forget the MQTT broker and its password (asks first).
    RemoveBroker,
    UpdateCheck,
    CheckNow,
    KeepStats,
    Mqtt,
    TestMqtt,
    /// Edit a setting in text fields.
    Edit(Form),
    /// The broker form's protocol: mqtts:// (true) or mqtt://.
    FormTls(bool),
    /// The push form's service.
    FormService(crate::push::Service),
    /// Save what's in the fields.
    SaveForm,
    /// Close the fields without saving.
    CancelForm,
    /// Fill the fields from an address on the clipboard.
    PasteForm,
    /// Open a web page.
    Open(String),
    /// Switch the system monitor as a whole on or off.
    MonitorOn,
    /// Switch a value of the system monitor (a `sysmon` bit) on or off.
    Monitor(u8),
    /// Show a value of the system monitor (a `sysmon` bit): off (`None`), as
    /// a percentage (`Some(false)`) or the other way (`Some(true)`: GB, rates).
    MonitorShow(u8, Option<bool>),
    MonitorGraphs,
}

type Hit = crate::hits::Hit<Action>;

struct Layout<'a> {
    c: Option<&'a Canvas<'a>>,
    g: std::rc::Rc<gfx::Gfx>,
    pal: &'static Palette,
    hits: Vec<Hit>,
    /// Hovered hit index from the previous layout.
    hover: Option<usize>,
    /// Cards go into this column (x, width) instead of the page's whole width.
    column: Option<(f32, f32)>,
    /// The next card's description, drawn this way (taken by `card`).
    desc_style: Option<DescStyle>,
    /// The next card's title and description are the user's: not translated, no help.
    verbatim: bool,
    /// The next card's "?" is this topic's, not its title's (a card titled with a name).
    help_topic: Option<&'static str>,
    /// The hovered "?": where it is and its short help, drawn last (on top).
    help_tip: Option<(f32, f32, &'static str)>,
    /// The text fields laid out so far.
    fields: Vec<crate::edit::Want<Field>>,
    /// Where the mouse is (DIPs), for the tooltips of descriptions cut short.
    mouse: Option<(f32, f32)>,
    /// The descriptions cut short so far.
    tips: Vec<Rect>,
    /// The hovered one: where it is and all of it, in its colour; drawn last.
    desc_tip: Option<(Rect, String, Color)>,
    /// The drop-down list that's open.
    select: Option<Select>,
    /// Its box and its options, drawn last (over everything).
    popup: Option<(Rect, Vec<Choice>)>,
}

impl Layout<'_> {
    fn next_hovered(&self) -> bool {
        self.hover == Some(self.hits.len())
    }

    fn push(&mut self, x: f32, y: f32, w: f32, h: f32, action: Action) {
        self.hits.push(Hit { x, y, w, h, action, tag: () });
    }

    /// Whether the card titled `title`, whose own area comes after its "?"
    /// (if it has help), is hovered.
    fn card_hovered(&self, title: &str) -> bool {
        let help = !self.verbatim && crate::help::tip(title).is_some();
        self.hover == Some(self.hits.len() + usize::from(help))
    }

    /// A text field `w` wide with its label above it, from `y`; returns its height.
    /// The field itself is a child window over the box drawn here (see `edit.rs`).
    #[allow(clippy::too_many_arguments, reason = "a text field's position, size, text, state and action")]
    fn field(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        label: &str,
        id: Field,
        kind: crate::edit::Kind,
        hint: &str,
        text: &str,
    ) -> Result<f32> {
        let label = self.g.text(&tr(label), 12.0, NORMAL, Some(w))?;
        let by = y + FIELD_LABEL_H;
        if let Some(c) = self.c {
            c.text(&label, x + 2.0, y, self.pal.text_dim);
            c.fill_round_rect(x, by, w, FIELD_H, 4.0, self.pal.window);
            c.stroke_round_rect(x + 0.5, by + 0.5, w - 1.0, FIELD_H - 1.0, 4.0, 1.0, self.pal.divider.with_alpha(2.5));
        }
        // The edit control inside the box, its line centred (13 px text).
        let line = 18.0;
        self.fields.push(crate::edit::Want {
            id,
            rect: (x + 9.0, by + (FIELD_H - line) / 2.0, w - 18.0, line),
            kind,
            hint: hint.to_string(),
            text: text.to_string(),
        });
        Ok(FIELD_LABEL_H + FIELD_H)
    }

    /// How wide the button for `label` and `action` is: its label, and its
    /// icon in front when it has one.
    fn button_w(&self, label: &str, action: &Action) -> Result<f32> {
        let icon = button_icon(action).map(|i| self.g.glyph(i, 13.0)).transpose()?;
        Ok(self.g.text(&tr(label), 13.0, NORMAL, None)?.width + 24.0 + icon.map_or(0.0, |i| i.width + 7.0))
    }

    /// A button from `x` (or ending at `x` when `right`), centred on `cy`:
    /// primary (the main action, in the accent colour) or secondary (a quiet
    /// fill with an outline). Returns its width.
    fn button(&mut self, x: f32, right: bool, cy: f32, label: &str, primary: bool, action: Action) -> Result<f32> {
        let text = self.g.text(&tr(label), 13.0, NORMAL, None)?;
        let icon = button_icon(&action).map(|i| self.g.glyph(i, 13.0)).transpose()?;
        let (w, h) = (self.button_w(label, &action)?, 30.0);
        let x = if right { x - w } else { x };
        let y = cy - h / 2.0;
        let hovered = self.next_hovered();
        if let Some(c) = self.c {
            let pal = self.pal;
            // Removing something: red, so it doesn't look like the others.
            let destructive = destructive(&action);
            if primary {
                c.fill_round_rect(x, y, w, h, 5.0, pal.accent.with_alpha(if hovered { 0.85 } else { 1.0 }));
            } else if destructive {
                c.fill_round_rect(x, y, w, h, 5.0, pal.crit.with_alpha(if hovered { 0.16 } else { 0.06 }));
                c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 5.0, 1.0, pal.crit);
            } else {
                c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover.with_alpha(if hovered { 2.6 } else { 1.6 }));
                c.stroke_round_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0, 5.0, 1.0, pal.divider.with_alpha(2.0));
            }
            let color = match (primary, destructive) {
                (true, _) => pal.on_accent,
                (false, true) => pal.crit,
                (false, false) => pal.text,
            };
            let mut tx = x + 12.0;
            if let Some(i) = &icon {
                c.text(i, tx, cy - i.height / 2.0, color);
                tx += i.width + 7.0;
            }
            c.text(&text, tx, cy - text.height / 2.0, color);
        }
        self.push(x, y, w, h, action);
        Ok(w)
    }

    /// The buttons under a form: Save and Cancel on the left, Paste on the right.
    fn form_buttons(&mut self, x: f32, w: f32, cy: f32, paste: &str) -> Result<()> {
        let save = self.button(x, false, cy, "Save", true, Action::SaveForm)?;
        self.button(x + save + 8.0, false, cy, "Cancel", false, Action::CancelForm)?;
        self.button(x + w, true, cy, paste, false, Action::PasteForm)?;
        Ok(())
    }

    /// The hits, after drawing the open drop-down list and the hovered help's
    /// box on top of everything.
    fn finish(mut self) -> Result<Drawn> {
        let mut top = None;
        if let Some(((bx, by, bw, bh), options)) = self.popup.take() {
            let (ih, pad) = (30.0, 4.0);
            let h = options.len() as f32 * ih + 2.0 * pad;
            // Under the box, or above it when there's no room.
            let ly = if by + bh + 4.0 + h <= HEIGHT - 8.0 { by + bh + 4.0 } else { (by - 4.0 - h).max(8.0) };
            let pal = self.pal;
            if let Some(c) = self.c {
                c.fill_round_rect(bx, ly, bw, h, 6.0, pal.window);
                c.stroke_round_rect(bx + 0.5, ly + 0.5, bw - 1.0, h - 1.0, 6.0, 1.0, pal.divider.with_alpha(2.5));
            }
            top = Some(self.hits.len());
            for (i, (label, action, chosen)) in options.into_iter().enumerate() {
                let iy = ly + pad + i as f32 * ih;
                let (ix, iw) = (bx + pad, bw - 2.0 * pad);
                let hovered = self.next_hovered();
                if let Some(c) = self.c {
                    if chosen {
                        c.fill_round_rect(ix, iy, iw, ih, 4.0, pal.row_hover.with_alpha(1.6));
                        c.fill_round_rect(ix, iy + 9.0, 3.0, ih - 18.0, 1.5, pal.accent);
                    } else if hovered {
                        c.fill_round_rect(ix, iy, iw, ih, 4.0, pal.row_hover);
                    }
                    let text = self.g.text(&tr(&label), 13.0, NORMAL, None)?;
                    c.text(&text, bx + 12.0, iy + (ih - text.height) / 2.0, pal.text);
                }
                self.push(ix, iy, iw, ih, action);
            }
        }
        if let (Some(c), Some((x, y, tip))) = (self.c, self.help_tip) {
            let pal = self.pal;
            let text = self.g.wrapped(&tr(tip), 12.0, NORMAL, HELP_TIP_W - 24.0)?;
            let more = self.g.text(t("Click for the full help"), 11.0, NORMAL, None)?;
            let h = 12.0 + text.height + 6.0 + more.height + 10.0;
            // Below the "?", or above it when there's no room; inside the window.
            let bx = (x - 12.0).clamp(8.0, WIDTH - HELP_TIP_W - 8.0);
            let by = if y + 14.0 + h <= HEIGHT - 8.0 { y + 14.0 } else { (y - 10.0 - h).max(8.0) };
            c.fill_round_rect(bx, by, HELP_TIP_W, h, 6.0, pal.window);
            c.stroke_round_rect(bx + 0.5, by + 0.5, HELP_TIP_W - 1.0, h - 1.0, 6.0, 1.0, pal.divider.with_alpha(2.5));
            c.text(&text, bx + 12.0, by + 12.0, pal.text);
            c.text(&more, bx + 12.0, by + 12.0 + text.height + 6.0, pal.accent);
        }
        // A description cut short, in full: under it, or above it when there's no
        // room or a text field is there (the fields are windows: they'd cover it).
        if let (Some(c), Some(((ax, ay, aw, ah), full, color))) = (self.c, &self.desc_tip) {
            let pal = self.pal;
            let w = (self.g.text(full, 12.0, NORMAL, None)?.width + 24.0).min(aw.max(HELP_TIP_W)).min(WIDTH - 16.0);
            let text = self.g.wrapped(full, 12.0, NORMAL, w - 24.0)?;
            let h = text.height + 20.0;
            let bx = (ax - 12.0).clamp(8.0, WIDTH - w - 8.0);
            let below = ay + ah + 6.0;
            let covered = self.fields.iter().any(|f| {
                let (fx, fy, fw, fh) = f.rect;
                fx < bx + w && bx < fx + fw && fy - 8.0 < below + h && below < fy + fh + 8.0
            });
            let by = if below + h <= HEIGHT - 8.0 && !covered { below } else { (ay - 6.0 - h).max(8.0) };
            c.fill_round_rect(bx, by, w, h, 6.0, pal.window);
            c.stroke_round_rect(bx + 0.5, by + 0.5, w - 1.0, h - 1.0, 6.0, 1.0, pal.divider.with_alpha(2.5));
            c.text(&text, bx + 12.0, by + 10.0, *color);
        }
        Ok(Drawn { hits: self.hits, fields: self.fields, tips: self.tips, top })
    }

    /// A card with a title and a description, leaving `control_w` on the
    /// right for its control; returns its content box.
    fn card(
        &mut self,
        y: f32,
        h: f32,
        title: &str,
        desc: &str,
        hovered: bool,
        control_w: f32,
    ) -> Result<(f32, f32, f32)> {
        let (x, w) = self.column.unwrap_or((NAV_W + PAD, WIDTH - NAV_W - 2.0 * PAD));
        let text_w = w - 36.0 - control_w;
        // The user's own text (an extension's label): as written, without help.
        let verbatim = std::mem::take(&mut self.verbatim);
        let tr = |s: &str| if verbatim { s.to_string() } else { tr(s) };
        let topic = self.help_topic.take();
        let help = match topic {
            Some(topic) => crate::help::tip(topic),
            None if verbatim => None,
            None => crate::help::tip(title),
        };
        let help_title = topic.unwrap_or(title);
        // Room for the "?" after the title.
        let title_w = text_w - if help.is_some() { 22.0 } else { 0.0 };
        let title = self.g.text(&tr(title), 14.0, NORMAL, Some(title_w))?;
        let style = self.desc_style.take();
        let spin =
            style.and_then(|s| s.spin).map(|a| self.g.text("↻", 12.0, NORMAL, None).map(|t| (t, a))).transpose()?;
        let indent = spin.as_ref().map_or(0.0, |(t, _)| t.width + 5.0);
        let full = tr(desc);
        let desc_w = text_w - indent;
        let desc = (!desc.is_empty()).then(|| self.g.text(&full, 12.0, NORMAL, Some(desc_w))).transpose()?;
        let block = title.height + desc.as_ref().map_or(0.0, |d| d.height + 1.0);
        let top = if h > CARD_H { y + 14.0 } else { y + (h - block) / 2.0 };
        // Cut short: all of it on hover.
        let cut_short = match &desc {
            Some(_) => self.g.text(&full, 12.0, NORMAL, None)?.width > desc_w + 0.5,
            None => false,
        };
        if let Some(d) = desc.as_ref().filter(|_| cut_short) {
            let area = (x + 18.0 + indent, top + title.height + 1.0, desc_w, d.height);
            self.tips.push(area);
            if self.mouse.is_some_and(|m| inside(area, m)) {
                let color = style.map_or(self.pal.text, |s| s.color);
                self.desc_tip = Some((area, full.to_string(), color));
            }
        }
        if let Some(c) = self.c {
            c.fill_round_rect(x, y, w, h, 6.0, if hovered { self.pal.card_hover } else { self.pal.card });
            c.text(&title, x + 18.0, top, self.pal.text);
            if let Some(d) = &desc {
                let color = style.map_or(self.pal.text_dim, |s| s.color);
                let dy = top + title.height + 1.0;
                if let Some((arrow, angle)) = &spin {
                    c.text_rotated(arrow, x + 18.0, dy + (d.height - arrow.height) / 2.0, color, f32::from(*angle));
                }
                c.text(d, x + 18.0 + indent, dy, color);
            }
        }
        if let Some(tip) = help {
            let url = crate::help::url(help_title);
            self.help_icon(url, tip, 12.0, x + 18.0 + title.width + 6.0, top + title.height / 2.0)?;
        }
        Ok((x, y, w))
    }

    /// The "?" from `x`, `size` high, centred on `cy`: its help on hover, its
    /// section of the help page (`url`) on click.
    fn help_icon(&mut self, url: String, tip: &'static str, size: f32, x: f32, cy: f32) -> Result<()> {
        let hovered = self.next_hovered();
        let glyph = self.g.glyph(HELP, size)?;
        if let Some(c) = self.c {
            let color = if hovered { self.pal.accent } else { self.pal.text_dim };
            c.text(&glyph, x, cy - glyph.height / 2.0, color);
        }
        if hovered {
            self.help_tip = Some((x, cy, tip));
        }
        self.push(x - 4.0, cy - 10.0, glyph.width + 8.0, 20.0, Action::Open(url));
        Ok(())
    }

    /// A card whose whole area toggles a switch.
    fn toggle_card(&mut self, y: f32, title: &str, desc: &str, on: bool, action: Action) -> Result<f32> {
        self.toggle_card_h(y, CARD_H, title, desc, on, action)
    }

    /// A toggle card `h` high.
    fn toggle_card_h(&mut self, y: f32, h: f32, title: &str, desc: &str, on: bool, action: Action) -> Result<f32> {
        let hovered = self.card_hovered(title);
        // Room for the switch and its On/Off label.
        let (x, y, w) = self.card(y, h, title, desc, hovered, 80.0)?;
        let label = self.g.text(if on { t("On") } else { t("Off") }, 13.0, NORMAL, None)?;
        if let Some(c) = self.c {
            let (tw, th) = (40.0, 20.0);
            let tx = x + w - 18.0 - tw;
            let ty = y + (h - th) / 2.0;
            if on {
                c.fill_round_rect(tx, ty, tw, th, th / 2.0, self.pal.accent);
                c.fill_circle(tx + tw - th / 2.0, ty + th / 2.0, 6.0, self.pal.on_accent);
            } else {
                c.stroke_round_rect(tx + 0.5, ty + 0.5, tw - 1.0, th - 1.0, th / 2.0, 1.0, self.pal.stroke);
                c.fill_circle(tx + th / 2.0, ty + th / 2.0, 5.0, self.pal.stroke);
            }
            c.text(&label, tx - 10.0 - label.width, y + (h - label.height) / 2.0, self.pal.text);
        }
        self.push(x, y, w, h, action);
        Ok(h + GAP)
    }

    /// Segmented choices, right-aligned at `right` (or left-aligned at `left`),
    /// centred on `cy`. Returns the width used.
    fn segments(&mut self, left: Option<f32>, right: f32, cy: f32, options: &[(String, Action, bool)]) -> Result<f32> {
        let texts =
            options.iter().map(|(o, _, _)| self.g.text(&tr(o), 13.0, NORMAL, None)).collect::<Result<Vec<_>>>()?;
        let widths: Vec<f32> = texts.iter().map(|t| t.width + 22.0).collect();
        let total: f32 = widths.iter().sum();
        let h = 30.0;
        let mut x = left.unwrap_or(right - total);
        let y = cy - h / 2.0;
        if let Some(c) = self.c {
            c.stroke_round_rect(x + 0.5, y + 0.5, total - 1.0, h - 1.0, 5.0, 1.0, self.pal.divider.with_alpha(2.5));
        }
        for ((t, w), (_, action, selected)) in texts.iter().zip(&widths).zip(options) {
            let hovered = self.next_hovered();
            if let Some(c) = self.c {
                if *selected {
                    c.fill_round_rect(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 4.0, self.pal.accent);
                } else if hovered {
                    c.fill_round_rect(x + 2.0, y + 2.0, w - 4.0, h - 4.0, 4.0, self.pal.row_hover);
                }
                let color = if *selected { self.pal.on_accent } else { self.pal.text };
                c.text(t, x + (w - t.width) / 2.0, cy - t.height / 2.0, color);
            }
            self.push(x, y, *w, h, action.clone());
            x += w;
        }
        Ok(total)
    }

    /// A card with segmented choices on the right.
    fn choice_card(&mut self, y: f32, title: &str, desc: &str, options: &[(String, Action, bool)]) -> Result<f32> {
        self.choice_card_h(y, CARD_H, title, desc, options)
    }

    /// A choice card `h` high.
    fn choice_card_h(
        &mut self,
        y: f32,
        h: f32,
        title: &str,
        desc: &str,
        options: &[(String, Action, bool)],
    ) -> Result<f32> {
        let mut control_w = 0.0;
        for (o, _, _) in options {
            control_w += self.g.text(&tr(o), 13.0, NORMAL, None)?.width + 22.0;
        }
        let (x, y, w) = self.card(y, h, title, desc, false, control_w)?;
        self.segments(None, x + w - 18.0, y + h / 2.0, options)?;
        Ok(h + GAP)
    }

    /// A card with buttons on the right, each on its own; the ones marked
    /// `true` filled (the main one).
    fn buttons_card(&mut self, y: f32, title: &str, desc: &str, buttons: &[(String, Action, bool)]) -> Result<f32> {
        self.buttons_card_h(y, CARD_H, title, desc, buttons)
    }

    /// A buttons card `h` high.
    fn buttons_card_h(
        &mut self,
        y: f32,
        h: f32,
        title: &str,
        desc: &str,
        buttons: &[(String, Action, bool)],
    ) -> Result<f32> {
        let mut control_w = BUTTON_SPACE * buttons.len().saturating_sub(1) as f32;
        for (label, action, _) in buttons {
            control_w += self.button_w(label, action)?;
        }
        let (x, y, w) = self.card(y, h, title, desc, false, control_w)?;
        self.buttons(x + w - 18.0 - control_w, y + h / 2.0, buttons)?;
        Ok(h + GAP)
    }

    /// A card with its buttons on a line of their own, under the description:
    /// for a description that needs the whole width (an address, a status).
    fn buttons_below_card(
        &mut self,
        y: f32,
        title: &str,
        desc: &str,
        buttons: &[(String, Action, bool)],
    ) -> Result<f32> {
        let (x, y, _) = self.card(y, BUTTONS_CARD_H, title, desc, false, 0.0)?;
        self.buttons(x + 18.0, y + BUTTONS_CARD_H - 30.0, buttons)?;
        Ok(BUTTONS_CARD_H + GAP)
    }

    /// Buttons side by side from `x`, centred on `cy`.
    fn buttons(&mut self, mut x: f32, cy: f32, buttons: &[(String, Action, bool)]) -> Result<()> {
        for (label, action, primary) in buttons {
            x += self.button(x, false, cy, label, *primary, action.clone())? + BUTTON_SPACE;
        }
        Ok(())
    }

    /// A card with a drop-down list on the right: the chosen option in a box,
    /// all of them under it (over everything else) while it's open.
    fn select_card(
        &mut self,
        y: f32,
        title: &str,
        desc: &str,
        select: Select,
        options: &[(String, Action, bool)],
    ) -> Result<f32> {
        let mut widest = 0.0f32;
        for (label, _, _) in options {
            widest = widest.max(self.g.text(&tr(label), 13.0, NORMAL, None)?.width);
        }
        let chevron = self.g.glyph(CHEVRON, 10.0)?;
        let (bw, bh) = ((widest + chevron.width + 44.0).max(120.0), 30.0);
        let (x, y, w) = self.card(y, CARD_H, title, desc, false, bw)?;
        let (bx, by) = (x + w - 18.0 - bw, y + (CARD_H - bh) / 2.0);
        let open = self.select == Some(select);
        let hovered = self.next_hovered();
        let chosen = options.iter().find(|o| o.2).map(|o| tr(&o.0)).unwrap_or_default();
        let label = self.g.text(&chosen, 13.0, NORMAL, None)?;
        if let Some(c) = self.c {
            let pal = self.pal;
            c.fill_round_rect(bx, by, bw, bh, 4.0, if hovered { pal.row_hover } else { pal.window });
            let border = if open { pal.accent } else { pal.divider.with_alpha(2.5) };
            c.stroke_round_rect(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, 4.0, 1.0, border);
            c.text(&label, bx + 12.0, by + (bh - label.height) / 2.0, pal.text);
            c.text(&chevron, bx + bw - 14.0 - chevron.width, by + (bh - chevron.height) / 2.0, pal.text_dim);
        }
        self.push(bx, by, bw, bh, Action::Select(select));
        if open {
            self.popup = Some(((bx, by, bw, bh), options.to_vec()));
        }
        Ok(CARD_H + GAP)
    }

    /// A small round icon button.
    fn icon_button(&mut self, cx: f32, cy: f32, glyph: char, action: Action) -> Result<()> {
        let hovered = self.next_hovered();
        let t = self.g.glyph(glyph, 14.0)?;
        let r = 15.0;
        if let Some(c) = self.c {
            if hovered {
                c.fill_circle(cx, cy, r, self.pal.row_hover);
            }
            c.stroke_circle(cx, cy, r - 0.5, 1.0, self.pal.divider.with_alpha(2.5));
            c.text(&t, cx - t.width / 2.0, cy - t.height / 2.0, self.pal.text);
        }
        self.push(cx - r, cy - r, 2.0 * r, 2.0 * r, action);
        Ok(())
    }

    fn sound_card(&mut self, y: f32, title: &str, kind: Kind, choice: &SoundChoice) -> Result<f32> {
        let desc = match choice {
            SoundChoice::Off => "No sound".to_string(),
            SoundChoice::Builtin(n) => sound::descriptions(kind)[usize::from(n.saturating_sub(1)).min(2)].to_string(),
            SoundChoice::File(path) => path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
        };
        // The choices sit on their own line, so the text can use the full width.
        let (x, y, w) = self.card(y, SOUND_CARD_H, title, &desc, false, 0.0)?;
        let mut options = vec![("Off".to_string(), Action::Sound(kind, 0), *choice == SoundChoice::Off)];
        for (i, name) in sound::names(kind).iter().enumerate() {
            let n = i as u8 + 1;
            options.push((name.to_string(), Action::Sound(kind, n), *choice == SoundChoice::Builtin(n)));
        }
        options.push(("Custom".into(), Action::Sound(kind, 4), matches!(choice, SoundChoice::File(_))));
        let cy = y + SOUND_CARD_H - 30.0;
        let used = self.segments(Some(x + 18.0), 0.0, cy, &options)?;
        let mut bx = x + 18.0 + used + 26.0;
        if *choice != SoundChoice::Off {
            self.icon_button(bx, cy, PLAY, Action::Play(kind))?;
            bx += 38.0;
        }
        if matches!(choice, SoundChoice::File(_)) {
            self.icon_button(bx, cy, FOLDER, Action::ChooseFile(kind))?;
        }
        let _ = w;
        Ok(SOUND_CARD_H + GAP)
    }
}

/// Lays out the window (and draws it, with a canvas), with `form` open on its
/// page. Returns the clickable areas and the text fields.
fn render<'a>(
    c: Option<&'a Canvas<'a>>,
    m: &Model,
    page: usize,
    hover: Option<usize>,
    form: Option<Form>,
    select: Option<Select>,
    mouse: Option<(f32, f32)>,
) -> Result<Drawn> {
    let pal = theme::palette(m.dark);
    let mut l = Layout {
        c,
        g: gfx::get()?,
        pal,
        hits: Vec::new(),
        hover,
        column: None,
        desc_style: None,
        verbatim: false,
        help_topic: None,
        help_tip: None,
        fields: Vec::new(),
        mouse,
        tips: Vec::new(),
        desc_tip: None,
        select,
        popup: None,
    };

    // Navigation.
    let mut y = PAD + 8.0;
    if let Some(c) = c {
        let t = l.g.text("Trayward", 14.0, SEMIBOLD, None)?;
        c.text(&t, PAD, PAD - 4.0, pal.text);
        let v = l.g.text(crate::app::VERSION, 12.0, NORMAL, None)?;
        c.text(&v, PAD + t.width + 8.0, PAD - 4.0 + (t.height - v.height) / 2.0 + 1.0, pal.text_dim);
    }
    y += 24.0;
    // A page whose extension was switched off: the Extensions page instead.
    let page = if page_shown(page, &m.prefs) { page } else { page::EXTENSIONS };
    // Rows 30 high, closer together when every extension is on and they wouldn't fit.
    let shown = (0..PAGES.len()).filter(|&i| page_shown(i, &m.prefs)).count().max(1) as f32;
    let pitch = ((HEIGHT - 8.0 - 7.0 - y) / shown).min(33.0);
    for (i, (name, glyph)) in PAGES.iter().enumerate().filter(|(i, _)| page_shown(*i, &m.prefs)) {
        // The extensions' pages under a line, apart from the others.
        let first_extension =
            i > page::EXTENSIONS && i < page::ABOUT && (page::EXTENSIONS + 1..i).all(|j| !page_shown(j, &m.prefs));
        if first_extension {
            if let Some(c) = c {
                c.fill_rect(PAD, y + 1.0, NAV_W - 2.0 * PAD, 1.0, pal.divider);
            }
            y += 7.0;
        }
        let hovered = l.next_hovered();
        let (x, w, h) = (PAD / 2.0, NAV_W - PAD, pitch - 3.0);
        if let Some(c) = c {
            if i == page {
                c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover.with_alpha(1.6));
                c.fill_round_rect(x, y + h / 3.0, 3.0, h / 3.0, 1.5, pal.accent);
            } else if hovered {
                c.fill_round_rect(x, y, w, h, 5.0, pal.row_hover);
            }
            let icon = l.g.glyph(*glyph, 16.0)?;
            c.text(&icon, x + 14.0, y + (h - icon.height) / 2.0, pal.text);
            let label = l.g.text(&tr(name), 13.0, NORMAL, None)?;
            c.text(&label, x + 42.0, y + (h - label.height) / 2.0, pal.text);
        }
        l.push(x, y, w, h, Action::Page(i));
        y += pitch;
    }

    // Page.
    let x = NAV_W + PAD;
    let title = l.g.text(&tr(PAGES[page].0), 22.0, SEMIBOLD, None)?;
    if let Some(c) = c {
        c.text(&title, x, PAD - 6.0, pal.text);
    }
    // The page's "?": what's on it, and its part of the help page.
    if let Some(tip) = crate::help::page_tip(PAGES[page].0) {
        let url = crate::help::page_url(PAGES[page].0);
        l.help_icon(url, tip, 14.0, x + title.width + 10.0, PAD - 6.0 + title.height / 2.0 + 1.0)?;
    }
    let mut y = PAD + title.height + 8.0;
    let p = &m.prefs;
    match page {
        page::GENERAL => {
            y += l.toggle_card(
                y,
                "Start with Windows",
                "Start the app when you sign in",
                m.autostart,
                Action::Autostart,
            )?;
            let options: Vec<_> = settings::INTERVALS
                .iter()
                .map(|(secs, _)| (format!("{}m", secs / 60), Action::Interval(*secs), *secs == m.interval))
                .collect();
            y += l.choice_card(y, "Refresh usage every", "The usage API is rate-limited", &options)?;
            y += l.toggle_card(
                y,
                "Animations",
                "Glow, shake and the refresh spinner",
                p.animations,
                Action::Animations,
            )?;
            y += l.toggle_card(
                y,
                "Simulate a 5-hour limit",
                "On plans without one: a day's share of the week",
                p.simulate_five_hour,
                Action::SimulateFiveHour,
            )?;
            use crate::i18n::Choice;
            // The languages in their own names.
            let options = [
                ("System".to_string(), Action::Language(Choice::System), p.language == Choice::System),
                ("English".to_string(), Action::Language(Choice::English), p.language == Choice::English),
                ("Deutsch".to_string(), Action::Language(Choice::German), p.language == Choice::German),
            ];
            l.select_card(y, "Language", "System: Windows' display language", Select::Language, &options)?;
        }
        page::TASKBAR => {
            // Lower cards than elsewhere: this page has the most of them, and
            // the displays below need the room.
            const H: f32 = TASKBAR_CARD_H;
            use settings::Place;
            let options = [
                ("Tray".to_string(), Action::Place(Place::Tray), m.place == Place::Tray),
                ("Widgets".to_string(), Action::Place(Place::Widgets), m.place == Place::Widgets),
            ];
            let desc = match (m.place, m.widgets_button, position_fit(m)) {
                // English: the card translates it (the status is translated already).
                (Place::Widgets, false, _) => "Switch on Widgets in Windows first".to_string(),
                (_, _, Some(status)) => {
                    l.desc_style = Some(DescStyle { color: tone_color(pal, status.tone), spin: None });
                    status.text
                }
                _ => "Widgets: on its button, apps keep away".to_string(),
            };
            y += l.choice_card_h(y, H, "Position", &desc, &options)?;
            let options = [
                ("Normal".to_string(), Action::Slim(false), !p.slim),
                ("Slim".to_string(), Action::Slim(true), p.slim),
            ];
            y += l.choice_card_h(y, H, "Display mode", "Slim: sessions on top, limits below in turn", &options)?;
            let turn_label = |s: u8| if s == 0 { "Off".to_string() } else { format!("{s} s") };
            let options: Vec<_> =
                settings::TURN_SECS.iter().map(|&s| (turn_label(s), Action::TurnSecs(s), s == p.turn_secs)).collect();
            y += l.choice_card_h(y, H, "Take turns", "Today's limit with the 5-hour one", &options)?;
            let options = [
                ("Automatic".to_string(), Action::Bars(Bars::Auto), m.bars == Bars::Auto),
                ("Always".to_string(), Action::Bars(Bars::Always), m.bars == Bars::Always),
                ("Never".to_string(), Action::Bars(Bars::Never), m.bars == Bars::Never),
            ];
            y += l.choice_card_h(y, H, "Usage bars", "Automatic: only when there's room", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Solid background",
                "Cover taskbar buttons under the widget",
                m.solid,
                Action::Solid,
            )?;
            let options = [
                ("Main".to_string(), Action::Taskbars(Taskbars::Main), m.taskbars == Taskbars::Main),
                ("All".to_string(), Action::Taskbars(Taskbars::All), m.taskbars == Taskbars::All),
                ("Choose".to_string(), Action::Taskbars(Taskbars::Chosen), m.taskbars == Taskbars::Chosen),
            ];
            let desc = if m.more_monitors { "Windows must show taskbars there" } else { "With several displays" };
            y += l.choice_card_h(y, H, "Show on taskbars", desc, &options)?;
            if m.taskbars == Taskbars::Chosen {
                // With many displays the cards get lower (and lose their second
                // line), so all of them fit: the window doesn't scroll.
                // More than fit in one column: two or three side by side.
                let room = HEIGHT - 10.0 - y;
                let fits = |columns: usize| m.displays.len().div_ceil(columns) as f32 * (22.0 + 2.0) <= room;
                let columns = (1..=3).find(|c| fits(*c)).unwrap_or(3);
                let n = m.displays.len().div_ceil(columns) as f32;
                let (h, gap) = if n * (DISPLAY_CARD_H + GAP) > room {
                    ((room / n - 2.0).clamp(22.0, DISPLAY_CARD_H), 2.0)
                } else {
                    (DISPLAY_CARD_H, GAP)
                };
                let full = WIDTH - NAV_W - 2.0 * PAD;
                let col_w = (full - GAP * (columns as f32 - 1.0)) / columns as f32;
                let top = y;
                for (i, (id, name, detail)) in m.displays.iter().enumerate() {
                    let (col, row) = (i % columns, i / columns);
                    l.column = Some((NAV_W + PAD + col as f32 * (col_w + GAP), col_w));
                    let on = m.chosen.contains(id);
                    let detail = if h < 44.0 || columns > 1 { "" } else { detail.as_str() };
                    l.toggle_card_h(top + row as f32 * (h + gap), h, name, detail, on, Action::Display(id.clone()))?;
                }
                l.column = None;
                y = top + n * (h + gap);
            }
            let _ = y;
        }
        page::SYSTEM_MONITOR => {
            use crate::sysmon::{CPU, DISK, GPU, NET, RAM};
            let mon = p.monitor;
            // Off, or one of two ways to show the value.
            let ways = |bit: u8, on: bool, other: bool, a: &str, b: &str| {
                [
                    ("Off".to_string(), Action::MonitorShow(bit, None), !on),
                    (a.to_string(), Action::MonitorShow(bit, Some(false)), on && !other),
                    (b.to_string(), Action::MonitorShow(bit, Some(true)), on && other),
                ]
            };
            let h = DISPLAY_CARD_H;
            let desc = match monitor_fit(m) {
                Some(status) => {
                    l.desc_style = Some(DescStyle { color: tone_color(pal, status.tone), spin: None });
                    status.text
                }
                None => "Your PC's load in the taskbar".to_string(),
            };
            y += l.toggle_card_h(y, h, "System monitor", &desc, mon.on, Action::MonitorOn)?;
            if !mon.on {
                return l.finish();
            }
            y += l.toggle_card_h(y, h, "CPU", "Processor use", mon.cpu, Action::Monitor(CPU))?;
            let options = ways(RAM, mon.ram, mon.ram_gb, "%", "GB");
            y += l.choice_card_h(y, h, "Memory (RAM)", "In use, in percent or GB", &options)?;
            let options = ways(GPU, mon.gpu, mon.gpu_memory, "%", "GB");
            y += l.choice_card_h(y, h, "GPU", "Busiest engine (%) or video memory (GB)", &options)?;
            y += l.toggle_card_h(y, h, "Network", "Sent ↑ and received ↓ per second", mon.net, Action::Monitor(NET))?;
            let options = ways(DISK, mon.disk, mon.disk_rates, "%", "Read/write");
            y += l.choice_card_h(y, h, "Disk", "Busy time (%) or read and write rates", &options)?;
            l.toggle_card_h(
                y,
                DISPLAY_CARD_H,
                "Graphs",
                "The last 30 seconds behind each value",
                mon.graphs,
                Action::MonitorGraphs,
            )?;
        }
        page::SESSIONS => {
            let hooks_desc = match m.tracking {
                tracking::State::Outdated => {
                    l.desc_style = Some(DescStyle { color: pal.warn, spin: None });
                    "Needs repair: switch on to fix"
                }
                _ => "Hooks: instant changes, agent counts",
            };
            y += l.toggle_card(y, "Instant updates", hooks_desc, m.tracking == tracking::State::On, Action::Hooks)?;
            y += l.toggle_card(
                y,
                "Session details",
                "Titles, tokens and cache timers",
                p.transcripts,
                Action::Transcripts,
            )?;
            y += l.toggle_card(y, "Sessions in WSL", "Running WSL distributions only", p.wsl, Action::Wsl)?;
            y += l.toggle_card(
                y,
                "Keep statistics",
                "Even after Claude Code deletes old transcripts",
                p.keep_stats,
                Action::KeepStats,
            )?;
            y += l.toggle_card(
                y,
                "Hide archived sessions",
                "Archived in the extension; shown again when they need you",
                p.hide_archived,
                Action::HideArchived,
            )?;
            let options: Vec<_> = settings::SESSION_POLLS
                .iter()
                .map(|(secs, _)| (format!("{secs}s"), Action::Poll(*secs), *secs == p.session_poll_secs))
                .collect();
            l.choice_card(y, "Check sessions every", "Less often uses less power", &options)?;
        }
        page::NOTIFICATIONS => {
            y += l.toggle_card(y, "Bubble", "Says which session is done or needs you", p.bubbles, Action::Bubbles)?;
            y += l.sound_card(y, "Sound when a session needs you", Kind::NeedsYou, &p.needs_sound)?;
            y += l.sound_card(y, "Sound when a session is done", Kind::Done, &p.done_sound)?;
            // With the merge and pull requests extension on.
            if p.forge {
                y += l.toggle_card(
                    y,
                    "Pipeline bubbles",
                    "When a pipeline ends or a request is merged",
                    p.forge_bubbles,
                    Action::ForgeBubbles,
                )?;
            }
            let options: Vec<_> = settings::VOLUMES
                .iter()
                .map(|(v, label)| (label.to_string(), Action::Volume(*v), *v == p.volume))
                .collect();
            let desc = if p.muted { "Muted: unmute from the right-click menu" } else { "For the built-in sounds" };
            l.choice_card(y, "Volume", desc, &options)?;
        }
        page::MQTT => {
            // The switch says how the connection is; the card below, where to.
            let status = m
                .mqtt_line
                .clone()
                .unwrap_or_else(|| Status::new(Tone::Plain, t("Limits, sessions and events to your broker")));
            l.desc_style = Some(DescStyle { color: tone_color(pal, status.tone), spin: None });
            let status = status.text;
            y += l.toggle_card(y, "MQTT", &status, p.mqtt, Action::Mqtt)?;
            let saved = p.mqtt_broker.as_deref().and_then(|b| crate::mqtt::parse(b).ok());
            if let Some(Form::Broker { tls }) = form {
                y += broker_form(&mut l, y, m, saved.as_ref(), tls)?;
            } else {
                let desc = p
                    .mqtt_broker
                    .clone()
                    .unwrap_or_else(|| t("None yet: Edit to enter it or paste an address").to_string());
                let tls = saved.as_ref().is_some_and(|b| b.tls);
                let mut options = vec![("Edit".to_string(), Action::Edit(Form::Broker { tls }), true)];
                if p.mqtt_broker.is_some() && p.mqtt {
                    options.push(("Test".to_string(), Action::TestMqtt, false));
                }
                if p.mqtt_broker.is_some() {
                    options.push(("Remove".to_string(), Action::RemoveBroker, false));
                }
                y += l.buttons_below_card(y, "Broker", &desc, &options)?;
            }
            let note = l.g.wrapped(
                t("Edit: the broker as Home Assistant's MQTT settings show it (host, port, user name, password). Or copy mqtt://user:password@host (mqtts:// for TLS) and Paste. The password is kept encrypted for your Windows account. Home Assistant finds the entities by itself (MQTT discovery). Only names and states go out, no prompts."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            // Below the form only when there's room.
            if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        page::MAILBOX => {
            y += l.toggle_card(
                y,
                "Mailbox",
                "Mail between Claude sessions in a shared folder",
                p.mailbox,
                Action::Mailbox,
            )?;
            if p.mailbox {
                // How the folder stands: looked for (a turning arrow), in use (a
                // green tick), or missing (yellow, with how to set it).
                let (desc, style) = match (&m.mailbox_note, &m.mailbox_folder) {
                    (Some(note), _) => (note.clone(), DescStyle { color: pal.warn, spin: None }),
                    (None, MailboxFolder::Searching(angle)) => {
                        (t("Looking for it…").to_string(), DescStyle { color: pal.accent, spin: Some(*angle) })
                    }
                    (None, MailboxFolder::Found { path, set }) => {
                        let text = if *set { f("✓ {0}", &[path]) } else { f("✓ Found: {0}", &[path]) };
                        (text, DescStyle { color: pal.done, spin: None })
                    }
                    (None, MailboxFolder::Missing(Some(dir))) => {
                        (f("Not there: {0}", &[dir]), DescStyle { color: pal.warn, spin: None })
                    }
                    (None, MailboxFolder::Missing(None)) => (
                        t("Not found: copy its folder's path, then Paste").to_string(),
                        DescStyle { color: pal.warn, spin: None },
                    ),
                };
                let mut options = vec![("Paste".to_string(), Action::PasteMailbox, false)];
                if p.mailbox_dir.is_some() {
                    options.push(("Find it".to_string(), Action::FindMailbox, false));
                }
                l.desc_style = Some(style);
                y += l.buttons_card(y, "Folder", &desc, &options)?;
                if !m.mailbox_inboxes.is_empty() {
                    y = inbox_list(&mut l, &m.mailbox_inboxes, x, y)?;
                }
            }
            let note = l.g.wrapped(
                t("For sessions that can't message each other, like one in Windows and one in WSL: they leave Markdown messages in each other's inbox folders. The flyout shows what's unread and what was handled; Trayward only reads the folder."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            // Below the inboxes when there's room.
            if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        page::OWN_EXTENSIONS => {
            // Switched on and off on the Extensions page only; here, what's found.
            const H: f32 = COMPACT_CARD_H;
            let snap = &m.extensions;
            let intro = l.g.wrapped(
                t("Programs of your own show what they do in the tooltip and the flyout: each writes a JSON file into this folder. Trayward only reads it; nothing in it is run, opened or sent anywhere."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let Some(c) = c {
                c.text(&intro, x + 4.0, y, pal.text_dim);
            }
            y += intro.height + 12.0;
            let dir = snap.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default();
            let desc = if snap.more > 0 {
                l.desc_style = Some(DescStyle { color: pal.warn, spin: None });
                f("Only the first {0} files (by name) are read", &[&crate::extensions::MAX_FILES])
            } else {
                dir
            };
            let open = [("Open".to_string(), Action::ExtensionsFolder, false)];
            y += l.buttons_card(y, "Extensions folder", &desc, &open)?;
            // Each extension found: its switch by its id (a new one is off), or what's wrong with its file.
            for file in &snap.files {
                l.verbatim = true;
                y += match &file.read {
                    Ok(e) => {
                        let mut desc = format!("{} · {}", e.id, file.name);
                        if !e.description.is_empty() {
                            desc = format!("{desc} · {}", e.description);
                        }
                        let on = p.extensions_on.contains(&e.id);
                        l.toggle_card_h(y, H, &e.label, &desc, on, Action::Extension(e.id.clone()))?
                    }
                    Err(why) => {
                        l.desc_style = Some(DescStyle { color: pal.crit, spin: None });
                        l.card(y, H, &file.name, why, false, 0.0)?;
                        H + GAP
                    }
                };
            }
            if snap.files.is_empty() {
                let note = l.g.wrapped(
                    t("No extension files found yet. The page's \"?\" has the format, and an example."),
                    12.0,
                    NORMAL,
                    WIDTH - x - PAD - 8.0,
                )?;
                if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                    c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
                }
            }
        }
        page::ABOUT => {
            let version = crate::app::VERSION;
            let (desc, mut options) = match &m.update {
                Some(u) => (
                    crate::i18n::f("Version {0} is out", &[&u.version]),
                    vec![("Download".to_string(), Action::Open(u.url.clone()), true)],
                ),
                None => {
                    l.desc_style = Some(DescStyle { color: tone_color(pal, m.update_status.tone), spin: None });
                    (m.update_status.text.clone(), Vec::new())
                }
            };
            options.push(("Check now".to_string(), Action::CheckNow, false));
            y += l.buttons_card(y, &format!("Trayward {version}"), &desc, &options)?;
            y += l.toggle_card(
                y,
                "Check for updates",
                "Once a day, asks GitHub for the newest release",
                p.update_check,
                Action::UpdateCheck,
            )?;
            // The exe's folder (a long one shows in full on hover); Open selects the exe in it.
            let options = [("Open".to_string(), Action::ShowExe, false)];
            y += l.buttons_card(y, "Location", &m.exe_dir, &options)?;
            let repo = crate::update::REPO_URL;
            let options = [("Open".to_string(), Action::Open(AUTHOR_URL.into()), false)];
            y += l.buttons_card(y, &crate::i18n::f("Made by {0}", &[&AUTHOR]), "github.com/TheDelta", &options)?;
            let options = [
                ("Open".to_string(), Action::Open(repo.into()), false),
                ("Report a problem".to_string(), Action::Open(format!("{repo}/issues/new/choose")), false),
                // The licenses of the crates this version is built from, with its release.
                ("Licenses".to_string(), Action::Open(crate::update::licenses_url(crate::app::VERSION)), false),
            ];
            y += l.buttons_card(y, "Source code", "On GitHub · MIT license", &options)?;
            let note = l.g.wrapped(
                t("Trayward is an independent project: not made by, affiliated with or endorsed by Anthropic, OpenAI, GitHub, Microsoft, GitLab or anyone else whose products it works with. Claude, Codex, Copilot and all other names and logos are trademarks of their owners."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let Some(c) = c {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        page::FORGE => {
            const H: f32 = COMPACT_CARD_H;
            let tools = &m.forge_tools;
            let line = forge_status(p.forge, tools);
            l.desc_style = Some(DescStyle { color: tone_color(pal, line.tone), spin: None });
            y += l.toggle_card_h(y, H, "Merge and pull requests", &line.text, p.forge, Action::Forge)?;
            // In the flyout, as bubbles, or both.
            let shows = |label: &str, flyout: bool, bubbles: bool| {
                let chosen = p.forge_flyout == flyout && p.forge_bubbles == bubbles;
                (label.to_string(), Action::ForgeShows { flyout, bubbles }, chosen)
            };
            let options = [shows("Both", true, true), shows("Flyout", true, false), shows("Bubbles", false, true)];
            y += l.choice_card_h(y, H, "Where it shows", "Merge requests and pipelines", &options)?;
            // The two tools: there or not.
            for (forge, tool) in [(crate::forge::Forge::GitLab, &tools.glab), (crate::forge::Forge::GitHub, &tools.gh)]
            {
                let status = tool_status(forge, tool);
                l.desc_style = Some(DescStyle { color: tone_color(pal, status.tone), spin: None });
                l.card(y, H, crate::forge::program(forge), &status.text, false, 0.0)?;
                y += H + GAP;
            }
            // Each host: which login is used there, and Trayward's own token for it.
            let now = crate::win::now_ms() / 1000;
            for (forge, host) in host_rows(tools) {
                let status = match &m.forge_note {
                    Some((name, note)) if *name == host.name => note.clone(),
                    _ => host_status(forge, &host, tools, now),
                };
                let mut buttons =
                    vec![("Paste token".to_string(), Action::PasteToken(forge, host.name.clone()), false)];
                if host.token {
                    buttons.push(("Remove".to_string(), Action::RemoveToken(forge, host.name.clone()), false));
                }
                l.desc_style = Some(DescStyle { color: tone_color(pal, status.tone), spin: None });
                // The host's name as it is, not translated; its "?" says which token it needs.
                l.verbatim = true;
                l.help_topic = Some(match forge {
                    crate::forge::Forge::GitLab => "GitLab token",
                    crate::forge::Forge::GitHub => "GitHub token",
                });
                y += l.buttons_card_h(y, H, &host.name, &status.text, &buttons)?;
            }
        }
        page::AWAY => {
            y += l.toggle_card(
                y,
                "Away mode",
                "While you're away, notifications go to your phone too",
                p.away,
                Action::Away,
            )?;
            let options: Vec<_> = settings::AWAY_MINUTES
                .iter()
                .map(|&m| (format!("{m}m"), Action::AwayAfter(m), m == p.away_minutes))
                .collect();
            y += l.choice_card(y, "Away after", "Minutes without mouse or keyboard", &options)?;
            y += l.toggle_card(y, "Done too", "Not only when a session needs you", p.away_done, Action::AwayDone)?;
            let target = p.push.as_ref();
            let (desc, tone) = match (&m.push_status, target) {
                (Some(s), _) => (s.text.clone(), s.tone),
                // Plain http://: allowed, with a warning.
                (None, Some(c)) if c.insecure() => (
                    f(
                        "{0} · Potentially insecure (http://): use it only on a trusted local network",
                        &[&crate::push::describe(c)],
                    ),
                    Tone::Warn,
                ),
                (None, Some(c)) => (crate::push::describe(c), Tone::Plain),
                (None, None) => (t("None yet: Edit to pick a service or paste an address").to_string(), Tone::Plain),
            };
            l.desc_style = Some(DescStyle { color: tone_color(pal, tone), spin: None });
            if let Some(Form::Push { service }) = form {
                y += push_form(&mut l, y, &desc, service, target)?;
            } else {
                let service = target.map_or(crate::push::Service::Ntfy, crate::push::Config::service);
                let mut options = vec![("Edit".to_string(), Action::Edit(Form::Push { service }), true)];
                if target.is_some() {
                    options.push(("Test".to_string(), Action::TestPush, false));
                    options.push(("Remove".to_string(), Action::RemovePush, false));
                }
                y += l.buttons_below_card(y, "Push service", &desc, &options)?;
            }
            let note = l.g.wrapped(
                t("ntfy, Gotify, a Discord or Slack webhook, or any https:// address that takes JSON (http:// only on a trusted local network). Only the session's name and state go out."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        page::PROVIDERS => {
            // Always on: a card without a switch.
            // Lower cards than elsewhere: up to seven of them.
            const H: f32 = PROVIDERS_CARD_H;
            let desc = if m.claude_found {
                "Always on: limits, sessions and statistics"
            } else {
                "Not used on this PC yet (no ~/.claude)"
            };
            l.card(y, H, "Claude Code", desc, false, 0.0)?;
            y += H + GAP;
            let desc = if m.fable_found {
                "Its own share of the weekly limit, under Weekly"
            } else {
                "Shown when your plan has one"
            };
            y += l.toggle_card_h(y, H, "Fable limit", desc, p.fable, Action::Fable)?;
            let desc = if m.codex_found {
                "OpenAI's Codex CLI: its limits and sessions"
            } else {
                "OpenAI's Codex CLI: not used on this PC yet (no ~/.codex)"
            };
            y += l.toggle_card_h(y, H, "Codex", desc, p.codex, Action::Codex)?;
            if p.codex {
                use settings::Shown;
                let options = [
                    ("All".to_string(), Action::Shows(Shown::Both), p.taskbar_shows == Shown::Both),
                    ("Claude".to_string(), Action::Shows(Shown::Claude), p.taskbar_shows == Shown::Claude),
                    ("Codex".to_string(), Action::Shows(Shown::Codex), p.taskbar_shows == Shown::Codex),
                ];
                y += l.choice_card_h(y, H, "In the taskbar", "Whose limits the widget shows", &options)?;
            }
            let (sessions, limit) = (p.copilot, p.copilot && p.copilot_limit);
            let options = [
                ("Off".to_string(), Action::Copilot(false, false), !sessions),
                ("Sessions".to_string(), Action::Copilot(true, false), sessions && !limit),
                ("With limit".to_string(), Action::Copilot(true, true), limit),
            ];
            let desc = match &m.copilot_error {
                Some(why) if limit => {
                    l.desc_style = Some(DescStyle { color: pal.crit, spin: None });
                    why.as_str()
                }
                _ if m.copilot_found => "Its agent's sessions; the limit through gh",
                _ => "Not used on this PC yet (no ~/.copilot)",
            };
            y += l.choice_card_h(y, H, "Copilot", desc, &options)?;
            let note = l.g.wrapped(
                t("Codex and Copilot are read from their own files, without signing in. Codex's limits are as of its last use. Copilot's limit and merge requests are asked through gh and glab."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            // With every card shown there's no room for it: the cards say enough.
            if y + 8.0 + note.height <= HEIGHT - 8.0 {
                if let Some(c) = c {
                    c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
                }
            }
        }
        page::EXTENSIONS => {
            // Each optional feature: its switch; while on, its page or its part of the settings.
            const H: f32 = COMPACT_CARD_H;
            let mon = p.monitor.on;
            y += l.toggle_card_h(y, H, "System monitor", "Your PC's load in the taskbar", mon, Action::MonitorOn)?;
            // How its two tools stand: there or not, and (switched on) logged in or not.
            let tools = forge_status(p.forge, &m.forge_tools);
            l.desc_style = Some(DescStyle { color: tone_color(pal, tools.tone), spin: None });
            y += l.toggle_card_h(y, H, "Merge and pull requests", &tools.text, p.forge, Action::Forge)?;
            // How git and GnuPG stand: there or not, and (switched on) whether there's a key to watch.
            let gpg = gpg_status(p.gpg_timer, m.gpg_check);
            l.desc_style = Some(DescStyle { color: tone_color(pal, gpg.tone), spin: None });
            y += l.toggle_card_h(y, H, "GPG key timer", &gpg.text, p.gpg_timer, Action::GpgTimer)?;
            y += l.toggle_card_h(
                y,
                H,
                "Away mode",
                "While you're away, notifications go to your phone too",
                p.away,
                Action::Away,
            )?;
            y += l.toggle_card_h(y, H, "MQTT", "Limits, sessions and events to your broker", p.mqtt, Action::Mqtt)?;
            y += l.toggle_card_h(
                y,
                H,
                "Mailbox",
                "Mail between Claude sessions in a shared folder",
                p.mailbox,
                Action::Mailbox,
            )?;
            y += l.toggle_card_h(
                y,
                H,
                "Your custom extensions",
                "What your own programs report, through JSON files",
                p.own_extensions,
                Action::OwnExtensions,
            )?;
            let note = l.g.wrapped(
                t("Switched on, an extension adds its page to the list on the left, or its part of the flyout."),
                12.0,
                NORMAL,
                WIDTH - x - PAD - 8.0,
            )?;
            if let (Some(c), true) = (c, y + 8.0 + note.height <= HEIGHT - 8.0) {
                c.text(&note, x + 4.0, y + 8.0, pal.text_dim);
            }
        }
        page::LIMITS => {
            let label = |v: u8| if v == 0 { "Off".to_string() } else { format!("{v} %") };
            let options: Vec<_> =
                settings::WARN_WEEKLY.iter().map(|&v| (label(v), Action::WarnWeekly(v), v == p.warn_weekly)).collect();
            // Lower cards than elsewhere: this page has the most of them.
            const H: f32 = COMPACT_CARD_H;
            y += l.choice_card_h(y, H, "Weekly limit", "A bubble at this level, and again at 95 %", &options)?;
            let options: Vec<_> = settings::WARN_FIVE_HOUR
                .iter()
                .map(|&v| (label(v), Action::WarnFiveHour(v), v == p.warn_five_hour))
                .collect();
            y += l.choice_card_h(y, H, "5-hour limit", "A bubble at this level", &options)?;
            use crate::forecast::Budget;
            let options = [
                ("Adaptive".to_string(), Action::DailyBudget(Budget::Adaptive), p.daily_budget == Budget::Adaptive),
                ("Even".to_string(), Action::DailyBudget(Budget::Even), p.daily_budget == Budget::Even),
            ];
            y += l.choice_card_h(y, H, "Daily budget", "Adaptive: what a day leaves goes to the next", &options)?;
            use crate::flyout::WeekBar;
            let options = [
                ("Days".to_string(), Action::WeekBar(WeekBar::Days), p.week_bar == WeekBar::Days),
                ("Budgets".to_string(), Action::WeekBar(WeekBar::Budgets), p.week_bar == WeekBar::Budgets),
            ];
            y +=
                l.choice_card_h(y, H, "Weekly bar", "Each day's use of its budget, or the budgets' sizes", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Pace",
                "A bubble when your pace would use up the week before it resets",
                p.warn_pace,
                Action::WarnPace,
            )?;
            let options: Vec<_> = settings::COUNTDOWN_AT
                .iter()
                .map(|&v| {
                    (if v == 100 { "Full".to_string() } else { label(v) }, Action::CountdownAt(v), v == p.countdown_at)
                })
                .collect();
            y += l.choice_card_h(y, H, "Countdown", "Time until the reset instead of %, from", &options)?;
            y += l.toggle_card_h(
                y,
                H,
                "Free again",
                "A bubble when a limit that was at 80 % or more resets",
                p.notify_free,
                Action::NotifyFree,
            )?;
            l.toggle_card_h(
                y,
                H,
                "Weekly summary",
                "A bubble on Monday morning about last week",
                p.week_summary,
                Action::WeekSummary,
            )?;
        }
        _ => {}
    }
    l.finish()
}

/// The broker's card while it's edited: the protocol, address and port, user
/// name and password, as Home Assistant's MQTT settings show them. Starts with
/// the saved broker; the password stays empty (left empty, the saved one is kept).
fn broker_form(l: &mut Layout, y: f32, m: &Model, saved: Option<&crate::mqtt::Broker>, tls: bool) -> Result<f32> {
    use crate::edit::Kind;
    let status = m.mqtt_line.clone().unwrap_or_else(|| Status::new(Tone::Plain, t("Enter saves, Escape cancels")));
    l.desc_style = Some(DescStyle { color: tone_color(l.pal, status.tone), spin: None });
    let desc = status.text;
    let (x, y, w) = l.card(y, BROKER_FORM_H, "Broker", &desc, false, 0.0)?;
    let (cx, cw) = (x + 18.0, w - 36.0);
    let mut ry = y + 58.0;
    let label = l.g.text(t("Protocol"), 12.0, NORMAL, None)?;
    if let Some(c) = l.c {
        c.text(&label, cx + 2.0, ry + (30.0 - label.height) / 2.0, l.pal.text_dim);
    }
    let options = [
        ("mqtt://".to_string(), Action::FormTls(false), !tls),
        ("mqtts:// (TLS)".to_string(), Action::FormTls(true), tls),
    ];
    l.segments(Some(cx + label.width + 14.0), 0.0, ry + 15.0, &options)?;
    ry += 40.0;
    let port_w = 84.0;
    let host = saved.map_or("", |b| b.host.as_str());
    let default_port = if tls { 8883 } else { 1883 };
    let port = saved.map(|b| b.port).filter(|p| *p != default_port).map(|p| p.to_string()).unwrap_or_default();
    l.field(
        cx,
        ry,
        cw - port_w - 12.0,
        "Address",
        Field::MqttHost,
        Kind::Text,
        t("homeassistant.local or 192.168.1.10"),
        host,
    )?;
    let port_hint = default_port.to_string();
    ry += l.field(cx + cw - port_w, ry, port_w, "Port", Field::MqttPort, Kind::Number, &port_hint, &port)? + 8.0;
    let half = (cw - 12.0) / 2.0;
    let user = saved.and_then(|b| b.user.as_deref()).unwrap_or("");
    l.field(cx, ry, half, "User name", Field::MqttUser, Kind::Text, t("None"), user)?;
    let hint = if m.mqtt_password_set { t("Saved: leave empty to keep it") } else { t("None") };
    ry += l.field(cx + half + 12.0, ry, half, "Password", Field::MqttPassword, Kind::Password, hint, "")? + 12.0;
    l.form_buttons(cx, cw, ry + 15.0, "Paste address")?;
    Ok(BROKER_FORM_H + GAP)
}

/// The push setup's card while it's edited: the service, then what it needs.
/// The server starts as saved; secrets (topic, tokens, a webhook's address,
/// which is its secret) start empty, and left empty they stay as saved.
fn push_form(
    l: &mut Layout,
    y: f32,
    desc: &str,
    service: crate::push::Service,
    saved: Option<&crate::push::Config>,
) -> Result<f32> {
    use crate::edit::Kind;
    use crate::push::Service;
    let (x, y, w) = l.card(y, PUSH_FORM_H, "Push service", desc, false, 0.0)?;
    let (cx, cw) = (x + 18.0, w - 36.0);
    let mut ry = y + 58.0;
    let label = l.g.text(t("Service"), 12.0, NORMAL, None)?;
    if let Some(c) = l.c {
        c.text(&label, cx + 2.0, ry + (30.0 - label.height) / 2.0, l.pal.text_dim);
    }
    let options: Vec<_> =
        Service::ALL.iter().map(|&s| (s.name().to_string(), Action::FormService(s), s == service)).collect();
    l.segments(Some(cx + label.width + 14.0), 0.0, ry + 15.0, &options)?;
    ry += 40.0;
    // A saved secret of this service: the field says so instead of showing it.
    let same = saved.filter(|c| c.service() == service);
    let kept = |set: bool, none: &'static str| if set { t("Saved: empty keeps it") } else { t(none) };
    let server = same.and_then(crate::push::Config::server).unwrap_or("");
    match service {
        // In one row: the server, then the topic and token.
        Service::Ntfy => {
            let third = (cw - 24.0) / 3.0;
            l.field(cx, ry, third, "Server", Field::PushServer, Kind::Text, crate::push::NTFY_SERVER, server)?;
            let hint = kept(same.is_some(), "e.g. my-phone-7f3a");
            l.field(cx + third + 12.0, ry, third, "Topic", Field::PushTopic, Kind::Password, hint, "")?;
            let token = matches!(same, Some(crate::push::Config::Ntfy { token: Some(_), .. }));
            let hint = kept(token, "Optional (tk_…)");
            ry += l.field(
                cx + 2.0 * (third + 12.0),
                ry,
                third,
                "Access token",
                Field::PushToken,
                Kind::Password,
                hint,
                "",
            )?;
        }
        Service::Gotify => {
            let half = (cw - 12.0) / 2.0;
            let hint = t("https://gotify.example.org");
            l.field(cx, ry, half, "Server", Field::PushServer, Kind::Text, hint, server)?;
            let hint = kept(same.is_some(), "Gotify → Apps → its token");
            ry += l.field(cx + half + 12.0, ry, half, "App token", Field::PushToken, Kind::Password, hint, "")?;
        }
        Service::Discord | Service::Slack | Service::Webhook => {
            let none = match service {
                Service::Discord => "Discord → channel settings → Integrations → Webhooks",
                Service::Slack => "Slack → Incoming Webhooks: https://hooks.slack.com/…",
                _ => "https://… (it gets JSON: title, message, source)",
            };
            let hint = kept(same.is_some(), none);
            ry += l.field(cx, ry, cw, "Webhook address", Field::PushAddress, Kind::Password, hint, "")?;
        }
    }
    l.form_buttons(cx, cw, ry + 12.0 + 15.0, "Paste address")?;
    Ok(PUSH_FORM_H + GAP)
}

/// Most inboxes the Mailbox page lists.
const INBOX_ROWS: usize = 7;

/// The mailbox's inboxes as a table: inbox, unread (and how long the oldest
/// waited), handled, last handled, whether a session runs for it. Returns where it ends.
fn inbox_list(l: &mut Layout, rows: &[InboxRow], x: f32, y: f32) -> Result<f32> {
    let pal = l.pal;
    let w = WIDTH - x - PAD;
    // Column starts, from the card's left edge.
    let cols = [12.0, w * 0.36, w * 0.56, w * 0.72, w * 0.88];
    let heads = ["Inbox", "Unread", "Handled", "Last handled", "Session"];
    let mut y = y + 2.0;
    if let Some(c) = l.c {
        for (head, cx) in heads.iter().zip(cols) {
            let t = l.g.text(&tr(head), 11.0, NORMAL, None)?;
            c.text(&t, x + cx, y, pal.text_dim);
        }
    }
    y += 18.0;
    let shown = rows.len().min(INBOX_ROWS);
    if let Some(c) = l.c {
        c.fill_round_rect(x, y - 2.0, w, shown as f32 * 22.0 + 4.0, 6.0, pal.card);
    }
    for row in rows.iter().take(INBOX_ROWS) {
        let name = l.g.text(&row.name, 12.5, SEMIBOLD, Some(cols[1] - cols[0] - 8.0))?;
        let unread = match (&row.waiting, row.unread) {
            (_, 0) => "0".to_string(),
            (Some(waited), n) => f("{0} · {1}", &[&n, waited]),
            (None, n) => n.to_string(),
        };
        let unread = l.g.text(&unread, 12.0, if row.unread > 0 { SEMIBOLD } else { NORMAL }, None)?;
        let done = l.g.text(&row.done.to_string(), 12.0, NORMAL, None)?;
        let last = l.g.text(row.last_done.as_deref().map_or("–", |s| s), 12.0, NORMAL, None)?;
        let session = l.g.text(if row.session { t("running") } else { "–" }, 12.0, NORMAL, None)?;
        if let Some(c) = l.c {
            let mid = y + 9.0;
            let at = |t: &gfx::Text| mid - t.height / 2.0;
            c.text(&name, x + cols[0], at(&name), pal.text);
            c.text(&unread, x + cols[1], at(&unread), if row.unread > 0 { pal.warn } else { pal.text_dim });
            c.text(&done, x + cols[2], at(&done), pal.text_dim);
            c.text(&last, x + cols[3], at(&last), pal.text_dim);
            if row.session {
                c.fill_circle(x + cols[4] + 4.0, mid, 3.5, pal.done);
                c.text(&session, x + cols[4] + 12.0, at(&session), pal.text);
            } else {
                c.text(&session, x + cols[4], at(&session), pal.text_dim);
            }
        }
        y += 22.0;
    }
    if rows.len() > INBOX_ROWS {
        let more = l.g.text(&f("{0} more", &[&(rows.len() - INBOX_ROWS)]), 11.0, NORMAL, None)?;
        if let Some(c) = l.c {
            c.text(&more, x + cols[0], y + 4.0, pal.text_dim);
        }
        y += more.height + 4.0;
    }
    Ok(y + 4.0)
}

pub struct SettingsWindow {
    pub hwnd: Option<HWND>,
    pub page: usize,
    model: Option<Model>,
    hits: Vec<Hit>,
    hover: Option<usize>,
    /// The setting being edited in text fields, if any.
    pub form: Option<Form>,
    /// The text fields on screen.
    fields: crate::edit::Fields<Field>,
    /// The descriptions cut short, and the one the mouse is on.
    tips: Vec<Rect>,
    tip: Option<usize>,
    /// Where the mouse is (DIPs).
    mouse: Option<(f32, f32)>,
    /// The open drop-down list, and where its options start in `hits`.
    select: Option<Select>,
    top: Option<usize>,
}

fn scale_of(hwnd: HWND) -> f32 {
    // SAFETY: plain query on our own window.
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

impl SettingsWindow {
    pub const fn new() -> SettingsWindow {
        SettingsWindow {
            hwnd: None,
            page: 0,
            model: None,
            hits: Vec::new(),
            hover: None,
            form: None,
            fields: crate::edit::Fields::new(),
            tips: Vec::new(),
            tip: None,
            mouse: None,
            select: None,
            top: None,
        }
    }

    pub fn visible(&self) -> bool {
        // SAFETY: plain query on our own window.
        self.hwnd.is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
    }

    /// Window size for a client area of WIDTH × HEIGHT DIPs at `dpi`.
    fn window_size(dpi: u32) -> (i32, i32) {
        let s = dpi as f32 / 96.0;
        let mut r = RECT { left: 0, top: 0, right: (WIDTH * s).ceil() as i32, bottom: (HEIGHT * s).ceil() as i32 };
        // SAFETY: valid rectangle.
        let _ = unsafe { AdjustWindowRectExForDpi(&mut r, STYLE, false, WINDOW_EX_STYLE(0), dpi) };
        (r.right - r.left, r.bottom - r.top)
    }

    /// Opens the window (or brings it to the front), centred on the monitor of `anchor`.
    /// The language changed: the title follows it.
    pub fn retitle(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window; the title is copied.
            let _ = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                    hwnd,
                    &windows::core::HSTRING::from(t("Trayward Settings")),
                )
            };
        }
    }

    pub fn open(&mut self, model: Model, anchor: RECT, dpi: u32) {
        if self.hwnd.is_none() {
            // SAFETY: plain window creation with a registered class.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS,
                    &windows::core::HSTRING::from(t("Trayward Settings")),
                    STYLE,
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    None,
                    None,
                )
            };
            self.hwnd = hwnd.ok();
        }
        let Some(hwnd) = self.hwnd else { return };
        // SAFETY: our own window; the title is copied. Set again: the language may have changed.
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                hwnd,
                &windows::core::HSTRING::from(t("Trayward Settings")),
            )
        };
        let was_visible = self.visible();
        self.set_model(model);
        // SAFETY: Win32 calls on our own window with valid out-pointers.
        unsafe {
            if !was_visible {
                let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
                let work = mi.rcWork;
                let (w, h) = Self::window_size(dpi);
                let x = work.left + ((work.right - work.left) - w) / 2;
                let y = work.top + ((work.bottom - work.top) - h) / 2;
                let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    pub fn close(&mut self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
        self.hover = None;
        self.form = None;
        self.select = None;
        self.fields.clear();
    }

    /// Shows new values, repainting only when something changed.
    pub fn set_model(&mut self, model: Model) {
        let Some(hwnd) = self.hwnd else { return };
        if self.model.as_ref() == Some(&model) {
            return;
        }
        // Its extension was switched off: the Extensions page, where that happened.
        if !page_shown(self.page, &model.prefs) {
            self.page = page::EXTENSIONS;
        }
        // SAFETY: attribute value of the documented size; our own window.
        unsafe {
            let dark = BOOL::from(model.dark);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
        }
        self.model = Some(model);
        self.relayout();
    }

    pub fn set_page(&mut self, page: usize) {
        if page < PAGES.len() && page != self.page {
            self.page = page;
            self.hover = None;
            // A form or a list doesn't follow to another page.
            self.form = None;
            self.select = None;
            self.relayout();
        }
    }

    /// Opens a setting's text fields (or another protocol in the broker's), or
    /// closes them.
    pub fn set_form(&mut self, form: Option<Form>) {
        if form != self.form {
            self.form = form;
            self.hover = None;
            self.relayout();
        }
    }

    /// Lays the page out again: its clickable areas and its text fields.
    fn relayout(&mut self) {
        let (Some(hwnd), Some(m)) = (self.hwnd, &self.model) else { return };
        let drawn = render(None, m, self.page, None, self.form, self.select, None).unwrap_or_default();
        self.hits = drawn.hits;
        self.top = drawn.top;
        self.tips = drawn.tips;
        self.tip = self.mouse.and_then(|m| self.tips.iter().position(|r| inside(*r, m)));
        let s = scale_of(hwnd);
        let wanted: Vec<_> = drawn
            .fields
            .into_iter()
            .map(|f| {
                let (x, y, w, h) = f.rect;
                crate::edit::Want { rect: (x * s, y * s, w * s, h * s), ..f }
            })
            .collect();
        let pal = theme::palette(m.dark);
        crate::edit::set_colors(pal.text, pal.window);
        if let Some(first) = self.fields.sync(hwnd, &wanted, (13.0 * s).round() as i32, m.dark) {
            crate::edit::focus(first);
        }
        self.invalidate();
    }

    /// What's typed into a field (empty when it isn't shown).
    pub fn field_text(&self, field: Field) -> String {
        self.fields.text(field).unwrap_or_default()
    }

    /// Puts text into the fields (from a pasted address).
    pub fn fill(&self, values: &[(Field, String)]) {
        for (field, text) in values {
            self.fields.set_text(*field, text);
        }
    }

    fn invalidate(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: our own window.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        }
    }

    fn find(&self, x: i32, y: i32) -> Option<usize> {
        let s = scale_of(self.hwnd?);
        let (x, y) = (x as f32 / s, y as f32 / s);
        match self.top {
            // An open list: only its options.
            Some(top) => crate::hits::find(self.hits.get(top..).unwrap_or_default(), x, y).map(|i| top + i),
            None => crate::hits::find(&self.hits, x, y),
        }
    }

    /// Opens a drop-down list, or closes the open one.
    fn set_select(&mut self, select: Option<Select>) {
        if select != self.select {
            self.select = select;
            self.hover = None;
            self.relayout();
        }
    }

    /// Closes the open drop-down list; false when none was open.
    pub fn close_select(&mut self) -> bool {
        let open = self.select.is_some();
        self.set_select(None);
        open
    }

    pub fn set_mouse(&mut self, pos: Option<(i32, i32)>) {
        let hover = pos.and_then(|(x, y)| self.find(x, y));
        let s = self.hwnd.map_or(1.0, scale_of);
        self.mouse = pos.map(|(x, y)| (x as f32 / s, y as f32 / s));
        let tip = self.mouse.and_then(|m| self.tips.iter().position(|r| inside(*r, m)));
        if hover != self.hover || tip != self.tip {
            self.hover = hover;
            self.tip = tip;
            self.invalidate();
        }
    }

    pub fn hovering(&self) -> bool {
        self.hover.is_some()
    }

    /// What a click at (`x`, `y`) does. A click on a drop-down list opens it;
    /// while it's open, any click closes it, and one on an option picks it.
    pub fn click(&mut self, x: i32, y: i32) -> Option<Action> {
        let action = self.find(x, y).map(|i| self.hits[i].action.clone());
        if self.close_select() {
            return action;
        }
        if let Some(Action::Select(select)) = action {
            self.set_select(Some(select));
            return None;
        }
        action
    }

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
    }

    /// Handles WM_PAINT.
    pub fn paint(&self, hwnd: HWND) {
        // SAFETY: BeginPaint/EndPaint pair on our own window.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            if let Some(m) = &self.model {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let pal = theme::palette(m.dark);
                let bg: Color = pal.window;
                let _ = gfx::paint_window(hwnd, rc.right, rc.bottom, 96.0 * scale_of(hwnd), bg, |c| {
                    let _ = render(Some(c), m, self.page, self.hover, self.form, self.select, self.mouse);
                });
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

/// Asks for a WAV file with the standard Open dialog. Modal: run it outside
/// event handling.
pub fn pick_wav(owner: HWND) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut buf = [0u16; 1024];
    // The filter is "name\0pattern\0\0"; both in the app's language.
    let filter: Vec<u16> = format!("{}\0*.wav\0\0", t("Sounds (*.wav)")).encode_utf16().collect();
    let title: Vec<u16> = t("Choose a sound").encode_utf16().chain(Some(0)).collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: the struct points at `buf`, which outlives the modal call.
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(buf.get(..len).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Choice};

    /// Settings with everything that adds a card: Codex on, chosen displays.
    fn model() -> Model {
        Model {
            dark: true,
            prefs: Prefs {
                forge: true,
                away: true,
                mailbox: true,
                own_extensions: true,
                extensions_on: vec!["ext-0".into()],
                push: crate::push::Config::from_address("https://ntfy.sh/my-phone"),
                mqtt: true,
                mqtt_broker: Some("mqtt://ha@homeassistant.local:1883".into()),
                // On: the page with all its cards.
                monitor: crate::settings::Monitor { on: true, ..Prefs::defaults().monitor },
                ..Prefs::defaults()
            },
            interval: 120,
            bars: Bars::Auto,
            solid: false,
            place: settings::Place::Tray,
            widgets_button: false,
            fit: None,
            exe_dir: r"C:\Users\you\AppData\Local\Programs\Trayward".into(),
            taskbars: Taskbars::Chosen,
            chosen: vec!["a".into()],
            displays: vec![
                ("a".into(), "DELL U2723QE".into(), "Main display · 3840 × 2160".into()),
                ("b".into(), "LG 27GP850".into(), "Right of the main display · 2560 × 1440".into()),
            ],
            more_monitors: true,
            fable_found: true,
            claude_found: true,
            codex_found: false,
            copilot_found: false,
            copilot_error: Some("gh: program not found".into()),
            forge_tools: Default::default(),
            forge_note: None,
            gpg_check: None,
            tracking: tracking::State::Outdated,
            autostart: true,
            push_status: Some(Status::bad("Not sent: The server answered 404")),
            update: Some(crate::update::Release { version: "0.12.0".into(), url: "https://example.com".into() }),
            mqtt_line: Some(Status::bad("Not connected: wrong user name or password")),
            mqtt_password_set: true,
            update_status: Status::new(Tone::Good, "Up to date"),
            mailbox_folder: MailboxFolder::Found { path: r"C:\work\.claude-mailbox".into(), set: false },
            mailbox_note: None,
            mailbox_inboxes: (0..9)
                .map(|i| InboxRow {
                    name: format!("project-number-{i}"),
                    unread: i,
                    waiting: (i > 0).then(|| "14h".into()),
                    done: 26,
                    last_done: Some("2h".into()),
                    session: i % 2 == 0,
                })
                .collect(),
            // As many files as are read, one of them wrong, and more not read.
            extensions: crate::extensions::Snapshot {
                dir: Some(r"C:\Users\someone\AppData\Local\Trayward\extensions".into()),
                files: (0..crate::extensions::MAX_FILES)
                    .map(|i| crate::extensions::File {
                        name: format!("extension-{i}.json"),
                        read: if i == 3 {
                            Err(r#""tooltip" row 2: "value" must be a time like 2026-11-18T23:00:00+01:00"#.into())
                        } else {
                            crate::extensions::parse(
                                &format!(
                                    r#"{{"version":1,"id":"ext-{i}","label":"Extension {i}","description":"Nightly backup of the NAS"}}"#
                                ),
                                0,
                            )
                        },
                    })
                    .collect(),
                more: 2,
            },
        }
    }

    /// Texts of the settings that read the same in German.
    const SAME_IN_GERMAN: &[&str] = &[
        "System",
        "English",
        "Deutsch",
        "MQTT",
        "glab",
        "gh",
        "Flyout",
        "Claude",
        "Claude Code",
        "Codex",
        "Copilot",
        "GB",
        "%",
        "github.com/TheDelta",
    ];

    #[test]
    fn says_how_git_and_gpg_stand() {
        use crate::gpg::{Check, Missing};
        i18n::set(Choice::English);
        let s = |on, check| {
            let s = gpg_status(on, check);
            (s.tone, s.text)
        };
        // Switched off, nothing is run: only whether they're there.
        let found = Some(Check::Found { git: true, gpg: true });
        assert_eq!(s(false, found), (Tone::Plain, "How long your key stays unlocked · GnuPG found".into()));
        let none = Some(Check::Found { git: false, gpg: false });
        let text = "How long your key stays unlocked · GnuPG not found · git not found";
        assert_eq!(s(false, none), (Tone::Plain, text.into()));
        assert_eq!(s(false, None).1, "How long your key stays unlocked");
        // Just switched on: what was found while off says nothing about the key yet.
        assert_eq!(s(true, found).0, Tone::Busy);
        assert_eq!(s(true, None).0, Tone::Busy);
        // What's missing is said in yellow, a key that's watched in green.
        let missing = s(true, Some(Check::Missing(Missing::NotSigning)));
        assert_eq!(missing, (Tone::Warn, "git doesn't sign commits (commit.gpgsign is off)".into()));
        let ready = s(true, Some(Check::Ready { running: true, cached: false }));
        assert_eq!(ready, (Tone::Good, "✓ git signs with GnuPG · key locked".into()));
    }

    #[test]
    fn says_which_login_a_host_uses() {
        use crate::forge::{Forge, Host, Tool, Tools};
        i18n::set(Choice::English);
        let now = 1_800_000_000;
        let tools = Tools { glab: Tool { found: true, ..Default::default() }, gh: Tool::default(), asked: true };
        let host = |token, ok, expires| Host { name: "gitlab.example.com".into(), ok, token, expires };
        let say = |h: &Host| {
            let s = host_status(Forge::GitLab, h, &tools, now);
            (s.tone, s.text)
        };
        assert_eq!(say(&host(false, true, None)), (Tone::Good, "✓ glab's own login".into()));
        assert_eq!(say(&host(false, false, None)), (Tone::Warn, "Not logged in: paste a token".into()));
        assert_eq!(say(&host(true, true, None)), (Tone::Good, "✓ Trayward's token".into()));
        assert_eq!(say(&host(true, false, None)).0, Tone::Warn);
        // In yellow from 14 days before it runs out.
        assert_eq!(say(&host(true, true, Some(now + 100 * 86_400))).0, Tone::Good);
        assert_eq!(say(&host(true, true, Some(now + 13 * 86_400))).0, Tone::Warn);
        // gh isn't there: its hosts say so.
        let github = Host { name: "github.com".into(), ..Default::default() };
        assert_eq!(host_status(Forge::GitHub, &github, &tools, now).text, "gh not found");
        // github.com is always listed, for a token before gh is there; at most a page full.
        let rows = host_rows(&tools);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0, rows[0].1.name.as_str()), (Forge::GitHub, "github.com"));
        let many =
            Tool { found: true, hosts: (0..9).map(|i| host(false, true, Some(i))).collect(), ..Default::default() };
        let rows = host_rows(&Tools { glab: many, ..tools.clone() });
        assert_eq!(rows.len(), HOST_ROWS);
        // github.com and hosts with Trayward's token are never left out.
        assert_eq!((rows[2].0, rows[2].1.name.as_str()), (Forge::GitHub, "github.com"));
        let mut kept =
            Tool { found: true, hosts: (0..9).map(|i| host(false, true, Some(i))).collect(), ..Default::default() };
        kept.hosts[7] = Host { name: "kept.example.com".into(), ..host(true, true, None) };
        let rows = host_rows(&Tools { glab: kept, ..tools.clone() });
        let names: Vec<&str> = rows.iter().map(|(_, h)| h.name.as_str()).collect();
        assert_eq!(names.len(), HOST_ROWS);
        assert!(names.contains(&"kept.example.com") && names.contains(&"github.com"), "{names:?}");
        // Never more than fit, tokens first: four hosts with a token show three.
        let tokens =
            Tool { found: true, hosts: (0..4).map(|i| host(true, true, Some(i))).collect(), ..Default::default() };
        let rows = host_rows(&Tools { glab: tokens, ..tools.clone() });
        assert_eq!(rows.len(), HOST_ROWS);
        assert!(rows.iter().all(|(_, h)| h.token));
        // A tool's card: how to install it, in yellow only when a host needs it.
        assert_eq!(tool_status(Forge::GitHub, &Tool::default()).text, "Not found: winget install --id GitHub.cli");
        assert_eq!(tool_status(Forge::GitHub, &Tool::default()).tone, Tone::Plain);
        let found = Tool {
            found: true,
            version: Some("glab 1.52.0".into()),
            path: Some(r"C:\glab.exe".into()),
            ..Default::default()
        };
        assert_eq!(tool_status(Forge::GitLab, &found).text, r"✓ glab 1.52.0 · C:\glab.exe");
    }

    #[test]
    fn says_how_glab_and_gh_stand() {
        use crate::forge::{Tool, Tools};
        let tool = |found: bool, hosts: &[(&str, bool)]| Tool {
            found,
            hosts: hosts
                .iter()
                .map(|(h, ok)| crate::forge::Host { name: h.to_string(), ok: *ok, ..Default::default() })
                .collect(),
            ..Default::default()
        };
        // Switched off, neither is run: only whether they're there.
        let off = Tools { glab: tool(true, &[]), gh: tool(false, &[]), asked: false };
        let s = forge_status(false, &off);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Plain, "GitLab and GitHub · glab found · gh not found"));
        // Just switched on: not asked yet.
        assert_eq!(forge_status(true, &off).tone, Tone::Busy);
        // Both logged in where the sessions' projects are.
        let fine = Tools {
            glab: tool(true, &[("gitlab.example.com", true)]),
            gh: tool(true, &[("github.com", true)]),
            asked: true,
        };
        let s = forge_status(true, &fine);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Good, "✓ glab: gitlab.example.com · ✓ gh: github.com"));
        // A tool nobody needs may be missing; one that is needed, or has no login, is said in yellow.
        let unused = Tools { gh: tool(false, &[]), ..fine.clone() };
        assert_eq!(forge_status(true, &unused).tone, Tone::Good);
        let needed = Tools { gh: tool(false, &[("github.com", false)]), ..fine.clone() };
        let s = forge_status(true, &needed);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Warn, "✓ glab: gitlab.example.com · gh not found"));
        let locked = Tools { glab: tool(true, &[("a.example", true), ("b.example", false)]), ..fine };
        let s = forge_status(true, &locked);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Warn, "glab: not logged in to b.example · ✓ gh: github.com"));
    }

    #[test]
    fn many_displays_fit() {
        i18n::set(Choice::English);
        let page = PAGES.iter().position(|(name, _)| *name == "Taskbar").unwrap();
        for n in [3, 4, 6, 8, 12] {
            let displays = (0..n).map(|i| (format!("d{i}"), format!("Display {i}"), "Somewhere · 2560 × 1440".into()));
            let m = Model { displays: displays.collect(), ..model() };
            let hits = render(None, &m, page, None, None, None, None).unwrap().hits;
            let bottom = hits.iter().map(|h| h.y + h.h).fold(0.0, f32::max);
            assert!(bottom <= HEIGHT, "{n} displays end at {bottom}, the window at {HEIGHT}");
            // Each display can still be switched.
            let switches = hits.iter().filter(|h| matches!(h.action, Action::Display(_))).count();
            assert_eq!(switches, n);
            gfx::take_cut();
        }
    }

    #[test]
    fn the_forms_fit_with_their_fields() {
        let page = |name: &str| PAGES.iter().position(|(n, _)| *n == name).unwrap();
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            for (name, form, fields) in [
                ("MQTT", Form::Broker { tls: true }, 4),
                ("Away", Form::Push { service: crate::push::Service::Ntfy }, 3),
                ("Away", Form::Push { service: crate::push::Service::Gotify }, 2),
                ("Away", Form::Push { service: crate::push::Service::Discord }, 1),
                ("MQTT", Form::Broker { tls: false }, 4),
            ] {
                let drawn = render(None, &model(), page(name), None, Some(form), None, None).unwrap();
                assert!(gfx::take_cut().is_empty(), "{lang:?}, {name}: cut off");
                assert_eq!(drawn.fields.len(), fields, "{name}");
                let bottom = drawn.hits.iter().map(|h| h.y + h.h).fold(0.0, f32::max);
                let fields_end = drawn.fields.iter().map(|f| f.rect.1 + f.rect.3).fold(0.0, f32::max);
                assert!(bottom.max(fields_end) <= HEIGHT, "{lang:?}, {name}: ends at {bottom}");
                for action in [Action::SaveForm, Action::CancelForm, Action::PasteForm] {
                    assert!(drawn.hits.iter().any(|h| h.action == action), "{name}: {action:?}");
                }
            }
            // Closed, no fields; the saved broker's parts start the form.
            assert!(render(None, &model(), page("MQTT"), None, None, None, None).unwrap().fields.is_empty());
            let drawn =
                render(None, &model(), page("MQTT"), None, Some(Form::Broker { tls: false }), None, None).unwrap();
            let text = |id| drawn.fields.iter().find(|f| f.id == id).map(|f| f.text.clone());
            assert_eq!(text(Field::MqttHost).as_deref(), Some("homeassistant.local"));
            assert_eq!(text(Field::MqttPort).as_deref(), Some(""));
            assert_eq!(text(Field::MqttUser).as_deref(), Some("ha"));
            assert_eq!(text(Field::MqttPassword).as_deref(), Some(""));
        }
    }

    #[test]
    fn a_hosts_help_says_which_token_it_needs() {
        i18n::set(Choice::English);
        let drawn = render(None, &model(), page::FORGE, None, None, None, None).unwrap();
        let links: Vec<String> = drawn
            .hits
            .iter()
            .filter_map(|h| match &h.action {
                Action::Open(url) => url.split_once('#').map(|(_, anchor)| anchor.to_string()),
                _ => None,
            })
            .collect();
        // The page, the switch, where it shows, the two tools, and github.com's card: its token.
        assert_eq!(
            links,
            ["gitlab-and-github-page", "merge-and-pull-requests", "where-it-shows", "glab", "gh", "github-token"]
        );
    }

    #[test]
    fn a_description_cut_short_shows_in_full_on_hover() {
        i18n::set(Choice::English);
        let page = PAGES.iter().position(|(n, _)| *n == "MQTT").unwrap();
        let long = "Not connected: No connection could be made because the target machine actively refused it.";
        let m = Model { mqtt_line: Some(Status::bad(long)), ..model() };
        let drawn = render(None, &m, page, None, None, None, None).unwrap();
        gfx::take_cut();
        // The MQTT card's status is cut short: hovering it shows all of it.
        assert!(!drawn.tips.is_empty());
        let (x, y, w, h) = drawn.tips[0];
        assert!(drawn.hits.iter().any(|hit| inside((hit.x, hit.y, hit.w, hit.h), (x + w / 2.0, y + h / 2.0))));
        // A short one isn't.
        let m = Model { mqtt_line: Some(Status::new(Tone::Good, "Connected")), ..model() };
        assert!(render(None, &m, page, None, None, None, None).unwrap().tips.is_empty());
    }

    #[test]
    fn every_page_fits_in_both_languages() {
        let code = include_str!("settings_window.rs").split("#[cfg(test)]\nmod tests").next().unwrap_or_default();
        let mut untranslated = Vec::new();
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            for (page, (name, _)) in PAGES.iter().enumerate() {
                i18n::take_untranslated();
                let hits = render(None, &model(), page, None, None, None, None).unwrap().hits;
                let cut = gfx::take_cut();
                assert!(cut.is_empty(), "{lang:?}, {name}: cut off: {cut:?}");
                // Card titles, descriptions and choices aren't literals for the
                // scan of the code: what's shown in German must be German, but
                // for what is the same in both (names, numbers, units).
                // Only texts written in this file: values (names, versions, messages) come from elsewhere.
                let mut english: Vec<String> = i18n::take_untranslated();
                english.retain(|s| !SAME_IN_GERMAN.contains(&s.as_str()) && code.contains(&format!("{s:?}")));
                if lang == Choice::German {
                    untranslated.extend(english.into_iter().map(|s| format!("{name}: {s}")));
                }
                // Every control is inside the window.
                let bottom = hits.iter().map(|h| h.y + h.h).fold(0.0, f32::max);
                assert!(bottom <= HEIGHT, "{lang:?}, {name}: ends at {bottom}, the window at {HEIGHT}");
            }
        }
        untranslated.sort();
        untranslated.dedup();
        assert!(
            untranslated.is_empty(),
            "no German translation for:
{}",
            untranslated.join(
                "
"
            )
        );
    }

    /// Lays out and draws a page into memory, as the window paints it. What's drawn
    /// has the same clickable areas and text fields as the layout without drawing.
    fn draw(
        m: &Model,
        page: usize,
        hover: Option<usize>,
        form: Option<Form>,
        select: Option<Select>,
        mouse: Option<(f32, f32)>,
    ) -> Drawn {
        let laid_out = render(None, m, page, hover, form, select, mouse).unwrap();
        let mut drawn = None;
        let pixels = gfx::testing::offscreen(WIDTH as i32, HEIGHT as i32, 96.0, |c| {
            drawn = Some(render(Some(c), m, page, hover, form, select, mouse).unwrap());
        })
        .unwrap();
        assert!(gfx::testing::drew(&pixels), "page {page}: nothing drawn");
        gfx::take_cut();
        let drawn = drawn.unwrap();
        let actions = |d: &Drawn| d.hits.iter().map(|h| h.action.clone()).collect::<Vec<_>>();
        assert_eq!(actions(&drawn), actions(&laid_out), "page {page}");
        assert_eq!(drawn.fields.len(), laid_out.fields.len(), "page {page}");
        assert_eq!(drawn.top, laid_out.top, "page {page}");
        drawn
    }

    /// Every page shown, by its index.
    fn pages(m: &Model) -> Vec<usize> {
        (0..PAGES.len()).filter(|&p| page_shown(p, &m.prefs)).collect()
    }

    /// Hosts of both tools: with Trayward's token (one running out soon), with
    /// the tool's own login, without any.
    fn tools() -> crate::forge::Tools {
        use crate::forge::{Host, Tool, Tools};
        let now = crate::win::now_ms() / 1000;
        let host = |name: &str, ok, token, expires| Host { name: name.into(), ok, token, expires };
        Tools {
            glab: Tool {
                found: true,
                path: Some(r"C:\Tools\glab.exe".into()),
                version: Some("glab 1.52.0".into()),
                hosts: vec![
                    host("gitlab.example.com", true, true, Some(now + 200 * 86_400)),
                    host("git.example.org", true, true, Some(now + 3 * 86_400)),
                ],
            },
            gh: Tool { found: true, path: None, version: None, hosts: vec![host("github.com", false, false, None)] },
            asked: true,
        }
    }

    #[test]
    fn every_page_is_drawn_in_both_themes_and_languages() {
        for lang in [Choice::English, Choice::German] {
            i18n::set(lang);
            for dark in [true, false] {
                let m = Model { dark, ..model() };
                for page in pages(&m) {
                    draw(&m, page, None, None, None, None);
                }
            }
        }
        i18n::set(Choice::English);
    }

    #[test]
    fn every_control_is_drawn_hovered() {
        i18n::set(Choice::English);
        let m = model();
        for page in pages(&m) {
            let hits = render(None, &m, page, None, None, None, None).unwrap().hits.len();
            for hover in 0..hits {
                draw(&m, page, Some(hover), None, None, None);
            }
        }
        // The forms' controls too.
        for (page, form) in [
            (page::MQTT, Form::Broker { tls: false }),
            (page::AWAY, Form::Push { service: crate::push::Service::Ntfy }),
        ] {
            let hits = render(None, &m, page, None, Some(form), None, None).unwrap().hits.len();
            for hover in 0..hits {
                draw(&m, page, Some(hover), Some(form), None, None);
            }
        }
    }

    #[test]
    fn a_hovered_help_shows_its_short_text() {
        i18n::set(Choice::English);
        let m = model();
        for page in pages(&m) {
            let drawn = render(None, &m, page, None, None, None, None).unwrap();
            // Each "?": a link to its part of the help page, its short text on hover.
            for (i, hit) in drawn.hits.iter().enumerate() {
                if matches!(&hit.action, Action::Open(url) if url.contains('#')) {
                    let pixels = gfx::testing::offscreen(WIDTH as i32, HEIGHT as i32, 96.0, |c| {
                        render(Some(c), &m, page, Some(i), None, None, None).unwrap();
                    })
                    .unwrap();
                    let plain = gfx::testing::offscreen(WIDTH as i32, HEIGHT as i32, 96.0, |c| {
                        render(Some(c), &m, page, None, None, None, None).unwrap();
                    })
                    .unwrap();
                    assert_ne!(pixels, plain, "page {page}, hit {i}: no help shown");
                    gfx::take_cut();
                }
            }
        }
    }

    #[test]
    fn the_language_list_opens_over_everything() {
        i18n::set(Choice::English);
        let m = model();
        let drawn = draw(&m, page::GENERAL, None, None, Some(Select::Language), None);
        // Its options come last, the only ones a click reaches.
        let top = drawn.top.unwrap();
        let options: Vec<_> = drawn.hits.get(top..).unwrap().iter().map(|h| h.action.clone()).collect();
        use crate::i18n::Choice as Language;
        assert_eq!(options, [Language::System, Language::English, Language::German].map(Action::Language).to_vec());
        // Each option hovered, and the chosen one.
        for hover in top..drawn.hits.len() {
            draw(&m, page::GENERAL, Some(hover), None, Some(Select::Language), None);
        }
        let german = Model { prefs: Prefs { language: Language::German, ..model().prefs }, ..model() };
        draw(&german, page::GENERAL, None, None, Some(Select::Language), None);
        // Closed: no options on top.
        assert!(draw(&m, page::GENERAL, None, None, None, None).top.is_none());
    }

    #[test]
    fn a_description_cut_short_is_drawn_in_full_on_hover() {
        i18n::set(Choice::English);
        let long = "Not connected: No connection could be made because the target machine actively refused it, \
                    again and again, for as long as anyone cared to try.";
        let m = Model { mqtt_line: Some(Status::bad(long)), ..model() };
        for form in [None, Some(Form::Broker { tls: true })] {
            let drawn = render(None, &m, page::MQTT, None, form, None, None).unwrap();
            gfx::take_cut();
            let &(x, y, w, h) = drawn.tips.first().unwrap();
            draw(&m, page::MQTT, None, form, None, Some((x + w / 2.0, y + h / 2.0)));
        }
        // On a card low on the page: above it.
        let ext = Model {
            forge_tools: crate::forge::Tools {
                asked: true,
                glab: crate::forge::Tool { found: true, ..Default::default() },
                gh: crate::forge::Tool {
                    found: true,
                    hosts: (0..12)
                        .map(|i| crate::forge::Host { name: format!("host-{i}.example.com"), ..Default::default() })
                        .collect(),
                    ..Default::default()
                },
            },
            gpg_check: Some(crate::gpg::Check::Missing(crate::gpg::Missing::OtherFormat)),
            prefs: Prefs { gpg_timer: true, ..model().prefs },
            ..model()
        };
        let drawn = render(None, &ext, page::EXTENSIONS, None, None, None, None).unwrap();
        gfx::take_cut();
        for &(x, y, w, h) in &drawn.tips {
            draw(&ext, page::EXTENSIONS, None, None, None, Some((x + w / 2.0, y + h / 2.0)));
        }
        assert!(!drawn.tips.is_empty());
    }

    /// The widget on the Widgets button without the bars, the network and the disk.
    fn short() -> Option<crate::widget::Fit> {
        let values = vec!["↑", "↓", "R", "W"];
        Some(crate::widget::Fit { room: 180, full: 262, bars: true, values, slim: false })
    }

    #[test]
    fn says_what_the_widgets_button_has_no_room_for() {
        i18n::set(Choice::English);
        let fit = short().unwrap();
        let s = fit_status(&fit);
        assert_eq!(s.tone, Tone::Info);
        let text =
            "Room for 180 of 262 px on the Widgets button: without usage bars, network, disk · Tray shows all of it";
        assert_eq!(s.text, text);
        let slim = fit_status(&crate::widget::Fit { slim: true, ..fit });
        assert_eq!(slim.text, "Room for 180 of 262 px on the Widgets button: slim instead · Tray shows all of it");
        // On the Widgets button, both cards say it...
        let button = Model { place: settings::Place::Widgets, widgets_button: true, fit: short(), ..model() };
        assert!(position_fit(&button).is_some() && monitor_fit(&button).is_some());
        // ...not in the tray, and the monitor's card not about the bars alone, or while it's off.
        let tray = Model { place: settings::Place::Tray, ..button.clone() };
        assert!(position_fit(&tray).is_none() && monitor_fit(&tray).is_none());
        let bars = Model { fit: short().map(|f| crate::widget::Fit { values: Vec::new(), ..f }), ..button.clone() };
        assert!(position_fit(&bars).is_some() && monitor_fit(&bars).is_none());
        let mut off = button.clone();
        off.prefs.monitor.on = false;
        assert!(monitor_fit(&off).is_none());
    }

    #[test]
    fn each_state_of_the_cards_is_drawn() {
        use crate::gpg::{Check, Missing};
        use crate::push::{Config, Service};
        i18n::set(Choice::English);
        let base = model();
        let prefs = |p: Prefs| Model { prefs: p, ..model() };
        let variants: Vec<(usize, Model)> = vec![
            // General and Taskbar.
            (page::GENERAL, Model { autostart: false, interval: 600, ..model() }),
            (page::TASKBAR, Model { place: settings::Place::Widgets, widgets_button: false, ..model() }),
            (page::TASKBAR, Model { place: settings::Place::Widgets, widgets_button: true, ..model() }),
            // Not all of it fits onto the Widgets button: said on both pages.
            (page::TASKBAR, Model { place: settings::Place::Widgets, widgets_button: true, fit: short(), ..model() }),
            (page::SYSTEM_MONITOR, Model { place: settings::Place::Widgets, fit: short(), ..model() }),
            (
                page::SYSTEM_MONITOR,
                Model {
                    place: settings::Place::Widgets,
                    fit: short().map(|f| crate::widget::Fit { slim: true, ..f }),
                    ..model()
                },
            ),
            (
                page::TASKBAR,
                Model { taskbars: Taskbars::All, more_monitors: false, bars: Bars::Never, solid: true, ..model() },
            ),
            (page::TASKBAR, Model { taskbars: Taskbars::Main, bars: Bars::Always, ..model() }),
            (page::TASKBAR, prefs(Prefs { slim: true, turn_secs: 0, ..base.prefs.clone() })),
            // The system monitor: off, and every value in its other way.
            (
                page::SYSTEM_MONITOR,
                prefs(Prefs {
                    monitor: settings::Monitor {
                        on: true,
                        cpu: true,
                        ram: true,
                        gpu: true,
                        net: true,
                        disk: true,
                        graphs: false,
                        ram_gb: true,
                        gpu_memory: true,
                        disk_rates: true,
                    },
                    ..base.prefs.clone()
                }),
            ),
            (
                page::EXTENSIONS,
                prefs(Prefs {
                    monitor: settings::Monitor { on: false, ..base.prefs.monitor },
                    forge: false,
                    gpg_timer: true,
                    ..base.prefs.clone()
                }),
            ),
            // Sessions: the hooks on, and off.
            (page::SESSIONS, Model { tracking: tracking::State::On, ..model() }),
            (
                page::SESSIONS,
                Model {
                    tracking: tracking::State::Off,
                    prefs: Prefs { transcripts: false, wsl: false, session_poll_secs: 10, ..base.prefs.clone() },
                    ..model()
                },
            ),
            // Notifications: no sound, a file of one's own, muted, without merge requests.
            (
                page::NOTIFICATIONS,
                prefs(Prefs {
                    needs_sound: SoundChoice::Off,
                    done_sound: SoundChoice::File(r"C:\Sounds\done.wav".into()),
                    muted: true,
                    forge: false,
                    volume: settings::Volume::Loud,
                    ..base.prefs.clone()
                }),
            ),
            (
                page::NOTIFICATIONS,
                prefs(Prefs {
                    needs_sound: SoundChoice::Builtin(3),
                    done_sound: SoundChoice::File("done.wav".into()),
                    ..base.prefs.clone()
                }),
            ),
            // Limits, other choices.
            (
                page::LIMITS,
                prefs(Prefs {
                    warn_weekly: 0,
                    countdown_at: 100,
                    daily_budget: crate::forecast::Budget::Even,
                    week_bar: crate::flyout::WeekBar::Budgets,
                    ..base.prefs.clone()
                }),
            ),
            // Providers: nothing found, Codex on with its choice, Copilot's limit failing.
            (
                page::PROVIDERS,
                Model {
                    claude_found: false,
                    fable_found: false,
                    codex_found: true,
                    copilot_found: true,
                    copilot_error: None,
                    prefs: Prefs { codex: false, copilot: false, ..base.prefs.clone() },
                    ..model()
                },
            ),
            (
                page::PROVIDERS,
                Model {
                    prefs: Prefs {
                        codex: true,
                        copilot: true,
                        copilot_limit: true,
                        taskbar_shows: settings::Shown::Codex,
                        ..base.prefs.clone()
                    },
                    ..model()
                },
            ),
            (
                page::PROVIDERS,
                Model {
                    copilot_found: true,
                    prefs: Prefs { codex: false, copilot: true, copilot_limit: false, ..base.prefs.clone() },
                    ..model()
                },
            ),
            // MQTT: nothing set yet, switched off, connected.
            (
                page::MQTT,
                Model {
                    mqtt_line: None,
                    mqtt_password_set: false,
                    prefs: Prefs { mqtt_broker: None, ..base.prefs.clone() },
                    ..model()
                },
            ),
            (
                page::MQTT,
                Model {
                    mqtt_line: Some(Status::new(Tone::Good, "✓ Connected to homeassistant.local")),
                    prefs: Prefs { mqtt_broker: Some("mqtts://homeassistant.local:8884".into()), ..base.prefs.clone() },
                    ..model()
                },
            ),
            // Away: an http:// address, an https:// one, none, a test under way.
            (
                page::AWAY,
                Model {
                    push_status: None,
                    prefs: Prefs { push: Config::from_address("http://192.168.1.10/hook"), ..base.prefs.clone() },
                    ..model()
                },
            ),
            (page::AWAY, Model { push_status: None, ..model() }),
            (page::AWAY, Model { push_status: None, prefs: Prefs { push: None, ..base.prefs.clone() }, ..model() }),
            (
                page::AWAY,
                Model {
                    push_status: Some(Status::new(Tone::Busy, "Sending…")),
                    prefs: Prefs { push: None, away_done: false, away_minutes: 30, ..base.prefs.clone() },
                    ..model()
                },
            ),
            // Mailbox: each state of its folder, few inboxes, none, switched off.
            (page::MAILBOX, Model { mailbox_note: Some("Not a folder: x".into()), ..model() }),
            (page::MAILBOX, Model { mailbox_folder: MailboxFolder::Searching(90), mailbox_inboxes: vec![], ..model() }),
            (
                page::MAILBOX,
                Model {
                    mailbox_folder: MailboxFolder::Found { path: r"W:\dev\.mailbox".into(), set: true },
                    prefs: Prefs { mailbox_dir: Some(r"W:\dev\.mailbox".into()), ..base.prefs.clone() },
                    // Unread, without knowing for how long.
                    mailbox_inboxes: vec![InboxRow { waiting: None, unread: 3, ..base.mailbox_inboxes[1].clone() }],
                    ..model()
                },
            ),
            (page::MAILBOX, Model { mailbox_folder: MailboxFolder::Missing(Some(r"X:\gone".into())), ..model() }),
            (page::MAILBOX, Model { mailbox_folder: MailboxFolder::Missing(None), ..model() }),
            (page::EXTENSIONS, prefs(Prefs { mailbox: false, ..base.prefs.clone() })),
            // The user's own extensions: none yet, and a few.
            (page::OWN_EXTENSIONS, Model { extensions: crate::extensions::Snapshot::default(), ..model() }),
            (
                page::OWN_EXTENSIONS,
                Model {
                    // One without a description.
                    extensions: crate::extensions::Snapshot {
                        more: 0,
                        files: vec![crate::extensions::File {
                            name: "plain.json".into(),
                            read: crate::extensions::parse(r#"{"version":1,"id":"plain","label":"Plain"}"#, 0),
                        }],
                        ..base.extensions.clone()
                    },
                    ..model()
                },
            ),
            // About: no newer release, in each tone of the check.
            (page::ABOUT, Model { update: None, ..model() }),
            (page::ABOUT, Model { update: None, update_status: Status::new(Tone::Busy, "Checking…"), ..model() }),
            (page::ABOUT, Model { update: None, update_status: Status::bad("Couldn't ask GitHub"), ..model() }),
            (page::ABOUT, Model { update: None, update_status: Status::new(Tone::Plain, ""), ..model() }),
            // GitLab and GitHub: hosts with and without tokens, a note on one, the tools missing.
            (page::FORGE, Model { forge_tools: tools(), ..model() }),
            (
                page::FORGE,
                Model {
                    forge_tools: tools(),
                    forge_note: Some(("gitlab.example.com".into(), Status::new(Tone::Busy, "Token saved, checking…"))),
                    ..model()
                },
            ),
            (
                page::FORGE,
                Model {
                    forge_tools: crate::forge::Tools {
                        asked: false,
                        glab: crate::forge::Tool { hosts: tools().glab.hosts, ..Default::default() },
                        ..Default::default()
                    },
                    ..model()
                },
            ),
            (page::FORGE, prefs(Prefs { forge_flyout: false, forge_bubbles: true, ..base.prefs.clone() })),
            (page::EXTENSIONS, Model { gpg_check: Some(Check::Ready { running: true, cached: true }), ..model() }),
            (page::EXTENSIONS, Model { gpg_check: Some(Check::Missing(Missing::Key)), ..model() }),
        ];
        for (page, m) in &variants {
            for dark in [true, false] {
                draw(&Model { dark, ..m.clone() }, *page, None, None, None, None);
            }
        }
        // The forms: each service, with and without what's saved for it.
        let ntfy_token =
            Config::Ntfy { server: "https://ntfy.example.org".into(), topic: "t".into(), token: Some("tk_x".into()) };
        let gotify = Config::Gotify { server: "https://gotify.example.org".into(), token: "A1".into() };
        let slack = Config::Slack { address: "https://hooks.slack.com/services/x".into() };
        for saved in [Some(ntfy_token), Some(gotify), Some(slack), None] {
            let m = prefs(Prefs { push: saved, ..base.prefs.clone() });
            for service in Service::ALL {
                let drawn = draw(&m, page::AWAY, None, Some(Form::Push { service }), None, None);
                assert!(!drawn.fields.is_empty(), "{service:?}");
            }
        }
        // The broker's form: nothing saved, a saved broker on another port.
        for broker in [None, Some("mqtt://user@broker.local:1999".to_string())] {
            let m = Model {
                mqtt_password_set: false,
                mqtt_line: None,
                prefs: Prefs { mqtt_broker: broker.clone(), ..base.prefs.clone() },
                ..model()
            };
            let drawn = draw(&m, page::MQTT, None, Some(Form::Broker { tls: false }), None, None);
            let port = drawn.fields.iter().find(|f| f.id == Field::MqttPort).map(|f| f.text.clone());
            assert_eq!(port.as_deref(), Some(if broker.is_some() { "1999" } else { "" }));
        }
        i18n::set(Choice::German);
        for (page, m) in &variants {
            draw(m, *page, None, None, None, None);
        }
        i18n::set(Choice::English);
    }

    #[test]
    fn a_page_whose_extension_is_off_shows_the_extensions() {
        i18n::set(Choice::English);
        let off = Model { prefs: Prefs::defaults(), ..model() };
        for page in [page::SYSTEM_MONITOR, page::FORGE, page::AWAY, page::MQTT, page::MAILBOX, page::OWN_EXTENSIONS] {
            assert!(!page_shown(page, &off.prefs));
            let drawn = draw(&off, page, None, None, None, None);
            // The Extensions page's switches.
            assert!(drawn.hits.iter().any(|h| h.action == Action::OwnExtensions), "{page}");
        }
        assert!(!page_shown(PAGES.len(), &off.prefs));
        // Every extension on, with Taskbar's displays: the sidebar's rows closer together.
        let m = model();
        assert_eq!(pages(&m).len(), PAGES.len());
        let rows = draw(&m, page::GENERAL, None, None, None, None)
            .hits
            .iter()
            .filter(|h| matches!(h.action, Action::Page(_)))
            .count();
        assert_eq!(rows, PAGES.len());
    }

    #[test]
    fn many_displays_are_drawn_in_columns() {
        i18n::set(Choice::English);
        for n in [1, 5, 9, 14, 30] {
            let displays = (0..n).map(|i| (format!("d{i}"), format!("Display {i}"), "Somewhere · 2560 × 1440".into()));
            let m = Model { displays: displays.collect(), chosen: vec!["d1".into(), "d3".into()], ..model() };
            let drawn = draw(&m, page::TASKBAR, None, None, None, None);
            assert_eq!(drawn.hits.iter().filter(|h| matches!(h.action, Action::Display(_))).count(), n);
        }
    }

    #[test]
    fn says_how_git_and_gpg_stand_in_every_case() {
        use crate::gpg::{Check, Missing};
        i18n::set(Choice::English);
        let found = Some(Check::Found { git: true, gpg: false });
        assert_eq!(gpg_status(false, found).text, "How long your key stays unlocked · GnuPG not found");
        let ready = |running, cached| gpg_status(true, Some(Check::Ready { running, cached }));
        assert_eq!(ready(false, true).text, "✓ git signs with GnuPG · gpg-agent not running");
        assert_eq!(ready(true, true).text, "✓ git signs with GnuPG · key unlocked");
        for why in
            [Missing::Git, Missing::NotSigning, Missing::OtherFormat, Missing::GnuPg, Missing::Key, Missing::Agent]
        {
            let s = gpg_status(true, Some(Check::Missing(why)));
            assert_eq!(s.tone, Tone::Warn, "{why:?}");
            assert!(!s.text.is_empty());
        }
        // The two tools found, without a host to log in to: nothing to say but that.
        let found = crate::forge::Tool { found: true, ..Default::default() };
        let tools = crate::forge::Tools { glab: found.clone(), gh: found, asked: true };
        let s = forge_status(true, &tools);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Plain, "glab found · gh found"));
        // Not asked yet: a host says so.
        let host = crate::forge::Host { name: "gitlab.example.com".into(), ..Default::default() };
        let waiting = crate::forge::Tools { asked: false, ..tools };
        let s = host_status(crate::forge::Forge::GitLab, &host, &waiting, 0);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Busy, "Checking…"));
        // A tool a host needs but that isn't there: in yellow.
        let needed = crate::forge::Tool { hosts: vec![host], ..Default::default() };
        let s = tool_status(crate::forge::Forge::GitLab, &needed);
        assert_eq!((s.tone, s.text.as_str()), (Tone::Warn, "Not found: winget install --id GLab.GLab"));
        // A day with its year.
        assert!(day_with_year(1_800_000_000).ends_with(" 2027"), "{}", day_with_year(1_800_000_000));
    }

    #[test]
    fn each_tone_has_its_colour_and_removing_is_red() {
        let pal = theme::palette(true);
        let colors: Vec<Color> =
            [Tone::Plain, Tone::Busy, Tone::Good, Tone::Warn, Tone::Bad].iter().map(|&t| tone_color(pal, t)).collect();
        assert_eq!(colors, [pal.text_dim, pal.accent, pal.done, pal.warn, pal.crit]);
        use crate::forge::Forge;
        for action in
            [Action::RemovePush, Action::RemoveBroker, Action::RemoveToken(Forge::GitHub, "github.com".into())]
        {
            assert!(destructive(&action));
            assert_eq!(button_icon(&action), Some(BIN));
        }
        assert!(!destructive(&Action::TestPush));
        assert_eq!(button_icon(&Action::TestPush), None);
        assert_eq!(button_icon(&Action::Edit(Form::Broker { tls: true })), Some(PENCIL));
        for action in [Action::PasteMailbox, Action::PasteForm, Action::PasteToken(Forge::GitLab, "x".into())] {
            assert_eq!(button_icon(&action), Some(CLIPBOARD));
        }
        assert_eq!(Status::bad("x"), Status::new(Tone::Bad, "x"));
    }

    /// Without its window (never opened), the settings window keeps its state but
    /// shows, finds and clicks nothing.
    #[test]
    fn without_a_window_nothing_is_shown() {
        let mut w = SettingsWindow::new();
        assert!(!w.visible() && !w.hovering());
        w.retitle();
        // The model is only taken for a window that shows it.
        w.set_model(model());
        assert!(w.model().is_none());
        w.set_page(page::ABOUT);
        assert_eq!(w.page, page::ABOUT);
        w.set_page(PAGES.len());
        assert_eq!(w.page, page::ABOUT);
        w.set_form(Some(Form::Broker { tls: false }));
        w.set_form(Some(Form::Broker { tls: false }));
        assert_eq!(w.form, Some(Form::Broker { tls: false }));
        // No fields on screen: nothing typed, nothing to fill.
        assert_eq!(w.field_text(Field::MqttHost), "");
        w.fill(&[(Field::MqttHost, "homeassistant.local".into())]);
        assert_eq!(w.field_text(Field::MqttHost), "");
        // The mouse over a description cut short: its tip, but no hit.
        w.tips = vec![(10.0, 10.0, 50.0, 20.0)];
        w.set_mouse(Some((20, 15)));
        w.set_mouse(Some((21, 15)));
        assert_eq!((w.tip, w.hover), (Some(0), None));
        w.set_mouse(None);
        assert_eq!(w.tip, None);
        assert_eq!(w.click(20, 15), None);
        // An open list closes on any click.
        w.select = Some(Select::Language);
        assert_eq!(w.click(20, 15), None);
        assert!(w.select.is_none());
        assert!(!w.close_select());
        w.set_select(Some(Select::Language));
        assert!(w.close_select());
        w.close();
        assert_eq!((w.form, w.select, w.hover), (None, None, None));
    }

    #[test]
    fn the_window_is_as_big_as_its_client_area_and_frame() {
        let (w, h) = SettingsWindow::window_size(96);
        assert!(w >= WIDTH as i32 && h > HEIGHT as i32, "{w} × {h}");
        let (w2, h2) = SettingsWindow::window_size(144);
        assert!(w2 >= (WIDTH * 1.5) as i32 && h2 > h, "{w2} × {h2}");
        // A window that isn't there: its scale is 1.
        assert_eq!(scale_of(HWND::default()), 1.0);
    }
}
