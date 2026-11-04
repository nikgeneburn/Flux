pub mod Actions;
pub mod Commands;
pub mod Input;
pub mod Panes;
pub mod Session;
pub mod Sizes;

use crate::Application::{App, Event, Focus};
use crate::FileSystem::Enumerate;
use crate::FileSystem::Search::{self, Query, SearchStatistics};
use crate::FileSystem::Sorting::{self, SortKey};
use crate::FileSystem::Watcher::ChangeBatch;
use crate::FileSystem::{IsServerPath, Listing, Location, Parent, RawEntry};
use crate::Interface::Overlays::{Overlay, Palette, PaletteMode};
use crate::Platform;
use crate::Services::Config::{Density, ViewMode};
use crate::Services::Workers::{CancelToken, NewIdentifier, Spawn};
use Commands::Command;
use Panes::{LoadState, Tab};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl App {
    pub fn Tab(&self, Pane: usize) -> &Tab {
        self.Panes[Pane].Current()
    }

    pub fn TabMut(&mut self, Pane: usize) -> &mut Tab {
        self.Panes[Pane].CurrentMut()
    }

    pub fn CurrentDirectory(&self, Pane: usize) -> Option<PathBuf> {
        match &self.Tab(Pane).Location {
            Location::Directory(Item) => Some(Item.clone()),
            Location::Search { Root, .. } => Some(Root.clone()),
            _ => None,
        }
    }

    pub fn OtherPane(&self) -> Option<usize> {
        if self.Settings.DualPane { Some(1 - self.ActivePane) } else { None }
    }

    pub fn Navigate(&mut self, Pane: usize, Target: Location) {
        self.NavigateTo(Pane, Target, true);
    }

    pub fn NavigateTo(&mut self, Pane: usize, Target: Location, Record: bool) {
        self.CancelRename();
        let Previous = self.Tab(Pane).Location.clone();
        let FocusName = match (&Previous, &Target) {
            (Location::Directory(Old), Location::Directory(New)) if Parent(Old).is_some_and(|Up| crate::FileSystem::SamePath(&Up, New)) => {
                Some(crate::FileSystem::LeafName(Old).encode_utf16().collect::<Vec<u16>>())
            }
            _ => None,
        };
        let Tab = self.TabMut(Pane);
        if Record && Previous != Target {
            Tab.Back.push(Previous);
            if Tab.Back.len() > 200 {
                Tab.Back.remove(0);
            }
            Tab.Forward.clear();
        }
        Tab.Location = Target;
        Tab.PendingFocus = FocusName;
        Tab.Filter.Clear();
        Tab.FilterQuery = None;
        let Index = self.Panes[Pane].Active;
        self.Panes[Pane].Editing = false;
        self.Panes[Pane].Completions.clear();
        if self.Focus == Focus::Address || self.Focus == Focus::Filter {
            self.Focus = Focus::List;
        }
        self.LoadTab(Pane, Index);
        self.SyncTree();
    }

    pub fn LoadTab(&mut self, Pane: usize, Index: usize) {
        let Visible = self.Visibility();
        let Sink = self.Sink.clone();
        let Token = NewIdentifier();
        let Cancel = CancelToken::New();
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        if let Some(Previous) = Tab.Cancel.take() {
            Previous.Cancel();
        }
        if let (Some(Watch), Some(Watcher)) = (Tab.Watch.take(), &self.Watcher) {
            Watcher.Unwatch(Watch);
        }
        let KeepResults = matches!(Tab.Location, Location::Results { .. });
        let Previous = if KeepResults { Some(std::mem::take(&mut Tab.Listing)) } else { None };
        let PendingFocus = Tab.PendingFocus.take();
        Tab.ResetContent();
        Tab.PendingFocus = PendingFocus;
        Tab.Token = Token;
        Tab.Cancel = Some(Cancel.clone());
        Tab.State = LoadState::Loading;
        Tab.LoadStarted = Some(Instant::now());
        let Sort = Tab.Sort;
        match Tab.Location.clone() {
            Location::Computer => {
                let _ = Spawn("Drives", move || {
                    let Started = Instant::now();
                    let Data = Enumerate::LoadComputer();
                    let mut Order = Sorting::LiveIndices(&Data);
                    Order.sort_by(|&Left, &Right| Data.RelativePath(Left).cmp(Data.RelativePath(Right)));
                    Sink.Send(Event::ListingLoaded { Token, Result: Ok((Data, Order)), Elapsed: Started.elapsed() });
                });
            }
            Location::Directory(Directory) => {
                if let Some(Text) = Directory.to_str() {
                    crate::Services::Diagnostics::SetActivity(Text);
                }
                self.Session.AddRecent(&Directory);
                let _ = Spawn("Enumerate", move || {
                    let Started = Instant::now();
                    let Result = if IsServerPath(&Directory) {
                        Enumerate::LoadServer(&Directory)
                    } else {
                        let PartialSink = Sink.clone();
                        Enumerate::LoadDirectory(&Directory, &Cancel, &mut |Data, From| {
                            let mut Chunk = Listing::New(Data.Root.clone());
                            for Index in From as u32..Data.Entries.len() as u32 {
                                let Item = Data.Entries[Index as usize];
                                Chunk.Push(&[], &RawEntry { Name: Data.Name(Index), Attributes: Item.Attributes, Size: Item.Size, Modified: Item.Modified, Created: Item.Created });
                            }
                            PartialSink.Send(Event::ListingPartial { Token, Chunk });
                        })
                    };
                    if Cancel.IsCancelled() {
                        return;
                    }
                    let Result = Result.map(|Data| {
                        let mut Order = Sorting::LiveIndices(&Data);
                        Sorting::SortOrder(&Data, &mut Order, Sort);
                        (Data, Order)
                    });
                    Sink.Send(Event::ListingLoaded { Token, Result, Elapsed: Started.elapsed() });
                });
                self.RequestFreeSpace(Pane);
            }
            Location::Search { Root, Query: Text } => {
                let Parsed = Query::Parse(&Text);
                let IncludeHidden = Visible.ShowHidden;
                let _ = Spawn("Search", move || {
                    let BatchSink = Sink.clone();
                    let Statistics = Search::Run(&Root, &Parsed, IncludeHidden, &Cancel, &mut |Chunk| BatchSink.Send(Event::SearchBatch { Token, Chunk }));
                    Sink.Send(Event::SearchDone { Token, Statistics });
                });
            }
            Location::Results { .. } => {
                let Items: Vec<PathBuf> = match &Previous {
                    Some(Data) => (0..Data.Entries.len() as u32).filter(|&Index| Data.Entries[Index as usize].Flags & crate::FileSystem::FlagRemoved == 0).map(|Index| Data.FullPath(Index)).collect(),
                    None => Vec::new(),
                };
                let _ = Spawn("Results", move || {
                    let Started = Instant::now();
                    let Data = BuildResults(&Items);
                    let mut Order = Sorting::LiveIndices(&Data);
                    Sorting::SortOrder(&Data, &mut Order, Sort);
                    Sink.Send(Event::ListingLoaded { Token, Result: Ok((Data, Order)), Elapsed: Started.elapsed() });
                });
            }
        }
        self.Invalidate();
    }

    fn FindToken(&self, Token: u64) -> Option<(usize, usize)> {
        (0..2).find_map(|Pane| self.Panes[Pane].FindTab(Token).map(|Index| (Pane, Index)))
    }

    pub fn OnListingPartial(&mut self, Token: u64, Chunk: Listing) {
        let Visible = self.Visibility();
        let Some((Pane, Index)) = self.FindToken(Token) else { return };
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        Tab.Append(&Chunk, 0, &Visible);
    }

    pub fn OnListingLoaded(&mut self, Token: u64, Result: Result<(Listing, Vec<u32>), u32>, Elapsed: Duration) {
        let Visible = self.Visibility();
        let Some((Pane, Index)) = self.FindToken(Token) else { return };
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        Tab.Cancel = None;
        let Requested = Tab.LoadStarted.take();
        match Result {
            Ok((Data, Order)) => {
                let Count = Data.Entries.len();
                Tab.Install(Data, Order, &Visible);
                Tab.State = LoadState::Ready;
                let Cursor = Tab.Cursor;
                if let Location::Directory(Directory) = Tab.Location.clone() {
                    if let Some(Watcher) = &mut self.Watcher {
                        if !IsServerPath(&Directory) {
                            self.Panes[Pane].Tabs[Index].Watch = Some(Watcher.Watch(Directory));
                        }
                    }
                }
                if let (Some(Start), true) = (Requested, Index == self.Panes[Pane].Active) {
                    self.Performance.RecordNavigation(Start.elapsed(), Elapsed, Count);
                }
                if Index == self.Panes[Pane].Active {
                    self.WatchFolderSizes(Pane);
                } else {
                    self.ApplyKnownSizes(Pane, Index);
                }
                if let Some(Cursor) = Cursor {
                    self.RevealIndex(Pane, Cursor);
                }
            }
            Err(1223) => {}
            Err(Code) => {
                Tab.State = LoadState::Failed(crate::FileSystem::ErrorText(Code));
            }
        }
        if Pane == self.ActivePane {
            self.RefreshInspector();
        }
    }

    pub fn OnSearchBatch(&mut self, Token: u64, Chunk: Listing) {
        let Visible = self.Visibility();
        let Some((Pane, Index)) = self.FindToken(Token) else { return };
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        Tab.Append(&Chunk, 0, &Visible);
        Tab.Status = Some(format!("Searching… {} found", Tab.Listing.Len()));
    }

    pub fn OnSearchDone(&mut self, Token: u64, Statistics: SearchStatistics) {
        let Visible = self.Visibility();
        let Some((Pane, Index)) = self.FindToken(Token) else { return };
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        Tab.Cancel = None;
        Tab.State = LoadState::Ready;
        Tab.LoadStarted = None;
        let mut Sorted = Sorting::LiveIndices(&Tab.Listing);
        Sorting::SortOrder(&Tab.Listing, &mut Sorted, Tab.Sort);
        Tab.Sorted = Sorted;
        Tab.RebuildOrder(&Visible);
        Tab.Status = Some(format!(
            "{} match{} · {} folders scanned · {} entries · {:.2}s{}{}",
            Statistics.Matches,
            if Statistics.Matches == 1 { "" } else { "es" },
            Statistics.Directories,
            Statistics.Scanned,
            Statistics.Elapsed.as_secs_f64(),
            if Statistics.Errors > 0 { format!(" · {} inaccessible", Statistics.Errors) } else { String::new() },
            if Statistics.Truncated { " · result limit reached" } else { "" }
        ));
    }

    pub fn OnDirectoryChanged(&mut self, Batch: ChangeBatch) {
        let Visible = self.Visibility();
        for Pane in 0..2 {
            for Index in 0..self.Panes[Pane].Tabs.len() {
                if self.Panes[Pane].Tabs[Index].Watch != Some(Batch.Watch) {
                    continue;
                }
                if Batch.Overflow || Batch.Records.len() > 512 {
                    self.Panes[Pane].Tabs[Index].Watch = None;
                    let Focused = {
                        let Tab = &self.Panes[Pane].Tabs[Index];
                        Tab.Cursor.map(|Cursor| Tab.Listing.Name(Cursor).to_vec())
                    };
                    let Scroll = self.Panes[Pane].Tabs[Index].Scroll;
                    self.Panes[Pane].Tabs[Index].PendingFocus = Focused;
                    self.LoadTab(Pane, Index);
                    self.Panes[Pane].Tabs[Index].Scroll = Scroll;
                    continue;
                }
                let Tab = &mut self.Panes[Pane].Tabs[Index];
                for Record in &Batch.Records {
                    Tab.ApplyChange(&Record.Name, Record.Entry.as_ref(), &Visible);
                    if Record.Entry.is_some() {
                        let Full = Tab.Listing.Root.as_ref().map(|Root| Root.join(std::ffi::OsString::from_wide(&Record.Name)));
                        if let Some(Full) = Full {
                            self.Media.Forget(&Full);
                        }
                    }
                }
                if Tab.Listing.RemovedCount > 4096 && Tab.Listing.RemovedCount > Tab.Listing.Entries.len() / 2 {
                    let Cursor = Tab.Cursor.map(|Cursor| Tab.Listing.Name(Cursor).to_vec());
                    let Selected: Vec<Vec<u16>> = Tab.SelectedIndices().into_iter().map(|Item| Tab.Listing.Name(Item).to_vec()).collect();
                    Tab.Listing.Compact();
                    Tab.Sorted = Sorting::LiveIndices(&Tab.Listing);
                    Tab.SelectedCount = 0;
                    for Item in &mut Tab.Listing.Entries {
                        Item.Flags = 0;
                    }
                    Tab.Resort(&Visible);
                    for Name in Selected {
                        if let Some(Found) = Tab.Listing.Find(&Name) {
                            Tab.SetSelected(Found, true);
                        }
                    }
                    Tab.Cursor = Cursor.and_then(|Name| Tab.Listing.Find(&Name));
                    Tab.Anchor = Tab.Cursor;
                }
                if Index == self.Panes[Pane].Active {
                    self.WatchFolderSizes(Pane);
                } else {
                    self.ApplyKnownSizes(Pane, Index);
                }
                if Pane == self.ActivePane && self.Rename.is_none() {
                    self.RefreshInspector();
                }
            }
        }
        self.UpdateRenameAfterChange();
    }

    fn UpdateRenameAfterChange(&mut self) {
        let Some(State) = &self.Rename else { return };
        let Pane = State.Pane;
        let Valid = self.Panes[Pane].Tabs.iter().any(|Tab| {
            Tab.Identifier == State.TabIdentifier && Tab.Listing.Entries.get(State.Index as usize).is_some_and(|Item| Item.Flags & crate::FileSystem::FlagRemoved == 0)
        });
        if !Valid {
            self.Rename = None;
            self.Focus = Focus::List;
        }
    }

    pub fn RequestFreeSpace(&mut self, Pane: usize) {
        let Some(Directory) = self.CurrentDirectory(Pane) else {
            self.Panes[Pane].FreeSpace = None;
            return;
        };
        let Sink = self.Sink.clone();
        self.Background.Submit(move || {
            let Value = Enumerate::FreeSpace(&Directory);
            Sink.Send(Event::FreeSpace { Directory, Value });
        });
    }

    pub fn RequestCompletions(&mut self, Pane: usize) {
        let Text = self.Panes[Pane].Address.Value();
        let Token = NewIdentifier();
        self.Panes[Pane].CompletionToken = Token;
        let Base = self.CurrentDirectory(Pane);
        let Expanded = crate::FileSystem::ExpandEnvironment(Text.trim().trim_matches('"'));
        let Split = Expanded.rfind(['\\', '/']);
        let Some(Split) = Split else {
            self.Panes[Pane].Completions.clear();
            return;
        };
        let (Directory, Prefix) = (Expanded[..=Split].to_string(), Expanded[Split + 1..].to_string());
        let Some(Resolved) = crate::FileSystem::Resolve(&Directory, Base.as_deref()) else { return };
        let Sink = self.Sink.clone();
        let ShowHidden = self.Settings.ShowHidden;
        self.Background.Submit(move || {
            let Folded = Sorting::FoldString(&Prefix);
            let Names = Enumerate::Subdirectories(&Resolved, ShowHidden, 20000).unwrap_or_default();
            let Items: Vec<String> = Names
                .into_iter()
                .filter(|Name| {
                    let Wide: Vec<u16> = Name.to_string_lossy().encode_utf16().collect();
                    Sorting::StartsWithFolded(&Wide, &Folded)
                })
                .take(12)
                .map(|Name| format!("{}{}", Directory, Name.to_string_lossy()))
                .collect();
            Sink.Send(Event::Completions { Pane, Token, Items });
        });
    }

    pub fn ApplyFilter(&mut self, Pane: usize) {
        let Visible = self.Visibility();
        let Tab = self.TabMut(Pane);
        let Text = Tab.Filter.Value();
        Tab.FilterQuery = if Text.trim().is_empty() { None } else { Some(Query::Parse(&Text)) };
        Tab.RebuildOrder(&Visible);
        Tab.Scroll.Jump(0.0);
        if let Some(First) = Tab.Order.first().copied() {
            if Tab.Cursor.is_none() {
                Tab.Cursor = Some(First);
                Tab.Anchor = Some(First);
            }
        }
        self.Invalidate();
    }

    pub fn StartSearch(&mut self, Pane: usize) {
        let Text = self.Tab(Pane).Filter.Value();
        if Text.trim().is_empty() {
            return;
        }
        let Root = match &self.Tab(Pane).Location {
            Location::Directory(Item) => Item.clone(),
            Location::Search { Root, .. } => Root.clone(),
            _ => {
                self.Inform("Recursive search needs a folder; filtering the current list instead");
                return;
            }
        };
        self.Navigate(Pane, Location::Search { Root, Query: Text.trim().to_string() });
        self.Focus = Focus::List;
    }

    pub fn OpenEntry(&mut self, Pane: usize, Index: u32, NewTab: bool, InOtherPane: bool) {
        let Tab = self.Tab(Pane);
        let Item = Tab.Listing.FullPath(Index);
        let IsDirectory = Tab.Listing.IsDirectory(Index);
        let Target = if IsDirectory {
            Some(Item.clone())
        } else if Tab.Listing.Extension(Index).iter().map(|&Unit| Sorting::Fold(Unit)).eq("lnk".encode_utf16()) {
            Platform::Shell::ShortcutTarget(&Item).filter(|Resolved| Enumerate::Stat(Resolved).is_some_and(|Data| Data.Attributes & crate::FileSystem::AttributeDirectory != 0))
        } else {
            None
        };
        match Target {
            Some(Directory) => {
                let Destination = if InOtherPane && self.Settings.DualPane { 1 - Pane } else { Pane };
                if NewTab {
                    self.OpenTab(Destination, Location::Directory(Directory));
                } else {
                    self.Navigate(Destination, Location::Directory(Directory));
                }
            }
            None => self.Deferred.push(crate::Application::Deferred::Open(Item)),
        }
    }

    pub fn OpenTab(&mut self, Pane: usize, Target: Location) {
        let Sort = self.DefaultSort();
        let Template = Tab::New(Target, self.Tab(Pane).View, Sort, self.Tab(Pane).ItemSize);
        let Insert = self.Panes[Pane].Active + 1;
        self.Panes[Pane].Tabs.insert(Insert, Template);
        self.Panes[Pane].Active = Insert;
        self.ActivePane = Pane;
        self.LoadTab(Pane, Insert);
        self.Focus = Focus::List;
    }

    pub fn OpenExternal(&mut self, Text: &str) {
        let Pane = self.ActivePane;
        let (Target, Select) = Platform::Integration::ResolveTarget(Text).unwrap_or_else(|| (self.HomeLocation(), None));
        if Select.is_some() || self.Tab(Pane).Location != Target {
            self.OpenTab(Pane, Target);
        }
        if let Some(Name) = Select.as_ref().and_then(|Item| Item.file_name()) {
            self.TabMut(Pane).PendingFocus = Some(Name.encode_wide().collect());
        }
    }

    pub fn CloseTab(&mut self, Pane: usize, Index: usize) {
        if Index >= self.Panes[Pane].Tabs.len() {
            return;
        }
        if self.Panes[Pane].Tabs.len() == 1 {
            let Home = self.HomeLocation();
            self.Navigate(Pane, Home);
            return;
        }
        let Removed = self.Panes[Pane].Tabs.remove(Index);
        if let Some(Cancel) = &Removed.Cancel {
            Cancel.Cancel();
        }
        if let (Some(Watch), Some(Watcher)) = (Removed.Watch, &self.Watcher) {
            Watcher.Unwatch(Watch);
        }
        self.Panes[Pane].Closed.push(Removed.Location);
        if self.Panes[Pane].Closed.len() > 32 {
            self.Panes[Pane].Closed.remove(0);
        }
        let Active = self.Panes[Pane].Active;
        if Active > Index || Active >= self.Panes[Pane].Tabs.len() {
            self.Panes[Pane].Active = Active.saturating_sub(1);
        }
        self.CancelRename();
        self.RequestFreeSpace(Pane);
        self.RefreshInspector();
        self.SyncTree();
        self.Invalidate();
    }

    pub fn ActivateTab(&mut self, Pane: usize, Index: usize) {
        if Index < self.Panes[Pane].Tabs.len() {
            self.CancelRename();
            self.Panes[Pane].Active = Index;
            self.ActivePane = Pane;
            self.Panes[Pane].Editing = false;
            self.RequestFreeSpace(Pane);
            self.WatchFolderSizes(Pane);
            self.RefreshInspector();
            self.SyncTree();
            self.Invalidate();
        }
    }

    pub fn MoveTab(&mut self, Pane: usize, From: usize, To: usize) {
        let Tabs = &mut self.Panes[Pane].Tabs;
        if From >= Tabs.len() || To >= Tabs.len() || From == To {
            return;
        }
        let Item = Tabs.remove(From);
        Tabs.insert(To, Item);
        let Active = &mut self.Panes[Pane].Active;
        if *Active == From {
            *Active = To;
        } else if From < *Active && To >= *Active {
            *Active -= 1;
        } else if From > *Active && To <= *Active {
            *Active += 1;
        }
        self.Invalidate();
    }

    pub fn TransferTab(&mut self, From: usize, Index: usize) {
        if !self.Settings.DualPane || self.Panes[From].Tabs.len() < 2 {
            return;
        }
        let Item = self.Panes[From].Tabs.remove(Index);
        if self.Panes[From].Active >= self.Panes[From].Tabs.len() || self.Panes[From].Active > Index {
            self.Panes[From].Active = self.Panes[From].Active.saturating_sub(1);
        }
        let Target = 1 - From;
        self.Panes[Target].Tabs.push(Item);
        self.Panes[Target].Active = self.Panes[Target].Tabs.len() - 1;
        self.ActivePane = Target;
        self.Invalidate();
    }

    pub fn RevealIndex(&mut self, Pane: usize, Index: u32) {
        let Area = self.ViewAreas[Pane];
        let Sizes = self.Theme.Sizes.clone();
        let Tab = self.TabMut(Pane);
        let Some(Position) = Tab.PositionOf(Index) else { return };
        crate::Interface::FileView::Reveal(Tab, Area, &Sizes, Position);
    }

    pub fn SetView(&mut self, Mode: ViewMode) {
        let Pane = self.ActivePane;
        let Tab = self.TabMut(Pane);
        Tab.View = Mode;
        Tab.Scroll.Jump(0.0);
        Tab.ScrollX = 0.0;
        self.Settings.DefaultView = Mode;
        if let Some(Cursor) = self.Tab(Pane).Cursor {
            self.RevealIndex(Pane, Cursor);
        }
        self.Invalidate();
    }

    pub fn SortBy(&mut self, Pane: usize, Key: SortKey) {
        let Visible = self.Visibility();
        let Started = Instant::now();
        self.TabMut(Pane).SetSort(Key, &Visible);
        self.Performance.RecordSort(Started.elapsed(), self.Tab(Pane).Listing.Len());
        if let Some(Cursor) = self.Tab(Pane).Cursor {
            self.RevealIndex(Pane, Cursor);
        }
        self.Invalidate();
    }

    pub fn RefilterAll(&mut self) {
        let Visible = self.Visibility();
        for Pane in &mut self.Panes {
            for Tab in &mut Pane.Tabs {
                Tab.Sort.FoldersFirst = self.Settings.FoldersFirst;
                Tab.Resort(&Visible);
            }
        }
        self.Invalidate();
    }

    pub fn OpenPalette(&mut self, Mode: PaletteMode) {
        let mut State = Palette::New(Mode);
        State.Refresh(self);
        self.Overlay = Some(Overlay::Palette(State));
        self.Focus = Focus::Overlay;
        self.Invalidate();
    }

    pub fn Execute(&mut self, Item: Command) {
        let Pane = self.ActivePane;
        match Item {
            Command::Back => {
                let Tab = self.TabMut(Pane);
                if let Some(Target) = Tab.Back.pop() {
                    let Current = Tab.Location.clone();
                    Tab.Forward.push(Current);
                    self.NavigateTo(Pane, Target, false);
                }
            }
            Command::Forward => {
                let Tab = self.TabMut(Pane);
                if let Some(Target) = Tab.Forward.pop() {
                    let Current = Tab.Location.clone();
                    Tab.Back.push(Current);
                    self.NavigateTo(Pane, Target, false);
                }
            }
            Command::Up => match self.Tab(Pane).Location.clone() {
                Location::Directory(Directory) => match Parent(&Directory) {
                    Some(Up) => self.Navigate(Pane, Location::Directory(Up)),
                    None => self.Navigate(Pane, Location::Computer),
                },
                Location::Search { Root, .. } => self.Navigate(Pane, Location::Directory(Root)),
                Location::Results { .. } => {
                    let Home = self.HomeLocation();
                    self.Navigate(Pane, Home);
                }
                Location::Computer => {}
            },
            Command::Refresh => {
                let Index = self.Panes[Pane].Active;
                let Focused = self.Tab(Pane).Cursor.map(|Cursor| self.Tab(Pane).Listing.Name(Cursor).to_vec());
                self.TabMut(Pane).PendingFocus = Focused;
                self.LoadTab(Pane, Index);
                self.Media.CancelPending();
            }
            Command::FocusAddress => {
                let Text = match &self.Tab(Pane).Location {
                    Location::Directory(Item) => Item.display().to_string(),
                    Location::Search { Root, .. } => Root.display().to_string(),
                    _ => String::new(),
                };
                let Target = &mut self.Panes[Pane];
                Target.Editing = true;
                Target.Address.SetText(&Text);
                self.Panes[Pane].Address.SelectAll();
                self.Panes[Pane].Completions.clear();
                self.Focus = Focus::Address;
            }
            Command::FocusFilter => {
                self.Focus = Focus::Filter;
                self.TabMut(Pane).Filter.SelectAll();
            }
            Command::GoTo => self.OpenPalette(PaletteMode::GoTo),
            Command::Palette => self.OpenPalette(PaletteMode::Commands),
            Command::ViewMenu => {
                let Area = self.Hits.AreaOf(crate::Interface::Hit::Run(Command::ViewMenu)).unwrap_or(crate::Interface::Rect::New(self.Width - 260.0, 0.0, 40.0, 30.0));
                self.ShowOptionsMenu(Pane, (Area.Right - 230.0).max(4.0), Area.Bottom + 4.0);
            }
            Command::NewTab => {
                let Target = self.Tab(Pane).Location.clone();
                let Target = if matches!(Target, Location::Results { .. }) { self.HomeLocation() } else { Target };
                self.OpenTab(Pane, Target);
            }
            Command::DuplicateTab => {
                let Target = self.Tab(Pane).Location.clone();
                self.OpenTab(Pane, Target);
            }
            Command::CloseTab => {
                let Index = self.Panes[Pane].Active;
                self.CloseTab(Pane, Index);
            }
            Command::ReopenTab => {
                if let Some(Target) = self.Panes[Pane].Closed.pop() {
                    self.OpenTab(Pane, Target);
                }
            }
            Command::NextTab | Command::PreviousTab => {
                let Count = self.Panes[Pane].Tabs.len();
                let Current = self.Panes[Pane].Active;
                let Next = if Item == Command::NextTab { (Current + 1) % Count } else { (Current + Count - 1) % Count };
                self.ActivateTab(Pane, Next);
            }
            Command::ToggleDualPane => {
                self.Settings.DualPane = !self.Settings.DualPane;
                self.WatchFolderSizes(1);
                if !self.Settings.DualPane {
                    self.ActivePane = 0;
                } else if self.Panes[1].Tabs.iter().all(|Tab| Tab.State == LoadState::Ready && Tab.Listing.Entries.is_empty()) {
                    let Target = self.Tab(0).Location.clone();
                    self.Navigate(1, Target);
                }
                self.SaveSettings();
            }
            Command::SwitchPane => {
                if self.Settings.DualPane {
                    self.ActivePane = 1 - self.ActivePane;
                    self.CancelRename();
                    self.RefreshInspector();
                    self.SyncTree();
                }
            }
            Command::SyncPanes => {
                if let Some(Other) = self.OtherPane() {
                    let Target = self.Tab(Pane).Location.clone();
                    self.Navigate(Other, Target);
                }
            }
            Command::SwapPanes => {
                if self.Settings.DualPane {
                    self.Panes.swap(0, 1);
                    self.ActivePane = 1 - self.ActivePane;
                }
            }
            Command::ViewDetails => self.SetView(ViewMode::Details),
            Command::ViewList => self.SetView(ViewMode::List),
            Command::ViewGrid => self.SetView(ViewMode::Grid),
            Command::ViewThumbnails => self.SetView(ViewMode::Thumbnails),
            Command::ToggleHidden => {
                self.Settings.ShowHidden = !self.Settings.ShowHidden;
                self.RefilterAll();
                self.SaveSettings();
                self.Inform(if self.Settings.ShowHidden { "Hidden items are shown" } else { "Hidden items are hidden" });
            }
            Command::ToggleSidebar => {
                self.Settings.ShowSidebar = !self.Settings.ShowSidebar;
                self.SaveSettings();
            }
            Command::ToggleInspector => {
                self.Settings.ShowInspector = !self.Settings.ShowInspector;
                self.RefreshInspector();
                self.SaveSettings();
            }
            Command::ToggleFolderTree => {
                self.Settings.ShowFolderTree = !self.Settings.ShowFolderTree;
                if !self.Settings.ShowSidebar {
                    self.Settings.ShowSidebar = true;
                }
                self.SyncTree();
                self.SaveSettings();
            }
            Command::CycleTheme => {
                let Names = crate::Interface::Theme::Theme::Available(&self.Paths.ThemesDirectory());
                let Current = Names.iter().position(|Name| Name.eq_ignore_ascii_case(&self.Settings.Theme)).unwrap_or(0);
                self.Settings.Theme = Names[(Current + 1) % Names.len()].clone();
                self.ReloadTheme();
                self.SaveSettings();
                self.Inform(&format!("Theme: {}", self.Settings.Theme));
            }
            Command::CycleDensity => {
                self.Settings.Density = match self.Settings.Density {
                    Density::Compact => Density::Normal,
                    Density::Normal => Density::Comfortable,
                    Density::Comfortable => Density::Compact,
                };
                self.ReloadTheme();
                self.SaveSettings();
            }
            Command::ToggleFoldersFirst => {
                self.Settings.FoldersFirst = !self.Settings.FoldersFirst;
                self.RefilterAll();
                self.SaveSettings();
            }
            Command::ToggleSmoothScrolling => {
                self.Settings.SmoothScrolling = !self.Settings.SmoothScrolling;
                self.SaveSettings();
                self.Inform(if self.Settings.SmoothScrolling { "Smooth scrolling on" } else { "Smooth scrolling off" });
            }
            Command::ToggleAnimations => {
                self.Settings.Animations = !self.Settings.Animations;
                self.Motion.Enabled = self.Settings.Animations;
                self.SaveSettings();
                self.Inform(if self.Settings.Animations { "Animations on" } else { "Animations off" });
            }
            Command::SortName => self.SortBy(Pane, SortKey::Name),
            Command::SortSize => self.SortBy(Pane, SortKey::Size),
            Command::SortModified => self.SortBy(Pane, SortKey::Modified),
            Command::SortType => self.SortBy(Pane, SortKey::Type),
            Command::SortCreated => self.SortBy(Pane, SortKey::Created),
            Command::ZoomIn | Command::ZoomOut => {
                let Tab = self.TabMut(Pane);
                let Step = if Item == Command::ZoomIn { 1.2 } else { 1.0 / 1.2 };
                Tab.ItemSize = (Tab.ItemSize * Step).clamp(64.0, 320.0);
                self.Settings.ThumbnailSize = self.Tab(Pane).ItemSize;
                if matches!(self.Tab(Pane).View, ViewMode::Details | ViewMode::List) {
                    self.SetView(ViewMode::Thumbnails);
                }
            }
            Command::TogglePerformance => {
                self.Settings.ShowPerformance = !self.Settings.ShowPerformance;
                let Enabled = self.Settings.ShowPerformance;
                self.SetPerformanceTimer(Enabled || self.Bench.is_some());
                self.Performance.Sample(self.Media.CacheBytes());
            }
            Command::Exit => Platform::Close(self.Window),
            Other => self.ExecuteAction(Other),
        }
        self.Invalidate();
    }

    pub fn SyncTree(&mut self) {
        if !self.Settings.ShowSidebar || !self.Settings.ShowFolderTree {
            return;
        }
        let Some(Directory) = self.CurrentDirectory(self.ActivePane) else { return };
        let Requests = self.Sidebar.Reveal(&Directory);
        for Folder in Requests {
            self.RequestTreeChildren(Folder);
        }
    }

    pub fn RequestTreeChildren(&mut self, Folder: PathBuf) {
        if !self.Sidebar.BeginLoading(&Folder) {
            return;
        }
        let Sink = self.Sink.clone();
        let ShowHidden = self.Settings.ShowHidden;
        self.Background.Submit(move || {
            let Children = Enumerate::Subdirectories(&Folder, ShowHidden, 50000);
            Sink.Send(Event::TreeChildren { Directory: Folder, Children });
        });
    }
}

pub fn BuildResults(Items: &[PathBuf]) -> Listing {
    let mut Data = Listing::New(None);
    for Item in Items {
        let Some(Entry) = Enumerate::Stat(Item) else { continue };
        let Folder: Vec<u16> = Item.parent().map(|Directory| Directory.as_os_str().encode_wide().collect()).unwrap_or_default();
        Data.Push(&Folder, &RawEntry { Name: &Entry.Name, Attributes: Entry.Attributes, Size: Entry.Size, Modified: Entry.Modified, Created: Entry.Created });
    }
    Data
}
