use crate::Services::Config::{Density, Ini};
use std::path::Path;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color {
    pub R: f32,
    pub G: f32,
    pub B: f32,
    pub A: f32,
}

impl Color {
    pub const fn Hex(Value: u32) -> Color {
        Color { R: ((Value >> 16) & 0xFF) as f32 / 255.0, G: ((Value >> 8) & 0xFF) as f32 / 255.0, B: (Value & 0xFF) as f32 / 255.0, A: 1.0 }
    }

    pub const fn WithAlpha(self, A: f32) -> Color {
        Color { R: self.R, G: self.G, B: self.B, A }
    }

    pub fn Mix(self, Other: Color, Amount: f32) -> Color {
        Color {
            R: self.R + (Other.R - self.R) * Amount,
            G: self.G + (Other.G - self.G) * Amount,
            B: self.B + (Other.B - self.B) * Amount,
            A: self.A + (Other.A - self.A) * Amount,
        }
    }

    pub fn Parse(Text: &str) -> Option<Color> {
        let Digits = Text.trim().trim_start_matches('#');
        match Digits.len() {
            6 => u32::from_str_radix(Digits, 16).ok().map(Color::Hex),
            8 => {
                let Value = u32::from_str_radix(Digits, 16).ok()?;
                Some(Color::Hex(Value >> 8).WithAlpha((Value & 0xFF) as f32 / 255.0))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub Background: Color,
    pub Surface: Color,
    pub Sidebar: Color,
    pub Chrome: Color,
    pub Raised: Color,
    pub Border: Color,
    pub BorderStrong: Color,
    pub Text: Color,
    pub TextMuted: Color,
    pub TextFaint: Color,
    pub Accent: Color,
    pub AccentStrong: Color,
    pub AccentText: Color,
    pub Selection: Color,
    pub SelectionInactive: Color,
    pub Hover: Color,
    pub Pressed: Color,
    pub Input: Color,
    pub Danger: Color,
    pub Warning: Color,
    pub Success: Color,
    pub Scrim: Color,
}

impl Palette {
    pub fn Dark() -> Palette {
        Palette {
            Background: Color::Hex(0x15171A),
            Surface: Color::Hex(0x1B1E22),
            Sidebar: Color::Hex(0x181A1E),
            Chrome: Color::Hex(0x111316),
            Raised: Color::Hex(0x23272C),
            Border: Color::Hex(0x2A2F35),
            BorderStrong: Color::Hex(0x3B4149),
            Text: Color::Hex(0xE4E7EA),
            TextMuted: Color::Hex(0x9AA1AB),
            TextFaint: Color::Hex(0x666D77),
            Accent: Color::Hex(0x2EC4B6),
            AccentStrong: Color::Hex(0x5EDFD2),
            AccentText: Color::Hex(0x081311),
            Selection: Color::Hex(0x2EC4B6).WithAlpha(0.20),
            SelectionInactive: Color::Hex(0xFFFFFF).WithAlpha(0.075),
            Hover: Color::Hex(0xFFFFFF).WithAlpha(0.045),
            Pressed: Color::Hex(0xFFFFFF).WithAlpha(0.09),
            Input: Color::Hex(0x0F1113),
            Danger: Color::Hex(0xEF5F6B),
            Warning: Color::Hex(0xE6AE4A),
            Success: Color::Hex(0x5AC78A),
            Scrim: Color::Hex(0x000000).WithAlpha(0.45),
        }
    }

    pub fn Light() -> Palette {
        Palette {
            Background: Color::Hex(0xFBFBFA),
            Surface: Color::Hex(0xF3F3F1),
            Sidebar: Color::Hex(0xEEEEEB),
            Chrome: Color::Hex(0xE7E7E3),
            Raised: Color::Hex(0xFFFFFF),
            Border: Color::Hex(0xDCDCD6),
            BorderStrong: Color::Hex(0xC4C4BD),
            Text: Color::Hex(0x1B1D20),
            TextMuted: Color::Hex(0x5C626A),
            TextFaint: Color::Hex(0x8E949C),
            Accent: Color::Hex(0x0E9384),
            AccentStrong: Color::Hex(0x0A776B),
            AccentText: Color::Hex(0xFFFFFF),
            Selection: Color::Hex(0x0E9384).WithAlpha(0.16),
            SelectionInactive: Color::Hex(0x000000).WithAlpha(0.065),
            Hover: Color::Hex(0x000000).WithAlpha(0.04),
            Pressed: Color::Hex(0x000000).WithAlpha(0.08),
            Input: Color::Hex(0xFFFFFF),
            Danger: Color::Hex(0xC9363F),
            Warning: Color::Hex(0xB27500),
            Success: Color::Hex(0x23874F),
            Scrim: Color::Hex(0x000000).WithAlpha(0.25),
        }
    }

    fn Override(&mut self, Data: &Ini) {
        let Fields: [(&str, &mut Color); 22] = [
            ("Background", &mut self.Background),
            ("Surface", &mut self.Surface),
            ("Sidebar", &mut self.Sidebar),
            ("Chrome", &mut self.Chrome),
            ("Raised", &mut self.Raised),
            ("Border", &mut self.Border),
            ("BorderStrong", &mut self.BorderStrong),
            ("Text", &mut self.Text),
            ("TextMuted", &mut self.TextMuted),
            ("TextFaint", &mut self.TextFaint),
            ("Accent", &mut self.Accent),
            ("AccentStrong", &mut self.AccentStrong),
            ("AccentText", &mut self.AccentText),
            ("Selection", &mut self.Selection),
            ("SelectionInactive", &mut self.SelectionInactive),
            ("Hover", &mut self.Hover),
            ("Pressed", &mut self.Pressed),
            ("Input", &mut self.Input),
            ("Danger", &mut self.Danger),
            ("Warning", &mut self.Warning),
            ("Success", &mut self.Success),
            ("Scrim", &mut self.Scrim),
        ];
        let AccentChanged = Data.Get("Theme", "Accent").is_some() && Data.Get("Theme", "Selection").is_none();
        for (Name, Slot) in Fields {
            if let Some(Value) = Data.Get("Theme", Name).and_then(Color::Parse) {
                *Slot = Value;
            }
        }
        if AccentChanged {
            self.Selection = self.Accent.WithAlpha(self.Selection.A);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Metrics {
    pub FontSize: f32,
    pub SmallFontSize: f32,
    pub TitleHeight: f32,
    pub TabHeight: f32,
    pub ToolbarHeight: f32,
    pub HeaderHeight: f32,
    pub RowHeight: f32,
    pub StatusHeight: f32,
    pub SidebarRowHeight: f32,
    pub Radius: f32,
    pub ScrollbarWidth: f32,
    pub SplitterWidth: f32,
}

impl Metrics {
    pub fn For(Level: Density) -> Metrics {
        let (FontSize, RowHeight, Scale): (f32, f32, f32) = match Level {
            Density::Compact => (12.0, 20.0, 0.86),
            Density::Normal => (12.5, 24.0, 1.0),
            Density::Comfortable => (13.5, 30.0, 1.14),
        };
        Metrics {
            FontSize,
            SmallFontSize: FontSize - 1.0,
            TitleHeight: (34.0 * Scale).round(),
            TabHeight: (30.0 * Scale).round(),
            ToolbarHeight: (36.0 * Scale).round(),
            HeaderHeight: (26.0 * Scale).round(),
            RowHeight,
            StatusHeight: (24.0 * Scale).round(),
            SidebarRowHeight: (RowHeight + 2.0).round(),
            Radius: 3.0,
            ScrollbarWidth: 10.0,
            SplitterWidth: 5.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub IsDark: bool,
    pub Colors: Palette,
    pub Sizes: Metrics,
}

pub fn SystemPrefersDark() -> bool {
    let mut Value = 1u32;
    let mut Size = 4u32;
    let Result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut Value as *mut u32 as *mut _),
            Some(&mut Size),
        )
    };
    Result.is_ok() && Value == 0
}

impl Theme {
    pub fn Load(Name: &str, Level: Density, ThemesDirectory: &Path) -> Theme {
        let Sizes = Metrics::For(Level);
        let Builtin = |Dark: bool| Theme {
            IsDark: Dark,
            Colors: if Dark { Palette::Dark() } else { Palette::Light() },
            Sizes: Sizes.clone(),
        };
        if Name.eq_ignore_ascii_case("Light") {
            return Builtin(false);
        }
        if Name.eq_ignore_ascii_case("System") {
            return Builtin(SystemPrefersDark());
        }
        if Name.eq_ignore_ascii_case("Dark") || Name.is_empty() {
            return Builtin(true);
        }
        let Some(Data) = Ini::Load(&ThemesDirectory.join(format!("{Name}.ini"))) else { return Builtin(true) };
        let Dark = match Data.Get("Theme", "Base").unwrap_or("Dark") {
            Value if Value.eq_ignore_ascii_case("Light") => false,
            Value if Value.eq_ignore_ascii_case("System") => SystemPrefersDark(),
            _ => true,
        };
        let mut Output = Builtin(Dark);
        Output.Colors.Override(&Data);
        Output
    }

    pub fn Available(ThemesDirectory: &Path) -> Vec<String> {
        let mut Names = vec!["Dark".to_string(), "Light".to_string(), "System".to_string()];
        if let Ok(Reader) = std::fs::read_dir(ThemesDirectory) {
            for Item in Reader.flatten() {
                let ItemPath = Item.path();
                if ItemPath.extension().is_some_and(|Extension| Extension.eq_ignore_ascii_case("ini")) {
                    if let Some(Stem) = ItemPath.file_stem() {
                        Names.push(Stem.to_string_lossy().into_owned());
                    }
                }
            }
        }
        Names
    }
}
