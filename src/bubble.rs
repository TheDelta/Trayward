//! A small speech bubble above the widget that says a session is done or
//! needs you. It shows for a few seconds without taking focus; hovering keeps
//! it open, clicking it switches to the session, right-clicking closes it (a
//! finished session then counts as seen).

use crate::gfx::{self, Canvas, NORMAL, SEMIBOLD};
use crate::sessions::Status;
use crate::theme::{self, Palette};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClientRect, HWND_TOPMOST, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, Result, w};

pub const CLASS: PCWSTR = w!("Trayward.Bubble");

const PAD: f32 = 12.0;
const MAX_TEXT_W: f32 = 300.0;
const MARGIN: f32 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub dark: bool,
    pub session_id: String,
    pub status: Status,
    /// A warning (red dot), e.g. an expiring cache.
    pub urgent: bool,
    /// e.g. "web-game is done".
    pub headline: String,
    /// The conversation title.
    pub detail: String,
    /// For a merge or pull request: its service's logo instead of the dot,
    /// and its state as a coloured pill before `detail`.
    pub badge: Option<Badge>,
}

/// A merge or pull request's service and what happened to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub forge: crate::forge::Forge,
    pub tone: Tone,
    /// e.g. "Pipeline passed".
    pub label: String,
}

/// How good the news is: the colour of the bubble's edge and pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warn,
    Danger,
}

const LOGO: f32 = 18.0;

/// Lays out the bubble and returns its size in DIPs. With a canvas it also draws.
fn render(c: Option<&Canvas>, m: &Model) -> Result<(f32, f32)> {
    let g = gfx::get()?;
    let pal: &Palette = theme::palette(m.dark);
    if let Some(b) = &m.badge {
        return render_badge(c, m, b, pal);
    }
    let head = g.text(&m.headline, 13.0, SEMIBOLD, Some(MAX_TEXT_W))?;
    let detail = (!m.detail.is_empty()).then(|| g.text(&m.detail, 12.0, NORMAL, Some(MAX_TEXT_W))).transpose()?;
    let text_x = PAD + 16.0;
    let text_w = head.width.max(detail.as_ref().map_or(0.0, |d| d.width));
    let h = PAD + head.height + detail.as_ref().map_or(0.0, |d| d.height + 2.0) + PAD;
    if let Some(c) = c {
        let color = if m.urgent { pal.crit } else { pal.status(m.status) };
        let cy = PAD + head.height / 2.0;
        c.glow(PAD + 4.0, cy, 9.0, color.with_alpha(0.35));
        c.fill_circle(PAD + 4.0, cy, 4.0, color);
        c.text(&head, text_x, PAD, pal.text);
        if let Some(d) = &detail {
            c.text(d, text_x, PAD + head.height + 2.0, pal.text_dim);
        }
    }
    Ok((text_x + text_w + PAD, h))
}

/// A merge or pull request: the logo, the number and title, then the state
/// as a pill in the tone's colour (which also runs down the left edge) and
/// the project.
fn render_badge(c: Option<&Canvas>, m: &Model, b: &Badge, pal: &Palette) -> Result<(f32, f32)> {
    let g = gfx::get()?;
    let tone = match b.tone {
        Tone::Success => pal.done,
        Tone::Warn => pal.warn,
        Tone::Danger => pal.crit,
    };
    let text_x = PAD + LOGO + 10.0;
    let head = g.text(&m.headline, 13.0, SEMIBOLD, Some(MAX_TEXT_W))?;
    let pill = g.text(&b.label, 11.5, SEMIBOLD, None)?;
    let (pill_w, pill_h) = (pill.width + 14.0, pill.height + 4.0);
    let detail = (!m.detail.is_empty())
        .then(|| g.text(&m.detail, 12.0, NORMAL, Some((MAX_TEXT_W - pill_w - 8.0).max(40.0))))
        .transpose()?;
    let line_w = pill_w + detail.as_ref().map_or(0.0, |d| d.width + 8.0);
    let w = text_x + head.width.max(line_w) + PAD;
    let line_y = PAD + head.height + 6.0;
    let h = line_y + pill_h + PAD;
    if let Some(c) = c {
        c.fill_rect(0.0, 0.0, 3.0, h, tone);
        crate::logo::draw(c, b.forge, PAD, PAD + (head.height - LOGO) / 2.0, LOGO, pal);
        c.text(&head, text_x, PAD, pal.text);
        c.fill_round_rect(text_x, line_y, pill_w, pill_h, pill_h / 2.0, tone.with_alpha(0.18));
        c.text(&pill, text_x + 7.0, line_y + 2.0, tone);
        if let Some(d) = &detail {
            c.text(d, text_x + pill_w + 8.0, line_y + (pill_h - d.height) / 2.0, pal.text_dim);
        }
    }
    Ok((w, h))
}

pub struct Bubble {
    pub hwnd: Option<HWND>,
    pub model: Option<Model>,
    scale: f32,
}

impl Bubble {
    pub const fn new() -> Bubble {
        Bubble { hwnd: None, model: None, scale: 1.0 }
    }

    pub fn create(&mut self) {
        // SAFETY: plain window creation with a registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
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

    /// Shows `model` next to `anchor` (the widget, at `dpi`) without taking focus.
    pub fn show(&mut self, model: Model, anchor: RECT, dpi: u32) {
        let Some(hwnd) = self.hwnd else { return };
        let Ok((w_dip, h_dip)) = render(None, &model) else { return };
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
            // Out of the taskbar's edge, whichever it is.
            let (x, y) = crate::taskbars::popup(&anchor, w, h, margin, false);
            self.model = Some(model);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    pub fn hide(&mut self) {
        if let (Some(hwnd), true) = (self.hwnd, self.visible()) {
            self.model = None;
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
