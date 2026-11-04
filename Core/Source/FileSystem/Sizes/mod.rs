use super::Enumerate::ForEach;
use super::Sorting::EqualIgnoreCase;
use super::{ApiPath, AttributeDirectory, AttributeReparsePoint, RawEntry};
use crate::Application::Event;
use crate::Services::Workers::{EventSink, Spawn};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetDriveTypeW, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::core::{Owned, PCWSTR};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FolderSize {
    pub Bytes: u64,
    pub Files: u64,
    pub Folders: u64,
    pub Complete: bool,
}

enum Request {
    Watch { Owner: usize, Folders: Vec<PathBuf> },
    Changed(PathBuf),
    Overflow(PathBuf),
    Stop(Sender<()>),
}

pub struct FolderSizes {
    Requests: Sender<Request>,
}

impl FolderSizes {
    pub fn Start(CacheFile: PathBuf, Sink: EventSink) -> Option<FolderSizes> {
        let (Requests, Incoming) = channel();
        let Echo = Requests.clone();
        Spawn("FolderSizes", move || Service::New(CacheFile, Sink, Echo).Run(Incoming)).ok()?;
        Some(FolderSizes { Requests })
    }

    pub fn Watch(&self, Owner: usize, Folders: Vec<PathBuf>) {
        let _ = self.Requests.send(Request::Watch { Owner, Folders });
    }

    pub fn Shutdown(&self) {
        let (Done, Finished) = channel();
        if self.Requests.send(Request::Stop(Done)).is_ok() {
            let _ = Finished.recv_timeout(Duration::from_secs(2));
        }
    }
}

const Missing: u32 = u32::MAX;
const MaximumNodes: usize = 4_000_000;
const BatchSize: usize = 128;
const DirtyLimit: usize = 65_536;
const CacheMagic: &[u8; 8] = b"FLUXSZ01";

struct Node {
    NameStart: u32,
    NameLength: u16,
    Parent: u32,
    FirstChild: u32,
    NextSibling: u32,
    OwnBytes: u64,
    OwnFiles: u64,
    Bytes: u64,
    Files: u64,
    Folders: u64,
    Stamp: i64,
    Hash: u64,
    Unknown: u32,
    Scanned: bool,
    Verified: bool,
    Queued: bool,
    Alive: bool,
}

struct ScanResult {
    Index: u32,
    OwnBytes: u64,
    OwnFiles: u64,
    Hash: u64,
    Subdirectories: Vec<(Vec<u16>, i64)>,
}

struct Service {
    Nodes: Vec<Node>,
    Names: Vec<u16>,
    Free: Vec<u32>,
    Roots: Vec<u32>,
    Queue: VecDeque<u32>,
    Watched: HashMap<usize, Vec<(PathBuf, u32)>>,
    Sent: HashMap<u32, FolderSize>,
    Dirty: HashSet<PathBuf>,
    Volumes: HashSet<String>,
    CacheFile: PathBuf,
    Sink: EventSink,
    Echo: Sender<Request>,
    Modified: bool,
    Lost: bool,
}

fn Mix(Hash: u64, Value: u64) -> u64 {
    (Hash ^ Value).wrapping_mul(0x0000_0100_0000_01B3)
}

fn EntryHash(Raw: &RawEntry) -> u64 {
    let mut Hash = 0xCBF2_9CE4_8422_2325u64;
    for &Unit in Raw.Name {
        Hash = Mix(Hash, Unit as u64);
    }
    Mix(Mix(Hash, Raw.Size), Raw.Modified as u64)
}

fn Scan(Index: u32, Directory: &Path, Buffer: &mut Vec<u8>) -> ScanResult {
    let mut Result = ScanResult { Index, OwnBytes: 0, OwnFiles: 0, Hash: 0, Subdirectories: Vec::new() };
    let _ = ForEach(Directory, Buffer, &mut |Raw| {
        if Raw.Attributes & AttributeDirectory != 0 {
            if Raw.Attributes & AttributeReparsePoint == 0 {
                Result.Subdirectories.push((Raw.Name.to_vec(), Raw.Modified));
            }
        } else {
            Result.OwnBytes += Raw.Size;
            Result.OwnFiles += 1;
            Result.Hash = Result.Hash.wrapping_add(EntryHash(Raw));
        }
        true
    });
    Result
}

fn RootKey(Item: &Path) -> Option<(String, Vec<OsString>)> {
    let mut Root = String::new();
    let mut Parts = Vec::new();
    for Part in Item.components() {
        match Part {
            Component::Prefix(Prefix) => Root.push_str(&Prefix.as_os_str().to_string_lossy()),
            Component::RootDir => Root.push('\\'),
            Component::Normal(Name) => Parts.push(Name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir => return None,
        }
    }
    if !Root.ends_with('\\') {
        return None;
    }
    Some((Root.to_uppercase(), Parts))
}

impl Service {
    fn New(CacheFile: PathBuf, Sink: EventSink, Echo: Sender<Request>) -> Service {
        let mut Output = Service {
            Nodes: Vec::new(),
            Names: Vec::new(),
            Free: Vec::new(),
            Roots: Vec::new(),
            Queue: VecDeque::new(),
            Watched: HashMap::new(),
            Sent: HashMap::new(),
            Dirty: HashSet::new(),
            Volumes: HashSet::new(),
            CacheFile,
            Sink,
            Echo,
            Modified: false,
            Lost: false,
        };
        Output.Load();
        Output
    }

    fn Run(mut self, Incoming: Receiver<Request>) {
        let mut LastFlush = Instant::now();
        let mut LastSave = Instant::now();
        let mut LastReport = Instant::now();
        loop {
            let Wait = if self.Queue.is_empty() { Duration::from_millis(400) } else { Duration::ZERO };
            match Incoming.recv_timeout(Wait) {
                Ok(Request::Stop(Done)) => {
                    self.Save();
                    let _ = Done.send(());
                    return;
                }
                Ok(Item) => self.Handle(Item),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.Save();
                    return;
                }
            }
            while let Ok(Item) = Incoming.try_recv() {
                if let Request::Stop(Done) = Item {
                    self.Save();
                    let _ = Done.send(());
                    return;
                }
                self.Handle(Item);
            }
            if (!self.Dirty.is_empty() || self.Lost) && LastFlush.elapsed() >= Duration::from_millis(400) {
                self.FlushDirty();
                LastFlush = Instant::now();
            }
            if !self.Queue.is_empty() {
                self.ProcessBatch();
            }
            if self.Queue.is_empty() || LastReport.elapsed() >= Duration::from_millis(200) {
                self.Report();
                LastReport = Instant::now();
            }
            if self.Modified && self.Queue.is_empty() && LastSave.elapsed() >= Duration::from_secs(120) {
                self.Save();
                LastSave = Instant::now();
            }
        }
    }

    fn Handle(&mut self, Item: Request) {
        match Item {
            Request::Watch { Owner, Folders } => self.Watch(Owner, Folders),
            Request::Changed(Item) => {
                if self.Dirty.len() < DirtyLimit {
                    self.Dirty.insert(Item);
                } else {
                    self.Lost = true;
                }
            }
            Request::Overflow(Root) => {
                if let Some(Index) = self.Resolve(&Root) {
                    self.QueueSubtree(Index, true);
                }
            }
            Request::Stop(_) => {}
        }
    }

    fn Name(&self, Index: u32) -> &[u16] {
        let Item = &self.Nodes[Index as usize];
        &self.Names[Item.NameStart as usize..Item.NameStart as usize + Item.NameLength as usize]
    }

    fn FullPath(&self, Index: u32) -> PathBuf {
        let mut Chain = Vec::new();
        let mut Current = Index;
        while Current != Missing {
            Chain.push(Current);
            Current = self.Nodes[Current as usize].Parent;
        }
        let mut Wide: Vec<u16> = Vec::new();
        for (Position, &Item) in Chain.iter().rev().enumerate() {
            if Position > 1 {
                Wide.push(b'\\' as u16);
            }
            Wide.extend_from_slice(self.Name(Item));
        }
        PathBuf::from(OsString::from_wide(&Wide))
    }

    fn Allocate(&mut self, Name: &[u16], Parent: u32) -> Option<u32> {
        if self.Nodes.len() - self.Free.len() >= MaximumNodes {
            return None;
        }
        let NameStart = self.Names.len() as u32;
        self.Names.extend_from_slice(Name);
        let Fresh = Node {
            NameStart,
            NameLength: Name.len().min(u16::MAX as usize) as u16,
            Parent,
            FirstChild: Missing,
            NextSibling: if Parent == Missing { Missing } else { self.Nodes[Parent as usize].FirstChild },
            OwnBytes: 0,
            OwnFiles: 0,
            Bytes: 0,
            Files: 0,
            Folders: 0,
            Stamp: 0,
            Hash: 0,
            Unknown: 1,
            Scanned: false,
            Verified: false,
            Queued: false,
            Alive: true,
        };
        let Index = match self.Free.pop() {
            Some(Slot) => {
                self.Nodes[Slot as usize] = Fresh;
                Slot
            }
            None => {
                self.Nodes.push(Fresh);
                (self.Nodes.len() - 1) as u32
            }
        };
        if Parent == Missing {
            self.Roots.push(Index);
        } else {
            self.Nodes[Parent as usize].FirstChild = Index;
            self.AddUp(Parent, 0, 0, 1, 1);
        }
        self.Modified = true;
        Some(Index)
    }

    fn AddUp(&mut self, From: u32, Bytes: i64, Files: i64, Folders: i64, Unknown: i64) {
        let mut Current = From;
        while Current != Missing {
            let Item = &mut self.Nodes[Current as usize];
            Item.Bytes = Item.Bytes.saturating_add_signed(Bytes);
            Item.Files = Item.Files.saturating_add_signed(Files);
            Item.Folders = Item.Folders.saturating_add_signed(Folders);
            Item.Unknown = (Item.Unknown as i64 + Unknown).max(0) as u32;
            Current = Item.Parent;
        }
    }

    fn Child(&self, Parent: u32, Name: &[u16]) -> Option<u32> {
        let mut Current = self.Nodes[Parent as usize].FirstChild;
        while Current != Missing {
            if EqualIgnoreCase(self.Name(Current), Name) {
                return Some(Current);
            }
            Current = self.Nodes[Current as usize].NextSibling;
        }
        None
    }

    fn Children(&self, Parent: u32) -> Vec<u32> {
        let mut Output = Vec::new();
        let mut Current = self.Nodes[Parent as usize].FirstChild;
        while Current != Missing {
            Output.push(Current);
            Current = self.Nodes[Current as usize].NextSibling;
        }
        Output
    }

    fn Remove(&mut self, Index: u32) {
        let (Parent, Bytes, Files, Folders, Unknown) = {
            let Item = &self.Nodes[Index as usize];
            (Item.Parent, Item.Bytes, Item.Files, Item.Folders, Item.Unknown)
        };
        if Parent != Missing {
            let Next = self.Nodes[Index as usize].NextSibling;
            if self.Nodes[Parent as usize].FirstChild == Index {
                self.Nodes[Parent as usize].FirstChild = Next;
            } else {
                let mut Current = self.Nodes[Parent as usize].FirstChild;
                while Current != Missing {
                    if self.Nodes[Current as usize].NextSibling == Index {
                        self.Nodes[Current as usize].NextSibling = Next;
                        break;
                    }
                    Current = self.Nodes[Current as usize].NextSibling;
                }
            }
            self.AddUp(Parent, -(Bytes as i64), -(Files as i64), -(Folders as i64) - 1, -(Unknown as i64));
        }
        let mut Stack = vec![Index];
        while let Some(Current) = Stack.pop() {
            Stack.extend(self.Children(Current));
            self.Nodes[Current as usize].Alive = false;
            self.Nodes[Current as usize].FirstChild = Missing;
            self.Sent.remove(&Current);
            self.Free.push(Current);
        }
        self.Modified = true;
    }

    fn Resolve(&self, Item: &Path) -> Option<u32> {
        let (Root, Parts) = RootKey(Item)?;
        let RootWide: Vec<u16> = Root.encode_utf16().collect();
        let mut Current = *self.Roots.iter().find(|&&Index| self.Nodes[Index as usize].Alive && EqualIgnoreCase(self.Name(Index), &RootWide))?;
        for Part in Parts {
            let Wide: Vec<u16> = Part.encode_wide().collect();
            Current = self.Child(Current, &Wide)?;
        }
        Some(Current)
    }

    fn Ensure(&mut self, Item: &Path) -> Option<u32> {
        let (Root, Parts) = RootKey(Item)?;
        let RootWide: Vec<u16> = Root.encode_utf16().collect();
        let mut Current = match self.Roots.iter().copied().find(|&Index| self.Nodes[Index as usize].Alive && EqualIgnoreCase(self.Name(Index), &RootWide)) {
            Some(Index) => Index,
            None => self.Allocate(&RootWide, Missing)?,
        };
        self.WatchVolume(&Root);
        for Part in Parts {
            let Wide: Vec<u16> = Part.encode_wide().collect();
            Current = match self.Child(Current, &Wide) {
                Some(Index) => Index,
                None => self.Allocate(&Wide, Current)?,
            };
        }
        Some(Current)
    }

    fn Enqueue(&mut self, Index: u32, Front: bool) {
        let Item = &mut self.Nodes[Index as usize];
        if Item.Queued || !Item.Alive {
            return;
        }
        Item.Queued = true;
        if Front { self.Queue.push_front(Index) } else { self.Queue.push_back(Index) }
    }

    fn QueueSubtree(&mut self, Index: u32, Force: bool) {
        let mut Stack = vec![Index];
        while let Some(Current) = Stack.pop() {
            let Item = &self.Nodes[Current as usize];
            if Force || !Item.Verified {
                self.Enqueue(Current, false);
            }
            Stack.extend(self.Children(Current));
        }
    }

    fn Watch(&mut self, Owner: usize, Folders: Vec<PathBuf>) {
        let mut Entries = Vec::with_capacity(Folders.len());
        for Folder in Folders {
            let Some(Index) = self.Ensure(&Folder) else { continue };
            self.Sent.remove(&Index);
            if !self.Nodes[Index as usize].Scanned {
                self.Enqueue(Index, true);
            } else {
                self.QueueSubtree(Index, false);
            }
            Entries.push((Folder, Index));
        }
        self.Watched.insert(Owner, Entries);
    }

    fn FlushDirty(&mut self) {
        if std::mem::take(&mut self.Lost) {
            let Watched: Vec<u32> = self.Watched.values().flatten().map(|(_, Index)| *Index).collect();
            for Index in Watched {
                if self.Nodes[Index as usize].Alive {
                    self.QueueSubtree(Index, true);
                }
            }
        }
        let Items: Vec<PathBuf> = self.Dirty.drain().collect();
        for Item in Items {
            let Some(Parent) = Item.parent() else { continue };
            if let Some(Index) = self.Resolve(Parent).filter(|&Index| self.Nodes[Index as usize].Scanned) {
                self.Enqueue(Index, true);
            }
        }
    }

    fn ProcessBatch(&mut self) {
        let mut Work: Vec<(u32, PathBuf)> = Vec::with_capacity(BatchSize);
        while Work.len() < BatchSize {
            let Some(Index) = self.Queue.pop_front() else { break };
            if !self.Nodes[Index as usize].Alive {
                continue;
            }
            Work.push((Index, self.FullPath(Index)));
        }
        if Work.is_empty() {
            return;
        }
        let Threads = std::thread::available_parallelism().map(|Count| Count.get()).unwrap_or(2).clamp(1, 4).min(Work.len());
        let Chunk = Work.len().div_ceil(Threads);
        let Results: Vec<ScanResult> = std::thread::scope(|Scope| {
            let Handles: Vec<_> = Work
                .chunks(Chunk)
                .map(|Slice| {
                    Scope.spawn(move || {
                        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
                        let mut Buffer = Vec::new();
                        Slice.iter().map(|(Index, Directory)| Scan(*Index, Directory, &mut Buffer)).collect::<Vec<_>>()
                    })
                })
                .collect();
            Handles.into_iter().flat_map(|Handle| Handle.join().unwrap_or_default()).collect()
        });
        for Result in Results {
            self.Apply(Result);
        }
    }

    fn Apply(&mut self, Result: ScanResult) {
        let Index = Result.Index;
        if !self.Nodes[Index as usize].Alive {
            return;
        }
        let (OldBytes, OldFiles, OldHash, WasScanned) = {
            let Item = &self.Nodes[Index as usize];
            (Item.OwnBytes, Item.OwnFiles, Item.Hash, Item.Scanned)
        };
        {
            let Item = &mut self.Nodes[Index as usize];
            Item.Queued = false;
            Item.Verified = true;
            Item.OwnBytes = Result.OwnBytes;
            Item.OwnFiles = Result.OwnFiles;
            Item.Hash = Result.Hash;
        }
        if OldHash != Result.Hash || OldBytes != Result.OwnBytes || OldFiles != Result.OwnFiles || !WasScanned {
            self.AddUp(Index, Result.OwnBytes as i64 - OldBytes as i64, Result.OwnFiles as i64 - OldFiles as i64, 0, 0);
            self.Modified = true;
        }
        let Existing: HashMap<Vec<u16>, u32> = self.Children(Index).into_iter().map(|Child| (Folded(self.Name(Child)), Child)).collect();
        let mut Seen: HashSet<u32> = HashSet::with_capacity(Result.Subdirectories.len());
        for (Name, Stamp) in Result.Subdirectories {
            match Existing.get(&Folded(&Name)).copied() {
                Some(Child) => {
                    Seen.insert(Child);
                    let Item = &mut self.Nodes[Child as usize];
                    if Item.Stamp != Stamp {
                        Item.Stamp = Stamp;
                        if Item.Scanned {
                            self.Enqueue(Child, false);
                        }
                    }
                }
                None => {
                    if let Some(Child) = self.Allocate(&Name, Index) {
                        self.Nodes[Child as usize].Stamp = Stamp;
                        Seen.insert(Child);
                        self.Enqueue(Child, true);
                    }
                }
            }
        }
        for Child in self.Children(Index) {
            if !Seen.contains(&Child) {
                self.Remove(Child);
            }
        }
        if !WasScanned {
            self.Nodes[Index as usize].Scanned = true;
            self.AddUp(Index, 0, 0, 0, -1);
        }
    }

    fn Report(&mut self) {
        let mut Changes = Vec::new();
        for Entries in self.Watched.values() {
            for (Folder, Index) in Entries {
                let Item = &self.Nodes[*Index as usize];
                if !Item.Alive || (!Item.Scanned && Item.Bytes == 0) {
                    continue;
                }
                let Value = FolderSize { Bytes: Item.Bytes, Files: Item.Files, Folders: Item.Folders, Complete: Item.Scanned && Item.Unknown == 0 };
                if self.Sent.get(Index) != Some(&Value) {
                    self.Sent.insert(*Index, Value);
                    Changes.push((Folder.clone(), Value));
                }
            }
        }
        if !Changes.is_empty() {
            self.Sink.Send(Event::FolderSizes(Changes));
        }
    }

    fn WatchVolume(&mut self, Root: &str) {
        if !self.Volumes.insert(Root.to_string()) {
            return;
        }
        let Wide: Vec<u16> = Root.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe { GetDriveTypeW(PCWSTR(Wide.as_ptr())) } != DRIVE_FIXED {
            return;
        }
        let Directory = PathBuf::from(Root);
        let Echo = self.Echo.clone();
        let _ = Spawn("FolderSizeWatch", move || WatchLoop(Directory, Echo));
    }

    fn Save(&mut self) {
        if !self.Modified {
            return;
        }
        let mut Bytes: Vec<u8> = Vec::with_capacity(self.Nodes.len() * 48);
        Bytes.extend_from_slice(CacheMagic);
        let mut Order: Vec<u32> = Vec::new();
        let mut Remap: HashMap<u32, u32> = HashMap::new();
        let mut Pending: VecDeque<u32> = self.Roots.iter().copied().filter(|&Index| self.Nodes[Index as usize].Alive).collect();
        while let Some(Index) = Pending.pop_front() {
            if !self.Nodes[Index as usize].Scanned {
                continue;
            }
            Remap.insert(Index, Order.len() as u32);
            Order.push(Index);
            Pending.extend(self.Children(Index));
        }
        Bytes.extend_from_slice(&(Order.len() as u32).to_le_bytes());
        for &Index in &Order {
            let Item = &self.Nodes[Index as usize];
            let Parent = if Item.Parent == Missing { Missing } else { Remap.get(&Item.Parent).copied().unwrap_or(Missing) };
            let Name = self.Name(Index);
            Bytes.extend_from_slice(&Parent.to_le_bytes());
            Bytes.extend_from_slice(&(Name.len() as u16).to_le_bytes());
            for Unit in Name {
                Bytes.extend_from_slice(&Unit.to_le_bytes());
            }
            Bytes.extend_from_slice(&Item.OwnBytes.to_le_bytes());
            Bytes.extend_from_slice(&Item.OwnFiles.to_le_bytes());
            Bytes.extend_from_slice(&Item.Stamp.to_le_bytes());
            Bytes.extend_from_slice(&Item.Hash.to_le_bytes());
        }
        if let Some(Folder) = self.CacheFile.parent() {
            let _ = std::fs::create_dir_all(Folder);
        }
        let Temporary = self.CacheFile.with_extension("tmp");
        let Written = std::fs::File::create(&Temporary).and_then(|mut File| File.write_all(&Bytes).and_then(|_| File.sync_all()));
        if Written.is_ok() && std::fs::rename(&Temporary, &self.CacheFile).is_ok() {
            self.Modified = false;
        }
    }

    fn Load(&mut self) {
        let mut Data = Vec::new();
        let Opened = std::fs::File::open(&self.CacheFile).and_then(|mut File| File.read_to_end(&mut Data));
        if Opened.is_err() || Data.len() < 12 || &Data[..8] != CacheMagic {
            return;
        }
        let mut Reader = CacheReader { Data: &Data, Offset: 8 };
        let Some(Count) = Reader.U32() else { return };
        let mut Loaded: Vec<u32> = Vec::with_capacity(Count as usize);
        for _ in 0..Count {
            let Some(Record) = Reader.Record() else { break };
            let Parent = if Record.Parent == Missing { Missing } else { Loaded.get(Record.Parent as usize).copied().unwrap_or(Missing) };
            if Record.Parent != Missing && Parent == Missing {
                continue;
            }
            let Some(Index) = self.Allocate(&Record.Name, Parent) else { break };
            let Item = &mut self.Nodes[Index as usize];
            Item.OwnBytes = Record.OwnBytes;
            Item.OwnFiles = Record.OwnFiles;
            Item.Stamp = Record.Stamp;
            Item.Hash = Record.Hash;
            Item.Scanned = true;
            Loaded.push(Index);
        }
        for &Index in &Loaded {
            let Item = &mut self.Nodes[Index as usize];
            Item.Bytes = 0;
            Item.Files = 0;
            Item.Folders = 0;
            Item.Unknown = 0;
        }
        for &Index in Loaded.iter().rev() {
            let Item = &mut self.Nodes[Index as usize];
            Item.Bytes += Item.OwnBytes;
            Item.Files += Item.OwnFiles;
            let (Bytes, Files, Folders, Unknown, Parent) = (Item.Bytes, Item.Files, Item.Folders, Item.Unknown, Item.Parent);
            if Parent != Missing {
                let Above = &mut self.Nodes[Parent as usize];
                Above.Bytes += Bytes;
                Above.Files += Files;
                Above.Folders += Folders + 1;
                Above.Unknown += Unknown;
            }
        }
        self.Modified = false;
    }
}

struct CacheRecord {
    Parent: u32,
    Name: Vec<u16>,
    OwnBytes: u64,
    OwnFiles: u64,
    Stamp: i64,
    Hash: u64,
}

struct CacheReader<'a> {
    Data: &'a [u8],
    Offset: usize,
}

impl CacheReader<'_> {
    fn Take<const Count: usize>(&mut self) -> Option<[u8; Count]> {
        let Slice = self.Data.get(self.Offset..self.Offset + Count)?;
        self.Offset += Count;
        Slice.try_into().ok()
    }

    fn U32(&mut self) -> Option<u32> {
        self.Take::<4>().map(u32::from_le_bytes)
    }

    fn U64(&mut self) -> Option<u64> {
        self.Take::<8>().map(u64::from_le_bytes)
    }

    fn Record(&mut self) -> Option<CacheRecord> {
        let Parent = self.U32()?;
        let Length = u16::from_le_bytes(self.Take::<2>()?) as usize;
        let mut Name = Vec::with_capacity(Length);
        for _ in 0..Length {
            Name.push(u16::from_le_bytes(self.Take::<2>()?));
        }
        Some(CacheRecord { Parent, Name, OwnBytes: self.U64()?, OwnFiles: self.U64()?, Stamp: self.U64()? as i64, Hash: self.U64()? })
    }
}

fn WatchLoop(Root: PathBuf, Echo: Sender<Request>) {
    let Wide = ApiPath(&Root);
    let Opened = unsafe {
        CreateFileW(
            PCWSTR(Wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let Ok(Handle) = Opened else { return };
    let Handle = unsafe { Owned::new(Handle) };
    let mut Buffer = vec![0u32; 16 * 1024];
    let Filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
    loop {
        let mut Returned = 0u32;
        let Result = unsafe { ReadDirectoryChangesW(*Handle, Buffer.as_mut_ptr() as *mut _, (Buffer.len() * 4) as u32, true, Filter, Some(&mut Returned), None, None) };
        if Result.is_err() {
            return;
        }
        if Returned == 0 {
            if Echo.send(Request::Overflow(Root.clone())).is_err() {
                return;
            }
            continue;
        }
        let Bytes = Buffer.as_ptr() as *const u8;
        let mut Offset = 0usize;
        loop {
            let Record = unsafe { &*(Bytes.add(Offset) as *const FILE_NOTIFY_INFORMATION) };
            let Name = unsafe { std::slice::from_raw_parts(Record.FileName.as_ptr(), Record.FileNameLength as usize / 2) };
            if Echo.send(Request::Changed(Root.join(OsString::from_wide(Name)))).is_err() {
                return;
            }
            if Record.NextEntryOffset == 0 {
                break;
            }
            Offset += Record.NextEntryOffset as usize;
        }
    }
}

fn Folded(Name: &[u16]) -> Vec<u16> {
    Name.iter().map(|&Unit| super::Sorting::Fold(Unit)).collect()
}
