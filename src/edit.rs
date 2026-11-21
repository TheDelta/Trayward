//! Text fields in the settings window: Windows' own edit controls, placed over
//! boxes the window draws. They bring typing, the clipboard, undo, input
//! methods and screen readers; the window only says where they go.
//!
//! Enter and Escape in a field are posted to the window as [`SUBMIT`]
//! (wParam 1: save, 0: cancel); Tab goes to the next field.

use crate::theme::Color;
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE,
    FW_NORMAL, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SetBkColor, SetTextColor,
};
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_TAB};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, ES_AUTOHSCROLL, ES_NUMBER, ES_PASSWORD, GetNextDlgTabItem, GetWindowTextLengthW,
    GetWindowTextW, PostMessageW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW, WM_APP,
    WM_CHAR, WM_KEYDOWN, WM_SETFONT, WS_CHILD, WS_EX_LEFT, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

/// Posted to the window by a field: Enter (wParam 1) or Escape (wParam 0).
pub const SUBMIT: u32 = WM_APP + 0x51;

const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETLIMITTEXT: u32 = 0x00C5;
/// The subclass's id.
const SUBCLASS: usize = 0x7472_6177;

/// How a field behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Shown as dots.
    Password,
    /// Digits only.
    Number,
}

/// A field the window wants: which, where (x, y, width, height; pixels when
/// handed to [`Fields::sync`]), how, and its grey hint.
#[derive(Clone, Debug, PartialEq)]
pub struct Want<F> {
    pub id: F,
    pub rect: (f32, f32, f32, f32),
    pub kind: Kind,
    pub hint: String,
    /// The text a new field starts with.
    pub text: String,
}

/// The fields on screen, in the order they were wanted (the Tab order).
pub struct Fields<F> {
    list: Vec<(F, HWND)>,
    font: Option<(HFONT, i32)>,
}

thread_local! {
    /// The fields' colours: text and background, and the background's brush.
    static COLORS: RefCell<Option<(COLORREF, COLORREF, HBRUSH)>> = const { RefCell::new(None) };
}

fn gdi_color(c: Color) -> COLORREF {
    let v = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    COLORREF(v(c.r) | (v(c.g) << 8) | (v(c.b) << 16))
}

/// Sets the colours the fields draw with (the window's theme).
pub fn set_colors(text: Color, back: Color) {
    let (text, back) = (gdi_color(text), gdi_color(back));
    COLORS.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().is_some_and(|(t, b, _)| *t == text && *b == back) {
            return;
        }
        // SAFETY: a brush we own; the old one is no longer selected anywhere.
        unsafe {
            if let Some((_, _, old)) = c.take() {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
            *c = Some((text, back, CreateSolidBrush(back)));
        }
    });
}

/// WM_CTLCOLOREDIT: the fields' colours, for the device context it hands over.
pub fn ctl_color(hdc: HDC) -> Option<LRESULT> {
    COLORS.with(|c| {
        let (text, back, brush) = (*c.borrow())?;
        // SAFETY: the device context Windows passed with the message.
        unsafe {
            SetTextColor(hdc, text);
            SetBkColor(hdc, back);
        }
        Some(LRESULT(brush.0 as isize))
    })
}

impl<F: Copy + PartialEq> Fields<F> {
    pub const fn new() -> Fields<F> {
        Fields { list: Vec::new(), font: None }
    }

    /// The font for `height` pixels, made once per size.
    fn font(&mut self, height: i32) -> HFONT {
        if let Some((font, _)) = self.font.filter(|(_, h)| *h == height) {
            return font;
        }
        // SAFETY: a font we own; the old one is replaced in every field below before it's deleted.
        unsafe {
            let font = CreateFontW(
                -height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                FF_DONTCARE.0 as u32,
                w!("Segoe UI"),
            );
            for (_, hwnd) in &self.list {
                SendMessageW(*hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
            if let Some((old, _)) = self.font.replace((font, height)) {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
            font
        }
    }

    /// Makes the fields on screen the ones wanted: new ones created, the others
    /// moved (their text kept), the ones no longer wanted removed. Returns the
    /// first field when there were none before (to give it the focus).
    pub fn sync(&mut self, parent: HWND, wanted: &[Want<F>], font_px: i32, dark: bool) -> Option<HWND> {
        let was_empty = self.list.is_empty();
        // First, while `list` still has the fields: a font of another size replaces
        // the old one in each of them before the old one is deleted.
        let font = (!wanted.is_empty()).then(|| self.font(font_px));
        let keep: Vec<(F, HWND)> = std::mem::take(&mut self.list);
        for (id, hwnd) in &keep {
            if !wanted.iter().any(|w| w.id == *id) {
                // SAFETY: a child window we created.
                let _ = unsafe { DestroyWindow(*hwnd) };
            }
        }
        let font = font?;
        for want in wanted {
            let (x, y, w, h) = px(want.rect);
            let existing = keep.iter().find(|(id, _)| *id == want.id).map(|(_, h)| *h);
            let hwnd = match existing {
                // SAFETY: a child window we created.
                Some(hwnd) => unsafe {
                    let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
                    hwnd
                },
                None => match create(parent, want, font) {
                    Some(hwnd) => hwnd,
                    None => continue,
                },
            };
            // SAFETY: our child window; the theme name and hint are copied.
            unsafe {
                let theme = if dark { w!("DarkMode_CFD") } else { PCWSTR::null() };
                let _ = SetWindowTheme(hwnd, theme, PCWSTR::null());
                let hint = HSTRING::from(want.hint.as_str());
                SendMessageW(hwnd, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(hint.as_ptr() as isize)));
            }
            self.list.push((want.id, hwnd));
        }
        was_empty.then(|| self.list.first().map(|(_, h)| *h)).flatten()
    }

    /// Removes every field.
    pub fn clear(&mut self) {
        for (_, hwnd) in self.list.drain(..) {
            // SAFETY: a child window we created.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
    }

    /// What's typed into a field.
    pub fn text(&self, id: F) -> Option<String> {
        let (_, hwnd) = self.list.iter().find(|(f, _)| *f == id)?;
        // SAFETY: our child window; the buffer is as long as the text and its NUL.
        unsafe {
            let n = GetWindowTextLengthW(*hwnd).max(0) as usize;
            let mut buf = vec![0u16; n + 1];
            let got = GetWindowTextW(*hwnd, &mut buf).max(0) as usize;
            Some(String::from_utf16_lossy(buf.get(..got.min(n)).unwrap_or_default()))
        }
    }

    /// Replaces a field's text.
    pub fn set_text(&self, id: F, text: &str) {
        if let Some((_, hwnd)) = self.list.iter().find(|(f, _)| *f == id) {
            // SAFETY: our child window; the text is copied.
            let _ = unsafe { SetWindowTextW(*hwnd, &HSTRING::from(text)) };
        }
    }
}

fn px((x, y, w, h): (f32, f32, f32, f32)) -> (i32, i32, i32, i32) {
    (x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32)
}

/// Creates one field.
fn create(parent: HWND, want: &Want<impl Copy>, font: HFONT) -> Option<HWND> {
    let (x, y, w, h) = px(want.rect);
    let mut style = WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
    style |= match want.kind {
        Kind::Text => 0,
        Kind::Password => ES_PASSWORD as u32,
        Kind::Number => ES_NUMBER as u32,
    };
    // SAFETY: a child of our own window; the subclass lives as long as the field.
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LEFT,
            w!("EDIT"),
            &HSTRING::from(want.text.as_str()),
            windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
            x,
            y,
            w,
            h,
            Some(parent),
            None,
            None,
            None,
        )
        .ok()?;
        SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        SendMessageW(hwnd, EM_SETLIMITTEXT, Some(WPARAM(2048)), None);
        let _ = SetWindowSubclass(hwnd, Some(keys), SUBCLASS, 0);
        Some(hwnd)
    }
}

/// Puts the focus into a field.
pub fn focus(hwnd: HWND) {
    // SAFETY: our child window.
    let _ = unsafe { SetFocus(Some(hwnd)) };
}

/// The fields' keys: Enter saves, Escape cancels (both told to the window), Tab
/// moves to the next field (Shift+Tab: the one before).
unsafe extern "system" fn keys(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize) -> LRESULT {
    let key = wp.0 as u16;
    match msg {
        WM_KEYDOWN if key == VK_RETURN.0 || key == VK_ESCAPE.0 => {
            // SAFETY: the field's parent is our settings window.
            unsafe {
                if let Ok(parent) = windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) {
                    let _ = PostMessageW(Some(parent), SUBMIT, WPARAM(usize::from(key == VK_RETURN.0)), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN if key == VK_TAB.0 => {
            // SAFETY: plain queries and focus changes among our own windows.
            unsafe {
                let back = GetKeyState(i32::from(VK_SHIFT.0)) < 0;
                let parent = windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd);
                if let Ok(next) = parent.and_then(|parent| GetNextDlgTabItem(parent, Some(hwnd), back)) {
                    let _ = SetFocus(Some(next));
                }
            }
            LRESULT(0)
        }
        // No beep for the keys handled above.
        WM_CHAR if matches!(key, 0x0D | 0x1B | 0x09) => LRESULT(0),
        // SAFETY: default handling of the edit control.
        _ => unsafe { DefSubclassProc(hwnd, msg, wp, lp) },
    }
}
