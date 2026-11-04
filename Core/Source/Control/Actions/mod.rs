use super::Commands::Command;
use crate::Application::{App, Deferred, Focus, RenameState};
use crate::Control::Session::Undo::{Purpose, RecordFor};
use crate::FileSystem::Enumerate::{self, Exists};
use crate::FileSystem::Operations::{CollisionChoice, CollisionQuery, Notification, Operation, Request};
use crate::FileSystem::{IsSameOrInside, Location, SamePath, UniqueName, ValidateName};
use crate::Interface::Overlays::{Overlay, Prompt, PromptPurpose};
use crate::Interface::Widgets::LineEdit;
use crate::Platform::Clipboard;
use crate::Platform::Menus::ContextOutcome;
use crate::Services::Modules::{InvocationContext, ModuleEvent, Requirement};
use crate::Services::Workers::NewIdentifier;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn FormatTime(Value: i64) -> String {
    crate::Interface::FileView::Columns::FormatFileTime(Value)
}

impl App {
    pub fn ExecuteAction(&mut self, Item: Command) {
        let Pane = self.ActivePane;
        match Item {
            Command::Open => {
                if let Some(Cursor) = self.Tab(Pane).Cursor {
                    let Targets = self.Tab(Pane).Targets();
                    if Targets.len() > 1 {
                        let Tab = self.Tab(Pane);
                        let Files: Vec<PathBuf> = Targets.into_iter().filter(|&Index| !Tab.Listing.IsDirectory(Index)).take(32).map(|Index| Tab.Listing.FullPath(Index)).collect();
                        for Path in Files {
                            self.Deferred.push(Deferred::Open(Path));
                        }
                    } else {
                        self.OpenEntry(Pane, Cursor, false, false);
                    }
                }
            }
            Command::OpenInNewTab => {
                for Index in self.Tab(Pane).Targets() {
                    if self.Tab(Pane).Listing.IsDirectory(Index) {
                        self.OpenEntry(Pane, Index, true, false);
                    }
                }
            }
            Command::OpenInOtherPane => {
                if let Some(Cursor) = self.Tab(Pane).Cursor {
                    if !self.Settings.DualPane {
                        self.Execute(Command::ToggleDualPane);
                    }
                    self.OpenEntry(Pane, Cursor, false, true);
                }
            }
            Command::OpenWith => {
                if let Some(Item) = self.Tab(Pane).TargetPaths().into_iter().next() {
                    self.Deferred.push(Deferred::OpenWith(Item));
                }
            }
            Command::RunAsAdministrator => {
                if let Some(Item) = self.Tab(Pane).TargetPaths().into_iter().next() {
                    self.Deferred.push(Deferred::RunAs(Item));
                }
            }
            Command::Properties => {
                let Items = self.Tab(Pane).TargetPaths();
                let Items = if Items.is_empty() { self.CurrentDirectory(Pane).into_iter().collect() } else { Items };
                if !Items.is_empty() {
                    self.Deferred.push(Deferred::Properties(Items));
                }
            }
            Command::Rename => {
                if let Some(Cursor) = self.Tab(Pane).Cursor {
                    self.BeginRename(Pane, Cursor);
                }
            }
            Command::Delete => self.DeleteTargets(false),
            Command::DeletePermanently => self.DeleteTargets(true),
            Command::NewFolder => self.CreateItem(true),
            Command::NewFile => self.CreateItem(false),
            Command::Copy | Command::Cut => {
                let Items = self.Tab(Pane).TargetPaths();
                if !Items.is_empty() && Clipboard::SetFiles(self.Window, &Items, Item == Command::Cut) {
                    let Verb = if Item == Command::Cut { "Cut" } else { "Copied" };
                    self.Inform(&format!("{Verb} {} item{} to the clipboard", Items.len(), if Items.len() == 1 { "" } else { "s" }));
                }
            }
            Command::Paste => {
                if let Some(Directory) = self.CurrentDirectory(Pane).filter(|_| matches!(self.Tab(Pane).Location, Location::Directory(_))) {
                    self.PasteInto(&Directory);
                }
            }
            Command::CopyToOtherPane | Command::MoveToOtherPane => {
                let Some(Other) = self.OtherPane() else {
                    self.Inform("Enable dual pane (F9) to copy or move between panes");
                    return;
                };
                let Items = self.Tab(Pane).TargetPaths();
                if let (false, Some(Destination)) = (Items.is_empty(), self.CurrentDirectory(Other)) {
                    let Request = if Item == Command::CopyToOtherPane {
                        Request::CopyInto { Sources: Items, Destination }
                    } else {
                        Request::MoveInto { Sources: Items, Destination }
                    };
                    self.StartOperation(Request, Purpose::Normal);
                }
            }
            Command::CopyPath => {
                let Items = self.Tab(Pane).TargetPaths();
                let Items = if Items.is_empty() { self.CurrentDirectory(Pane).into_iter().collect() } else { Items };
                let Text: Vec<String> = Items.iter().map(|Item| Item.display().to_string()).collect();
                if !Text.is_empty() && Clipboard::SetText(self.Window, &Text.join("\r\n")) {
                    self.Inform("Path copied");
                }
            }
            Command::CopyName => {
                let Tab = self.Tab(Pane);
                let Text: Vec<String> = Tab.Targets().into_iter().map(|Index| Tab.Listing.NameString(Index)).collect();
                if !Text.is_empty() && Clipboard::SetText(self.Window, &Text.join("\r\n")) {
                    self.Inform("Name copied");
                }
            }
            Command::SelectAll => self.TabMut(Pane).SelectAll(),
            Command::InvertSelection => self.TabMut(Pane).InvertSelection(),
            Command::ClearSelection => self.TabMut(Pane).ClearSelection(),
            Command::Undo => self.UndoLast(),
            Command::Redo => self.RedoLast(),
            Command::ContextMenu => {
                let Area = self.ViewAreas[Pane];
                let Position = self.Tab(Pane).CursorPosition();
                let Anchor = Position.and_then(|Position| crate::Interface::FileView::ItemArea(self, Pane, Position));
                let (X, Y) = Anchor.map(|Item| (Item.Left + 24.0, Item.Bottom)).unwrap_or((Area.Left + 40.0, Area.Top + 40.0));
                self.ShowContextMenu(Pane, X, Y, Position.is_some() && self.Tab(Pane).Cursor.is_some());
            }
            Command::OpenTerminal => {
                if let Some(Directory) = self.CurrentDirectory(Pane) {
                    if let Err(Message) = crate::Platform::Shell::LaunchTerminal(&Directory, &self.Settings.TerminalCommand.clone()) {
                        self.Fail("Terminal could not be started", &Message);
                    }
                }
            }
            Command::CalculateSize => {
                if !self.Settings.ShowInspector {
                    self.Settings.ShowInspector = true;
                }
                self.RefreshInspector();
                self.StartFolderSize();
            }
            Command::PinFavorite | Command::UnpinFavorite => {
                let Tab = self.Tab(Pane);
                let Target = Tab.Targets().into_iter().find(|&Index| Tab.Listing.IsDirectory(Index)).map(|Index| Tab.Listing.FullPath(Index)).or_else(|| self.CurrentDirectory(Pane));
                if let Some(Target) = Target {
                    let Changed = if Item == Command::PinFavorite { self.Session.Pin(&Target) } else { self.Session.Unpin(&Target) };
                    if Changed {
                        let _ = self.Session.Save(&self.Paths.SessionFile());
                        self.Inform(&format!("{} {}", if Item == Command::PinFavorite { "Pinned" } else { "Unpinned" }, Target.display()));
                    }
                }
            }
            Command::ManageModules => {
                self.Overlay = Some(Overlay::Modules(Default::default()));
                self.Focus = Focus::Overlay;
            }
            Command::InstallModule => self.Deferred.push(Deferred::PickModuleFolder),
            Command::ReloadModules => {
                self.Modules.Discover();
                self.Inform(&format!("{} module(s) found", self.Modules.Entries().len()));
            }
            Command::OpenSettings => self.OpenSettingsPanel(),
            Command::OpenKeybindings => self.Deferred.push(Deferred::Open(self.Paths.KeybindingsFile())),
            Command::OpenThemesFolder => {
                let Folder = self.Paths.ThemesDirectory();
                let _ = std::fs::create_dir_all(&Folder);
                let Sample = Folder.join("Example.ini");
                if !Sample.exists() {
                    let _ = std::fs::write(&Sample, "[Theme]\nBase=Dark\nAccent=#E0A040\nBackground=#141414\n");
                }
                self.OpenTab(Pane, Location::Directory(Folder));
            }
            Command::ReloadConfiguration => {
                self.Settings = crate::Services::Config::Settings::Load(&self.Paths.SettingsFile());
                self.Keys = crate::Control::Commands::Keymap::Load(&self.Paths.KeybindingsFile());
                self.Media.SetBudget(self.Settings.ThumbnailCacheMegabytes);
                self.ReloadTheme();
                self.RefilterAll();
                self.Inform("Settings, theme and shortcuts reloaded");
            }
            Command::OpenCrashFolder => {
                let Folder = self.Paths.CrashDirectory();
                let _ = std::fs::create_dir_all(&Folder);
                self.OpenTab(Pane, Location::Directory(Folder));
            }
            Command::TestCrash => {
                let Prompt = Prompt::New(
                    "Test the crash reporter",
                    "Flux will deliberately crash and write a diagnostic report to the crash folder. Unsaved state is lost.",
                    &["Crash Now", "Cancel"],
                    PromptPurpose::TestCrash,
                )
                .Dangerous();
                self.ShowPrompt(Prompt);
            }
            Command::Module(Index) => self.RunModuleCommand(Index),
            _ => {}
        }
    }

    pub fn ShowPrompt(&mut self, Value: Prompt) {
        self.Overlay = Some(Overlay::Prompt(Value));
        self.Focus = Focus::Overlay;
        self.Invalidate();
    }

    pub fn CloseOverlay(&mut self) {
        self.Overlay = None;
        self.Focus = Focus::List;
        self.NextCollision();
        self.Invalidate();
    }

    pub fn StartOperation(&mut self, Value: Request, Reason: Purpose) {
        let Identifier = NewIdentifier();
        let Sink = self.Sink.clone();
        let Notify = Arc::new(move |Update: Notification| Sink.Send(crate::Application::Event::Operation(Update)));
        match Operation::Start(Identifier, Value, self.Window.0 as isize, Notify) {
            Ok(Running) => {
                self.Purposes.insert(Identifier, Reason);
                self.Operations.push(Running);
            }
            Err(Error) => self.Fail("The operation could not start", &Error.to_string()),
        }
        self.Invalidate();
    }

    pub fn OnOperation(&mut self, Update: Notification) {
        match Update {
            Notification::Progress { Identifier } => {
                if !self.Operations.iter().any(|Running| Running.Identifier == Identifier) {
                    return;
                }
            }
            Notification::Collision { Identifier, Query } => {
                self.Collisions.push_back((Identifier, Query));
                self.NextCollision();
            }
            Notification::Finished { Identifier, Summary } => {
                self.Operations.retain(|Running| Running.Identifier != Identifier);
                self.Collisions.retain(|(Owner, _)| *Owner != Identifier);
                if let Some(Overlay::Prompt(Current)) = &self.Overlay {
                    if matches!(Current.Purpose, PromptPurpose::Collision(Owner) if Owner == Identifier) {
                        self.CloseOverlay();
                    }
                }
                let Reason = self.Purposes.remove(&Identifier).unwrap_or(Purpose::Normal);
                let Failed = !Summary.Errors.is_empty();
                match Reason {
                    Purpose::Normal => {
                        if let Some(Record) = RecordFor(&Summary) {
                            self.Undo.Push(Record);
                        }
                    }
                    Purpose::Undo(Record) => {
                        if !Failed && !Summary.Cancelled {
                            self.Undo.PushUndone(Record);
                        }
                    }
                    Purpose::Redo => {
                        if let Some(Record) = RecordFor(&Summary) {
                            self.Undo.PushDone(Record);
                        }
                    }
                }
                if let Request::CreateFolder(Item) | Request::CreateFile(Item) = &Summary.Request {
                    if !Summary.Created.is_empty() {
                        self.FocusCreated(Item.clone());
                    }
                }
                if let Request::MovePairs(Pairs) = &Summary.Request {
                    if Pairs.len() == 1 && Summary.Moved.len() == 1 {
                        self.FocusPath(&Summary.Moved[0].1);
                    }
                }
                if Failed {
                    let Lines: Vec<String> = Summary.Errors.iter().take(40).map(|(Item, Message)| format!("{}\n    {}", Item.display(), Message)).collect();
                    let More = if Summary.Errors.len() > 40 { format!("\n… and {} more", Summary.Errors.len() - 40) } else { String::new() };
                    let Title = format!("{} finished with {} problem{}", Summary.Request.Title(), Summary.Errors.len(), if Summary.Errors.len() == 1 { "" } else { "s" });
                    self.Fail(&Title, &Summary.Errors[0].1);
                    if self.Overlay.is_none() {
                        self.ShowPrompt(Prompt::New(&Title, &format!("{}{}", Lines.join("\n"), More), &["Close"], PromptPurpose::Information));
                    }
                } else if Summary.Cancelled {
                    self.Inform(&format!("{} cancelled", Summary.Request.Title()));
                } else {
                    self.Inform(&format!("{} — done in {:.1}s", Summary.Request.Title(), Summary.Elapsed.as_secs_f64()));
                }
                self.RequestFreeSpace(self.ActivePane);
                self.Sidebar.InvalidateSpace();
            }
        }
        self.Invalidate();
    }

    fn FocusCreated(&mut self, Item: PathBuf) {
        let Pane = self.ActivePane;
        let Visible = self.Visibility();
        let Some(Parent) = Item.parent().map(Path::to_path_buf) else { return };
        if !self.CurrentDirectory(Pane).is_some_and(|Current| SamePath(&Current, &Parent)) {
            return;
        }
        let Some(Data) = Enumerate::Stat(&Item) else { return };
        let Tab = self.TabMut(Pane);
        Tab.ApplyChange(&Data.Name, Some(&Data), &Visible);
        if let Some(Index) = Tab.Listing.Find(&Data.Name) {
            Tab.ClearSelection();
            Tab.Cursor = Some(Index);
            Tab.Anchor = Some(Index);
            Tab.SetSelected(Index, true);
            self.RevealIndex(Pane, Index);
            self.BeginRename(Pane, Index);
        }
    }

    fn FocusPath(&mut self, Item: &Path) {
        let Pane = self.ActivePane;
        let Visible = self.Visibility();
        let (Some(Parent), Some(Data)) = (Item.parent(), Enumerate::Stat(Item)) else { return };
        if !self.CurrentDirectory(Pane).is_some_and(|Current| SamePath(&Current, Parent)) {
            return;
        }
        let Tab = self.TabMut(Pane);
        Tab.ApplyChange(&Data.Name, Some(&Data), &Visible);
        if let Some(Index) = Tab.Listing.Find(&Data.Name) {
            Tab.ClearSelection();
            Tab.Cursor = Some(Index);
            Tab.Anchor = Some(Index);
            Tab.SetSelected(Index, true);
            self.RevealIndex(Pane, Index);
        }
    }

    pub fn NextCollision(&mut self) {
        if self.Overlay.is_some() {
            return;
        }
        let Some((Identifier, Query)) = self.Collisions.pop_front() else { return };
        if !self.Operations.iter().any(|Running| Running.Identifier == Identifier) {
            return self.NextCollision();
        }
        self.ShowPrompt(CollisionPrompt(Identifier, &Query));
    }

    pub fn OnPromptResult(&mut self, Value: Prompt, Button: Option<usize>) {
        let Toggle = Value.Toggle.as_ref().is_some_and(|(_, State)| *State);
        let Input = Value.Input.as_ref().map(LineEdit::Value).unwrap_or_default();
        match Value.Purpose {
            PromptPurpose::DeletePermanently(Items) => {
                if Button == Some(0) {
                    self.StartOperation(Request::DeletePermanently(Items), Purpose::Normal);
                }
            }
            PromptPurpose::Collision(Identifier) => {
                let Choice = match Button {
                    Some(0) => CollisionChoice::Replace,
                    Some(1) => CollisionChoice::Skip,
                    Some(2) => CollisionChoice::KeepBoth,
                    _ => CollisionChoice::Cancel,
                };
                if let Some(Running) = self.Operations.iter().find(|Running| Running.Identifier == Identifier) {
                    Running.Resolve(Choice, Toggle);
                }
            }
            PromptPurpose::ModuleArgument(Index) => {
                if Button == Some(0) && !Input.trim().is_empty() {
                    self.InvokeModule(Index, Input);
                }
            }
            PromptPurpose::ApproveModule(Index) => {
                if Button == Some(0) {
                    match self.Modules.Approve(Index) {
                        Ok(Hash) => self.Inform(&format!("Module trusted (SHA-256 {}…)", &Hash[..16.min(Hash.len())])),
                        Err(Message) => self.Fail("Module could not be approved", &Message),
                    }
                }
                self.Overlay = Some(Overlay::Modules(Default::default()));
                self.Focus = Focus::Overlay;
                return;
            }
            PromptPurpose::TestCrash => {
                if Button == Some(0) {
                    self.SaveState();
                    self.Deferred.push(Deferred::TestCrash);
                }
            }
            PromptPurpose::Information => {}
        }
        self.CloseOverlay();
    }

    pub fn DeleteTargets(&mut self, Permanent: bool) {
        let Pane = self.ActivePane;
        let Items = self.Tab(Pane).TargetPaths();
        if Items.is_empty() {
            return;
        }
        if !Permanent {
            self.StartOperation(Request::Recycle(Items), Purpose::Normal);
            return;
        }
        let Message = match Items.as_slice() {
            [Single] => format!("Permanently delete \"{}\"?\nThis cannot be undone.", Single.display()),
            _ => format!("Permanently delete {} items?\nThis cannot be undone.", Items.len()),
        };
        self.ShowPrompt(Prompt::New("Delete permanently", &Message, &["Delete", "Cancel"], PromptPurpose::DeletePermanently(Items)).Dangerous());
    }

    pub fn CreateItem(&mut self, Folder: bool) {
        let Pane = self.ActivePane;
        let Some(Directory) = self.CurrentDirectory(Pane).filter(|_| matches!(self.Tab(Pane).Location, Location::Directory(_))) else {
            self.Inform("New items can only be created inside a folder");
            return;
        };
        let Base = if Folder { "New folder" } else { "New text file.txt" };
        let Name = UniqueName(&Directory, Base, Folder, &|Candidate| Exists(Candidate));
        let Target = Directory.join(Name);
        self.StartOperation(if Folder { Request::CreateFolder(Target) } else { Request::CreateFile(Target) }, Purpose::Normal);
    }

    pub fn PasteInto(&mut self, Directory: &Path) {
        let Some((Items, Cut)) = Clipboard::GetFiles(self.Window) else {
            self.Inform("The clipboard does not contain files");
            return;
        };
        if Cut {
            if Items.iter().all(|Item| Item.parent().is_some_and(|Parent| SamePath(Parent, Directory))) {
                return;
            }
            self.StartOperation(Request::MoveInto { Sources: Items, Destination: Directory.to_path_buf() }, Purpose::Normal);
            Clipboard::ClearIfCut(self.Window);
        } else {
            self.StartOperation(Request::CopyInto { Sources: Items, Destination: Directory.to_path_buf() }, Purpose::Normal);
        }
    }

    pub fn BeginRename(&mut self, Pane: usize, Index: u32) {
        let Tab = self.Tab(Pane);
        if !matches!(Tab.Location, Location::Directory(_) | Location::Search { .. } | Location::Results { .. }) {
            return;
        }
        let Name = Tab.Listing.NameString(Index);
        let Original = Tab.Listing.FullPath(Index);
        let mut Edit = LineEdit::New();
        Edit.SetText(&Name);
        let IsDirectory = Tab.Listing.IsDirectory(Index);
        let StemEnd = if IsDirectory { Edit.Text.len() } else { Name.rfind('.').filter(|&Position| Position > 0).map(|Position| Name[..Position].encode_utf16().count()).unwrap_or(Edit.Text.len()) };
        Edit.SelectRange(0, StemEnd);
        let TabIdentifier = Tab.Identifier;
        self.Rename = Some(RenameState { Pane, TabIdentifier, Index, Edit, Original });
        self.Focus = Focus::Rename;
        self.RevealIndex(Pane, Index);
        self.Invalidate();
    }

    pub fn CancelRename(&mut self) {
        if self.Rename.take().is_some() && self.Focus == Focus::Rename {
            self.Focus = Focus::List;
        }
    }

    pub fn CommitRename(&mut self) {
        let Some(State) = self.Rename.take() else { return };
        self.Focus = Focus::List;
        let NewName = State.Edit.Value();
        let OldName = State.Original.file_name().map(|Name| Name.to_string_lossy().into_owned()).unwrap_or_default();
        if NewName == OldName {
            return;
        }
        if let Err(Message) = ValidateName(&NewName) {
            self.Fail("Invalid name", Message);
            return;
        }
        let Some(Parent) = State.Original.parent() else { return };
        let Target = Parent.join(OsString::from(NewName.trim_end_matches([' ', '.'])));
        self.StartOperation(Request::MovePairs(vec![(State.Original, Target)]), Purpose::Normal);
    }

    pub fn UndoLast(&mut self) {
        match self.Undo.TakeUndo() {
            Some(Record) => {
                self.Inform(&format!("Undo: {}", Record.Title));
                self.StartOperation(Record.Reverse.clone(), Purpose::Undo(Record));
            }
            None => self.Inform("Nothing to undo"),
        }
    }

    pub fn RedoLast(&mut self) {
        match self.Undo.TakeRedo() {
            Some(Record) => {
                self.Inform(&format!("Redo: {}", Record.Title));
                self.StartOperation(Record.Forward, Purpose::Redo);
            }
            None => self.Inform("Nothing to redo"),
        }
    }

    pub fn ApplyContextOutcome(&mut self, Pane: usize, Outcome: ContextOutcome) {
        self.ActivePane = Pane;
        match Outcome {
            ContextOutcome::Nothing | ContextOutcome::Invoked => {}
            ContextOutcome::Rename => self.Execute(Command::Rename),
            ContextOutcome::Delete => self.Execute(if crate::Platform::KeyDown(0x10) { Command::DeletePermanently } else { Command::Delete }),
            ContextOutcome::Paste => self.Execute(Command::Paste),
            ContextOutcome::Copy => self.Execute(Command::Copy),
            ContextOutcome::Cut => self.Execute(Command::Cut),
            ContextOutcome::Failed(Message) => self.Fail("Context menu command failed", &Message),
        }
        self.Invalidate();
    }

    pub fn AfterDrag(&mut self, Effect: Option<u32>) {
        if let Some(Effect) = Effect {
            if Effect & Clipboard::EffectMove != 0 {
                self.Inform("Items moved");
            }
        }
        self.DropHover = None;
        self.Invalidate();
    }

    pub fn RunModuleCommand(&mut self, Index: u32) {
        let Some(Item) = self.Modules.Command(Index) else { return };
        if Item.Requires == Requirement::OtherPane && !self.Settings.DualPane {
            self.Inform("This command compares against the other pane; enable dual pane (F9) first");
            return;
        }
        if Item.Requires == Requirement::Selection && self.Tab(self.ActivePane).Targets().is_empty() {
            self.Inform("Select one or more items first");
            return;
        }
        if !Item.Prompt.is_empty() {
            let Title = Item.Title.clone();
            let Message = Item.Prompt.clone();
            self.ShowPrompt(Prompt::New(&Title, &Message, &["Run", "Cancel"], PromptPurpose::ModuleArgument(Index)).WithInput(""));
            return;
        }
        self.InvokeModule(Index, String::new());
    }

    pub fn InvokeModule(&mut self, Index: u32, Argument: String) {
        let Pane = self.ActivePane;
        let Context = InvocationContext {
            Directory: self.CurrentDirectory(Pane).unwrap_or_default(),
            OtherDirectory: self.OtherPane().and_then(|Other| self.CurrentDirectory(Other)),
            Argument,
            Selection: self.Tab(Pane).TargetPaths(),
        };
        match self.Modules.Invoke(Index, Context, self.Sink.clone()) {
            Ok(_) => self.Inform("Module command started"),
            Err(Message) => self.Fail("Module command could not run", &Message),
        }
    }

    pub fn OnModuleEvent(&mut self, Update: ModuleEvent) {
        match Update {
            ModuleEvent::Progress => {}
            ModuleEvent::Preview { Token, Text } => self.Inspector.AcceptModuleText(Token, Text),
            ModuleEvent::Finished { Title, Status, Text, Paths, Renames, Elapsed } => {
                let Outcome = match Status {
                    FluxSdk::StatusOk => "completed",
                    FluxSdk::StatusCancelled => "cancelled",
                    FluxSdk::StatusUnsupported => "is not supported here",
                    _ => "failed",
                };
                let Summary = format!("{Title} {Outcome} in {:.2}s", Elapsed.as_secs_f64());
                if Status == FluxSdk::StatusFailed {
                    self.Fail(&Summary, Text.lines().last().unwrap_or(""));
                } else {
                    self.Inform(&Summary);
                }
                if !Renames.is_empty() && Status == FluxSdk::StatusOk {
                    self.StartOperation(Request::MovePairs(Renames), Purpose::Normal);
                }
                if !Paths.is_empty() || !Text.trim().is_empty() {
                    self.OpenResults(&Title, Paths, Text);
                }
            }
        }
    }

    pub fn OpenResults(&mut self, Title: &str, Paths: Vec<PathBuf>, Text: String) {
        let Pane = self.ActivePane;
        if Paths.is_empty() {
            self.ShowPrompt(Prompt::New(Title, &Text, &["Close"], PromptPurpose::Information));
            return;
        }
        self.OpenTab(Pane, Location::Results { Title: Title.to_string(), Identifier: NewIdentifier() });
        let Visible = self.Visibility();
        let Data = crate::Control::BuildResults(&Paths);
        let Tab = self.TabMut(Pane);
        if let Some(Cancel) = Tab.Cancel.take() {
            Cancel.Cancel();
        }
        Tab.Token = NewIdentifier();
        let mut Order = crate::FileSystem::Sorting::LiveIndices(&Data);
        Tab.Sort.Key = crate::FileSystem::Sorting::SortKey::Folder;
        crate::FileSystem::Sorting::SortOrder(&Data, &mut Order, Tab.Sort);
        Tab.Install(Data, Order, &Visible);
        Tab.State = crate::Control::Panes::LoadState::Ready;
        Tab.Report = Some(crate::Control::Panes::Report { Title: Title.to_string(), Text });
        if !self.Settings.ShowInspector {
            self.Settings.ShowInspector = true;
        }
        self.RefreshInspector();
    }

    pub fn InstallModuleFrom(&mut self, Folder: &Path) {
        match self.Modules.Install(Folder) {
            Ok(Name) => {
                self.Inform(&format!("Module {Name} installed — review and approve it before use"));
                self.Overlay = Some(Overlay::Modules(Default::default()));
                self.Focus = Focus::Overlay;
            }
            Err(Message) => self.Fail("Module installation failed", &Message),
        }
    }

    pub fn DropInto(&mut self, Items: Vec<PathBuf>, Destination: PathBuf, Move: bool) {
        if Items.iter().any(|Item| IsSameOrInside(&Destination, Item)) {
            self.Fail("Cannot drop", "A folder cannot be moved or copied into itself");
            return;
        }
        if Move && Items.iter().all(|Item| Item.parent().is_some_and(|Parent| SamePath(Parent, &Destination))) {
            return;
        }
        let Request = if Move { Request::MoveInto { Sources: Items, Destination } } else { Request::CopyInto { Sources: Items, Destination } };
        self.StartOperation(Request, Purpose::Normal);
    }
}

pub fn CollisionPrompt(Identifier: u64, Query: &CollisionQuery) -> Prompt {
    let Describe = |IsDirectory: bool, Size: u64, Modified: i64| {
        if IsDirectory { format!("Folder · modified {}", FormatTime(Modified)) } else { format!("{} · modified {}", FluxSdk::Utilities::FormatSize(Size), FormatTime(Modified)) }
    };
    let Message = format!(
        "\"{}\" already exists in {}.\n\nExisting:  {}\nIncoming:  {}\nFrom:      {}",
        Query.Destination.file_name().map(|Name| Name.to_string_lossy().into_owned()).unwrap_or_default(),
        Query.Destination.parent().map(|Parent| Parent.display().to_string()).unwrap_or_default(),
        Describe(Query.DestinationIsDirectory, Query.DestinationSize, Query.DestinationModified),
        Describe(Query.SourceIsDirectory, Query.SourceSize, Query.SourceModified),
        Query.Source.display(),
    );
    Prompt::New("Item already exists", &Message, &["Replace", "Skip", "Keep Both", "Cancel"], PromptPurpose::Collision(Identifier)).WithToggle("Do this for all conflicts")
}
