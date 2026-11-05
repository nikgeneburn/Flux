use super::Commands::{Chord, Command};
use crate::Application::{App, DragState, DropHover, Focus};
use crate::FileSystem::{IsServerPath, Location, Resolve};
use crate::FileSystem::Enumerate;
use crate::FileSystem::Sorting::{Fold, StartsWithFolded};
use crate::Interface::FileView;
use crate::Interface::Widgets::{EditOutcome, OffsetForThumb};
use crate::Interface::{Hit, Splitter};
use crate::Platform::Clipboard::DragDrop::{DropAnswer, DropPhase, DropRequest};
use crate::Platform::Clipboard::{EffectCopy, EffectMove};
use crate::Platform::{self, Modifiers};
use crate::Services::Config::ColumnKind;
use std::path::PathBuf;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::GetMessageTime;

fn IsNavigationKey(Key: u16) -> bool {
    [VK_UP.0, VK_DOWN.0, VK_LEFT.0, VK_RIGHT.0, VK_PRIOR.0, VK_NEXT.0, VK_HOME.0, VK_END.0].contains(&Key)
}

fn IsButtonHit(Target: Hit) -> bool {
    matches!(
        Target,
        Hit::Run(_)
            | Hit::Minimize
            | Hit::Maximize
            | Hit::Close
            | Hit::TabClose { .. }
            | Hit::TabNew { .. }
            | Hit::Crumb { .. }
            | Hit::CrumbMenu { .. }
            | Hit::PromptButton(_)
            | Hit::PromptToggle
            | Hit::ModuleAction { .. }
            | Hit::OperationCancel(_)
            | Hit::PaletteRow(_)
            | Hit::InspectorAction(_)
            | Hit::SidebarRow(_)
            | Hit::FilterClear { .. }
            | Hit::Completion { .. }
            | Hit::MenuRow { .. }
            | Hit::SettingsRow(_)
            | Hit::Header { .. }
            | Hit::StatusMessage
    )
}

impl App {
    pub fn OnKeyDown(&mut self, Key: u16, System: bool) -> bool {
        let (Control, Shift, Alt) = Modifiers();
        let Pane = self.ActivePane;
        match self.Focus {
            Focus::Overlay if self.Overlay.is_some() => {
                return crate::Interface::Overlays::OverlayKey(self, Key, Control, Shift);
            }
            Focus::Rename => {
                let Window = self.Window;
                if let Some(State) = &mut self.Rename {
                    match State.Edit.HandleKey(Key, Control, Shift, Window) {
                        EditOutcome::Submit => self.CommitRename(),
                        EditOutcome::Cancel => self.CancelRename(),
                        EditOutcome::Unhandled if Key == VK_TAB.0 => self.CommitRename(),
                        EditOutcome::Unhandled => return false,
                        _ => {}
                    }
                    self.Invalidate();
                    return true;
                }
                self.Focus = Focus::List;
            }
            Focus::Address => {
                if self.AddressKey(Key, Control, Shift) {
                    self.Invalidate();
                    return true;
                }
            }
            Focus::Filter => {
                let Window = self.Window;
                let Outcome = self.TabMut(Pane).Filter.HandleKey(Key, Control, Shift, Window);
                match Outcome {
                    EditOutcome::Changed => self.ApplyFilter(Pane),
                    EditOutcome::Submit => self.StartSearch(Pane),
                    EditOutcome::Cancel => {
                        if self.Tab(Pane).Filter.IsEmpty() {
                            self.Focus = Focus::List;
                        } else {
                            self.TabMut(Pane).Filter.Clear();
                            self.ApplyFilter(Pane);
                        }
                    }
                    EditOutcome::Unhandled if Key == VK_DOWN.0 => self.Focus = Focus::List,
                    EditOutcome::Unhandled => {}
                    EditOutcome::Handled => {}
                }
                if Outcome != EditOutcome::Unhandled || Key == VK_DOWN.0 {
                    self.Invalidate();
                    return true;
                }
                if !(Control || Alt || (0x70..0x88).contains(&Key)) {
                    return true;
                }
            }
            _ => {}
        }
        if self.Focus == Focus::List && IsNavigationKey(Key) && !Alt {
            self.MoveCursor(Pane, Key, Control, Shift);
            return true;
        }
        if self.Focus == Focus::List && Key == VK_SPACE.0 && Control {
            let Tab = self.TabMut(Pane);
            if let Some(Cursor) = Tab.Cursor {
                let Selected = Tab.IsSelected(Cursor);
                Tab.SetSelected(Cursor, !Selected);
                Tab.Anchor = Some(Cursor);
            }
            self.RefreshInspector();
            self.Invalidate();
            return true;
        }
        let Pressed = Chord { Key, Control, Shift, Alt };
        if let Some(Item) = self.Keys.Resolve(Pressed) {
            if Item == Command::SwitchPane && !self.Settings.DualPane {
                return false;
            }
            if Item == Command::ClearSelection && self.Tab(Pane).SelectedCount == 0 {
                if !self.Tab(Pane).Filter.IsEmpty() {
                    self.TabMut(Pane).Filter.Clear();
                    self.ApplyFilter(Pane);
                }
                return true;
            }
            self.Execute(Item);
            if Item == Command::Exit {
                return true;
            }
            self.RefreshInspector();
            return true;
        }
        let _ = System;
        false
    }

    fn AddressKey(&mut self, Key: u16, Control: bool, Shift: bool) -> bool {
        let Pane = self.ActivePane;
        let Count = self.Panes[Pane].Completions.len();
        if Key == VK_DOWN.0 || Key == VK_UP.0 {
            if Count > 0 {
                let Current = self.Panes[Pane].CompletionIndex;
                let Next = match (Current, Key == VK_DOWN.0) {
                    (None, true) => 0,
                    (None, false) => Count - 1,
                    (Some(Index), true) => (Index + 1) % Count,
                    (Some(Index), false) => (Index + Count - 1) % Count,
                };
                self.Panes[Pane].CompletionIndex = Some(Next);
                let Text = self.Panes[Pane].Completions[Next].clone();
                self.Panes[Pane].Address.SetText(&Text);
            }
            return true;
        }
        if Key == VK_TAB.0 {
            let Choice = self.Panes[Pane].CompletionIndex.unwrap_or(0);
            if let Some(Text) = self.Panes[Pane].Completions.get(Choice).cloned() {
                self.Panes[Pane].Address.SetText(&format!("{Text}\\"));
                self.RequestCompletions(Pane);
            }
            return true;
        }
        let Window = self.Window;
        match self.Panes[Pane].Address.HandleKey(Key, Control, Shift, Window) {
            EditOutcome::Submit => {
                self.SubmitAddress(Pane);
                true
            }
            EditOutcome::Cancel => {
                self.Panes[Pane].Completions.clear();
                self.Focus = Focus::List;
                true
            }
            EditOutcome::Changed => {
                self.RequestCompletions(Pane);
                true
            }
            EditOutcome::Handled => true,
            EditOutcome::Unhandled => !(Control || (0x70..0x88).contains(&Key)),
        }
    }

    pub fn SubmitAddress(&mut self, Pane: usize) {
        let Text = self.Panes[Pane].Address.Value();
        self.Panes[Pane].Completions.clear();
        self.Focus = Focus::List;
        let Trimmed = Text.trim();
        if Trimmed.is_empty() {
            return;
        }
        if Trimmed.eq_ignore_ascii_case("this pc") || Trimmed.eq_ignore_ascii_case("computer") {
            self.Navigate(Pane, Location::Computer);
            return;
        }
        let Base = self.CurrentDirectory(Pane);
        let Some(Target) = Resolve(Trimmed, Base.as_deref()) else {
            self.Fail("Invalid path", Trimmed);
            return;
        };
        if IsServerPath(&Target) {
            self.Navigate(Pane, Location::Directory(Target));
            return;
        }
        match Enumerate::Stat(&Target) {
            Some(Data) if Data.Attributes & crate::FileSystem::AttributeDirectory != 0 => self.Navigate(Pane, Location::Directory(Target)),
            Some(Data) => {
                if let Some(Parent) = Target.parent() {
                    self.TabMut(Pane).PendingFocus = Some(Data.Name.clone());
                    self.Navigate(Pane, Location::Directory(Parent.to_path_buf()));
                    self.TabMut(Pane).PendingFocus = Some(Data.Name);
                }
            }
            None => {
                if Target.parent().is_none() && Target.to_string_lossy().len() <= 3 {
                    self.Navigate(Pane, Location::Directory(Target));
                } else {
                    self.Fail("Location not found", &Target.display().to_string());
                }
            }
        }
    }

    fn MoveCursor(&mut self, Pane: usize, Key: u16, Control: bool, Shift: bool) {
        if self.Tab(Pane).Order.is_empty() {
            return;
        }
        let Current = self.Tab(Pane).CursorPosition();
        let Next = match Current {
            Some(Position) => FileView::Step(self, Pane, Position, Key),
            None => 0,
        };
        let Tab = self.TabMut(Pane);
        let Index = Tab.Order[Next];
        if Shift {
            let AnchorPosition = Tab.Anchor.and_then(|Anchor| Tab.PositionOf(Anchor)).unwrap_or(Next);
            if !Control {
                Tab.ClearSelection();
            }
            Tab.SelectRange(AnchorPosition, Next);
        } else if !Control {
            Tab.ClearSelection();
            Tab.SetSelected(Index, true);
            Tab.Anchor = Some(Index);
        }
        Tab.Cursor = Some(Index);
        self.RevealIndex(Pane, Index);
        self.RefreshInspector();
        self.Invalidate();
    }

    pub fn OnChar(&mut self, Unit: u16) {
        let Pane = self.ActivePane;
        match self.Focus {
            Focus::Overlay => crate::Interface::Overlays::OverlayChar(self, Unit),
            Focus::Rename => {
                if let Some(State) = &mut self.Rename {
                    State.Edit.HandleChar(Unit);
                }
            }
            Focus::Address => {
                if self.Panes[Pane].Address.HandleChar(Unit) == EditOutcome::Changed {
                    self.RequestCompletions(Pane);
                }
            }
            Focus::Filter => {
                if self.TabMut(Pane).Filter.HandleChar(Unit) == EditOutcome::Changed {
                    self.ApplyFilter(Pane);
                }
            }
            Focus::List => self.TypeAhead(Pane, Unit),
        }
        self.Invalidate();
    }

    fn TypeAhead(&mut self, Pane: usize, Unit: u16) {
        if Unit < 32 || Unit == 127 || (Unit == b' ' as u16 && self.TypeAhead.0.is_empty()) {
            return;
        }
        let Now = unsafe { GetMessageTime() } as u32;
        if Now.wrapping_sub(self.TypeAhead.1) > 900 {
            self.TypeAhead.0.clear();
        }
        self.TypeAhead.1 = Now;
        self.TypeAhead.0.push(Fold(Unit));
        let Prefix = self.TypeAhead.0.clone();
        let Repeated = Prefix.len() > 1 && Prefix.iter().all(|&Item| Item == Prefix[0]);
        let Needle = if Repeated { vec![Prefix[0]] } else { Prefix.clone() };
        let Tab = self.Tab(Pane);
        let Count = Tab.Order.len();
        if Count == 0 {
            return;
        }
        let Start = Tab.CursorPosition().map(|Position| if Needle.len() == 1 { Position + 1 } else { Position }).unwrap_or(0);
        let Found = (0..Count).map(|Offset| (Start + Offset) % Count).find(|&Position| StartsWithFolded(Tab.Listing.Name(Tab.Order[Position]), &Needle));
        if let Some(Position) = Found {
            let Tab = self.TabMut(Pane);
            let Index = Tab.Order[Position];
            Tab.ClearSelection();
            Tab.SetSelected(Index, true);
            Tab.Cursor = Some(Index);
            Tab.Anchor = Some(Index);
            self.RevealIndex(Pane, Index);
            self.RefreshInspector();
        }
    }

    fn PaneOf(&self, Target: Hit) -> Option<usize> {
        match Target {
            Hit::Tab { Pane, .. }
            | Hit::TabClose { Pane, .. }
            | Hit::TabNew { Pane }
            | Hit::TabStrip { Pane }
            | Hit::Crumb { Pane, .. }
            | Hit::CrumbMenu { Pane, .. }
            | Hit::Address { Pane }
            | Hit::Filter { Pane }
            | Hit::FilterClear { Pane }
            | Hit::Header { Pane, .. }
            | Hit::HeaderResize { Pane, .. }
            | Hit::List { Pane }
            | Hit::ScrollThumb { Pane }
            | Hit::ScrollTrack { Pane } => Some(Pane as usize),
            _ => None,
        }
    }

    pub fn OnMouseDown(&mut self, Button: u8, X: f32, Y: f32, Double: bool) {
        let (Target, Area) = self.Hits.Find(X, Y);
        let EditingBefore = (self.Focus == Focus::Address).then_some(self.ActivePane);
        let (Control, Shift, _) = Modifiers();
        self.Pointer.X = X;
        self.Pointer.Y = Y;
        if let Some(Pane) = self.PaneOf(Target) {
            if Pane != self.ActivePane && Pane < 2 {
                self.ActivePane = Pane;
                self.CancelRename();
                self.RefreshInspector();
                self.SyncTree();
            }
        }
        if self.Rename.is_some() && !matches!(Target, Hit::List { .. }) {
            self.CommitRename();
        }
        let Pane = self.ActivePane;
        if let Some(Editing) = EditingBefore.filter(|_| !matches!(Target, Hit::Address { .. } | Hit::Completion { .. } | Hit::Overlay)) {
            self.Panes[Editing].Completions.clear();
            self.Focus = Focus::List;
        }
        if self.Focus == Focus::Filter && !matches!(Target, Hit::Filter { .. }) && self.Overlay.is_none() {
            self.Focus = Focus::List;
        }
        Platform::Capture(self.Window);
        if self.Overlay.is_some()
            && !matches!(Target, Hit::PaletteRow(_) | Hit::PaletteInput | Hit::PromptButton(_) | Hit::PromptInput | Hit::PromptToggle | Hit::ModuleAction { .. } | Hit::MenuRow { .. } | Hit::SettingsRow(_) | Hit::Overlay)
        {
            if !matches!(self.Overlay, Some(crate::Interface::Overlays::Overlay::Prompt(_))) {
                self.CloseOverlay();
            }
            return;
        }
        if Button == 0 && IsButtonHit(Target) {
            self.Pointer.Drag = DragState::Idle;
            self.Pointer.Hover = Target;
            self.Invalidate();
            return;
        }
        match (Button, Target) {
            (0, Hit::Tab { Pane: TabPane, Index }) => {
                self.ActivateTab(TabPane as usize, Index as usize);
                self.Pointer.Drag = DragState::TabMove { Pane: TabPane as usize, Index: Index as usize, StartX: X, Moved: false };
            }
            (0, Hit::TabStrip { Pane: StripPane }) if Double => self.Execute(if StripPane as usize == Pane { Command::NewTab } else { Command::NewTab }),
            (0, Hit::Address { Pane: AddressPane }) => {
                let AddressPane = AddressPane as usize;
                if EditingBefore != Some(AddressPane) {
                    self.ActivePane = AddressPane;
                    self.Execute(Command::FocusAddress);
                } else if Double {
                    self.Panes[AddressPane].Address.SelectWordAt(&self.Draw, Area, X);
                } else {
                    self.Panes[AddressPane].Address.ClickAt(&self.Draw, Area, X, Shift);
                    self.Pointer.Drag = DragState::Text { Target };
                }
            }
            (0, Hit::Filter { Pane: FilterPane }) => {
                self.Focus = Focus::Filter;
                let Draw = &self.Draw;
                let Edit = &mut self.Panes[FilterPane as usize].CurrentMut().Filter;
                if Double { Edit.SelectWordAt(Draw, Area, X) } else { Edit.ClickAt(Draw, Area, X, Shift) }
                self.Pointer.Drag = DragState::Text { Target };
            }
            (0, Hit::PaletteInput) | (0, Hit::PromptInput) => {
                crate::Interface::Overlays::OverlayClick(self, Target, Area, X, Shift);
                self.Pointer.Drag = DragState::Text { Target };
            }
            (0, Hit::HeaderResize { Column, .. }) => {
                let Width = self.Settings.Columns.get(Column as usize).map(|Item| Item.Width).unwrap_or(100.0);
                self.Pointer.Drag = DragState::Column { Column: Column as usize, StartX: X, StartWidth: Width };
            }
            (0, Hit::Split(Kind)) => {
                let Value = match Kind {
                    Splitter::Sidebar => self.Settings.SidebarWidth,
                    Splitter::Inspector => self.Settings.InspectorWidth,
                    Splitter::Panes => self.Settings.PaneSplit,
                };
                self.Pointer.Drag = DragState::Split { Kind, StartX: X, StartValue: Value };
            }
            (0, Hit::ScrollThumb { Pane: ThumbPane }) => {
                self.Pointer.Drag = DragState::Thumb { Pane: ThumbPane as usize, Grab: Y - Area.Top };
            }
            (0, Hit::ScrollTrack { Pane: TrackPane }) => {
                let TrackPane = TrackPane as usize;
                let Page = FileView::PageExtent(self, TrackPane);
                let Thumb = self.Hits.AreaOf(Hit::ScrollThumb { Pane: TrackPane as u8 });
                let Direction = if Thumb.is_some_and(|Thumb| Y < Thumb.Top) { -1.0 } else { 1.0 };
                FileView::ScrollBy(self, TrackPane, Direction * Page, false);
            }
            (_, Hit::List { Pane: ListPane }) => self.ListMouseDown(Button, ListPane as usize, X, Y, Double, Control, Shift),
            (0, Hit::SidebarToggle(Row)) => self.ToggleTreeRow(Row as usize),
            (0, Hit::Overlay) => {}
            (0, Hit::Inspector) => {}
            (1, _) | (2, _) => self.Pointer.Hover = Target,
            _ => {}
        }
        self.Invalidate();
    }

    fn ListMouseDown(&mut self, Button: u8, Pane: usize, X: f32, Y: f32, Double: bool, Control: bool, Shift: bool) {
        self.Focus = Focus::List;
        let Hit = FileView::HitItem(self, Pane, X, Y);
        if Button == 2 {
            if let Some(Position) = Hit {
                let Index = self.Tab(Pane).Order[Position];
                if self.Tab(Pane).Listing.IsDirectory(Index) {
                    self.OpenEntry(Pane, Index, true, false);
                }
            }
            return;
        }
        if self.Rename.is_some() {
            let Same = Hit.is_some_and(|Position| self.Rename.as_ref().is_some_and(|State| self.Tab(Pane).Order.get(Position) == Some(&State.Index)));
            if Same {
                return;
            }
            self.CommitRename();
        }
        match Hit {
            Some(Position) => {
                let Tab = self.TabMut(Pane);
                let Index = Tab.Order[Position];
                if Button == 1 {
                    if !Tab.IsSelected(Index) {
                        Tab.ClearSelection();
                        Tab.SetSelected(Index, true);
                        Tab.Anchor = Some(Index);
                    }
                    Tab.Cursor = Some(Index);
                    self.RefreshInspector();
                    return;
                }
                if Double {
                    Tab.ClearSelection();
                    Tab.SetSelected(Index, true);
                    Tab.Cursor = Some(Index);
                    self.Pointer.Drag = DragState::Idle;
                    self.OpenEntry(Pane, Index, Control, Shift);
                    return;
                }
                if Shift {
                    let AnchorPosition = Tab.Anchor.and_then(|Anchor| Tab.PositionOf(Anchor)).unwrap_or(Position);
                    if !Control {
                        Tab.ClearSelection();
                    }
                    Tab.SelectRange(AnchorPosition, Position);
                } else if Control {
                    let Selected = Tab.IsSelected(Index);
                    Tab.SetSelected(Index, !Selected);
                    Tab.Anchor = Some(Index);
                } else if !Tab.IsSelected(Index) {
                    Tab.ClearSelection();
                    Tab.SetSelected(Index, true);
                    Tab.Anchor = Some(Index);
                }
                Tab.Cursor = Some(Index);
                self.Pointer.Drag = DragState::Pending { Pane, X, Y, Index };
            }
            None => {
                if Button == 1 {
                    self.TabMut(Pane).ClearSelection();
                    return;
                }
                let (ContentX, ContentY) = FileView::ToContent(self, Pane, X, Y);
                let Tab = self.TabMut(Pane);
                if !Control && !Shift {
                    Tab.ClearSelection();
                }
                let Base = Tab.SelectedIndices();
                self.Pointer.Drag = DragState::Rubber { Pane, StartX: ContentX, StartY: ContentY, X: ContentX, Y: ContentY, Base };
            }
        }
        self.RefreshInspector();
    }

    pub fn OnMouseUp(&mut self, Button: u8, X: f32, Y: f32) {
        let (Target, _) = self.Hits.Find(X, Y);
        let Drag = std::mem::replace(&mut self.Pointer.Drag, DragState::Idle);
        Platform::Release();
        let Pressed = self.Pointer.Hover;
        match Drag {
            DragState::Pending { Pane, Index, .. } if Button == 0 => {
                let (Control, Shift, _) = Modifiers();
                if !Control && !Shift {
                    let Tab = self.TabMut(Pane);
                    Tab.ClearSelection();
                    Tab.SetSelected(Index, true);
                    Tab.Anchor = Some(Index);
                }
                self.RefreshInspector();
            }
            DragState::Split { .. } | DragState::Column { .. } => self.SaveSettings(),
            _ => {}
        }
        if Button == 0 && IsButtonHit(Target) && Target == Pressed {
            self.Click(Target, X, Y);
        }
        if Button == 1 {
            self.RightClick(Target, X, Y);
        }
        if Button == 2 {
            self.MiddleClick(Target);
        }
        self.OnMouseMove(X, Y);
        self.Invalidate();
    }

    fn Click(&mut self, Target: Hit, X: f32, Y: f32) {
        match Target {
            Hit::Run(Item) => self.Execute(Item),
            Hit::Minimize => Platform::Minimize(self.Window),
            Hit::Maximize => Platform::ToggleMaximize(self.Window),
            Hit::Close => Platform::Close(self.Window),
            Hit::TabClose { Pane, Index } => self.CloseTab(Pane as usize, Index as usize),
            Hit::TabNew { Pane } => {
                self.ActivePane = Pane as usize;
                self.Execute(Command::NewTab);
            }
            Hit::Crumb { Pane, Index } => {
                let Pane = Pane as usize;
                if let Some(Target) = crate::Interface::Chrome::CrumbTarget(self, Pane, Index as usize) {
                    self.Navigate(Pane, Target);
                }
            }
            Hit::CrumbMenu { Pane, Index } => {
                let Pane = Pane as usize;
                if let Some(Location::Directory(Base)) = crate::Interface::Chrome::CrumbTarget(self, Pane, Index as usize) {
                    let Area = self.Hits.AreaOf(Target).unwrap_or_default();
                    let (MenuX, MenuY) = (Area.Left, Area.Bottom + 2.0);
                    let Sink = self.Sink.clone();
                    let ShowHidden = self.Settings.ShowHidden;
                    self.Background.Submit(move || {
                        let Children = Enumerate::Subdirectories(&Base, ShowHidden, 2000).unwrap_or_default();
                        Sink.Send(crate::Application::Event::CrumbChildren { Pane, Base, Children, X: MenuX, Y: MenuY });
                    });
                }
            }
            Hit::FilterClear { Pane } => {
                let Pane = Pane as usize;
                self.TabMut(Pane).Filter.Clear();
                self.ApplyFilter(Pane);
                if let Location::Search { Root, .. } = self.Tab(Pane).Location.clone() {
                    self.Navigate(Pane, Location::Directory(Root));
                }
            }
            Hit::Header { Pane, Column } => {
                if let Some(Setting) = self.Settings.Columns.get(Column as usize).copied() {
                    let Key = match Setting.Kind {
                        ColumnKind::Name => crate::FileSystem::Sorting::SortKey::Name,
                        ColumnKind::Size => crate::FileSystem::Sorting::SortKey::Size,
                        ColumnKind::Modified => crate::FileSystem::Sorting::SortKey::Modified,
                        ColumnKind::Created => crate::FileSystem::Sorting::SortKey::Created,
                        ColumnKind::Type => crate::FileSystem::Sorting::SortKey::Type,
                        ColumnKind::Folder => crate::FileSystem::Sorting::SortKey::Folder,
                        ColumnKind::Attributes => return,
                    };
                    self.SortBy(Pane as usize, Key);
                }
            }
            Hit::SidebarRow(Row) => self.ActivateSidebarRow(Row as usize, false),
            Hit::OperationCancel(Identifier) => {
                if let Some(Running) = self.Operations.iter().find(|Running| Running.Identifier == Identifier) {
                    Running.Cancel();
                }
                self.Modules.Cancel(Identifier);
            }
            Hit::StatusMessage => {
                if let Some(Detail) = self.Notice.as_ref().and_then(|Notice| Notice.Detail.clone()) {
                    let Title = self.Notice.as_ref().map(|Notice| Notice.Text.clone()).unwrap_or_default();
                    self.ShowPrompt(crate::Interface::Overlays::Prompt::New(&Title, &Detail, &["Close"], crate::Interface::Overlays::PromptPurpose::Information));
                }
            }
            Hit::PaletteRow(_) | Hit::PromptButton(_) | Hit::PromptToggle | Hit::ModuleAction { .. } | Hit::Completion { .. } | Hit::MenuRow { .. } | Hit::SettingsRow(_) => {
                crate::Interface::Overlays::OverlayButton(self, Target);
            }
            Hit::InspectorAction(Action) => crate::Interface::Overlays::Inspector::Action(self, Action),
            _ => {}
        }
        let _ = (X, Y);
    }

    fn RightClick(&mut self, Target: Hit, X: f32, Y: f32) {
        match Target {
            Hit::List { Pane } => {
                let Pane = Pane as usize;
                let OnItem = FileView::HitItem(self, Pane, X, Y).is_some();
                self.ShowContextMenu(Pane, X, Y, OnItem);
            }
            Hit::Tab { Pane, Index } => self.ShowTabMenu(Pane as usize, Index as usize, X, Y),
            Hit::Header { .. } | Hit::HeaderResize { .. } => self.ShowColumnsMenu(X, Y),
            Hit::SidebarRow(Row) => self.ShowSidebarMenu(Row as usize, X, Y),
            Hit::Crumb { Pane, .. } | Hit::Address { Pane } => {
                self.ActivePane = Pane as usize;
                self.ShowContextMenu(Pane as usize, X, Y, false);
            }
            _ => {}
        }
    }

    fn MiddleClick(&mut self, Target: Hit) {
        match Target {
            Hit::Tab { Pane, Index } => self.CloseTab(Pane as usize, Index as usize),
            Hit::SidebarRow(Row) => self.ActivateSidebarRow(Row as usize, true),
            Hit::Crumb { Pane, Index } => {
                if let Some(Target) = crate::Interface::Chrome::CrumbTarget(self, Pane as usize, Index as usize) {
                    self.OpenTab(Pane as usize, Target);
                }
            }
            _ => {}
        }
    }

    pub fn OnMouseMove(&mut self, X: f32, Y: f32) {
        self.Pointer.X = X;
        self.Pointer.Y = Y;
        let (Target, _) = self.Hits.Find(X, Y);
        let mut Repaint = false;
        if Target != self.Pointer.Hover && matches!(self.Pointer.Drag, DragState::Idle) {
            self.Pointer.Hover = Target;
            Repaint = true;
        }
        if matches!(Target, Hit::List { .. }) && matches!(self.Pointer.Drag, DragState::Idle) {
            Repaint = true;
        }
        match self.Pointer.Drag.clone() {
            DragState::Idle => {}
            DragState::Pending { Pane, X: StartX, Y: StartY, .. } => {
                if !Platform::KeyDown(0x01) {
                    self.Pointer.Drag = DragState::Idle;
                } else if (X - StartX).abs() > 5.0 || (Y - StartY).abs() > 5.0 {
                    let Items = self.Tab(Pane).TargetPaths();
                    self.Pointer.Drag = DragState::Idle;
                    Platform::Release();
                    if !Items.is_empty() && !matches!(self.Tab(Pane).Location, Location::Computer) {
                        self.Deferred.push(crate::Application::Deferred::StartDrag { Items });
                    }
                }
            }
            DragState::Rubber { Pane, StartX, StartY, Base, .. } => {
                let Area = self.ViewAreas[Pane];
                if Y > Area.Bottom - 4.0 {
                    FileView::ScrollBy(self, Pane, (Y - Area.Bottom + 4.0).min(40.0), true);
                } else if Y < Area.Top + 30.0 {
                    FileView::ScrollBy(self, Pane, -(Area.Top + 30.0 - Y).min(40.0), true);
                }
                let (ContentX, ContentY) = FileView::ToContent(self, Pane, X, Y);
                let Inside = FileView::ItemsInContentRect(self, Pane, StartX.min(ContentX), StartY.min(ContentY), StartX.max(ContentX), StartY.max(ContentY));
                let (Control, _, _) = Modifiers();
                let Tab = self.TabMut(Pane);
                Tab.ClearSelection();
                for &Index in &Base {
                    Tab.SetSelected(Index, true);
                }
                for Position in Inside {
                    let Index = Tab.Order[Position];
                    let Selected = Tab.IsSelected(Index);
                    Tab.SetSelected(Index, if Control { !Selected } else { true });
                    Tab.Cursor = Some(Index);
                }
                self.Pointer.Drag = DragState::Rubber { Pane, StartX, StartY, X: ContentX, Y: ContentY, Base };
                Repaint = true;
            }
            DragState::Split { Kind, StartX, StartValue } => {
                let Delta = X - StartX;
                match Kind {
                    Splitter::Sidebar => self.Settings.SidebarWidth = (StartValue + Delta).clamp(140.0, 600.0),
                    Splitter::Inspector => self.Settings.InspectorWidth = (StartValue - Delta).clamp(180.0, 900.0),
                    Splitter::Panes => {
                        let Total = self.ViewAreas[0].Width() + self.ViewAreas[1].Width();
                        if Total > 0.0 {
                            self.Settings.PaneSplit = (StartValue + Delta / Total).clamp(0.15, 0.85);
                        }
                    }
                }
                Repaint = true;
            }
            DragState::Column { Column, StartX, StartWidth, .. } => {
                if let Some(Setting) = self.Settings.Columns.get_mut(Column) {
                    Setting.Width = (StartWidth + X - StartX).clamp(40.0, 2000.0);
                }
                Repaint = true;
            }
            DragState::TabMove { Pane, Index, StartX, Moved } => {
                if Moved || (X - StartX).abs() > 8.0 {
                    if let Hit::Tab { Pane: OverPane, Index: Over } = Target {
                        if OverPane as usize == Pane && Over as usize != Index {
                            self.MoveTab(Pane, Index, Over as usize);
                            self.Pointer.Drag = DragState::TabMove { Pane, Index: Over as usize, StartX, Moved: true };
                        } else {
                            self.Pointer.Drag = DragState::TabMove { Pane, Index, StartX, Moved: true };
                        }
                    } else if let Some(Hit::TabStrip { Pane: OverPane }) | Some(Hit::TabNew { Pane: OverPane }) = Some(Target) {
                        if OverPane as usize != Pane {
                            self.TransferTab(Pane, Index);
                            let NewIndex = self.Panes[OverPane as usize].Tabs.len() - 1;
                            self.Pointer.Drag = DragState::TabMove { Pane: OverPane as usize, Index: NewIndex, StartX: X, Moved: true };
                        }
                    }
                    Repaint = true;
                }
            }
            DragState::Thumb { Pane, Grab } => {
                if let Some(Track) = FileView::TrackArea(self, Pane) {
                    let (Content, Viewport) = FileView::Extents(self, Pane);
                    let Offset = OffsetForThumb(Track, Y - Grab, Content, Viewport);
                    self.TabMut(Pane).Scroll.Jump(Offset);
                    Repaint = true;
                }
            }
            DragState::Text { Target: Owner } => {
                crate::Interface::Overlays::DragText(self, Owner, X);
                Repaint = true;
            }
        }
        if Repaint {
            self.Invalidate();
        }
    }

    pub fn OnWheel(&mut self, Delta: f32, Horizontal: bool, X: f32, Y: f32) {
        let (Target, _) = self.Hits.Find(X, Y);
        let (Control, Shift, _) = Modifiers();
        if self.Overlay.is_some() {
            crate::Interface::Overlays::OverlayWheel(self, Delta);
            return;
        }
        match Target {
            Hit::List { Pane } | Hit::ScrollThumb { Pane } | Hit::ScrollTrack { Pane } | Hit::Header { Pane, .. } => {
                let Pane = Pane as usize;
                if Control {
                    self.ActivePane = Pane;
                    self.Execute(if Delta > 0.0 { Command::ZoomIn } else { Command::ZoomOut });
                    return;
                }
                let Lines = Delta / 120.0 * 3.0 * self.Theme.Sizes.RowHeight;
                if Horizontal || Shift {
                    FileView::ScrollHorizontal(self, Pane, if Horizontal { Lines } else { -Lines });
                } else {
                    let Smooth = self.Settings.SmoothScrolling;
                    FileView::ScrollBy(self, Pane, -Lines, !Smooth);
                }
            }
            Hit::SidebarRow(_) | Hit::SidebarToggle(_) | Hit::SidebarArea => {
                crate::Interface::Chrome::Sidebar::Scroll(self, -Delta / 120.0 * 3.0 * self.Theme.Sizes.SidebarRowHeight);
            }
            Hit::Inspector | Hit::InspectorAction(_) => self.Inspector.ScrollBy(-Delta / 120.0 * 48.0),
            Hit::Tab { Pane, .. } | Hit::TabStrip { Pane } => {
                self.ActivePane = Pane as usize;
                self.Execute(if Delta > 0.0 { Command::PreviousTab } else { Command::NextTab });
            }
            _ => {}
        }
        self.EnsureScrollTimer();
        self.Invalidate();
    }

    pub fn OnDrop(&mut self, Request: &DropRequest, Commit: bool) -> DropAnswer {
        let Refuse = DropAnswer { Effect: 0, Shell: None };
        if Request.Phase == DropPhase::Leave {
            self.DropHover = None;
            self.Invalidate();
            return Refuse;
        }
        let Scale = self.Scale();
        let (X, Y) = (Request.X as f32 / Scale, Request.Y as f32 / Scale);
        let (Target, _) = self.Hits.Find(X, Y);
        let mut Hover: Option<DropHover> = None;
        let mut Destination: Option<PathBuf> = None;
        match Target {
            Hit::List { Pane } => {
                let Pane = Pane as usize;
                let Under = FileView::HitItem(self, Pane, X, Y).map(|Position| self.Tab(Pane).Order[Position]);
                let Over = Under.filter(|&Index| self.Tab(Pane).Listing.IsDirectory(Index));
                if let Some(Index) = Under.filter(|_| Over.is_none()) {
                    let Item = self.Tab(Pane).Listing.FullPath(Index);
                    let Dragged = Request.Items.iter().any(|Source| crate::FileSystem::SamePath(Source, &Item));
                    if !Dragged && crate::Platform::Clipboard::DragDrop::AcceptsDrops(self.Window, &Item) {
                        self.DropHover = if Request.Phase == DropPhase::Drop { None } else { Some(DropHover::List { Pane, Target: Some(Index) }) };
                        self.Invalidate();
                        return DropAnswer { Effect: 0, Shell: Some(Item) };
                    }
                }
                Destination = match Over {
                    Some(Index) => Some(self.Tab(Pane).Listing.FullPath(Index)),
                    None => self.CurrentDirectory(Pane).filter(|_| matches!(self.Tab(Pane).Location, Location::Directory(_))),
                };
                Hover = Some(DropHover::List { Pane, Target: Over });
            }
            Hit::SidebarRow(Row) => {
                Destination = crate::Interface::Chrome::Sidebar::RowPath(self, Row as usize);
                Hover = Some(DropHover::Sidebar(Row));
            }
            Hit::Crumb { Pane, Index } => {
                if let Some(Location::Directory(Folder)) = crate::Interface::Chrome::CrumbTarget(self, Pane as usize, Index as usize) {
                    Destination = Some(Folder);
                    Hover = Some(DropHover::Crumb { Pane: Pane as usize, Index });
                }
            }
            Hit::Tab { Pane, Index } => {
                let Location = &self.Panes[Pane as usize].Tabs[Index as usize].Location;
                if let Location::Directory(Directory) = Location {
                    Destination = Some(Directory.clone());
                }
            }
            _ => {}
        }
        self.DropHover = if Request.Phase == DropPhase::Drop { None } else { Hover };
        self.Invalidate();
        let Some(Destination) = Destination else { return Refuse };
        if Request.Items.is_empty() {
            return DropAnswer { Effect: 0, Shell: Some(Destination) };
        }
        if Request.Items.iter().any(|Item| crate::FileSystem::IsSameOrInside(&Destination, Item)) {
            return Refuse;
        }
        if Request.Items.iter().all(|Item| Item.parent().is_some_and(|Folder| crate::FileSystem::SamePath(Folder, &Destination))) {
            return Refuse;
        }
        let SameVolume = Request.Items.first().is_some_and(|Item| VolumeOf(Item) == VolumeOf(&Destination));
        let Control = Request.Keys & 0x8 != 0;
        let Shift = Request.Keys & 0x4 != 0;
        let Wanted = if Control {
            EffectCopy
        } else if Shift {
            EffectMove
        } else if SameVolume {
            EffectMove
        } else {
            EffectCopy
        };
        let Effect = if Request.Allowed & Wanted != 0 { Wanted } else if Request.Allowed & EffectCopy != 0 { EffectCopy } else { 0 };
        if Commit && Effect != 0 {
            let Move = Effect == EffectMove;
            self.DropInto(Request.Items.to_vec(), Destination, Move);
            return DropAnswer { Effect: if Move { 0 } else { Effect }, Shell: None };
        }
        DropAnswer { Effect, Shell: None }
    }

    fn ActivateSidebarRow(&mut self, Row: usize, NewTab: bool) {
        if crate::Interface::Chrome::Sidebar::ToggleSection(self, Row) {
            return;
        }
        let Target = crate::Interface::Chrome::Sidebar::RowLocation(self, Row);
        if let Some(Target) = Target {
            let Pane = self.ActivePane;
            if NewTab { self.OpenTab(Pane, Target) } else { self.Navigate(Pane, Target) }
        }
    }

    fn ToggleTreeRow(&mut self, Row: usize) {
        if let Some(Folder) = crate::Interface::Chrome::Sidebar::ToggleRow(self, Row) {
            self.RequestTreeChildren(Folder);
        }
    }
}

fn VolumeOf(Item: &std::path::Path) -> String {
    let Text = Item.to_string_lossy().to_ascii_lowercase();
    if let Some(Rest) = Text.strip_prefix("\\\\") {
        let Parts: Vec<&str> = Rest.splitn(3, '\\').collect();
        return format!("\\\\{}\\{}", Parts.first().unwrap_or(&""), Parts.get(1).unwrap_or(&""));
    }
    Text.chars().take(2).collect()
}
