use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct Ini {
    pub Sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn Parse(Text: &str) -> Ini {
        let mut Output = Ini::default();
        let mut Current = String::new();
        for RawLine in Text.lines() {
            let Line = RawLine.trim().trim_start_matches('\u{feff}');
            if Line.is_empty() || Line.starts_with(';') || Line.starts_with('#') {
                continue;
            }
            if Line.starts_with('[') && Line.ends_with(']') {
                Current = Line[1..Line.len() - 1].trim().to_string();
                if !Output.Sections.iter().any(|(Name, _)| *Name == Current) {
                    Output.Sections.push((Current.clone(), Vec::new()));
                }
                continue;
            }
            let Some((Key, Value)) = Line.split_once('=') else { continue };
            Output.Set(&Current, Key.trim(), Value.trim());
        }
        Output
    }

    pub fn Load(Item: &Path) -> Option<Ini> {
        fs::read_to_string(Item).ok().map(|Text| Ini::Parse(&Text))
    }

    pub fn Section(&self, Name: &str) -> Option<&[(String, String)]> {
        self.Sections.iter().find(|(Section, _)| Section.eq_ignore_ascii_case(Name)).map(|(_, Values)| Values.as_slice())
    }

    pub fn Get(&self, Section: &str, Key: &str) -> Option<&str> {
        self.Section(Section)?.iter().find(|(Name, _)| Name.eq_ignore_ascii_case(Key)).map(|(_, Value)| Value.as_str())
    }

    pub fn Set(&mut self, Section: &str, Key: &str, Value: &str) {
        let Index = match self.Sections.iter().position(|(Name, _)| Name.eq_ignore_ascii_case(Section)) {
            Some(Index) => Index,
            None => {
                self.Sections.push((Section.to_string(), Vec::new()));
                self.Sections.len() - 1
            }
        };
        let Values = &mut self.Sections[Index].1;
        match Values.iter_mut().find(|(Name, _)| Name.eq_ignore_ascii_case(Key)) {
            Some(Existing) => Existing.1 = Value.to_string(),
            None => Values.push((Key.to_string(), Value.to_string())),
        }
    }

    pub fn Push(&mut self, Section: &str, Key: &str, Value: &str) {
        match self.Sections.iter_mut().find(|(Name, _)| Name.eq_ignore_ascii_case(Section)) {
            Some((_, Values)) => Values.push((Key.to_string(), Value.to_string())),
            None => self.Sections.push((Section.to_string(), vec![(Key.to_string(), Value.to_string())])),
        }
    }

    pub fn ToText(&self) -> String {
        let mut Text = String::new();
        for (Name, Values) in &self.Sections {
            if !Text.is_empty() {
                Text.push('\n');
            }
            Text.push('[');
            Text.push_str(Name);
            Text.push_str("]\n");
            for (Key, Value) in Values {
                Text.push_str(Key);
                Text.push('=');
                Text.push_str(Value);
                Text.push('\n');
            }
        }
        Text
    }

    pub fn Save(&self, Item: &Path) -> io::Result<()> {
        WriteAtomically(Item, self.ToText().as_bytes())
    }

    pub fn Bool(&self, Section: &str, Key: &str, Default: bool) -> bool {
        match self.Get(Section, Key).map(|Value| Value.to_ascii_lowercase()) {
            Some(Value) if Value == "true" || Value == "1" || Value == "yes" => true,
            Some(Value) if Value == "false" || Value == "0" || Value == "no" => false,
            _ => Default,
        }
    }

    pub fn Number(&self, Section: &str, Key: &str, Default: f32) -> f32 {
        self.Get(Section, Key).and_then(|Value| Value.parse::<f32>().ok()).filter(|Value| Value.is_finite()).unwrap_or(Default)
    }
}

pub fn WriteAtomically(Item: &Path, Data: &[u8]) -> io::Result<()> {
    if let Some(Parent) = Item.parent() {
        fs::create_dir_all(Parent)?;
    }
    let Temporary = Item.with_extension("tmp");
    {
        let mut File = fs::File::create(&Temporary)?;
        File.write_all(Data)?;
        File.sync_all()?;
    }
    fs::rename(&Temporary, Item)
}

#[derive(Clone)]
pub struct AppPaths {
    pub Config: PathBuf,
    pub Data: PathBuf,
    pub Executable: PathBuf,
}

impl AppPaths {
    pub fn Discover() -> AppPaths {
        let ExecutablePath = std::env::current_exe().unwrap_or_default();
        let Executable = ExecutablePath.parent().map(Path::to_path_buf).unwrap_or_default();
        if Executable.join("Flux.portable").exists() {
            let Base = Executable.join("Data");
            return AppPaths { Config: Base.clone(), Data: Base, Executable };
        }
        let Roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| Executable.clone());
        let Local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| Roaming.clone());
        AppPaths { Config: Roaming.join("Flux"), Data: Local.join("Flux"), Executable }
    }

    pub fn SettingsFile(&self) -> PathBuf {
        self.Config.join("Settings.ini")
    }

    pub fn SessionFile(&self) -> PathBuf {
        self.Config.join("Session.ini")
    }

    pub fn KeybindingsFile(&self) -> PathBuf {
        self.Config.join("Keybindings.ini")
    }

    pub fn TrustFile(&self) -> PathBuf {
        self.Config.join("ModuleTrust.ini")
    }

    pub fn ThemesDirectory(&self) -> PathBuf {
        self.Config.join("Themes")
    }

    pub fn ModuleDirectories(&self) -> Vec<PathBuf> {
        vec![self.Executable.join("Modules"), self.Data.join("Modules")]
    }

    pub fn InstalledModulesDirectory(&self) -> PathBuf {
        self.Data.join("Modules")
    }

    pub fn MenuUsageFile(&self) -> PathBuf {
        self.Config.join("MenuUsage.ini")
    }

    pub fn FolderSizeCache(&self) -> PathBuf {
        self.Data.join("Cache").join("FolderSizes.bin")
    }

    pub fn CrashDirectory(&self) -> PathBuf {
        self.Data.join("Crashes")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Density {
    Compact,
    Normal,
    Comfortable,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Details,
    List,
    Grid,
    Thumbnails,
}

impl ViewMode {
    pub fn Name(self) -> &'static str {
        match self {
            ViewMode::Details => "Details",
            ViewMode::List => "List",
            ViewMode::Grid => "Grid",
            ViewMode::Thumbnails => "Thumbnails",
        }
    }

    pub fn Parse(Text: &str) -> Option<ViewMode> {
        [ViewMode::Details, ViewMode::List, ViewMode::Grid, ViewMode::Thumbnails].into_iter().find(|Mode| Mode.Name().eq_ignore_ascii_case(Text))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnKind {
    Name,
    Type,
    Size,
    Modified,
    Created,
    Attributes,
    Folder,
}

impl ColumnKind {
    pub const All: [ColumnKind; 7] =
        [ColumnKind::Name, ColumnKind::Folder, ColumnKind::Type, ColumnKind::Size, ColumnKind::Modified, ColumnKind::Created, ColumnKind::Attributes];

    pub fn Name(self) -> &'static str {
        match self {
            ColumnKind::Name => "Name",
            ColumnKind::Type => "Type",
            ColumnKind::Size => "Size",
            ColumnKind::Modified => "Modified",
            ColumnKind::Created => "Created",
            ColumnKind::Attributes => "Attributes",
            ColumnKind::Folder => "Folder",
        }
    }

    pub fn Parse(Text: &str) -> Option<ColumnKind> {
        ColumnKind::All.into_iter().find(|Kind| Kind.Name().eq_ignore_ascii_case(Text))
    }

    pub fn DefaultWidth(self) -> f32 {
        match self {
            ColumnKind::Name => 320.0,
            ColumnKind::Type => 130.0,
            ColumnKind::Size => 88.0,
            ColumnKind::Modified | ColumnKind::Created => 140.0,
            ColumnKind::Attributes => 80.0,
            ColumnKind::Folder => 240.0,
        }
    }

    pub fn RightAligned(self) -> bool {
        self == ColumnKind::Size
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ColumnSetting {
    pub Kind: ColumnKind,
    pub Width: f32,
    pub Visible: bool,
}

#[derive(Clone)]
pub struct Settings {
    pub Theme: String,
    pub Density: Density,
    pub FontFamily: String,
    pub ShowHidden: bool,
    pub ShowSystem: bool,
    pub FoldersFirst: bool,
    pub DualPane: bool,
    pub ShowSidebar: bool,
    pub ShowInspector: bool,
    pub ShowFolderTree: bool,
    pub SidebarWidth: f32,
    pub InspectorWidth: f32,
    pub PaneSplit: f32,
    pub SmoothScrolling: bool,
    pub Animations: bool,
    pub FavoritesCollapsed: bool,
    pub RecentCollapsed: bool,
    pub FoldersCollapsed: bool,
    pub DefaultView: ViewMode,
    pub Columns: Vec<ColumnSetting>,
    pub ThumbnailSize: f32,
    pub ThumbnailCacheMegabytes: u32,
    pub RestoreSession: bool,
    pub FolderSizes: bool,
    pub ShellActionsInline: bool,
    pub ReuseWindow: bool,
    pub CrashMonitor: bool,
    pub FullMemoryDump: bool,
    pub ShowPerformance: bool,
    pub TerminalCommand: String,
    pub AutoUpdate: bool,
    pub SkippedVersion: String,
    pub LastUpdateCheck: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            Theme: "Dark".to_string(),
            Density: Density::Normal,
            FontFamily: "Segoe UI".to_string(),
            ShowHidden: false,
            ShowSystem: false,
            FoldersFirst: true,
            DualPane: false,
            ShowSidebar: true,
            ShowInspector: false,
            ShowFolderTree: true,
            SidebarWidth: 220.0,
            InspectorWidth: 300.0,
            PaneSplit: 0.5,
            SmoothScrolling: true,
            Animations: true,
            FavoritesCollapsed: false,
            RecentCollapsed: false,
            FoldersCollapsed: false,
            DefaultView: ViewMode::Details,
            Columns: [ColumnKind::Name, ColumnKind::Folder, ColumnKind::Modified, ColumnKind::Type, ColumnKind::Size, ColumnKind::Created, ColumnKind::Attributes]
                .into_iter()
                .map(|Kind| ColumnSetting { Kind, Width: Kind.DefaultWidth(), Visible: !matches!(Kind, ColumnKind::Created | ColumnKind::Attributes) })
                .collect(),
            ThumbnailSize: 160.0,
            ThumbnailCacheMegabytes: 96,
            RestoreSession: true,
            FolderSizes: true,
            ShellActionsInline: true,
            ReuseWindow: true,
            CrashMonitor: true,
            FullMemoryDump: false,
            ShowPerformance: false,
            TerminalCommand: String::new(),
            AutoUpdate: true,
            SkippedVersion: String::new(),
            LastUpdateCheck: 0,
        }
    }
}

const SettingsGroup: &str = "Flux";

impl Settings {
    pub fn Load(Item: &Path) -> Settings {
        let mut Output = Settings::default();
        let Some(Data) = Ini::Load(Item) else { return Output };
        if let Some(Value) = Data.Get(SettingsGroup, "Theme") {
            Output.Theme = Value.to_string();
        }
        Output.Density = match Data.Get(SettingsGroup, "Density").unwrap_or("Normal") {
            "Compact" => Density::Compact,
            "Comfortable" => Density::Comfortable,
            _ => Density::Normal,
        };
        if let Some(Value) = Data.Get(SettingsGroup, "FontFamily").filter(|Value| !Value.is_empty()) {
            Output.FontFamily = Value.to_string();
        }
        Output.ShowHidden = Data.Bool(SettingsGroup, "ShowHidden", Output.ShowHidden);
        Output.ShowSystem = Data.Bool(SettingsGroup, "ShowSystem", Output.ShowSystem);
        Output.FoldersFirst = Data.Bool(SettingsGroup, "FoldersFirst", Output.FoldersFirst);
        Output.DualPane = Data.Bool(SettingsGroup, "DualPane", Output.DualPane);
        Output.ShowSidebar = Data.Bool(SettingsGroup, "ShowSidebar", Output.ShowSidebar);
        Output.ShowInspector = Data.Bool(SettingsGroup, "ShowInspector", Output.ShowInspector);
        Output.ShowFolderTree = Data.Bool(SettingsGroup, "ShowFolderTree", Output.ShowFolderTree);
        Output.SidebarWidth = Data.Number(SettingsGroup, "SidebarWidth", Output.SidebarWidth).clamp(140.0, 600.0);
        Output.InspectorWidth = Data.Number(SettingsGroup, "InspectorWidth", Output.InspectorWidth).clamp(180.0, 900.0);
        Output.PaneSplit = Data.Number(SettingsGroup, "PaneSplit", Output.PaneSplit).clamp(0.15, 0.85);
        Output.SmoothScrolling = Data.Bool(SettingsGroup, "SmoothScrolling", Output.SmoothScrolling);
        Output.Animations = Data.Bool(SettingsGroup, "Animations", Output.Animations);
        Output.FavoritesCollapsed = Data.Bool(SettingsGroup, "FavoritesCollapsed", Output.FavoritesCollapsed);
        Output.RecentCollapsed = Data.Bool(SettingsGroup, "RecentCollapsed", Output.RecentCollapsed);
        Output.FoldersCollapsed = Data.Bool(SettingsGroup, "FoldersCollapsed", Output.FoldersCollapsed);
        if let Some(Mode) = Data.Get(SettingsGroup, "DefaultView").and_then(ViewMode::Parse) {
            Output.DefaultView = Mode;
        }
        if let Some(Value) = Data.Get(SettingsGroup, "Columns") {
            let mut Columns = Vec::new();
            for Part in Value.split(',') {
                let Fields: Vec<&str> = Part.split(':').collect();
                let Some(Kind) = Fields.first().and_then(|Name| ColumnKind::Parse(Name.trim())) else { continue };
                if Columns.iter().any(|Existing: &ColumnSetting| Existing.Kind == Kind) {
                    continue;
                }
                let Width = Fields.get(1).and_then(|Text| Text.trim().parse::<f32>().ok()).unwrap_or(Kind.DefaultWidth()).clamp(40.0, 2000.0);
                let Visible = Kind == ColumnKind::Name || Fields.get(2).map(|Text| Text.trim() != "0").unwrap_or(true);
                Columns.push(ColumnSetting { Kind, Width, Visible });
            }
            for Kind in ColumnKind::All {
                if !Columns.iter().any(|Existing| Existing.Kind == Kind) {
                    Columns.push(ColumnSetting { Kind, Width: Kind.DefaultWidth(), Visible: false });
                }
            }
            if let Some(NamePosition) = Columns.iter().position(|Column| Column.Kind == ColumnKind::Name) {
                let NameColumn = Columns.remove(NamePosition);
                Columns.insert(0, NameColumn);
            }
            Output.Columns = Columns;
        }
        Output.ThumbnailSize = Data.Number(SettingsGroup, "ThumbnailSize", Output.ThumbnailSize).clamp(64.0, 320.0);
        Output.ThumbnailCacheMegabytes = Data.Number(SettingsGroup, "ThumbnailCacheMegabytes", Output.ThumbnailCacheMegabytes as f32).clamp(8.0, 2048.0) as u32;
        Output.RestoreSession = Data.Bool(SettingsGroup, "RestoreSession", Output.RestoreSession);
        Output.FolderSizes = Data.Bool(SettingsGroup, "FolderSizes", Output.FolderSizes);
        Output.ShellActionsInline = Data.Bool(SettingsGroup, "ShellActionsInline", Output.ShellActionsInline);
        Output.ReuseWindow = Data.Bool(SettingsGroup, "ReuseWindow", Output.ReuseWindow);
        Output.CrashMonitor = Data.Bool(SettingsGroup, "CrashMonitor", Output.CrashMonitor);
        Output.FullMemoryDump = Data.Bool(SettingsGroup, "FullMemoryDump", Output.FullMemoryDump);
        Output.ShowPerformance = Data.Bool(SettingsGroup, "ShowPerformance", Output.ShowPerformance);
        Output.AutoUpdate = Data.Bool(SettingsGroup, "AutoUpdate", Output.AutoUpdate);
        Output.SkippedVersion = Data.Get(SettingsGroup, "SkippedVersion").unwrap_or_default().to_string();
        Output.LastUpdateCheck = Data.Get(SettingsGroup, "LastUpdateCheck").and_then(|Value| Value.trim().parse().ok()).unwrap_or(0);
        if let Some(Value) = Data.Get(SettingsGroup, "TerminalCommand") {
            Output.TerminalCommand = Value.to_string();
        }
        Output
    }

    pub fn Save(&self, Item: &Path) -> io::Result<()> {
        let mut Data = Ini::Load(Item).unwrap_or_default();
        let Flag = |Value: bool| if Value { "true" } else { "false" };
        Data.Set(SettingsGroup, "Theme", &self.Theme);
        Data.Set(
            SettingsGroup,
            "Density",
            match self.Density {
                Density::Compact => "Compact",
                Density::Normal => "Normal",
                Density::Comfortable => "Comfortable",
            },
        );
        Data.Set(SettingsGroup, "FontFamily", &self.FontFamily);
        Data.Set(SettingsGroup, "ShowHidden", Flag(self.ShowHidden));
        Data.Set(SettingsGroup, "ShowSystem", Flag(self.ShowSystem));
        Data.Set(SettingsGroup, "FoldersFirst", Flag(self.FoldersFirst));
        Data.Set(SettingsGroup, "DualPane", Flag(self.DualPane));
        Data.Set(SettingsGroup, "ShowSidebar", Flag(self.ShowSidebar));
        Data.Set(SettingsGroup, "ShowInspector", Flag(self.ShowInspector));
        Data.Set(SettingsGroup, "ShowFolderTree", Flag(self.ShowFolderTree));
        Data.Set(SettingsGroup, "SidebarWidth", &format!("{:.0}", self.SidebarWidth));
        Data.Set(SettingsGroup, "InspectorWidth", &format!("{:.0}", self.InspectorWidth));
        Data.Set(SettingsGroup, "PaneSplit", &format!("{:.3}", self.PaneSplit));
        Data.Set(SettingsGroup, "SmoothScrolling", Flag(self.SmoothScrolling));
        Data.Set(SettingsGroup, "Animations", Flag(self.Animations));
        Data.Set(SettingsGroup, "FavoritesCollapsed", Flag(self.FavoritesCollapsed));
        Data.Set(SettingsGroup, "RecentCollapsed", Flag(self.RecentCollapsed));
        Data.Set(SettingsGroup, "FoldersCollapsed", Flag(self.FoldersCollapsed));
        Data.Set(SettingsGroup, "DefaultView", self.DefaultView.Name());
        let Columns: Vec<String> = self.Columns.iter().map(|Column| format!("{}:{:.0}:{}", Column.Kind.Name(), Column.Width, Column.Visible as u8)).collect();
        Data.Set(SettingsGroup, "Columns", &Columns.join(","));
        Data.Set(SettingsGroup, "ThumbnailSize", &format!("{:.0}", self.ThumbnailSize));
        Data.Set(SettingsGroup, "ThumbnailCacheMegabytes", &self.ThumbnailCacheMegabytes.to_string());
        Data.Set(SettingsGroup, "RestoreSession", Flag(self.RestoreSession));
        Data.Set(SettingsGroup, "FolderSizes", Flag(self.FolderSizes));
        Data.Set(SettingsGroup, "ShellActionsInline", Flag(self.ShellActionsInline));
        Data.Set(SettingsGroup, "ReuseWindow", Flag(self.ReuseWindow));
        Data.Set(SettingsGroup, "CrashMonitor", Flag(self.CrashMonitor));
        Data.Set(SettingsGroup, "FullMemoryDump", Flag(self.FullMemoryDump));
        Data.Set(SettingsGroup, "ShowPerformance", Flag(self.ShowPerformance));
        Data.Set(SettingsGroup, "TerminalCommand", &self.TerminalCommand);
        Data.Set(SettingsGroup, "AutoUpdate", Flag(self.AutoUpdate));
        Data.Set(SettingsGroup, "SkippedVersion", &self.SkippedVersion);
        Data.Set(SettingsGroup, "LastUpdateCheck", &self.LastUpdateCheck.to_string());
        Data.Save(Item)
    }
}
