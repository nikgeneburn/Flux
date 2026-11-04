use super::Renderer;
use crate::Application::Event;
use crate::FileSystem::Sorting::Fold;
use crate::FileSystem::{AttributeDirectory, AttributeReadOnly, AttributeSystem, Listing};
use crate::Platform::Imaging::{self, Pixels};
use crate::Platform::Shell;
use crate::Services::Workers::{EventSink, Spawn};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};

pub enum MediaJob {
    Icon { Item: PathBuf, Attributes: u32 },
    Thumbnail { Item: PathBuf, Size: u32, ThumbnailOnly: bool },
}

pub enum MediaResult {
    Icon { Item: PathBuf, Index: i32 },
    Thumbnail { Item: PathBuf, Size: u32, Image: Option<Pixels> },
}

struct Queue {
    Jobs: Mutex<(Vec<MediaJob>, bool)>,
    Signal: Condvar,
}

struct Thumb {
    Image: Option<ID2D1Bitmap>,
    Width: u32,
    Height: u32,
    Bytes: usize,
    Used: u64,
}

pub struct Media {
    Shared: Arc<Queue>,
    ExtensionIcons: HashMap<(Vec<u16>, bool), i32>,
    PathIcons: HashMap<PathBuf, i32>,
    PendingIcons: HashSet<PathBuf>,
    WantedIcons: HashSet<PathBuf>,
    WantedThumbs: HashSet<(PathBuf, u32)>,
    Bitmaps: HashMap<(i32, u32), Option<ID2D1Bitmap>>,
    Thumbs: HashMap<(PathBuf, u32), Thumb>,
    PendingThumbs: HashSet<(PathBuf, u32)>,
    UsedBytes: usize,
    Budget: usize,
    Clock: u64,
    Generation: u64,
}

const UniqueIconExtensions: [&str; 11] = ["exe", "ico", "lnk", "url", "cur", "ani", "msc", "cpl", "scr", "appref-ms", "website"];
const QueueLimit: usize = 256;

fn Worker(Shared: Arc<Queue>, Sink: EventSink) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    loop {
        let Job = {
            let mut Guard = Shared.Jobs.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
            loop {
                if Guard.1 {
                    unsafe { CoUninitialize() };
                    return;
                }
                if let Some(Job) = Guard.0.pop() {
                    break Job;
                }
                Guard = Shared.Signal.wait(Guard).unwrap_or_else(|Poisoned| Poisoned.into_inner());
            }
        };
        let Result = match Job {
            MediaJob::Icon { Item, Attributes } => {
                let Index = Shell::IconIndex(&Item, Attributes, false);
                MediaResult::Icon { Item, Index }
            }
            MediaJob::Thumbnail { Item, Size, ThumbnailOnly } => {
                let Image = Imaging::Thumbnail(&Item, Size, ThumbnailOnly);
                MediaResult::Thumbnail { Item, Size, Image }
            }
        };
        Sink.Send(Event::Media(Result));
    }
}

pub fn Bucket(PixelSize: u32) -> u32 {
    [32u32, 48, 64, 96, 128, 192, 256, 384, 512, 768].into_iter().find(|&Size| Size >= PixelSize).unwrap_or(1024)
}

impl Media {
    pub fn New(Sink: EventSink, BudgetMegabytes: u32) -> Media {
        let Shared = Arc::new(Queue { Jobs: Mutex::new((Vec::new(), false)), Signal: Condvar::new() });
        let WorkerShared = Shared.clone();
        let _ = Spawn("Media", move || Worker(WorkerShared, Sink));
        Media {
            Shared,
            ExtensionIcons: HashMap::new(),
            PathIcons: HashMap::new(),
            PendingIcons: HashSet::new(),
            WantedIcons: HashSet::new(),
            WantedThumbs: HashSet::new(),
            Bitmaps: HashMap::new(),
            Thumbs: HashMap::new(),
            PendingThumbs: HashSet::new(),
            UsedBytes: 0,
            Budget: BudgetMegabytes as usize * 1024 * 1024,
            Clock: 0,
            Generation: 0,
        }
    }

    pub fn SetBudget(&mut self, BudgetMegabytes: u32) {
        self.Budget = BudgetMegabytes as usize * 1024 * 1024;
        self.Trim();
    }

    fn Submit(&mut self, Job: MediaJob) {
        let mut Guard = self.Shared.Jobs.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
        Guard.0.push(Job);
        let Dropped: Vec<MediaJob> = if Guard.0.len() > QueueLimit {
            let Excess = Guard.0.len() - QueueLimit;
            Guard.0.drain(..Excess).collect()
        } else {
            Vec::new()
        };
        drop(Guard);
        self.Shared.Signal.notify_one();
        for Job in Dropped {
            match Job {
                MediaJob::Icon { Item, .. } => {
                    self.PendingIcons.remove(&Item);
                }
                MediaJob::Thumbnail { Item, Size, .. } => {
                    self.PendingThumbs.remove(&(Item, Size));
                }
            }
        }
    }

    pub fn CancelPending(&mut self) {
        let mut Guard = self.Shared.Jobs.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
        Guard.0.clear();
        drop(Guard);
        self.PendingIcons.clear();
        self.PendingThumbs.clear();
    }

    pub fn BeginFrame(&mut self, Draw: &Renderer) {
        if Draw.Generation != self.Generation {
            self.Generation = Draw.Generation;
            self.Bitmaps.clear();
            for Thumb in self.Thumbs.values_mut() {
                Thumb.Image = None;
            }
            self.Thumbs.clear();
            self.UsedBytes = 0;
        }
        self.Clock += 1;
    }

    pub fn EndFrame(&mut self) {
        let mut Guard = self.Shared.Jobs.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner());
        let mut Stale = Vec::new();
        Guard.0.retain(|Job| {
            let Keep = match Job {
                MediaJob::Icon { Item, .. } => self.WantedIcons.contains(Item),
                MediaJob::Thumbnail { Item, Size, .. } => self.WantedThumbs.contains(&(Item.clone(), *Size)),
            };
            if !Keep {
                Stale.push(match Job {
                    MediaJob::Icon { Item, .. } => (Item.clone(), None),
                    MediaJob::Thumbnail { Item, Size, .. } => (Item.clone(), Some(*Size)),
                });
            }
            Keep
        });
        drop(Guard);
        for (Item, Size) in Stale {
            match Size {
                None => self.PendingIcons.remove(&Item),
                Some(Size) => self.PendingThumbs.remove(&(Item, Size)),
            };
        }
        self.WantedIcons.clear();
        self.WantedThumbs.clear();
    }

    fn NeedsPerFileIcon(Data: &Listing, Index: u32) -> bool {
        let Item = &Data.Entries[Index as usize];
        if Data.Root.is_none() && Data.Drives.len() == Data.Entries.len() && !Data.Drives.is_empty() {
            return true;
        }
        if Item.Attributes & AttributeDirectory != 0 {
            return Item.Attributes & (AttributeReadOnly | AttributeSystem) != 0;
        }
        let Extension = Data.Extension(Index);
        UniqueIconExtensions.iter().any(|Candidate| Candidate.len() == Extension.len() && Candidate.bytes().zip(Extension).all(|(A, &B)| A as u16 == Fold(B)))
    }

    pub fn IconIndex(&mut self, Data: &mut Listing, Index: u32) -> i32 {
        let Cached = Data.Entries[Index as usize].Icon;
        if Cached >= 0 {
            return Cached;
        }
        let IsDirectory = Data.IsDirectory(Index);
        if Self::NeedsPerFileIcon(Data, Index) {
            let Item = Data.FullPath(Index);
            if let Some(&Known) = self.PathIcons.get(&Item) {
                Data.Entries[Index as usize].Icon = Known;
                return Known;
            }
            if self.PendingIcons.insert(Item.clone()) {
                self.Submit(MediaJob::Icon { Item: Item.clone(), Attributes: Data.Entries[Index as usize].Attributes });
            }
            self.WantedIcons.insert(Item);
        }
        let Key: Vec<u16> = Data.Extension(Index).iter().map(|&Unit| Fold(Unit)).collect();
        let Resolved = *self.ExtensionIcons.entry((Key, IsDirectory)).or_insert_with_key(|(Extension, Directory)| Shell::ExtensionIconIndex(Extension, *Directory));
        if !Self::NeedsPerFileIcon(Data, Index) {
            Data.Entries[Index as usize].Icon = Resolved;
        }
        Resolved
    }

    pub fn IconBitmap(&mut self, Draw: &Renderer, Index: i32, PixelSize: u32) -> Option<ID2D1Bitmap> {
        if Index < 0 {
            return None;
        }
        let Size = match PixelSize {
            0..=16 => 16,
            17..=32 => 32,
            33..=48 => 48,
            _ => 256,
        };
        self.Bitmaps
            .entry((Index, Size))
            .or_insert_with(|| Imaging::SystemIcon(Index, Size).and_then(|Image| Draw.CreateBitmap(Image.Width, Image.Height, &Image.Data)))
            .clone()
    }

    pub fn Thumbnail(&mut self, Item: &Path, PixelSize: u32, ThumbnailOnly: bool) -> Option<(ID2D1Bitmap, u32, u32)> {
        let Size = Bucket(PixelSize);
        let Key = (Item.to_path_buf(), Size);
        if let Some(Entry) = self.Thumbs.get_mut(&Key) {
            Entry.Used = self.Clock;
            return Entry.Image.clone().map(|Image| (Image, Entry.Width, Entry.Height));
        }
        if self.PendingThumbs.insert(Key.clone()) {
            self.Submit(MediaJob::Thumbnail { Item: Key.0.clone(), Size, ThumbnailOnly });
        }
        self.WantedThumbs.insert(Key);
        None
    }

    pub fn Accept(&mut self, Result: MediaResult, Draw: &Renderer) {
        match Result {
            MediaResult::Icon { Item, Index } => {
                self.PendingIcons.remove(&Item);
                if self.PathIcons.len() > 8192 {
                    self.PathIcons.clear();
                }
                self.PathIcons.insert(Item, Index);
            }
            MediaResult::Thumbnail { Item, Size, Image } => {
                let Key = (Item, Size);
                self.PendingThumbs.remove(&Key);
                let (Bitmap, Width, Height, Bytes) = match Image {
                    Some(Image) => {
                        let Bytes = Image.Data.len();
                        (Draw.CreateBitmap(Image.Width, Image.Height, &Image.Data), Image.Width, Image.Height, Bytes)
                    }
                    None => (None, 0, 0, 0),
                };
                self.UsedBytes += Bytes;
                self.Thumbs.insert(Key, Thumb { Image: Bitmap, Width, Height, Bytes, Used: self.Clock });
                self.Trim();
            }
        }
    }

    fn Trim(&mut self) {
        if self.UsedBytes <= self.Budget && self.Thumbs.len() <= 20000 {
            return;
        }
        let mut Ages: Vec<(u64, (PathBuf, u32))> = self.Thumbs.iter().map(|(Key, Entry)| (Entry.Used, Key.clone())).collect();
        Ages.sort_unstable_by_key(|(Used, _)| *Used);
        for (_, Key) in Ages {
            if self.UsedBytes <= self.Budget * 3 / 4 && self.Thumbs.len() <= 15000 {
                break;
            }
            if let Some(Removed) = self.Thumbs.remove(&Key) {
                self.UsedBytes -= Removed.Bytes;
            }
        }
    }

    pub fn Forget(&mut self, Item: &Path) {
        self.Thumbs.retain(|(Path, _), Entry| {
            if Path == Item {
                false
            } else {
                let _ = Entry;
                true
            }
        });
        self.UsedBytes = self.Thumbs.values().map(|Entry| Entry.Bytes).sum();
        self.PathIcons.remove(Item);
    }

    pub fn CacheBytes(&self) -> usize {
        self.UsedBytes
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Ok(mut Guard) = self.Shared.Jobs.lock() {
            Guard.0.clear();
            Guard.1 = true;
        }
        self.Shared.Signal.notify_all();
    }
}
