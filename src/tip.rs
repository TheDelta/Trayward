//! The tooltip of the widget: the limits with their reset times and the
//! sessions in a few lines, after the mouse rests on the widget for a moment.
//! In Trayward's own style (light and dark), above the widget; it never takes
//! focus or the mouse.

use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::theme::{self, Palette};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    PAINTSTRUCT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, HWND_TOPMOST, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Tip");

const PAD: f32 = 10.0;
const COL_GAP: f32 = 14.0;
const ROW_GAP: f32 = 3.0;
const MAX_VALUE_W: f32 = 320.0;
/// Space between the taskbar and the tip.
const MARGIN: f32 = 8.0;

/// A row that draws a line between parts of the tip.
pub const DIVIDER: (String, String) = (String::new(), String::new());
const DIVIDER_GAP: f32 = 5.0;

/// What the tip says: rows of a label and a value (or [`DIVIDER`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub rows: Vec<(String, String)>,
    /// The rows (by index) whose value is a warning, shown in yellow.
    pub warnings: Vec<usize>,
}

/// Lays out the tip and returns its size in DIPs. With a canvas it also draws.
fn render(c: Option<&Canvas>, m: &Model) -> Result<(f32, f32)> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    let mut rows = Vec::new();
    for (label, value) in &m.rows {
        rows.push((g.text(label, 12.0, SEMIBOLD, None)?, g.text(value, 12.0, NORMAL, Some(MAX_VALUE_W))?));
    }
    let label_w = rows.iter().map(|(l, _)| l.width).fold(0.0, f32::max);
    let value_w = rows.iter().map(|(_, v)| v.width).fold(0.0, f32::max);
    let width = PAD + label_w + COL_GAP + value_w + PAD;
    let mut y = PAD;
    for (i, ((label, value), row)) in rows.iter().zip(&m.rows).enumerate() {
        if *row == DIVIDER {
            if let Some(c) = c {
                c.fill_rect(PAD, y + DIVIDER_GAP - ROW_GAP, width - 2.0 * PAD, 1.0, pal.divider);
            }
            y += 2.0 * DIVIDER_GAP - ROW_GAP + 1.0;
            continue;
        }
        if let Some(c) = c {
            c.text(label, PAD, y, pal.text_dim);
            let ink = if m.warnings.contains(&i) { pal.warn } else { pal.text };
            c.text(value, PAD + label_w + COL_GAP, y, ink);
        }
        y += label.height.max(value.height) + ROW_GAP;
    }
    Ok((width, y - ROW_GAP + PAD))
}

/// A smaller size this close to the shown one keeps the shown one.
const SLACK: f32 = 12.0;
/// How long the content must stay clearly smaller before the tip shrinks.
const SHRINK_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// The tip's size while it's open: it grows at once, but shrinks only when
/// the content has been clearly smaller for a few seconds, so values that
/// change every second (network rates) don't make it jump.
#[derive(Default)]
struct Sizer {
    shown: Option<(f32, f32)>,
    smaller_since: Option<std::time::Instant>,
}

impl Sizer {
    fn size(&mut self, want: (f32, f32), now: std::time::Instant) -> (f32, f32) {
        let Some(shown) = self.shown else {
            self.shown = Some(want);
            return want;
        };
        let clearly_smaller = shown.0 - want.0 > SLACK || shown.1 - want.1 > SLACK;
        if !clearly_smaller {
            self.smaller_since = None;
        }
        let waited = clearly_smaller && now.duration_since(*self.smaller_since.get_or_insert(now)) >= SHRINK_AFTER;
        let size = if waited {
            self.smaller_since = None;
            want
        } else {
            (shown.0.max(want.0), shown.1.max(want.1))
        };
        self.shown = Some(size);
        size
    }
}

pub struct Tip {
    hwnd: Option<HWND>,
    model: Option<Model>,
    scale: f32,
    sizer: Sizer,
}

impl Tip {
    pub const fn new() -> Tip {
        Tip { hwnd: None, model: None, scale: 1.0, sizer: Sizer { shown: None, smaller_since: None } }
    }

    pub fn create(&mut self) {
        // SAFETY: plain window creation with a registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                CLASS,
                w!("Trayward"),
                WS_POPUP,
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
        if let Ok(hwnd) = hwnd {
            let round = DWMWCP_ROUND;
            // SAFETY: attribute value of the documented size.
            let _ = unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &round as *const _ as *const _,
                    std::mem::size_of_val(&round) as u32,
                )
            };
            self.hwnd = Some(hwnd);
        }
    }

    pub fn visible(&self) -> bool {
        self.model.is_some()
    }

    /// Shows `model` above `anchor` (the widget, at `dpi`), or updates it in place.
    pub fn show(&mut self, model: Model, anchor: RECT, dpi: u32) {
        let Some(hwnd) = self.hwnd else { return };
        // The same content needs nothing, unless a smaller size is waiting to be taken.
        if self.model.as_ref() == Some(&model) && self.sizer.smaller_since.is_none() {
            return;
        }
        let Ok(want) = render(None, &model) else { return };
        let (w_dip, h_dip) = self.sizer.size(want, std::time::Instant::now());
        // SAFETY: Win32 calls on our own window with valid out-pointers.
        unsafe {
            let dark = BOOL::from(model.dark);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                std::mem::size_of::<BOOL>() as u32,
            );
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST), &mut mi);
            let work = mi.rcWork;
            self.scale = dpi.max(96) as f32 / 96.0;
            let (w, h) = ((w_dip * self.scale).ceil() as i32, (h_dip * self.scale).ceil() as i32);
            let margin = (MARGIN * self.scale) as i32;
            // Right-aligned with the widget, like the flyout.
            // On a work area narrower than this, the left edge wins (a clamp with max < min panics).
            let left = work.left + margin;
            let x = (anchor.right - w).clamp(left, (work.right - w - margin).max(left));
            let y = (anchor.top.min(work.bottom) - h - margin).max(work.top);
            self.model = Some(model);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    pub fn hide(&mut self) {
        if let (Some(hwnd), true) = (self.hwnd, self.visible()) {
            self.model = None;
            self.sizer = Sizer::default();
            // SAFETY: our own window.
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
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
                let _ = gfx::paint_window(hwnd, rc.right, rc.bottom, 96.0 * self.scale, pal.surface, |c| {
                    let _ = render(Some(c), m);
                });
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_to_its_rows() {
        let row = |l: &str, v: &str| (l.to_string(), v.to_string());
        let one = Model { dark: false, warnings: vec![], rows: vec![row("Weekly", "46 % · resets Sat 23:31")] };
        let three = Model {
            rows: vec![row("Weekly", "46 %"), row("5-hour", "31 %"), row("Sessions", "None active")],
            ..one.clone()
        };
        let (w1, h1) = render(None, &one).unwrap();
        let (_, h3) = render(None, &three).unwrap();
        assert!(w1 > 100.0 && h3 > h1 * 2.0);
    }

    #[test]
    fn grows_at_once_and_shrinks_late() {
        use std::time::{Duration, Instant};
        let mut s = Sizer::default();
        let t0 = Instant::now();
        assert_eq!(s.size((200.0, 100.0), t0), (200.0, 100.0));
        assert_eq!(s.size((230.0, 100.0), t0), (230.0, 100.0));
        // A few DIPs smaller: stays.
        assert_eq!(s.size((224.0, 100.0), t0 + Duration::from_secs(10)), (230.0, 100.0));
        // Clearly smaller: only after a few seconds of it.
        assert_eq!(s.size((180.0, 80.0), t0 + Duration::from_secs(11)), (230.0, 100.0));
        assert_eq!(s.size((180.0, 80.0), t0 + Duration::from_secs(12)), (230.0, 100.0));
        assert_eq!(s.size((180.0, 80.0), t0 + Duration::from_secs(14)), (180.0, 80.0));
        // Back to about the shown size in between: the wait starts again.
        let mut s = Sizer::default();
        s.size((200.0, 100.0), t0);
        s.size((150.0, 100.0), t0);
        s.size((199.0, 100.0), t0 + Duration::from_secs(2));
        assert_eq!(s.size((150.0, 100.0), t0 + Duration::from_secs(4)), (200.0, 100.0));
    }
}
