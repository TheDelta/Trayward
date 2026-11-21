//! Reads where things are on each taskbar that the widget can't learn from
//! windows: where the app buttons end (so the widget can tell how much room
//! is left), where Windows' Widgets button is (the widget can sit on it) and,
//! on the taskbars of other displays, where the clock starts (the widget sits
//! left of it). Both are XAML without windows of their own;
//! UI Automation is the supported way to see them.
//!
//! Runs on its own thread: UI Automation calls go to Explorer and must never
//! block the UI thread.

use crate::taskbars;
use crate::win::{SendHwnd, post_boxed};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::core::Result;

const TASK_BUTTON: &str = "Taskbar.TaskListButtonAutomationPeer";
/// The clock (with the date) on the taskbars of other displays.
const CLOCK: &str = "SystemTray.OmniButton";
/// Windows' Widgets button (the weather), by its automation id.
const WIDGETS: &str = "WidgetsButton";
const EVERY: Duration = Duration::from_secs(3);

/// What UI Automation found on one taskbar (screen pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edges {
    /// The taskbar window, as a number (window handles can't cross threads).
    pub taskbar: isize,
    /// Right edge of the last app button.
    pub buttons_right: Option<i32>,
    /// Left edge of the clock.
    pub clock_left: Option<i32>,
    /// Top edge of the clock (on a taskbar at the left or right).
    pub clock_top: Option<i32>,
    /// The Widgets button: its left and right edge.
    pub widgets: Option<(i32, i32)>,
}

fn edges(automation: &IUIAutomation, taskbar: HWND) -> Result<Edges> {
    // SAFETY: COM calls on valid interfaces; the taskbar handle is checked.
    unsafe {
        let mut tb = RECT::default();
        GetWindowRect(taskbar, &mut tb)?;
        let root = automation.ElementFromHandle(taskbar)?;
        // Fetch class names and positions in one round trip.
        let cache = automation.CreateCacheRequest()?;
        cache.AddProperty(UIA_ClassNamePropertyId)?;
        cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
        cache.AddProperty(UIA_AutomationIdPropertyId)?;
        let all = root.FindAllBuildCache(TreeScope_Descendants, &automation.CreateTrueCondition()?, &cache)?;
        let mut found = Edges {
            taskbar: taskbar.0 as isize,
            buttons_right: None,
            clock_left: None,
            clock_top: None,
            widgets: None,
        };
        for i in 0..all.Length()? {
            let e = all.GetElement(i)?;
            let class = e.CachedClassName()?;
            let r = e.CachedBoundingRectangle()?;
            // Only what's on this taskbar and actually shown.
            if r.right <= r.left
                || r.top < tb.top - 2
                || r.bottom > tb.bottom + 2
                || r.left < tb.left - 2
                || r.right > tb.right + 2
            {
                continue;
            }
            if class == TASK_BUTTON {
                found.buttons_right = Some(found.buttons_right.map_or(r.right, |x| x.max(r.right)));
            } else if class == CLOCK {
                found.clock_left = Some(found.clock_left.map_or(r.left, |x| x.min(r.left)));
                found.clock_top = Some(found.clock_top.map_or(r.top, |y| y.min(r.top)));
            } else if found.widgets.is_none() && e.CachedAutomationId()? == WIDGETS {
                found.widgets = Some((r.left, r.right));
            }
        }
        Ok(found)
    }
}

/// Starts the watcher. Posts `Vec<Edges>`, one per taskbar, to `ctl` as
/// message `msg` whenever something moved. It only looks while `active`.
pub fn spawn(ctl: SendHwnd, msg: u32, active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        // SAFETY: COM for this thread only; it lives as long as the process.
        let automation: Result<IUIAutomation> = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        };
        let Ok(automation) = automation else { return };
        let mut last: Option<Vec<Edges>> = None;
        loop {
            if !active.load(Ordering::Relaxed) {
                last = None;
                std::thread::sleep(EVERY);
                continue;
            }
            let now: Vec<Edges> = taskbars::list()
                .into_iter()
                .map(|t| {
                    let taskbar = t.hwnd.0 as isize;
                    // A query that failed (Explorer busy) keeps what was found
                    // last: "nothing found" would put the widget over the clock.
                    let before = || last.as_ref()?.iter().find(|e| e.taskbar == taskbar).copied();
                    edges(&automation, t.hwnd).ok().or_else(before).unwrap_or(Edges {
                        taskbar,
                        buttons_right: None,
                        clock_left: None,
                        clock_top: None,
                        widgets: None,
                    })
                })
                .collect();
            if last.as_ref() != Some(&now) {
                last = Some(now.clone());
                post_boxed(ctl, msg, now);
            }
            std::thread::sleep(EVERY);
        }
    });
}
