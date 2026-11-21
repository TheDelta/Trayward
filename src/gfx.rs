//! Drawing with Direct2D and DirectWrite: anti-aliased shapes and text, in
//! device-independent pixels (DIPs). Both windows draw through a software
//! DC render target, so there is one code path and no GPU device to lose.

use crate::theme::Color;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_GAMMA_2_2, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE,
    D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DCRenderTarget,
    ID2D1Factory, ID2D1HwndRenderTarget, ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::SHDefExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, IsWindowVisible, ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{HSTRING, Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

pub use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_NORMAL as NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD as SEMIBOLD,
};

/// Factories and cached text formats. Lives on the UI thread.
pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    family: HSTRING,
    /// Windows' icon font.
    icon_family: HSTRING,
    formats: RefCell<HashMap<(u32, i32, bool), IDWriteTextFormat>>,
    wic: RefCell<Option<IWICImagingFactory>>,
    icons: RefCell<HashMap<PathBuf, Option<Icon>>>,
    /// Laid-out text by (text, size, weight, icon font, max width or wrap width):
    /// the same labels are drawn on every frame.
    layouts: RefCell<HashMap<TextKey, Text>>,
}

type TextKey = (String, u32, i32, bool, u32);

/// Layouts kept at most; the cache starts over beyond this.
const MAX_LAYOUTS: usize = 4000;

/// An app icon, ready to draw at any size.
#[derive(Clone)]
pub struct Icon(IWICBitmapSource);

thread_local! {
    static GFX: RefCell<Option<Rc<Gfx>>> = const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Texts that were cut off to fit (see `Gfx::text`), for tests.
    static CUT: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The texts cut off since the last call (tests only).
#[cfg(test)]
pub fn take_cut() -> Vec<String> {
    CUT.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// The UI thread's graphics factories, created on first use.
pub fn get() -> Result<Rc<Gfx>> {
    GFX.with(|cell| {
        if let Some(g) = cell.borrow().as_ref() {
            return Ok(g.clone());
        }
        let g = Rc::new(Gfx::new()?);
        *cell.borrow_mut() = Some(g.clone());
        Ok(g)
    })
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

impl Gfx {
    fn new() -> Result<Gfx> {
        // SAFETY: plain factory creation.
        let (d2d, dw) = unsafe {
            (
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)?,
            )
        };
        let family = Self::pick_family(&dw, &["Segoe UI Variable Text", "Segoe UI"]);
        let icon_family = Self::pick_family(&dw, &["Segoe Fluent Icons", "Segoe MDL2 Assets"]);
        Ok(Gfx {
            d2d,
            dw,
            family,
            icon_family,
            formats: RefCell::new(HashMap::new()),
            wic: RefCell::new(None),
            icons: RefCell::new(HashMap::new()),
            layouts: RefCell::new(HashMap::new()),
        })
    }

    /// Windows 11's UI font, or Segoe UI on systems without it.
    /// The first of `candidates` installed on this system, else the last one.
    fn pick_family(dw: &IDWriteFactory, candidates: &[&str]) -> HSTRING {
        // SAFETY: out-pointers are valid; COM objects are released on drop.
        unsafe {
            let mut fonts = None;
            if dw.GetSystemFontCollection(&mut fonts, false).is_ok() {
                if let Some(fonts) = fonts {
                    for name in candidates {
                        let name = HSTRING::from(*name);
                        let mut index = 0u32;
                        let mut exists = Default::default();
                        if fonts.FindFamilyName(&name, &mut index, &mut exists).is_ok() && exists.as_bool() {
                            return name;
                        }
                    }
                }
            }
        }
        HSTRING::from(*candidates.last().unwrap_or(&"Segoe UI"))
    }

    fn format(&self, size: f32, weight: DWRITE_FONT_WEIGHT, icons: bool) -> Result<IDWriteTextFormat> {
        let key = ((size * 100.0).round() as u32, weight.0, icons);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let family = if icons { &self.icon_family } else { &self.family };
        // SAFETY: valid strings; the format is a COM object we keep.
        let f = unsafe {
            let f = self.dw.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!(""),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f
        };
        self.formats.borrow_mut().insert(key, f.clone());
        Ok(f)
    }

    /// Lays out one line of text. With `max_width`, longer text ends in "…".
    pub fn text(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, max_width: Option<f32>) -> Result<Text> {
        // Tests check which texts didn't fit.
        #[cfg(test)]
        if let Some(max) = max_width {
            if self.text(s, size, weight, None)?.width > max + 0.5 {
                CUT.with(|c| c.borrow_mut().push(s.to_string()));
            }
        }
        let key = Self::key(s, size, weight, false, max_width.map_or(u32::MAX, |w| (w * 4.0) as u32));
        self.cached(key, || self.layout(s, self.format(size, weight, false)?, max_width))
    }

    fn key(s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, icons: bool, width: u32) -> TextKey {
        (s.to_string(), (size * 100.0).round() as u32, weight.0, icons, width)
    }

    fn cached(&self, key: TextKey, make: impl FnOnce() -> Result<Text>) -> Result<Text> {
        if let Some(t) = self.layouts.borrow().get(&key) {
            return Ok(t.clone());
        }
        let t = make()?;
        let mut layouts = self.layouts.borrow_mut();
        if layouts.len() >= MAX_LAYOUTS {
            layouts.clear();
        }
        layouts.insert(key, t.clone());
        Ok(t)
    }

    /// A symbol from Windows' icon font (Segoe Fluent Icons), e.g. U+E713 for settings.
    pub fn glyph(&self, ch: char, size: f32) -> Result<Text> {
        let key = Self::key(&ch.to_string(), size, NORMAL, true, u32::MAX);
        self.cached(key, || self.layout(&ch.to_string(), self.format(size, NORMAL, true)?, None))
    }

    /// Lays out text wrapped into lines of at most `width`.
    pub fn wrapped(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, width: f32) -> Result<Text> {
        // Wrapped and single-line layouts of the same text differ: mark the width.
        let key = Self::key(s, size, weight, false, (width * 4.0) as u32 | (1 << 31));
        self.cached(key, || self.wrap(s, size, weight, width))
    }

    fn wrap(&self, s: &str, size: f32, weight: DWRITE_FONT_WEIGHT, width: f32) -> Result<Text> {
        let format = self.format(size, weight, false)?;
        let utf16: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: valid buffers; COM objects released on drop.
        unsafe {
            let layout = self.dw.CreateTextLayout(&utf16, &format, width, 10_000.0)?;
            layout.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok(Text { layout, width: m.width.min(width), height: m.height })
        }
    }

    fn layout(&self, s: &str, format: IDWriteTextFormat, max_width: Option<f32>) -> Result<Text> {
        let utf16: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: valid buffers; COM objects released on drop.
        unsafe {
            let layout = self.dw.CreateTextLayout(&utf16, &format, max_width.unwrap_or(10_000.0), 1_000.0)?;
            if max_width.is_some() {
                let sign = self.dw.CreateEllipsisTrimmingSign(&format)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                layout.SetTrimming(&trimming, &sign)?;
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let width = match max_width {
                Some(max) => m.widthIncludingTrailingWhitespace.min(max),
                None => m.widthIncludingTrailingWhitespace,
            };
            Ok(Text { layout, width, height: m.height })
        }
    }
}

/// A laid-out line of text and its size in DIPs.
impl Gfx {
    /// The icon of an executable (e.g. VS Code), loaded once.
    pub fn exe_icon(&self, exe: &Path) -> Option<Icon> {
        if let Some(icon) = self.icons.borrow().get(exe) {
            return icon.clone();
        }
        let icon = self.load_icon(exe).ok();
        self.icons.borrow_mut().insert(exe.to_path_buf(), icon.clone());
        icon
    }

    fn load_icon(&self, exe: &Path) -> Result<Icon> {
        // SAFETY: COM calls on valid objects; the HICON is destroyed after copying.
        unsafe {
            let cached = self.wic.borrow().clone();
            let wic = match cached {
                Some(wic) => wic,
                None => {
                    let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
                    *self.wic.borrow_mut() = Some(f.clone());
                    f
                }
            };
            let mut hicon = HICON::default();
            SHDefExtractIconW(&HSTRING::from(exe.as_os_str()), 0, 0, Some(&mut hicon), None, 32).ok()?;
            let bitmap = wic.CreateBitmapFromHICON(hicon);
            let _ = DestroyIcon(hicon);
            let converter = wic.CreateFormatConverter()?;
            converter.Initialize(
                &bitmap?,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            Ok(Icon(converter.cast()?))
        }
    }
}

#[derive(Clone)]
pub struct Text {
    layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

/// A piece of a vector shape, in absolute coordinates (see `logo::parse`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    /// A cubic Bézier: two control points, then the end point.
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Drawing operations on a render target, in DIPs.
pub struct Canvas<'a> {
    rt: &'a ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    /// Opacity of everything drawn with the brush and of icons (see `set_opacity`).
    opacity: Cell<f32>,
}

impl<'a> Canvas<'a> {
    fn new(rt: &'a ID2D1RenderTarget) -> Result<Canvas<'a>> {
        // SAFETY: valid render target and colour.
        let brush = unsafe { rt.CreateSolidColorBrush(&d2d_color(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), None)? };
        Ok(Canvas { rt, brush, opacity: Cell::new(1.0) })
    }

    /// Fades what is drawn from now on (shapes, text, icons) to `a`; 1 is opaque.
    pub fn set_opacity(&self, a: f32) {
        self.opacity.set(a);
        // SAFETY: valid brush.
        unsafe { self.brush.SetOpacity(a) };
    }

    fn set(&self, c: Color) {
        // SAFETY: valid brush.
        unsafe { self.brush.SetColor(&d2d_color(c)) };
    }

    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, c: Color) {
        self.set(c);
        let r = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.FillRectangle(&r, &self.brush) };
    }

    pub fn fill_round_rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, c: Color) {
        self.set(c);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.FillRoundedRectangle(&r, &self.brush) };
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        self.set(c);
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.FillEllipse(&e, &self.brush) };
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        self.set(c);
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.DrawEllipse(&e, &self.brush, width, None) };
    }

    #[allow(clippy::too_many_arguments, reason = "like fill_round_rect, plus the line width")]
    pub fn stroke_round_rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32, c: Color) {
        self.set(c);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.DrawRoundedRectangle(&r, &self.brush, width, None) };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        self.set(c);
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, &self.brush, width, None) };
    }

    /// A small clock face with hands at about ten past twelve.
    pub fn clock(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        self.stroke_circle(cx, cy, radius, 1.1, c);
        self.line(cx, cy, cx, cy - radius * 0.55, 1.1, c);
        self.line(cx, cy, cx + radius * 0.45, cy + radius * 0.1, 1.1, c);
    }

    /// Draws `t` with its top-left corner at (x, y).
    pub fn text(&self, t: &Text, x: f32, y: f32, c: Color) {
        self.set(c);
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe {
            self.rt.DrawTextLayout(
                Vector2 { X: x, Y: y },
                &t.layout,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            )
        };
    }

    /// A soft glow: `c` fading out from the centre to `radius`.
    pub fn glow(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(c) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(c.with_alpha(0.0)) },
        ];
        let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
            center: Vector2 { X: cx, Y: cy },
            gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
            radiusX: radius,
            radiusY: radius,
        };
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        // SAFETY: drawing between BeginDraw and EndDraw; brushes are released on drop.
        unsafe {
            let Ok(collection) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return;
            };
            if let Ok(brush) = self.rt.CreateRadialGradientBrush(&props, None, &collection) {
                self.rt.FillEllipse(&e, &brush);
            }
        }
    }

    /// A path through `pts`. With `base`, closed down to that y for filling.
    fn path(&self, pts: &[(f32, f32)], base: Option<f32>) -> Option<ID2D1PathGeometry> {
        let (first, last) = (pts.first()?, pts.last()?);
        let v = |x: f32, y: f32| Vector2 { X: x, Y: y };
        let rest: Vec<Vector2> = pts.iter().skip(1).map(|&(x, y)| v(x, y)).collect();
        // SAFETY: the sink is closed before the geometry is used.
        unsafe {
            let geometry = self.rt.GetFactory().ok()?.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            match base {
                Some(b) => {
                    sink.BeginFigure(v(first.0, b), D2D1_FIGURE_BEGIN_FILLED);
                    sink.AddLines(&[v(first.0, first.1)]);
                    sink.AddLines(&rest);
                    sink.AddLines(&[v(last.0, b)]);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                }
                None => {
                    sink.BeginFigure(v(first.0, first.1), D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddLines(&rest);
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
            }
            sink.Close().ok()?;
            Some(geometry)
        }
    }

    /// Fills a shape made of `segs`, scaled by `scale` and moved to (x, y).
    pub fn fill_path(&self, segs: &[Seg], x: f32, y: f32, scale: f32, c: Color) {
        let v = |px: f32, py: f32| Vector2 { X: x + px * scale, Y: y + py * scale };
        // SAFETY: the sink is closed before the geometry is drawn, between BeginDraw and EndDraw.
        unsafe {
            let Some(geometry) = self.rt.GetFactory().ok().and_then(|f| f.CreatePathGeometry().ok()) else { return };
            let Ok(sink) = geometry.Open() else { return };
            let mut open = false;
            for s in segs {
                match *s {
                    Seg::Move(px, py) => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        }
                        sink.BeginFigure(v(px, py), D2D1_FIGURE_BEGIN_FILLED);
                        open = true;
                    }
                    Seg::Line(px, py) if open => sink.AddLine(v(px, py)),
                    Seg::Cubic(ax, ay, bx, by, px, py) if open => {
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v(ax, ay), point2: v(bx, by), point3: v(px, py) })
                    }
                    Seg::Close if open => {
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        open = false;
                    }
                    _ => {}
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            if sink.Close().is_err() {
                return;
            }
            self.set(c);
            self.rt.FillGeometry(&geometry, &self.brush, None);
        }
    }

    /// A line through `pts`.
    pub fn polyline(&self, pts: &[(f32, f32)], width: f32, c: Color) {
        if let Some(g) = self.path(pts, None) {
            self.set(c);
            // SAFETY: drawing between BeginDraw and EndDraw.
            unsafe { self.rt.DrawGeometry(&g, &self.brush, width, None) };
        }
    }

    /// The area under the line through `pts` down to `base`, filled with a
    /// vertical gradient from `top` (at `top_y`) to `bottom`.
    pub fn area(&self, pts: &[(f32, f32)], base: f32, top_y: f32, top: Color, bottom: Color) {
        let Some(g) = self.path(pts, Some(base)) else { return };
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(top) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(bottom) },
        ];
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: 0.0, Y: top_y },
            endPoint: Vector2 { X: 0.0, Y: base },
        };
        // SAFETY: drawing between BeginDraw and EndDraw; brushes are released on drop.
        unsafe {
            let Ok(collection) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return;
            };
            if let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &collection) {
                self.rt.FillGeometry(&g, &brush, None);
            }
        }
    }

    /// Draws `t` turned by `degrees` around its centre.
    pub fn text_rotated(&self, t: &Text, x: f32, y: f32, c: Color, degrees: f32) {
        let center = Vector2 { X: x + t.width / 2.0, Y: y + t.height / 2.0 };
        let mut current = Matrix3x2::identity();
        // SAFETY: drawing between BeginDraw and EndDraw; the transform is restored afterwards.
        unsafe {
            self.rt.GetTransform(&mut current);
            self.rt.SetTransform(&(Matrix3x2::rotation_around(degrees, center) * current));
        }
        self.text(t, x, y, c);
        // SAFETY: as above.
        unsafe { self.rt.SetTransform(&current) };
    }

    /// Moves everything drawn from now on by (dx, dy).
    pub fn set_offset(&self, dx: f32, dy: f32) {
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe { self.rt.SetTransform(&Matrix3x2::translation(dx, dy)) };
    }

    /// Limits drawing to a rectangle until [`Canvas::pop_clip`].
    pub fn push_clip(&self, x: f32, y: f32, w: f32, h: f32) {
        let r = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        // SAFETY: drawing between BeginDraw and EndDraw; popped by the caller.
        unsafe { self.rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn pop_clip(&self) {
        // SAFETY: matches a push_clip.
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    /// Draws an icon into a `size`×`size` square at (x, y).
    pub fn icon(&self, icon: &Icon, x: f32, y: f32, size: f32) {
        let dest = D2D_RECT_F { left: x, top: y, right: x + size, bottom: y + size };
        // SAFETY: drawing between BeginDraw and EndDraw.
        unsafe {
            if let Ok(bitmap) = self.rt.CreateBitmapFromWicBitmap(&icon.0, None) {
                self.rt.DrawBitmap(
                    &bitmap,
                    Some(&dest),
                    self.opacity.get(),
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }
        }
    }

    /// Draws `t` filled with a left-to-right gradient from `from` to `to`.
    pub fn text_gradient(&self, t: &Text, x: f32, y: f32, from: Color, to: Color) {
        self.text_stops(t, x, y, &[(0.0, from), (1.0, to)]);
    }

    /// Draws `t` filled with a left-to-right gradient through `stops`
    /// (position 0 to 1 across the text, colour).
    pub fn text_stops(&self, t: &Text, x: f32, y: f32, stops: &[(f32, Color)]) {
        let Some(&(_, from)) = stops.first() else { return };
        let stops: Vec<D2D1_GRADIENT_STOP> =
            stops.iter().map(|&(position, c)| D2D1_GRADIENT_STOP { position, color: d2d_color(c) }).collect();
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: x, Y: y },
            endPoint: Vector2 { X: x + t.width.max(1.0), Y: y },
        };
        // SAFETY: drawing between BeginDraw and EndDraw; brushes are released on drop.
        unsafe {
            let Ok(collection) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return self.text(t, x, y, from);
            };
            let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &collection) else {
                return self.text(t, x, y, from);
            };
            self.rt.DrawTextLayout(Vector2 { X: x, Y: y }, &t.layout, &brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        }
    }
}

fn dc_target(g: &Gfx, dpi: f32, alpha: D2D1_ALPHA_MODE) -> Result<ID2D1DCRenderTarget> {
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: alpha },
        dpiX: dpi,
        dpiY: dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    // SAFETY: valid properties struct.
    unsafe { g.d2d.CreateDCRenderTarget(&props) }
}

fn draw_on(
    rt: &ID2D1DCRenderTarget,
    hdc: HDC,
    w: i32,
    h: i32,
    clear: Color,
    aa: D2D1_TEXT_ANTIALIAS_MODE,
    draw: impl FnOnce(&Canvas),
) -> Result<()> {
    // SAFETY: the DC is valid for the duration of the draw.
    unsafe {
        rt.BindDC(hdc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
        rt.SetTextAntialiasMode(aa);
        rt.BeginDraw();
        rt.Clear(Some(&d2d_color(clear)));
        let canvas = Canvas::new(rt)?;
        draw(&canvas);
        rt.EndDraw(None, None)
    }
}

/// A memory DC with a 32-bit top-down DIB selected into it.
struct MemoryBitmap {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    /// The pixels (4 bytes each, rows top down), owned by `bmp`.
    #[cfg_attr(not(test), allow(dead_code, reason = "only the tests read the pixels back"))]
    bits: *mut std::ffi::c_void,
}

impl MemoryBitmap {
    fn new(w: i32, h: i32) -> Result<MemoryBitmap> {
        // SAFETY: GDI calls with valid arguments; cleanup in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e);
                }
            };
            let old = SelectObject(dc, bmp.into());
            Ok(MemoryBitmap { dc, bmp, old, bits })
        }
    }
}

impl Drop for MemoryBitmap {
    fn drop(&mut self) {
        // SAFETY: we created both objects and restore the DC before deleting.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// What a layered window is drawn into: its bitmap and the render target on
/// it, kept between frames (the widget is drawn up to 30 times a second).
struct Layer {
    rt: ID2D1DCRenderTarget,
    mem: MemoryBitmap,
    /// What it was made for: pixels and DPI.
    made_for: (i32, i32, u32),
}

impl Layer {
    fn new(g: &Gfx, w: i32, h: i32, dpi: f32) -> Result<Layer> {
        Ok(Layer {
            rt: dc_target(g, dpi, D2D1_ALPHA_MODE_PREMULTIPLIED)?,
            mem: MemoryBitmap::new(w, h)?,
            made_for: (w, h, dpi.to_bits()),
        })
    }

    /// Draws a frame: everything from before is cleared first.
    fn draw(&self, draw: impl FnOnce(&Canvas)) -> Result<()> {
        // Fully transparent pixels would let clicks fall through to the taskbar,
        // so the background is almost (but not quite) transparent.
        let clear = Color { r: 0.0, g: 0.0, b: 0.0, a: 2.0 / 255.0 };
        let (w, h, _) = self.made_for;
        draw_on(&self.rt, self.mem.dc, w, h, clear, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, draw)
    }
}

thread_local! {
    /// The layers of the layered windows (the widgets), by window.
    static LAYERS: RefCell<HashMap<isize, Layer>> = RefCell::new(HashMap::new());
}

/// Renders a `w`×`h` pixel frame with per-pixel alpha and shows it on the
/// layered window `hwnd`.
pub fn present_layered(hwnd: HWND, w: i32, h: i32, dpi: f32, draw: impl FnOnce(&Canvas)) -> Result<()> {
    let g = get()?;
    let key = hwnd.0 as isize;
    // Taken out while drawing: `draw` may come back here for another window.
    let kept = LAYERS.with(|l| l.borrow_mut().remove(&key)).filter(|l| l.made_for == (w, h, dpi.to_bits()));
    let layer = match kept {
        Some(layer) => layer,
        None => Layer::new(&g, w, h, dpi)?,
    };
    // A layer that failed to draw isn't kept: the next frame gets a new one.
    layer.draw(draw)?;
    let mem = &layer.mem;
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: the memory DC holds a w×h premultiplied bitmap.
    let shown = unsafe {
        UpdateLayeredWindow(
            hwnd,
            None,
            None,
            Some(&SIZE { cx: w, cy: h }),
            Some(mem.dc),
            Some(&POINT { x: 0, y: 0 }),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )
    };
    LAYERS.with(|l| l.borrow_mut().insert(key, layer));
    shown
}

thread_local! {
    /// Render targets of ordinary windows, kept between paints while they're visible.
    static TARGETS: RefCell<HashMap<isize, ID2D1HwndRenderTarget>> = RefCell::new(HashMap::new());
}

/// Paints an ordinary (not layered) window during WM_PAINT, `w`×`h` pixels,
/// through a render target kept for the window.
///
/// In software on purpose: a GPU render target loads the graphics driver
/// into the process (on NVIDIA about 70 MB more memory and 35 more threads),
/// far more than a few small windows need. Drawing them on the CPU takes a
/// few milliseconds.
pub fn paint_window(hwnd: HWND, w: i32, h: i32, dpi: f32, background: Color, draw: impl FnOnce(&Canvas)) -> Result<()> {
    let g = get()?;
    let size = D2D_SIZE_U { width: w.max(1) as u32, height: h.max(1) as u32 };
    let key = hwnd.0 as isize;
    let rt = match TARGETS.with(|t| t.borrow().get(&key).cloned()) {
        Some(rt) => rt,
        None => {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: dpi,
                dpiY: dpi,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let window =
                D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: size, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            // SAFETY: valid property structs for our own window.
            let rt = unsafe { g.d2d.CreateHwndRenderTarget(&props, &window)? };
            TARGETS.with(|t| t.borrow_mut().insert(key, rt.clone()));
            rt
        }
    };
    let canvas = || -> Result<()> {
        let base: ID2D1RenderTarget = rt.cast()?;
        draw(&Canvas::new(&base)?);
        Ok(())
    };
    // SAFETY: drawing on the window's own target between BeginDraw and EndDraw
    // (which always follows, whatever went wrong in between).
    let result = unsafe {
        let current = rt.GetPixelSize();
        let sized =
            if current.width != size.width || current.height != size.height { rt.Resize(&size) } else { Ok(()) };
        sized.and_then(|()| {
            rt.SetDpi(dpi, dpi);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            rt.BeginDraw();
            rt.Clear(Some(&d2d_color(background)));
            let drawn = canvas();
            rt.EndDraw(None, None).and(drawn)
        })
    };
    if result.is_err() {
        // The target is no use as it is (the device was lost: a driver update, sleep;
        // or it couldn't take the new size): start over next paint.
        TARGETS.with(|t| t.borrow_mut().remove(&key));
    }
    result
}

/// Releases the render targets of windows that are hidden or gone; a window
/// gets a new one when it's painted again. Called every second.
pub fn forget_hidden() {
    TARGETS.with(|t| {
        // SAFETY: plain queries on window handles, which may be stale.
        t.borrow_mut().retain(|&h, _| unsafe { IsWindowVisible(HWND(h as *mut _)).as_bool() });
    });
    LAYERS.with(|l| {
        // SAFETY: as above.
        l.borrow_mut().retain(|&h, _| unsafe { IsWindowVisible(HWND(h as *mut _)).as_bool() });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(layer: &Layer) -> Vec<u8> {
        let (w, h, _) = layer.made_for;
        // SAFETY: the bitmap is w×h pixels of 4 bytes, alive as long as the layer.
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::GdiFlush();
            std::slice::from_raw_parts(layer.mem.bits as *const u8, (w * h * 4) as usize).to_vec()
        }
    }

    /// The widget's layer is kept between frames: a frame drawn over an
    /// earlier one must come out exactly like one drawn on a new layer.
    #[test]
    fn a_kept_layer_draws_like_a_new_one() {
        let g = get().unwrap();
        let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let blue = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
        let frame = |c: &Canvas| {
            c.fill_rect(2.0, 2.0, 10.0, 6.0, red);
            c.fill_round_rect(20.0, 4.0, 30.0, 6.0, 3.0, blue.with_alpha(0.5));
            let text = get().unwrap().text("42%", 12.0, SEMIBOLD, None).unwrap();
            c.text(&text, 14.0, 8.0, red);
        };
        let fresh = Layer::new(&g, 64, 28, 96.0).unwrap();
        fresh.draw(frame).unwrap();
        let kept = Layer::new(&g, 64, 28, 96.0).unwrap();
        kept.draw(|c| c.fill_rect(0.0, 0.0, 64.0, 28.0, blue)).unwrap();
        assert_ne!(pixels(&kept), pixels(&fresh));
        kept.draw(frame).unwrap();
        assert_eq!(pixels(&kept), pixels(&fresh));
        // Something was drawn (a fully red, opaque pixel: B, G, R, A), and the
        // rest is the almost transparent background.
        let px = pixels(&fresh);
        assert!(px.chunks(4).any(|p| p == [0, 0, 255, 255]));
        assert_eq!(px[px.len() - 4..], [0, 0, 0, 2]);
    }
}
