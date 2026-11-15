//! App state and the UI thread.
//!
//! Window procedures turn messages into [`Event`]s and [`dispatch`] them. Win32
//! can call a window procedure again while we are still handling a message
//! (e.g. a hook's WM_COPYDATA arrives while we wait on a cross-process
//! SetWindowPos), so events go through a queue: the outermost dispatch runs them
//! one after another and nested calls only enqueue. Nothing that runs a modal
//! loop (menus, message boxes) happens inside event handling; those run from
//! the window procedure directly and dispatch their outcome.

use crate::bubble::{self, Bubble};
use crate::codex;
use crate::collector::{self, Snapshot};
use crate::copilot;
use crate::flyout::{self, Flyout};
use crate::forge;
use crate::hook::{self, HookMessage};
use crate::i18n;
use crate::mailbox;
use crate::sessions::{Session, Sessions, Status};
use crate::settings_window::{self, SettingsWindow};
use crate::stats_window::{self, StatsWindow};
use crate::tip;
use crate::usage::{self, FetchError, Usage};
use crate::widget::{self, Widgets};
use crate::win::{self, SendHwnd, now_ms, post_boxed, take_boxed};
use crate::{forecast, menu, settings, sound, store, theme, timefmt, tracking, transcripts, uia, vscode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_BACK, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_NEXT,
    VK_PRIOR, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HTCLIENT, IDC_ARROW,
    IDC_HAND, IDYES, KillTimer, LoadCursorW, LoadIconW, MA_NOACTIVATE, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK,
    MB_SETFOREGROUND, MB_YESNO, MESSAGEBOX_STYLE, MSG, MessageBoxW, PBT_APMRESUMEAUTOMATIC, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SetCursor,
    SetTimer, SetWindowPos, TranslateMessage, WA_INACTIVE, WM_ACTIVATE, WM_APP, WM_CHAR, WM_CLOSE, WM_COPYDATA,
    WM_DESTROY, WM_DPICHANGED, WM_ENDSESSION, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE,
    WM_THEMECHANGED, WM_TIMER, WM_XBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

/// Class of the hidden control window; hook processes find the app by it.
pub const CONTROL_CLASS: PCWSTR = w!("Trayward.Control");
/// The demo's control window: hooks of real sessions must not find it.
const DEMO_CONTROL_CLASS: PCWSTR = w!("Trayward.Control.Demo");

fn control_class() -> PCWSTR {
    if crate::demo::active() { DEMO_CONTROL_CLASS } else { CONTROL_CLASS }
}
const APP_NAME: PCWSTR = w!("Trayward");

const WM_APP_FETCHED: u32 = WM_APP + 1;
const WM_APP_SWITCHED: u32 = WM_APP + 2;
const WM_APP_ENABLE_TRACKING: u32 = WM_APP + 3;
const WM_APP_HIDE_FLYOUT: u32 = WM_APP + 4;
const WM_APP_SNAPSHOT: u32 = WM_APP + 5;
const WM_APP_BUTTONS: u32 = WM_APP + 6;
const WM_APP_STATS: u32 = WM_APP + 7;
const WM_APP_CODEX: u32 = WM_APP + 8;
const WM_APP_FORGE: u32 = WM_APP + 9;
const WM_APP_WEEK: u32 = WM_APP + 10;
const WM_APP_PUSH: u32 = WM_APP + 11;
const WM_APP_UPDATE: u32 = WM_APP + 12;
const WM_APP_MQTT: u32 = WM_APP + 13;
const WM_APP_SYSMON: u32 = WM_APP + 14;
const WM_APP_COPILOT: u32 = WM_APP + 15;
const WM_APP_COPILOT_QUOTA: u32 = WM_APP + 16;
const WM_APP_PROVIDERS: u32 = WM_APP + 17;
const WM_APP_TRACKING: u32 = WM_APP + 18;
const WM_APP_MAILBOX: u32 = WM_APP + 19;
/// How often the update check asks GitHub.
const UPDATE_EVERY_SECS: i64 = 24 * 3600;
/// Kept statistics are saved this long after start, then this often.
const KEEP_AFTER: Duration = Duration::from_secs(60);
const KEEP_EVERY: Duration = Duration::from_secs(6 * 3600);

const T_LAYOUT: usize = 1;
const T_FETCH: usize = 2;
const T_HOUSEKEEPING: usize = 3;
const T_ANIMATION: usize = 4;
const ANIMATION_MS: u32 = 33;
/// After a manual refresh the button rests this long (the usage API rate-limits).
const REFRESH_REST: Duration = Duration::from_secs(15);
/// The refresh spinner turns at least this long, so a fast refresh is still visible.
const MIN_SPIN: Duration = Duration::from_millis(700);
/// A session that has needed the user this long turns the dot red.
const URGENT_AFTER_MS: i64 = 10 * 60 * 1000;
/// The GPG key timer turns amber in its last minutes.
const GPG_ENDING_SECS: i64 = 5 * 60;
/// How long a finished session stays with the active ones, seen or not.
const DONE_STAYS_MS: i64 = 60 * 60 * 1000;
/// One glow pulse, and how often and how long the needs-you dot shakes.
const PULSE_SECS: f32 = 1.6;
const SHAKE_EVERY_SECS: f32 = 6.0;
const SHAKE_SECS: f32 = 0.6;
const T_BUBBLE: usize = 5;
/// The tooltip shows when the mouse rests on the widget this long.
const T_TIP: usize = 6;
const TIP_MS: u32 = 600;
/// How long a bubble stays (longer while the mouse is on it).
const BUBBLE_MS: u32 = 6_000;
/// Right after start-up, sessions settle from the saved state: no sounds or bubbles yet.
/// How long statistics stay cached after their window is closed.
const STATS_KEEP_MS: i64 = 10 * 60 * 1000;
const STARTUP_QUIET: Duration = Duration::from_secs(10);
/// How long a VS Code window must be in front to count as looking at a finished session.
const FOCUS_SEEN: Duration = Duration::from_secs(2);
/// The same sound plays at most this often.
const SOUND_GAP: Duration = Duration::from_secs(2);
/// How often the subscription plan is re-read.
const PLAN_EVERY_SECS: i64 = 6 * 3600;
/// Numbers older than this, with failing refreshes, are shown dimmed.
const STALE_AFTER_SECS: i64 = 15 * 60;
const MAX_BACKOFF_SECS: u32 = 15 * 60;

#[derive(Debug)]
pub enum Event {
    LayoutTick,
    FetchDue,
    Fetched(Box<Fetched>),
    Housekeeping,
    Snapshot(Box<Snapshot>),
    /// Where buttons and clocks are on the taskbars (from UI Automation).
    Edges(Vec<uia::Edges>),
    AnimationTick,
    Hook(Box<HookMessage>),
    ThemeChanged,
    TaskbarCreated,
    Resumed,
    WidgetHover(HWND, bool),
    WidgetClick(HWND),
    /// The widget was right-clicked: windows opened from its menu appear next to it.
    WidgetMenu(HWND),
    FlyoutMouse(Option<(i32, i32)>),
    FlyoutClick(i32, i32),
    /// Mouse-wheel delta over the flyout.
    FlyoutWheel(i16),
    FlyoutHide,
    FlyoutPaint,
    Refresh,
    SetInterval(u32),
    SetBars(settings::Bars),
    SetSolidBackground(bool),
    SetTaskbars(settings::Taskbars),
    /// Show or hide the widget on the taskbar of this display (by id).
    ToggleDisplay(String),
    SetPrefs(Box<settings::Prefs>),
    BubbleClick,
    BubbleMouse,
    BubbleTimeout,
    /// The mouse rested on a widget: show the tooltip.
    TipDue,
    /// What Codex's files say now.
    Codex(Box<codex::Snapshot>),
    /// Copilot's sessions now.
    Copilot(Vec<codex::Session>),
    /// Copilot's monthly limit, or why it couldn't be read.
    CopilotQuota(copilot::QuotaResult),
    /// Whether Codex's and Copilot's folders exist (looked up off the UI thread).
    ProvidersFound(bool, bool),
    /// The session mailbox now.
    Mailbox(Box<mailbox::Snapshot>),
    /// Why a pasted mailbox folder wasn't taken (or nothing to say).
    MailboxNote(Option<String>),
    /// Merge and pull requests and pipelines, from GitLab and GitHub.
    Forge(Box<forge::Snapshot>),
    /// Last week, summed up for the weekly summary.
    Week(Box<crate::alerts::Week>),
    /// A push went out (or didn't); true for the settings' test.
    Pushed(Box<(bool, Result<(), String>)>),
    /// Send a test push.
    TestPush,
    /// What the settings say about the push address.
    PushStatus(Option<String>),
    /// Ask GitHub for a newer release now (Settings → About).
    CheckUpdate,
    /// The MQTT connection: connected, or why not.
    MqttStatus(Box<crate::mqtt::Status>),
    /// A second of the system monitor.
    Monitor(crate::sysmon::Sample),
    /// What the settings say about the broker address (a paste that didn't take).
    MqttNote(Option<String>),
    /// Connect to the broker now (the settings' Test).
    MqttTest,
    /// GitHub's answer: a newer release, or none.
    Update(Box<Result<Option<crate::update::Release>, String>>),
    /// Snooze notifications until then (unix seconds), or resume them.
    Snooze(Option<i64>),
    /// Demo mode: show this plan (an index into `demo::PLANS`).
    DemoPlan(usize),
    OpenSettings,
    OpenStats(Option<String>),
    StatsClick(i32, i32),
    /// Text typed (or pasted) into the statistics window.
    StatsType(String),
    StatsBackspace,
    StatsEscape,
    StatsBack,
    /// Scroll by DIPs, or by mouse-wheel delta.
    StatsScroll(f32),
    StatsWheel(i16),
    /// A step of smooth scrolling.
    StatsTick,
    StatsData(Box<(stats_window::Page, stats_window::Data)>),
    StatsMouse(Option<(i32, i32)>),
    StatsPaint,
    StatsClose,
    SettingsPage(usize),
    SettingsMouse(Option<(i32, i32)>),
    SettingsPaint,
    SettingsClose,
    /// "Start with Windows" was switched: how it is now.
    AutostartChanged(bool),
    /// Session tracking was switched: how the hooks are now.
    TrackingChanged(tracking::State),
    Exit,
}

struct App {
    ctl: HWND,
    widgets: Widgets,
    flyout: Flyout,
    dark: bool,
    usage: Option<Usage>,
    /// Unix seconds of the last successful fetch.
    fetched_at: Option<i64>,
    error: Option<FetchError>,
    fetching: bool,
    rate_limited: u32,
    interval: u32,
    /// Subscription plan, e.g. "Ultra".
    plan: Option<String>,
    /// Readings of the weekly limit.
    history: Vec<forecast::Sample>,
    /// When the plan was last read (unix seconds).
    plan_checked: Option<i64>,
    sessions: Sessions,
    tracking: tracking::State,
    config_dir: PathBuf,
    /// Reads session lists, transcripts and window names in the background.
    collector: Sender<collector::Request>,
    widget_options: widget::Options,
    /// VS Code's exe, for its icon in the session list.
    vscode_exe: Option<PathBuf>,
    /// When the last manual refresh started.
    refresh_started: Option<Instant>,
    /// The manual refresh waits for fresh session data.
    awaiting_snapshot: bool,
    animating: bool,
    /// The VS Code window in front: handle, title, and since when.
    focus: Option<(isize, String, Instant)>,
    /// Which processes are VS Code, by pid.
    vscode_pids: HashMap<u32, bool>,
    /// The system monitor's values switched on (`sysmon` bits), shared with its sampler.
    monitor_on: Arc<AtomicU8>,
    /// Its last samples, oldest first (up to `widget::HISTORY`).
    monitor: VecDeque<crate::sysmon::Sample>,
    /// Start of the animation clock.
    started: Instant,
    gpg: GpgTimer,
    prefs: settings::Prefs,
    /// Limit warnings already shown, so each shows once per window.
    warned: Vec<(String, i64)>,
    /// Notifications are snoozed until then (unix seconds).
    snoozed_until: Option<i64>,
    /// The session pinned to the taskbar.
    pinned: Option<String>,
    /// The week (its Monday) the weekly summary was last made for.
    summary_week: Option<i64>,
    /// A weekly summary waiting for a quiet moment to end.
    week_bubble: Option<bubble::Model>,
    /// How the last test push went, or why a paste didn't take.
    push_status: Option<String>,
    /// A newer release of Trayward.
    update: Option<crate::update::Release>,
    /// The MQTT thread, the broker it was last given, how that went, and the
    /// state it was last sent.
    mqtt: std::sync::mpsc::Sender<crate::mqtt::Command>,
    mqtt_broker: Option<crate::mqtt::Broker>,
    mqtt_status: Option<crate::mqtt::Status>,
    mqtt_note: Option<String>,
    mqtt_sent: String,
    /// When kept statistics were last saved, and whether that's running now.
    kept_at: Option<Instant>,
    keeping: Arc<AtomicBool>,
    /// When the update check last asked (unix seconds).
    update_checked: Option<i64>,
    /// The newer version a bubble already told about.
    update_announced: Option<String>,
    /// The update check is asking right now; or why it couldn't.
    update_checking: bool,
    update_error: Option<String>,
    /// Codex's limits and sessions, while Codex is on.
    codex: codex::Snapshot,
    /// The Codex reader looks only while this is set.
    codex_on: Arc<AtomicBool>,
    /// Copilot's sessions, while Copilot is on.
    copilot: Vec<codex::Session>,
    copilot_on: Arc<AtomicBool>,
    /// Copilot's monthly limit, while it's asked for; why the last try failed.
    copilot_quota: Option<copilot::Quota>,
    copilot_error: Option<String>,
    copilot_quota_on: Arc<AtomicBool>,
    /// Set to ask for Copilot's limit right away.
    copilot_poke: Arc<AtomicBool>,
    /// The session mailbox, while it's switched on.
    mailbox: mailbox::Snapshot,
    mailbox_on: Arc<AtomicBool>,
    /// The mailbox folder set in the settings, for its reader.
    mailbox_dir: Arc<Mutex<Option<String>>>,
    mailbox_note: Option<String>,
    /// Asks the mailbox's reader to look now.
    mailbox_poke: Arc<AtomicBool>,
    /// Since when the mailbox is being looked for, and whether the reader answered.
    mailbox_search: Option<(Instant, bool)>,
    /// Whether Codex's and Copilot's folders exist, as of opening the settings.
    providers_found: (bool, bool),
    /// "Start with Windows", as of opening the settings or switching it there.
    autostart: bool,
    /// Writes the state file.
    writer: store::Writer,
    /// The usage endpoint asked to wait until then (Retry-After).
    fetch_not_before: Option<Instant>,
    /// Merge and pull requests, while switched on.
    forge: forge::Snapshot,
    forge_on: Arc<AtomicBool>,
    /// Ask GitLab and GitHub now (the flyout opened).
    forge_poke: Arc<AtomicBool>,
    /// The sessions' folders, for the merge and pull request reader.
    forge_folders: Arc<Mutex<Vec<String>>>,
    bubble: Bubble,
    tip: tip::Tip,
    settings: SettingsWindow,
    stats: StatsWindow,
    /// When the statistics window was closed; their cache is released a while later.
    stats_closed_at: Option<i64>,
    /// Whether the UI Automation watcher should look at the taskbar.
    uia_active: Arc<AtomicBool>,
    /// When each kind of sound last played.
    last_sound: HashMap<sound::Kind, Instant>,
    /// Sessions already warned about their cache, with the expiry warned for.
    cache_warned: HashMap<String, i64>,
}

/// What is known about the GPG signing key's cache.
#[derive(Default)]
struct GpgTimer {
    reading: Option<collector::GpgReading>,
    /// When the key was seen to get unlocked (unix seconds).
    unlocked_at: Option<i64>,
    /// The agent and when it unlocked the key, kept in `state.json` so a
    /// restart of Trayward doesn't forget it.
    saved: Option<(u32, i64)>,
}

impl GpgTimer {
    /// Takes a new reading; true when what's worth saving changed.
    fn update(&mut self, r: collector::GpgReading, now: i64) -> bool {
        let was = self.reading.as_ref().map(|r| (r.cached, r.agent));
        self.unlocked_at = match (was, r.cached) {
            (_, false) => None,
            // Seen getting unlocked.
            (Some((false, _)), true) => Some(now),
            // A new agent that's already unlocked: it happened since the last look.
            (Some((true, agent)), true) if agent != r.agent => Some(now),
            (Some((true, _)), true) => self.unlocked_at,
            // The first look after start: the last run may have seen the
            // unlocking, by this same agent.
            // (Not a pid that Windows gave to a newer agent started after it.)
            (None, true) => self
                .saved
                .filter(|&(pid, at)| {
                    Some(pid) == r.agent && now - at < r.max_ttl as i64 && r.agent_started.is_none_or(|s| s <= at)
                })
                .map(|(_, at)| at),
        };
        let saved = r.agent.zip(self.unlocked_at);
        self.reading = Some(r);
        let changed = saved != self.saved;
        self.saved = saved;
        changed
    }

    /// The key's state for the flyout. The agent doesn't say how long is left:
    /// each signature restarts a `default_ttl` timer and `max_ttl` caps the
    /// whole time, so the last signature is taken to be the last commit.
    fn view(&self, now: i64) -> Option<flyout::Gpg> {
        let r = self.reading.as_ref()?;
        if !r.cached {
            return Some(flyout::Gpg { unlocked: false, left: None, total: r.default_ttl as i64, at_least: false });
        }
        let last_use = match (self.unlocked_at, r.last_commit) {
            (Some(u), Some(c)) => Some(u.max(c)),
            (Some(u), None) => Some(u),
            (None, c) => c,
        };
        let (default_ttl, max_ttl) = (r.default_ttl as i64, r.max_ttl as i64);
        let idle_deadline = last_use.map(|t| t + default_ttl);
        // Unlocked at an unknown time: not before the agent started, so the
        // hard limit is at least that far away.
        let at_least = self.unlocked_at.is_none();
        let hard_deadline = self.unlocked_at.or(r.agent_started).map(|t| t + max_ttl);
        // The deadline that comes first, and the length of its window (for the bar).
        let (deadline, total, hard) = match (idle_deadline, hard_deadline) {
            (Some(a), Some(b)) if b < a => (Some(b), max_ttl, true),
            (Some(a), _) => (Some(a), default_ttl, false),
            (None, Some(b)) => (Some(b), max_ttl, true),
            (None, None) => (None, default_ttl, false),
        };
        // Still unlocked past the estimate: it was used in a way we can't see.
        let left = deadline.map(|d| d - now).filter(|left| *left > 0);
        Some(flyout::Gpg { unlocked: true, left, total, at_least: at_least && hard })
    }
}

/// Result of a background refresh.
#[derive(Debug)]
pub struct Fetched {
    usage: Result<Usage, FetchError>,
    /// `None` when the plan wasn't due for a check (or the check failed).
    plan: Option<String>,
    /// The plan was asked for.
    plan_tried: bool,
}

/// A rise of the weekly percentage between two readings by more than this is
/// more likely a smaller plan than use: the plan is asked for again.
const PLAN_JUMP: f64 = 10.0;

/// After a failed plan check, the next one comes this much later (not with
/// every refresh, and not only after the usual 6 hours).
const PLAN_RETRY_SECS: i64 = 15 * 60;

/// How long to wait before asking for the usage again: the interval, doubled
/// for each rate limit in a row (within bounds), and at least as long as the
/// server asked for.
fn fetch_wait(interval: u32, rate_limited: u32, retry_after: Option<u32>) -> u32 {
    let doubled = interval.saturating_mul(1 << rate_limited.min(4)).clamp(interval, MAX_BACKOFF_SECS.max(interval));
    doubled.max(retry_after.unwrap_or(0).min(MAX_RETRY_AFTER_SECS))
}

/// A server's `Retry-After` counts up to this.
const MAX_RETRY_AFTER_SECS: u32 = 3600;

/// Whether this is the only Trayward (or the only demo: the demo may run
/// next to the real app). Holds a mutex until the process ends.
pub fn single_instance(demo: bool) -> bool {
    let name = if demo { w!("Local\\Trayward.Demo") } else { w!("Local\\Trayward.Singleton") };
    // SAFETY: a plain call; the handle is kept for the life of the process on purpose.
    unsafe {
        let mutex = CreateMutexW(None, false, name);
        mutex.is_ok() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Message id Explorer broadcasts when the taskbar is (re)created.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
}

/// Queues `ev` and, unless an outer call is already doing so, handles the queue.
pub fn dispatch(ev: Event) {
    QUEUE.with(|q| q.borrow_mut().push_back(ev));
    APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(app) = guard.as_mut() else { return };
        while let Some(ev) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
            app.handle(ev);
        }
    });
}

/// Reads app state, unless it is busy (re-entrant call) or not running.
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

fn now() -> i64 {
    now_ms() / 1000
}

impl App {
    fn handle(&mut self, ev: Event) {
        match ev {
            Event::LayoutTick => {
                self.notice_focus();
                self.warn_expiring_caches();
                self.announce_resets();
                self.summarize_week();
                self.check_update(false);
                self.announce_update();
                self.warn_copilot();
                self.keep_stats();
                // Windows that were closed give their drawing memory back.
                crate::gfx::forget_hidden();
                self.refresh_views();
            }
            Event::FetchDue => self.start_fetch(),
            Event::Refresh => self.manual_refresh(),
            Event::Fetched(r) => self.on_fetched(*r),
            Event::Housekeeping => self.housekeeping(),
            Event::Snapshot(snap) => self.on_snapshot(*snap),
            Event::Edges(edges) => {
                if self.widgets.set_edges(edges) {
                    self.refresh_views();
                }
            }
            Event::AnimationTick => self.animate(),
            // Hook events are about real sessions, not the demo's.
            Event::Hook(_) if crate::demo::active() => {}
            Event::Hook(m) => {
                let before = self.statuses();
                if self.sessions.apply(&m) {
                    self.announce(&before);
                    self.save();
                    self.refresh_views();
                }
            }
            Event::ThemeChanged => {
                self.dark = theme::system_dark();
                self.refresh_views();
            }
            Event::TaskbarCreated => {
                self.widgets.reset();
                self.refresh_views();
            }
            Event::Resumed => self.schedule_fetch(5),
            Event::WidgetHover(hwnd, on) => {
                if self.widgets.set_hover(hwnd, on) {
                    // The tooltip after a moment of rest; gone when the mouse leaves.
                    self.hide_tip();
                    if on {
                        // SAFETY: our own window and timer id.
                        unsafe { SetTimer(Some(self.ctl), T_TIP, TIP_MS, None) };
                    }
                    self.refresh_views();
                }
            }
            Event::TipDue => self.show_tip(),
            Event::Pushed(r) => {
                let status = match *r {
                    (true, Ok(())) => Some(i18n::t("Sent: have a look at your phone").to_string()),
                    (_, Err(e)) => Some(i18n::f("Not sent: {0}", &[&e])),
                    // A push that went out clears an earlier failure.
                    (false, Ok(())) => None,
                };
                self.set_push_status(status);
            }
            Event::TestPush => {
                if let Some(url) = self.prefs.push_url.clone() {
                    self.set_push_status(Some(i18n::t("Sending…").to_string()));
                    self.send_push(url, i18n::t("Trayward can reach your phone.").to_string(), true);
                }
            }
            Event::PushStatus(s) => self.set_push_status(s),
            Event::CheckUpdate => self.check_update(true),
            Event::Monitor(sample) => {
                // A sample can still arrive after everything was switched off.
                if self.prefs.monitor.bits() != 0 {
                    if self.monitor.len() >= widget::HISTORY {
                        self.monitor.pop_front();
                    }
                    self.monitor.push_back(sample);
                    // Only the widget and its tooltip show it: no need to lay out the other windows.
                    let m = self.widget_model();
                    self.widgets.update(&m, &self.widget_options);
                    if self.tip.visible() {
                        self.show_tip();
                    }
                }
            }
            Event::MqttStatus(s) => {
                self.mqtt_status = Some(*s);
                self.refresh_settings();
            }
            Event::MqttNote(n) => {
                self.mqtt_note = n;
                self.configure_mqtt();
                self.refresh_settings();
            }
            Event::MqttTest => {
                self.mqtt_note = None;
                self.mqtt_status = None;
                let _ = self.mqtt.send(crate::mqtt::Command::Test);
                self.refresh_settings();
            }
            Event::Update(r) => {
                self.update_checking = false;
                self.update_checked = Some(now());
                match *r {
                    Ok(u) => {
                        self.update = u;
                        self.update_error = None;
                    }
                    Err(e) => self.update_error = Some(e),
                }
                self.save();
                self.announce_update();
                self.refresh_views();
            }
            Event::Week(week) => {
                if let Some((headline, detail)) = crate::alerts::week_text(&week) {
                    self.week_bubble = Some(bubble::Model {
                        dark: self.dark,
                        // A click opens the statistics.
                        session_id: "stats:".into(),
                        status: Status::Done,
                        urgent: false,
                        headline,
                        detail,
                        badge: None,
                    });
                    self.summarize_week();
                }
            }
            Event::Forge(snap) => {
                self.announce_forge(&snap);
                self.forge = *snap;
                self.refresh_views();
            }
            Event::Codex(snap) => {
                let before = std::mem::take(&mut self.codex.sessions);
                self.announce_other("Codex", &before, &snap.sessions);
                self.codex = *snap;
                self.refresh_views();
            }
            Event::Copilot(sessions) => {
                let before = std::mem::take(&mut self.copilot);
                self.announce_other("Copilot", &before, &sessions);
                self.copilot = sessions;
                self.refresh_views();
            }
            Event::ProvidersFound(codex, copilot) => {
                self.providers_found = (codex, copilot);
                self.refresh_views();
            }
            Event::Mailbox(snap) => {
                self.mailbox = *snap;
                if let Some((_, answered)) = &mut self.mailbox_search {
                    *answered = true;
                }
                self.refresh_views();
            }
            Event::MailboxNote(note) => {
                self.mailbox_note = note;
                self.refresh_views();
            }
            Event::CopilotQuota(result) => {
                // A failed try keeps the last numbers; the settings say why.
                match result {
                    Ok(quota) => {
                        self.copilot_quota = quota;
                        self.copilot_error = None;
                        self.warn_copilot();
                    }
                    Err(why) => self.copilot_error = Some(why),
                }
                self.refresh_views();
            }
            Event::Snooze(until) => {
                self.snoozed_until = until;
                self.save();
                self.refresh_views();
            }
            Event::DemoPlan(i) if crate::demo::active() => {
                crate::demo::set_plan(i);
                self.start_fetch();
            }
            Event::DemoPlan(_) => {}
            Event::WidgetClick(hwnd) => {
                self.hide_tip();
                // A click on another taskbar's widget moves the flyout there.
                let here = self.widgets.is_active(hwnd);
                self.widgets.set_active(hwnd);
                if self.flyout.visible() && here {
                    self.flyout.hide();
                } else if self.flyout.visible() || !self.flyout.just_hidden() || !here {
                    // Fresh merge requests while the flyout is open.
                    self.forge_poke.store(true, Ordering::Relaxed);
                    self.show_flyout(true);
                }
            }
            Event::WidgetMenu(hwnd) => {
                self.hide_tip();
                self.widgets.set_active(hwnd);
            }
            Event::FlyoutMouse(pos) => self.flyout.set_mouse(pos),
            Event::FlyoutClick(x, y) => match self.flyout.hit(x, y) {
                Some(flyout::Action::Session(id)) => self.open_session(&id),
                Some(flyout::Action::Hide(id)) => {
                    if self.sessions.hide(&id, now_ms()) {
                        self.save();
                        self.refresh_views();
                    }
                }
                Some(flyout::Action::UnhideAll) => {
                    if self.sessions.unhide_all() {
                        self.save();
                        self.refresh_views();
                    }
                }
                Some(flyout::Action::Refresh) => dispatch(Event::Refresh),
                Some(flyout::Action::Settings) => dispatch(Event::OpenSettings),
                Some(flyout::Action::Stats(id)) => dispatch(Event::OpenStats(id)),
                Some(flyout::Action::Pin(id)) => {
                    self.pinned = if self.pinned.as_deref() == Some(id.as_str()) { None } else { Some(id) };
                    self.save();
                    self.refresh_views();
                }
                Some(flyout::Action::Open(url)) => {
                    self.flyout.hide();
                    open_url(&url);
                }
                Some(flyout::Action::MoreSessions) => {
                    self.flyout.set_expanded(true);
                    self.show_flyout(false);
                }
                Some(flyout::Action::FewerSessions) => {
                    self.flyout.set_expanded(false);
                    self.show_flyout(false);
                }
                Some(flyout::Action::EnableTracking) => {
                    self.flyout.hide();
                    // Asks for confirmation (a modal dialog), so it runs outside event handling.
                    // SAFETY: posting to our own window.
                    let _ = unsafe { PostMessageW(Some(self.ctl), WM_APP_ENABLE_TRACKING, WPARAM(0), LPARAM(0)) };
                }
                // Days of the weekly bar only show their numbers.
                Some(flyout::Action::Folder(path)) => {
                    self.flyout.hide();
                    stats_window::explore(&path, false);
                }
                Some(flyout::Action::Day(_)) | None => {}
            },
            Event::FlyoutWheel(delta) => {
                if self.flyout.wheel(delta) {
                    self.show_flyout(false);
                }
            }
            Event::FlyoutHide => self.flyout.hide(),
            Event::FlyoutPaint => {
                if let Some(h) = self.flyout.hwnd {
                    // SAFETY: our own window.
                    let _ = unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(Some(h), None, false) };
                }
            }
            Event::SetInterval(secs) => {
                self.interval = secs;
                settings::set_interval(secs);
                self.start_fetch();
            }
            Event::SetBars(bars) => {
                self.widget_options.bars = bars;
                settings::set_bars(bars);
                self.refresh_views();
            }
            Event::SetSolidBackground(on) => {
                self.widget_options.solid = on;
                settings::set_solid_background(on);
                self.refresh_views();
            }
            Event::SetTaskbars(t) => {
                if t == settings::Taskbars::Chosen && self.widget_options.chosen.is_empty() {
                    // Start from where the widget is now.
                    let shown = crate::taskbars::pick(
                        self.widgets.displays(),
                        self.widget_options.taskbars,
                        &self.widget_options.chosen,
                    );
                    self.widget_options.chosen = shown.iter().map(|d| d.id.clone()).collect();
                    settings::set_chosen_displays(&self.widget_options.chosen);
                }
                self.widget_options.taskbars = t;
                settings::set_taskbars(t);
                self.refresh_views();
            }
            Event::ToggleDisplay(id) => {
                let chosen = &mut self.widget_options.chosen;
                if let Some(i) = chosen.iter().position(|c| *c == id) {
                    // Keep at least one connected display.
                    let connected = |c: &String| self.widgets.displays().iter().any(|d| d.id == *c);
                    if chosen.iter().filter(|c| connected(c)).count() > 1 {
                        chosen.remove(i);
                    }
                } else {
                    chosen.push(id);
                }
                settings::set_chosen_displays(chosen);
                self.refresh_views();
            }
            Event::SetPrefs(p) => self.set_prefs(*p),
            Event::BubbleClick => {
                if let Some(id) = self.bubble.model.as_ref().map(|m| m.session_id.clone()) {
                    self.hide_bubble();
                    if let Some(url) = id.strip_prefix("url:") {
                        open_url(url);
                    } else if id == "stats:" {
                        dispatch(Event::OpenStats(None));
                    } else if id.is_empty() {
                        // A limit warning: show the limits.
                        self.show_flyout(true);
                    } else {
                        self.open_session(&id);
                    }
                }
            }
            Event::BubbleMouse => {
                if self.bubble.visible() {
                    // SAFETY: our own window; re-setting the timer restarts it.
                    unsafe { SetTimer(Some(self.ctl), T_BUBBLE, BUBBLE_MS, None) };
                }
            }
            Event::BubbleTimeout => self.hide_bubble(),
            Event::OpenStats(id) => {
                self.stats_closed_at = None;
                self.flyout.hide();
                let page = id.map_or(self.stats.overview(), stats_window::Page::Session);
                let (anchor, dpi) = self.widgets.anchor().unwrap_or((RECT::default(), 96));
                self.stats.open(self.stats_model(), page.clone(), anchor, dpi);
                self.compute_stats(page.data_key());
            }
            Event::StatsClick(x, y) => {
                let cmd = self.stats.click(x, y);
                self.stats_command(cmd);
            }
            Event::StatsBack => {
                let cmd = self.stats.back();
                self.stats_command(cmd);
            }
            Event::StatsType(text) => self.stats.type_text(&text),
            Event::StatsBackspace => {
                // Deletes from the search, else goes back.
                if !self.stats.backspace() {
                    let cmd = self.stats.back();
                    self.stats_command(cmd);
                }
            }
            Event::StatsEscape => {
                // Clears the search, else closes.
                if !self.stats.clear_search() {
                    self.stats.close();
                    self.stats_closed_at = Some(now_ms());
                }
            }
            Event::StatsScroll(dy) => self.stats.scroll_by(dy),
            Event::StatsTick => self.stats.tick(),
            Event::StatsWheel(delta) => self.stats.wheel(delta),
            Event::StatsData(d) => {
                let (page, data) = *d;
                self.stats.set_data(&page, data);
            }
            Event::StatsMouse(pos) => self.stats.set_mouse(pos),
            Event::StatsPaint => {
                if let Some(h) = self.stats.hwnd {
                    // SAFETY: our own window.
                    let _ = unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(Some(h), None, false) };
                }
            }
            Event::StatsClose => {
                self.stats.close();
                self.stats_closed_at = Some(now_ms());
            }
            Event::OpenSettings => {
                self.flyout.hide();
                // The folders may be on a slow drive: looked up on a thread, shown when known.
                let ctl = SendHwnd::new(self.ctl);
                std::thread::spawn(move || {
                    post_boxed(ctl, WM_APP_PROVIDERS, (codex::home().is_some(), copilot::home().is_some()))
                });
                self.autostart = settings::autostart();
                let model = self.settings_model();
                let (anchor, dpi) = self.widgets.anchor().unwrap_or((RECT::default(), 96));
                self.settings.open(model, anchor, dpi);
            }
            Event::SettingsPage(i) => self.settings.set_page(i),
            Event::SettingsMouse(pos) => self.settings.set_mouse(pos),
            Event::SettingsPaint => {
                if let Some(h) = self.settings.hwnd {
                    // SAFETY: our own window.
                    let _ = unsafe { windows::Win32::Graphics::Gdi::InvalidateRect(Some(h), None, false) };
                }
            }
            Event::SettingsClose => self.settings.close(),
            Event::AutostartChanged(on) => {
                self.autostart = on;
                self.refresh_views();
            }
            Event::TrackingChanged(state) => {
                self.tracking = state;
                self.refresh_views();
            }
            Event::Exit => {
                self.save();
                self.writer.flush();
                // Say offline and disconnect (the broker's last will covers it otherwise).
                if self.mqtt_broker.is_some() {
                    let _ = self.mqtt.send(crate::mqtt::Command::Stop);
                    std::thread::sleep(Duration::from_millis(300));
                }
                // SAFETY: destroying our own windows; WM_DESTROY handlers don't touch app state.
                unsafe {
                    for h in self.widgets.hwnds() {
                        let _ = DestroyWindow(h);
                    }
                    if let Some(h) = self.flyout.hwnd {
                        let _ = DestroyWindow(h);
                    }
                    let _ = DestroyWindow(self.ctl);
                }
            }
        }
    }

    // ---------- usage ----------

    fn start_fetch(&mut self) {
        if self.fetching {
            return;
        }
        // The server asked to wait (Retry-After): not before then, whoever asks.
        if let Some(wait) = self.fetch_not_before.and_then(|t| t.checked_duration_since(Instant::now())) {
            self.schedule_fetch(wait.as_secs().max(1) as u32);
            return;
        }
        self.fetching = true;
        // SAFETY: our own window and timer id.
        let _ = unsafe { KillTimer(Some(self.ctl), T_FETCH) };
        let ctl = SendHwnd::new(self.ctl);
        if crate::demo::active() {
            let usage = Ok(crate::demo::usage(now()));
            post_boxed(
                ctl,
                WM_APP_FETCHED,
                Fetched { usage, plan: Some(crate::demo::plan().into()), plan_tried: true },
            );
            return;
        }
        let dir = self.config_dir.clone();
        let plan_due = self.plan_checked.is_none_or(|t| now() - t >= PLAN_EVERY_SECS);
        std::thread::spawn(move || {
            let usage = usage::fetch(&dir);
            let plan_tried = plan_due && usage.is_ok();
            let plan = if plan_tried { usage::fetch_plan(&dir).ok() } else { None };
            post_boxed(ctl, WM_APP_FETCHED, Fetched { usage, plan, plan_tried });
        });
    }

    fn schedule_fetch(&self, secs: u32) {
        // SAFETY: our own window; re-setting a timer id replaces it.
        unsafe { SetTimer(Some(self.ctl), T_FETCH, secs.saturating_mul(1000), None) };
    }

    fn on_fetched(&mut self, r: Fetched) {
        self.fetching = false;
        if r.plan.is_some() {
            self.plan = r.plan;
            self.plan_checked = Some(now());
        } else if r.plan_tried {
            // Failed: the plan stays as it was, and the next try waits a while.
            self.plan_checked = Some(now() - PLAN_EVERY_SECS + PLAN_RETRY_SECS);
        }
        let mut retry_after = None;
        match r.usage {
            Ok(u) => {
                if let Some(w) = u.weekly {
                    // The percentage fell, or jumped, within its window: likely the
                    // plan changed (a bigger limit, or a smaller one). Its name is
                    // asked for only every few hours: now, with the next refresh.
                    let last = self.history.last().filter(|s| s.resets_at == w.resets_at || s.resets_at.is_none());
                    if last.is_some_and(|s| w.used < s.used - 0.5 || w.used > s.used + PLAN_JUMP) {
                        self.plan_checked = None;
                    }
                    forecast::record(
                        &mut self.history,
                        forecast::Sample { at: now(), used: w.used, resets_at: w.resets_at, plan: self.plan.clone() },
                    );
                }
                self.usage = Some(u);
                self.fetched_at = Some(now());
                self.error = None;
                self.rate_limited = 0;
                self.warn_limits();
                self.save();
            }
            Err(e) => {
                self.rate_limited = match e {
                    FetchError::RateLimited(after) => {
                        retry_after = after;
                        self.rate_limited + 1
                    }
                    _ => 0,
                };
                self.error = Some(e);
            }
        }
        self.fetch_not_before =
            retry_after.map(|s| Instant::now() + Duration::from_secs(u64::from(s.min(MAX_RETRY_AFTER_SECS))));
        self.schedule_fetch(fetch_wait(self.interval, self.rate_limited, retry_after));
        self.refresh_views();
    }

    fn stale(&self) -> bool {
        self.error.is_some() && self.fetched_at.is_none_or(|t| now() - t > STALE_AFTER_SECS)
    }

    // ---------- sessions ----------

    fn housekeeping(&mut self) {
        let at = now_ms();
        if self.stats_closed_at.is_some_and(|t| at - t > STATS_KEEP_MS) {
            // Statistics not looked at for a while: free what they read.
            // On a thread: it waits for a transcript another thread may be reading.
            std::thread::spawn(crate::stats::forget);
            self.stats_closed_at = None;
        }
        let changed = self.sessions.retain_alive(|s| match (&s.distro, s.claude_pid) {
            // WSL sessions are checked by the collector, inside the distribution.
            (Some(_), _) => true,
            (None, Some(pid)) => win::process_alive(pid, s.claude_started),
            // Without a pid we can't tell; drop it after a day without events.
            (None, None) => at - s.updated_ms < 24 * 3600 * 1000,
        });
        if changed {
            self.save();
        }
        self.request_snapshot(false);
        // A limit window just reset: fetch fresh numbers instead of waiting.
        let reset_passed = self.usage.is_some_and(|u| {
            [u.five_hour, u.weekly]
                .iter()
                .flatten()
                .any(|l| l.resets_at.is_some_and(|t| t <= now() && self.fetched_at.is_some_and(|f| f < t)))
        });
        if reset_passed && self.error.is_none() {
            self.start_fetch();
        }
        self.refresh_views();
    }

    /// Asks the collector for fresh session data.
    fn request_snapshot(&self, force: bool) {
        let known = self
            .sessions
            .iter()
            .map(|s| collector::Known { id: s.id.clone(), cwd: s.cwd.clone(), distro: s.distro.clone() })
            .collect();
        let _ = self.collector.send(collector::Request {
            known,
            force,
            transcripts: self.prefs.transcripts,
            wsl: self.prefs.wsl,
            gpg: self.prefs.gpg_timer,
        });
    }

    fn on_snapshot(&mut self, snap: Snapshot) {
        // As of when the lists were read: a pass can take a while after that.
        let at = if snap.listed_at_ms > 0 { snap.listed_at_ms } else { now_ms() };
        self.awaiting_snapshot = false;
        let before = self.statuses();
        let mut changed = self.sessions.sync(&snap.listed, at);
        changed |= self.sessions.retain_wsl(&snap.wsl_running, &snap.wsl_scanned, &snap.listed);
        for (id, stats) in snap.stats {
            changed |= self.sessions.set_stats(&id, stats);
        }
        for (id, name) in snap.workspaces {
            changed |= self.sessions.set_workspace(&id, name);
        }
        self.announce(&before);
        if let Some(reading) = snap.gpg {
            if self.gpg.update(reading, now()) {
                self.save();
            }
        }
        if let Some(archived) = &snap.archived {
            changed |= self.sessions.set_archived(archived);
        }
        if snap.vscode_exe.is_some() {
            self.vscode_exe = snap.vscode_exe;
        }
        if changed {
            self.save();
        }
        self.refresh_views();
    }

    /// Marks a finished session as seen once its VS Code window has been in
    /// front for a moment after it finished (see `vscode::seen_in`).
    fn notice_focus(&mut self) {
        let window = if crate::demo::active() { None } else { win::foreground_window() };
        let Some((hwnd, _, title)) = window.filter(|(_, pid, _)| self.is_vscode(*pid)) else {
            self.focus = None;
            return;
        };
        // A new window or tab (the title names the active editor) starts again.
        let since = match &self.focus {
            Some((h, t, since)) if *h == hwnd && *t == title => *since,
            _ => Instant::now(),
        };
        self.focus = Some((hwnd, title, since));
        if since.elapsed() < FOCUS_SEEN {
            return;
        }
        let at = now_ms();
        let finished = FOCUS_SEEN.as_millis() as i64;
        let unseen: Vec<vscode::Unseen> = self
            .sessions
            .iter()
            .filter(|s| s.status == Status::Done && s.in_vscode() && s.done_at.is_some_and(|d| at - d >= finished))
            .map(|s| vscode::Unseen {
                id: &s.id,
                window: s.workspace.as_deref().unwrap_or_else(|| s.project()),
                title: s.description(),
            })
            .collect();
        let Some((_, title, _)) = &self.focus else { return };
        let seen: Vec<String> = vscode::seen_in(title, &unseen).into_iter().map(str::to_string).collect();
        let mut changed = false;
        for id in seen {
            changed |= self.sessions.mark_seen(&id, at);
        }
        if changed {
            self.save();
            self.refresh_views();
        }
    }

    /// The system monitor's cells for the widget, from the last samples.
    fn monitor_view(&self) -> Option<widget::Monitor> {
        use crate::sysmon::{self, Sample};
        let m = self.prefs.monitor;
        if m.bits() == 0 {
            return None;
        }
        let last = self.monitor.back().copied().unwrap_or_default();
        let dash = || "–".to_string();
        // The graph of a value, as shares of 0..1.
        let history = |f: &dyn Fn(&Sample) -> Option<f64>| -> Vec<f32> {
            if !m.graphs {
                return Vec::new();
            }
            self.monitor.iter().map(|s| f(s).unwrap_or(0.0).clamp(0.0, 1.0) as f32).collect()
        };
        let percent = |label, f: &dyn Fn(&Sample) -> Option<f32>| widget::Cell {
            label,
            value: f(&last).map_or_else(dash, |v| format!("{v:.0}%")),
            widest: &["100%"],
            level: f(&last),
            history: history(&|s| f(s).map(|v| f64::from(v) / 100.0)),
        };
        // An amount of memory (in use, size if known), drawn against its size
        // or else the most shown; red near full, like a percentage.
        let amount = |label, f: &dyn Fn(&Sample) -> Option<sysmon::Amount>| {
            let most = self.monitor.iter().filter_map(f).map(|(used, size)| size.unwrap_or(used)).fold(1.0, f64::max);
            widget::Cell {
                label,
                value: f(&last).map_or_else(dash, |(used, _)| sysmon::gb(used)),
                widest: &["88.8 GB", "888 GB"],
                level: f(&last).and_then(|(used, size)| size.map(|s| (used / s * 100.0) as f32)),
                history: history(&|s| f(s).map(|(used, size)| used / size.unwrap_or(most))),
            }
        };
        // Rates are drawn against the busiest second shown (at least 100 KB/s).
        let rate = |label, f: &dyn Fn(&Sample) -> Option<f64>| {
            let max = self.monitor.iter().filter_map(f).fold(100_000.0, f64::max);
            widget::Cell {
                label,
                value: f(&last).map_or_else(dash, sysmon::rate),
                widest: &["888 KB/s", "88 MB/s", "8.8 GB/s"],
                level: None,
                history: history(&|s| f(s).map(|v| v / max)),
            }
        };
        // Network (and disk use) left, disk rates next, CPU, memory and GPU right.
        let mut columns = Vec::new();
        let mut left = Vec::new();
        if m.net {
            left.push(rate("↑", &|s| s.up));
            left.push(rate("↓", &|s| s.down));
        }
        if m.disk && !m.disk_rates {
            left.push(percent("Disk", &|s| s.disk));
        }
        columns.push(left);
        if m.disk && m.disk_rates {
            columns.push(vec![rate("R", &|s| s.disk_read), rate("W", &|s| s.disk_write)]);
        }
        let mut right = Vec::new();
        if m.cpu {
            right.push(percent("CPU", &|s| s.cpu));
        }
        if m.ram {
            right.push(if m.ram_gb {
                amount("RAM", &|s| s.ram_bytes.map(|(used, size)| (used, Some(size))))
            } else {
                percent("RAM", &|s| s.ram)
            });
        }
        if m.gpu {
            right.push(if m.gpu_memory { amount("GPU", &|s| s.vram) } else { percent("GPU", &|s| s.gpu) });
        }
        columns.push(right);
        columns.retain(|c| !c.is_empty());
        Some(widget::Monitor { columns })
    }

    /// Whether process `pid` is VS Code (remembered per pid).
    fn is_vscode(&mut self, pid: u32) -> bool {
        if self.vscode_pids.len() > 64 {
            self.vscode_pids.clear();
        }
        *self.vscode_pids.entry(pid).or_insert_with(|| {
            win::process_image(pid)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()))
                .is_some_and(|n| n == "code.exe" || n == "code - insiders.exe")
        })
    }

    // ---------- manual refresh ----------

    fn refresh_state(&self) -> flyout::Refresh {
        let Some(started) = self.refresh_started else { return flyout::Refresh::Ready };
        let elapsed = started.elapsed();
        if self.fetching || self.awaiting_snapshot || elapsed < MIN_SPIN {
            // One turn per second; standing still without animations.
            let angle = if self.prefs.animations { ((elapsed.as_millis() * 360 / 1000) % 360) as u16 } else { 0 };
            flyout::Refresh::Busy(angle)
        } else if elapsed < REFRESH_REST {
            flyout::Refresh::Resting
        } else {
            flyout::Refresh::Ready
        }
    }

    /// Reloads usage and sessions now, unless that just happened.
    fn manual_refresh(&mut self) {
        if self.refresh_state() != flyout::Refresh::Ready {
            return;
        }
        self.refresh_started = Some(Instant::now());
        self.awaiting_snapshot = true;
        self.copilot_poke.store(true, Ordering::Relaxed);
        self.start_fetch();
        self.request_snapshot(true);
        self.refresh_views();
    }

    // ---------- announcements ----------

    fn statuses(&self) -> HashMap<String, Status> {
        self.sessions.iter().map(|s| (s.id.clone(), s.status)).collect()
    }

    /// Plays a sound and shows a bubble for sessions that just started to
    /// need the user or just finished. Sessions seen for the first time
    /// (e.g. at start-up) are not announced.
    fn announce(&mut self, before: &HashMap<String, Status>) {
        if self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        let mut needs: Option<&Session> = None;
        let mut done: Option<&Session> = None;
        for s in self.sessions.iter().filter(|s| self.listed(s)) {
            let Some(prev) = before.get(&s.id) else { continue };
            match (prev, s.status) {
                (Status::NeedsInput, Status::NeedsInput) => {}
                // Given back after a tool event of parallel work: announced already.
                (_, Status::NeedsInput) if s.restored => {}
                (_, Status::NeedsInput) => needs = Some(s),
                (Status::Working | Status::NeedsInput, Status::Done) => done = Some(s),
                _ => {}
            }
        }
        let bubble = needs.map(|s| (s, Status::NeedsInput)).or(done.map(|s| (s, Status::Done)));
        let bubble = bubble.map(|(s, status)| {
            let name = s.display_name();
            let headline = match status {
                Status::NeedsInput => i18n::f("{0} needs you", &[&name]),
                _ => i18n::f("{0} is done", &[&name]),
            };
            bubble::Model {
                dark: self.dark,
                session_id: s.id.clone(),
                status,
                urgent: false,
                headline,
                detail: s.description().unwrap_or_default().to_string(),
                badge: None,
            }
        });
        let (needs, done) = (needs.is_some(), done.is_some());
        if let Some(m) = &bubble {
            self.push_away(&m.headline, m.status);
            let kind = if m.status == Status::NeedsInput { "needs_you" } else { "done" };
            let name = self.sessions.get(&m.session_id).map(|s| s.display_name());
            self.mqtt_event(kind, name, &m.headline);
        }
        if needs {
            self.play(sound::Kind::NeedsYou);
        }
        if done && !needs {
            self.play(sound::Kind::Done);
        }
        // Presenting, full screen or Do not disturb: the widget's dots still say it.
        if let Some(model) = bubble.filter(|_| self.prefs.bubbles && !self.flyout.visible() && !self.quiet()) {
            self.show_bubble(model);
        }
    }

    /// Away mode: while nobody has touched mouse or keyboard for the chosen
    /// time, a session that needs the user (or is done) goes to the phone too.
    /// Snoozing stops it; presenting and Do not disturb don't (that's this PC).
    fn push_away(&mut self, text: &str, status: Status) {
        let p = &self.prefs;
        if !p.away || (status == Status::Done && !p.away_done) || self.snoozed_until.is_some_and(|u| u > now()) {
            return;
        }
        let Some(url) = p.push_url.clone() else { return };
        if win::idle_secs() < u64::from(p.away_minutes) * 60 {
            return;
        }
        self.send_push(url, text.to_string(), false);
    }

    /// The update check: once a day (unless switched off), or now when asked,
    /// on a background thread; the answer comes back as `Update`. The demo
    /// never asks.
    fn check_update(&mut self, now_asked: bool) {
        if self.update_checking || crate::demo::active() {
            return;
        }
        if !now_asked {
            let due = self.update_checked.is_none_or(|t| now() - t >= UPDATE_EVERY_SECS);
            if !self.prefs.update_check || !due || self.started.elapsed() < STARTUP_QUIET {
                return;
            }
        }
        self.update_checking = true;
        self.refresh_views();
        let ctl = SendHwnd::new(self.ctl);
        std::thread::spawn(move || {
            win::background_thread();
            post_boxed(ctl, WM_APP_UPDATE, crate::update::check());
        });
    }

    /// A bubble, once per version, when a newer release is out; it waits while it's quiet.
    fn announce_update(&mut self) {
        let Some(u) = &self.update else { return };
        if self.update_announced.as_deref() == Some(u.version.as_str())
            || !self.prefs.bubbles
            || self.quiet()
            || self.bubble.visible()
            || self.flyout.visible()
        {
            return;
        }
        let model = bubble::Model {
            dark: self.dark,
            session_id: format!("url:{}", u.url),
            status: Status::Done,
            urgent: false,
            headline: i18n::f("Trayward {0} is out", &[&u.version]),
            detail: i18n::t("Click for what's new and the download").into(),
            badge: None,
        };
        self.update_announced = Some(u.version.clone());
        self.save();
        self.show_bubble(model);
    }

    /// Kept statistics: soon after start and then every 6 hours, the numbers
    /// of transcripts that changed are saved (`keep`), on a background thread.
    fn keep_stats(&mut self) {
        crate::keep::set_enabled(self.prefs.keep_stats);
        if !self.prefs.keep_stats || self.started.elapsed() < KEEP_AFTER {
            return;
        }
        if self.kept_at.is_some_and(|t| t.elapsed() < KEEP_EVERY) || self.keeping.load(Ordering::Relaxed) {
            return;
        }
        self.kept_at = Some(Instant::now());
        self.keeping.store(true, Ordering::Relaxed);
        let keeping = self.keeping.clone();
        let config_dir = self.config_dir.clone();
        let wsl = self.prefs.wsl;
        std::thread::spawn(move || {
            win::background_thread();
            let mut roots = vec![crate::registry::Source { distro: None, config_dir }];
            if wsl {
                for d in crate::registry::wsl_running().unwrap_or_default() {
                    roots.extend(crate::registry::wsl_sources(&d));
                }
            }
            if crate::keep::keep_all(&roots, now()) > 0 {
                // Reading transcripts took memory the app no longer needs.
                win::trim_heap();
            }
            keeping.store(false, Ordering::Relaxed);
        });
    }

    /// What Settings → About says about updates.
    fn update_status(&self) -> String {
        if crate::demo::active() {
            return i18n::t("The demo doesn't check").into();
        }
        if self.update_checking {
            return i18n::t("Checking…").into();
        }
        if let Some(e) = &self.update_error {
            return i18n::f("Couldn't check: {0}", &[e]);
        }
        match self.update_checked {
            Some(t) => i18n::f("Up to date · checked {0}", &[&timefmt::local_time(t, true)]),
            None => i18n::t("Not checked yet").into(),
        }
    }

    /// Sends a push on a background thread; the result comes back as `Pushed`.
    fn send_push(&self, url: String, text: String, test: bool) {
        let ctl = SendHwnd::new(self.ctl);
        std::thread::spawn(move || {
            win::background_thread();
            let r = crate::push::send(&url, &text);
            post_boxed(ctl, WM_APP_PUSH, (test, r));
        });
    }

    fn set_push_status(&mut self, s: Option<String>) {
        if self.push_status != s {
            self.push_status = s;
            if self.settings.visible() {
                let model = self.settings_model();
                self.settings.set_model(model);
            }
        }
    }

    /// Sounds and bubbles wait: Windows says the user is busy (presenting, Do
    /// not disturb), or the user snoozed them.
    fn quiet(&self) -> bool {
        self.snoozed_until.is_some_and(|u| u > now()) || sound::quiet()
    }

    /// A sound and a bubble when a session of another provider (`name`: Codex,
    /// Copilot) starts needing the user or finishes.
    fn announce_other(&mut self, name: &str, before: &[codex::Session], next: &[codex::Session]) {
        if self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        let before = |id: &str| before.iter().find(|s| s.id == id).map(|s| s.status);
        let mut news = None;
        for s in next {
            match (before(&s.id), s.status) {
                (Some(Status::NeedsInput), Status::NeedsInput) | (None, _) => {}
                (Some(_), Status::NeedsInput) => news = Some((s, Status::NeedsInput)),
                (Some(Status::Working | Status::NeedsInput), Status::Done) if news.is_none() => {
                    news = Some((s, Status::Done));
                }
                _ => {}
            }
        }
        let Some((s, status)) = news else { return };
        let headline = match status {
            Status::NeedsInput => i18n::f("{0} in {1} needs you", &[&name, &s.project()]),
            _ => i18n::f("{0} in {1} is done", &[&name, &s.project()]),
        };
        let model = bubble::Model {
            dark: self.dark,
            session_id: s.id.clone(),
            status,
            urgent: false,
            headline,
            detail: s.title.clone(),
            badge: None,
        };
        self.push_away(&model.headline, status);
        let kind = if status == Status::NeedsInput { "needs_you" } else { "done" };
        self.mqtt_event(kind, Some(format!("{name} · {}", s.project())), &model.headline);
        self.play(if status == Status::NeedsInput { sound::Kind::NeedsYou } else { sound::Kind::Done });
        if self.prefs.bubbles && !self.flyout.visible() && !self.quiet() {
            self.show_bubble(model);
        }
    }

    fn play(&mut self, kind: sound::Kind) {
        if self.prefs.muted || self.snoozed_until.is_some_and(|u| u > now()) {
            return;
        }
        if self.last_sound.get(&kind).is_some_and(|t| t.elapsed() < SOUND_GAP) {
            return;
        }
        self.last_sound.insert(kind, Instant::now());
        let choice = match kind {
            sound::Kind::NeedsYou => &self.prefs.needs_sound,
            sound::Kind::Done => &self.prefs.done_sound,
        };
        sound::play(kind, choice, self.prefs.volume, false);
    }

    /// A bubble when a limit gets close, once per limit window: the weekly
    /// limit at the chosen level and 95 %, the 5-hour limit at its level, and
    /// a pace that would use up the weekly limit before it resets (Settings →
    /// Limits). While it's quiet (presenting, Do not disturb) it waits.
    fn warn_limits(&mut self) {
        // Right after start, wait for the next reading rather than lose the warning.
        if self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        let Some(u) = self.usage else { return };
        let t = now();
        // Windows that have reset are forgotten.
        self.warned.retain(|(_, reset)| *reset > t - 3600);
        let levels = crate::alerts::Levels {
            weekly: self.prefs.warn_weekly,
            five_hour: self.prefs.warn_five_hour,
            pace: self.prefs.warn_pace,
        };
        let due = crate::alerts::warnings(&u, levels, &self.history, t);
        // Shown later rather than lost.
        if due.is_empty() || self.quiet() {
            return;
        }
        let Some(w) = crate::alerts::take_fresh(due, &mut self.warned) else { return };
        self.mqtt_event("limit_warning", None, &w.headline);
        if self.prefs.bubbles {
            self.show_bubble(bubble::Model {
                dark: self.dark,
                session_id: String::new(),
                status: Status::NeedsInput,
                urgent: true,
                headline: w.headline,
                detail: w.detail,
                badge: None,
            });
        }
    }

    /// A bubble when Copilot's monthly limit reaches the weekly limit's
    /// warning level (Settings → Limits), and again at 95 %; once per month each.
    fn warn_copilot(&mut self) {
        if self.started.elapsed() < STARTUP_QUIET || self.quiet() {
            return;
        }
        let Some((limit, reset)) = self.copilot_shown().and_then(|q| Some((q.limit, q.limit.resets_at?))) else {
            return;
        };
        let (t, level) = (now(), self.prefs.warn_weekly);
        let used = limit.used_at(t);
        let steps: &[u8] = match level {
            0 => &[],
            l if l < 95 => &[95, l],
            _ => &[95],
        };
        let due: Vec<crate::alerts::Warning> = steps
            .iter()
            .filter(|th| used >= f64::from(**th))
            .map(|th| crate::alerts::Warning {
                key: format!("copilot{th}"),
                reset,
                headline: i18n::f("Copilot's monthly limit at {0} %", &[&format!("{used:.0}")]),
                detail: i18n::f("Resets {0}", &[&timefmt::local_time(reset, true)]),
            })
            .collect();
        let Some(w) = crate::alerts::take_fresh(due, &mut self.warned) else { return };
        // Remembered across restarts, like the other warnings.
        self.save();
        self.mqtt_event("limit_warning", None, &w.headline);
        if self.prefs.bubbles {
            self.show_bubble(bubble::Model {
                dark: self.dark,
                session_id: String::new(),
                status: Status::NeedsInput,
                urgent: true,
                headline: w.headline,
                detail: w.detail,
                badge: None,
            });
        }
    }

    /// A bubble when a limit that was nearly used up resets (Settings → Limits),
    /// once per window; then fresh numbers.
    fn announce_resets(&mut self) {
        let Some(u) = self.usage else { return };
        if !self.prefs.notify_free || self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        let free = crate::alerts::freed(&u, &self.warned, now());
        if free.is_empty() || self.quiet() {
            return;
        }
        for f in &free {
            self.warned.push((f.key.to_string(), f.reset));
        }
        self.mqtt_event("free_again", None, crate::alerts::freed_headline(&free));
        self.play(sound::Kind::Done);
        if self.prefs.bubbles {
            self.show_bubble(bubble::Model {
                dark: self.dark,
                session_id: String::new(),
                status: Status::Done,
                urgent: false,
                headline: crate::alerts::freed_headline(&free).to_string(),
                detail: i18n::t("Back to work, Claude is ready.").into(),
                badge: None,
            });
        }
        self.save();
        self.start_fetch();
    }

    fn show_bubble(&mut self, model: bubble::Model) {
        self.hide_tip();
        let Some((anchor, dpi)) = self.widgets.anchor_in_front() else { return };
        self.bubble.show(model, anchor, dpi);
        // SAFETY: our own window and timer id.
        unsafe { SetTimer(Some(self.ctl), T_BUBBLE, BUBBLE_MS, None) };
    }

    /// The widget's tooltip: limits with reset times, plan and sessions.
    fn show_tip(&mut self) {
        let Some(hwnd) = self.widgets.hovered() else { return };
        if self.flyout.visible() || self.bubble.visible() {
            return;
        }
        let Some((anchor, dpi)) = self.widgets.rect_of(hwnd) else { return };
        self.tip.show(self.tip_model(), anchor, dpi);
    }

    fn tip_model(&self) -> tip::Model {
        let t = now();
        let (u, simulated) = self.shown_usage();
        let mut rows = Vec::new();
        let mut warnings = Vec::new();
        let limit = |l: usage::Limit, weekday: bool| {
            let used = format!("{:.0} %", l.used_at(t));
            match l.resets_at {
                Some(r) => i18n::f("{0} · resets {1}", &[&used, &timefmt::local_time(r, weekday)]),
                None => used,
            }
        };
        if let Some(w) = u.weekly {
            rows.push((i18n::t("Weekly").to_string(), limit(w, true)));
        }
        if let Some(five) = u.five_hour {
            let label = if simulated { "5-hour (simulated)" } else { "5-hour" };
            rows.push((i18n::t(label).to_string(), limit(five, false)));
        }
        if let Some(day) = self.week_plan().map(|p| p.today_day()) {
            let pct = |v: f64| if v < 9.95 { format!("{v:.1} %") } else { format!("{v:.0} %") };
            let text = match day.used {
                Some(used) if day.over() => i18n::f(
                    "{0} of {1} daily budget · {2} over",
                    &[&pct(used), &pct(day.budget), &pct(used - day.budget)],
                ),
                Some(used) => i18n::f("{0} of {1} daily budget", &[&pct(used), &pct(day.budget)]),
                None => i18n::f("{0} daily budget", &[&pct(day.budget)]),
            };
            if day.over() {
                warnings.push(rows.len());
            }
            rows.push((i18n::t("Today").to_string(), text));
        }
        if let Some(plan) = &self.plan {
            rows.push((i18n::t("Plan").to_string(), plan.clone()));
        }
        if let (true, Some(l)) = (self.prefs.codex, self.codex.limits) {
            if let Some(w) = l.weekly {
                rows.push((i18n::t("Codex weekly").to_string(), limit(w, true)));
            }
            if let Some(five) = l.five_hour {
                rows.push((i18n::t("Codex 5-hour").to_string(), limit(five, false)));
            }
        }
        if let Some(q) = self.copilot_shown() {
            rows.push((i18n::t("Copilot monthly").to_string(), limit(q.limit, true)));
        }
        if self.prefs.mailbox && self.mailbox.root.is_some() {
            let waiting: Vec<&str> =
                self.mailbox.inboxes.iter().filter(|b| b.unread_count > 0).map(|b| b.name.as_str()).collect();
            let text = match self.mailbox.unread() {
                0 => i18n::t("All handled").to_string(),
                n => i18n::f("{0} unread · {1}", &[&n, &waiting.join(", ")]),
            };
            if !waiting.is_empty() {
                warnings.push(rows.len());
            }
            rows.push((i18n::t("Mailbox").to_string(), text));
        }
        let [needs, done, working] = self.session_counts();
        let mut parts = Vec::new();
        // The flyout's templates; "… " (with a space) is the plural.
        for (n, one, many) in [
            (needs, "{0} needs you", "{0} need you"),
            (working, "{0} working", "{0} working "),
            (done, "{0} done", "{0} done "),
        ] {
            if n > 0 {
                parts.push(i18n::f(if n == 1 { one } else { many }, &[&n]).trim_end().to_string());
            }
        }
        let sessions = if parts.is_empty() { i18n::t("None active").to_string() } else { parts.join(" · ") };
        rows.push((i18n::t("Sessions").to_string(), sessions));
        if let Some(until) = self.snoozed_until.filter(|u| *u > t) {
            rows.push((i18n::t("Snoozed").to_string(), i18n::f("until {0}", &[&timefmt::local_time(until, true)])));
        }
        if self.stale() {
            let when = self.fetched_at.map_or_else(|| i18n::t("never").to_string(), |at| timefmt::local_time(at, true));
            rows.push((i18n::t("Updated").to_string(), i18n::f("{0} (refresh failed)", &[&when])));
        }
        let system = self.monitor_rows();
        if !system.is_empty() {
            rows.push(tip::DIVIDER);
            rows.extend(system);
        }
        tip::Model { dark: self.dark, rows, warnings }
    }

    /// The system monitor's last second in detail, for the tooltip: each
    /// value switched on, with both its amount and its share.
    fn monitor_rows(&self) -> Vec<(String, String)> {
        use crate::sysmon::{gb, rate};
        let m = self.prefs.monitor;
        let Some(s) = self.monitor.back().filter(|_| m.bits() != 0) else { return Vec::new() };
        let pct = |v: f32| format!("{v:.0} %");
        let of = |used: f64, size: f64| {
            i18n::f("{0} of {1} · {2}", &[&gb(used), &gb(size), &pct((used / size * 100.0) as f32)])
        };
        let mut rows = Vec::new();
        if let Some(v) = s.cpu.filter(|_| m.cpu) {
            rows.push(("CPU".to_string(), pct(v)));
        }
        if let Some((used, size)) = s.ram_bytes.filter(|_| m.ram) {
            rows.push(("RAM".to_string(), of(used, size)));
        }
        if m.gpu {
            if let Some(v) = s.gpu {
                rows.push(("GPU".to_string(), i18n::f("{0} (busiest engine)", &[&pct(v)])));
            }
            match s.vram {
                Some((used, Some(size))) => rows.push((i18n::t("Video memory").to_string(), of(used, size))),
                Some((used, None)) => rows.push((i18n::t("Video memory").to_string(), gb(used))),
                None => {}
            }
        }
        if let (true, Some(up), Some(down)) = (m.net, s.up, s.down) {
            rows.push((
                i18n::t("Network").to_string(),
                i18n::f("↑ {0} sent · ↓ {1} received", &[&rate(up), &rate(down)]),
            ));
        }
        if m.disk {
            let mut parts = Vec::new();
            if let Some(v) = s.disk {
                parts.push(i18n::f("{0} busy", &[&pct(v)]));
            }
            if let Some(r) = s.disk_read {
                parts.push(i18n::f("read {0}", &[&rate(r)]));
            }
            if let Some(w) = s.disk_write {
                parts.push(i18n::f("write {0}", &[&rate(w)]));
            }
            if !parts.is_empty() {
                rows.push((i18n::t("Disk").to_string(), parts.join(" · ")));
            }
        }
        rows
    }

    fn hide_tip(&mut self) {
        // SAFETY: our own window and timer id.
        let _ = unsafe { KillTimer(Some(self.ctl), T_TIP) };
        self.tip.hide();
    }

    fn hide_bubble(&mut self) {
        self.bubble.hide();
        // SAFETY: our own window and timer id.
        let _ = unsafe { KillTimer(Some(self.ctl), T_BUBBLE) };
    }

    fn set_prefs(&mut self, p: settings::Prefs) {
        settings::set_prefs(&p);
        crate::i18n::set(p.language);
        let poll_changed = p.session_poll_secs != self.prefs.session_poll_secs;
        self.codex_on.store(p.codex, Ordering::Relaxed);
        self.copilot_on.store(p.copilot, Ordering::Relaxed);
        self.mailbox_on.store(p.mailbox, Ordering::Relaxed);
        if let Ok(mut dir) = self.mailbox_dir.lock() {
            dir.clone_from(&p.mailbox_dir);
        }
        // Switched on or off, or another folder: read again now; when on, with a
        // turning arrow in the settings until the reader answered.
        if p.mailbox != self.prefs.mailbox || p.mailbox_dir != self.prefs.mailbox_dir {
            self.mailbox = mailbox::Snapshot::default();
            self.mailbox_search = p.mailbox.then(|| (Instant::now(), false));
            self.mailbox_poke.store(true, Ordering::Relaxed);
        }
        copilot::set_on(p.copilot);
        self.copilot_quota_on.store(p.copilot && p.copilot_limit, Ordering::Relaxed);
        // Codex's and Copilot's last readings stay while switched off (every
        // view checks the switch): switched on again before their readers
        // noticed, they'd have nothing new to say.
        self.monitor_on.store(p.monitor.bits(), Ordering::Relaxed);
        if p.monitor.bits() == 0 {
            self.monitor.clear();
        }
        self.forge_on.store(p.forge, Ordering::Relaxed);
        if !p.forge {
            self.forge = forge::Snapshot::default();
        }
        let language_changed = p.language != self.prefs.language;
        let transcripts_off = self.prefs.transcripts && !p.transcripts;
        self.prefs = p;
        if language_changed {
            self.stats.retranslate();
            self.settings.retitle();
        }
        if poll_changed {
            // SAFETY: our own window; re-setting a timer id replaces it.
            unsafe { SetTimer(Some(self.ctl), T_HOUSEKEEPING, self.prefs.session_poll_secs * 1000, None) };
        }
        if transcripts_off {
            let ids: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
            for id in ids {
                self.sessions.set_stats(&id, transcripts::Stats::default());
            }
            self.save();
        }
        if !self.prefs.bubbles {
            self.hide_bubble();
        }
        self.configure_mqtt();
        self.request_snapshot(true);
        self.refresh_views();
    }

    /// Hands the MQTT thread the broker to use: the one in the settings while
    /// MQTT is on (never in the demo), else none.
    fn configure_mqtt(&mut self) {
        let broker = (self.prefs.mqtt && !crate::demo::active())
            .then(|| self.prefs.mqtt_broker.as_deref().and_then(|b| crate::mqtt::parse(b).ok()))
            .flatten()
            .map(|b| crate::mqtt::Broker { password: settings::mqtt_password(), ..b });
        if broker != self.mqtt_broker {
            self.mqtt_status = None;
            self.mqtt_sent.clear();
            self.mqtt_broker = broker.clone();
            let _ = self.mqtt.send(crate::mqtt::Command::Config(broker));
            self.publish_mqtt_state();
        }
    }

    /// The state for MQTT: limits, plan and sessions (names and states only).
    fn mqtt_state(&self) -> serde_json::Value {
        let t = now();
        let (u, _) = self.shown_usage();
        let used = |l: Option<usage::Limit>| l.map(|l| l.used_at(t).round());
        let resets = |l: Option<usage::Limit>| l.and_then(|l| l.resets_at).map(timefmt::rfc3339);
        let state = |s: Status| match s {
            Status::NeedsInput => "needs_you",
            Status::Working => "working",
            Status::Done => "done",
            Status::Idle => "idle",
        };
        let listed: Vec<&Session> = self.sessions.iter().filter(|s| self.listed(s)).collect();
        let count = |st: Status| listed.iter().filter(|s| s.status == st).count();
        // As the widget's dots count them.
        let others = |st: Status| self.other_sessions().filter(|(s, _)| s.status == st).count();
        let sessions: Vec<serde_json::Value> = listed
            .iter()
            .map(|s| {
                let name = s.display_name();
                serde_json::json!({ "name": name, "state": state(s.status) })
            })
            .collect();
        let codex = self.codex.limits.filter(|_| self.codex_shown());
        serde_json::json!({
            "weekly": used(u.weekly),
            "weekly_resets": resets(u.weekly),
            "five_hour": used(u.five_hour),
            "five_hour_resets": resets(u.five_hour),
            "plan": self.plan,
            "needs_you": count(Status::NeedsInput) + others(Status::NeedsInput),
            "working": count(Status::Working) + others(Status::Working),
            "done": count(Status::Done),
            "sessions": sessions,
            "codex_weekly": codex.and_then(|c| used(c.weekly)),
            "codex_five_hour": codex.and_then(|c| used(c.five_hour)),
            "copilot_monthly": self.copilot_shown().and_then(|q| used(Some(q.limit))),
        })
    }

    /// Sends the state to the broker when it changed.
    fn publish_mqtt_state(&mut self) {
        if self.mqtt_broker.is_none() {
            return;
        }
        let state = self.mqtt_state().to_string();
        if state != self.mqtt_sent {
            let _ = self.mqtt.send(crate::mqtt::Command::State(state.clone()));
            self.mqtt_sent = state;
        }
    }

    /// Sends an event to the broker (for automations), whether or not a bubble shows.
    fn mqtt_event(&self, kind: &str, session: Option<String>, text: &str) {
        if self.mqtt_broker.is_some() {
            let e = serde_json::json!({ "event_type": kind, "session": session, "text": text });
            let _ = self.mqtt.send(crate::mqtt::Command::Event(e.to_string()));
        }
    }

    /// What Settings → MQTT says about the connection (None: nothing to say).
    fn mqtt_line(&self) -> Option<String> {
        if let Some(n) = &self.mqtt_note {
            return Some(n.clone());
        }
        if crate::demo::active() {
            return Some(i18n::t("The demo connects nowhere").into());
        }
        if !self.prefs.mqtt || self.prefs.mqtt_broker.is_none() {
            return None;
        }
        Some(match &self.mqtt_status {
            None => i18n::t("Connecting…").to_string(),
            Some(Ok(())) => i18n::t("Connected").to_string(),
            Some(Err(e)) => i18n::f("Not connected: {0}", &[e]),
        })
    }

    fn refresh_settings(&mut self) {
        if self.settings.visible() {
            let model = self.settings_model();
            self.settings.set_model(model);
        }
    }

    // ---------- animation ----------

    /// Seconds on the animation clock.
    fn clock(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Sessions that need the user, not hidden.
    fn waiting(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter().filter(|s| self.listed(s) && s.status == Status::NeedsInput)
    }

    /// Codex's and Copilot's sessions, while they're switched on, with their provider's name.
    fn other_sessions(&self) -> impl Iterator<Item = (&codex::Session, &'static str)> {
        let codex = self.codex.sessions.iter().filter(|_| self.prefs.codex).map(|s| (s, "Codex"));
        codex.chain(self.copilot.iter().filter(|_| self.prefs.copilot).map(|s| (s, "Copilot")))
    }

    /// Codex's and Copilot's sessions that need the user.
    fn others_waiting(&self) -> impl Iterator<Item = &codex::Session> {
        self.other_sessions().map(|(s, _)| s).filter(|s| s.status == Status::NeedsInput)
    }

    /// Sessions that need the user, are done and are working (see
    /// `widget::STATES`), of every provider. Codex's and Copilot's count
    /// while they need the user or work, not when done: nothing says the
    /// user has seen them, so their dot would stay for the hour they count as done.
    fn session_counts(&self) -> [usize; 3] {
        widget::STATES.map(|state| {
            let claude = self
                .sessions
                .iter()
                .filter(|s| {
                    self.listed(s)
                        && (s.status == state
                            || (state == Status::Done && self.finished(s, now_ms()) && self.cache_expiring(s)))
                })
                .count();
            let others = self.other_sessions().filter(|(s, _)| s.status == state && state != Status::Done).count();
            claude + others
        })
    }

    /// The needs-you dot glows with a slow pulse and shakes briefly every few seconds.
    fn attention(&self) -> widget::Attention {
        let at = now_ms();
        let waiting: Vec<&Session> = self.waiting().collect();
        let others: Vec<i64> = self.others_waiting().map(|s| s.since * 1000).collect();
        // Red when a session has waited long, or its cache is about to expire.
        let urgent = waiting.iter().any(|s| at - s.since_ms >= URGENT_AFTER_MS || self.cache_expiring(s))
            || others.iter().any(|since| at - since >= URGENT_AFTER_MS);
        let done_expiring = self.sessions.iter().any(|s| self.finished(s, at) && self.cache_expiring(s));
        if waiting.is_empty() && others.is_empty() && !done_expiring {
            return widget::Attention::default();
        }
        if !self.prefs.animations {
            let glow = if waiting.is_empty() && others.is_empty() { 0.0 } else { 0.45 };
            return widget::Attention { urgent, glow, shake: 0.0, done_expiring, done_glow: 0.45 };
        }
        let t = self.clock();
        let wave = 0.5 + 0.5 * (t * std::f32::consts::TAU / PULSE_SECS).sin();
        let glow = match (waiting.is_empty(), urgent) {
            (true, _) => 0.0,
            (false, true) => 0.5 + 0.5 * wave,
            (false, false) => 0.3 + 0.45 * wave,
        };
        let phase = t % SHAKE_EVERY_SECS;
        let shake = if !waiting.is_empty() && phase < SHAKE_SECS {
            2.0 * (phase * std::f32::consts::TAU * 5.0).sin() * (1.0 - phase / SHAKE_SECS)
        } else {
            0.0
        };
        // Rounded, so the widget only redraws when it would look different.
        let round = |v: f32, steps: f32| (v * steps).round() / steps;
        widget::Attention {
            urgent,
            glow: round(glow, 20.0),
            shake: round(shake, 10.0),
            done_expiring,
            done_glow: if done_expiring { round(0.5 + 0.5 * wave, 20.0) } else { 0.0 },
        }
    }

    /// The session's prompt cache runs out within the warning time.
    fn cache_expiring(&self, s: &Session) -> bool {
        let t = now();
        self.prefs.transcripts && s.stats.cache_expires_at.is_some_and(|e| e > t && e - t <= flyout::CACHE_WARN_SECS)
    }

    /// Listed as done: finished and not seen, or finished within the hour.
    fn finished(&self, s: &Session, at_ms: i64) -> bool {
        self.listed(s) && (s.status == Status::Done || s.recently_done(at_ms, DONE_STAYS_MS))
    }

    /// Warns once when the cache of a session that waits for the user enters
    /// its last minutes: a bubble and the "needs you" sound.
    fn warn_expiring_caches(&mut self) {
        // A bubble that's showing (e.g. "… is done") goes first; the warning comes after it.
        if self.started.elapsed() < STARTUP_QUIET || !self.prefs.transcripts || self.bubble.visible() {
            return;
        }
        let at = now_ms();
        let due: Vec<(String, i64)> = self
            .sessions
            .iter()
            .filter(|s| s.status == Status::NeedsInput || self.finished(s, at))
            .filter(|s| self.listed(s) && self.cache_expiring(s))
            .filter_map(|s| s.stats.cache_expires_at.map(|e| (s.id.clone(), e)))
            .filter(|(id, e)| self.cache_warned.get(id) != Some(e))
            .collect();
        let Some((id, expires)) = due.into_iter().min_by_key(|(_, e)| *e) else { return };
        self.cache_warned.insert(id.clone(), expires);
        let Some(s) = self.sessions.get(&id) else { return };
        let name = s.display_name();
        let minutes = ((expires - now()) + 59) / 60;
        let model = bubble::Model {
            dark: self.dark,
            session_id: id.clone(),
            status: s.status,
            urgent: true,
            headline: i18n::f("{0}: cache expires in {1}m", &[&name, &minutes]),
            detail: s.description().unwrap_or_default().to_string(),
            badge: None,
        };
        self.play(sound::Kind::NeedsYou);
        if self.prefs.bubbles && !self.flyout.visible() && !self.quiet() {
            self.show_bubble(model);
        }
    }

    /// Pulse (0 to 1) for things in the flyout that ask for attention.
    fn pulse(&self) -> f32 {
        if !self.prefs.animations {
            return 0.5;
        }
        let wave = 0.5 + 0.5 * (self.clock() * std::f32::consts::TAU / PULSE_SECS).sin();
        (wave * 20.0).round() / 20.0
    }

    /// Whether anything on screen is moving: the refresh spinner, the
    /// needs-you dot, or an expiring cache timer in the open flyout.
    fn wants_animation(&self) -> bool {
        if !self.prefs.animations {
            return false;
        }
        let cache_warning = || {
            let t = now();
            self.sessions.iter().any(|s| {
                self.listed(s) && s.stats.cache_expires_at.is_some_and(|e| e > t && e - t <= flyout::CACHE_WARN_SECS)
            })
        };
        let at = now_ms();
        let done_expiring = || self.sessions.iter().any(|s| self.finished(s, at) && self.cache_expiring(s));
        self.refresh_state().is_busy()
            || (self.settings.visible() && self.mailbox_searching())
            || self.shining()
            || self.waiting().next().is_some()
            || self.others_waiting().next().is_some()
            || done_expiring()
            || (self.flyout.visible() && cache_warning())
    }

    /// Time left until each limit resets, once it's at the countdown level.
    fn countdowns(&self) -> [Option<String>; 2] {
        let (u, _) = self.shown_usage();
        crate::alerts::countdowns([u.weekly, u.five_hour], self.prefs.countdown_at, now())
    }

    /// The weekly summary: from Monday 08:00 (local time), once per week, last
    /// week's numbers in a bubble. They're summed up on a background thread;
    /// the bubble waits while it's quiet. The demo shows one 30 s after start.
    fn summarize_week(&mut self) {
        if !self.prefs.week_summary || !self.prefs.bubbles || self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        if self.week_bubble.is_some() {
            if !self.quiet() && !self.bubble.visible() && !self.flyout.visible() {
                let model = self.week_bubble.take().expect("checked above");
                self.show_bubble(model);
            }
            return;
        }
        let t = now();
        // With that Monday's own offset: on the Sunday the clocks change, today's is an hour off.
        let offset = timefmt::utc_offset(crate::alerts::week_start(t, timefmt::utc_offset(t)));
        let monday = crate::alerts::week_start(t, offset);
        let due = if crate::demo::active() {
            self.started.elapsed() >= Duration::from_secs(30)
        } else {
            t >= monday + 8 * 3600
        };
        if !due || self.summary_week == Some(monday) {
            return;
        }
        self.summary_week = Some(monday);
        self.save();
        let ctl = SendHwnd::new(self.ctl);
        let config_dir = self.config_dir.clone();
        let wsl = self.prefs.wsl;
        std::thread::spawn(move || {
            crate::win::background_thread();
            let mut roots = vec![crate::registry::Source { distro: None, config_dir }];
            if wsl {
                for d in crate::registry::wsl_running().unwrap_or_default() {
                    roots.extend(crate::registry::wsl_sources(&d));
                }
            }
            let archived = crate::vscode_state::archived_sessions().unwrap_or_default();
            // The demo's sessions are of this week.
            let (from, to) = if crate::demo::active() { (t - 7 * 86_400, t) } else { (monday - 7 * 86_400, monday) };
            let s = crate::stats::overview(&roots, from, to, &timefmt::offsets(), &archived).summary;
            let week = crate::alerts::Week {
                prompts: s.prompts,
                tokens: s.output(),
                cost: s.total_cost(),
                working: s.working_secs(),
            };
            post_boxed(ctl, WM_APP_WEEK, week);
        });
    }

    /// The pinned session for the widget, while it's still listed: its state,
    /// or its prompt cache timer while Claude waits.
    fn pinned_view(&self) -> Option<widget::Pinned> {
        let id = self.pinned.as_deref()?;
        let s = self.sessions.get(id).filter(|s| self.listed(s))?;
        let row = session_row(s, now_ms());
        let line = match row.cache_left {
            // Agents at work matter more than the cache: the session isn't finished.
            _ if row.agents > 0 => status_line(row.status, &flyout::agents_label(row.agents)),
            Some(left) if left > 0 && row.status != Status::Working => format!("🕒 {}", flyout::cache_time(left)),
            _ => row.status_text.clone(),
        };
        let expiring = row.cache_left.is_some_and(|l| l > 0 && l <= flyout::CACHE_WARN_SECS);
        Some(widget::Pinned { name: row.workspace, status: row.status, line, urgent: row.urgent || expiring })
    }

    /// The pill under a session for its branch's merge or pull request, or
    /// the branch's pipeline. `session_title` is the row's own title: the
    /// request's title is left out when it would only repeat it.
    fn change_line(&self, folder: &str, session_title: &str) -> Option<flyout::ChangeLine> {
        if !self.prefs.forge {
            return None;
        }
        let b = self.forge.sessions.get(folder)?;
        match &b.change {
            Some(c) => {
                let state = match (c.state, c.checks) {
                    (forge::State::Merged, _) => i18n::t("Merged"),
                    (forge::State::Closed, _) => i18n::t("Closed"),
                    (_, Some(k)) => flyout::checks_text(c.forge, k),
                    (_, None) if c.draft => i18n::t("Draft"),
                    (_, None) => i18n::t("open"),
                };
                Some(flyout::ChangeLine {
                    // "!39 Fix the flaky login… · Pipeline failed": a short title, so the state still fits.
                    text: match short_title(&c.title, PILL_TITLE).filter(|_| !same_topic(&c.title, session_title)) {
                        Some(title) => i18n::f("{0} {1} · {2}", &[&c.reference(), &title, &state]),
                        None => i18n::f("{0} · {1}", &[&c.reference(), &state]),
                    },
                    url: Some(c.url.clone()),
                    checks: c.checks,
                    merged: c.state == forge::State::Merged,
                })
            }
            // No merge request: the branch's own pipeline (GitLab).
            None => b.checks.map(|k| flyout::ChangeLine {
                text: i18n::f("{0} · {1}", &[&b.name, &flyout::checks_text(forge::Forge::GitLab, k)]),
                url: None,
                checks: Some(k),
                merged: false,
            }),
        }
    }

    /// Tells the merge and pull request reader which folders the sessions are in.
    fn update_forge_folders(&self) {
        let mut folders: Vec<String> = self.sessions.iter().filter(|s| self.listed(s)).map(|s| s.cwd.clone()).collect();
        folders.extend(self.other_sessions().map(|(s, _)| s.cwd.clone()));
        folders.retain(|f| !f.is_empty());
        folders.sort();
        folders.dedup();
        if let Ok(mut current) = self.forge_folders.lock() {
            if *current != folders {
                *current = folders;
                self.forge_poke.store(true, Ordering::Relaxed);
            }
        }
    }

    /// A bubble when a pipeline of the user's ends or a request is merged.
    fn announce_forge(&mut self, next: &forge::Snapshot) {
        if !self.prefs.forge_bubbles || self.started.elapsed() < STARTUP_QUIET {
            return;
        }
        let Some((c, what)) = forge::news(&self.forge, next).into_iter().next() else { return };
        let (label, tone) = match what {
            forge::News::Passed => (flyout::checks_text(c.forge, forge::Checks::Passed), bubble::Tone::Success),
            forge::News::Failed => (flyout::checks_text(c.forge, forge::Checks::Failed), bubble::Tone::Danger),
            forge::News::Canceled => (flyout::checks_text(c.forge, forge::Checks::Canceled), bubble::Tone::Warn),
            forge::News::Merged => (i18n::t("Merged"), bubble::Tone::Success),
        };
        // "!42 Checkout flow redesign"; the bubble shortens a long title.
        let headline = if c.title.is_empty() { c.reference() } else { format!("{} {}", c.reference(), c.title) };
        let failed = what == forge::News::Failed;
        self.play(if failed { sound::Kind::NeedsYou } else { sound::Kind::Done });
        if self.prefs.bubbles && !self.flyout.visible() && !self.quiet() {
            self.show_bubble(bubble::Model {
                dark: self.dark,
                // A click opens the request.
                session_id: format!("url:{}", c.url),
                status: if failed { Status::NeedsInput } else { Status::Done },
                urgent: failed,
                headline,
                // The project, without its group.
                detail: c.project.rsplit('/').next().unwrap_or_default().to_string(),
                badge: Some(bubble::Badge { forge: c.forge, tone, label: label.to_string() }),
            });
        }
    }

    /// Codex is on and has limits to show.
    fn codex_shown(&self) -> bool {
        self.prefs.codex && self.codex.limits.is_some()
    }

    /// Copilot's monthly limit, while Copilot and its limit are on.
    fn copilot_shown(&self) -> Option<&copilot::Quota> {
        self.copilot_quota.as_ref().filter(|_| self.prefs.copilot && self.prefs.copilot_limit)
    }

    /// Copilot's limit for the widget: with Codex on, only when the taskbar shows everyone's.
    fn copilot_widget(&self) -> Option<widget::Monthly> {
        let q = self.copilot_shown()?;
        if self.prefs.codex && self.prefs.taskbar_shows != settings::Shown::Both {
            return None;
        }
        let t = now();
        let [countdown, _] = crate::alerts::countdowns([Some(q.limit), None], self.prefs.countdown_at, t);
        Some(widget::Monthly {
            used: Some(q.limit.used_at(t).round() as u8),
            countdown,
            note: i18n::f("{0} left", &[&copilot_count(q.remaining)]),
        })
    }

    /// Codex's limits for the widget, when it shows them.
    fn codex_widget(&self) -> Option<widget::Limits> {
        if !self.codex_shown() || self.prefs.taskbar_shows == settings::Shown::Claude {
            return None;
        }
        let l = self.codex.limits?;
        let t = now();
        let used = |l: Option<usage::Limit>| l.map(|l| l.used_at(t).round() as u8);
        let countdown = crate::alerts::countdowns([l.weekly, l.five_hour], self.prefs.countdown_at, t);
        Some(widget::Limits { weekly: used(l.weekly), five_hour: used(l.five_hour), countdown })
    }

    /// The Ultra plan's name shines while the user looks at it: the mouse on a
    /// widget, or the flyout open.
    fn shining(&self) -> bool {
        self.prefs.animations
            && self.plan.as_deref().is_some_and(|p| crate::plan::tier(p) == crate::plan::Tier::Ultra)
            && ((self.widgets.hovered().is_some() && !self.prefs.slim) || self.flyout.visible())
    }

    fn plan_shine(&self) -> Option<f32> {
        if self.shining() { crate::plan::shine(self.clock()) } else { None }
    }

    /// Starts or stops the animation timer as needed.
    fn update_animation(&mut self) {
        let wanted = self.wants_animation();
        if wanted != self.animating {
            self.animating = wanted;
            // SAFETY: our own window and timer id.
            unsafe {
                if wanted {
                    SetTimer(Some(self.ctl), T_ANIMATION, ANIMATION_MS, None);
                } else {
                    let _ = KillTimer(Some(self.ctl), T_ANIMATION);
                }
            }
        }
    }

    fn open_session(&mut self, id: &str) {
        self.flyout.hide();
        if id.starts_with("copilot:") {
            let why = i18n::t("Copilot sessions can't be switched to from here yet.");
            post_boxed(SendHwnd::new(self.ctl), WM_APP_SWITCHED, Err::<(), String>(why.into()));
            return;
        }
        if id.starts_with("codex:") {
            let ctl = SendHwnd::new(self.ctl);
            let why = i18n::t(
                "Codex sessions can't be switched to from here yet: Codex doesn't say which terminal runs them.",
            );
            post_boxed(ctl, WM_APP_SWITCHED, Err::<(), String>(why.into()));
            return;
        }
        let Some(s) = self.sessions.get(id) else { return };
        let target = s.in_vscode().then(|| vscode::Target {
            session_id: s.id.clone(),
            cwd: s.cwd.clone(),
            ide_pid: s.ide_pid,
            distro: s.distro.clone(),
        });
        // A terminal on Windows: its console; in WSL the process id means nothing here.
        let terminal = (!s.in_vscode() && s.distro.is_none()).then_some(s.claude_pid).flatten();
        let cwd = s.cwd.clone();
        let in_wsl = s.distro.is_some();
        if self.sessions.mark_seen(id, now_ms()) {
            self.save();
            self.refresh_views();
        }
        let ctl = SendHwnd::new(self.ctl);
        let dir = self.config_dir.clone();
        match target {
            Some(_) if crate::demo::active() => post_boxed(
                ctl,
                WM_APP_SWITCHED,
                Err::<(), String>(i18n::t("Demo mode: this would now open the session in VS Code.").into()),
            ),
            Some(t) => {
                std::thread::spawn(move || post_boxed(ctl, WM_APP_SWITCHED, vscode::switch_to(&dir, &t)));
            }
            None => match terminal {
                Some(_) if crate::demo::active() => post_boxed(
                    ctl,
                    WM_APP_SWITCHED,
                    Err::<(), String>(i18n::t("Demo mode: this would now switch to the session's terminal.").into()),
                ),
                Some(pid) => {
                    std::thread::spawn(move || post_boxed(ctl, WM_APP_SWITCHED, crate::terminal::switch_to(pid, &cwd)));
                }
                None => {
                    let why = if in_wsl {
                        i18n::t("This session runs in a terminal in WSL, so it can't be switched to from here.")
                    } else {
                        i18n::t("This session runs in a terminal Trayward can't find.")
                    };
                    post_boxed(ctl, WM_APP_SWITCHED, Err::<(), String>(why.into()));
                }
            },
        }
    }

    // ---------- views ----------

    fn widget_model(&self) -> widget::Model {
        let t = now();
        let used = |l: Option<usage::Limit>| l.map(|l| l.used_at(t).round() as u8);
        let (u, _) = self.shown_usage();
        let mut m = widget::Model {
            dark: self.dark,
            // Set per widget.
            hover: false,
            weekly: used(u.weekly),
            five_hour: used(u.five_hour),
            plan: self.plan.clone(),
            stale: self.stale(),
            sessions: self.session_counts(),
            attention: self.attention(),
            gpg_left: self.gpg_left(),
            // Decided by the widget from the options.
            bars: true,
            background: None,
            shine: if self.prefs.slim { None } else { self.plan_shine() },
            countdown: self.countdowns(),
            claude: !(self.codex_shown() && self.prefs.taskbar_shows == settings::Shown::Codex),
            codex: self.codex_widget(),
            copilot: self.copilot_widget(),
            // The slim widget shows neither: left out, so they don't make it draw again.
            pinned: if self.prefs.slim { None } else { self.pinned_view() },
            monitor: if self.prefs.slim { None } else { self.monitor_view() },
            slim: None,
            day: None,
            day_turn: false,
            weekly_pace: self.week_plan().map(|p| p.standing()),
        };
        // Limits take turns: today's limit with the 5-hour one (slim: all of
        // them). The clock picks the turn, so every widget shows the same.
        let secs = i64::from(self.prefs.turn_secs);
        let turn = if secs > 0 { (t / secs) as usize } else { 0 };
        if secs > 0 {
            m.day = self.week_plan().and_then(|p| p.today_day().share()).map(|s| s.round().clamp(0.0, 255.0) as u8);
        }
        if self.prefs.slim {
            // Within the limits there are, so one limit alone never changes.
            m.slim = Some(turn % widget::slim_turns(&m).max(1));
        } else {
            m.day_turn = m.day.is_some() && turn % 2 == 1;
        }
        m
    }

    /// The weekly limit split into days, when its reset is known.
    fn week_plan(&self) -> Option<forecast::Plan> {
        let (u, _) = self.shown_usage();
        let t = now();
        let w = u.weekly?;
        let reset = w.resets_at.filter(|r| *r > t)?;
        Some(forecast::plan_with(&self.history, w.used_at(t), reset, t, self.prefs.daily_budget))
    }

    fn flyout_model(&self) -> flyout::Model {
        let at = now_ms();
        let (u, five_hour_simulated) = self.shown_usage();
        flyout::Model {
            dark: self.dark,
            now: now(),
            weekly: u.weekly,
            five_hour: u.five_hour,
            plan: self.plan.clone(),
            five_hour_simulated,
            weekly_plan: self.week_plan(),
            updated_at: self.fetched_at,
            error: self.error.as_ref().map(ToString::to_string),
            tracking: self.tracking,
            sessions: self.session_rows(at),
            hidden: self.sessions.hidden_count(),
            refresh: self.refresh_state(),
            vscode_exe: self.vscode_exe.clone(),
            pulse: self.pulse(),
            shine: self.plan_shine(),
            codex: self.codex.limits.filter(|_| self.prefs.codex).map(|l| flyout::Codex {
                plan: self.codex.plan.clone(),
                weekly: l.weekly,
                five_hour: l.five_hour,
                at: l.at,
            }),
            copilot: self.copilot_shown().map(|q| {
                let what = match q.kind {
                    copilot::Kind::Premium => i18n::t("Premium requests"),
                    copilot::Kind::Chat => i18n::t("Chat messages"),
                };
                let left = i18n::f("{0} of {1} left", &[&copilot_count(q.remaining), &copilot_count(q.entitlement)]);
                flyout::Copilot { plan: q.plan.clone(), label: format!("{what} · {left}"), limit: q.limit, at: q.at }
            }),
            mine: if self.prefs.forge { self.forge.mine.clone() } else { Vec::new() },
            mailbox: self.prefs.mailbox.then(|| self.mailbox.clone()),
            gpg: if self.prefs.gpg_timer { self.gpg.view(now()) } else { None },
        }
    }

    /// Rows for the flyout: needs you, working, done (including finished
    /// sessions already seen, for an hour), then idle; newest first in each.
    fn session_rows(&self, at: i64) -> Vec<flyout::SessionRow> {
        let group = |s: &Session| match s.status {
            Status::NeedsInput => (0, s.since_ms),
            Status::Working => (1, s.since_ms),
            _ if s.recently_done(at, DONE_STAYS_MS) || !s.watchers(at).is_empty() => {
                (2, s.done_at.unwrap_or(s.since_ms))
            }
            _ => (3, s.since_ms),
        };
        let mut list: Vec<(u8, i64, flyout::SessionRow)> = self
            .sessions
            .iter()
            .filter(|s| self.listed(s))
            .map(|s| {
                let (g, t) = group(s);
                let mut row = session_row(s, at);
                row.change = self.change_line(&s.cwd, &row.title);
                row.pinned = self.pinned.as_deref() == Some(s.id.as_str());
                self.mark_mail(&mut row, &s.cwd);
                (g, t, row)
            })
            .collect();
        let codex = self.codex.sessions.iter().filter(|_| self.prefs.codex).map(|s| (s, "Codex"));
        let copilot = self.copilot.iter().filter(|_| self.prefs.copilot).map(|s| (s, "Copilot"));
        for (s, provider) in codex.chain(copilot) {
            let g = match s.status {
                Status::NeedsInput => 0,
                Status::Working => 1,
                Status::Done => 2,
                Status::Idle => 3,
            };
            let mut row = other_row(s, provider, at);
            row.change = self.change_line(&s.cwd, &row.title);
            self.mark_mail(&mut row, &s.cwd);
            list.push((g, s.since * 1000, row));
        }
        list.sort_by_key(|(g, t, _)| (*g, std::cmp::Reverse(*t)));
        list.into_iter().map(|(_, _, row)| row).collect()
    }

    /// Whether the mailbox is being looked for: until its reader answered, and
    /// a moment at least, so the arrow shows.
    fn mailbox_searching(&self) -> bool {
        self.prefs.mailbox && self.mailbox_search.is_some_and(|(at, answered)| !answered || at.elapsed() < MIN_SPIN)
    }

    /// How the mailbox's folder stands, for the settings.
    fn mailbox_folder(&self) -> settings_window::MailboxFolder {
        use settings_window::MailboxFolder;
        if self.mailbox_searching() {
            let elapsed = self.mailbox_search.map_or(Duration::ZERO, |(at, _)| at.elapsed());
            let angle = if self.prefs.animations { ((elapsed.as_millis() * 360 / 1000) % 360) as u16 } else { 0 };
            return MailboxFolder::Searching(angle);
        }
        match &self.mailbox.root {
            Some(root) => {
                MailboxFolder::Found { path: root.display().to_string(), set: self.prefs.mailbox_dir.is_some() }
            }
            None => MailboxFolder::Missing(self.prefs.mailbox_dir.clone()),
        }
    }

    /// The mailbox's inboxes for the settings: what's unread and handled, and
    /// whether a session runs in the project folder of that name.
    fn inbox_rows(&self) -> Vec<settings_window::InboxRow> {
        let t = now();
        let folder = |cwd: &str| std::path::Path::new(cwd).file_name().map(|n| n.to_string_lossy().to_lowercase());
        let mut running: Vec<String> =
            self.sessions.iter().filter(|s| self.listed(s)).filter_map(|s| folder(&s.cwd)).collect();
        running.extend(self.other_sessions().filter_map(|(s, _)| folder(&s.cwd)));
        self.mailbox
            .inboxes
            .iter()
            .map(|b| settings_window::InboxRow {
                name: b.name.clone(),
                unread: b.unread_count,
                waiting: b.waiting_since().map(|at| timefmt::ago(t - at)),
                done: b.done,
                last_done: b.last_done.map(|at| timefmt::ago(t - at)),
                session: running.contains(&b.name.to_lowercase()),
            })
            .collect()
    }

    /// Puts "✉ 2" in front of a session row's details when the inbox of its
    /// project folder (`cwd`'s name) has unread mail.
    fn mark_mail(&self, row: &mut flyout::SessionRow, cwd: &str) {
        if !self.prefs.mailbox {
            return;
        }
        let project = std::path::Path::new(cwd).file_name().map(|n| n.to_string_lossy().into_owned());
        let unread = project.and_then(|p| self.mailbox.inbox(&p)).map_or(0, |i| i.unread_count);
        if unread > 0 {
            row.details = if row.details.is_empty() {
                format!("✉ {unread}")
            } else {
                format!("✉ {unread} · {}", row.details)
            };
        }
    }

    /// The GPG key's remaining time for the widget's bottom line.
    fn gpg_left(&self) -> Option<widget::GpgLeft> {
        if !self.prefs.gpg_timer {
            return None;
        }
        let view = self.gpg.view(now())?;
        let left = view.left.filter(|_| view.unlocked)?;
        Some(widget::GpgLeft {
            fraction: (left * 100 / view.total.max(1)).clamp(0, 100) as u8,
            ending: left <= GPG_ENDING_SECS,
        })
    }

    /// The usage to show: as reported, plus a simulated 5-hour limit on plans
    /// without one when that's switched on. Also says whether it's simulated.
    fn shown_usage(&self) -> (Usage, bool) {
        let mut u = self.usage.unwrap_or_default();
        let simulate = self.prefs.simulate_five_hour && u.five_hour.is_none() && self.usage.is_some();
        if simulate {
            let t = now();
            let weekly = u.weekly.map(|w| (w.used_at(t), w.resets_at));
            let window = weekly.and_then(|(used, reset)| forecast::five_hour_window(&self.history, used, reset, t));
            u.five_hour = Some(match window {
                Some(w) => usage::Limit {
                    used: (w.used / forecast::FIVE_HOUR_SHARE * 100.0).min(100.0),
                    resets_at: Some(w.ends),
                },
                None => usage::Limit { used: 0.0, resets_at: None },
            });
        }
        (u, simulate)
    }

    fn stats_model(&self) -> stats_window::Model {
        let at = now_ms();
        let sessions = self
            .session_rows(at)
            .into_iter()
            .filter(|r| r.other != Some("Codex"))
            .map(|r| stats_window::Entry { id: r.id, title: r.title, workspace: r.workspace, status: r.status })
            .collect();
        stats_window::Model {
            dark: self.dark,
            // To the minute: the window shows times in minutes, and a model
            // that changes every second would repaint it every second.
            now: now() - now().rem_euclid(60),
            sessions,
            history: self.history.clone(),
            weekly_reset: self.usage.and_then(|u| u.weekly).and_then(|w| w.resets_at),
        }
    }

    fn stats_command(&self, cmd: Option<stats_window::Command>) {
        match cmd {
            Some(stats_window::Command::Load(page)) => self.compute_stats(page),
            Some(stats_window::Command::Open(path)) => stats_window::explore(&path, false),
            Some(stats_window::Command::Reveal(path)) => stats_window::explore(&path, true),
            Some(stats_window::Command::Copy(text)) => win::set_clipboard_text(self.ctl, &text),
            Some(stats_window::Command::Export { name, csv }) => {
                // On a thread: Downloads can be a slow or redirected folder, and
                // a message box is modal.
                std::thread::spawn(move || {
                    let path = win::downloads_folder().join(name);
                    match std::fs::write(&path, csv) {
                        Ok(()) => stats_window::explore(&path, true),
                        Err(e) => {
                            let text = i18n::f("Couldn't save {0}:\n{1}", &[&path.display(), &e]);
                            message(&text, MB_ICONWARNING | MB_OK);
                        }
                    }
                });
            }
            None => {}
        }
    }

    /// Computes the statistics for page `key` in the background, at low
    /// priority so reading transcripts doesn't slow anything else down.
    fn compute_stats(&self, key: stats_window::Page) {
        use crate::stats;
        use stats_window::{Data, Listing, Page};
        let ctl = SendHwnd::new(self.ctl);
        let config_dir = self.config_dir.clone();
        let wsl = self.prefs.wsl;
        std::thread::spawn(move || {
            crate::win::background_thread();
            let mut roots = vec![crate::registry::Source { distro: None, config_dir }];
            if wsl {
                for d in crate::registry::wsl_running().unwrap_or_default() {
                    roots.extend(crate::registry::wsl_sources(&d));
                }
            }
            let t = now();
            let offset = timefmt::offsets();
            let range = |p: stats::Period| (p.start(stats::day_start_at(t, &offset), &offset), t + 1);
            let span = |p: stats::Period| match p {
                stats::Period::Today => i18n::t("Today").to_string(),
                stats::Period::All => i18n::t("All time").to_string(),
                p => i18n::f("Last {0}", &[&i18n::t(p.label())]),
            };
            let archived = || crate::vscode_state::archived_sessions().unwrap_or_default();
            let data = match &key {
                Page::Overview(p) => {
                    let (from, to) = range(*p);
                    let archived = archived();
                    let mut o = stats::overview(&roots, from, to, &offset, &archived);
                    // For comparison: as long a time right before (all time has nothing before).
                    if *p != stats::Period::All {
                        let len = to - from;
                        o.previous = Some(stats::overview(&roots, from - len, from, &offset, &archived).summary);
                    }
                    Data::Overview(Box::new(o))
                }
                Page::Session(id) => stats::session(&roots, id).map_or(Data::Missing, |d| Data::Session(Box::new(d))),
                Page::Calls { session: Some(id), .. } => stats::session(&roots, id).map_or(Data::Missing, |d| {
                    let mut items: Vec<stats::CallRow> =
                        d.stats.calls.iter().map(|c| stats::CallRow { call: c.clone(), session: None }).collect();
                    items.reverse();
                    Data::Calls(Box::new(Listing { title: d.stats.display_title(), items }))
                }),
                Page::Calls { session: None, period, .. } => {
                    let (from, to) = range(*period);
                    Data::Calls(Box::new(Listing { title: span(*period), items: stats::calls(&roots, from, to) }))
                }
                Page::Files { session: Some(id), .. } => stats::session(&roots, id).map_or(Data::Missing, |d| {
                    let items = stats::session_files(&d.stats, d.distro.as_ref());
                    Data::Files(Box::new(Listing { title: d.stats.display_title(), items }))
                }),
                Page::Files { session: None, period } => {
                    let (from, to) = range(*period);
                    Data::Files(Box::new(Listing { title: span(*period), items: stats::files(&roots, from, to) }))
                }
                Page::Sessions { .. } => Data::Sessions(stats::sessions(&roots, &archived())),
                Page::Storage => Data::Storage(stats::storage(&roots)),
            };
            post_boxed(ctl, WM_APP_STATS, (key, data));
        });
    }

    fn settings_model(&self) -> settings_window::Model {
        settings_window::Model {
            dark: self.dark,
            prefs: self.prefs.clone(),
            interval: self.interval,
            bars: self.widget_options.bars,
            solid: self.widget_options.solid,
            taskbars: self.widget_options.taskbars,
            chosen: self.widget_options.chosen.clone(),
            displays: self
                .widgets
                .displays()
                .iter()
                .map(|d| (d.id.clone(), d.name.clone(), d.detail.clone()))
                .collect(),
            more_monitors: crate::taskbars::monitor_count() > self.widgets.displays().len(),
            codex_found: self.providers_found.0,
            copilot_found: self.providers_found.1,
            copilot_error: self.copilot_error.clone(),
            tracking: self.tracking,
            autostart: self.autostart,
            push_status: self.push_status.clone(),
            update: self.update.clone(),
            update_status: self.update_status(),
            mailbox_folder: self.mailbox_folder(),
            mailbox_note: self.mailbox_note.clone(),
            mailbox_inboxes: self.inbox_rows(),
            mqtt_line: self.mqtt_line(),
        }
    }

    /// Whether a session appears (flyout, widget, bubbles): not hidden by the
    /// user, and not archived in the extension unless it needs the user. Work
    /// an archived session does on its own (another session's message, a
    /// background task) doesn't bring it back.
    fn listed(&self, s: &Session) -> bool {
        !self.sessions.is_hidden(&s.id) && !(self.prefs.hide_archived && s.archived && s.status != Status::NeedsInput)
    }

    fn show_flyout(&mut self, activate: bool) {
        let Some((anchor, dpi)) = self.widgets.anchor() else { return };
        self.flyout.show(self.flyout_model(), anchor, dpi, activate);
        // The plan's shine starts with the flyout, not a tick later.
        self.update_animation();
    }

    /// One frame of the animations (30 a second while a session needs you,
    /// a refresh runs or the plan shines): only what animates is drawn again,
    /// the widget and the open flyout. Everything else follows the events
    /// and the once-a-second tick, through [`Self::refresh_views`].
    fn animate(&mut self) {
        let m = self.widget_model();
        self.widgets.redraw(&m, &self.widget_options);
        if self.settings.visible() && self.mailbox_search.is_some() {
            let model = self.settings_model();
            self.settings.set_model(model);
            if !self.mailbox_searching() {
                self.mailbox_search = None;
            }
        }
        if self.flyout.visible() {
            self.show_flyout(false);
        }
        self.update_animation();
    }

    fn refresh_views(&mut self) {
        let m = self.widget_model();
        self.widgets.update(&m, &self.widget_options);
        self.update_forge_folders();
        if self.tip.visible() {
            if self.widgets.hovered().is_some() {
                self.show_tip();
            } else {
                // Its widget is gone (Explorer restarted, a display was removed).
                self.hide_tip();
            }
        }
        // UI Automation is needed for the bars' room and for clocks on other taskbars.
        let uia = self.widget_options.bars == settings::Bars::Auto || self.widgets.beyond_main();
        self.uia_active.store(uia, Ordering::Relaxed);
        if self.flyout.visible() {
            self.show_flyout(false);
        }
        if self.stats.visible() {
            let model = self.stats_model();
            self.stats.set_model(model);
        }
        if self.settings.visible() {
            let model = self.settings_model();
            self.settings.set_model(model);
        }
        self.publish_mqtt_state();
        self.update_animation();
    }

    /// Saves the state soon, on the writer's thread.
    fn save(&self) {
        self.writer.save(store::Saved {
            usage: self.usage,
            fetched_at: self.fetched_at,
            plan: self.plan.clone(),
            history: self.history.clone(),
            sessions: self.sessions.clone(),
            warned: self.warned.clone(),
            snoozed_until: self.snoozed_until,
            pinned: self.pinned.clone(),
            summary_week: self.summary_week,
            update_checked: self.update_checked,
            update_announced: self.update_announced.clone(),
            gpg_unlocked: self.gpg.saved,
        });
    }
}

/// A session's state and a detail, e.g. "Done · 3m" or "Working · 2 agents".
fn status_line(status: Status, detail: &str) -> String {
    match status {
        Status::Working => i18n::f("Working · {0}", &[&detail]),
        Status::NeedsInput => i18n::f("Needs you · {0}", &[&detail]),
        Status::Done => i18n::f("Done · {0}", &[&detail]),
        Status::Idle => detail.to_string(),
    }
}

/// How many requests: whole, or with one decimal when there's a fraction.
fn copilot_count(n: f64) -> String {
    if (n - n.round()).abs() < 0.05 { format!("{n:.0}") } else { format!("{n:.1}") }
}

/// A session of another provider (`provider`: Codex, Copilot) in the flyout.
fn other_row(s: &codex::Session, provider: &'static str, at_ms: i64) -> flyout::SessionRow {
    let status_text = status_line(s.status, &timefmt::ago(at_ms / 1000 - s.since));
    let mut details = Vec::new();
    if let Some(model) = &s.model {
        details.push(model.clone());
    }
    if s.context_tokens > 0 {
        details.push(format!("{} ctx", transcripts::tokens(s.context_tokens)));
    }
    flyout::SessionRow {
        id: s.id.clone(),
        workspace: s.project().to_string(),
        title: if s.title.is_empty() { s.cwd.clone() } else { s.title.clone() },
        status: s.status,
        seen: false,
        status_text,
        details: details.join(" · "),
        vscode: false,
        archived: false,
        other: Some(provider),
        change: None,
        pinned: false,
        cache_left: None,
        urgent: false,
        watching: None,
        agents: 0,
        context: None,
    }
}

fn session_row(s: &Session, at_ms: i64) -> flyout::SessionRow {
    // Background work still running, shown with an eye: done with the turn,
    // but not finished.
    let watchers = s.watchers(at_ms);
    let watching = (s.status != Status::Working && !watchers.is_empty())
        .then(|| watchers.iter().map(|w| watch_label(w)).collect::<Vec<_>>().join(" · "));
    let agents = s.running_agents(at_ms);
    // A finished session that was already seen still shows as done for a while,
    // and as long as something it started runs in the background.
    let seen_done =
        s.status == Status::Idle && (s.recently_done(at_ms, DONE_STAYS_MS) || watching.is_some() || agents > 0);
    let status = if seen_done { Status::Done } else { s.status };
    let since = if status == Status::Done { s.done_at.unwrap_or(s.since_ms) } else { s.since_ms };
    let status_text = status_line(status, &timefmt::ago((at_ms - since) / 1000));
    let mut details = Vec::new();
    if let Some(model) = &s.stats.model {
        details.push(transcripts::model_name(model));
    }
    if s.stats.context_tokens > 0 {
        details.push(format!("{} ctx", transcripts::tokens(s.stats.context_tokens)));
    }
    if s.stats.output_tokens > 0 {
        details.push(format!("{} out", transcripts::tokens(s.stats.output_tokens)));
    }
    let mut workspace = s.display_name();
    if let Some(d) = &s.distro {
        workspace.push_str(&format!(" · WSL {d}"));
    }
    flyout::SessionRow {
        id: s.id.clone(),
        workspace,
        title: s.description().map_or_else(|| s.cwd.clone(), str::to_string),
        status,
        seen: seen_done,
        status_text,
        details: details.join(" · "),
        vscode: s.in_vscode(),
        archived: s.archived,
        other: None,
        change: None,
        pinned: false,
        cache_left: s.stats.cache_expires_at.map(|e| e - at_ms / 1000),
        urgent: s.status == Status::NeedsInput && at_ms - s.since_ms >= URGENT_AFTER_MS,
        watching,
        agents,
        context: s.stats.model.as_deref().and_then(|m| transcripts::context_fill(m, s.stats.context_tokens)),
    }
}

/// What a watcher is waiting for, for the flyout.
fn watch_label(w: &transcripts::Watcher) -> String {
    use transcripts::WatchKind;
    match (w.kind, w.label.is_empty()) {
        (WatchKind::Wakeup, _) => match w.until {
            Some(at) => i18n::f("wakes up {0}", &[&timefmt::local_time(at, false)]),
            None => i18n::t("wake-up").into(),
        },
        (_, false) => w.label.clone(),
        (WatchKind::Command, true) => i18n::t("command").into(),
        (WatchKind::Monitor, true) => i18n::t("monitor").into(),
        (WatchKind::Agent, true) => i18n::t("agent").into(),
    }
}

// ---------- outside event handling (modal UI) ----------

pub fn message(text: &str, style: MESSAGEBOX_STYLE) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    // SAFETY: plain modal message box.
    unsafe { MessageBoxW(None, &HSTRING::from(text), APP_NAME, style | MB_SETFOREGROUND) }
}

/// Whether one title already says the other (ignoring case, spaces and
/// punctuation): then the pill doesn't repeat the session's title.
fn same_topic(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}
/// How many characters of a merge or pull request's title its pill shows.
const PILL_TITLE: usize = 24;

/// A title cut to about `max` characters, at a word if one ends late enough,
/// with "…" when cut. None for an empty title.
fn short_title(title: &str, max: usize) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    if title.chars().count() <= max {
        return Some(title.to_string());
    }
    let cut: String = title.chars().take(max - 1).collect();
    // Keep whole words when that loses little.
    let cut = match cut.rfind(' ') {
        Some(i) if cut[..i].chars().count() >= max * 2 / 3 => &cut[..i],
        _ => cut.as_str(),
    };
    Some(format!("{}…", cut.trim_end()))
}

/// Opens a web page in the default browser (https only: links come from
/// GitLab's and GitHub's answers).
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let url = HSTRING::from(url);
    // On a thread: starting the browser can take seconds, and can show a dialog of its own.
    std::thread::spawn(move || {
        use windows::Win32::System::Com::CoUninitialize;
        use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
        // SAFETY: COM for this thread, as ShellExecute wants, released after;
        // the info struct and the URL outlive the call. NOASYNC: the thread
        // ends right after, so the shell must be done when it returns.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOASYNC,
                lpVerb: w!("open"),
                lpFile: PCWSTR(url.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = ShellExecuteExW(&mut info);
            if com {
                CoUninitialize();
            }
        }
    });
}

/// Turns session tracking on or off after asking. Runs outside event handling.
pub fn set_tracking(on: bool) {
    let dir = usage::config_dir();
    let path = tracking::settings_path(&dir);
    if on {
        let text = format!(
            "Session tracking adds hooks to Claude Code's settings file:\n{}\n\n\
             They report when a session starts working, needs you or is done, \
             and never change what Claude does. Nothing else in the file is touched, \
             and turning tracking off removes them again.\n\n\
             New sessions are tracked from their first prompt. Sessions that are already \
             open may need to be restarted first.\n\nTurn on session tracking?",
            path.display()
        );
        if message(&text, MB_YESNO | MB_ICONINFORMATION) != IDYES {
            return;
        }
    }
    // The settings file is changed on a thread (it can be on a slow drive),
    // and the app hears how the hooks are afterwards.
    let Some(ctl) = with(|a| SendHwnd::new(a.ctl)) else { return };
    std::thread::spawn(move || {
        if let Err(e) = tracking::set(&dir, on) {
            message(&i18n::f("Couldn't change session tracking:\n\n{0}", &[&e]), MB_OK | MB_ICONWARNING);
        }
        post_boxed(ctl, WM_APP_TRACKING, tracking::current(&dir));
    });
}

// ---------- startup and the message loop ----------

pub fn run() {
    // SAFETY: process start-up: single-instance mutex, COM, window classes, windows.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (class, proc_, style) in [
            (control_class(), ctl_proc as WndProc, Default::default()),
            (widget::CLASS, widget_proc as WndProc, Default::default()),
            (flyout::CLASS, flyout_proc as WndProc, CS_DROPSHADOW),
            (bubble::CLASS, bubble_proc as WndProc, CS_DROPSHADOW),
            (tip::CLASS, tip_proc as WndProc, CS_DROPSHADOW),
            (settings_window::CLASS, settings_proc as WndProc, Default::default()),
            (stats_window::CLASS, stats_proc as WndProc, Default::default()),
        ] {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: instance.into(),
                lpszClassName: class,
                hCursor: arrow,
                hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
                style,
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        let Ok(ctl) =
            CreateWindowExW(WS_EX_TOOLWINDOW, control_class(), APP_NAME, WS_POPUP, 0, 0, 0, 0, None, None, None, None)
        else {
            return;
        };

        // Settings, autostart and state from when the app was called Claude Task Bar.
        if !crate::demo::active() {
            settings::migrate_legacy();
            settings::repair_autostart();
        }
        store::migrate_legacy();
        let config_dir = usage::config_dir();
        // Our hooks point at this exe; if it moved, point them at the new place.
        if tracking::current(&config_dir) == tracking::State::Outdated {
            let _ = tracking::set(&config_dir, true);
        }
        let saved = store::load();
        let mut flyout = Flyout::new();
        flyout.create();
        let mut bubble = Bubble::new();
        bubble.create();
        let mut tip = tip::Tip::new();
        tip.create();
        let widget_options = widget::Options {
            bars: settings::bars(),
            solid: settings::solid_background(),
            taskbars: settings::taskbars(),
            chosen: settings::chosen_displays(),
        };
        let uia_active = Arc::new(AtomicBool::new(
            widget_options.bars == settings::Bars::Auto || widget_options.taskbars != settings::Taskbars::Main,
        ));
        let prefs = settings::prefs();
        crate::i18n::set(prefs.language);
        let codex_on = Arc::new(AtomicBool::new(prefs.codex));
        let copilot_on = Arc::new(AtomicBool::new(prefs.copilot));
        let mailbox_on = Arc::new(AtomicBool::new(prefs.mailbox));
        let mailbox_dir = Arc::new(Mutex::new(prefs.mailbox_dir.clone()));
        let mailbox_poke = Arc::new(AtomicBool::new(false));
        copilot::set_on(prefs.copilot);
        let copilot_quota_on = Arc::new(AtomicBool::new(prefs.copilot && prefs.copilot_limit));
        let copilot_poke = Arc::new(AtomicBool::new(false));
        let forge_on = Arc::new(AtomicBool::new(prefs.forge));
        let forge_poke = Arc::new(AtomicBool::new(false));
        let forge_folders: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let poll_ms = prefs.session_poll_secs * 1000;
        let mqtt_ctl = SendHwnd::new(ctl);
        let monitor_ctl = SendHwnd::new(ctl);
        let app = App {
            ctl,
            widgets: Widgets::new(),
            flyout,
            dark: theme::system_dark(),
            usage: saved.usage,
            fetched_at: saved.fetched_at,
            error: None,
            fetching: false,
            rate_limited: 0,
            interval: settings::interval(),
            plan: saved.plan,
            history: saved.history,
            warned: saved.warned,
            snoozed_until: saved.snoozed_until,
            pinned: saved.pinned,
            summary_week: saved.summary_week,
            week_bubble: None,
            push_status: None,
            update: None,
            kept_at: None,
            mqtt: crate::mqtt::spawn(move |s| post_boxed(mqtt_ctl, WM_APP_MQTT, s)),
            mqtt_broker: None,
            mqtt_status: None,
            mqtt_note: None,
            mqtt_sent: String::new(),
            keeping: Arc::new(AtomicBool::new(false)),
            update_checked: saved.update_checked,
            update_announced: saved.update_announced,
            update_checking: false,
            update_error: None,
            codex: codex::Snapshot::default(),
            codex_on: codex_on.clone(),
            copilot: Vec::new(),
            copilot_on: copilot_on.clone(),
            copilot_quota: None,
            copilot_error: None,
            copilot_quota_on: copilot_quota_on.clone(),
            copilot_poke: copilot_poke.clone(),
            providers_found: (false, false),
            mailbox: mailbox::Snapshot::default(),
            mailbox_on: mailbox_on.clone(),
            mailbox_dir: mailbox_dir.clone(),
            mailbox_note: None,
            mailbox_poke: mailbox_poke.clone(),
            mailbox_search: None,
            autostart: false,
            writer: store::Writer::spawn(),
            fetch_not_before: None,
            forge: forge::Snapshot::default(),
            forge_on: forge_on.clone(),
            forge_poke: forge_poke.clone(),
            forge_folders: forge_folders.clone(),
            plan_checked: None,
            sessions: {
                let mut s = saved.sessions;
                s.migrate(now_ms());
                s
            },
            tracking: tracking::current(&config_dir),
            collector: collector::spawn(&config_dir, SendHwnd::new(ctl), WM_APP_SNAPSHOT),
            config_dir,
            widget_options,
            vscode_exe: None,
            refresh_started: None,
            awaiting_snapshot: false,
            animating: false,
            focus: None,
            vscode_pids: HashMap::new(),
            monitor_on: {
                let on = crate::sysmon::spawn(move |s| post_boxed(monitor_ctl, WM_APP_SYSMON, s));
                on.store(prefs.monitor.bits(), Ordering::Relaxed);
                on
            },
            monitor: VecDeque::new(),
            started: Instant::now(),
            gpg: GpgTimer { saved: saved.gpg_unlocked, ..GpgTimer::default() },
            prefs,
            bubble,
            tip,
            settings: SettingsWindow::new(),
            stats: StatsWindow::new(),
            stats_closed_at: None,
            uia_active: uia_active.clone(),
            last_sound: HashMap::new(),
            cache_warned: HashMap::new(),
        };
        uia::spawn(SendHwnd::new(ctl), WM_APP_BUTTONS, uia_active);
        codex::spawn(SendHwnd::new(ctl), WM_APP_CODEX, codex_on);
        copilot::spawn(SendHwnd::new(ctl), WM_APP_COPILOT, copilot_on);
        // The sessions' folders (as for merge requests) say where the mailbox is.
        mailbox::spawn(
            SendHwnd::new(ctl),
            WM_APP_MAILBOX,
            mailbox_on,
            mailbox_poke,
            mailbox_dir,
            forge_folders.clone(),
        );
        copilot::spawn_quota(SendHwnd::new(ctl), WM_APP_COPILOT_QUOTA, copilot_quota_on, copilot_poke);
        forge::spawn(SendHwnd::new(ctl), WM_APP_FORGE, forge_on, forge_poke, forge_folders);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        APP.with(|cell| *cell.borrow_mut() = Some(app));

        SetTimer(Some(ctl), T_LAYOUT, 1000, None);
        SetTimer(Some(ctl), T_HOUSEKEEPING, poll_ms, None);
        dispatch(Event::Housekeeping);
        dispatch(Event::FetchDue);
        // Connects to the MQTT broker, if one is set up.
        dispatch(Event::MqttNote(None));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // However the loop ended (Exit, WM_CLOSE): what's waiting reaches the disk.
        if let Some(app) = APP.with(|cell| cell.borrow_mut().take()) {
            app.save();
            app.writer.flush();
        }
    }
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// Saves the state now and waits until it's on disk (Windows is ending the session).
fn flush_state() {
    with(|a| {
        a.save();
        a.writer.flush();
    });
}

fn mouse_pos(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn track_leave(hwnd: HWND) {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    // SAFETY: valid struct for our own window.
    let _ = unsafe { TrackMouseEvent(&mut tme) };
}

unsafe extern "system" fn ctl_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wp.0 {
                T_LAYOUT => dispatch(Event::LayoutTick),
                T_HOUSEKEEPING => dispatch(Event::Housekeeping),
                T_ANIMATION => dispatch(Event::AnimationTick),
                T_BUBBLE => {
                    // SAFETY: our own one-shot timer.
                    let _ = unsafe { KillTimer(Some(hwnd), T_BUBBLE) };
                    dispatch(Event::BubbleTimeout);
                }
                T_TIP => {
                    // SAFETY: our own one-shot timer.
                    let _ = unsafe { KillTimer(Some(hwnd), T_TIP) };
                    dispatch(Event::TipDue);
                }
                T_FETCH => {
                    // SAFETY: our own one-shot timer.
                    let _ = unsafe { KillTimer(Some(hwnd), T_FETCH) };
                    dispatch(Event::FetchDue);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_COPYDATA => {
            // SAFETY: for WM_COPYDATA, lParam points to a COPYDATASTRUCT valid during this call.
            let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
            if cds.dwData != hook::COPYDATA_TAG || cds.cbData as usize > hook::MAX_MESSAGE || cds.lpData.is_null() {
                return LRESULT(0);
            }
            // SAFETY: the sender guarantees cbData bytes at lpData for the duration of the call.
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
            match serde_json::from_slice::<HookMessage>(bytes) {
                Ok(m) => {
                    dispatch(Event::Hook(Box::new(m)));
                    LRESULT(1)
                }
                Err(_) => LRESULT(0),
            }
        }
        WM_APP_FETCHED => {
            // SAFETY: posted by start_fetch with exactly this type.
            dispatch(Event::Fetched(Box::new(unsafe { take_boxed::<Fetched>(lp) })));
            LRESULT(0)
        }
        WM_APP_SNAPSHOT => {
            // SAFETY: posted by the collector with exactly this type.
            dispatch(Event::Snapshot(Box::new(unsafe { take_boxed::<Snapshot>(lp) })));
            LRESULT(0)
        }
        WM_APP_STATS => {
            // SAFETY: posted by compute_stats with exactly this type.
            let d = unsafe { take_boxed::<(stats_window::Page, stats_window::Data)>(lp) };
            dispatch(Event::StatsData(Box::new(d)));
            LRESULT(0)
        }
        WM_APP_MQTT => {
            // SAFETY: posted by the MQTT thread with exactly this type.
            dispatch(Event::MqttStatus(Box::new(unsafe { take_boxed::<crate::mqtt::Status>(lp) })));
            LRESULT(0)
        }
        WM_APP_UPDATE => {
            // SAFETY: posted by check_update with exactly this type.
            let r = unsafe { take_boxed::<Result<Option<crate::update::Release>, String>>(lp) };
            dispatch(Event::Update(Box::new(r)));
            LRESULT(0)
        }
        WM_APP_SYSMON => {
            // SAFETY: posted by the system monitor's sampler with exactly this type.
            dispatch(Event::Monitor(unsafe { take_boxed::<crate::sysmon::Sample>(lp) }));
            LRESULT(0)
        }
        WM_APP_PUSH => {
            // SAFETY: posted by send_push with exactly this type.
            dispatch(Event::Pushed(Box::new(unsafe { take_boxed::<(bool, Result<(), String>)>(lp) })));
            LRESULT(0)
        }
        WM_APP_WEEK => {
            // SAFETY: posted by summarize_week with exactly this type.
            dispatch(Event::Week(Box::new(unsafe { take_boxed::<crate::alerts::Week>(lp) })));
            LRESULT(0)
        }
        WM_APP_FORGE => {
            // SAFETY: posted by the merge request reader with exactly this type.
            dispatch(Event::Forge(Box::new(unsafe { take_boxed::<forge::Snapshot>(lp) })));
            LRESULT(0)
        }
        WM_APP_MAILBOX => {
            // SAFETY: posted by the mailbox reader with exactly this type.
            dispatch(Event::Mailbox(Box::new(unsafe { take_boxed::<mailbox::Snapshot>(lp) })));
            LRESULT(0)
        }
        WM_APP_COPILOT => {
            // SAFETY: posted by Copilot's session reader with exactly this type.
            dispatch(Event::Copilot(unsafe { take_boxed::<Vec<codex::Session>>(lp) }));
            LRESULT(0)
        }
        WM_APP_COPILOT_QUOTA => {
            // SAFETY: posted by Copilot's limit reader with exactly this type.
            dispatch(Event::CopilotQuota(unsafe { take_boxed::<copilot::QuotaResult>(lp) }));
            LRESULT(0)
        }
        WM_APP_CODEX => {
            // SAFETY: posted by the Codex reader with exactly this type.
            dispatch(Event::Codex(Box::new(unsafe { take_boxed::<codex::Snapshot>(lp) })));
            LRESULT(0)
        }
        WM_APP_BUTTONS => {
            // SAFETY: posted by the UI Automation watcher with exactly this type.
            dispatch(Event::Edges(unsafe { take_boxed::<Vec<uia::Edges>>(lp) }));
            LRESULT(0)
        }
        WM_APP_SWITCHED => {
            // SAFETY: posted by open_session with exactly this type.
            if let Err(e) = unsafe { take_boxed::<Result<(), String>>(lp) } {
                message(&i18n::f("Couldn't switch to the session.\n\n{0}", &[&e]), MB_OK | MB_ICONINFORMATION);
            }
            LRESULT(0)
        }
        WM_APP_ENABLE_TRACKING => {
            set_tracking(true);
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            dispatch(Event::ThemeChanged);
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if wp.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                dispatch(Event::Resumed);
            }
            LRESULT(1)
        }
        // Windows shuts down, restarts or logs off: the state goes to disk now,
        // the process may end without the message loop finishing.
        WM_QUERYENDSESSION => {
            flush_state();
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wp.0 != 0 {
                flush_state();
            }
            LRESULT(0)
        }
        // Asked to close (`npm run stop` posts WM_CLOSE here): as Exit in the menu.
        // taskkill without /F doesn't reach this hidden window.
        WM_CLOSE => {
            dispatch(Event::Exit);
            LRESULT(0)
        }
        WM_APP_PROVIDERS => {
            // SAFETY: posted by OpenSettings' look-up with exactly this type.
            let (codex, copilot) = unsafe { take_boxed::<(bool, bool)>(lp) };
            dispatch(Event::ProvidersFound(codex, copilot));
            LRESULT(0)
        }
        WM_APP_TRACKING => {
            // SAFETY: posted by set_tracking with exactly this type.
            dispatch(Event::TrackingChanged(unsafe { take_boxed::<tracking::State>(lp) }));
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends our message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
            if taskbar_created != 0 && msg == taskbar_created {
                dispatch(Event::TaskbarCreated);
                return LRESULT(0);
            }
            // SAFETY: default handling.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

unsafe extern "system" fn widget_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            dispatch(Event::WidgetHover(hwnd, true));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            dispatch(Event::WidgetHover(hwnd, false));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            dispatch(Event::WidgetClick(hwnd));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            dispatch(Event::WidgetMenu(hwnd));
            dispatch(Event::FlyoutHide);
            if let Some((ctl, prefs, snoozed, update)) =
                with(|a| (a.ctl, a.prefs.clone(), a.snoozed_until.filter(|u| *u > now()), a.update.clone()))
            {
                menu::show(ctl, &prefs, snoozed, update.as_ref());
            }
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn flyout_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Alt+F4: hidden like any other way of closing it, never destroyed (it's made once).
        WM_CLOSE => {
            dispatch(Event::FlyoutHide);
            LRESULT(0)
        }
        WM_PAINT => {
            if with(|a| a.flyout.paint(hwnd)).is_none() {
                // Busy: validate now, repaint once the current event is handled.
                // SAFETY: our own window.
                let _ = unsafe { windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None) };
                dispatch(Event::FlyoutPaint);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            dispatch(Event::FlyoutMouse(Some(mouse_pos(lp))));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            dispatch(Event::FlyoutMouse(None));
            LRESULT(0)
        }
        WM_SETCURSOR if with(|a| a.flyout.hovering()) == Some(true) => {
            // SAFETY: system cursor.
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_pos(lp);
            dispatch(Event::FlyoutClick(x, y));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            dispatch(Event::FlyoutWheel((wp.0 >> 16) as u16 as i16));
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == usize::from(VK_ESCAPE.0) => {
            dispatch(Event::FlyoutHide);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if u32::from(wp.0 as u16) == WA_INACTIVE {
                // Activation changes arrive in the middle of other calls; hide afterwards.
                // SAFETY: posting to our own window.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_HIDE_FLYOUT, WPARAM(0), LPARAM(0)) };
            }
            LRESULT(0)
        }
        WM_APP_HIDE_FLYOUT => {
            dispatch(Event::FlyoutHide);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn tip_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with(|a| a.tip.paint(hwnd)).is_none() {
                // SAFETY: our own window; it is repainted when shown again.
                let _ = unsafe { windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None) };
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn bubble_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with(|a| a.bubble.paint(hwnd)).is_none() {
                // SAFETY: our own window; it is repainted when shown again.
                let _ = unsafe { windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None) };
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            dispatch(Event::BubbleMouse);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // SAFETY: system cursor.
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            dispatch(Event::BubbleClick);
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn stats_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with(|a| a.stats.paint(hwnd)).is_none() {
                // SAFETY: our own window.
                let _ = unsafe { windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None) };
                dispatch(Event::StatsPaint);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            dispatch(Event::StatsMouse(Some(mouse_pos(lp))));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            dispatch(Event::StatsMouse(None));
            LRESULT(0)
        }
        WM_SETCURSOR if with(|a| a.stats.hovering()) == Some(true) && (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            // SAFETY: system cursor.
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_pos(lp);
            dispatch(Event::StatsClick(x, y));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            dispatch(Event::StatsWheel((wp.0 >> 16) as u16 as i16));
            LRESULT(0)
        }
        WM_TIMER if wp.0 == stats_window::SCROLL_TIMER => {
            dispatch(Event::StatsTick);
            LRESULT(0)
        }
        // The mouse's back button.
        WM_XBUTTONUP if (wp.0 >> 16) & 0xFFFF == 1 => {
            dispatch(Event::StatsBack);
            LRESULT(1)
        }
        WM_KEYDOWN if wp.0 == usize::from(VK_BACK.0) => {
            dispatch(Event::StatsBackspace);
            LRESULT(0)
        }
        // Ctrl+V pastes into the search.
        WM_KEYDOWN if wp.0 == usize::from(b'V') && win::ctrl_down() => {
            if let Some(text) = win::clipboard_text() {
                dispatch(Event::StatsType(text));
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(ch) = char::from_u32(wp.0 as u32).filter(|c| !c.is_control()) {
                dispatch(Event::StatsType(ch.to_string()));
            }
            LRESULT(0)
        }
        WM_KEYDOWN if [VK_PRIOR, VK_NEXT, VK_HOME, VK_END, VK_UP, VK_DOWN].iter().any(|k| wp.0 == usize::from(k.0)) => {
            let page = stats_window::StatsWindow::page_height();
            let dy = match VIRTUAL_KEY(wp.0 as u16) {
                VK_PRIOR => -page,
                VK_NEXT => page,
                VK_HOME => -1e6,
                VK_END => 1e6,
                VK_UP => -40.0,
                _ => 40.0,
            };
            dispatch(Event::StatsScroll(dy));
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == usize::from(VK_ESCAPE.0) => {
            dispatch(Event::StatsEscape);
            LRESULT(0)
        }
        WM_CLOSE => {
            dispatch(Event::StatsClose);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // SAFETY: for WM_DPICHANGED, lParam points to the suggested window rectangle.
            let r = unsafe { &*(lp.0 as *const windows::Win32::Foundation::RECT) };
            // SAFETY: our own window.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            dispatch(Event::StatsPaint);
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with(|a| a.settings.paint(hwnd)).is_none() {
                // Busy: validate now, repaint once the current event is handled.
                // SAFETY: our own window.
                let _ = unsafe { windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None) };
                dispatch(Event::SettingsPaint);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            track_leave(hwnd);
            dispatch(Event::SettingsMouse(Some(mouse_pos(lp))));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            dispatch(Event::SettingsMouse(None));
            LRESULT(0)
        }
        WM_SETCURSOR if with(|a| a.settings.hovering()) == Some(true) && (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            // SAFETY: system cursor.
            unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_pos(lp);
            if let Some(Some(action)) = with(|a| a.settings.hit(x, y)) {
                settings_click(hwnd, action);
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == usize::from(VK_ESCAPE.0) => {
            dispatch(Event::SettingsClose);
            LRESULT(0)
        }
        WM_CLOSE => {
            // Hide instead of destroying; the window is reused.
            dispatch(Event::SettingsClose);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // SAFETY: for WM_DPICHANGED, lParam points to the suggested window rectangle.
            let r = unsafe { &*(lp.0 as *const windows::Win32::Foundation::RECT) };
            // SAFETY: our own window.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            dispatch(Event::SettingsPaint);
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Carries out a click in the settings window. Runs outside event handling,
/// because some settings ask first or open a file picker.
fn settings_click(hwnd: HWND, action: settings_window::Action) {
    use settings::SoundChoice;
    use settings_window::Action;
    let Some(Some(m)) = with(|a| a.settings.model().cloned()) else { return };
    let mut p = m.prefs.clone();
    let set = |p: settings::Prefs| dispatch(Event::SetPrefs(Box::new(p)));
    let sound_of = |p: &settings::Prefs, kind: sound::Kind| match kind {
        sound::Kind::NeedsYou => p.needs_sound.clone(),
        sound::Kind::Done => p.done_sound.clone(),
    };
    let set_sound = |p: &mut settings::Prefs, kind: sound::Kind, choice: SoundChoice| match kind {
        sound::Kind::NeedsYou => p.needs_sound = choice,
        sound::Kind::Done => p.done_sound = choice,
    };
    match action {
        Action::Page(i) => dispatch(Event::SettingsPage(i)),
        Action::Autostart => {
            if let Err(e) = settings::set_autostart(!m.autostart) {
                message(&i18n::f("Couldn't change \"Start with Windows\":\n\n{0}", &[&e]), MB_OK | MB_ICONWARNING);
            }
            dispatch(Event::AutostartChanged(settings::autostart()));
        }
        Action::Interval(secs) => dispatch(Event::SetInterval(secs)),
        Action::Bars(b) => dispatch(Event::SetBars(b)),
        Action::Solid => dispatch(Event::SetSolidBackground(!m.solid)),
        Action::Taskbars(t) => dispatch(Event::SetTaskbars(t)),
        Action::Display(id) => dispatch(Event::ToggleDisplay(id)),
        Action::Hooks => set_tracking(m.tracking != tracking::State::On),
        Action::Slim(slim) => {
            p.slim = slim;
            set(p);
        }
        Action::TurnSecs(secs) => {
            p.turn_secs = secs;
            set(p);
        }
        Action::DailyBudget(budget) => {
            p.daily_budget = budget;
            set(p);
        }
        Action::Animations => {
            p.animations = !p.animations;
            set(p);
        }
        Action::Transcripts => {
            p.transcripts = !p.transcripts;
            set(p);
        }
        Action::Wsl => {
            p.wsl = !p.wsl;
            set(p);
        }
        Action::HideArchived => {
            p.hide_archived = !p.hide_archived;
            set(p);
        }
        Action::MonitorOn => {
            let m = &mut p.monitor;
            m.on = !m.on;
            // Switched on with nothing chosen: CPU and memory to start with.
            if m.on && m.chosen() == 0 {
                m.cpu = true;
                m.ram = true;
            }
            set(p);
        }
        Action::Monitor(bit) => {
            use crate::sysmon::{CPU, DISK, GPU, NET, RAM};
            let m = &mut p.monitor;
            match bit {
                CPU => m.cpu = !m.cpu,
                RAM => m.ram = !m.ram,
                GPU => m.gpu = !m.gpu,
                NET => m.net = !m.net,
                DISK => m.disk = !m.disk,
                _ => {}
            }
            set(p);
        }
        Action::MonitorShow(bit, way) => {
            use crate::sysmon::{DISK, GPU, RAM};
            let m = &mut p.monitor;
            let (on, other) = match bit {
                RAM => (&mut m.ram, &mut m.ram_gb),
                GPU => (&mut m.gpu, &mut m.gpu_memory),
                DISK => (&mut m.disk, &mut m.disk_rates),
                _ => return,
            };
            *on = way.is_some();
            if let Some(w) = way {
                *other = w;
            }
            set(p);
        }
        Action::MonitorGraphs => {
            p.monitor.graphs = !p.monitor.graphs;
            set(p);
        }
        Action::GpgTimer => {
            p.gpg_timer = !p.gpg_timer;
            set(p);
        }
        Action::SimulateFiveHour => {
            p.simulate_five_hour = !p.simulate_five_hour;
            set(p);
        }
        Action::Bubbles => {
            p.bubbles = !p.bubbles;
            set(p);
        }
        Action::Poll(secs) => {
            p.session_poll_secs = secs;
            set(p);
        }
        Action::WarnWeekly(v) => {
            p.warn_weekly = v;
            set(p);
        }
        Action::WarnFiveHour(v) => {
            p.warn_five_hour = v;
            set(p);
        }
        Action::WarnPace => {
            p.warn_pace = !p.warn_pace;
            set(p);
        }
        Action::CountdownAt(v) => {
            p.countdown_at = v;
            set(p);
        }
        Action::NotifyFree => {
            p.notify_free = !p.notify_free;
            set(p);
        }
        Action::Codex => {
            p.codex = !p.codex;
            set(p);
        }
        Action::Copilot(sessions, limit) => {
            p.copilot = sessions;
            p.copilot_limit = limit;
            set(p);
        }
        Action::Forge => {
            p.forge = !p.forge;
            set(p);
        }
        Action::ForgeBubbles => {
            p.forge_bubbles = !p.forge_bubbles;
            set(p);
        }
        Action::WeekSummary => {
            p.week_summary = !p.week_summary;
            set(p);
        }
        Action::Away => {
            p.away = !p.away;
            set(p);
        }
        Action::AwayAfter(m) => {
            p.away_minutes = m;
            set(p);
        }
        Action::AwayDone => {
            p.away_done = !p.away_done;
            set(p);
        }
        Action::PasteUrl => {
            let text = win::clipboard_text().unwrap_or_default();
            if crate::push::parse(&text).is_some() {
                p.push_url = Some(text.trim().to_string());
                set(p);
                dispatch(Event::PushStatus(None));
            } else {
                dispatch(Event::PushStatus(Some(i18n::t("No https:// address on the clipboard").into())));
            }
        }
        Action::TestPush => dispatch(Event::TestPush),
        Action::Mqtt => {
            p.mqtt = !p.mqtt;
            set(p);
        }
        Action::Mailbox => {
            p.mailbox = !p.mailbox;
            set(p);
        }
        Action::PasteMailbox => {
            // A path as Explorer copies it ("Copy as path" puts quotes around it).
            let text = win::clipboard_text().unwrap_or_default();
            let path = text.trim().trim_matches('"').trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).is_dir() {
                p.mailbox_dir = Some(path);
                set(p);
                dispatch(Event::MailboxNote(None));
            } else {
                dispatch(Event::MailboxNote(Some(i18n::t("No folder's path on the clipboard").into())));
            }
        }
        Action::FindMailbox => {
            p.mailbox_dir = None;
            set(p);
            dispatch(Event::MailboxNote(None));
        }
        Action::PasteBroker => match crate::mqtt::parse(&win::clipboard_text().unwrap_or_default()) {
            Ok(b) => {
                settings::set_mqtt_password(b.password.as_deref());
                p.mqtt_broker = Some(b.shown());
                set(p);
                dispatch(Event::MqttNote(None));
            }
            Err(e) => dispatch(Event::MqttNote(Some(e.text().into()))),
        },
        Action::TestMqtt => dispatch(Event::MqttTest),
        Action::UpdateCheck => {
            p.update_check = !p.update_check;
            set(p);
        }
        Action::CheckNow => dispatch(Event::CheckUpdate),
        Action::KeepStats => {
            p.keep_stats = !p.keep_stats;
            // Switched off: what was kept goes too.
            if !p.keep_stats {
                crate::keep::set_enabled(false);
                std::thread::spawn(crate::keep::delete_all);
            }
            set(p);
        }
        Action::Open(url) => open_url(&url),
        Action::Shows(s) => {
            p.taskbar_shows = s;
            set(p);
        }
        Action::Language(l) => {
            p.language = l;
            set(p);
        }
        Action::Volume(v) => {
            p.volume = v;
            // Let the user hear the new volume.
            sound::play(sound::Kind::Done, &p.done_sound, v, true);
            set(p);
        }
        Action::Sound(kind, n) => {
            let choice = match n {
                0 => Some(SoundChoice::Off),
                1..=3 => Some(SoundChoice::Builtin(n)),
                // Custom: keep the chosen file, or ask for one.
                _ => match sound_of(&p, kind) {
                    file @ SoundChoice::File(_) => Some(file),
                    _ => settings_window::pick_wav(hwnd).map(SoundChoice::File),
                },
            };
            if let Some(choice) = choice {
                sound::play(kind, &choice, p.volume, true);
                set_sound(&mut p, kind, choice);
                set(p);
            }
        }
        Action::ChooseFile(kind) => {
            if let Some(path) = settings_window::pick_wav(hwnd) {
                let choice = SoundChoice::File(path);
                sound::play(kind, &choice, p.volume, true);
                set_sound(&mut p, kind, choice);
                set(p);
            }
        }
        Action::Play(kind) => sound::play(kind, &sound_of(&p, kind), p.volume, true),
    }
}

/// German translations of the app's messages, bubbles and tooltip (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("{0} of {1} · {2}", "{0} von {1} · {2}"),
    ("{0} (busiest engine)", "{0} (meistgenutzte Engine)"),
    ("Video memory", "Grafikspeicher"),
    ("↑ {0} sent · ↓ {1} received", "↑ {0} gesendet · ↓ {1} empfangen"),
    ("{0} busy", "{0} aktiv"),
    ("read {0}", "Lesen {0}"),
    ("write {0}", "Schreiben {0}"),
    ("{0} is done", "{0} ist fertig"),
    ("{0} in {1} needs you", "{0} in {1} braucht dich"),
    ("{0} in {1} is done", "{0} in {1} ist fertig"),
    ("Copilot monthly", "Copilot Monat"),
    ("No folder's path on the clipboard", "Kein Ordnerpfad in der Zwischenablage"),
    ("Today", "Heute"),
    ("{0} of {1} daily budget · {2} over", "{0} von {1} Tagesbudget · {2} darüber"),
    ("{0} of {1} daily budget", "{0} von {1} Tagesbudget"),
    ("{0} daily budget", "{0} Tagesbudget"),
    ("Copilot's monthly limit at {0} %", "Monatslimit von Copilot bei {0} %"),
    ("Premium requests", "Premium-Anfragen"),
    ("Chat messages", "Chat-Nachrichten"),
    ("{0} of {1} left", "{0} von {1} übrig"),
    ("{0} left", "{0} übrig"),
    ("Copilot sessions can't be switched to from here yet.", "Zu Copilot-Sitzungen kann man noch nicht wechseln."),
    ("Resets {0}", "Reset {0}"),
    ("Weekly limit at {0} %", "Wochenlimit bei {0} %"),
    ("At this pace, around {0} (resets {1})", "Bei diesem Tempo etwa {0} (Reset {1})"),
    ("The weekly limit could run out early", "Das Wochenlimit könnte früher aufgebraucht sein"),
    ("5-hour limit at {0} %", "5-Stunden-Limit bei {0} %"),
    ("The weekly limit is free again", "Das Wochenlimit ist wieder frei"),
    ("The 5-hour limit is free again", "Das 5-Stunden-Limit ist wieder frei"),
    ("Weekly and 5-hour limits are free again", "Wochen- und 5-Stunden-Limit sind wieder frei"),
    ("Back to work, Claude is ready.", "Weiter geht's, Claude ist bereit."),
    ("{0} · resets {1}", "{0} · Reset {1}"),
    ("Plan", "Plan"),
    ("Codex weekly", "Codex Woche"),
    ("Codex 5-hour", "Codex 5 Stunden"),
    ("None active", "Keine aktiv"),
    ("Snoozed", "Pausiert"),
    ("until {0}", "bis {0}"),
    ("never", "nie"),
    ("Updated", "Aktualisiert"),
    ("{0} (refresh failed)", "{0} (Aktualisierung fehlgeschlagen)"),
    ("{0}: cache expires in {1}m", "{0}: Cache läuft in {1} Min. ab"),
    ("Codex sessions can't be switched to from here yet: Codex doesn't say which terminal runs them.", "Zu Codex-Sitzungen kann man noch nicht wechseln: Codex verrät nicht, in welchem Terminal sie laufen."),
    ("Demo mode: this would now open the session in VS Code.", "Demo-Modus: Jetzt würde sich die Sitzung in VS Code öffnen."),
    ("Demo mode: this would now switch to the session's terminal.", "Demo-Modus: Jetzt würde zum Terminal der Sitzung gewechselt."),
    ("This session runs in a terminal in WSL, so it can't be switched to from here.", "Diese Sitzung läuft in einem Terminal in WSL, dorthin kann Trayward nicht wechseln."),
    ("This session runs in a terminal Trayward can't find.", "Diese Sitzung läuft in einem Terminal, das Trayward nicht findet."),
    ("Couldn't save {0}:
{1}", "{0} konnte nicht gespeichert werden:
{1}"),
    ("Last {0}", "Letzte {0}"),
    ("Working · {0}", "Arbeitet · {0}"),
    ("Needs you · {0}", "Braucht dich · {0}"),
    ("Done · {0}", "Fertig · {0}"),
    ("wakes up {0}", "meldet sich {0}"),
    ("wake-up", "Weckruf"),
    ("command", "Befehl"),
    ("monitor", "Monitor"),
    ("agent", "Agent"),
    ("Couldn't change session tracking:

{0}", "Die Sitzungsverfolgung ließ sich nicht ändern:

{0}"),
    ("Couldn't switch to the session.

{0}", "Wechsel zur Sitzung nicht möglich.

{0}"),
    ("Couldn't change \"Start with Windows\":

{0}", "„Mit Windows starten“ ließ sich nicht ändern:

{0}"),
    ("Your week: {0} prompts, {1} tokens", "Deine Woche: {0} Prompts, {1} Token"),
    ("{0} API value · Claude worked {1}", "{0} API-Wert · Claude hat {1} gearbeitet"),
    ("Sent: have a look at your phone", "Gesendet: schau aufs Handy"),
    ("Not sent: {0}", "Nicht gesendet: {0}"),
    ("Sending…", "Wird gesendet …"),
    ("Trayward can reach your phone.", "Trayward erreicht dein Handy."),
    ("No https:// address on the clipboard", "Keine https://-Adresse in der Zwischenablage"),
    ("Not an https:// address", "Keine https://-Adresse"),
    ("The demo sends nothing", "Die Demo sendet nichts"),
    ("The server answered {0}", "Der Server antwortete {0}"),
    ("JSON to {0}", "JSON an {0}"),
    ("Trayward {0} is out", "Trayward {0} ist da"),
    ("Click for what's new and the download", "Klick für Neuigkeiten und den Download"),
    ("The demo doesn't check", "Die Demo prüft nicht"),
    ("Checking…", "Wird geprüft …"),
    ("Couldn't check: {0}", "Prüfen ging nicht: {0}"),
    ("Up to date · checked {0}", "Aktuell · geprüft {0}"),
    ("Not checked yet", "Noch nicht geprüft"),
    ("GitHub answered {0}", "GitHub antwortete {0}"),
    ("The demo connects nowhere", "Die Demo verbindet sich nirgends"),
    ("Connecting…", "Verbindet …"),
    ("Connected", "Verbunden"),
    ("Not connected: {0}", "Nicht verbunden: {0}"),
    ("No mqtt:// address on the clipboard", "Keine mqtt://-Adresse in der Zwischenablage"),
    ("the broker doesn't speak MQTT 3.1.1", "der Broker spricht kein MQTT 3.1.1"),
    ("the broker refused the client id", "der Broker lehnt die Client-ID ab"),
    ("the broker is unavailable", "der Broker ist nicht verfügbar"),
    ("wrong user name or password", "Benutzer oder Passwort falsch"),
    ("not authorised", "nicht berechtigt"),
    ("the broker answered {0}", "der Broker antwortete {0}"),
    ("{0} not found ({1})", "{0} nicht gefunden ({1})"),
    ("{0} not found", "{0} nicht gefunden"),
    ("no answer from the broker", "keine Antwort vom Broker"),
    ("connection lost", "Verbindung verloren"),
    ("TLS: {0}", "TLS: {0}"),
    ("{0} {1} · {2}", "{0} {1} · {2}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_longer_after_rate_limits() {
        assert_eq!(fetch_wait(120, 0, None), 120);
        // Doubled for each one in a row, up to the longest wait.
        assert_eq!(fetch_wait(120, 1, None), 240);
        assert_eq!(fetch_wait(120, 9, None), MAX_BACKOFF_SECS);
        // At least what the server asked for, within reason.
        assert_eq!(fetch_wait(120, 1, Some(600)), 600);
        assert_eq!(fetch_wait(120, 1, Some(30)), 240);
        assert_eq!(fetch_wait(120, 1, Some(999_999)), MAX_RETRY_AFTER_SECS);
        // An interval above the longest back-off stays.
        assert_eq!(fetch_wait(1800, 3, None), 1800);
    }

    fn reading(cached: bool, agent: Option<u32>) -> collector::GpgReading {
        collector::GpgReading { cached, agent, agent_started: None, default_ttl: 600, max_ttl: 7200, last_commit: None }
    }

    #[test]
    fn leaves_out_titles_the_row_already_says() {
        assert!(same_topic("Fix the flaky login test", "Fix the flaky login test"));
        assert!(same_topic("fix: flaky login", "Fix flaky login (CI)"));
        assert!(!same_topic("Move CI to GitHub Actions", "Release notes for 2.4"));
        assert!(!same_topic("", "anything"));
    }

    #[test]
    fn shortens_titles_for_pills() {
        assert_eq!(short_title("", 24), None);
        assert_eq!(short_title("  Fix login  ", 24).as_deref(), Some("Fix login"));
        assert_eq!(short_title("Fix the flaky login test", 24).as_deref(), Some("Fix the flaky login test"));
        // Cut at a word when it ends late enough, else mid-word.
        assert_eq!(short_title("Move CI to GitHub Actions and more", 24).as_deref(), Some("Move CI to GitHub…"));
        // Cut words aren't words: the spell check skips these lines.
        // cspell:disable-next-line
        assert_eq!(short_title("Supercalifragilisticexpialidocious", 24).as_deref(), Some("Supercalifragilisticexp…"));
        // cspell:disable-next-line
        assert_eq!(short_title("Ünïcödé títlé wïth áccents everywhere", 12).as_deref(), Some("Ünïcödé tít…"));
    }

    #[test]
    fn gpg_timer_counts_from_the_unlocking() {
        let mut g = GpgTimer::default();
        assert!(!g.update(reading(false, Some(7)), 1000));
        assert!(!g.view(1000).unwrap().unlocked);
        // Unlocked: the 10 minutes run from now, and that's worth saving.
        assert!(g.update(reading(true, Some(7)), 1030));
        assert_eq!(g.saved, Some((7, 1030)));
        assert_eq!(g.view(1100).unwrap().left, Some(530));
        // A commit restarts the 10 minutes; the 2 hours still count from unlocking.
        g.update(collector::GpgReading { last_commit: Some(8000), ..reading(true, Some(7)) }, 8000);
        let v = g.view(8100).unwrap();
        assert_eq!((v.left, v.total), (Some(130), 7200));
        // Locked again: nothing to save.
        assert!(g.update(reading(false, Some(7)), 9000));
        assert_eq!(g.saved, None);
    }

    #[test]
    fn gpg_timer_remembers_across_restarts() {
        // The last run saw agent 7 unlock the key at 1000.
        let fresh = || GpgTimer { saved: Some((7, 1000)), ..GpgTimer::default() };
        let mut g = fresh();
        assert!(!g.update(reading(true, Some(7)), 2000));
        assert_eq!(g.view(2000).unwrap().left, None, "10 minutes after unlocking, nothing signed: past the estimate");
        let mut g = fresh();
        g.update(collector::GpgReading { max_ttl: 28_800, default_ttl: 28_800, ..reading(true, Some(7)) }, 2000);
        assert_eq!(g.view(2000).unwrap().left, Some(27_800));
        // Another agent: the saved time is someone else's.
        let mut g = fresh();
        g.update(reading(true, Some(8)), 2000);
        assert_eq!(g.unlocked_at, None);
        // Unknown, but the agent started at 1500: at least until 1500 + 2 hours.
        let mut g = GpgTimer::default();
        g.update(collector::GpgReading { agent_started: Some(1500), ..reading(true, Some(7)) }, 2000);
        let v = g.view(2000).unwrap();
        assert_eq!((v.left, v.at_least), (Some(6700), true));
        // A saved time from before the agent started belongs to an older agent with the same pid.
        let mut g = fresh();
        g.update(collector::GpgReading { agent_started: Some(1500), ..reading(true, Some(7)) }, 2000);
        assert_eq!(g.unlocked_at, None);
        // An agent restarted between two looks and already unlocked again.
        let mut g = GpgTimer::default();
        g.update(reading(true, Some(7)), 1000);
        g.update(reading(true, Some(9)), 1030);
        assert_eq!(g.unlocked_at, Some(1030));
    }
}
