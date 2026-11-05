use crate::Control::Commands::Keymap;
use crate::Control::Panes::{Pane, Tab, Visibility};
use crate::Control::Session::Session;
use crate::Control::Session::Undo::{Purpose, UndoStack};
use crate::FileSystem::Operations::{Notification, Operation};
use crate::FileSystem::Search::SearchStatistics;
use crate::FileSystem::Sorting::{SortKey, SortSpec};
use crate::FileSystem::Watcher::{ChangeBatch, Watcher};
use crate::FileSystem::{Listing, Location};
use crate::Interface::Chrome::Sidebar::SidebarState;
use crate::Interface::Media::{Media, MediaResult};
use crate::Interface::Overlays::Inspector::{InspectorState, PreviewContent};
use crate::Interface::Overlays::Overlay;
use crate::Interface::Theme::Theme;
use crate::Interface::Widgets::LineEdit;
use crate::Interface::Widgets::Motion::{Channel, Motion};
use crate::Interface::{Hit, HitList, Rect, Renderer, Splitter};
use crate::Platform;
use crate::Platform::Clipboard::DragDrop::DropRequest;
use crate::Platform::Menus::ContextTarget;
use crate::Services::Config::{AppPaths, Settings};
use crate::Services::Modules::{ModuleEvent, ModuleHost};
use crate::Services::Performance::Performance;
use crate::Services::Workers::{EventQueue, EventSink, Pool, WakeMessage};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::*;

pub enum Event {
    ListingPartial { Token: u64, Chunk: Listing },
    ListingLoaded { Token: u64, Result: Result<(Listing, Vec<u32>), u32>, Elapsed: Duration },
    SearchBatch { Token: u64, Chunk: Listing },
    SearchDone { Token: u64, Statistics: SearchStatistics },
    DirectoryChanged(ChangeBatch),
    Media(MediaResult),
    Operation(Notification),
    FreeSpace { Directory: PathBuf, Value: Option<(u64, u64)> },
    Completions { Pane: usize, Token: u64, Items: Vec<String> },
    TreeChildren { Directory: PathBuf, Children: Result<Vec<OsString>, u32> },
    CrumbChildren { Pane: usize, Base: PathBuf, Children: Vec<OsString>, X: f32, Y: f32 },
    Preview { Token: u64, Content: PreviewContent },
    FolderSize { Token: u64, Bytes: u64, Files: u64, Folders: u64, Done: bool },
    Module(ModuleEvent),
    ShellMenu(crate::Platform::Menus::Extensions::ShellMenuUpdate),
    FolderSizes(Vec<(PathBuf, crate::FileSystem::Sizes::FolderSize)>),
    Reveal(PathBuf, bool),
    Update(crate::Services::Update::UpdateEvent),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    List,
    Address,
    Filter,
    Rename,
    Overlay,
}

pub struct RenameState {
    pub Pane: usize,
    pub TabIdentifier: u64,
    pub Index: u32,
    pub Edit: LineEdit,
    pub Original: PathBuf,
}

#[derive(Clone, Debug)]
pub enum DragState {
    Idle,
    Pending { Pane: usize, X: f32, Y: f32, Index: u32 },
    Rubber { Pane: usize, StartX: f32, StartY: f32, X: f32, Y: f32, Base: Vec<u32> },
    Split { Kind: Splitter, StartX: f32, StartValue: f32 },
    Column { Column: usize, StartX: f32, StartWidth: f32 },
    TabMove { Pane: usize, Index: usize, StartX: f32, Moved: bool },
    Thumb { Pane: usize, Grab: f32 },
    Text { Target: Hit },
}

pub enum Deferred {
    ContextMenu { Pane: usize, Items: Vec<PathBuf>, Background: Option<PathBuf>, X: i32, Y: i32 },
    StartDrag { Items: Vec<PathBuf> },
    Properties(Vec<PathBuf>),
    OpenWith(PathBuf),
    Open(PathBuf),
    RunAs(PathBuf),
    PickModuleFolder,
    TestCrash,
    PasteShell(PathBuf),
    DropShell { Sources: Vec<PathBuf>, Destination: PathBuf, Moving: bool },
}

pub struct Notice {
    pub Text: String,
    pub Error: bool,
    pub Detail: Option<String>,
}

pub struct PointerState {
    pub X: f32,
    pub Y: f32,
    pub Hover: Hit,
    pub Drag: DragState,
    pub Tracking: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DropHover {
    List { Pane: usize, Target: Option<u32> },
    Sidebar(u32),
    Crumb { Pane: usize, Index: u16 },
}

pub struct App {
    pub Window: HWND,
    pub Paths: AppPaths,
    pub Settings: Settings,
    pub Theme: Theme,
    pub Keys: Keymap,
    pub Session: Session,
    pub Draw: Renderer,
    pub Media: Media,
    pub Hits: HitList,
    pub Panes: Vec<Pane>,
    pub ActivePane: usize,
    pub Focus: Focus,
    pub Rename: Option<RenameState>,
    pub Overlay: Option<Overlay>,
    pub Pointer: PointerState,
    pub DropHover: Option<DropHover>,
    pub Sink: EventSink,
    pub Queue: EventQueue,
    pub Watcher: Option<Watcher>,
    pub Operations: Vec<Operation>,
    pub Purposes: HashMap<u64, Purpose>,
    pub Collisions: VecDeque<(u64, crate::FileSystem::Operations::CollisionQuery)>,
    pub Undo: UndoStack,
    pub Modules: ModuleHost,
    pub ShellMenus: Option<crate::Platform::Menus::Extensions::ShellMenus>,
    pub ShellUsage: crate::Interface::Overlays::Menu::Native::ShellUsage,
    pub Performance: Performance,
    pub Sidebar: SidebarState,
    pub Inspector: InspectorState,
    pub FolderSizes: Option<crate::FileSystem::Sizes::FolderSizes>,
    pub KnownSizes: HashMap<PathBuf, crate::FileSystem::Sizes::FolderSize>,
    pub Notice: Option<Notice>,
    pub Background: Pool,
    pub TypeAhead: (Vec<u16>, u32),
    pub Width: f32,
    pub Height: f32,
    pub ViewAreas: [Rect; 2],
    pub Deferred: Vec<Deferred>,
    pub Active: bool,
    pub ScrollTimer: bool,
    pub PerformanceTimer: bool,
    pub Bench: Option<crate::Services::Performance::UiBenchmark>,
    pub Updating: bool,
    pub PendingUpdate: Option<crate::Services::Update::Release>,
    pub Relaunch: Option<PathBuf>,
    pub ShellFresh: bool,
    pub Motion: Motion,
    pub Logo: Option<(u64, u32, windows::Win32::Graphics::Direct2D::ID2D1Bitmap)>,
}

thread_local! {
    static Instance: RefCell<Option<Box<App>>> = const { RefCell::new(None) };
}

pub fn WithApp<R>(Work: impl FnOnce(&mut App) -> R) -> Option<R> {
    Instance.with(|Slot| {
        let mut Guard = Slot.try_borrow_mut().ok()?;
        let App = Guard.as_mut()?;
        Some(Work(App))
    })
}

pub fn CaptionTest(Window: HWND, X: i32, Y: i32) -> u32 {
    let _ = Window;
    WithApp(|App| {
        let Scale = App.Draw.Dpi / 96.0;
        match App.Hits.Find(X as f32 / Scale, Y as f32 / Scale).0 {
            Hit::Caption => HTCAPTION,
            _ => HTCLIENT,
        }
    })
    .unwrap_or(HTCLIENT)
}

pub fn DropFeedback(Request: &DropRequest) -> crate::Platform::Clipboard::DragDrop::DropAnswer {
    WithApp(|App| App.OnDrop(Request, false)).unwrap_or(crate::Platform::Clipboard::DragDrop::DropAnswer { Effect: 0, Shell: None })
}

pub fn DropCommit(Request: &DropRequest) -> u32 {
    WithApp(|App| App.OnDrop(Request, true).Effect).unwrap_or(0)
}

pub fn AfterShellDrop() {
    WithApp(|App| App.ReloadArchives(None));
}

pub fn Dispatch(Window: HWND, Message: u32, WParameter: WPARAM, LParameter: LPARAM) -> Option<LRESULT> {
    if let Some(Result) = Platform::Menus::RouteMenuMessage(Message, WParameter, LParameter) {
        return Some(Result);
    }
    if Message == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return Some(LRESULT(0));
    }
    let Result = WithApp(|App| App.OnMessage(Message, WParameter, LParameter)).flatten();
    RunDeferred(Window);
    if let Some((Folder, Fresh)) = WithApp(|App| (App.ShellFolder(), App.ShellFresh)) {
        match Platform::Browser::Navigate(Folder.as_deref(), Fresh) {
            Platform::Browser::Report::Done if Fresh => {
                WithApp(|App| App.ShellFresh = false);
            }
            Platform::Browser::Report::Later => unsafe {
                let _ = PostMessageW(Some(Window), WakeMessage, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
    }
    Result
}

fn RunDeferred(Window: HWND) {
    loop {
        let Pending = WithApp(|App| std::mem::take(&mut App.Deferred)).unwrap_or_default();
        if Pending.is_empty() {
            return;
        }
        for Item in Pending {
            match Item {
                Deferred::ContextMenu { Pane, Items, Background, X, Y } => {
                    let Extended = Platform::KeyDown(0x10);
                    let Outcome = match &Background {
                        Some(Directory) => Platform::Menus::ShowContextMenu(Window, ContextTarget::Background(Directory), X, Y, Extended),
                        None => Platform::Menus::ShowContextMenu(Window, ContextTarget::Items(&Items), X, Y, Extended),
                    };
                    WithApp(|App| App.ApplyContextOutcome(Pane, Outcome));
                }
                Deferred::StartDrag { Items } => {
                    let Effect = if Platform::KeyDown(0x01) { Platform::Clipboard::DragDrop::StartDrag(Window, &Items) } else { None };
                    WithApp(|App| App.AfterDrag(Effect));
                }
                Deferred::Properties(Items) => {
                    if let Err(Message) = Platform::Shell::ShowProperties(Window, &Items) {
                        WithApp(|App| App.Fail("Properties", &Message));
                    }
                }
                Deferred::OpenWith(Item) => {
                    if let Err(Message) = Platform::Shell::OpenWithDialog(Window, &Item) {
                        WithApp(|App| App.Fail("Open With", &Message));
                    }
                }
                Deferred::Open(Item) => {
                    if let Err(Message) = Platform::Shell::OpenItem(Window, &Item) {
                        WithApp(|App| App.Fail(&format!("Could not open {}", Item.display()), &Message));
                    }
                }
                Deferred::RunAs(Item) => {
                    if let Err(Message) = Platform::Shell::RunAsAdministrator(Window, &Item) {
                        WithApp(|App| App.Fail("Run as administrator", &Message));
                    }
                }
                Deferred::PickModuleFolder => {
                    if let Some(Folder) = Platform::Shell::PickFolder(Window, "Select a Flux module folder (containing Module.ini)") {
                        WithApp(|App| App.InstallModuleFrom(&Folder));
                    }
                }
                Deferred::TestCrash => crate::Services::Diagnostics::TriggerTestCrash(),
                Deferred::PasteShell(Directory) => {
                    let Result = Platform::Clipboard::PasteShell(Window, &Directory);
                    WithApp(|App| {
                        if let Err(Message) = Result {
                            App.Fail("Paste failed", &Message);
                        }
                        App.ReloadArchives(None);
                    });
                }
                Deferred::DropShell { Sources, Destination, Moving } => {
                    let Result = Platform::Clipboard::DropFiles(Window, &Sources, &Destination, Moving);
                    WithApp(|App| {
                        if let Err(Message) = Result {
                            App.Fail(if Moving { "Move failed" } else { "Copy failed" }, &Message);
                        }
                        App.ReloadArchives(None);
                    });
                }
            }
        }
        Platform::Invalidate(Window);
    }
}

pub fn Run(Arguments: &[String]) -> i32 {
    let Paths = AppPaths::Discover();
    let Settings = Settings::Load(&Paths.SettingsFile());
    let Session = Session::Load(&Paths.SessionFile());
    let Window = match Platform::CreateMainWindow(if Settings.RestoreSession { Session.Window.as_ref() } else { None }) {
        Ok(Window) => Window,
        Err(Error) => {
            Platform::ShowError(None, "Flux", &format!("The main window could not be created: {}", Error.message()));
            return 1;
        }
    };
    let Maximized = Settings.RestoreSession && Session.Window.as_ref().is_some_and(|Place| Place.Maximized);
    let App = match App::New(Window, Paths, Settings, Session, Arguments) {
        Ok(App) => App,
        Err(Message) => {
            Platform::ShowError(Some(Window), "Flux", &Message);
            return 1;
        }
    };
    Instance.with(|Slot| *Slot.borrow_mut() = Some(Box::new(App)));
    Platform::Clipboard::DragDrop::Register(Window);
    WithApp(|App| App.Start());
    if let Some(Text) = Platform::Integration::StartupTarget(Arguments) {
        WithApp(|App| App.OpenExternal(&Text));
    }
    Platform::Show(Window, Maximized);
    let Benchmarking = WithApp(|App| App.Bench.is_some()).unwrap_or(false);
    if !Benchmarking && Platform::Integration::FluxIsDefault() {
        if let Some(Sink) = WithApp(|App| App.Sink.clone()) {
            Platform::Browser::Register(Window, Sink);
        }
    }
    if !Benchmarking {
        WithApp(|App| App.StartUpdates());
    }
    let Code = Platform::RunMessageLoop();
    Platform::Browser::Unregister();
    let Exit = WithApp(|App| App.Bench.as_ref().map(|Bench| Bench.ExitCode)).flatten();
    let Relaunch = WithApp(|App| App.Relaunch.take()).flatten();
    Instance.with(|Slot| {
        if let Ok(mut Guard) = Slot.try_borrow_mut() {
            Guard.take();
        }
    });
    if let Some(Executable) = Relaunch {
        let _ = std::process::Command::new(Executable).spawn();
    }
    Exit.unwrap_or(Code)
}

impl App {
    fn New(Window: HWND, Paths: AppPaths, Settings: Settings, Session: Session, Arguments: &[String]) -> Result<App, String> {
        let Look = Theme::Load(&Settings.Theme, Settings.Density, &Paths.ThemesDirectory());
        let Dpi = Platform::WindowDpi(Window) as f32;
        let mut Draw = Renderer::New(Window, Dpi, &Settings.FontFamily, &Look.Sizes).map_err(|Error| format!("Direct2D initialization failed: {}", Error.message()))?;
        let (PixelWidth, PixelHeight) = Platform::ClientSize(Window);
        Draw.Resize(PixelWidth, PixelHeight);
        let (Sink, Queue) = crate::Services::Workers::CreateEventChannel(Window);
        let Keys = Keymap::Load(&Paths.KeybindingsFile());
        let Media = Media::New(Sink.clone(), Settings.ThumbnailCacheMegabytes);
        let Watcher = Watcher::New(Sink.clone());
        let mut Modules = ModuleHost::New(&Paths);
        Modules.Discover();
        let Sort = SortSpec { Key: SortKey::Name, Descending: false, FoldersFirst: Settings.FoldersFirst };
        let ItemSize = Settings.ThumbnailSize;
        let NewPane = || Pane::New(Tab::New(Location::Computer, Settings.DefaultView, Sort, ItemSize));
        let Bench = crate::Services::Performance::UiBenchmark::FromArguments(Arguments);
        Platform::ApplyDarkFrame(Window, Look.IsDark);
        Platform::Menus::ApplyMenuTheme(Look.IsDark);
        let Scale = Dpi / 96.0;
        Ok(App {
            Window,
            Keys,
            Draw,
            Media,
            Hits: HitList::default(),
            Panes: vec![NewPane(), NewPane()],
            ActivePane: Session.ActivePane.min(1),
            Focus: Focus::List,
            Rename: None,
            Overlay: None,
            Pointer: PointerState { X: -1.0, Y: -1.0, Hover: Hit::Nothing, Drag: DragState::Idle, Tracking: false },
            DropHover: None,
            Sink,
            Queue,
            Watcher,
            Operations: Vec::new(),
            Purposes: HashMap::new(),
            Collisions: VecDeque::new(),
            Undo: UndoStack::default(),
            Modules,
            ShellMenus: None,
            ShellUsage: Default::default(),
            Performance: Performance::New(),
            Sidebar: SidebarState::default(),
            Inspector: InspectorState::default(),
            FolderSizes: None,
            KnownSizes: HashMap::new(),
            Notice: None,
            Background: Pool::New("Background", 3),
            TypeAhead: (Vec::new(), 0),
            Width: PixelWidth as f32 / Scale,
            Height: PixelHeight as f32 / Scale,
            ViewAreas: [Rect::default(); 2],
            Deferred: Vec::new(),
            Active: true,
            ScrollTimer: false,
            PerformanceTimer: false,
            Bench,
            Updating: false,
            PendingUpdate: None,
            Relaunch: None,
            ShellFresh: false,
            Motion: Motion::New(Settings.Animations),
            Logo: None,
            Theme: Look,
            Settings,
            Session,
            Paths,
        })
    }

    pub fn Visibility(&self) -> Visibility {
        Visibility { ShowHidden: self.Settings.ShowHidden, ShowSystem: self.Settings.ShowSystem }
    }

    pub fn DefaultSort(&self) -> SortSpec {
        SortSpec { Key: SortKey::Name, Descending: false, FoldersFirst: self.Settings.FoldersFirst }
    }

    fn Start(&mut self) {
        let Requested = self.Bench.as_ref().map(|Bench| Bench.Directory.clone());
        let Restore = self.Settings.RestoreSession && Requested.is_none();
        for Pane in 0..2 {
            let Saved = if Restore { self.Session.Tabs[Pane].clone() } else { Vec::new() };
            let Locations = if Saved.is_empty() {
                vec![match &Requested {
                    Some(Directory) => Location::Directory(Directory.clone()),
                    None => self.HomeLocation(),
                }]
            } else {
                Saved
            };
            let Sort = self.DefaultSort();
            self.Panes[Pane].Tabs = Locations.into_iter().map(|Item| Tab::New(Item, self.Settings.DefaultView, Sort, self.Settings.ThumbnailSize)).collect();
            self.Panes[Pane].Active = if Restore { self.Session.ActiveTabs[Pane].min(self.Panes[Pane].Tabs.len() - 1) } else { 0 };
            let Active = self.Panes[Pane].Active;
            self.LoadTab(Pane, Active);
        }
        if !self.Settings.DualPane {
            self.ActivePane = 0;
        }
        if let Some(Bench) = &mut self.Bench {
            Bench.Begin();
        }
        if self.Settings.ShowPerformance {
            self.SetPerformanceTimer(true);
        }
        self.SyncTree();
        self.StartFolderSizes();
    }

    pub fn HomeLocation(&self) -> Location {
        match std::env::var_os("USERPROFILE") {
            Some(Profile) => Location::Directory(PathBuf::from(Profile)),
            None => Location::Computer,
        }
    }

    pub fn Invalidate(&self) {
        Platform::Invalidate(self.Window);
    }

    pub fn Scale(&self) -> f32 {
        self.Draw.Dpi / 96.0
    }

    pub fn Inform(&mut self, Text: &str) {
        self.Notice = Some(Notice { Text: Text.to_string(), Error: false, Detail: None });
        self.Invalidate();
    }

    pub fn Fail(&mut self, Title: &str, Detail: &str) {
        self.Notice = Some(Notice { Text: format!("{Title}: {Detail}"), Error: true, Detail: Some(Detail.to_string()) });
        self.Invalidate();
    }

    pub fn SetPerformanceTimer(&mut self, Enabled: bool) {
        if Enabled && !self.PerformanceTimer {
            Platform::StartTimer(self.Window, Platform::TimerPerformance, 1000);
        } else if !Enabled && self.PerformanceTimer {
            Platform::StopTimer(self.Window, Platform::TimerPerformance);
        }
        self.PerformanceTimer = Enabled;
    }

    fn OnMessage(&mut self, Message: u32, WParameter: WPARAM, LParameter: LPARAM) -> Option<LRESULT> {
        let Scale = self.Scale();
        let PointX = ((LParameter.0 & 0xFFFF) as i16) as f32 / Scale;
        let PointY = (((LParameter.0 >> 16) & 0xFFFF) as i16) as f32 / Scale;
        match Message {
            WM_PAINT => {
                self.Paint();
                Some(LRESULT(0))
            }
            WM_SIZE => {
                self.SyncSize();
                self.Invalidate();
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                let Dpi = (WParameter.0 & 0xFFFF) as f32;
                self.Draw.SetDpi(Dpi);
                let Suggested = unsafe { &*(LParameter.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        self.Window,
                        None,
                        Suggested.left,
                        Suggested.top,
                        Suggested.right - Suggested.left,
                        Suggested.bottom - Suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                self.Invalidate();
                Some(LRESULT(0))
            }
            WakeMessage => {
                self.ProcessEvents();
                Some(LRESULT(0))
            }
            WM_TIMER => {
                self.OnTimer(WParameter.0);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                if !self.Pointer.Tracking {
                    let mut Track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.Window, dwHoverTime: 0 };
                    unsafe {
                        let _ = TrackMouseEvent(&mut Track);
                    }
                    self.Pointer.Tracking = true;
                }
                self.OnMouseMove(PointX, PointY);
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.Pointer.Tracking = false;
                if self.Pointer.Hover != Hit::Nothing {
                    self.Pointer.Hover = Hit::Nothing;
                    self.Invalidate();
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.OnMouseDown(0, PointX, PointY, Message == WM_LBUTTONDBLCLK);
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN => {
                self.OnMouseDown(1, PointX, PointY, false);
                Some(LRESULT(0))
            }
            WM_MBUTTONDOWN => {
                self.OnMouseDown(2, PointX, PointY, false);
                Some(LRESULT(0))
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                let Button = match Message {
                    WM_LBUTTONUP => 0,
                    WM_RBUTTONUP => 1,
                    _ => 2,
                };
                self.OnMouseUp(Button, PointX, PointY);
                Some(LRESULT(0))
            }
            WM_XBUTTONDOWN => {
                let Which = ((WParameter.0 >> 16) & 0xFFFF) as u16;
                self.Execute(if Which == XBUTTON1 { crate::Control::Commands::Command::Back } else { crate::Control::Commands::Command::Forward });
                Some(LRESULT(1))
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let Delta = ((WParameter.0 >> 16) & 0xFFFF) as i16 as f32;
                let (CursorX, CursorY) = Platform::CursorClientPosition(self.Window);
                self.OnWheel(Delta, Message == WM_MOUSEHWHEEL, CursorX as f32 / Scale, CursorY as f32 / Scale);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                if !matches!(self.Pointer.Drag, DragState::Idle) {
                    self.Pointer.Drag = DragState::Idle;
                    self.Invalidate();
                }
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                if (LParameter.0 & 0xFFFF) as u32 == HTCLIENT {
                    let Shape = match &self.Pointer.Drag {
                        DragState::Split { .. } | DragState::Column { .. } => crate::Interface::CursorShape::ResizeHorizontal,
                        _ => self.Pointer.Hover.Cursor(),
                    };
                    Platform::SetCursorShape(Shape);
                    return Some(LRESULT(1));
                }
                None
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let Handled = self.OnKeyDown(WParameter.0 as u16, Message == WM_SYSKEYDOWN);
                if Handled { Some(LRESULT(0)) } else { None }
            }
            WM_CHAR => {
                self.OnChar(WParameter.0 as u16);
                Some(LRESULT(0))
            }
            WM_SYSCHAR => {
                if self.Focus == Focus::List || self.Focus == Focus::Overlay {
                    return Some(LRESULT(0));
                }
                None
            }
            WM_ACTIVATE => {
                self.Active = (WParameter.0 & 0xFFFF) != 0;
                self.Invalidate();
                None
            }
            WM_SETTINGCHANGE => {
                if self.Settings.Theme.eq_ignore_ascii_case("System") {
                    self.ReloadTheme();
                }
                None
            }
            WM_ENDSESSION => {
                if WParameter.0 != 0 {
                    self.SaveState();
                }
                None
            }
            windows::Win32::UI::WindowsAndMessaging::WM_COPYDATA => {
                let Text = Platform::Integration::DecodeForward(LParameter)?;
                self.OpenExternal(&Text);
                Platform::Activate(self.Window);
                self.Invalidate();
                Some(LRESULT(1))
            }
            WM_CLOSE => {
                self.SaveState();
                if let Some(Service) = &self.FolderSizes {
                    Service.Shutdown();
                }
                for Pane in &mut self.Panes {
                    for Tab in &mut Pane.Tabs {
                        if let Some(Cancel) = &Tab.Cancel {
                            Cancel.Cancel();
                        }
                    }
                }
                for Running in &self.Operations {
                    Running.Cancel();
                }
                Platform::Clipboard::DragDrop::Revoke(self.Window);
                unsafe {
                    let _ = DestroyWindow(self.Window);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn OnTimer(&mut self, Identifier: usize) {
        match Identifier {
            Platform::TimerSmoothScroll => {
                let mut Animating = false;
                for Pane in &mut self.Panes {
                    let Tab = Pane.CurrentMut();
                    if Tab.Scroll.Step() {
                        Animating = true;
                    }
                }
                if self.Sidebar.Scroll.Step() || self.Motion.Animating() {
                    Animating = true;
                }
                if !Animating {
                    Platform::StopTimer(self.Window, Platform::TimerSmoothScroll);
                    self.ScrollTimer = false;
                }
                self.Invalidate();
            }
            Platform::TimerUpdate => self.UpdateTick(),
            Platform::TimerPerformance => {
                self.Performance.Sample(self.Media.CacheBytes());
                if self.BenchTick() {
                    Platform::Close(self.Window);
                }
                self.Invalidate();
            }
            _ => {}
        }
    }

    pub fn EnsureScrollTimer(&mut self) {
        let Animating = self.Panes.iter().any(|Pane| Pane.Current().Scroll.IsAnimating()) || self.Sidebar.Scroll.IsAnimating() || self.Motion.Animating();
        if Animating && !self.ScrollTimer {
            Platform::StartTimer(self.Window, Platform::TimerSmoothScroll, 8);
            self.ScrollTimer = true;
        }
    }

    fn SyncSize(&mut self) {
        let (Width, Height) = Platform::ClientSize(self.Window);
        if Width == 0 || Height == 0 {
            return;
        }
        if self.Draw.PixelSize() != (Width, Height) {
            self.Draw.Resize(Width, Height);
        }
        let Scale = self.Scale();
        self.Width = Width as f32 / Scale;
        self.Height = Height as f32 / Scale;
    }

    fn Paint(&mut self) {
        let mut Paint = PAINTSTRUCT::default();
        unsafe { BeginPaint(self.Window, &mut Paint) };
        self.SyncSize();
        let Started = Instant::now();
        self.Media.BeginFrame(&self.Draw);
        self.Motion.BeginFrame();
        if self.Draw.Begin(self.Theme.Colors.Background) {
            self.Hits.Clear();
            self.Compose();
            self.Media.EndFrame();
            if !self.Draw.End() {
                self.Invalidate();
            }
        }
        unsafe {
            let _ = EndPaint(self.Window, &Paint);
        }
        self.Performance.RecordFrame(Started.elapsed());
        if self.Bench.is_some() {
            self.BenchStep();
        }
        self.EnsureScrollTimer();
    }

    fn Compose(&mut self) {
        let Sizes = self.Theme.Sizes.clone();
        let mut Area = Rect::New(0.0, 0.0, self.Width, self.Height);
        let Title = Area.TakeTop(Sizes.TitleHeight);
        let Status = Area.TakeBottom(Sizes.StatusHeight);
        crate::Interface::Chrome::DrawTitleBar(self, Title);
        crate::Interface::Chrome::DrawStatusBar(self, Status);
        let SidebarShown = self.Motion.Watch(Channel::Sidebar, self.Settings.ShowSidebar as u64, 180, false);
        let SidebarFraction = if self.Settings.ShowSidebar { SidebarShown } else { 1.0 - SidebarShown };
        if SidebarFraction > 0.01 && Area.Width() > 420.0 {
            let Width = self.Settings.SidebarWidth.min(Area.Width() * 0.45);
            let Visible = (Width * SidebarFraction).round();
            let Bar = Area.TakeLeft(Visible);
            self.Draw.PushClip(Bar);
            crate::Interface::Chrome::Sidebar::DrawSidebar(self, Rect { Left: Bar.Right - Width, ..Bar });
            self.Draw.PopClip();
            self.Draw.VerticalLine(Bar.Right - 0.5, Bar.Top, Bar.Bottom, self.Theme.Colors.Border);
            if self.Settings.ShowSidebar {
                self.Hits.Push(Rect::New(Bar.Right - 2.0, Bar.Top, Sizes.SplitterWidth, Bar.Height()), Hit::Split(Splitter::Sidebar));
            }
        }
        let InspectorShown = self.Motion.Watch(Channel::Inspector, self.Settings.ShowInspector as u64, 180, false);
        let InspectorFraction = if self.Settings.ShowInspector { InspectorShown } else { 1.0 - InspectorShown };
        if InspectorFraction > 0.01 && Area.Width() > 520.0 {
            let Width = self.Settings.InspectorWidth.min(Area.Width() * 0.5);
            let Panel = Area.TakeRight((Width * InspectorFraction).round());
            self.Draw.PushClip(Panel);
            crate::Interface::Overlays::Inspector::DrawInspector(self, Rect { Right: Panel.Left + Width, ..Panel });
            self.Draw.PopClip();
            self.Draw.VerticalLine(Panel.Left, Panel.Top, Panel.Bottom, self.Theme.Colors.Border);
            if self.Settings.ShowInspector {
                self.Hits.Push(Rect::New(Panel.Left - 2.0, Panel.Top, Sizes.SplitterWidth, Panel.Height()), Hit::Split(Splitter::Inspector));
            }
        }
        if self.Settings.DualPane {
            let LeftWidth = (Area.Width() * self.Settings.PaneSplit).round();
            let mut Left = Area;
            Left.Right = Area.Left + LeftWidth;
            let mut Right = Area;
            Right.Left = Left.Right + 1.0;
            self.DrawPane(0, Left);
            self.DrawPane(1, Right);
            self.Draw.Fill(Rect::New(Left.Right, Area.Top, 1.0, Area.Height()), self.Theme.Colors.BorderStrong);
            self.Hits.Push(Rect::New(Left.Right - 2.0, Area.Top, Sizes.SplitterWidth, Area.Height()), Hit::Split(Splitter::Panes));
        } else {
            self.DrawPane(0, Area);
            self.ViewAreas[1] = Rect::default();
        }
        crate::Interface::Overlays::DrawOverlay(self);
    }

    fn DrawPane(&mut self, Pane: usize, Area: Rect) {
        let mut Remaining = Area;
        let Header = Remaining.TakeTop(self.Theme.Sizes.TabHeight + self.Theme.Sizes.ToolbarHeight);
        crate::Interface::Chrome::DrawPaneHeader(self, Pane, Header);
        self.ViewAreas[Pane] = Remaining;
        crate::Interface::FileView::DrawFileView(self, Pane, Remaining);
        if self.Settings.DualPane && Pane == self.ActivePane {
            self.Draw.Fill(Rect::New(Area.Left, Area.Top, Area.Width(), 2.0), self.Theme.Colors.Accent);
        }
    }

    fn ProcessEvents(&mut self) {
        let Items: Vec<Event> = self.Queue.Drain().collect();
        if Items.is_empty() {
            return;
        }
        for Item in Items {
            self.HandleEvent(Item);
        }
        self.Invalidate();
    }

    fn HandleEvent(&mut self, Item: Event) {
        match Item {
            Event::ListingPartial { Token, Chunk } => self.OnListingPartial(Token, Chunk),
            Event::ListingLoaded { Token, Result, Elapsed } => self.OnListingLoaded(Token, Result, Elapsed),
            Event::SearchBatch { Token, Chunk } => self.OnSearchBatch(Token, Chunk),
            Event::SearchDone { Token, Statistics } => self.OnSearchDone(Token, Statistics),
            Event::DirectoryChanged(Batch) => self.OnDirectoryChanged(Batch),
            Event::Media(Result) => self.Media.Accept(Result, &self.Draw),
            Event::Operation(Update) => self.OnOperation(Update),
            Event::FreeSpace { Directory, Value } => {
                self.Sidebar.AcceptSpace(&Directory, Value);
                for Pane in &mut self.Panes {
                    if Pane.Current().Location.DirectoryPath().is_some_and(|Current| Current.starts_with(&Directory)) {
                        Pane.FreeSpace = Value;
                    }
                }
            }
            Event::Completions { Pane, Token, Items } => {
                if let Some(Target) = self.Panes.get_mut(Pane) {
                    if Target.CompletionToken == Token {
                        Target.Completions = Items;
                        Target.CompletionIndex = None;
                    }
                }
            }
            Event::TreeChildren { Directory, Children } => self.Sidebar.AcceptChildren(Directory, Children),
            Event::CrumbChildren { Pane, Base, Children, X, Y } => self.ShowCrumbMenu(Pane, Base, Children, X, Y),
            Event::Preview { Token, Content } => self.Inspector.Accept(Token, Content),
            Event::FolderSize { Token, Bytes, Files, Folders, Done } => self.Inspector.AcceptSize(Token, Bytes, Files, Folders, Done),
            Event::Module(Update) => self.OnModuleEvent(Update),
            Event::ShellMenu(Update) => self.AcceptShellMenu(Update),
            Event::FolderSizes(Results) => self.AcceptFolderSizes(Results),
            Event::Update(Outcome) => self.OnUpdate(Outcome),
            Event::Reveal(Item, Keep) => {
                self.RevealItem(&Item, Keep);
                Platform::Activate(self.Window);
            }
        }
    }

    pub fn ReloadTheme(&mut self) {
        self.Theme = Theme::Load(&self.Settings.Theme, self.Settings.Density, &self.Paths.ThemesDirectory());
        let _ = self.Draw.RebuildFormats(&self.Settings.FontFamily, &self.Theme.Sizes);
        Platform::ApplyDarkFrame(self.Window, self.Theme.IsDark);
        Platform::Menus::ApplyMenuTheme(self.Theme.IsDark);
        self.Invalidate();
    }

    pub fn SaveSettings(&mut self) {
        if let Err(Error) = self.Settings.Save(&self.Paths.SettingsFile()) {
            self.Fail("Settings could not be saved", &Error.to_string());
        }
    }

    pub fn SaveState(&mut self) {
        if self.Bench.is_some() {
            return;
        }
        for Pane in 0..2 {
            self.Session.Tabs[Pane] = self.Panes[Pane].Tabs.iter().map(|Tab| Tab.Location.clone()).collect();
            self.Session.ActiveTabs[Pane] = self.Panes[Pane].Active;
        }
        self.Session.ActivePane = self.ActivePane;
        self.Session.Window = Platform::CurrentPlacement(self.Window);
        let _ = self.Session.Save(&self.Paths.SessionFile());
        let _ = self.Settings.Save(&self.Paths.SettingsFile());
    }
}
