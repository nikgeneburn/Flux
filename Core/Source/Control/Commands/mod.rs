use crate::Services::Config::Ini;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Command {
    Back,
    Forward,
    Up,
    Refresh,
    FocusAddress,
    FocusFilter,
    GoTo,
    Palette,
    ViewMenu,
    NewTab,
    CloseTab,
    ReopenTab,
    NextTab,
    PreviousTab,
    DuplicateTab,
    ToggleDualPane,
    SwitchPane,
    SyncPanes,
    SwapPanes,
    ViewDetails,
    ViewList,
    ViewGrid,
    ViewThumbnails,
    ToggleHidden,
    ToggleSidebar,
    ToggleInspector,
    ToggleFolderTree,
    CycleTheme,
    CycleDensity,
    ToggleFoldersFirst,
    ToggleSmoothScrolling,
    ToggleAnimations,
    SortName,
    SortSize,
    SortModified,
    SortType,
    SortCreated,
    ZoomIn,
    ZoomOut,
    Open,
    OpenInNewTab,
    OpenInOtherPane,
    OpenWith,
    RunAsAdministrator,
    Properties,
    Rename,
    Delete,
    DeletePermanently,
    NewFolder,
    NewFile,
    Copy,
    Cut,
    Paste,
    CopyToOtherPane,
    MoveToOtherPane,
    CopyPath,
    CopyName,
    SelectAll,
    InvertSelection,
    ClearSelection,
    Undo,
    Redo,
    ContextMenu,
    OpenTerminal,
    CalculateSize,
    PinFavorite,
    UnpinFavorite,
    ManageModules,
    InstallModule,
    ReloadModules,
    OpenSettings,
    OpenKeybindings,
    OpenThemesFolder,
    ReloadConfiguration,
    TogglePerformance,
    OpenCrashFolder,
    TestCrash,
    Exit,
    Module(u32),
}

pub struct CommandInfo {
    pub Item: Command,
    pub Identifier: &'static str,
    pub Title: &'static str,
    pub Keys: &'static str,
}

const fn Info(Item: Command, Identifier: &'static str, Title: &'static str, Keys: &'static str) -> CommandInfo {
    CommandInfo { Item, Identifier, Title, Keys }
}

pub const Builtins: &[CommandInfo] = &[
    Info(Command::Back, "Navigate.Back", "Go Back", "Alt+Left"),
    Info(Command::Forward, "Navigate.Forward", "Go Forward", "Alt+Right"),
    Info(Command::Up, "Navigate.Up", "Go to Parent Folder", "Alt+Up Backspace"),
    Info(Command::Refresh, "Navigate.Refresh", "Refresh", "F5 Ctrl+R"),
    Info(Command::FocusAddress, "Navigate.FocusAddress", "Edit Address", "Ctrl+L Alt+D"),
    Info(Command::FocusFilter, "Navigate.FocusFilter", "Filter or Search", "Ctrl+F F3"),
    Info(Command::GoTo, "Navigate.GoTo", "Go to Folder", "Ctrl+P Ctrl+G"),
    Info(Command::Palette, "App.Palette", "Command Palette", "Ctrl+Shift+P Ctrl+K"),
    Info(Command::ViewMenu, "App.ViewMenu", "View Options…", "Alt+V"),
    Info(Command::NewTab, "Tab.New", "New Tab", "Ctrl+T"),
    Info(Command::CloseTab, "Tab.Close", "Close Tab", "Ctrl+W Ctrl+F4"),
    Info(Command::ReopenTab, "Tab.Reopen", "Reopen Closed Tab", "Ctrl+Shift+T"),
    Info(Command::NextTab, "Tab.Next", "Next Tab", "Ctrl+Tab Ctrl+PageDown"),
    Info(Command::PreviousTab, "Tab.Previous", "Previous Tab", "Ctrl+Shift+Tab Ctrl+PageUp"),
    Info(Command::DuplicateTab, "Tab.Duplicate", "Duplicate Tab", "Ctrl+Shift+D"),
    Info(Command::ToggleDualPane, "Pane.ToggleDual", "Toggle Dual Pane", "F9 Ctrl+D"),
    Info(Command::SwitchPane, "Pane.Switch", "Switch Pane", "Tab"),
    Info(Command::SyncPanes, "Pane.Sync", "Show Same Folder in Other Pane", "Ctrl+Shift+S"),
    Info(Command::SwapPanes, "Pane.Swap", "Swap Panes", "Ctrl+Shift+X"),
    Info(Command::ViewDetails, "View.Details", "View: Details", "Ctrl+1"),
    Info(Command::ViewList, "View.List", "View: List", "Ctrl+2"),
    Info(Command::ViewGrid, "View.Grid", "View: Grid", "Ctrl+3"),
    Info(Command::ViewThumbnails, "View.Thumbnails", "View: Thumbnails", "Ctrl+4"),
    Info(Command::ToggleHidden, "View.ToggleHidden", "Toggle Hidden Items", "Ctrl+H"),
    Info(Command::ToggleSidebar, "View.ToggleSidebar", "Toggle Sidebar", "Ctrl+B"),
    Info(Command::ToggleInspector, "View.ToggleInspector", "Toggle Inspector", "Ctrl+I Alt+P"),
    Info(Command::ToggleFolderTree, "View.ToggleFolderTree", "Toggle Folder Tree", "Ctrl+Shift+E"),
    Info(Command::CycleTheme, "View.CycleTheme", "Switch Theme", "Ctrl+Shift+L"),
    Info(Command::CycleDensity, "View.CycleDensity", "Switch Density", ""),
    Info(Command::ToggleFoldersFirst, "View.ToggleFoldersFirst", "Toggle Folders First", ""),
    Info(Command::ToggleSmoothScrolling, "View.ToggleSmoothScrolling", "Toggle Smooth Scrolling", ""),
    Info(Command::ToggleAnimations, "View.ToggleAnimations", "Toggle Animations", ""),
    Info(Command::SortName, "Sort.Name", "Sort by Name", "Ctrl+Shift+1"),
    Info(Command::SortSize, "Sort.Size", "Sort by Size", "Ctrl+Shift+2"),
    Info(Command::SortModified, "Sort.Modified", "Sort by Date Modified", "Ctrl+Shift+3"),
    Info(Command::SortType, "Sort.Type", "Sort by Type", "Ctrl+Shift+4"),
    Info(Command::SortCreated, "Sort.Created", "Sort by Date Created", "Ctrl+Shift+5"),
    Info(Command::ZoomIn, "View.ZoomIn", "Larger Items", "Ctrl+Plus"),
    Info(Command::ZoomOut, "View.ZoomOut", "Smaller Items", "Ctrl+Minus"),
    Info(Command::Open, "File.Open", "Open", "Enter"),
    Info(Command::OpenInNewTab, "File.OpenInNewTab", "Open in New Tab", "Ctrl+Enter"),
    Info(Command::OpenInOtherPane, "File.OpenInOtherPane", "Open in Other Pane", "Shift+Enter"),
    Info(Command::OpenWith, "File.OpenWith", "Open With…", ""),
    Info(Command::RunAsAdministrator, "File.RunAsAdministrator", "Run as Administrator", ""),
    Info(Command::Properties, "File.Properties", "Properties", "Alt+Enter"),
    Info(Command::Rename, "File.Rename", "Rename", "F2"),
    Info(Command::Delete, "File.Delete", "Move to Recycle Bin", "Delete"),
    Info(Command::DeletePermanently, "File.DeletePermanently", "Delete Permanently", "Shift+Delete"),
    Info(Command::NewFolder, "File.NewFolder", "New Folder", "Ctrl+Shift+N"),
    Info(Command::NewFile, "File.NewFile", "New Text File", "Ctrl+Alt+N"),
    Info(Command::Copy, "Edit.Copy", "Copy", "Ctrl+C Ctrl+Insert"),
    Info(Command::Cut, "Edit.Cut", "Cut", "Ctrl+X"),
    Info(Command::Paste, "Edit.Paste", "Paste", "Ctrl+V Shift+Insert"),
    Info(Command::CopyToOtherPane, "Edit.CopyToOtherPane", "Copy to Other Pane", "F7"),
    Info(Command::MoveToOtherPane, "Edit.MoveToOtherPane", "Move to Other Pane", "F8"),
    Info(Command::CopyPath, "Edit.CopyPath", "Copy Path", "Ctrl+Shift+C"),
    Info(Command::CopyName, "Edit.CopyName", "Copy Name", ""),
    Info(Command::SelectAll, "Edit.SelectAll", "Select All", "Ctrl+A"),
    Info(Command::InvertSelection, "Edit.InvertSelection", "Invert Selection", "Ctrl+Shift+I"),
    Info(Command::ClearSelection, "Edit.ClearSelection", "Clear Selection", "Escape"),
    Info(Command::Undo, "Edit.Undo", "Undo", "Ctrl+Z"),
    Info(Command::Redo, "Edit.Redo", "Redo", "Ctrl+Y Ctrl+Shift+Z"),
    Info(Command::ContextMenu, "File.ContextMenu", "Show Context Menu", "Shift+F10 Apps"),
    Info(Command::OpenTerminal, "Tools.OpenTerminal", "Open Terminal Here", "Ctrl+Backquote"),
    Info(Command::CalculateSize, "Tools.CalculateSize", "Calculate Folder Size", "Ctrl+Shift+Space"),
    Info(Command::PinFavorite, "Favorites.Pin", "Pin to Favorites", "Ctrl+Shift+F"),
    Info(Command::UnpinFavorite, "Favorites.Unpin", "Unpin from Favorites", ""),
    Info(Command::ManageModules, "Modules.Manage", "Manage Modules", "Ctrl+Shift+M"),
    Info(Command::InstallModule, "Modules.Install", "Install Module from Folder…", ""),
    Info(Command::ReloadModules, "Modules.Rescan", "Rescan Modules", ""),
    Info(Command::OpenSettings, "App.OpenSettings", "Settings", "Ctrl+Comma"),
    Info(Command::OpenKeybindings, "App.OpenKeybindings", "Open Keyboard Shortcuts File", ""),
    Info(Command::OpenThemesFolder, "App.OpenThemesFolder", "Open Themes Folder", ""),
    Info(Command::ReloadConfiguration, "App.ReloadConfiguration", "Reload Settings, Theme and Shortcuts", "Ctrl+Shift+R"),
    Info(Command::TogglePerformance, "Diagnostics.TogglePerformance", "Toggle Performance Overlay", "F12"),
    Info(Command::OpenCrashFolder, "Diagnostics.OpenCrashReports", "Open Crash Reports Folder", ""),
    Info(Command::TestCrash, "Diagnostics.TestCrash", "Test Crash Reporter (terminates Flux)", ""),
    Info(Command::Exit, "App.Exit", "Exit Flux", "Alt+F4"),
];

pub fn FromIdentifier(Identifier: &str) -> Option<Command> {
    Builtins.iter().find(|Entry| Entry.Identifier.eq_ignore_ascii_case(Identifier)).map(|Entry| Entry.Item)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub Key: u16,
    pub Control: bool,
    pub Shift: bool,
    pub Alt: bool,
}

const KeyNames: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Escape", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Apps", 0x5D),
    ("Plus", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
];

impl Chord {
    pub fn Parse(Text: &str) -> Option<Chord> {
        let mut Output = Chord { Key: 0, Control: false, Shift: false, Alt: false };
        for Part in Text.split('+').map(str::trim).filter(|Part| !Part.is_empty()) {
            match Part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Output.Control = true,
                "shift" => Output.Shift = true,
                "alt" => Output.Alt = true,
                Lower => {
                    Output.Key = if let Some((_, Code)) = KeyNames.iter().find(|(Name, _)| Name.eq_ignore_ascii_case(Lower)) {
                        *Code
                    } else if Lower.len() == 1 && Lower.as_bytes()[0].is_ascii_alphanumeric() {
                        Lower.as_bytes()[0].to_ascii_uppercase() as u16
                    } else if let Some(Number) = Lower.strip_prefix('f').and_then(|Digits| Digits.parse::<u16>().ok()).filter(|Number| (1..=24).contains(Number)) {
                        0x70 + Number - 1
                    } else {
                        return None;
                    }
                }
            }
        }
        if Output.Key == 0 { None } else { Some(Output) }
    }

    pub fn Text(&self) -> String {
        let mut Parts: Vec<String> = Vec::new();
        if self.Control {
            Parts.push("Ctrl".into());
        }
        if self.Shift {
            Parts.push("Shift".into());
        }
        if self.Alt {
            Parts.push("Alt".into());
        }
        let Name = if let Some((Name, _)) = KeyNames.iter().find(|(_, Code)| *Code == self.Key) {
            Name.to_string()
        } else if (0x70..0x88).contains(&self.Key) {
            format!("F{}", self.Key - 0x6F)
        } else {
            char::from_u32(self.Key as u32).map(String::from).unwrap_or_default()
        };
        Parts.push(Name);
        Parts.join("+")
    }
}

pub struct Keymap {
    Bindings: HashMap<Chord, Command>,
    Display: HashMap<Command, String>,
}

impl Keymap {
    pub fn Load(Item: &Path) -> Keymap {
        let mut Bindings = HashMap::new();
        for Entry in Builtins {
            for Text in Entry.Keys.split_whitespace() {
                if let Some(Parsed) = Chord::Parse(Text) {
                    Bindings.entry(Parsed).or_insert(Entry.Item);
                }
            }
        }
        match Ini::Load(Item) {
            Some(Data) => {
                for (Key, Value) in Data.Section("Keys").unwrap_or(&[]) {
                    let Some(Parsed) = Chord::Parse(Key) else { continue };
                    match FromIdentifier(Value.trim()) {
                        Some(Target) => {
                            Bindings.insert(Parsed, Target);
                        }
                        None if Value.trim().is_empty() || Value.trim().eq_ignore_ascii_case("None") => {
                            Bindings.remove(&Parsed);
                        }
                        None => {}
                    }
                }
            }
            None => {
                let _ = Keymap::WriteDefaults(Item);
            }
        }
        let mut Display: HashMap<Command, String> = HashMap::new();
        let mut Sorted: Vec<(&Chord, &Command)> = Bindings.iter().collect();
        Sorted.sort_by_key(|(Key, _)| (Key.Alt, Key.Shift, Key.Control, Key.Key));
        for (Key, Target) in Sorted {
            Display.entry(*Target).or_insert_with(|| Key.Text());
        }
        Keymap { Bindings, Display }
    }

    fn WriteDefaults(Item: &Path) -> std::io::Result<()> {
        let mut Data = Ini::default();
        for Entry in Builtins {
            for Text in Entry.Keys.split_whitespace() {
                if let Some(Parsed) = Chord::Parse(Text) {
                    Data.Push("Keys", &Parsed.Text(), Entry.Identifier);
                }
            }
        }
        Data.Save(Item)
    }

    pub fn Resolve(&self, Pressed: Chord) -> Option<Command> {
        self.Bindings.get(&Pressed).copied()
    }

    pub fn ShortcutText(&self, Item: Command) -> &str {
        self.Display.get(&Item).map(String::as_str).unwrap_or("")
    }
}
