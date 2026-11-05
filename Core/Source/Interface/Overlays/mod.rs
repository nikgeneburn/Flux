pub mod Inspector;
pub mod Menu;

use super::Widgets::{EditOutcome, LineEdit};
use super::{Align, Hit, Rect, Style};
use super::Widgets::Motion::Channel;
use crate::Application::{App, Focus};
use crate::Control::Commands::{Builtins, Command};
use crate::FileSystem::Search::FuzzyScore;
use crate::FileSystem::Sorting::{FoldString, Fold};
use crate::FileSystem::{LeafName, Location, Resolve};
use crate::Services::Modules::ModuleState;
use std::path::PathBuf;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_TAB, VK_UP};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteMode {
    Commands,
    GoTo,
}

#[derive(Clone, Debug)]
pub enum PaletteAction {
    Run(Command),
    Open(Location),
}

#[derive(Clone, Debug)]
pub struct PaletteItem {
    pub Title: String,
    pub Detail: String,
    pub Shortcut: String,
    pub Action: PaletteAction,
    pub Score: i32,
}

pub struct Palette {
    pub Mode: PaletteMode,
    pub Input: LineEdit,
    pub Items: Vec<PaletteItem>,
    pub Selected: usize,
    pub First: usize,
}

pub enum PromptPurpose {
    DeletePermanently(Vec<PathBuf>),
    Collision(u64),
    ModuleArgument(u32),
    ApproveModule(usize),
    TestCrash,
    Information,
}

pub struct Prompt {
    pub Title: String,
    pub Message: String,
    pub Input: Option<LineEdit>,
    pub Buttons: Vec<String>,
    pub Default: usize,
    pub Toggle: Option<(String, bool)>,
    pub Purpose: PromptPurpose,
    pub Danger: bool,
    pub Scroll: f32,
}

#[derive(Default)]
pub struct ModulesView {
    pub Selected: usize,
}

pub enum Overlay {
    Palette(Palette),
    Prompt(Prompt),
    Modules(ModulesView),
    Menu(Menu::MenuState),
    Settings(Menu::Settings::SettingsView),
}

impl Prompt {
    pub fn New(Title: &str, Message: &str, Buttons: &[&str], Purpose: PromptPurpose) -> Prompt {
        Prompt {
            Title: Title.to_string(),
            Message: Message.to_string(),
            Input: None,
            Buttons: Buttons.iter().map(|Text| Text.to_string()).collect(),
            Default: 0,
            Toggle: None,
            Purpose,
            Danger: false,
            Scroll: 0.0,
        }
    }

    pub fn WithInput(mut self, Initial: &str) -> Prompt {
        let mut Edit = LineEdit::New();
        Edit.SetText(Initial);
        Edit.SelectAll();
        self.Input = Some(Edit);
        self
    }

    pub fn WithToggle(mut self, Label: &str) -> Prompt {
        self.Toggle = Some((Label.to_string(), false));
        self
    }

    pub fn Dangerous(mut self) -> Prompt {
        self.Danger = true;
        self
    }
}

const VisibleRows: usize = 12;

impl Palette {
    pub fn New(Mode: PaletteMode) -> Palette {
        Palette { Mode, Input: LineEdit::New(), Items: Vec::new(), Selected: 0, First: 0 }
    }

    fn Candidates(&self, App: &App) -> Vec<PaletteItem> {
        let mut Output = Vec::new();
        match self.Mode {
            PaletteMode::Commands => {
                for Entry in Builtins {
                    Output.push(PaletteItem {
                        Title: Entry.Title.to_string(),
                        Detail: Entry.Identifier.to_string(),
                        Shortcut: App.Keys.ShortcutText(Entry.Item).to_string(),
                        Action: PaletteAction::Run(Entry.Item),
                        Score: 0,
                    });
                }
                for (Index, Item) in App.Modules.Commands() {
                    Output.push(PaletteItem {
                        Title: Item.Title.clone(),
                        Detail: format!("Module · {}", App.Modules.Entries()[Item.Module].Manifest.Name),
                        Shortcut: String::new(),
                        Action: PaletteAction::Run(Command::Module(Index)),
                        Score: 0,
                    });
                }
            }
            PaletteMode::GoTo => {
                let mut Seen: Vec<String> = Vec::new();
                let mut Add = |Output: &mut Vec<PaletteItem>, Item: PathBuf, Detail: &str| {
                    let Key = Item.to_string_lossy().to_lowercase();
                    if Seen.contains(&Key) {
                        return;
                    }
                    Seen.push(Key);
                    Output.push(PaletteItem { Title: LeafName(&Item), Detail: format!("{} · {}", Detail, Item.display()), Shortcut: String::new(), Action: PaletteAction::Open(Location::Directory(Item)), Score: 0 });
                };
                let Text = self.Input.Value();
                if Text.contains('\\') || Text.contains(':') || Text.contains('%') {
                    if let Some(Target) = Resolve(&Text, App.CurrentDirectory(App.ActivePane).as_deref()) {
                        Output.push(PaletteItem { Title: format!("Go to {}", Target.display()), Detail: "Path".to_string(), Shortcut: String::new(), Action: PaletteAction::Open(Location::Directory(Target)), Score: 1000 });
                    }
                }
                for Favorite in &App.Session.Favorites {
                    Add(&mut Output, Favorite.clone(), "Favorite");
                }
                for Recent in &App.Session.Recent {
                    Add(&mut Output, Recent.clone(), "Recent");
                }
                for (Name, Folder) in crate::Platform::Shell::KnownFolders() {
                    Add(&mut Output, Folder, &Name);
                }
                let Tab = App.Tab(App.ActivePane);
                if let Some(Root) = &Tab.Listing.Root {
                    for &Index in Tab.Order.iter().filter(|&&Index| Tab.Listing.IsDirectory(Index)).take(5000) {
                        Add(&mut Output, Root.join(std::ffi::OsString::from(Tab.Listing.NameString(Index))), "Subfolder");
                    }
                }
                Output.push(PaletteItem { Title: "This PC".to_string(), Detail: "Drives".to_string(), Shortcut: String::new(), Action: PaletteAction::Open(Location::Computer), Score: 0 });
            }
        }
        Output
    }

    pub fn Refresh(&mut self, App: &App) {
        let Text = self.Input.Value();
        let Pattern = FoldString(Text.trim());
        let mut Items = self.Candidates(App);
        if Pattern.is_empty() {
            self.Items = Items;
        } else {
            let Spaceless: Vec<u16> = Pattern.iter().copied().filter(|&Unit| Unit != b' ' as u16).collect();
            Items.retain_mut(|Item| {
                if Item.Score >= 1000 {
                    return true;
                }
                let Title: Vec<u16> = Item.Title.encode_utf16().collect();
                let Detail: Vec<u16> = Item.Detail.encode_utf16().collect();
                let Best = FuzzyScore(&Spaceless, &Title).map(|Score| Score + 20).max(FuzzyScore(&Spaceless, &Detail).map(|Score| Score - 30));
                match Best {
                    Some(Score) => {
                        Item.Score = Score;
                        true
                    }
                    None => false,
                }
            });
            Items.sort_by(|Left, Right| Right.Score.cmp(&Left.Score).then_with(|| Left.Title.len().cmp(&Right.Title.len())));
            self.Items = Items;
        }
        self.Selected = 0;
        self.First = 0;
    }

    fn Clamp(&mut self) {
        if self.Items.is_empty() {
            self.Selected = 0;
            self.First = 0;
            return;
        }
        self.Selected = self.Selected.min(self.Items.len() - 1);
        if self.Selected < self.First {
            self.First = self.Selected;
        }
        if self.Selected >= self.First + VisibleRows {
            self.First = self.Selected + 1 - VisibleRows;
        }
    }
}

fn RefreshPalette(App: &mut App) {
    if let Some(Overlay::Palette(mut State)) = App.Overlay.take() {
        State.Refresh(App);
        App.Overlay = Some(Overlay::Palette(State));
    }
}

fn RunPaletteItem(App: &mut App, Index: usize) {
    let Some(Overlay::Palette(State)) = App.Overlay.take() else { return };
    App.Focus = Focus::List;
    let Some(Item) = State.Items.get(Index).cloned() else { return };
    match Item.Action {
        PaletteAction::Run(Target) => App.Execute(Target),
        PaletteAction::Open(Target) => {
            let Pane = App.ActivePane;
            App.Navigate(Pane, Target);
        }
    }
    App.Invalidate();
}

fn PressPromptButton(App: &mut App, Button: Option<usize>) {
    let Some(Overlay::Prompt(State)) = App.Overlay.take() else { return };
    App.Focus = Focus::List;
    App.OnPromptResult(State, Button);
}

fn ModuleAction(App: &mut App, Row: usize, Action: u8) {
    match Action {
        0 => {
            let Some(Entry) = App.Modules.Entries().get(Row) else { return };
            let Manifest = &Entry.Manifest;
            let Hash = App.Modules.ComputeHash(Row).unwrap_or_else(|Error| format!("unavailable: {Error}"));
            let Declared = match &Manifest.DeclaredHash {
                Some(Value) if Value.eq_ignore_ascii_case(&Hash) => "matches the publisher's declared SHA-256".to_string(),
                Some(_) => "DOES NOT match the publisher's declared SHA-256 — approval will be refused".to_string(),
                None => "no publisher hash declared".to_string(),
            };
            let Message = format!(
                "{} {} by {}\n{}\n\nLibrary: {}\nSHA-256: {}\nIntegrity: {}\n\nModules run native code with your user account's permissions. Only trust modules from sources you trust. The library is verified against this hash every time it loads.",
                Manifest.Name,
                Manifest.Version,
                if Manifest.Author.is_empty() { "unknown author" } else { &Manifest.Author },
                Manifest.Description,
                Manifest.Directory.join(&Manifest.Library).display(),
                Hash,
                Declared
            );
            let Title = format!("Trust module {}?", Manifest.Name);
            App.ShowPrompt(Prompt::New(&Title, &Message, &["Trust and Enable", "Cancel"], PromptPurpose::ApproveModule(Row)));
        }
        1 => App.Modules.Disable(Row),
        2 => {
            if let Err(Message) = App.Modules.Enable(Row) {
                App.Fail("Module could not be enabled", &Message);
            }
        }
        3 => App.Modules.Revoke(Row),
        100 => {
            App.CloseOverlay();
            App.Execute(Command::InstallModule);
        }
        101 => {
            App.Modules.Discover();
        }
        _ => App.CloseOverlay(),
    }
    App.Invalidate();
}

pub fn OverlayButton(App: &mut App, Target: Hit) {
    match Target {
        Hit::PaletteRow(Index) => RunPaletteItem(App, Index as usize),
        Hit::PromptButton(Index) => PressPromptButton(App, Some(Index as usize)),
        Hit::PromptToggle => {
            if let Some(Overlay::Prompt(State)) = &mut App.Overlay {
                if let Some((_, Value)) = &mut State.Toggle {
                    *Value = !*Value;
                }
            }
        }
        Hit::ModuleAction { Row, Action } => ModuleAction(App, Row as usize, Action),
        Hit::MenuRow { Level, Index } => Menu::Activate(App, Level, Index as usize),
        Hit::SettingsRow(Index) => Menu::Settings::SettingsClick(App, Index),
        Hit::Completion { Pane, Index } => {
            let Pane = Pane as usize;
            if let Some(Text) = App.Panes[Pane].Completions.get(Index as usize).cloned() {
                App.Panes[Pane].Address.SetText(&Text);
                App.SubmitAddress(Pane);
            }
        }
        _ => {}
    }
    App.Invalidate();
}

pub fn OverlayKey(App: &mut App, Key: u16, Control: bool, Shift: bool) -> bool {
    let Window = App.Window;
    match &mut App.Overlay {
        Some(Overlay::Palette(State)) => {
            let Page = VisibleRows - 1;
            match Key {
                Key if Key == VK_DOWN.0 || (Key == VK_TAB.0 && !Shift) => State.Selected = (State.Selected + 1).min(State.Items.len().saturating_sub(1)),
                Key if Key == VK_UP.0 || (Key == VK_TAB.0 && Shift) => State.Selected = State.Selected.saturating_sub(1),
                Key if Key == VK_NEXT.0 => State.Selected = (State.Selected + Page).min(State.Items.len().saturating_sub(1)),
                Key if Key == VK_PRIOR.0 => State.Selected = State.Selected.saturating_sub(Page),
                _ => match State.Input.HandleKey(Key, Control, Shift, Window) {
                    EditOutcome::Submit => {
                        let Selected = State.Selected;
                        RunPaletteItem(App, Selected);
                        return true;
                    }
                    EditOutcome::Cancel => {
                        App.CloseOverlay();
                        return true;
                    }
                    EditOutcome::Changed => {
                        RefreshPalette(App);
                        App.Invalidate();
                        return true;
                    }
                    EditOutcome::Handled => {}
                    EditOutcome::Unhandled => return Control,
                },
            }
            State.Clamp();
        }
        Some(Overlay::Prompt(State)) => {
            let Count = State.Buttons.len();
            if Key == VK_LEFT.0 && State.Input.is_none() {
                State.Default = State.Default.saturating_sub(1);
            } else if Key == VK_RIGHT.0 && State.Input.is_none() {
                State.Default = (State.Default + 1).min(Count.saturating_sub(1));
            } else if Key == VK_TAB.0 {
                State.Default = if Shift { (State.Default + Count - 1) % Count } else { (State.Default + 1) % Count };
            } else {
                let Outcome = match &mut State.Input {
                    Some(Edit) => Edit.HandleKey(Key, Control, Shift, Window),
                    None => match Key {
                        0x0D => EditOutcome::Submit,
                        0x1B => EditOutcome::Cancel,
                        0x20 => EditOutcome::Submit,
                        _ if Control && Key == b'C' as u16 => {
                            crate::Platform::Clipboard::SetText(Window, &format!("{}\r\n\r\n{}", State.Title, State.Message));
                            EditOutcome::Handled
                        }
                        _ => EditOutcome::Unhandled,
                    },
                };
                match Outcome {
                    EditOutcome::Submit => {
                        let Button = State.Default;
                        PressPromptButton(App, Some(Button));
                    }
                    EditOutcome::Cancel => PressPromptButton(App, None),
                    _ => {}
                }
            }
        }
        Some(Overlay::Menu(_)) => return Menu::MenuKey(App, Key),
        Some(Overlay::Settings(View)) => match Key {
            0x1B => App.CloseOverlay(),
            Key if Key == VK_DOWN.0 => View.Scroll += 46.0,
            Key if Key == VK_UP.0 => View.Scroll = (View.Scroll - 46.0).max(0.0),
            _ => {}
        },
        Some(Overlay::Modules(State)) => {
            let Count = App.Modules.Entries().len();
            match Key {
                Key if Key == VK_DOWN.0 => State.Selected = (State.Selected + 1).min(Count.saturating_sub(1)),
                Key if Key == VK_UP.0 => State.Selected = State.Selected.saturating_sub(1),
                0x1B => App.CloseOverlay(),
                0x0D => {
                    let Row = State.Selected;
                    if let Some(Entry) = App.Modules.Entries().get(Row) {
                        let Action = match Entry.State {
                            ModuleState::Enabled => 1,
                            ModuleState::Disabled => 2,
                            _ => 0,
                        };
                        ModuleAction(App, Row, Action);
                    }
                }
                _ => {}
            }
        }
        None => return false,
    }
    App.Invalidate();
    true
}

pub fn OverlayChar(App: &mut App, Unit: u16) {
    match &mut App.Overlay {
        Some(Overlay::Palette(State)) => {
            if State.Input.HandleChar(Unit) == EditOutcome::Changed {
                RefreshPalette(App);
            }
        }
        Some(Overlay::Prompt(State)) => {
            if let Some(Edit) = &mut State.Input {
                Edit.HandleChar(Unit);
            }
        }
        _ => {}
    }
}

pub fn OverlayClick(App: &mut App, Target: Hit, Area: Rect, X: f32, Shift: bool) {
    match (&mut App.Overlay, Target) {
        (Some(Overlay::Palette(State)), Hit::PaletteInput) => State.Input.ClickAt(&App.Draw, Area, X, Shift),
        (Some(Overlay::Prompt(State)), Hit::PromptInput) => {
            if let Some(Edit) = &mut State.Input {
                Edit.ClickAt(&App.Draw, Area, X, Shift);
            }
        }
        _ => {}
    }
}

pub fn DragText(App: &mut App, Target: Hit, X: f32) {
    let Some(Area) = App.Hits.AreaOf(Target) else { return };
    match Target {
        Hit::Address { Pane } => App.Panes[Pane as usize].Address.ClickAt(&App.Draw, Area, X, true),
        Hit::Filter { Pane } => {
            let Draw = &App.Draw;
            App.Panes[Pane as usize].CurrentMut().Filter.ClickAt(Draw, Area, X, true);
        }
        Hit::PaletteInput | Hit::PromptInput => OverlayClick(App, Target, Area, X, true),
        _ => {}
    }
}

pub fn OverlayWheel(App: &mut App, Delta: f32) {
    match &mut App.Overlay {
        Some(Overlay::Palette(State)) => {
            let Steps = (Delta / 120.0 * 3.0) as isize;
            let Maximum = State.Items.len().saturating_sub(VisibleRows);
            State.First = (State.First as isize - Steps).clamp(0, Maximum as isize) as usize;
            State.Selected = State.Selected.clamp(State.First, (State.First + VisibleRows - 1).min(State.Items.len().saturating_sub(1)));
        }
        Some(Overlay::Prompt(State)) => State.Scroll = (State.Scroll - Delta / 120.0 * 48.0).max(0.0),
        Some(Overlay::Settings(_)) => Menu::Settings::SettingsWheel(App, Delta),
        _ => {}
    }
    App.Invalidate();
}

fn DrawCompletions(App: &mut App) {
    let Pane = App.ActivePane;
    if !App.EditingAddress(Pane) || App.Panes[Pane].Completions.is_empty() {
        return;
    }
    let Some(Field) = App.Hits.AreaOf(Hit::Address { Pane: Pane as u8 }) else { return };
    let Colors = App.Theme.Colors.clone();
    let RowHeight = App.Theme.Sizes.RowHeight;
    let Items = App.Panes[Pane].Completions.clone();
    let Box = Rect::New(Field.Left, Field.Bottom + 2.0, Field.Width(), Items.len() as f32 * RowHeight + 8.0);
    App.Draw.FillRounded(Box.Inset(-1.0, -1.0), App.Theme.Sizes.Radius, Colors.Scrim.WithAlpha(0.2));
    App.Draw.FillRounded(Box, App.Theme.Sizes.Radius, Colors.Raised);
    App.Draw.StrokeRounded(Box, App.Theme.Sizes.Radius, Colors.Border, 1.0);
    App.Hits.Push(Box, Hit::Overlay);
    for (Index, Text) in Items.iter().enumerate() {
        let Row = Rect::New(Box.Left + 4.0, Box.Top + 4.0 + Index as f32 * RowHeight, Box.Width() - 8.0, RowHeight);
        let Target = Hit::Completion { Pane: Pane as u8, Index: Index as u16 };
        if App.Panes[Pane].CompletionIndex == Some(Index) || App.Pointer.Hover == Target {
            App.Draw.FillRounded(Row, 2.0, Colors.Selection);
        }
        App.Draw.Glyph(0xE8B7, Style::IconSmall, Rect::New(Row.Left + 4.0, Row.Top, 18.0, Row.Height()), Colors.TextMuted);
        App.Draw.Label(Text, Style::Body, Rect { Left: Row.Left + 28.0, ..Row }, Colors.Text, Align::Left);
        App.Hits.Push(Row, Target);
    }
}

fn DrawPalette(App: &mut App, Window: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    let RowHeight = Look.Sizes.RowHeight + 8.0;
    let Width = (Window.Width() * 0.5).clamp(420.0, 680.0);
    let Some(Overlay::Palette(State)) = &mut App.Overlay else { return };
    let Count = State.Items.len().min(VisibleRows);
    let Height = 44.0 + Count.max(1) as f32 * RowHeight + 10.0;
    let Box = Rect::New(Window.Left + (Window.Width() - Width) / 2.0, Window.Top + 64.0, Width, Height);
    App.Draw.FillRounded(Box.Inset(-6.0, -6.0), Look.Sizes.Radius + 4.0, Colors.Scrim.WithAlpha(0.18));
    App.Draw.FillRounded(Box, Look.Sizes.Radius + 1.0, Colors.Raised);
    App.Draw.StrokeRounded(Box, Look.Sizes.Radius + 1.0, Colors.BorderStrong, 1.0);
    App.Hits.Push(Box, Hit::Overlay);
    let Field = Rect::New(Box.Left + 10.0, Box.Top + 8.0, Box.Width() - 20.0, 30.0);
    let Placeholder = if State.Mode == PaletteMode::Commands { "Type a command" } else { "Folder name, favorite, recent or path" };
    State.Input.Draw(&mut App.Draw, &Look, Field, true, Placeholder);
    App.Hits.Push(Field, Hit::PaletteInput);
    if State.Items.is_empty() {
        App.Draw.Label("No matches", Style::Body, Rect::New(Box.Left, Field.Bottom + 6.0, Box.Width(), RowHeight), Colors.TextMuted, Align::Center);
        return;
    }
    let Pattern: Vec<u16> = FoldString(&State.Input.Value());
    for Visible in 0..Count {
        let Index = State.First + Visible;
        let Some(Item) = State.Items.get(Index) else { break };
        let Row = Rect::New(Box.Left + 6.0, Field.Bottom + 6.0 + Visible as f32 * RowHeight, Box.Width() - 12.0, RowHeight);
        let Target = Hit::PaletteRow(Index as u32);
        if Index == State.Selected {
            App.Draw.FillRounded(Row, Look.Sizes.Radius, Colors.Selection);
            App.Draw.Fill(Rect::New(Row.Left, Row.Top + 6.0, 2.0, Row.Height() - 12.0), Colors.Accent);
        } else if App.Pointer.Hover == Target {
            App.Draw.FillRounded(Row, Look.Sizes.Radius, Colors.Hover);
        }
        let Text = Rect { Left: Row.Left + 12.0, Right: Row.Right - 120.0, ..Row };
        let Upper = Rect { Bottom: Row.Top + Row.Height() * 0.58, ..Text };
        let Lower = Rect { Top: Row.Top + Row.Height() * 0.5, ..Text };
        let Highlight = !Pattern.is_empty() && Item.Title.encode_utf16().map(Fold).collect::<Vec<u16>>().windows(Pattern.len()).any(|Window| Window == Pattern.as_slice());
        App.Draw.Label(&Item.Title, if Highlight { Style::Strong } else { Style::Body }, Upper, Colors.Text, Align::Left);
        App.Draw.Label(&Item.Detail, Style::Small, Lower, Colors.TextFaint, Align::Left);
        if !Item.Shortcut.is_empty() {
            let Width = App.Draw.MeasureString(&Item.Shortcut, Style::Small) + 12.0;
            let Chip = Rect::New(Row.Right - Width - 8.0, Row.Top + (Row.Height() - 20.0) / 2.0, Width, 20.0);
            App.Draw.StrokeRounded(Chip, 2.0, Colors.Border, 1.0);
            App.Draw.Label(&Item.Shortcut, Style::Small, Chip, Colors.TextMuted, Align::Center);
        }
        App.Hits.Push(Row, Target);
    }
}

fn DrawPrompt(App: &mut App, Window: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    let Width = (Window.Width() * 0.5).clamp(380.0, 600.0);
    let Some(Overlay::Prompt(State)) = &mut App.Overlay else { return };
    let MessageWide: Vec<u16> = State.Message.encode_utf16().collect();
    let TextWidth = Width - 40.0;
    let Natural = App.Draw.WrappedHeight(&MessageWide, TextWidth, false);
    let MessageHeight = Natural.min(Window.Height() * 0.55);
    let InputHeight = if State.Input.is_some() { 40.0 } else { 0.0 };
    let ToggleHeight = if State.Toggle.is_some() { 30.0 } else { 0.0 };
    let Height = 52.0 + MessageHeight + InputHeight + ToggleHeight + 56.0;
    let Box = Rect::New(Window.Left + (Window.Width() - Width) / 2.0, Window.Top + ((Window.Height() - Height) / 2.5).max(40.0), Width, Height);
    App.Draw.FillRounded(Box.Inset(-6.0, -6.0), Look.Sizes.Radius + 4.0, Colors.Scrim.WithAlpha(0.18));
    App.Draw.FillRounded(Box, Look.Sizes.Radius + 1.0, Colors.Raised);
    App.Draw.StrokeRounded(Box, Look.Sizes.Radius + 1.0, Colors.BorderStrong, 1.0);
    App.Hits.Push(Box, Hit::Overlay);
    if State.Danger {
        App.Draw.Fill(Rect::New(Box.Left, Box.Top, Box.Width(), 2.0), Colors.Danger);
    }
    App.Draw.Label(&State.Title, Style::Heading, Rect::New(Box.Left + 20.0, Box.Top + 14.0, Box.Width() - 40.0, 26.0), Colors.Text, Align::Left);
    let MessageArea = Rect::New(Box.Left + 20.0, Box.Top + 50.0, TextWidth, MessageHeight);
    State.Scroll = State.Scroll.min((Natural - MessageHeight).max(0.0));
    App.Draw.PushClip(MessageArea);
    App.Draw.Wrapped(&MessageWide, Rect { Top: MessageArea.Top - State.Scroll, Bottom: MessageArea.Top - State.Scroll + Natural + 4.0, ..MessageArea }, Colors.TextMuted, false);
    App.Draw.PopClip();
    let mut Y = MessageArea.Bottom + 10.0;
    if let Some(Edit) = &mut State.Input {
        let Field = Rect::New(Box.Left + 20.0, Y, Box.Width() - 40.0, 30.0);
        Edit.Draw(&mut App.Draw, &Look, Field, true, "");
        App.Hits.Push(Field, Hit::PromptInput);
        Y += InputHeight;
    }
    if let Some((Label, Value)) = &State.Toggle {
        let Row = Rect::New(Box.Left + 20.0, Y, Box.Width() - 40.0, 24.0);
        let Check = Rect::New(Row.Left, Row.Top + 4.0, 16.0, 16.0);
        App.Draw.FillRounded(Check, 2.0, if *Value { Colors.Accent } else { Colors.Input });
        App.Draw.StrokeRounded(Check, 2.0, if *Value { Colors.Accent } else { Colors.BorderStrong }, 1.0);
        if *Value {
            App.Draw.Glyph(0xE73E, Style::IconSmall, Check, Colors.AccentText);
        }
        App.Draw.Label(Label, Style::Body, Rect { Left: Check.Right + 8.0, ..Row }, Colors.Text, Align::Left);
        App.Hits.Push(Row, Hit::PromptToggle);
    }
    let mut X = Box.Right - 20.0;
    for Index in (0..State.Buttons.len()).rev() {
        let Label = State.Buttons[Index].clone();
        let Width = App.Draw.MeasureString(&Label, Style::Body) + 28.0;
        let Cell = Rect::New(X - Width, Box.Bottom - 44.0, Width, 30.0);
        X -= Width + 8.0;
        let Target = Hit::PromptButton(Index as u8);
        let Primary = Index == State.Default;
        let Fill = if Primary { if State.Danger { Colors.Danger } else { Colors.Accent } } else { Colors.Input };
        App.Draw.FillRounded(Cell, Look.Sizes.Radius, if App.Pointer.Hover == Target { Fill.Mix(Colors.Text, 0.1) } else { Fill });
        if !Primary {
            App.Draw.StrokeRounded(Cell, Look.Sizes.Radius, Colors.Border, 1.0);
        }
        App.Draw.Label(&Label, Style::Body, Cell, if Primary { Colors.AccentText } else { Colors.Text }, Align::Center);
        App.Hits.Push(Cell, Target);
    }
}

fn DrawModules(App: &mut App, Window: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    let Width = (Window.Width() * 0.6).clamp(480.0, 760.0);
    let RowHeight = 64.0;
    let Count = App.Modules.Entries().len();
    let Height = (64.0 + Count.max(1) as f32 * RowHeight + 60.0).min(Window.Height() - 80.0);
    let Box = Rect::New(Window.Left + (Window.Width() - Width) / 2.0, Window.Top + 50.0, Width, Height);
    App.Draw.FillRounded(Box.Inset(-6.0, -6.0), Look.Sizes.Radius + 4.0, Colors.Scrim.WithAlpha(0.18));
    App.Draw.FillRounded(Box, Look.Sizes.Radius + 1.0, Colors.Raised);
    App.Draw.StrokeRounded(Box, Look.Sizes.Radius + 1.0, Colors.BorderStrong, 1.0);
    App.Hits.Push(Box, Hit::Overlay);
    App.Draw.Label("Modules", Style::Heading, Rect::New(Box.Left + 20.0, Box.Top + 14.0, 200.0, 26.0), Colors.Text, Align::Left);
    let Folder = App.Paths.InstalledModulesDirectory().display().to_string();
    App.Draw.Label(&Folder, Style::Small, Rect::New(Box.Left + 120.0, Box.Top + 16.0, Box.Width() - 140.0, 24.0), Colors.TextFaint, Align::Right);
    let Selected = match &App.Overlay {
        Some(Overlay::Modules(State)) => State.Selected,
        _ => 0,
    };
    if Count == 0 {
        App.Draw.Label("No modules found. Install one from a folder containing Module.ini.", Style::Body, Rect::New(Box.Left, Box.Top + 60.0, Box.Width(), 40.0), Colors.TextMuted, Align::Center);
    }
    let List = Rect::New(Box.Left + 12.0, Box.Top + 54.0, Box.Width() - 24.0, Box.Height() - 54.0 - 56.0);
    App.Draw.PushClip(List);
    for Row in 0..Count {
        let Entry = &App.Modules.Entries()[Row];
        let Area = Rect::New(List.Left, List.Top + Row as f32 * RowHeight, List.Width(), RowHeight - 4.0);
        if Row == Selected {
            App.Draw.FillRounded(Area, Look.Sizes.Radius, Colors.SelectionInactive);
        }
        let (StateText, StateColor, Actions): (String, _, Vec<(u8, &str)>) = match &Entry.State {
            ModuleState::Untrusted => ("Not trusted".to_string(), Colors.Warning, vec![(0, "Review & Trust")]),
            ModuleState::Modified => ("Library changed since approval".to_string(), Colors.Danger, vec![(0, "Review & Trust"), (3, "Revoke")]),
            ModuleState::Enabled => (if Entry.Loaded { "Enabled · loaded".to_string() } else { "Enabled · not loaded".to_string() }, Colors.Success, vec![(1, "Disable")]),
            ModuleState::Disabled => ("Disabled".to_string(), Colors.TextMuted, vec![(2, "Enable"), (3, "Revoke")]),
            ModuleState::Incompatible(Reason) => (format!("Incompatible: {Reason}"), Colors.Danger, vec![]),
            ModuleState::Failed(Reason) => (format!("Failed: {Reason}"), Colors.Danger, vec![(2, "Retry"), (3, "Revoke")]),
        };
        let Title = format!("{}  {}", Entry.Manifest.Name, Entry.Manifest.Version);
        App.Draw.Label(&Title, Style::Strong, Rect::New(Area.Left + 12.0, Area.Top + 6.0, Area.Width() - 260.0, 20.0), Colors.Text, Align::Left);
        let Detail = format!("{} · {} command(s){}", Entry.Manifest.Description, Entry.Manifest.Commands.len(), if Entry.Manifest.PreviewExtensions.is_empty() { "" } else { " · preview" });
        App.Draw.Label(&Detail, Style::Small, Rect::New(Area.Left + 12.0, Area.Top + 24.0, Area.Width() - 260.0, 18.0), Colors.TextMuted, Align::Left);
        App.Draw.Label(&StateText, Style::Small, Rect::New(Area.Left + 12.0, Area.Top + 40.0, Area.Width() - 260.0, 18.0), StateColor, Align::Left);
        let mut X = Area.Right - 10.0;
        for (Action, Label) in Actions.iter().rev() {
            let Width = App.Draw.MeasureString(Label, Style::Small) + 22.0;
            let Cell = Rect::New(X - Width, Area.Top + (Area.Height() - 26.0) / 2.0, Width, 26.0);
            X -= Width + 6.0;
            let Target = Hit::ModuleAction { Row: Row as u16, Action: *Action };
            let Primary = *Action == 0 || *Action == 2;
            let Fill = if Primary { Colors.Accent } else { Colors.Input };
            App.Draw.FillRounded(Cell, Look.Sizes.Radius, if App.Pointer.Hover == Target { Fill.Mix(Colors.Text, 0.1) } else { Fill });
            App.Draw.StrokeRounded(Cell, Look.Sizes.Radius, if Primary { Colors.Accent } else { Colors.Border }, 1.0);
            App.Draw.Label(Label, Style::Small, Cell, if Primary { Colors.AccentText } else { Colors.Text }, Align::Center);
            App.Hits.Push(Cell, Target);
        }
    }
    App.Draw.PopClip();
    let Footer = [(100u8, "Install from folder…"), (101u8, "Rescan"), (102u8, "Close")];
    let mut X = Box.Right - 20.0;
    for (Action, Label) in Footer.iter().rev() {
        let Width = App.Draw.MeasureString(Label, Style::Body) + 26.0;
        let Cell = Rect::New(X - Width, Box.Bottom - 44.0, Width, 30.0);
        X -= Width + 8.0;
        let Target = Hit::ModuleAction { Row: 0, Action: *Action };
        App.Draw.FillRounded(Cell, Look.Sizes.Radius, if App.Pointer.Hover == Target { Colors.Hover.Mix(Colors.Input, 0.5) } else { Colors.Input });
        App.Draw.StrokeRounded(Cell, Look.Sizes.Radius, Colors.Border, 1.0);
        App.Draw.Label(Label, Style::Body, Cell, Colors.Text, Align::Center);
        App.Hits.Push(Cell, Target);
    }
}

pub fn DrawOverlay(App: &mut App) {
    DrawCompletions(App);
    let Kind = match &App.Overlay {
        None => 0u64,
        Some(Overlay::Palette(_)) => 1,
        Some(Overlay::Prompt(_)) => 2,
        Some(Overlay::Modules(_)) => 3,
        Some(Overlay::Menu(_)) => 4,
        Some(Overlay::Settings(_)) => 5,
    };
    let Reveal = App.Motion.Watch(Channel::Overlay, Kind, 170, true);
    if Kind == 0 {
        return;
    }
    let Window = Rect::New(0.0, 0.0, App.Width, App.Height);
    let Lift = if Kind == 4 { 0.0 } else { (1.0 - Reveal) * 14.0 };
    let Shifted = Rect { Top: Window.Top + Lift, Bottom: Window.Bottom + Lift, ..Window };
    if Kind != 1 && Kind != 4 {
        let Scrim = App.Theme.Colors.Scrim;
        App.Draw.Fill(Window, Scrim.WithAlpha(Scrim.A * Reveal));
    }
    App.Hits.Push(Window, Hit::Nothing);
    match Kind {
        1 => DrawPalette(App, Shifted),
        2 => DrawPrompt(App, Shifted),
        3 => DrawModules(App, Shifted),
        4 => Menu::DrawMenu(App),
        _ => Menu::Settings::DrawSettings(App, Shifted),
    }
    if Reveal < 1.0 {
        if let Some(Box) = App.Hits.AreaOf(Hit::Overlay) {
            let Raised = App.Theme.Colors.Raised;
            App.Draw.FillRounded(Box, App.Theme.Sizes.Radius + 1.0, Raised.WithAlpha((1.0 - Reveal) * 0.92));
        }
    }
}
