use super::super::Overlay;
use crate::Application::{App, Deferred, Focus};
use crate::Control::Commands::Command;
use crate::Interface::Theme::Theme;
use crate::Interface::{Align, Hit, Rect, Style};
use crate::Platform::{Integration, Shell};
use crate::Services::Config::ViewMode;

#[derive(Default)]
pub struct SettingsView {
    pub Scroll: f32,
}

#[derive(Clone, Copy, Debug)]
enum SettingAction {
    Run(Command),
    Theme,
    Density,
    DefaultView,
    ShowSystem,
    RestoreSession,
    CrashMonitor,
    FullDump,
    ThumbnailCache,
    Terminal,
    Keybindings,
    SettingsFile,
    Themes,
    Crashes,
    DefaultManager,
    ReuseWindow,
    FolderSizes,
    ShellInline,
}

enum Control {
    Toggle(bool),
    Choice(String),
    Button(&'static str),
}

struct SettingRow {
    Section: Option<&'static str>,
    Label: &'static str,
    Detail: String,
    Value: Control,
    Action: SettingAction,
}

fn Row(Section: Option<&'static str>, Label: &'static str, Detail: &str, Value: Control, Action: SettingAction) -> SettingRow {
    SettingRow { Section, Label, Detail: Detail.to_string(), Value, Action }
}

fn TerminalLabel(App: &App) -> String {
    match App.Settings.TerminalCommand.strip_prefix("terminal:") {
        Some(Label) => Label.to_string(),
        None if App.Settings.TerminalCommand.trim().is_empty() => "Automatic".to_string(),
        None => "Custom command".to_string(),
    }
}

fn Rows(App: &App) -> Vec<SettingRow> {
    let Settings = &App.Settings;
    vec![
        Row(Some("Appearance"), "Theme", "Dark, Light, System, or a theme file from the Themes folder", Control::Choice(Settings.Theme.clone()), SettingAction::Theme),
        Row(None, "Density", "Row height and spacing", Control::Choice(format!("{:?}", Settings.Density)), SettingAction::Density),
        Row(None, "Animations", "Panel slides, fades and hover transitions", Control::Toggle(Settings.Animations), SettingAction::Run(Command::ToggleAnimations)),
        Row(None, "Smooth scrolling", "Interpolate wheel scrolling", Control::Toggle(Settings.SmoothScrolling), SettingAction::Run(Command::ToggleSmoothScrolling)),
        Row(Some("Files"), "Show hidden items", "Items with the hidden attribute", Control::Toggle(Settings.ShowHidden), SettingAction::Run(Command::ToggleHidden)),
        Row(None, "Show protected system files", "Hidden items that also carry the system attribute", Control::Toggle(Settings.ShowSystem), SettingAction::ShowSystem),
        Row(None, "Folders first", "Keep folders above files when sorting", Control::Toggle(Settings.FoldersFirst), SettingAction::Run(Command::ToggleFoldersFirst)),
        Row(None, "Folder sizes", "Total size of everything inside each folder, cached and kept up to date", Control::Toggle(Settings.FolderSizes), SettingAction::FolderSizes),
        Row(None, "App actions in right-click menu", "On: 7-Zip, Git and other app actions appear directly, most used first. Off: grouped under App actions", Control::Toggle(Settings.ShellActionsInline), SettingAction::ShellInline),
        Row(None, "Default view", "Layout used for new tabs", Control::Choice(Settings.DefaultView.Name().to_string()), SettingAction::DefaultView),
        Row(Some("Layout"), "Sidebar", "Favorites, recent locations and drives", Control::Toggle(Settings.ShowSidebar), SettingAction::Run(Command::ToggleSidebar)),
        Row(None, "Folder tree", "Expandable folder hierarchy in the sidebar", Control::Toggle(Settings.ShowFolderTree), SettingAction::Run(Command::ToggleFolderTree)),
        Row(None, "Inspector", "Preview and details panel", Control::Toggle(Settings.ShowInspector), SettingAction::Run(Command::ToggleInspector)),
        Row(None, "Dual pane", "Two independent panes side by side", Control::Toggle(Settings.DualPane), SettingAction::Run(Command::ToggleDualPane)),
        Row(Some("Behavior"), "Restore last session", "Reopen tabs and window placement on start", Control::Toggle(Settings.RestoreSession), SettingAction::RestoreSession),
        Row(None, "Default terminal", "Used by Open Terminal Here", Control::Choice(TerminalLabel(App)), SettingAction::Terminal),
        Row(None, "Thumbnail cache", "Memory budget for decoded thumbnails", Control::Choice(format!("{} MB", Settings.ThumbnailCacheMegabytes)), SettingAction::ThumbnailCache),
        Row(None, "Performance overlay", "Frame time, memory and CPU in the status bar", Control::Toggle(Settings.ShowPerformance), SettingAction::Run(Command::TogglePerformance)),
        Row(Some("Windows integration"), "Default file manager", "Open folders, drives and Win+E in Flux instead of File Explorer", Control::Toggle(IsDefaultManager()), SettingAction::DefaultManager),
        Row(None, "Open folders in existing window", "Folders opened from other apps become a new tab", Control::Toggle(Settings.ReuseWindow), SettingAction::ReuseWindow),
        Row(Some("Diagnostics"), "Crash reporter process", "Out-of-process crash reports (applies after restart)", Control::Toggle(Settings.CrashMonitor), SettingAction::CrashMonitor),
        Row(None, "Full memory dumps", "Larger dumps that may contain file names and contents", Control::Toggle(Settings.FullMemoryDump), SettingAction::FullDump),
        Row(None, "Crash reports", "Open the crash report folder", Control::Button("Open"), SettingAction::Crashes),
        Row(Some("Files and shortcuts"), "Keyboard shortcuts", "Edit Keybindings.ini; reload with Ctrl+Shift+R", Control::Button("Edit"), SettingAction::Keybindings),
        Row(None, "Themes folder", "Create custom themes as .ini files", Control::Button("Open"), SettingAction::Themes),
        Row(None, "Settings file", "All options in Settings.ini", Control::Button("Edit"), SettingAction::SettingsFile),
    ]
}

impl App {
    pub fn OpenSettingsPanel(&mut self) {
        self.Overlay = Some(Overlay::Settings(SettingsView::default()));
        self.Focus = Focus::Overlay;
        self.Invalidate();
    }

    fn ApplySetting(&mut self, Action: SettingAction) {
        match Action {
            SettingAction::Run(Item) => {
                self.Execute(Item);
                return;
            }
            SettingAction::Theme => {
                self.Execute(Command::CycleTheme);
                return;
            }
            SettingAction::Density => {
                self.Execute(Command::CycleDensity);
                return;
            }
            SettingAction::DefaultView => {
                self.Settings.DefaultView = match self.Settings.DefaultView {
                    ViewMode::Details => ViewMode::List,
                    ViewMode::List => ViewMode::Grid,
                    ViewMode::Grid => ViewMode::Thumbnails,
                    ViewMode::Thumbnails => ViewMode::Details,
                };
            }
            SettingAction::ShowSystem => {
                self.Settings.ShowSystem = !self.Settings.ShowSystem;
                self.RefilterAll();
            }
            SettingAction::RestoreSession => self.Settings.RestoreSession = !self.Settings.RestoreSession,
            SettingAction::ReuseWindow => self.Settings.ReuseWindow = !self.Settings.ReuseWindow,
            SettingAction::ShellInline => self.Settings.ShellActionsInline = !self.Settings.ShellActionsInline,
            SettingAction::FolderSizes => {
                self.Settings.FolderSizes = !self.Settings.FolderSizes;
                if self.Settings.FolderSizes {
                    self.StartFolderSizes();
                } else {
                    self.StopFolderSizes();
                }
            }
            SettingAction::DefaultManager => {
                let Enable = !IsDefaultManager();
                let Result = match std::env::current_exe() {
                    Ok(Executable) if Enable => Integration::Register(&Executable),
                    Ok(_) => Integration::Unregister(),
                    Err(Error) => Err(Error.to_string()),
                };
                match Result {
                    Ok(()) if Enable => self.Inform("Flux is now the default file manager"),
                    Ok(()) => self.Inform("File Explorer restored as the default file manager"),
                    Err(Message) => self.Fail("Could not change the default file manager", &Message),
                }
            }
            SettingAction::CrashMonitor => self.Settings.CrashMonitor = !self.Settings.CrashMonitor,
            SettingAction::FullDump => self.Settings.FullMemoryDump = !self.Settings.FullMemoryDump,
            SettingAction::ThumbnailCache => {
                let Steps = [32u32, 64, 96, 192, 384, 768];
                let Next = Steps.iter().copied().find(|&Step| Step > self.Settings.ThumbnailCacheMegabytes).unwrap_or(Steps[0]);
                self.Settings.ThumbnailCacheMegabytes = Next;
                self.Media.SetBudget(Next);
            }
            SettingAction::Terminal => {
                let Available = Shell::AvailableTerminals();
                let Current = TerminalLabel(self);
                let Position = Available.iter().position(|Kind| Kind.Label() == Current);
                self.Settings.TerminalCommand = match Position {
                    None if Current == "Automatic" => Available.first().map(|Kind| format!("terminal:{}", Kind.Label())).unwrap_or_default(),
                    None => String::new(),
                    Some(Index) if Index + 1 < Available.len() => format!("terminal:{}", Available[Index + 1].Label()),
                    Some(_) => String::new(),
                };
            }
            SettingAction::Keybindings => {
                self.Deferred.push(Deferred::Open(self.Paths.KeybindingsFile()));
                return;
            }
            SettingAction::SettingsFile => {
                self.SaveSettings();
                self.Deferred.push(Deferred::Open(self.Paths.SettingsFile()));
                return;
            }
            SettingAction::Themes => {
                self.CloseOverlay();
                self.Execute(Command::OpenThemesFolder);
                return;
            }
            SettingAction::Crashes => {
                self.CloseOverlay();
                self.Execute(Command::OpenCrashFolder);
                return;
            }
        }
        self.SaveSettings();
        self.Invalidate();
    }
}

fn IsDefaultManager() -> bool {
    std::env::current_exe().is_ok_and(|Executable| Integration::IsRegistered(&Executable))
}

pub fn SettingsClick(App: &mut App, Index: u16) {
    if Index == u16::MAX {
        App.CloseOverlay();
        return;
    }
    let Actions: Vec<SettingAction> = Rows(App).into_iter().map(|Item| Item.Action).collect();
    if let Some(Action) = Actions.get(Index as usize).copied() {
        App.ApplySetting(Action);
    }
}

pub fn SettingsWheel(App: &mut App, Delta: f32) {
    if let Some(Overlay::Settings(View)) = &mut App.Overlay {
        View.Scroll = (View.Scroll - Delta / 120.0 * 60.0).max(0.0);
    }
}

fn Switch(App: &mut App, Look: &Theme, Area: Rect, Value: bool) {
    let Colors = &Look.Colors;
    let Track = Rect::New(Area.Right - 36.0, Area.Top + (Area.Height() - 18.0) / 2.0, 34.0, 18.0);
    App.Draw.FillRounded(Track, 9.0, if Value { Colors.Accent } else { Colors.Input });
    App.Draw.StrokeRounded(Track, 9.0, if Value { Colors.Accent } else { Colors.BorderStrong }, 1.0);
    let Knob = if Value { Track.Right - 15.0 } else { Track.Left + 3.0 };
    App.Draw.FillRounded(Rect::New(Knob, Track.Top + 3.0, 12.0, 12.0), 6.0, if Value { Colors.AccentText } else { Colors.TextMuted });
}

pub fn DrawSettings(App: &mut App, Window: Rect) {
    let Look = App.Theme.clone();
    let Colors = Look.Colors.clone();
    let Width = (Window.Width() * 0.7).clamp(460.0, 660.0);
    let Height = (Window.Height() - 80.0).clamp(240.0, 720.0);
    let Panel = Rect::New(Window.Left + (Window.Width() - Width) / 2.0, Window.Top + (Window.Height() - Height) / 2.0, Width, Height);
    App.Draw.FillRounded(Panel.Inset(-6.0, -6.0), Look.Sizes.Radius + 4.0, Colors.Scrim.WithAlpha(0.18));
    App.Draw.FillRounded(Panel, Look.Sizes.Radius + 1.0, Colors.Raised);
    App.Draw.StrokeRounded(Panel, Look.Sizes.Radius + 1.0, Colors.BorderStrong, 1.0);
    App.Hits.Push(Panel, Hit::Overlay);
    App.Draw.Label("Settings", Style::Heading, Rect::New(Panel.Left + 22.0, Panel.Top + 14.0, 200.0, 28.0), Colors.Text, Align::Left);
    App.Draw.Label("Changes apply and save immediately", Style::Small, Rect::New(Panel.Left + 22.0, Panel.Top + 40.0, 300.0, 16.0), Colors.TextFaint, Align::Left);
    let Close = Rect::New(Panel.Right - 40.0, Panel.Top + 12.0, 28.0, 28.0);
    let CloseTarget = Hit::SettingsRow(u16::MAX);
    if App.Pointer.Hover == CloseTarget {
        App.Draw.FillRounded(Close, Look.Sizes.Radius, Colors.Hover);
    }
    App.Draw.Glyph(0xE8BB, Style::IconSmall, Close, Colors.TextMuted);
    App.Hits.Push(Close, CloseTarget);
    let List = Rect::New(Panel.Left + 10.0, Panel.Top + 64.0, Panel.Width() - 20.0, Panel.Height() - 74.0);
    let Items = Rows(App);
    let ContentHeight: f32 = Items.iter().map(|Item| if Item.Section.is_some() { 74.0 } else { 46.0 }).sum();
    let Scroll = match &mut App.Overlay {
        Some(Overlay::Settings(View)) => {
            View.Scroll = View.Scroll.min((ContentHeight - List.Height()).max(0.0));
            View.Scroll
        }
        _ => 0.0,
    };
    App.Draw.PushClip(List);
    let mut Y = List.Top - Scroll;
    for (Index, Item) in Items.iter().enumerate() {
        if let Some(Section) = Item.Section {
            App.Draw.Label(&Section.to_uppercase(), Style::Small, Rect::New(List.Left + 12.0, Y + 10.0, List.Width(), 18.0), Colors.Accent, Align::Left);
            Y += 28.0;
        }
        let Line = Rect::New(List.Left, Y, List.Width(), 44.0);
        Y += 46.0;
        if Line.Bottom < List.Top || Line.Top > List.Bottom {
            continue;
        }
        let Target = Hit::SettingsRow(Index as u16);
        if App.Pointer.Hover == Target {
            App.Draw.FillRounded(Line, Look.Sizes.Radius, Colors.Hover);
        }
        App.Draw.Label(Item.Label, Style::Body, Rect::New(Line.Left + 12.0, Line.Top + 4.0, Line.Width() - 200.0, 20.0), Colors.Text, Align::Left);
        App.Draw.Label(&Item.Detail, Style::Small, Rect::New(Line.Left + 12.0, Line.Top + 23.0, Line.Width() - 200.0, 16.0), Colors.TextFaint, Align::Left);
        let Control = Rect { Left: Line.Right - 190.0, Right: Line.Right - 12.0, ..Line };
        match &Item.Value {
            Control::Toggle(Value) => Switch(App, &Look, Control, *Value),
            Control::Choice(Value) => {
                let Width = App.Draw.MeasureString(Value, Style::Body) + 38.0;
                let Chip = Rect::New(Control.Right - Width, Control.Top + 9.0, Width, 26.0);
                App.Draw.FillRounded(Chip, Look.Sizes.Radius, Colors.Input);
                App.Draw.StrokeRounded(Chip, Look.Sizes.Radius, Colors.Border, 1.0);
                App.Draw.Label(Value, Style::Body, Rect { Left: Chip.Left + 10.0, Right: Chip.Right - 24.0, ..Chip }, Colors.Text, Align::Left);
                App.Draw.Glyph(0xE8AB, Style::IconSmall, Rect::New(Chip.Right - 22.0, Chip.Top, 16.0, Chip.Height()), Colors.TextMuted);
            }
            Control::Button(Label) => {
                let Width = App.Draw.MeasureString(Label, Style::Body) + 28.0;
                let Chip = Rect::New(Control.Right - Width, Control.Top + 9.0, Width, 26.0);
                App.Draw.FillRounded(Chip, Look.Sizes.Radius, Colors.Input);
                App.Draw.StrokeRounded(Chip, Look.Sizes.Radius, Colors.Border, 1.0);
                App.Draw.Label(Label, Style::Body, Chip, Colors.Text, Align::Center);
            }
        }
        App.Hits.Push(Line, Target);
    }
    App.Draw.PopClip();
}
