use crate::FileSystem::Enumerate::OwnedEntry;
use crate::FileSystem::Search::Query;
use crate::FileSystem::Sorting::{self, SortKey, SortSpec};
use crate::FileSystem::{AttributeHidden, AttributeSystem, FlagRemoved, FlagSelected, Listing, Location, RawEntry};
use crate::Interface::Widgets::{LineEdit, Scroller};
use crate::Services::Config::ViewMode;
use crate::Services::Workers::{CancelToken, NewIdentifier};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    Ready,
    Loading,
    Failed(String),
}

pub struct Report {
    pub Title: String,
    pub Text: String,
}

pub struct Tab {
    pub Identifier: u64,
    pub Location: Location,
    pub Back: Vec<Location>,
    pub Forward: Vec<Location>,
    pub Listing: Listing,
    pub Sorted: Vec<u32>,
    pub Order: Vec<u32>,
    pub State: LoadState,
    pub Token: u64,
    pub Cancel: Option<CancelToken>,
    pub Watch: Option<u64>,
    pub LoadStarted: Option<Instant>,
    pub View: ViewMode,
    pub Sort: SortSpec,
    pub Cursor: Option<u32>,
    pub Anchor: Option<u32>,
    pub SelectedCount: usize,
    pub Scroll: Scroller,
    pub ScrollX: f32,
    pub Filter: LineEdit,
    pub FilterQuery: Option<Query>,
    pub PendingFocus: Option<Vec<u16>>,
    pub Report: Option<Report>,
    pub Status: Option<String>,
    pub ItemSize: f32,
}

pub struct Visibility {
    pub ShowHidden: bool,
    pub ShowSystem: bool,
}

impl Visibility {
    pub fn Allows(&self, Attributes: u32) -> bool {
        if Attributes & AttributeHidden != 0 && !self.ShowHidden {
            return false;
        }
        !(Attributes & AttributeHidden != 0 && Attributes & AttributeSystem != 0 && !self.ShowSystem)
    }
}

impl Tab {
    pub fn New(Location: Location, View: ViewMode, Sort: SortSpec, ItemSize: f32) -> Tab {
        Tab {
            Identifier: NewIdentifier(),
            Location,
            Back: Vec::new(),
            Forward: Vec::new(),
            Listing: Listing::default(),
            Sorted: Vec::new(),
            Order: Vec::new(),
            State: LoadState::Ready,
            Token: 0,
            Cancel: None,
            Watch: None,
            LoadStarted: None,
            View,
            Sort,
            Cursor: None,
            Anchor: None,
            SelectedCount: 0,
            Scroll: Scroller::default(),
            ScrollX: 0.0,
            Filter: LineEdit::New(),
            FilterQuery: None,
            PendingFocus: None,
            Report: None,
            Status: None,
            ItemSize,
        }
    }

    pub fn Title(&self) -> String {
        self.Location.Title()
    }

    pub fn ResetContent(&mut self) {
        self.Listing = Listing::default();
        self.Sorted.clear();
        self.Order.clear();
        self.Cursor = None;
        self.Anchor = None;
        self.SelectedCount = 0;
        self.Scroll.Jump(0.0);
        self.ScrollX = 0.0;
        self.Report = None;
        self.Status = None;
    }

    fn Passes(&self, Rules: &Visibility, Index: u32) -> bool {
        let Item = &self.Listing.Entries[Index as usize];
        if Item.Flags & FlagRemoved != 0 || !Rules.Allows(Item.Attributes) {
            return false;
        }
        match &self.FilterQuery {
            Some(Filter) => Filter.Matches(self.Listing.Name(Index), self.Listing.Extension(Index), Item.Attributes, Item.Size, Item.Modified),
            None => true,
        }
    }

    pub fn RebuildOrder(&mut self, Rules: &Visibility) {
        let mut Order = std::mem::take(&mut self.Order);
        Order.clear();
        Order.extend(self.Sorted.iter().copied().filter(|&Index| self.Passes(Rules, Index)));
        self.Order = Order;
        let mut Selected = 0usize;
        let mut Visible = vec![false; self.Listing.Entries.len()];
        for &Index in &self.Order {
            Visible[Index as usize] = true;
        }
        for (Index, Item) in self.Listing.Entries.iter_mut().enumerate() {
            if Item.Flags & FlagSelected != 0 {
                if Visible[Index] {
                    Selected += 1;
                } else {
                    Item.Flags &= !FlagSelected;
                }
            }
        }
        self.SelectedCount = Selected;
        if self.Cursor.is_some_and(|Cursor| !Visible.get(Cursor as usize).copied().unwrap_or(false)) {
            self.Cursor = self.Order.first().copied();
        }
        if self.Anchor.is_some_and(|Anchor| !Visible.get(Anchor as usize).copied().unwrap_or(false)) {
            self.Anchor = self.Cursor;
        }
    }

    pub fn Resort(&mut self, Rules: &Visibility) {
        let mut Sorted = std::mem::take(&mut self.Sorted);
        Sorting::SortOrder(&self.Listing, &mut Sorted, self.Sort);
        self.Sorted = Sorted;
        self.RebuildOrder(Rules);
    }

    pub fn SetSort(&mut self, Key: SortKey, Rules: &Visibility) {
        if self.Sort.Key == Key {
            self.Sort.Descending = !self.Sort.Descending;
        } else {
            self.Sort.Key = Key;
            self.Sort.Descending = matches!(Key, SortKey::Size | SortKey::Modified | SortKey::Created);
        }
        self.Resort(Rules);
    }

    pub fn Install(&mut self, Data: Listing, Sorted: Vec<u32>, Rules: &Visibility) {
        self.Listing = Data;
        self.Sorted = Sorted;
        self.SelectedCount = 0;
        self.Cursor = None;
        self.Anchor = None;
        self.RebuildOrder(Rules);
        if let Some(Name) = self.PendingFocus.take() {
            if let Some(Index) = self.Listing.Find(&Name) {
                if self.Order.contains(&Index) {
                    self.Cursor = Some(Index);
                    self.Anchor = Some(Index);
                    self.SetSelected(Index, true);
                }
            }
        }
        if self.Cursor.is_none() {
            self.Cursor = self.Order.first().copied();
            self.Anchor = self.Cursor;
        }
    }

    pub fn FocusName(&mut self, Name: &[u16], Keep: bool) -> bool {
        let Some(Index) = self.Listing.Find(Name).filter(|Index| self.Order.contains(Index)) else { return false };
        if !Keep {
            self.ClearSelection();
        }
        self.SetSelected(Index, true);
        self.Cursor = Some(Index);
        self.Anchor = Some(Index);
        true
    }

    pub fn Append(&mut self, Chunk: &Listing, From: usize, Rules: &Visibility) {
        if self.Listing.Root.is_none() && self.Listing.Entries.is_empty() {
            self.Listing.Root = Chunk.Root.clone();
        }
        for Index in From as u32..Chunk.Entries.len() as u32 {
            let Item = Chunk.Entries[Index as usize];
            let Relative = Chunk.RelativePath(Index);
            let NameOffset = Item.NameOffset as usize;
            let Prefix = if NameOffset > 0 { &Relative[..NameOffset - 1] } else { &Relative[..0] };
            let Raw = RawEntry { Name: &Relative[NameOffset..], Attributes: Item.Attributes, Size: Item.Size, Modified: Item.Modified, Created: Item.Created };
            let NewIndex = self.Listing.Push(Prefix, &Raw);
            self.Sorted.push(NewIndex);
            if self.Passes(Rules, NewIndex) {
                self.Order.push(NewIndex);
            }
        }
        if self.Cursor.is_none() {
            self.Cursor = self.Order.first().copied();
            self.Anchor = self.Cursor;
        }
    }

    pub fn ApplyChange(&mut self, Name: &[u16], Entry: Option<&OwnedEntry>, Rules: &Visibility) {
        match (self.Listing.Find(Name), Entry) {
            (Some(Index), None) => self.RemoveEntry(Index),
            (Some(Index), Some(Data)) => {
                self.DetachEntry(Index);
                let Item = &mut self.Listing.Entries[Index as usize];
                Item.Attributes = Data.Attributes;
                Item.Size = Data.Size;
                Item.Modified = Data.Modified;
                Item.Created = Data.Created;
                self.AttachEntry(Index, Rules);
            }
            (None, Some(Data)) => {
                let Raw = RawEntry { Name: &Data.Name, Attributes: Data.Attributes, Size: Data.Size, Modified: Data.Modified, Created: Data.Created };
                let Index = self.Listing.Push(&[], &Raw);
                self.AttachEntry(Index, Rules);
                if self.PendingFocus.as_deref().is_some_and(|Wanted| Sorting::EqualIgnoreCase(Wanted, &Data.Name)) {
                    self.PendingFocus = None;
                    if self.Order.contains(&Index) {
                        self.ClearSelection();
                        self.Cursor = Some(Index);
                        self.Anchor = Some(Index);
                        self.SetSelected(Index, true);
                    }
                }
            }
            (None, None) => {}
        }
    }

    fn DetachEntry(&mut self, Index: u32) -> bool {
        if let Some(Position) = self.Sorted.iter().position(|&Existing| Existing == Index) {
            self.Sorted.remove(Position);
        }
        match self.Order.iter().position(|&Existing| Existing == Index) {
            Some(Position) => {
                self.Order.remove(Position);
                true
            }
            None => false,
        }
    }

    fn AttachEntry(&mut self, Index: u32, Rules: &Visibility) {
        Sorting::InsertSorted(&self.Listing, &mut self.Sorted, self.Sort, Index);
        if self.Passes(Rules, Index) {
            Sorting::InsertSorted(&self.Listing, &mut self.Order, self.Sort, Index);
        } else if self.Listing.Entries[Index as usize].Flags & FlagSelected != 0 {
            self.Listing.Entries[Index as usize].Flags &= !FlagSelected;
            self.SelectedCount = self.SelectedCount.saturating_sub(1);
        }
    }

    fn RemoveEntry(&mut self, Index: u32) {
        let Position = self.Order.iter().position(|&Existing| Existing == Index);
        self.DetachEntry(Index);
        if self.Listing.Entries[Index as usize].Flags & FlagSelected != 0 {
            self.SelectedCount = self.SelectedCount.saturating_sub(1);
        }
        self.Listing.Remove(Index);
        if self.Cursor == Some(Index) {
            self.Cursor = Position.and_then(|Position| self.Order.get(Position.min(self.Order.len().saturating_sub(1))).copied());
        }
        if self.Anchor == Some(Index) {
            self.Anchor = self.Cursor;
        }
    }

    pub fn IsSelected(&self, Index: u32) -> bool {
        self.Listing.Entries.get(Index as usize).is_some_and(|Item| Item.Flags & FlagSelected != 0)
    }

    pub fn SetSelected(&mut self, Index: u32, Selected: bool) {
        let Item = &mut self.Listing.Entries[Index as usize];
        let Was = Item.Flags & FlagSelected != 0;
        if Was == Selected {
            return;
        }
        if Selected {
            Item.Flags |= FlagSelected;
            self.SelectedCount += 1;
        } else {
            Item.Flags &= !FlagSelected;
            self.SelectedCount -= 1;
        }
    }

    pub fn ClearSelection(&mut self) {
        if self.SelectedCount == 0 {
            return;
        }
        for Item in &mut self.Listing.Entries {
            Item.Flags &= !FlagSelected;
        }
        self.SelectedCount = 0;
    }

    pub fn SelectAll(&mut self) {
        for Position in 0..self.Order.len() {
            let Index = self.Order[Position];
            self.SetSelected(Index, true);
        }
    }

    pub fn InvertSelection(&mut self) {
        for Position in 0..self.Order.len() {
            let Index = self.Order[Position];
            let Selected = self.IsSelected(Index);
            self.SetSelected(Index, !Selected);
        }
    }

    pub fn SelectRange(&mut self, From: usize, To: usize) {
        let (Start, End) = (From.min(To), From.max(To));
        for Position in Start..=End.min(self.Order.len().saturating_sub(1)) {
            let Index = self.Order[Position];
            self.SetSelected(Index, true);
        }
    }

    pub fn PositionOf(&self, Index: u32) -> Option<usize> {
        self.Order.iter().position(|&Existing| Existing == Index)
    }

    pub fn CursorPosition(&self) -> Option<usize> {
        self.Cursor.and_then(|Cursor| self.PositionOf(Cursor))
    }

    pub fn SelectedIndices(&self) -> Vec<u32> {
        if self.SelectedCount == 0 {
            return Vec::new();
        }
        self.Order.iter().copied().filter(|&Index| self.IsSelected(Index)).collect()
    }

    pub fn Targets(&self) -> Vec<u32> {
        let Selected = self.SelectedIndices();
        if !Selected.is_empty() {
            return Selected;
        }
        self.Cursor.filter(|Cursor| self.Order.contains(Cursor)).into_iter().collect()
    }

    pub fn TargetPaths(&self) -> Vec<PathBuf> {
        self.Targets().into_iter().map(|Index| self.Listing.FullPath(Index)).collect()
    }

    pub fn SelectedBytes(&self) -> u64 {
        if self.SelectedCount == 0 {
            return 0;
        }
        self.Order.iter().filter(|&&Index| self.IsSelected(Index)).map(|&Index| self.Listing.Entries[Index as usize].Size).sum()
    }

    pub fn CanGoBack(&self) -> bool {
        !self.Back.is_empty()
    }

    pub fn CanGoForward(&self) -> bool {
        !self.Forward.is_empty()
    }
}

pub struct Pane {
    pub Tabs: Vec<Tab>,
    pub Active: usize,
    pub Address: LineEdit,
    pub Completions: Vec<String>,
    pub CompletionIndex: Option<usize>,
    pub CompletionToken: u64,
    pub Closed: Vec<Location>,
    pub FreeSpace: Option<(u64, u64)>,
}

impl Pane {
    pub fn New(Initial: Tab) -> Pane {
        Pane {
            Tabs: vec![Initial],
            Active: 0,
            Address: LineEdit::New(),
            Completions: Vec::new(),
            CompletionIndex: None,
            CompletionToken: 0,
            Closed: Vec::new(),
            FreeSpace: None,
        }
    }

    pub fn Current(&self) -> &Tab {
        &self.Tabs[self.Active.min(self.Tabs.len() - 1)]
    }

    pub fn CurrentMut(&mut self) -> &mut Tab {
        let Index = self.Active.min(self.Tabs.len() - 1);
        &mut self.Tabs[Index]
    }

    pub fn FindTab(&self, Token: u64) -> Option<usize> {
        self.Tabs.iter().position(|Item| Item.Token == Token)
    }
}
