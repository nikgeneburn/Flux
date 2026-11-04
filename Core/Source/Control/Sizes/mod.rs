use crate::Application::App;
use crate::FileSystem::Sizes::{FolderSize, FolderSizes};
use crate::FileSystem::Sorting::SortKey;
use crate::FileSystem::{FlagSizeKnown, FlagSizePartial, FlagRemoved, Location};
use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

const KnownLimit: usize = 250_000;

impl App {
    pub fn StartFolderSizes(&mut self) {
        if self.Settings.FolderSizes && self.FolderSizes.is_none() {
            self.FolderSizes = FolderSizes::Start(self.Paths.FolderSizeCache(), self.Sink.clone());
            for Pane in 0..self.Panes.len() {
                self.WatchFolderSizes(Pane);
            }
        }
    }

    pub fn StopFolderSizes(&mut self) {
        if let Some(Service) = self.FolderSizes.take() {
            Service.Shutdown();
        }
        self.KnownSizes.clear();
        for Pane in &mut self.Panes {
            for Tab in &mut Pane.Tabs {
                for Item in &mut Tab.Listing.Entries {
                    if Item.Flags & FlagSizeKnown != 0 {
                        Item.Size = 0;
                        Item.Flags &= !(FlagSizeKnown | FlagSizePartial);
                    }
                }
            }
        }
        self.Invalidate();
    }

    pub fn WatchFolderSizes(&mut self, Pane: usize) {
        let Some(Service) = &self.FolderSizes else { return };
        let Tab = self.Tab(Pane);
        let Shown = Pane == 0 || self.Settings.DualPane;
        let Folders: Vec<PathBuf> = match &Tab.Location {
            Location::Directory(Directory) if Shown && !Directory.as_os_str().encode_wide().take(2).eq([b'\\' as u16, b'\\' as u16]) => (0..Tab.Listing.Len() as u32)
                .filter(|&Index| Tab.Listing.Entries[Index as usize].Flags & FlagRemoved == 0 && Tab.Listing.IsDirectory(Index))
                .map(|Index| Tab.Listing.FullPath(Index))
                .collect(),
            _ => Vec::new(),
        };
        Service.Watch(Pane, Folders);
        let Active = self.Panes[Pane].Active;
        self.ApplyKnownSizes(Pane, Active);
    }

    pub fn ApplyKnownSizes(&mut self, Pane: usize, Index: usize) {
        if self.KnownSizes.is_empty() {
            return;
        }
        let Visible = self.Visibility();
        let Known = &self.KnownSizes;
        let Tab = &mut self.Panes[Pane].Tabs[Index];
        if !matches!(Tab.Location, Location::Directory(_)) {
            return;
        }
        let mut Changed = false;
        for Entry in 0..Tab.Listing.Len() as u32 {
            if Tab.Listing.Entries[Entry as usize].Flags & FlagRemoved != 0 || !Tab.Listing.IsDirectory(Entry) {
                continue;
            }
            if let Some(Value) = Known.get(&Tab.Listing.FullPath(Entry)) {
                Changed |= Store(&mut Tab.Listing.Entries[Entry as usize], Value);
            }
        }
        if Changed && Tab.Sort.Key == SortKey::Size {
            Tab.Resort(&Visible);
        }
    }

    pub fn AcceptFolderSizes(&mut self, Results: Vec<(PathBuf, FolderSize)>) {
        if self.FolderSizes.is_none() {
            return;
        }
        if self.KnownSizes.len() + Results.len() > KnownLimit {
            self.KnownSizes.clear();
        }
        let mut ByParent: HashMap<PathBuf, Vec<(Vec<u16>, FolderSize)>> = HashMap::new();
        for (Folder, Value) in Results {
            if let (Some(Parent), Some(Name)) = (Folder.parent(), Folder.file_name()) {
                ByParent.entry(Parent.to_path_buf()).or_default().push((Name.encode_wide().collect(), Value));
            }
            self.KnownSizes.insert(Folder, Value);
        }
        let Visible = self.Visibility();
        for Pane in &mut self.Panes {
            for Tab in &mut Pane.Tabs {
                let Location::Directory(Directory) = &Tab.Location else { continue };
                let Some(Updates) = ByParent.get(Directory) else { continue };
                let Index: HashMap<Vec<u16>, u32> = (0..Tab.Listing.Len() as u32)
                    .filter(|&Entry| Tab.Listing.Entries[Entry as usize].Flags & FlagRemoved == 0 && Tab.Listing.IsDirectory(Entry))
                    .map(|Entry| (Folded(Tab.Listing.Name(Entry)), Entry))
                    .collect();
                let mut Changed = false;
                for (Name, Value) in Updates {
                    if let Some(&Entry) = Index.get(&Folded(Name)) {
                        Changed |= Store(&mut Tab.Listing.Entries[Entry as usize], Value);
                    }
                }
                if Changed && Tab.Sort.Key == SortKey::Size {
                    Tab.Resort(&Visible);
                }
            }
        }
        self.Invalidate();
    }
}

fn Folded(Name: &[u16]) -> Vec<u16> {
    Name.iter().map(|&Unit| crate::FileSystem::Sorting::Fold(Unit)).collect()
}

fn Store(Entry: &mut crate::FileSystem::Entry, Value: &FolderSize) -> bool {
    let Flags = (Entry.Flags & !(FlagSizeKnown | FlagSizePartial)) | FlagSizeKnown | if Value.Complete { 0 } else { FlagSizePartial };
    if Entry.Size == Value.Bytes && Entry.Flags == Flags {
        return false;
    }
    Entry.Size = Value.Bytes;
    Entry.Flags = Flags;
    true
}
