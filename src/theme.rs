//! Colours for light and dark mode, following the Windows taskbar theme.
//! Status colours are the Fluent semantic colours Windows 11 itself uses.

use crate::sessions::Status;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

const fn rgb(hex: u32) -> Color {
    rgba(hex, 1.0)
}

const fn rgba(hex: u32, a: f32) -> Color {
    Color {
        r: ((hex >> 16) & 255) as f32 / 255.0,
        g: ((hex >> 8) & 255) as f32 / 255.0,
        b: (hex & 255) as f32 / 255.0,
        a,
    }
}

impl Color {
    pub fn with_alpha(self, a: f32) -> Color {
        Color { a: self.a * a, ..self }
    }
}

pub struct Palette {
    pub text: Color,
    pub text_dim: Color,
    pub track: Color,
    pub bar: Color,
    pub warn: Color,
    pub crit: Color,
    pub hover: Color,
    pub divider: Color,
    pub surface: Color,
    pub row_hover: Color,
    pub accent: Color,
    pub working: Color,
    pub needs_input: Color,
    pub done: Color,
    pub idle: Color,
    /// Gradient for the plan name.
    pub plan_from: Color,
    pub plan_to: Color,
    /// Gradient for the Ultra plan (gold, rose, violet, cyan), and its shine.
    pub ultra: [Color; 4],
    pub shine: Color,
    /// Codex's mark and accents.
    pub codex: Color,
    /// Copilot's mark and accents.
    pub copilot: Color,
    /// Settings window: background, cards, outlines, and text on the accent colour.
    pub window: Color,
    pub card: Color,
    pub card_hover: Color,
    pub stroke: Color,
    pub on_accent: Color,
}

pub const DARK: Palette = Palette {
    text: rgb(0xFFFFFF),
    text_dim: rgba(0xFFFFFF, 0.62),
    track: rgba(0xFFFFFF, 0.18),
    bar: rgba(0xFFFFFF, 0.90),
    warn: rgb(0xFCE100),
    crit: rgb(0xFF99A4),
    hover: rgba(0xFFFFFF, 0.07),
    divider: rgba(0xFFFFFF, 0.10),
    surface: rgb(0x2B2B2B),
    row_hover: rgba(0xFFFFFF, 0.06),
    accent: rgb(0x60CDFF),
    working: rgb(0x60CDFF),
    needs_input: rgb(0xFCE100),
    done: rgb(0x6CCB5F),
    idle: rgba(0xFFFFFF, 0.38),
    plan_from: rgb(0xF0A07E),
    plan_to: rgb(0xB79CFF),
    ultra: [rgb(0xF7C948), rgb(0xFF7EB3), rgb(0xA98BFF), rgb(0x6FD6FF)],
    shine: rgb(0xFFFFFF),
    codex: rgb(0x7FD1C1),
    copilot: rgb(0xB8A1F5),
    window: rgb(0x202020),
    card: rgb(0x2B2B2B),
    card_hover: rgb(0x323232),
    stroke: rgba(0xFFFFFF, 0.55),
    on_accent: rgb(0x000000),
};

pub const LIGHT: Palette = Palette {
    text: rgb(0x1A1A1A),
    text_dim: rgba(0x000000, 0.58),
    track: rgba(0x000000, 0.14),
    bar: rgba(0x1A1A1A, 0.85),
    warn: rgb(0x9D5D00),
    crit: rgb(0xC42B1C),
    hover: rgba(0xFFFFFF, 0.60),
    divider: rgba(0x000000, 0.08),
    surface: rgb(0xF9F9F9),
    row_hover: rgba(0x000000, 0.04),
    accent: rgb(0x005FB8),
    working: rgb(0x005FB8),
    needs_input: rgb(0x9D5D00),
    done: rgb(0x0F7B0F),
    idle: rgba(0x000000, 0.35),
    plan_from: rgb(0xC15F3C),
    plan_to: rgb(0x7048E8),
    ultra: [rgb(0xB7791F), rgb(0xC2387A), rgb(0x6D4AE0), rgb(0x1D86B8)],
    shine: rgb(0xFFF4D6),
    codex: rgb(0x0E7C6B),
    copilot: rgb(0x6F42C1),
    window: rgb(0xF3F3F3),
    card: rgb(0xFFFFFF),
    card_hover: rgb(0xF9F9F9),
    stroke: rgba(0x000000, 0.55),
    on_accent: rgb(0xFFFFFF),
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

impl Palette {
    /// Bar/number colour for a used percentage: normal, warning from 80 %, critical from 95 %.
    pub fn level(&self, used: f64) -> Color {
        if used >= 95.0 {
            self.crit
        } else if used >= 80.0 {
            self.warn
        } else {
            self.bar
        }
    }

    pub fn status(&self, s: Status) -> Color {
        match s {
            Status::Working => self.working,
            Status::NeedsInput => self.needs_input,
            Status::Done => self.done,
            Status::Idle => self.idle,
        }
    }
}

/// Whether the taskbar (the "system" part of the Windows theme) is dark.
pub fn system_dark() -> bool {
    let mut v = 1u32;
    let mut size = 4u32;
    // SAFETY: out-pointers to a u32 and its size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && v == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_come_from_hex() {
        assert_eq!(rgb(0xFF8000), Color { r: 1.0, g: 128.0 / 255.0, b: 0.0, a: 1.0 });
        assert_eq!(rgba(0x0000FF, 0.5), Color { r: 0.0, g: 0.0, b: 1.0, a: 0.5 });
        assert_eq!(rgba(0x000000, 0.5).with_alpha(0.5).a, 0.25);
    }

    #[test]
    fn levels_and_states_have_their_colours() {
        for dark in [true, false] {
            let pal = palette(dark);
            assert_eq!(pal.level(10.0), pal.bar);
            assert_eq!(pal.level(80.0), pal.warn);
            assert_eq!(pal.level(95.0), pal.crit);
            assert_eq!(pal.status(Status::Working), pal.working);
            assert_eq!(pal.status(Status::NeedsInput), pal.needs_input);
            assert_eq!(pal.status(Status::Done), pal.done);
            assert_eq!(pal.status(Status::Idle), pal.idle);
        }
        assert_eq!(palette(true).window, DARK.window);
        assert_eq!(palette(false).window, LIGHT.window);
    }

    /// The taskbar's theme is read, read only (either answer is fine).
    #[test]
    fn reads_the_taskbar_theme() {
        assert_eq!(system_dark(), system_dark());
    }
}
