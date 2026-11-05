pub mod Native;
pub mod Settings;

use super::Overlay;
use crate::Application::{App, Deferred, Focus};
use crate::Control::Commands::Command;
use crate::FileSystem::Sorting::SortKey;
use crate::FileSystem::{Location, Parent};
use crate::Interface::{Align, Hit, Rect, Style};
use crate::Platform::Shell::{self, Terminal};
use crate::Services::Config::{ColumnKind, ViewMode};
use crate::Services::Modules::Requirement;
use std::ffi::OsString;
use std::path::PathBuf;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP};

const GlyphCheck: u16 = 0xE73E;
const GlyphChevron: u16 = 0xE76C;

#[derive(Clone, Debug)]
pub enum MenuPurpose {
    Context { Pane: usize, Items: Vec<PathBuf>, Background: Option<PathBuf>, ScreenX: i32, ScreenY: i32 },
    Tab { Pane: usize, Index: usize },
    Crumb { Pane: usize, Base: PathBuf, Children: Vec<OsString> },
    Columns,
    Options { Pane: usize },
    Favorite { Index: usize },
    SidebarPath { Item: PathBuf },
}

#[derive(Clone, Debug)]
pub enum MenuAction {
    Run(Command),
    Terminal(Terminal),
    CurrentTerminal(Terminal),
    MoreOptions,
    CopyDirectory,
    ExplorerHere,
    RevealInExplorer,
    OpenContainingFolder,
    Sort(SortKey),
    Settings,
    Choice(u32),
    Shell { Token: u64, Command: u32, Usage: String },
}

#[derive(Clone, Debug)]
pub struct MenuEntry {
    pub Label: String,
    pub Shortcut: String,
    pub Glyph: u16,
    pub Checked: Option<bool>,
    pub Enabled: bool,
    pub Danger: bool,
    pub Action: Option<MenuAction>,
    pub Children: Vec<MenuEntry>,
    pub Image: Option<std::rc::Rc<Native::MenuImage>>,
}

impl MenuEntry {
    pub fn Item(Label: &str, Glyph: u16, Action: MenuAction) -> MenuEntry {
        MenuEntry { Label: Label.to_string(), Shortcut: String::new(), Glyph, Checked: None, Enabled: true, Danger: false, Action: Some(Action), Children: Vec::new(), Image: None }
    }

    pub fn Separator() -> MenuEntry {
        MenuEntry { Label: String::new(), Shortcut: String::new(), Glyph: 0, Checked: None, Enabled: false, Danger: false, Action: None, Children: Vec::new(), Image: None }
    }

    pub fn Group(Label: &str, Glyph: u16, Children: Vec<MenuEntry>) -> MenuEntry {
        MenuEntry { Label: Label.to_string(), Shortcut: String::new(), Glyph, Checked: None, Enabled: !Children.is_empty(), Danger: false, Action: None, Children, Image: None }
    }

    pub fn Keys(mut self, App: &App, Item: Command) -> MenuEntry {
        self.Shortcut = App.Keys.ShortcutText(Item).to_string();
        self
    }

    pub fn Check(mut self, Value: bool) -> MenuEntry {
        self.Checked = Some(Value);
        self
    }

    pub fn Enable(mut self, Value: bool) -> MenuEntry {
        self.Enabled = Value;
        self
    }

    pub fn Dangerous(mut self) -> MenuEntry {
        self.Danger = true;
        self
    }

    fn IsSeparator(&self) -> bool {
        self.Label.is_empty()
    }
}

fn Run(App: &App, Label: &str, Glyph: u16, Item: Command) -> MenuEntry {
    MenuEntry::Item(Label, Glyph, MenuAction::Run(Item)).Keys(App, Item)
}

pub struct MenuState {
    pub Purpose: MenuPurpose,
    pub Entries: Vec<MenuEntry>,
    pub X: f32,
    pub Y: f32,
    pub Open: Vec<usize>,
    pub Selected: Vec<Option<usize>>,
    pub Token: u64,
}

impl MenuState {
    fn Level(&self, Level: usize) -> &[MenuEntry] {
        let mut Entries: &[MenuEntry] = &self.Entries;
        for &Index in self.Open.iter().take(Level) {
            match Entries.get(Index) {
                Some(Entry) => Entries = &Entry.Children,
                None => return &[],
            }
        }
        Entries
    }

    fn OpenAt(&mut self, Level: usize, Index: usize, SelectFirst: bool) {
        self.Open.truncate(Level);
        self.Selected.truncate(Level + 1);
        self.Selected.resize(Level + 1, None);
        self.Open.push(Index);
        let First = if SelectFirst { Step(self.Level(Level + 1), None, true) } else { None };
        self.Selected.push(First);
    }

    fn CloseFrom(&mut self, Level: usize) {
        self.Open.truncate(Level);
        self.Selected.truncate(Level + 1);
        self.Selected.resize(Level + 1, None);
    }
}

impl App {
    pub fn OpenMenu(&mut self, Purpose: MenuPurpose, Entries: Vec<MenuEntry>, X: f32, Y: f32) {
        if Entries.is_empty() {
            return;
        }
        self.Overlay = Some(Overlay::Menu(MenuState { Purpose, Entries, X, Y, Open: Vec::new(), Selected: vec![None], Token: 0 }));
        self.Focus = Focus::Overlay;
        self.Invalidate();
    }

    pub fn ShowContextMenu(&mut self, Pane: usize, X: f32, Y: f32, OnItems: bool) {
        let Scale = self.Scale();
        let (ScreenX, ScreenY) = crate::Platform::ScreenPoint(self.Window, (X * Scale) as i32, (Y * Scale) as i32);
        let Tab = self.Tab(Pane);
        let Directory = self.CurrentDirectory(Pane);
        let InFolder = matches!(Tab.Location, Location::Directory(_));
        let Dual = self.Settings.DualPane;
        let Terminals = Shell::AvailableTerminals();
        let mut Entries = Vec::new();
        if OnItems {
            let Targets = Tab.Targets();
            if Targets.is_empty() {
                return;
            }
            let Items: Vec<PathBuf> = Targets.iter().map(|&Index| Tab.Listing.FullPath(Index)).collect();
            let AllFolders = Targets.iter().all(|&Index| Tab.Listing.IsDirectory(Index));
            let Single = Items.len() == 1;
            let IsExecutable = Single && Items[0].extension().is_some_and(|Extension| ["exe", "bat", "cmd", "msi", "com"].iter().any(|Candidate| Extension.eq_ignore_ascii_case(Candidate)));
            let FromElsewhere = !InFolder;
            Entries.push(Run(self, "Open", 0xE8E5, Command::Open));
            if AllFolders {
                Entries.push(Run(self, "Open in new tab", 0xE8A7, Command::OpenInNewTab));
                Entries.push(Run(self, "Open in other pane", 0xE8A9, Command::OpenInOtherPane));
            } else if Single {
                Entries.push(MenuEntry::Item("Open with…", 0xE7AC, MenuAction::Run(Command::OpenWith)));
            }
            if IsExecutable {
                Entries.push(MenuEntry::Item("Run as administrator", 0xE7EF, MenuAction::Run(Command::RunAsAdministrator)));
            }
            if FromElsewhere && Single {
                Entries.push(MenuEntry::Item("Open containing folder", 0xE838, MenuAction::OpenContainingFolder));
            }
            Entries.push(MenuEntry::Separator());
            Entries.push(Run(self, "Cut", 0xE8C6, Command::Cut));
            Entries.push(Run(self, "Copy", 0xE8C8, Command::Copy));
            Entries.push(Run(self, "Copy path", 0xE71B, Command::CopyPath));
            Entries.push(Run(self, "Copy name", 0xE8AC, Command::CopyName));
            if Dual {
                Entries.push(MenuEntry::Group(
                    "Other pane",
                    0xE8A9,
                    vec![Run(self, "Copy to other pane", 0xE8C8, Command::CopyToOtherPane), Run(self, "Move to other pane", 0xE8DE, Command::MoveToOtherPane)],
                ));
            }
            Entries.push(MenuEntry::Separator());
            Entries.push(Run(self, "Rename", 0xE8AC, Command::Rename).Enable(Single));
            Entries.push(Run(self, "Delete", 0xE74D, Command::Delete));
            Entries.push(Run(self, "Delete permanently", 0xE74D, Command::DeletePermanently).Dangerous());
            Entries.push(MenuEntry::Separator());
            if Single && AllFolders {
                let Pinned = self.Session.IsFavorite(&Items[0]);
                Entries.push(if Pinned { Run(self, "Unpin from Favorites", 0xE735, Command::UnpinFavorite) } else { Run(self, "Pin to Favorites", 0xE734, Command::PinFavorite) });
                Entries.push(Run(self, "Calculate size", 0xE9D9, Command::CalculateSize));
                if !Terminals.is_empty() {
                    Entries.push(MenuEntry::Group("Open in terminal", 0xE756, Terminals.iter().map(|Kind| MenuEntry::Item(Kind.Label(), 0xE756, MenuAction::Terminal(*Kind))).collect()));
                }
            }
            if Single {
                Entries.push(MenuEntry::Item("Show in File Explorer", 0xEC50, MenuAction::RevealInExplorer));
            }
            let Modules = self.ModuleEntries(true);
            if !Modules.is_empty() {
                Entries.push(MenuEntry::Group("Modules", 0xE74C, Modules));
            }
            if Directory.is_some() {
                let mut Folder = vec![MenuEntry::Item("Copy current folder path", 0xE71B, MenuAction::CopyDirectory)];
                Folder.extend(Terminals.iter().map(|Kind| MenuEntry::Item(&format!("Open in {}", Kind.Label()), 0xE756, MenuAction::CurrentTerminal(*Kind))));
                Folder.push(MenuEntry::Item("Open in File Explorer", 0xEC50, MenuAction::ExplorerHere));
                Folder.push(MenuEntry::Separator());
                Folder.push(Run(self, "New folder", 0xE8F4, Command::NewFolder).Enable(InFolder));
                Folder.push(Run(self, "Paste here", 0xE77F, Command::Paste).Enable(InFolder && crate::Platform::Clipboard::HasFiles()));
                Folder.push(Run(self, "Refresh", 0xE72C, Command::Refresh));
                Entries.push(MenuEntry::Group("Current folder", 0xE838, Folder));
            }
            Entries.push(MenuEntry::Separator());
            Entries.push(Run(self, "Properties", 0xE946, Command::Properties));
            Entries.push(MenuEntry::Item("Show more options", 0xE712, MenuAction::MoreOptions).Keys(self, Command::ContextMenu));
            self.OpenMenu(MenuPurpose::Context { Pane, Items: Items.clone(), Background: None, ScreenX, ScreenY }, Entries, X, Y);
            self.RequestShellEntries(crate::Platform::Menus::Extensions::ShellTarget::Items(Items));
            return;
        }
        let Mode = Tab.View;
        let Sort = Tab.Sort;
        if InFolder {
            Entries.push(Run(self, "New folder", 0xE8F4, Command::NewFolder));
            Entries.push(Run(self, "New text document", 0xE8A5, Command::NewFile));
            Entries.push(MenuEntry::Separator());
            Entries.push(Run(self, "Paste", 0xE77F, Command::Paste).Enable(crate::Platform::Clipboard::HasFiles()));
            Entries.push(Run(self, "Undo", 0xE7A7, Command::Undo));
            Entries.push(Run(self, "Redo", 0xE7A6, Command::Redo));
            Entries.push(MenuEntry::Separator());
        }
        if Directory.is_some() {
            Entries.push(MenuEntry::Item("Copy current folder path", 0xE71B, MenuAction::CopyDirectory));
            for Kind in &Terminals {
                Entries.push(MenuEntry::Item(&format!("Open in {}", Kind.Label()), 0xE756, MenuAction::Terminal(*Kind)));
            }
            Entries.push(MenuEntry::Item("Open in File Explorer", 0xEC50, MenuAction::ExplorerHere));
            Entries.push(MenuEntry::Separator());
        }
        Entries.push(MenuEntry::Group(
            "View",
            0xE890,
            vec![
                Run(self, "Details", 0xE8FD, Command::ViewDetails).Check(Mode == ViewMode::Details),
                Run(self, "List", 0xE8FD, Command::ViewList).Check(Mode == ViewMode::List),
                Run(self, "Grid", 0xF0E2, Command::ViewGrid).Check(Mode == ViewMode::Grid),
                Run(self, "Thumbnails", 0xE8B9, Command::ViewThumbnails).Check(Mode == ViewMode::Thumbnails),
            ],
        ));
        let SortEntry = |Label: &str, Key: SortKey| MenuEntry::Item(Label, 0, MenuAction::Sort(Key)).Check(Sort.Key == Key);
        Entries.push(MenuEntry::Group(
            "Sort by",
            0xE8CB,
            vec![
                SortEntry("Name", SortKey::Name),
                SortEntry("Date modified", SortKey::Modified),
                SortEntry("Type", SortKey::Type),
                SortEntry("Size", SortKey::Size),
                SortEntry("Date created", SortKey::Created),
                MenuEntry::Separator(),
                Run(self, "Folders first", 0, Command::ToggleFoldersFirst).Check(self.Settings.FoldersFirst),
            ],
        ));
        Entries.push(Run(self, "Show hidden items", 0, Command::ToggleHidden).Check(self.Settings.ShowHidden));
        Entries.push(Run(self, "Select all", 0xE8B3, Command::SelectAll));
        Entries.push(Run(self, "Refresh", 0xE72C, Command::Refresh));
        if let Some(Folder) = Directory.as_ref().filter(|_| InFolder) {
            Entries.push(MenuEntry::Separator());
            let Pinned = self.Session.IsFavorite(Folder);
            Entries.push(if Pinned { Run(self, "Unpin folder from Favorites", 0xE735, Command::UnpinFavorite) } else { Run(self, "Pin folder to Favorites", 0xE734, Command::PinFavorite) });
            Entries.push(Run(self, "Calculate folder size", 0xE9D9, Command::CalculateSize));
        }
        let Modules = self.ModuleEntries(false);
        if !Modules.is_empty() {
            Entries.push(MenuEntry::Group("Modules", 0xE74C, Modules));
        }
        Entries.push(MenuEntry::Separator());
        Entries.push(Run(self, "Properties", 0xE946, Command::Properties));
        if InFolder {
            Entries.push(MenuEntry::Item("Show more options", 0xE712, MenuAction::MoreOptions));
        }
        let Background = Directory.filter(|_| InFolder);
        self.OpenMenu(MenuPurpose::Context { Pane, Items: Vec::new(), Background: Background.clone(), ScreenX, ScreenY }, Entries, X, Y);
        if let Some(Folder) = Background {
            self.RequestShellEntries(crate::Platform::Menus::Extensions::ShellTarget::Background(Folder));
        }
    }

    fn ModuleEntries(&self, OnItems: bool) -> Vec<MenuEntry> {
        self.Modules
            .Commands()
            .into_iter()
            .filter(|(_, Item)| OnItems || Item.Requires != Requirement::Selection)
            .map(|(Index, Item)| MenuEntry::Item(&Item.Title, 0, MenuAction::Run(Command::Module(Index))).Enable(Item.Requires != Requirement::OtherPane || self.Settings.DualPane))
            .collect()
    }

    pub fn ShowOptionsMenu(&mut self, Pane: usize, X: f32, Y: f32) {
        let Mode = self.Tab(Pane).View;
        let Toggle = |App: &App, Label: &str, Item: Command, Value: bool| Run(App, Label, 0, Item).Check(Value);
        let Entries = vec![
            MenuEntry::Group(
                "Layout",
                0xE8A1,
                vec![
                    Run(self, "Details", 0xE8FD, Command::ViewDetails).Check(Mode == ViewMode::Details),
                    Run(self, "List", 0xE8FD, Command::ViewList).Check(Mode == ViewMode::List),
                    Run(self, "Grid", 0xF0E2, Command::ViewGrid).Check(Mode == ViewMode::Grid),
                    Run(self, "Thumbnails", 0xE8B9, Command::ViewThumbnails).Check(Mode == ViewMode::Thumbnails),
                ],
            ),
            Toggle(self, "Sidebar", Command::ToggleSidebar, self.Settings.ShowSidebar),
            Toggle(self, "Folder tree", Command::ToggleFolderTree, self.Settings.ShowFolderTree),
            Toggle(self, "Inspector", Command::ToggleInspector, self.Settings.ShowInspector),
            Toggle(self, "Dual pane", Command::ToggleDualPane, self.Settings.DualPane),
            MenuEntry::Separator(),
            Toggle(self, "Show hidden items", Command::ToggleHidden, self.Settings.ShowHidden),
            Toggle(self, "Animations", Command::ToggleAnimations, self.Settings.Animations),
            Toggle(self, "Performance overlay", Command::TogglePerformance, self.Settings.ShowPerformance),
            MenuEntry::Separator(),
            Run(self, &format!("Theme: {}", self.Settings.Theme), 0xE790, Command::CycleTheme),
            Run(self, &format!("Density: {:?}", self.Settings.Density), 0xE9A9, Command::CycleDensity),
            MenuEntry::Separator(),
            Run(self, "Command palette", 0xE721, Command::Palette),
            Run(self, "Modules", 0xE74C, Command::ManageModules),
            MenuEntry::Item("Settings", 0xE713, MenuAction::Settings).Keys(self, Command::OpenSettings),
        ];
        self.OpenMenu(MenuPurpose::Options { Pane }, Entries, X, Y);
    }

    pub fn ShowTabMenu(&mut self, Pane: usize, Index: usize, X: f32, Y: f32) {
        let Entries = vec![
            MenuEntry::Item("Close tab", 0xE8BB, MenuAction::Choice(1)).Keys(self, Command::CloseTab),
            MenuEntry::Item("Close other tabs", 0, MenuAction::Choice(2)).Enable(self.Panes[Pane].Tabs.len() > 1),
            MenuEntry::Item("Duplicate tab", 0xE8C8, MenuAction::Choice(3)).Keys(self, Command::DuplicateTab),
            MenuEntry::Item("Move to other pane", 0xE8A9, MenuAction::Choice(4)).Enable(self.Settings.DualPane),
            MenuEntry::Separator(),
            Run(self, "Reopen closed tab", 0xE7A7, Command::ReopenTab).Enable(!self.Panes[Pane].Closed.is_empty()),
        ];
        self.OpenMenu(MenuPurpose::Tab { Pane, Index }, Entries, X, Y);
    }

    pub fn ShowColumnsMenu(&mut self, X: f32, Y: f32) {
        let Entries = ColumnKind::All
            .iter()
            .enumerate()
            .map(|(Index, Kind)| {
                let Visible = self.Settings.Columns.iter().any(|Column| Column.Kind == *Kind && Column.Visible);
                MenuEntry::Item(Kind.Name(), 0, MenuAction::Choice(Index as u32)).Check(Visible).Enable(*Kind != ColumnKind::Name)
            })
            .collect();
        self.OpenMenu(MenuPurpose::Columns, Entries, X, Y);
    }

    pub fn ShowCrumbMenu(&mut self, Pane: usize, Base: PathBuf, Children: Vec<OsString>, X: f32, Y: f32) {
        if Children.is_empty() {
            self.Inform("This folder has no subfolders");
            return;
        }
        let Entries = Children.iter().take(400).enumerate().map(|(Index, Name)| MenuEntry::Item(&Name.to_string_lossy(), 0xE8B7, MenuAction::Choice(Index as u32))).collect();
        self.OpenMenu(MenuPurpose::Crumb { Pane, Base, Children }, Entries, X, Y);
    }

    pub fn ShowSidebarMenu(&mut self, Row: usize, X: f32, Y: f32) {
        if let Some(Index) = crate::Interface::Chrome::Sidebar::FavoriteIndex(self, Row) {
            let Count = self.Session.Favorites.len();
            let Entries = vec![
                MenuEntry::Item("Open in new tab", 0xE8A7, MenuAction::Choice(1)),
                MenuEntry::Item("Unpin from Favorites", 0xE735, MenuAction::Choice(2)),
                MenuEntry::Separator(),
                MenuEntry::Item("Move up", 0xE74A, MenuAction::Choice(3)).Enable(Index > 0),
                MenuEntry::Item("Move down", 0xE74B, MenuAction::Choice(4)).Enable(Index + 1 < Count),
            ];
            self.OpenMenu(MenuPurpose::Favorite { Index }, Entries, X, Y);
            return;
        }
        if let Some(Item) = crate::Interface::Chrome::Sidebar::RowPath(self, Row) {
            let Entries = vec![
                MenuEntry::Item("Open in new tab", 0xE8A7, MenuAction::Choice(1)),
                MenuEntry::Item("Open in other pane", 0xE8A9, MenuAction::Choice(2)).Enable(self.Settings.DualPane),
                MenuEntry::Item("Pin to Favorites", 0xE734, MenuAction::Choice(3)).Enable(!self.Session.IsFavorite(&Item)),
                MenuEntry::Item("Copy path", 0xE71B, MenuAction::Choice(5)),
                MenuEntry::Separator(),
                MenuEntry::Item("Properties", 0xE946, MenuAction::Choice(4)),
            ];
            self.OpenMenu(MenuPurpose::SidebarPath { Item }, Entries, X, Y);
        }
    }

    fn MenuTargetDirectory(&self, Purpose: &MenuPurpose) -> Option<PathBuf> {
        match Purpose {
            MenuPurpose::Context { Pane, Items, Background, .. } => {
                if let Some(Directory) = Background {
                    return Some(Directory.clone());
                }
                let Tab = self.Tab(*Pane);
                match Items.as_slice() {
                    [Single] if Tab.Targets().first().is_some_and(|&Index| Tab.Listing.IsDirectory(Index)) => Some(Single.clone()),
                    _ => self.CurrentDirectory(*Pane),
                }
            }
            _ => self.CurrentDirectory(self.ActivePane),
        }
    }

    pub fn OnMenuAction(&mut self, Purpose: MenuPurpose, Action: MenuAction) {
        if let MenuPurpose::Context { Pane, .. } | MenuPurpose::Options { Pane } = &Purpose {
            self.ActivePane = *Pane;
        }
        match Action {
            MenuAction::Run(Item) => self.Execute(Item),
            MenuAction::Sort(Key) => {
                let Pane = self.ActivePane;
                if self.Tab(Pane).Sort.Key != Key {
                    self.SortBy(Pane, Key);
                }
            }
            MenuAction::Settings => self.OpenSettingsPanel(),
            MenuAction::Terminal(Kind) | MenuAction::CurrentTerminal(Kind) => {
                let Target = if matches!(Action, MenuAction::CurrentTerminal(_)) { self.CurrentDirectory(self.ActivePane) } else { self.MenuTargetDirectory(&Purpose) };
                if let Some(Directory) = Target {
                    if let Err(Message) = Shell::OpenTerminal(&Directory, Kind) {
                        self.Fail(&format!("{} could not be started", Kind.Label()), &Message);
                    }
                }
            }
            MenuAction::CopyDirectory => {
                if let Some(Directory) = self.CurrentDirectory(self.ActivePane) {
                    if crate::Platform::Clipboard::SetText(self.Window, &Directory.display().to_string()) {
                        self.Inform(&format!("Copied {}", Directory.display()));
                    }
                }
            }
            MenuAction::ExplorerHere => {
                if let Some(Directory) = self.CurrentDirectory(self.ActivePane) {
                    if let Err(Message) = Shell::OpenInExplorer(&Directory, false) {
                        self.Fail("File Explorer could not be opened", &Message);
                    }
                }
            }
            MenuAction::RevealInExplorer => {
                if let MenuPurpose::Context { Items, .. } = &Purpose {
                    if let Some(Item) = Items.first() {
                        if let Err(Message) = Shell::OpenInExplorer(Item, true) {
                            self.Fail("File Explorer could not be opened", &Message);
                        }
                    }
                }
            }
            MenuAction::OpenContainingFolder => {
                if let MenuPurpose::Context { Pane, Items, .. } = &Purpose {
                    if let Some(Folder) = Items.first().and_then(|Item| Parent(Item)) {
                        let Name: Vec<u16> = Items[0].file_name().map(|Name| std::os::windows::ffi::OsStrExt::encode_wide(Name).collect()).unwrap_or_default();
                        let Pane = *Pane;
                        self.OpenTab(Pane, Location::Directory(Folder));
                        self.TabMut(Pane).PendingFocus = Some(Name);
                    }
                }
            }
            MenuAction::MoreOptions => {
                if let MenuPurpose::Context { Pane, Items, Background, ScreenX, ScreenY } = Purpose {
                    self.Deferred.push(Deferred::ContextMenu { Pane, Items, Background, X: ScreenX, Y: ScreenY });
                }
            }
            MenuAction::Choice(Choice) => self.OnMenuChoice(Purpose, Choice),
            MenuAction::Shell { Token, Command, Usage } => {
                if let MenuPurpose::Context { ScreenX, ScreenY, .. } = Purpose {
                    self.InvokeShellCommand(Token, Command, ScreenX, ScreenY);
                    self.RecordShellUsage(&Usage);
                }
            }
        }
        self.RefreshInspector();
        self.Invalidate();
    }

    fn OnMenuChoice(&mut self, Purpose: MenuPurpose, Choice: u32) {
        match Purpose {
            MenuPurpose::Tab { Pane, Index } => match Choice {
                1 => self.CloseTab(Pane, Index),
                2 => {
                    let Keep = self.Panes[Pane].Tabs[Index].Identifier;
                    let Closing: Vec<usize> = (0..self.Panes[Pane].Tabs.len()).rev().filter(|&Other| self.Panes[Pane].Tabs[Other].Identifier != Keep).collect();
                    for Other in Closing {
                        self.CloseTab(Pane, Other);
                    }
                }
                3 => {
                    let Target = self.Panes[Pane].Tabs[Index].Location.clone();
                    self.OpenTab(Pane, Target);
                }
                4 => self.TransferTab(Pane, Index),
                _ => {}
            },
            MenuPurpose::Crumb { Pane, Base, Children } => {
                if let Some(Name) = Children.get(Choice as usize) {
                    self.Navigate(Pane, Location::Directory(Base.join(Name)));
                }
            }
            MenuPurpose::Columns => {
                if let Some(Kind) = ColumnKind::All.get(Choice as usize) {
                    if let Some(Column) = self.Settings.Columns.iter_mut().find(|Column| Column.Kind == *Kind && Column.Kind != ColumnKind::Name) {
                        Column.Visible = !Column.Visible;
                    }
                    self.SaveSettings();
                }
            }
            MenuPurpose::Favorite { Index } => match Choice {
                1 => {
                    if let Some(Item) = self.Session.Favorites.get(Index).cloned() {
                        self.OpenTab(self.ActivePane, Location::Directory(Item));
                    }
                }
                2 => {
                    if Index < self.Session.Favorites.len() {
                        self.Session.Favorites.remove(Index);
                        let _ = self.Session.Save(&self.Paths.SessionFile());
                    }
                }
                3 => self.Session.MoveFavorite(Index, Index.saturating_sub(1)),
                4 => self.Session.MoveFavorite(Index, Index + 1),
                _ => {}
            },
            MenuPurpose::SidebarPath { Item } => match Choice {
                1 => self.OpenTab(self.ActivePane, Location::Directory(Item)),
                2 => {
                    if let Some(Other) = self.OtherPane() {
                        self.Navigate(Other, Location::Directory(Item));
                    }
                }
                3 => {
                    self.Session.Pin(&Item);
                    let _ = self.Session.Save(&self.Paths.SessionFile());
                }
                4 => self.Deferred.push(Deferred::Properties(vec![Item])),
                5 => {
                    crate::Platform::Clipboard::SetText(self.Window, &Item.display().to_string());
                    self.Inform("Path copied");
                }
                _ => {}
            },
            MenuPurpose::Context { .. } | MenuPurpose::Options { .. } => {}
        }
    }
}

const RowHeight: f32 = 28.0;
const SeparatorHeight: f32 = 9.0;

fn PanelSize(App: &mut App, Entries: &[MenuEntry]) -> (f32, f32) {
    let mut Width: f32 = 180.0;
    let mut Height = 8.0;
    for Entry in Entries {
        if Entry.IsSeparator() {
            Height += SeparatorHeight;
            continue;
        }
        Height += RowHeight;
        let Label = App.Draw.MeasureString(&Entry.Label, Style::Body);
        let Shortcut = if Entry.Shortcut.is_empty() { 0.0 } else { App.Draw.MeasureString(&Entry.Shortcut, Style::Small) + 24.0 };
        Width = Width.max(36.0 + Label + Shortcut + 34.0);
    }
    (Width.min(420.0), Height)
}

fn DrawPanel(App: &mut App, Entries: &[MenuEntry], X: f32, Y: f32, Level: u8, Highlight: Option<usize>, Opened: Option<usize>) -> (Rect, Vec<Rect>) {
    let Colors = App.Theme.Colors.clone();
    let Radius = App.Theme.Sizes.Radius + 2.0;
    let (Width, Height) = PanelSize(App, Entries);
    let Left = X.min(App.Width - Width - 6.0).max(4.0);
    let Top = if Y + Height > App.Height - 4.0 { (Y - Height).max(4.0).min(App.Height - Height - 4.0).max(4.0) } else { Y };
    let Panel = Rect::New(Left, Top, Width, Height.min(App.Height - 8.0));
    App.Draw.FillRounded(Panel.Inset(-5.0, -5.0), Radius + 5.0, Colors.Scrim.WithAlpha(0.10));
    App.Draw.FillRounded(Panel.Inset(-2.0, -2.0), Radius + 2.0, Colors.Scrim.WithAlpha(0.16));
    App.Draw.FillRounded(Panel, Radius, Colors.Raised);
    App.Draw.StrokeRounded(Panel, Radius, Colors.BorderStrong, 1.0);
    App.Hits.Push(Panel, Hit::Overlay);
    let mut Rows = Vec::with_capacity(Entries.len());
    let mut RowTop = Panel.Top + 4.0;
    App.Draw.PushClip(Panel);
    for (Index, Entry) in Entries.iter().enumerate() {
        if Entry.IsSeparator() {
            App.Draw.HorizontalLine(Panel.Left + 10.0, Panel.Right - 10.0, RowTop + SeparatorHeight / 2.0, Colors.Border);
            Rows.push(Rect::New(Panel.Left, RowTop, Panel.Width(), SeparatorHeight));
            RowTop += SeparatorHeight;
            continue;
        }
        let Row = Rect::New(Panel.Left + 4.0, RowTop, Panel.Width() - 8.0, RowHeight);
        Rows.push(Row);
        RowTop += RowHeight;
        let Target = Hit::MenuRow { Level, Index: Index as u16 };
        let Active = Entry.Enabled && (App.Pointer.Hover == Target || Highlight == Some(Index) || Opened == Some(Index));
        if Active {
            App.Draw.FillRounded(Row, App.Theme.Sizes.Radius, if Entry.Danger { Colors.Danger.WithAlpha(0.18) } else { Colors.Selection });
        }
        let Base = if !Entry.Enabled {
            Colors.TextFaint.WithAlpha(0.6)
        } else if Entry.Danger {
            Colors.Danger
        } else {
            Colors.Text
        };
        let IconArea = Rect::New(Row.Left + 6.0, Row.Top, 20.0, Row.Height());
        match Entry.Checked {
            Some(true) => App.Draw.Glyph(GlyphCheck, Style::IconSmall, IconArea, Colors.Accent),
            Some(false) => {}
            None if Entry.Image.is_some() => {
                if let Some(Bitmap) = Entry.Image.as_ref().and_then(|Image| Image.Bitmap(&App.Draw)) {
                    let Size = 16.0;
                    let Area = Rect::New(IconArea.Left + 2.0, IconArea.Top + (IconArea.Height() - Size) / 2.0, Size, Size);
                    App.Draw.Bitmap(&Bitmap, Area, if Entry.Enabled { 1.0 } else { 0.45 });
                }
            }
            None if Entry.Glyph != 0 => App.Draw.Glyph(Entry.Glyph, Style::IconSmall, IconArea, if Entry.Enabled { Colors.TextMuted } else { Base }),
            None => {}
        }
        App.Draw.Label(&Entry.Label, Style::Body, Rect { Left: Row.Left + 32.0, Right: Row.Right - 12.0, ..Row }, Base, Align::Left);
        if !Entry.Children.is_empty() {
            App.Draw.Glyph(GlyphChevron, Style::IconSmall, Rect::New(Row.Right - 22.0, Row.Top, 16.0, Row.Height()), Colors.TextMuted);
        } else if !Entry.Shortcut.is_empty() {
            App.Draw.Label(&Entry.Shortcut, Style::Small, Rect { Right: Row.Right - 10.0, ..Row }, Colors.TextFaint, Align::Right);
        }
        App.Hits.Push(Row, Target);
    }
    App.Draw.PopClip();
    (Panel, Rows)
}

pub fn DrawMenu(App: &mut App) {
    if let Hit::MenuRow { Level, Index } = App.Pointer.Hover {
        let (Level, Index) = (Level as usize, Index as usize);
        if let Some(Overlay::Menu(State)) = &mut App.Overlay {
            let Target = State.Level(Level).get(Index).map(|Entry| (!Entry.Children.is_empty() && Entry.Enabled, Entry.IsSeparator()));
            match Target {
                Some((true, _)) if State.Open.get(Level) != Some(&Index) => State.OpenAt(Level, Index, false),
                Some((false, false)) if State.Open.len() > Level => State.CloseFrom(Level),
                _ => {}
            }
        }
    }
    let Some(Overlay::Menu(State)) = &App.Overlay else { return };
    let (X, Y) = (State.X, State.Y);
    let Open = State.Open.clone();
    let Selected = State.Selected.clone();
    let Levels: Vec<Vec<MenuEntry>> = (0..=Open.len()).map(|Level| State.Level(Level).to_vec()).collect();
    let (mut Panel, mut Rows) = DrawPanel(App, &Levels[0], X, Y, 0, Selected.first().copied().flatten(), Open.first().copied());
    for Level in 1..Levels.len() {
        let Children = &Levels[Level];
        let Some(Row) = Rows.get(Open[Level - 1]).copied() else { break };
        let (ChildWidth, _) = PanelSize(App, Children);
        let ChildX = if Panel.Right + ChildWidth - 4.0 < App.Width { Panel.Right - 4.0 } else { (Panel.Left - ChildWidth + 4.0).max(4.0) };
        let Highlight = Selected.get(Level).copied().flatten();
        (Panel, Rows) = DrawPanel(App, Children, ChildX, Row.Top - 4.0, Level as u8, Highlight, Open.get(Level).copied());
    }
}

fn Step(Entries: &[MenuEntry], From: Option<usize>, Forward: bool) -> Option<usize> {
    let Count = Entries.len();
    if Count == 0 {
        return None;
    }
    let mut Index = From.unwrap_or(if Forward { Count - 1 } else { 0 });
    for _ in 0..Count {
        Index = if Forward { (Index + 1) % Count } else { (Index + Count - 1) % Count };
        if !Entries[Index].IsSeparator() && Entries[Index].Enabled {
            return Some(Index);
        }
    }
    None
}

pub fn Activate(App: &mut App, Level: u8, Index: usize) {
    let Some(Overlay::Menu(State)) = &mut App.Overlay else { return };
    let Level = Level as usize;
    let Some(Entry) = State.Level(Level).get(Index).cloned().filter(|Entry| Entry.Enabled && !Entry.IsSeparator()) else { return };
    if !Entry.Children.is_empty() {
        State.OpenAt(Level, Index, true);
        return;
    }
    let Some(Action) = Entry.Action else { return };
    let Some(Overlay::Menu(State)) = App.Overlay.take() else { return };
    App.Focus = Focus::List;
    App.OnMenuAction(State.Purpose, Action);
}

pub fn MenuKey(App: &mut App, Key: u16) -> bool {
    let Some(Overlay::Menu(State)) = &mut App.Overlay else { return false };
    let Depth = State.Open.len();
    let Current = State.Selected.get(Depth).copied().flatten();
    match Key {
        Key if Key == VK_ESCAPE.0 => {
            if Depth > 0 {
                State.CloseFrom(Depth - 1);
            } else {
                App.CloseOverlay();
            }
        }
        Key if Key == VK_DOWN.0 || Key == VK_UP.0 => {
            let Next = Step(State.Level(Depth), Current, Key == VK_DOWN.0);
            State.Selected.resize(Depth + 1, None);
            State.Selected[Depth] = Next;
        }
        Key if Key == VK_RIGHT.0 => {
            if let Some(Index) = Current.filter(|&Index| State.Level(Depth).get(Index).is_some_and(|Entry| !Entry.Children.is_empty() && Entry.Enabled)) {
                State.OpenAt(Depth, Index, true);
            }
        }
        Key if Key == VK_LEFT.0 => {
            if Depth > 0 {
                State.CloseFrom(Depth - 1);
            }
        }
        Key if Key == VK_RETURN.0 || Key == VK_SPACE.0 => {
            if let Some(Index) = Current {
                Activate(App, Depth as u8, Index);
            }
        }
        _ => {}
    }
    App.Invalidate();
    true
}
