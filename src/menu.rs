//! The widget's right-click menu: just the everyday actions; everything else
//! is in the settings window. Runs outside event handling because the menu
//! has its own message loop.

use crate::app::{self, Event};
use crate::i18n::{f, t};
use crate::settings::Prefs;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    PostMessageW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{HSTRING, PCWSTR};

const ID_REFRESH: usize = 1;
/// Display mode: normal or slim.
const ID_NORMAL: usize = 2;
const ID_SLIM: usize = 10;
const ID_SETTINGS: usize = 3;
const ID_EXIT: usize = 4;
const ID_MUTE: usize = 5;
const ID_BUBBLES: usize = 6;
const ID_RESUME: usize = 7;
const ID_UPDATE: usize = 8;
const ID_ABOUT: usize = 9;
/// Snooze choices, from this id on (`SNOOZE`).
const ID_SNOOZE: usize = 20;
/// Snooze for: label and minutes (0: until 8:00 tomorrow).
const SNOOZE: [(&str, i64); 4] = [("30 minutes", 30), ("1 hour", 60), ("2 hours", 120), ("Until tomorrow morning", 0)];
/// Demo mode: the plans, from this id on (`demo::PLANS`).
const ID_DEMO_PLAN: usize = 100;

/// Shows the menu at the mouse. `snoozed_until`: notifications are snoozed until then (unix seconds).
/// `update`: a newer release.
pub fn show(owner: HWND, prefs: &Prefs, snoozed_until: Option<i64>, update: Option<&crate::update::Release>) {
    let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
    // SAFETY: the menu is created and destroyed here; TrackPopupMenu runs modally.
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, ID_REFRESH, &HSTRING::from(t("Refresh now")));
        if let Ok(modes) = CreatePopupMenu() {
            let _ = AppendMenuW(modes, check(!prefs.slim), ID_NORMAL, &HSTRING::from(t("Normal")));
            let _ = AppendMenuW(modes, check(prefs.slim), ID_SLIM, &HSTRING::from(t("Slim")));
            // The submenu is destroyed with the menu.
            let _ = AppendMenuW(menu, MF_POPUP, modes.0 as usize, &HSTRING::from(t("Display mode")));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, check(prefs.muted), ID_MUTE, &HSTRING::from(t("Mute sounds")));
        let _ = AppendMenuW(menu, check(prefs.bubbles), ID_BUBBLES, &HSTRING::from(t("Notifications")));
        match snoozed_until {
            Some(until) => {
                let text = f("Snoozed until {0}: resume", &[&crate::timefmt::local_time(until, true)]);
                let _ = AppendMenuW(menu, MF_STRING | MF_CHECKED, ID_RESUME, &HSTRING::from(text));
            }
            None => {
                if let Ok(snooze) = CreatePopupMenu() {
                    for (i, (label, _)) in SNOOZE.iter().enumerate() {
                        let _ = AppendMenuW(snooze, MF_STRING, ID_SNOOZE + i, &HSTRING::from(t(label)));
                    }
                    let _ = AppendMenuW(menu, MF_POPUP, snooze.0 as usize, &HSTRING::from(t("Snooze notifications")));
                }
            }
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, &HSTRING::from(t("Settings…")));
        let _ = AppendMenuW(menu, MF_STRING, ID_ABOUT, &HSTRING::from(t("About Trayward")));
        if let Some(u) = update {
            let text = f("Version {0} is out: download", &[&u.version]);
            let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, &HSTRING::from(text));
        }
        // The demo can show every plan, to see how each looks.
        if crate::demo::active() {
            if let Ok(plans) = CreatePopupMenu() {
                for (i, name) in crate::demo::PLANS.iter().enumerate() {
                    let flags = check(*name == crate::demo::plan());
                    let _ = AppendMenuW(plans, flags, ID_DEMO_PLAN + i, &HSTRING::from(*name));
                }
                // The submenu is destroyed with the menu.
                let _ = AppendMenuW(menu, MF_POPUP, plans.0 as usize, &HSTRING::from(t("Demo plan")));
            }
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, &HSTRING::from(t("Exit")));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // The owner must be in front, or the menu doesn't close when clicking elsewhere.
        let _ = SetForegroundWindow(owner);
        let cmd =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, None, owner, None);
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    };
    match cmd {
        ID_REFRESH => app::dispatch(Event::Refresh),
        ID_NORMAL | ID_SLIM => {
            let slim = cmd == ID_SLIM;
            if slim != prefs.slim {
                app::dispatch(Event::SetPrefs(Box::new(Prefs { slim, ..prefs.clone() })));
            }
        }
        ID_SETTINGS => app::dispatch(Event::OpenSettings),
        ID_ABOUT => {
            app::dispatch(Event::OpenSettings);
            app::dispatch(Event::SettingsPage(crate::settings_window::PAGES.len() - 1));
        }
        ID_UPDATE => {
            if let Some(u) = update {
                app::open_url(&u.url);
            }
        }
        ID_EXIT => app::dispatch(Event::Exit),
        ID_MUTE => app::dispatch(Event::SetPrefs(Box::new(Prefs { muted: !prefs.muted, ..prefs.clone() }))),
        ID_BUBBLES => app::dispatch(Event::SetPrefs(Box::new(Prefs { bubbles: !prefs.bubbles, ..prefs.clone() }))),
        ID_RESUME => app::dispatch(Event::Snooze(None)),
        id if (ID_SNOOZE..ID_SNOOZE + SNOOZE.len()).contains(&id) => {
            let now = crate::win::now_ms() / 1000;
            let until = match SNOOZE[id - ID_SNOOZE].1 {
                0 => crate::timefmt::next_local(now, 8),
                minutes => now + minutes * 60,
            };
            app::dispatch(Event::Snooze(Some(until)));
        }
        id if (ID_DEMO_PLAN..ID_DEMO_PLAN + crate::demo::PLANS.len()).contains(&id) => {
            app::dispatch(Event::DemoPlan(id - ID_DEMO_PLAN));
        }
        _ => {}
    }
}

/// German translations of the right-click menu (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("Refresh now", "Jetzt aktualisieren"),
    ("Display mode", "Darstellung"),
    ("Normal", "Normal"),
    ("Mute sounds", "Töne stumm"),
    ("Settings…", "Einstellungen…"),
    ("Exit", "Beenden"),
    ("Snooze notifications", "Benachrichtigungen pausieren"),
    ("Demo plan", "Demo-Plan"),
    ("Snoozed until {0}: resume", "Pausiert bis {0}: fortsetzen"),
    ("30 minutes", "30 Minuten"),
    ("1 hour", "1 Stunde"),
    ("2 hours", "2 Stunden"),
    ("Until tomorrow morning", "Bis morgen früh"),
    ("About Trayward", "Über Trayward"),
    ("Version {0} is out: download", "Version {0} ist da: herunterladen"),
];
