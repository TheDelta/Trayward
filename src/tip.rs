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
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
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
/// A heading's text size; the rows' is 12.
const HEADING_SIZE: f32 = 13.0;

/// A bar in front of a value: this wide, this high.
const BAR_W: f32 = 64.0;
const BAR_H: f32 = 4.0;
const BAR_GAP: f32 = 8.0;

/// What the tip says: rows of a label and a value (or [`DIVIDER`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub rows: Vec<(String, String)>,
    /// The rows (by index) whose value is a warning, shown in yellow.
    pub warnings: Vec<usize>,
    /// The rows (by index) shown otherwise: in a colour, or with a bar.
    pub looks: Vec<(usize, Look)>,
    /// The rows (by index) that head a part (the user's own extensions): the label
    /// larger, across the tip; a value beside it (a note) stays in the value column.
    pub headings: Vec<usize>,
}

/// How a row's value shows besides its text (the user's own extensions).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub ink: Ink,
    /// A bar in front of the text, filled this far (0 to 1).
    pub bar: Option<f32>,
}

/// A value's colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ink {
    #[default]
    Plain,
    Good,
    Warn,
    Bad,
}

/// Lays out the tip and returns its size in DIPs. With a canvas it also draws.
fn render(c: Option<&Canvas>, m: &Model) -> Result<(f32, f32)> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    let heading = |i: usize| m.headings.contains(&i);
    let mut rows = Vec::new();
    for (i, (label, value)) in m.rows.iter().enumerate() {
        let size = if heading(i) { HEADING_SIZE } else { 12.0 };
        rows.push((g.text(label, size, SEMIBOLD, None)?, g.text(value, 12.0, NORMAL, Some(MAX_VALUE_W))?));
    }
    let look = |i: usize| m.looks.iter().find(|(r, _)| *r == i).map(|(_, l)| *l).unwrap_or_default();
    let bar_w = |i: usize| if look(i).bar.is_some() { BAR_W + BAR_GAP } else { 0.0 };
    // A heading doesn't widen the label column: it runs on into the value's.
    let label_w = rows.iter().enumerate().filter(|(i, _)| !heading(*i)).map(|(_, (l, _))| l.width).fold(0.0, f32::max);
    let value_x = |i: usize, label: f32| {
        let column = PAD + label_w + COL_GAP;
        if heading(i) { column.max(PAD + label + COL_GAP) } else { column }
    };
    let width = rows
        .iter()
        .enumerate()
        .map(|(i, (l, v))| {
            let value = bar_w(i) + v.width;
            if value > 0.0 { value_x(i, l.width) + value + PAD } else { PAD + l.width + PAD }
        })
        .fold(0.0, f32::max);
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
            c.text(label, PAD, y, if heading(i) { pal.text } else { pal.text_dim });
            let look = look(i);
            let ink = match look.ink {
                _ if m.warnings.contains(&i) => pal.warn,
                Ink::Plain => pal.text,
                Ink::Good => pal.done,
                Ink::Warn => pal.warn,
                Ink::Bad => pal.crit,
            };
            let mut x = value_x(i, label.width);
            if let Some(fill) = look.bar {
                let by = y + (value.height - BAR_H) / 2.0;
                c.fill_round_rect(x, by, BAR_W, BAR_H, BAR_H / 2.0, pal.track);
                let bar = if look.ink == Ink::Plain { pal.accent } else { ink };
                if fill > 0.0 {
                    c.fill_round_rect(x, by, (BAR_W * fill.clamp(0.0, 1.0)).max(BAR_H), BAR_H, BAR_H / 2.0, bar);
                }
                x += BAR_W + BAR_GAP;
            }
            // Beside a heading, the value sits on the heading's baseline.
            let vy = if heading(i) { y + (label.height - value.height).max(0.0) * 0.75 } else { y };
            c.text(value, x, vy, ink);
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
            self.scale = dpi.max(96) as f32 / 96.0;
            let (w, h) = ((w_dip * self.scale).ceil() as i32, (h_dip * self.scale).ceil() as i32);
            let margin = (MARGIN * self.scale) as i32;
            // Right next to the widget, on the side away from the taskbar's edge.
            let (x, y) = crate::taskbars::popup(&anchor, w, h, margin, true);
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
        let one = Model {
            dark: false,
            warnings: vec![],
            looks: vec![],
            headings: vec![],
            rows: vec![row("Weekly", "46 % · resets Sat 23:31")],
        };
        let three = Model {
            rows: vec![row("Weekly", "46 %"), row("5-hour", "31 %"), row("Sessions", "None active")],
            ..one.clone()
        };
        let (w1, h1) = render(None, &one).unwrap();
        let (_, h3) = render(None, &three).unwrap();
        assert!(w1 > 100.0 && h3 > h1 * 2.0);
        // A bar in front of a value makes room for it.
        let bar = Model { looks: vec![(0, Look { ink: Ink::Good, bar: Some(0.4) })], ..one.clone() };
        assert_eq!(render(None, &bar).unwrap().0, w1 + BAR_W + BAR_GAP);
        // A heading leaves the label column as it was; it only widens the tip when it doesn't fit.
        let long = "A nightly backup";
        let headed = Model {
            rows: vec![row(long, ""), row("Weekly", "46 % · resets Sat 23:31")],
            headings: vec![0],
            ..one.clone()
        };
        let (wh, hh) = render(None, &headed).unwrap();
        assert_eq!(wh, w1);
        assert!(hh > render(None, &one).unwrap().1);
        let longer = Model { rows: vec![row(&long.repeat(3), ""), row("Weekly", "46 %")], ..headed.clone() };
        assert!(render(None, &longer).unwrap().0 > w1);
    }

    /// Lays `m` out and draws it into memory: the same size, and something shows.
    fn drawn(m: &Model) -> (f32, f32) {
        let size = render(None, m).unwrap();
        let mut again = None;
        let px = gfx::testing::offscreen(size.0.ceil() as i32, size.1.ceil() as i32, 96.0, |c| {
            again = Some(render(Some(c), m).unwrap());
        })
        .unwrap();
        assert!(gfx::testing::drew(&px));
        assert_eq!(again, Some(size));
        size
    }

    #[test]
    fn draws_every_kind_of_row_in_both_themes() {
        let row = |l: &str, v: &str| (l.to_string(), v.to_string());
        let rows = vec![
            row("Weekly", "46 % · resets Sat 23:31"),
            row("5-hour", "97 %"),
            DIVIDER,
            row("Backup", "Not updated for 2h"),
            row("Photos", "40%"),
            row("Music", "0%"),
            row("Next run", "in 1h"),
            row("Last run", "failed"),
            row("Documents", "Done"),
            DIVIDER,
            row("Sessions", ""),
            row("A rather long value", &"that goes on and on ".repeat(30)),
        ];
        let plain = Look::default();
        let looks = vec![
            (4, Look { bar: Some(0.4), ..plain }),
            (5, Look { ink: Ink::Warn, bar: Some(0.0) }),
            (6, Look { ink: Ink::Good, bar: Some(1.5) }),
            (7, Look { ink: Ink::Bad, bar: None }),
            (8, Look { ink: Ink::Good, bar: None }),
            (1, Look { ink: Ink::Bad, bar: None }),
        ];
        for dark in [true, false] {
            let m = Model { dark, rows: rows.clone(), warnings: vec![1], looks: looks.clone(), headings: vec![3, 10] };
            let (w, h) = drawn(&m);
            // A long value is cut at its column's width.
            assert!(w < 2.0 * PAD + COL_GAP + MAX_VALUE_W + BAR_W + BAR_GAP + 120.0, "{w}");
            assert!(h > 10.0 * 16.0, "{h}");
        }
        assert!(gfx::take_cut().iter().any(|c| c.starts_with("that goes on")));
    }

    #[test]
    fn opens_and_closes_only_with_a_window() {
        let mut tip = Tip::new();
        // Without its window class (the app registers it when it starts): no window.
        tip.create();
        assert!(tip.hwnd.is_none());
        let m = Model {
            dark: true,
            rows: vec![("Weekly".into(), "46 %".into())],
            warnings: vec![],
            looks: vec![],
            headings: vec![],
        };
        tip.show(m, RECT::default(), 144);
        assert!(!tip.visible());
        tip.hide();
        assert!(!tip.visible() && tip.sizer.shown.is_none());
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
