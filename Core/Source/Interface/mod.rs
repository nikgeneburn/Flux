pub mod Chrome;
pub mod FileView;
pub mod Media;
pub mod Overlays;
pub mod Theme;
pub mod Widgets;

use crate::Control::Commands::Command;
use Theme::{Color, Metrics};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_IMMEDIATELY, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1LinearGradientBrush, ID2D1SolidColorBrush,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, HSTRING};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Rect {
    pub Left: f32,
    pub Top: f32,
    pub Right: f32,
    pub Bottom: f32,
}

impl Rect {
    pub fn New(X: f32, Y: f32, Width: f32, Height: f32) -> Rect {
        Rect { Left: X, Top: Y, Right: X + Width.max(0.0), Bottom: Y + Height.max(0.0) }
    }

    pub fn Width(&self) -> f32 {
        self.Right - self.Left
    }

    pub fn Height(&self) -> f32 {
        self.Bottom - self.Top
    }

    pub fn Contains(&self, X: f32, Y: f32) -> bool {
        X >= self.Left && X < self.Right && Y >= self.Top && Y < self.Bottom
    }

    pub fn Inset(&self, Horizontal: f32, Vertical: f32) -> Rect {
        Rect { Left: self.Left + Horizontal, Top: self.Top + Vertical, Right: (self.Right - Horizontal).max(self.Left + Horizontal), Bottom: (self.Bottom - Vertical).max(self.Top + Vertical) }
    }

    pub fn TakeTop(&mut self, Height: f32) -> Rect {
        let Taken = Rect { Left: self.Left, Top: self.Top, Right: self.Right, Bottom: (self.Top + Height).min(self.Bottom) };
        self.Top = Taken.Bottom;
        Taken
    }

    pub fn TakeBottom(&mut self, Height: f32) -> Rect {
        let Taken = Rect { Left: self.Left, Top: (self.Bottom - Height).max(self.Top), Right: self.Right, Bottom: self.Bottom };
        self.Bottom = Taken.Top;
        Taken
    }

    pub fn TakeLeft(&mut self, Width: f32) -> Rect {
        let Taken = Rect { Left: self.Left, Top: self.Top, Right: (self.Left + Width).min(self.Right), Bottom: self.Bottom };
        self.Left = Taken.Right;
        Taken
    }

    pub fn TakeRight(&mut self, Width: f32) -> Rect {
        let Taken = Rect { Left: (self.Right - Width).max(self.Left), Top: self.Top, Right: self.Right, Bottom: self.Bottom };
        self.Right = Taken.Left;
        Taken
    }

    pub fn Intersects(&self, Other: &Rect) -> bool {
        self.Left < Other.Right && Other.Left < self.Right && self.Top < Other.Bottom && Other.Top < self.Bottom
    }

    fn Native(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.Left, top: self.Top, right: self.Right, bottom: self.Bottom }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Splitter {
    Sidebar,
    Inspector,
    Panes,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Nothing,
    Caption,
    Minimize,
    Maximize,
    Close,
    Run(Command),
    Tab { Pane: u8, Index: u16 },
    TabClose { Pane: u8, Index: u16 },
    TabNew { Pane: u8 },
    TabStrip { Pane: u8 },
    Crumb { Pane: u8, Index: u16 },
    CrumbMenu { Pane: u8, Index: u16 },
    Address { Pane: u8 },
    Filter { Pane: u8 },
    FilterClear { Pane: u8 },
    Header { Pane: u8, Column: u8 },
    HeaderResize { Pane: u8, Column: u8 },
    List { Pane: u8 },
    ScrollThumb { Pane: u8 },
    ScrollTrack { Pane: u8 },
    Split(Splitter),
    SidebarRow(u32),
    SidebarToggle(u32),
    SidebarArea,
    Inspector,
    InspectorAction(u8),
    PaletteRow(u32),
    PaletteInput,
    PromptButton(u8),
    PromptInput,
    PromptToggle,
    ModuleAction { Row: u16, Action: u8 },
    OperationCancel(u64),
    StatusMessage,
    Overlay,
    MenuRow { Level: u8, Index: u16 },
    SettingsRow(u16),
    Completion { Pane: u8, Index: u16 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Arrow,
    Text,
    ResizeHorizontal,
}

#[derive(Default)]
pub struct HitList {
    Items: Vec<(Rect, Hit)>,
}

impl HitList {
    pub fn Clear(&mut self) {
        self.Items.clear();
    }

    pub fn Push(&mut self, Area: Rect, Target: Hit) {
        if Area.Width() > 0.0 && Area.Height() > 0.0 {
            self.Items.push((Area, Target));
        }
    }

    pub fn Find(&self, X: f32, Y: f32) -> (Hit, Rect) {
        self.Items.iter().rev().find(|(Area, _)| Area.Contains(X, Y)).map(|(Area, Target)| (*Target, *Area)).unwrap_or((Hit::Nothing, Rect::default()))
    }

    pub fn AreaOf(&self, Target: Hit) -> Option<Rect> {
        self.Items.iter().rev().find(|(_, Item)| *Item == Target).map(|(Area, _)| *Area)
    }
}

impl Hit {
    pub fn Cursor(self) -> CursorShape {
        match self {
            Hit::Address { .. } | Hit::Filter { .. } | Hit::PaletteInput | Hit::PromptInput => CursorShape::Text,
            Hit::HeaderResize { .. } | Hit::Split(_) => CursorShape::ResizeHorizontal,
            _ => CursorShape::Arrow,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Body,
    Strong,
    Small,
    Icon,
    IconSmall,
    Heading,
}

const StyleCount: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Renderer {
    Factory: ID2D1Factory,
    Write: IDWriteFactory,
    Target: Option<ID2D1HwndRenderTarget>,
    Brush: Option<ID2D1SolidColorBrush>,
    Gradient: Option<ID2D1LinearGradientBrush>,
    Window: HWND,
    Formats: Vec<IDWriteTextFormat>,
    WrapFormat: Option<IDWriteTextFormat>,
    MonoWrapFormat: Option<IDWriteTextFormat>,
    Scratch: Vec<u16>,
    pub Dpi: f32,
    pub Generation: u64,
    PixelWidth: u32,
    PixelHeight: u32,
}

fn NativeColor(Value: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: Value.R, g: Value.G, b: Value.B, a: Value.A }
}

impl Renderer {
    pub fn New(Window: HWND, Dpi: f32, FontFamily: &str, Sizes: &Metrics) -> windows::core::Result<Renderer> {
        let Factory: ID2D1Factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let Write: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let mut Output = Renderer {
            Factory,
            Write,
            Target: None,
            Brush: None,
            Gradient: None,
            Window,
            Formats: Vec::new(),
            WrapFormat: None,
            MonoWrapFormat: None,
            Scratch: Vec::with_capacity(512),
            Dpi,
            Generation: 1,
            PixelWidth: 0,
            PixelHeight: 0,
        };
        Output.RebuildFormats(FontFamily, Sizes)?;
        Ok(Output)
    }

    fn FontExists(&self, Family: &str) -> bool {
        let mut Collection: Option<IDWriteFontCollection> = None;
        unsafe {
            if self.Write.GetSystemFontCollection(&mut Collection, false).is_err() {
                return false;
            }
            let Some(Collection) = Collection else { return false };
            let (mut Index, mut Exists) = (0u32, BOOL(0));
            Collection.FindFamilyName(&HSTRING::from(Family), &mut Index, &mut Exists).is_ok() && Exists.as_bool()
        }
    }

    fn CreateFormat(&self, Family: &str, Size: f32, Weight: DWRITE_FONT_WEIGHT, Wrap: bool) -> windows::core::Result<IDWriteTextFormat> {
        unsafe {
            let Format = self.Write.CreateTextFormat(&HSTRING::from(Family), None, Weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, Size, &HSTRING::from("en-us"))?;
            if Wrap {
                Format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                Format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            } else {
                Format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let Sign = self.Write.CreateEllipsisTrimmingSign(&Format)?;
                let Trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                Format.SetTrimming(&Trimming, &Sign)?;
            }
            Ok(Format)
        }
    }

    pub fn RebuildFormats(&mut self, FontFamily: &str, Sizes: &Metrics) -> windows::core::Result<()> {
        let Family = if self.FontExists(FontFamily) { FontFamily.to_string() } else { "Segoe UI".to_string() };
        let IconFamily = if self.FontExists("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        let MonoFamily = if self.FontExists("Cascadia Mono") { "Cascadia Mono" } else { "Consolas" };
        let mut Formats = Vec::with_capacity(StyleCount);
        Formats.push(self.CreateFormat(&Family, Sizes.FontSize, DWRITE_FONT_WEIGHT_NORMAL, false)?);
        Formats.push(self.CreateFormat(&Family, Sizes.FontSize, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?);
        Formats.push(self.CreateFormat(&Family, Sizes.SmallFontSize, DWRITE_FONT_WEIGHT_NORMAL, false)?);
        Formats.push(self.CreateFormat(IconFamily, 12.0, DWRITE_FONT_WEIGHT_NORMAL, false)?);
        Formats.push(self.CreateFormat(IconFamily, 10.0, DWRITE_FONT_WEIGHT_NORMAL, false)?);
        Formats.push(self.CreateFormat(&Family, Sizes.FontSize + 2.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, false)?);
        self.Formats = Formats;
        self.WrapFormat = Some(self.CreateFormat(&Family, Sizes.FontSize, DWRITE_FONT_WEIGHT_NORMAL, true)?);
        self.MonoWrapFormat = Some(self.CreateFormat(MonoFamily, Sizes.SmallFontSize, DWRITE_FONT_WEIGHT_NORMAL, true)?);
        Ok(())
    }

    pub fn SetDpi(&mut self, Dpi: f32) {
        self.Dpi = Dpi;
        if let Some(Target) = &self.Target {
            unsafe { Target.SetDpi(Dpi, Dpi) };
        }
    }

    pub fn PixelSize(&self) -> (u32, u32) {
        (self.PixelWidth, self.PixelHeight)
    }

    pub fn Resize(&mut self, Width: u32, Height: u32) {
        self.PixelWidth = Width;
        self.PixelHeight = Height;
        if let Some(Target) = &self.Target {
            if unsafe { Target.Resize(&D2D_SIZE_U { width: Width, height: Height }) }.is_err() {
                self.DiscardTarget();
            }
        }
    }

    fn DiscardTarget(&mut self) {
        self.Target = None;
        self.Brush = None;
        self.Gradient = None;
        self.Generation += 1;
    }

    fn EnsureTarget(&mut self) -> bool {
        if self.Target.is_some() {
            return true;
        }
        let Properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: self.Dpi,
            dpiY: self.Dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let WindowProperties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.Window,
            pixelSize: D2D_SIZE_U { width: self.PixelWidth.max(1), height: self.PixelHeight.max(1) },
            presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
        };
        let Created = unsafe { self.Factory.CreateHwndRenderTarget(&Properties, &WindowProperties) };
        let Ok(Target) = Created else { return false };
        let Brush = unsafe { Target.CreateSolidColorBrush(&NativeColor(Color::Hex(0)), None) };
        let Ok(Brush) = Brush else { return false };
        let Stops = [
            windows::Win32::Graphics::Direct2D::Common::D2D1_GRADIENT_STOP { position: 0.0, color: NativeColor(Color::Hex(0x3D8BFD)) },
            windows::Win32::Graphics::Direct2D::Common::D2D1_GRADIENT_STOP { position: 0.55, color: NativeColor(Color::Hex(0x9B5CF6)) },
            windows::Win32::Graphics::Direct2D::Common::D2D1_GRADIENT_STOP { position: 1.0, color: NativeColor(Color::Hex(0xEF4444)) },
        ];
        self.Gradient = unsafe {
            Target.CreateGradientStopCollection(&Stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok().and_then(|Collection| {
                let Line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: windows_numerics::Vector2 { X: 0.0, Y: 0.0 }, endPoint: windows_numerics::Vector2 { X: 100.0, Y: 0.0 } };
                Target.CreateLinearGradientBrush(&Line, None, &Collection).ok()
            })
        };
        self.Target = Some(Target);
        self.Brush = Some(Brush);
        true
    }

    pub fn UsageBar(&self, Area: Rect, Fraction: f32, Track: Color) {
        let Radius = Area.Height() / 2.0;
        self.FillRounded(Area, Radius, Track);
        let Width = Area.Width() * Fraction.clamp(0.0, 1.0);
        if Width < 1.0 {
            return;
        }
        let (Some(Target), Some(Brush)) = (&self.Target, &self.Gradient) else { return };
        let Fill = D2D1_ROUNDED_RECT { rect: self.SnapRect(Rect { Right: Area.Left + Width.max(Area.Height()), ..Area }), radiusX: Radius, radiusY: Radius };
        unsafe {
            Brush.SetStartPoint(windows_numerics::Vector2 { X: Area.Left, Y: Area.Top });
            Brush.SetEndPoint(windows_numerics::Vector2 { X: Area.Right, Y: Area.Top });
            Target.FillRoundedRectangle(&Fill, Brush);
        }
    }

    pub fn Begin(&mut self, Background: Color) -> bool {
        if !self.EnsureTarget() {
            return false;
        }
        let Target = self.Target.as_ref().unwrap();
        unsafe {
            Target.BeginDraw();
            Target.Clear(Some(&NativeColor(Background)));
        }
        true
    }

    pub fn End(&mut self) -> bool {
        let Some(Target) = &self.Target else { return false };
        match unsafe { Target.EndDraw(None, None) } {
            Ok(()) => true,
            Err(Error) => {
                if Error.code() == D2DERR_RECREATE_TARGET {
                    self.DiscardTarget();
                }
                false
            }
        }
    }

    fn Snap(&self, Value: f32) -> f32 {
        let Scale = self.Dpi / 96.0;
        (Value * Scale).round() / Scale
    }

    fn SnapRect(&self, Area: Rect) -> D2D_RECT_F {
        D2D_RECT_F { left: self.Snap(Area.Left), top: self.Snap(Area.Top), right: self.Snap(Area.Right), bottom: self.Snap(Area.Bottom) }
    }

    fn Paint(&self, Value: Color) -> Option<(&ID2D1HwndRenderTarget, &ID2D1SolidColorBrush)> {
        let Target = self.Target.as_ref()?;
        let Brush = self.Brush.as_ref()?;
        unsafe { Brush.SetColor(&NativeColor(Value)) };
        Some((Target, Brush))
    }

    pub fn Fill(&self, Area: Rect, Value: Color) {
        if Value.A <= 0.0 || Area.Width() <= 0.0 || Area.Height() <= 0.0 {
            return;
        }
        let Native = self.SnapRect(Area);
        if let Some((Target, Brush)) = self.Paint(Value) {
            unsafe { Target.FillRectangle(&Native, Brush) };
        }
    }

    pub fn FillRounded(&self, Area: Rect, Radius: f32, Value: Color) {
        if Value.A <= 0.0 || Area.Width() <= 0.0 || Area.Height() <= 0.0 {
            return;
        }
        let Rounded = D2D1_ROUNDED_RECT { rect: self.SnapRect(Area), radiusX: Radius, radiusY: Radius };
        if let Some((Target, Brush)) = self.Paint(Value) {
            unsafe { Target.FillRoundedRectangle(&Rounded, Brush) };
        }
    }

    pub fn StrokeRounded(&self, Area: Rect, Radius: f32, Value: Color, Width: f32) {
        let Half = Width / 2.0;
        let Snapped = self.SnapRect(Area);
        let Rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: Snapped.left + Half, top: Snapped.top + Half, right: Snapped.right - Half, bottom: Snapped.bottom - Half },
            radiusX: Radius,
            radiusY: Radius,
        };
        if let Some((Target, Brush)) = self.Paint(Value) {
            unsafe { Target.DrawRoundedRectangle(&Rounded, Brush, Width, None) };
        }
    }

    pub fn Stroke(&self, Area: Rect, Value: Color) {
        let Line = 96.0 / self.Dpi;
        self.Fill(Rect { Bottom: Area.Top + Line, ..Area }, Value);
        self.Fill(Rect { Top: Area.Bottom - Line, ..Area }, Value);
        self.Fill(Rect { Right: Area.Left + Line, ..Area }, Value);
        self.Fill(Rect { Left: Area.Right - Line, ..Area }, Value);
    }

    pub fn HorizontalLine(&self, Left: f32, Right: f32, Y: f32, Value: Color) {
        let Line = (96.0 / self.Dpi).max(1.0 * 96.0 / self.Dpi);
        let Top = self.Snap(Y);
        self.Fill(Rect { Left, Top, Right, Bottom: Top + Line }, Value);
    }

    pub fn VerticalLine(&self, X: f32, Top: f32, Bottom: f32, Value: Color) {
        let Line = 96.0 / self.Dpi;
        let Left = self.Snap(X);
        self.Fill(Rect { Left, Top, Right: Left + Line, Bottom }, Value);
    }

    fn Format(&self, Kind: Style, Alignment: Align) -> Option<&IDWriteTextFormat> {
        let Format = self.Formats.get(Kind as usize)?;
        let Native: DWRITE_TEXT_ALIGNMENT = match Alignment {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        unsafe {
            let _ = Format.SetTextAlignment(Native);
        }
        Some(Format)
    }

    pub fn Text(&self, Text: &[u16], Kind: Style, Area: Rect, Value: Color, Alignment: Align) {
        if Text.is_empty() || Area.Width() <= 1.0 {
            return;
        }
        let Some(Format) = self.Format(Kind, Alignment) else { return };
        let Native = Area.Native();
        if let Some((Target, Brush)) = self.Paint(Value) {
            unsafe {
                Target.DrawText(Text, Format, &Native, Brush, D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, DWRITE_MEASURING_MODE_NATURAL)
            };
        }
    }

    pub fn Label(&mut self, Text: &str, Kind: Style, Area: Rect, Value: Color, Alignment: Align) {
        let mut Buffer = std::mem::take(&mut self.Scratch);
        Buffer.clear();
        Buffer.extend(Text.encode_utf16());
        self.Text(&Buffer, Kind, Area, Value, Alignment);
        self.Scratch = Buffer;
    }

    pub fn Glyph(&self, Code: u16, Kind: Style, Area: Rect, Value: Color) {
        self.Text(&[Code], Kind, Area, Value, Align::Center);
    }

    pub fn Wrapped(&self, Text: &[u16], Area: Rect, Value: Color, Monospace: bool) -> f32 {
        let Format = if Monospace { self.MonoWrapFormat.as_ref() } else { self.WrapFormat.as_ref() };
        let Some(Format) = Format else { return 0.0 };
        let Height = match self.Layout(Text, Format, Area.Width()) {
            Some(Layout) => {
                let mut Metrics = DWRITE_TEXT_METRICS::default();
                unsafe {
                    let _ = Layout.GetMetrics(&mut Metrics);
                }
                Metrics.height
            }
            None => 0.0,
        };
        if Text.is_empty() {
            return 0.0;
        }
        let Native = Area.Native();
        if let Some((Target, Brush)) = self.Paint(Value) {
            unsafe { Target.DrawText(Text, Format, &Native, Brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL) };
        }
        Height
    }

    pub fn WrappedHeight(&self, Text: &[u16], Width: f32, Monospace: bool) -> f32 {
        let Format = if Monospace { self.MonoWrapFormat.as_ref() } else { self.WrapFormat.as_ref() };
        let Some(Format) = Format else { return 0.0 };
        let Some(Layout) = self.Layout(Text, Format, Width) else { return 0.0 };
        let mut Metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = Layout.GetMetrics(&mut Metrics);
        }
        Metrics.height
    }

    fn Layout(&self, Text: &[u16], Format: &IDWriteTextFormat, Width: f32) -> Option<IDWriteTextLayout> {
        unsafe { self.Write.CreateTextLayout(Text, Format, Width.max(1.0), 100000.0) }.ok()
    }

    pub fn Measure(&self, Text: &[u16], Kind: Style) -> f32 {
        let Some(Format) = self.Format(Kind, Align::Left) else { return 0.0 };
        let Some(Layout) = self.Layout(Text, Format, 100000.0) else { return 0.0 };
        let mut Metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = Layout.GetMetrics(&mut Metrics);
        }
        Metrics.widthIncludingTrailingWhitespace
    }

    pub fn MeasureString(&mut self, Text: &str, Kind: Style) -> f32 {
        let mut Buffer = std::mem::take(&mut self.Scratch);
        Buffer.clear();
        Buffer.extend(Text.encode_utf16());
        let Width = self.Measure(&Buffer, Kind);
        self.Scratch = Buffer;
        Width
    }

    pub fn CaretOffsets(&self, Text: &[u16], Kind: Style) -> Vec<f32> {
        let mut Offsets = Vec::with_capacity(Text.len() + 1);
        let Some(Format) = self.Format(Kind, Align::Left) else { return vec![0.0; Text.len() + 1] };
        let Some(Layout) = self.Layout(Text, Format, 100000.0) else { return vec![0.0; Text.len() + 1] };
        for Position in 0..=Text.len() {
            let (mut X, mut Y) = (0.0f32, 0.0f32);
            let mut Metrics = DWRITE_HIT_TEST_METRICS::default();
            unsafe {
                let _ = Layout.HitTestTextPosition(Position as u32, false, &mut X, &mut Y, &mut Metrics);
            }
            Offsets.push(X);
        }
        Offsets
    }

    pub fn CreateBitmap(&self, Width: u32, Height: u32, Pixels: &[u8]) -> Option<ID2D1Bitmap> {
        let Target = self.Target.as_ref()?;
        if Width == 0 || Height == 0 || Pixels.len() < (Width * Height * 4) as usize {
            return None;
        }
        let Properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe { Target.CreateBitmap(D2D_SIZE_U { width: Width, height: Height }, Some(Pixels.as_ptr() as *const _), Width * 4, &Properties) }.ok()
    }

    pub fn Bitmap(&self, Image: &ID2D1Bitmap, Area: Rect, Opacity: f32) {
        let Some(Target) = &self.Target else { return };
        let Native = self.SnapRect(Area);
        unsafe { Target.DrawBitmap(Image, Some(&Native), Opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    pub fn PushClip(&self, Area: Rect) {
        if let Some(Target) = &self.Target {
            unsafe { Target.PushAxisAlignedClip(&self.SnapRect(Area), D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn PopClip(&self) {
        if let Some(Target) = &self.Target {
            unsafe { Target.PopAxisAlignedClip() };
        }
    }
}

pub fn Fit(Width: f32, Height: f32, Area: Rect) -> Rect {
    if Width <= 0.0 || Height <= 0.0 {
        return Area;
    }
    let Scale = (Area.Width() / Width).min(Area.Height() / Height).min(1.0);
    let (W, H) = (Width * Scale, Height * Scale);
    Rect::New(Area.Left + (Area.Width() - W) / 2.0, Area.Top + (Area.Height() - H) / 2.0, W, H)
}
