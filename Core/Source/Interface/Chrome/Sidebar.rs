use crate::Application::{App, Event};
use crate::FileSystem::{LeafName, Location, Parent, SamePath};
use crate::Interface::Theme::Color;
use crate::Interface::Widgets::Motion::Channel;
use crate::Interface::Widgets::{DrawScrollbar, ScrollbarThumb, Scroller};
use crate::Interface::{Align, Hit, Rect, Style};
use FluxSdk::Utilities::FormatSize;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOTE, DRIVE_REMOVABLE};
use windows::core::PCWSTR;

const GlyphStar: u16 = 0xE734;
const GlyphFolder: u16 = 0xE8B7;
const GlyphDrive: u16 = 0xEDA2;
const GlyphRemovable: u16 = 0xE88E;
const GlyphOptical: u16 = 0xE958;
const GlyphNetwork: u16 = 0xE968;
const GlyphHistory: u16 = 0xE81C;
const GlyphComputer: u16 = 0xE977;
const GlyphCollapsed: u16 = 0xE76C;
const GlyphExpanded: u16 = 0xE70D;
const RecentShown: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Favorites,
    Recent,
    Folders,
}

impl Section {
    fn Title(self, Tree: bool) -> &'static str {
        match self {
            Section::Favorites => "FAVORITES",
            Section::Recent => "RECENT",
            Section::Folders if Tree => "FOLDERS",
            Section::Folders => "DRIVES",
        }
    }

    fn Channel(self) -> Channel {
        Channel::Section(self as u8)
    }
}

#[derive(Clone, Debug)]
pub enum SidebarRow {
    Header(Section, usize),
    Favorite(usize),
    Recent(usize),
    Computer,
    Tree { Item: PathBuf, Depth: u16, Glyph: u16 },
}

#[derive(Default)]
pub struct SidebarState {
    pub Scroll: Scroller,
    pub Rows: Vec<SidebarRow>,
    Expanded: HashSet<String>,
    Children: HashMap<String, Vec<OsString>>,
    Loading: HashSet<String>,
    Space: HashMap<String, (u64, u64)>,
    SpaceRequested: HashSet<String>,
    Generation: u64,
    Signature: u64,
    pub ContentHeight: f32,
}

fn Key(Item: &Path) -> String {
    let Text = Item.to_string_lossy().to_lowercase();
    let Trimmed = Text.trim_end_matches('\\');
    if Trimmed.len() == 2 && Trimmed.ends_with(':') { format!("{Trimmed}\\") } else { Trimmed.to_string() }
}

fn IsDriveRoot(Item: &Path) -> bool {
    let Text = Item.to_string_lossy();
    Text.len() <= 3 && Text.as_bytes().get(1) == Some(&b':')
}

impl SidebarState {
    pub fn Reveal(&mut self, Target: &Path) -> Vec<PathBuf> {
        let mut Needed = Vec::new();
        let mut Current = Parent(Target);
        while let Some(Folder) = Current {
            let FolderKey = Key(&Folder);
            if self.Expanded.insert(FolderKey.clone()) {
                self.Generation += 1;
            }
            if !self.Children.contains_key(&FolderKey) {
                Needed.push(Folder.clone());
            }
            Current = Parent(&Folder);
        }
        Needed
    }

    pub fn BeginLoading(&mut self, Folder: &Path) -> bool {
        self.Loading.insert(Key(Folder))
    }

    pub fn AcceptChildren(&mut self, Folder: PathBuf, Children: Result<Vec<OsString>, u32>) {
        let FolderKey = Key(&Folder);
        self.Loading.remove(&FolderKey);
        self.Children.insert(FolderKey, Children.unwrap_or_default());
        self.Generation += 1;
    }

    pub fn AcceptSpace(&mut self, Directory: &Path, Value: Option<(u64, u64)>) {
        if !IsDriveRoot(Directory) {
            return;
        }
        match Value {
            Some(Space) => {
                self.Space.insert(Key(Directory), Space);
            }
            None => {
                self.Space.remove(&Key(Directory));
            }
        }
    }

    pub fn InvalidateSpace(&mut self) {
        self.SpaceRequested.clear();
    }

    fn Toggle(&mut self, Folder: &Path) -> bool {
        let FolderKey = Key(Folder);
        self.Generation += 1;
        if !self.Expanded.remove(&FolderKey) {
            self.Expanded.insert(FolderKey.clone());
            return !self.Children.contains_key(&FolderKey);
        }
        false
    }

    fn Flatten(&self, Folder: &Path, Depth: u16, Output: &mut Vec<SidebarRow>) {
        let FolderKey = Key(Folder);
        if !self.Expanded.contains(&FolderKey) {
            return;
        }
        if let Some(Children) = self.Children.get(&FolderKey) {
            for Name in Children {
                let Item = Folder.join(Name);
                Output.push(SidebarRow::Tree { Item: Item.clone(), Depth, Glyph: GlyphFolder });
                self.Flatten(&Item, Depth + 1, Output);
            }
        }
    }

    pub fn HasChildren(&self, Folder: &Path) -> Option<bool> {
        self.Children.get(&Key(Folder)).map(|Children| !Children.is_empty())
    }

    pub fn IsExpanded(&self, Folder: &Path) -> bool {
        self.Expanded.contains(&Key(Folder))
    }

    pub fn IsLoading(&self, Folder: &Path) -> bool {
        self.Loading.contains(&Key(Folder))
    }
}

fn Drives() -> Vec<(PathBuf, u16)> {
    let Mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|Letter| Mask & (1 << Letter) != 0)
        .map(|Letter| {
            let Root = format!("{}:\\", (b'A' + Letter as u8) as char);
            let Wide: Vec<u16> = Root.encode_utf16().chain(std::iter::once(0)).collect();
            let Glyph = match unsafe { GetDriveTypeW(PCWSTR(Wide.as_ptr())) } {
                DRIVE_REMOTE => GlyphNetwork,
                DRIVE_REMOVABLE => GlyphRemovable,
                DRIVE_CDROM => GlyphOptical,
                _ => GlyphDrive,
            };
            (PathBuf::from(Root), Glyph)
        })
        .collect()
}

fn RequestSpace(App: &mut App, Drives: &[(PathBuf, u16)]) {
    for (Root, Glyph) in Drives {
        if *Glyph == GlyphOptical || !App.Sidebar.SpaceRequested.insert(Key(Root)) {
            continue;
        }
        let Sink = App.Sink.clone();
        let Directory = Root.clone();
        App.Background.Submit(move || {
            let Value = crate::FileSystem::Enumerate::FreeSpace(&Directory);
            Sink.Send(Event::FreeSpace { Directory, Value });
        });
    }
}

fn Rebuild(App: &mut App) {
    let DriveList = Drives();
    RequestSpace(App, &DriveList);
    let UncRoot = App.CurrentDirectory(App.ActivePane).filter(|Item| Item.to_string_lossy().starts_with("\\\\")).map(|Item| {
        let mut Current = Item;
        while let Some(Up) = Parent(&Current) {
            if crate::FileSystem::IsServerPath(&Up) {
                break;
            }
            Current = Up;
        }
        Current
    });
    let mut Hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher as _};
    App.Session.Favorites.hash(&mut Hasher);
    App.Session.Recent.iter().take(RecentShown).collect::<Vec<_>>().hash(&mut Hasher);
    DriveList.hash(&mut Hasher);
    UncRoot.hash(&mut Hasher);
    App.Sidebar.Generation.hash(&mut Hasher);
    (App.Settings.ShowFolderTree, App.Settings.FavoritesCollapsed, App.Settings.RecentCollapsed, App.Settings.FoldersCollapsed).hash(&mut Hasher);
    let Signature = Hasher.finish();
    if Signature == App.Sidebar.Signature && !App.Sidebar.Rows.is_empty() {
        return;
    }
    App.Sidebar.Signature = Signature;
    let mut Rows = Vec::new();
    let Favorites = App.Session.Favorites.len();
    if Favorites > 0 {
        Rows.push(SidebarRow::Header(Section::Favorites, Favorites));
        if !App.Settings.FavoritesCollapsed {
            Rows.extend((0..Favorites).map(SidebarRow::Favorite));
        }
    }
    let Recent = App.Session.Recent.len().min(RecentShown);
    if Recent > 0 {
        Rows.push(SidebarRow::Header(Section::Recent, Recent));
        if !App.Settings.RecentCollapsed {
            Rows.extend((0..Recent).map(SidebarRow::Recent));
        }
    }
    Rows.push(SidebarRow::Header(Section::Folders, DriveList.len()));
    if !App.Settings.FoldersCollapsed {
        Rows.push(SidebarRow::Computer);
        for (Root, Glyph) in DriveList.into_iter().chain(UncRoot.map(|Root| (Root, GlyphNetwork))) {
            Rows.push(SidebarRow::Tree { Item: Root.clone(), Depth: 0, Glyph });
            if App.Settings.ShowFolderTree {
                App.Sidebar.Flatten(&Root, 1, &mut Rows);
            }
        }
    }
    App.Sidebar.Rows = Rows;
}

pub fn RowPath(App: &App, Row: usize) -> Option<PathBuf> {
    match App.Sidebar.Rows.get(Row)? {
        SidebarRow::Favorite(Index) => App.Session.Favorites.get(*Index).cloned(),
        SidebarRow::Recent(Index) => App.Session.Recent.get(*Index).cloned(),
        SidebarRow::Tree { Item, .. } => Some(Item.clone()),
        _ => None,
    }
}

pub fn RowLocation(App: &App, Row: usize) -> Option<Location> {
    match App.Sidebar.Rows.get(Row)? {
        SidebarRow::Computer => Some(Location::Computer),
        _ => RowPath(App, Row).map(Location::Directory),
    }
}

pub fn FavoriteIndex(App: &App, Row: usize) -> Option<usize> {
    match App.Sidebar.Rows.get(Row)? {
        SidebarRow::Favorite(Index) => Some(*Index),
        _ => None,
    }
}

pub fn ToggleSection(App: &mut App, Row: usize) -> bool {
    let Some(SidebarRow::Header(Kind, _)) = App.Sidebar.Rows.get(Row).cloned() else { return false };
    let Flag = match Kind {
        Section::Favorites => &mut App.Settings.FavoritesCollapsed,
        Section::Recent => &mut App.Settings.RecentCollapsed,
        Section::Folders => &mut App.Settings.FoldersCollapsed,
    };
    *Flag = !*Flag;
    App.SaveSettings();
    true
}

pub fn ToggleRow(App: &mut App, Row: usize) -> Option<PathBuf> {
    let Some(SidebarRow::Tree { Item, .. }) = App.Sidebar.Rows.get(Row).cloned() else { return None };
    if App.Sidebar.Toggle(&Item) { Some(Item) } else { None }
}

pub fn Scroll(App: &mut App, Delta: f32) {
    let Limit = (App.Sidebar.ContentHeight - App.ViewAreas[0].Height()).max(0.0);
    let Target = (App.Sidebar.Scroll.Target + Delta).clamp(0.0, Limit.max(App.Sidebar.ContentHeight));
    if App.Settings.SmoothScrolling { App.Sidebar.Scroll.Target = Target } else { App.Sidebar.Scroll.Jump(Target) }
}

fn RowHeight(Row: &SidebarRow, Base: f32) -> f32 {
    match Row {
        SidebarRow::Header(..) => Base + 6.0,
        SidebarRow::Tree { Item, Depth: 0, .. } if IsDriveRoot(Item) => Base + 12.0,
        _ => Base,
    }
}

fn SectionOf(Rows: &[SidebarRow], Index: usize) -> Option<Section> {
    Rows[..=Index].iter().rev().find_map(|Row| match Row {
        SidebarRow::Header(Kind, _) => Some(*Kind),
        _ => None,
    })
}

fn Faded(Value: Color, Opacity: f32) -> Color {
    Value.WithAlpha(Value.A * Opacity)
}

pub fn DrawSidebar(App: &mut App, Area: Rect) {
    Rebuild(App);
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    let BaseHeight = Look.Sizes.SidebarRowHeight;
    App.Draw.Fill(Area, Colors.Sidebar);
    App.Hits.Push(Area, Hit::SidebarArea);
    let Heights: Vec<f32> = App.Sidebar.Rows.iter().map(|Row| RowHeight(Row, BaseHeight)).collect();
    let Content: f32 = Heights.iter().sum::<f32>() + 12.0;
    App.Sidebar.ContentHeight = Content;
    App.Sidebar.Scroll.Clamp(Content - Area.Height());
    let Offset = App.Sidebar.Scroll.Offset;
    let Current = App.CurrentDirectory(App.ActivePane);
    let AtComputer = matches!(App.Tab(App.ActivePane).Location, Location::Computer);
    let DropRow = match App.DropHover {
        Some(crate::Application::DropHover::Sidebar(Row)) => Some(Row),
        _ => None,
    };
    let Reveal = [Section::Favorites, Section::Recent, Section::Folders].map(|Kind| {
        let Collapsed = match Kind {
            Section::Favorites => App.Settings.FavoritesCollapsed,
            Section::Recent => App.Settings.RecentCollapsed,
            Section::Folders => App.Settings.FoldersCollapsed,
        };
        App.Motion.Watch(Kind.Channel(), Collapsed as u64, 200, false)
    });
    App.Draw.PushClip(Area);
    let mut Y = Area.Top + 6.0 - Offset;
    for Index in 0..App.Sidebar.Rows.len() {
        let Height = Heights[Index];
        if Y + Height < Area.Top {
            Y += Height;
            continue;
        }
        if Y > Area.Bottom {
            break;
        }
        let Row = Rect::New(Area.Left + 4.0, Y, Area.Width() - 8.0, Height);
        Y += Height;
        let Kind = App.Sidebar.Rows[Index].clone();
        let Target = Hit::SidebarRow(Index as u32);
        if let SidebarRow::Header(Group, Count) = Kind {
            let Collapsed = match Group {
                Section::Favorites => App.Settings.FavoritesCollapsed,
                Section::Recent => App.Settings.RecentCollapsed,
                Section::Folders => App.Settings.FoldersCollapsed,
            };
            let Line = Rect { Top: Row.Top + 6.0, ..Row };
            if App.Pointer.Hover == Target {
                App.Draw.FillRounded(Line, Look.Sizes.Radius, Colors.Hover);
            }
            App.Draw.Glyph(if Collapsed { GlyphCollapsed } else { GlyphExpanded }, Style::IconSmall, Rect::New(Line.Left + 2.0, Line.Top, 16.0, Line.Height()), Colors.TextFaint);
            App.Draw.Label(Group.Title(App.Settings.ShowFolderTree), Style::Small, Rect { Left: Line.Left + 22.0, ..Line }, Colors.TextFaint, Align::Left);
            if Collapsed {
                App.Draw.Label(&Count.to_string(), Style::Small, Rect { Right: Line.Right - 8.0, ..Line }, Colors.TextFaint, Align::Right);
            }
            App.Hits.Push(Line, Target);
            continue;
        }
        let Opacity = match SectionOf(&App.Sidebar.Rows, Index) {
            Some(Group) => Reveal[Group as usize],
            None => 1.0,
        };
        let (Label, Glyph, Depth, TreePath) = match &Kind {
            SidebarRow::Favorite(Item) => {
                let Path = App.Session.Favorites.get(*Item).cloned().unwrap_or_default();
                (LeafName(&Path), GlyphStar, 0u16, None)
            }
            SidebarRow::Recent(Item) => {
                let Path = App.Session.Recent.get(*Item).cloned().unwrap_or_default();
                (LeafName(&Path), GlyphHistory, 0, None)
            }
            SidebarRow::Computer => ("This PC".to_string(), GlyphComputer, 0, None),
            SidebarRow::Tree { Item, Depth, Glyph } => {
                let Name = if *Depth == 0 { Item.to_string_lossy().trim_end_matches('\\').to_string() } else { LeafName(Item) };
                (Name, *Glyph, *Depth, Some(Item.clone()))
            }
            SidebarRow::Header(..) => continue,
        };
        let RowPathValue = match &Kind {
            SidebarRow::Computer => None,
            _ => RowPath(App, Index),
        };
        let Selected = match &Kind {
            SidebarRow::Computer => AtComputer,
            _ => RowPathValue.as_ref().zip(Current.as_ref()).is_some_and(|(A, B)| SamePath(A, B)),
        };
        if Selected {
            App.Draw.FillRounded(Row, Look.Sizes.Radius, Faded(Colors.SelectionInactive, Opacity));
            App.Draw.Fill(Rect::New(Row.Left, Row.Top + 5.0, 2.0, Row.Height() - 10.0), Faded(Colors.Accent, Opacity));
        } else if App.Pointer.Hover == Target || App.Pointer.Hover == Hit::SidebarToggle(Index as u32) {
            App.Draw.FillRounded(Row, Look.Sizes.Radius, Faded(Colors.Hover, Opacity));
        }
        if DropRow == Some(Index as u32) {
            App.Draw.StrokeRounded(Row, Look.Sizes.Radius, Colors.Accent, 1.5);
        }
        App.Hits.Push(Row, Target);
        let DriveSpace = TreePath.as_ref().filter(|Item| Depth == 0 && IsDriveRoot(Item)).and_then(|Item| App.Sidebar.Space.get(&Key(Item)).copied());
        let Line = if TreePath.as_ref().is_some_and(|Item| Depth == 0 && IsDriveRoot(Item)) { Rect { Bottom: Row.Bottom - 10.0, ..Row } } else { Row };
        let mut X = Row.Left + 6.0 + Depth as f32 * 14.0;
        if let Some(Folder) = &TreePath {
            if App.Settings.ShowFolderTree {
                let Toggle = Rect::New(X, Line.Top, 16.0, Line.Height());
                let Expanded = App.Sidebar.IsExpanded(Folder);
                if App.Sidebar.HasChildren(Folder) != Some(false) || App.Sidebar.IsLoading(Folder) {
                    App.Draw.Glyph(if Expanded { GlyphExpanded } else { GlyphCollapsed }, Style::IconSmall, Toggle, Faded(Colors.TextFaint, Opacity));
                    App.Hits.Push(Toggle, Hit::SidebarToggle(Index as u32));
                }
                X += 16.0;
            }
        }
        App.Draw.Glyph(Glyph, Style::IconSmall, Rect::New(X, Line.Top, 18.0, Line.Height()), Faded(if Selected { Colors.Accent } else { Colors.TextMuted }, Opacity));
        X += 24.0;
        let TextColor = Faded(if Selected { Colors.Text } else { Colors.Text.Mix(Colors.TextMuted, 0.25) }, Opacity);
        App.Draw.Label(&Label, Style::Body, Rect { Left: X, Right: Line.Right - 6.0, ..Line }, TextColor, Align::Left);
        if let Some((Free, Total)) = DriveSpace.filter(|(_, Total)| *Total > 0) {
            let Used = 1.0 - Free as f32 / Total as f32;
            let FreeText = format!("{} free", FormatSize(Free));
            let TextWidth = App.Draw.MeasureString(&FreeText, Style::Small);
            let Bar = Rect::New(X, Row.Bottom - 9.0, (Row.Right - 10.0 - X - TextWidth - 8.0).max(24.0), 4.0);
            App.Draw.UsageBar(Bar, Used, Faded(Colors.Border, Opacity));
            App.Draw.Label(&FreeText, Style::Small, Rect::New(Bar.Right + 6.0, Row.Bottom - 15.0, TextWidth + 4.0, 16.0), Faded(Colors.TextFaint, Opacity), Align::Left);
        }
    }
    App.Draw.PopClip();
    let Track = Rect::New(Area.Right - 10.0, Area.Top + 2.0, 8.0, Area.Height() - 4.0);
    if ScrollbarThumb(Track, Offset, Content, Area.Height()).is_some() {
        let Hovered = matches!(App.Pointer.Hover, Hit::SidebarArea | Hit::SidebarRow(_) | Hit::SidebarToggle(_));
        let _ = DrawScrollbar(&App.Draw, &Look, Track, Offset, Content, Area.Height(), Hovered && App.Pointer.X > Track.Left - 4.0);
    }
}
