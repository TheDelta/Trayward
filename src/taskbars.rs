//! The taskbars Windows shows: the main one and, with "Show my taskbar on all
//! displays", one on each other display. Each is described by its display,
//! with an id that stays the same across restarts, so a choice of taskbars
//! can be remembered.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, IsWindowVisible};
use windows::core::{PCWSTR, w};

/// A taskbar and the display it's on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Taskbar {
    pub hwnd: HWND,
    /// The main taskbar (with the notification area).
    pub main: bool,
    /// Its display's handle and bounds (screen pixels).
    pub monitor: isize,
    pub bounds: RECT,
}

/// A display with a taskbar, as shown in the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    /// Stays the same for a monitor on the same connector: its device path,
    /// or its GDI name if that's unknown.
    pub id: String,
    /// The monitor's model, e.g. "DELL U2720Q".
    pub name: String,
    /// Where it is and its resolution.
    pub detail: String,
    pub main: bool,
    pub taskbar: HWND,
}

/// The taskbars shown now: the main one first, then the others left to right.
pub fn list() -> Vec<Taskbar> {
    let mut found = Vec::new();
    // SAFETY: window lookups and monitor queries with valid out-pointers.
    unsafe {
        let mut add = |hwnd: HWND, main: bool| {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(monitor, &mut mi.monitorInfo).as_bool() {
                found.push(Taskbar { hwnd, main, monitor: monitor.0 as isize, bounds: mi.monitorInfo.rcMonitor });
            }
        };
        if let Ok(main) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            add(main, true);
        }
        let mut after = None;
        while let Ok(h) = FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) {
            if IsWindowVisible(h).as_bool() {
                add(h, false);
            }
            after = Some(h);
        }
    }
    found.sort_by_key(|t| (!t.main, t.bounds.left, t.bounds.top));
    found
}

/// How many displays are connected (with a taskbar or not).
pub fn monitor_count() -> usize {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
    // SAFETY: plain query.
    usize::try_from(unsafe { GetSystemMetrics(SM_CMONITORS) }).unwrap_or(1)
}

/// Names and ids for the displays of `taskbars`.
pub fn describe(taskbars: &[Taskbar]) -> Vec<Display> {
    let names = monitor_names();
    let main = taskbars.iter().find(|t| t.main).map(|t| t.bounds);
    let mut out: Vec<Display> = taskbars
        .iter()
        .map(|t| {
            let gdi = gdi_name(t.monitor);
            let known = names.iter().find(|n| n.gdi == gdi);
            let id = known.map(|n| n.path.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| gdi.clone());
            let name = known.map(|n| n.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
                if known.is_some_and(|n| n.internal) {
                    crate::i18n::t("Built-in display").into()
                } else {
                    crate::i18n::t("Display").into()
                }
            });
            let b = t.bounds;
            let size = format!("{} × {}", b.right - b.left, b.bottom - b.top);
            let place = if t.main {
                crate::i18n::t("Main display").to_string()
            } else {
                main.map_or_else(String::new, |m| side(&b, &m))
            };
            let detail = if place.is_empty() { size } else { format!("{place} · {size}") };
            Display { id, name, detail, main: t.main, taskbar: t.hwnd }
        })
        .collect();
    // Same model twice: number them so they can be told apart.
    let names: Vec<String> = out.iter().map(|d| d.name.clone()).collect();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for d in &mut out {
        if names.iter().filter(|n| **n == d.name).count() > 1 {
            let n = seen.entry(d.name.clone()).or_insert(0);
            *n += 1;
            d.name = format!("{} ({n})", d.name);
        }
    }
    out
}

/// Where `r` is, seen from the main display `main`.
fn side(r: &RECT, main: &RECT) -> String {
    let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let (mx, my) = ((main.left + main.right) / 2, (main.top + main.bottom) / 2);
    let (dx, dy) = (cx - mx, cy - my);
    let s = if dx.abs() >= dy.abs() {
        if dx < 0 { "Left of the main display" } else { "Right of the main display" }
    } else if dy < 0 {
        "Above the main display"
    } else {
        "Below the main display"
    };
    crate::i18n::t(s).to_string()
}

fn gdi_name(monitor: isize) -> String {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: valid out-pointer sized as stated.
    let ok = unsafe {
        GetMonitorInfoW(windows::Win32::Graphics::Gdi::HMONITOR(monitor as *mut _), &mut mi.monitorInfo).as_bool()
    };
    if ok { utf16(&mi.szDevice) } else { String::new() }
}

fn utf16(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

struct MonitorName {
    /// GDI name of the source, e.g. `\\.\DISPLAY1`.
    gdi: String,
    name: String,
    path: String,
    internal: bool,
}

/// Model names and device paths of the active displays.
fn monitor_names() -> Vec<MonitorName> {
    let mut out = Vec::new();
    // SAFETY: buffers sized by GetDisplayConfigBufferSizes; each info packet
    // carries its own size in its header.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            != ERROR_SUCCESS
        {
            return out;
        }
        paths.truncate(np as usize);
        for p in &paths {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }
            let gdi = utf16(&source.viewGdiDeviceName);
            // A mirrored source has several targets; the first one names it.
            if out.iter().any(|n: &MonitorName| n.gdi == gdi) {
                continue;
            }
            out.push(MonitorName {
                gdi,
                name: utf16(&target.monitorFriendlyDeviceName),
                path: utf16(&target.monitorDevicePath),
                internal: target.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
            });
        }
    }
    out
}

/// Which taskbars to show the widget on, given the setting and what's there.
/// Never none: if no chosen display is connected, the main taskbar.
pub fn pick<'a>(displays: &'a [Display], mode: crate::settings::Taskbars, chosen: &[String]) -> Vec<&'a Display> {
    use crate::settings::Taskbars;
    let picked: Vec<&Display> = match mode {
        Taskbars::Main => displays.iter().filter(|d| d.main).collect(),
        Taskbars::All => displays.iter().collect(),
        Taskbars::Chosen => displays.iter().filter(|d| chosen.contains(&d.id)).collect(),
    };
    if picked.is_empty() { displays.iter().filter(|d| d.main).take(1).collect() } else { picked }
}

/// German translations of the display names (see `i18n`).
#[rustfmt::skip]
pub const DE: &[(&str, &str)] = &[
    ("Built-in display", "Eingebauter Bildschirm"),
    ("Display", "Bildschirm"),
    ("Main display", "Hauptbildschirm"),
    ("Left of the main display", "Links vom Hauptbildschirm"),
    ("Right of the main display", "Rechts vom Hauptbildschirm"),
    ("Above the main display", "Über dem Hauptbildschirm"),
    ("Below the main display", "Unter dem Hauptbildschirm"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Taskbars;

    fn display(id: &str, main: bool) -> Display {
        Display { id: id.into(), name: id.into(), detail: String::new(), main, taskbar: HWND::default() }
    }

    #[test]
    fn picks_taskbars() {
        let all = [display("a", true), display("b", false), display("c", false)];
        let ids = |v: Vec<&Display>| v.iter().map(|d| d.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(pick(&all, Taskbars::Main, &[])), ["a"]);
        assert_eq!(ids(pick(&all, Taskbars::All, &[])), ["a", "b", "c"]);
        assert_eq!(ids(pick(&all, Taskbars::Chosen, &["c".into()])), ["c"]);
        // A chosen display that isn't connected: back to the main taskbar.
        assert_eq!(ids(pick(&all, Taskbars::Chosen, &["gone".into()])), ["a"]);
    }

    #[test]
    fn names_sides() {
        let main = RECT { left: 0, top: 0, right: 100, bottom: 100 };
        let at = |l, t| side(&RECT { left: l, top: t, right: l + 100, bottom: t + 100 }, &main);
        assert_eq!(at(100, 20), "Right of the main display");
        assert_eq!(at(-100, -20), "Left of the main display");
        assert_eq!(at(10, -100), "Above the main display");
        assert_eq!(at(0, 100), "Below the main display");
    }
}
