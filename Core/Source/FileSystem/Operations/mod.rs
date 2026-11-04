mod Recycle;

use crate::FileSystem::{
    ApiPath, AttributeDirectory, AttributeHidden, AttributeReadOnly, AttributeReparsePoint, AttributeSystem, ErrorText, IsSameOrInside, LeafName, PathFromWide, SamePath,
    UniqueName,
};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_DIR_NOT_EMPTY, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED, ERROR_REQUEST_ABORTED, FILETIME,
    GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, COPY_FILE_COPY_SYMLINK, COPY_FILE_FAIL_IF_EXISTS, COPYFILE_FLAGS, COPYPROGRESSROUTINE_PROGRESS, CREATE_NEW, CopyFileExW,
    CreateDirectoryExW, CreateDirectoryW, CreateFileW, CreateSymbolicLinkW, DeleteFileW, FILE_ATTRIBUTE_NORMAL, FILE_CREATION_DISPOSITION, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FIND_FIRST_EX_FLAGS,
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetFileAttributesExW, GetFileExInfoStandard,
    GetFileInformationByHandle, LPPROGRESS_ROUTINE_CALLBACK_REASON, MAXIMUM_REPARSE_DATA_BUFFER_SIZE, MOVE_FILE_FLAGS, MOVEFILE_REPLACE_EXISTING, MoveFileExW,
    OPEN_EXISTING, PROGRESS_CANCEL, PROGRESS_CONTINUE, RemoveDirectoryW, SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE, SYMBOLIC_LINK_FLAG_DIRECTORY, SYMBOLIC_LINK_FLAGS,
    SetFileAttributesW, WIN32_FILE_ATTRIBUTE_DATA, WIN32_FIND_DATAW,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT};
use windows::Win32::System::SystemServices::{IO_REPARSE_TAG_MOUNT_POINT, IO_REPARSE_TAG_SYMLINK};
use windows::core::{HRESULT, PCWSTR};

const NameSurrogateBit: u32 = 0x2000_0000;
const SettableAttributes: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x100 | 0x1000 | 0x2000;
const ReportInterval: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub enum Request {
    CopyInto { Sources: Vec<PathBuf>, Destination: PathBuf },
    MoveInto { Sources: Vec<PathBuf>, Destination: PathBuf },
    MovePairs(Vec<(PathBuf, PathBuf)>),
    Recycle(Vec<PathBuf>),
    DeletePermanently(Vec<PathBuf>),
    CreateFolder(PathBuf),
    CreateFile(PathBuf),
}

impl Request {
    pub fn Title(&self) -> String {
        match self {
            Request::CopyInto { Sources, Destination } => format!("Copying {} to {}", Describe(Sources), LeafName(Destination)),
            Request::MoveInto { Sources, Destination } => format!("Moving {} to {}", Describe(Sources), LeafName(Destination)),
            Request::MovePairs(Pairs) => {
                let Renaming = Pairs.iter().all(|(From, To)| SameParent(From, To));
                let Verb = if Renaming { "Renaming" } else { "Moving" };
                match Pairs.as_slice() {
                    [(From, _)] => format!("{Verb} {}", LeafName(From)),
                    _ => format!("{Verb} {} items", Pairs.len()),
                }
            }
            Request::Recycle(Items) => format!("Deleting {}", Describe(Items)),
            Request::DeletePermanently(Items) => format!("Permanently deleting {}", Describe(Items)),
            Request::CreateFolder(Item) => format!("Creating folder {}", LeafName(Item)),
            Request::CreateFile(Item) => format!("Creating file {}", LeafName(Item)),
        }
    }
}

fn Describe(Items: &[PathBuf]) -> String {
    match Items {
        [Single] => LeafName(Single),
        _ => format!("{} items", Items.len()),
    }
}

fn SameParent(Left: &Path, Right: &Path) -> bool {
    match (Left.parent(), Right.parent()) {
        (Some(A), Some(B)) => SamePath(A, B),
        _ => false,
    }
}

fn PairsInto(Sources: &[PathBuf], Destination: &Path) -> Vec<(PathBuf, PathBuf)> {
    Sources.iter().map(|Source| (Source.clone(), Destination.join(LeafName(Source)))).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CollisionChoice {
    Replace,
    Skip,
    KeepBoth,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct CollisionQuery {
    pub Source: PathBuf,
    pub Destination: PathBuf,
    pub SourceSize: u64,
    pub SourceModified: i64,
    pub DestinationSize: u64,
    pub DestinationModified: i64,
    pub SourceIsDirectory: bool,
    pub DestinationIsDirectory: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Scanning,
    Working,
    WaitingForUser,
    Finished,
}

#[derive(Clone, Debug)]
pub struct ProgressSnapshot {
    pub Phase: Phase,
    pub BytesDone: u64,
    pub BytesTotal: u64,
    pub ItemsDone: u64,
    pub ItemsTotal: u64,
    pub Current: String,
}

#[derive(Debug)]
pub struct Summary {
    pub Request: Request,
    pub Cancelled: bool,
    pub Errors: Vec<(PathBuf, String)>,
    pub Created: Vec<PathBuf>,
    pub Moved: Vec<(PathBuf, PathBuf)>,
    pub Replaced: Vec<PathBuf>,
    pub Removed: Vec<PathBuf>,
    pub Elapsed: Duration,
}

pub enum Notification {
    Progress { Identifier: u64 },
    Collision { Identifier: u64, Query: CollisionQuery },
    Finished { Identifier: u64, Summary: Summary },
}

struct Shared {
    Identifier: u64,
    Notify: Arc<dyn Fn(Notification) + Send + Sync>,
    Cancelled: AtomicBool,
    Done: AtomicBool,
    Stage: Mutex<Phase>,
    BytesDone: AtomicU64,
    BytesTotal: AtomicU64,
    ItemsDone: AtomicU64,
    ItemsTotal: AtomicU64,
    Current: Mutex<String>,
    Decision: Mutex<Option<(CollisionChoice, bool)>>,
    Wake: Condvar,
    LastReport: Mutex<Option<Instant>>,
}

fn Lock<T>(Value: &Mutex<T>) -> MutexGuard<'_, T> {
    Value.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner())
}

impl Shared {
    fn IsCancelled(&self) -> bool {
        self.Cancelled.load(Ordering::Acquire)
    }

    fn SetPhase(&self, Value: Phase) {
        *Lock(&self.Stage) = Value;
    }

    fn SetCurrent(&self, Item: &Path) {
        *Lock(&self.Current) = LeafName(Item);
    }

    fn AddItems(&self, Count: u64) {
        self.ItemsDone.fetch_add(Count, Ordering::Relaxed);
    }

    fn AddTotals(&self, Items: u64, Bytes: u64) {
        self.ItemsTotal.fetch_add(Items, Ordering::Relaxed);
        self.BytesTotal.fetch_add(Bytes, Ordering::Relaxed);
    }

    fn Report(&self) {
        let Now = Instant::now();
        {
            let mut Last = Lock(&self.LastReport);
            if Last.is_some_and(|Previous| Now.duration_since(Previous) < ReportInterval) {
                return;
            }
            *Last = Some(Now);
        }
        (self.Notify)(Notification::Progress { Identifier: self.Identifier });
    }
}

pub struct Operation {
    pub Identifier: u64,
    pub Title: String,
    State: Arc<Shared>,
}

impl Operation {
    pub fn Start(Identifier: u64, Request: Request, Owner: isize, Notify: Arc<dyn Fn(Notification) + Send + Sync>) -> std::io::Result<Operation> {
        let Title = Request.Title();
        let State = Arc::new(Shared {
            Identifier,
            Notify,
            Cancelled: AtomicBool::new(false),
            Done: AtomicBool::new(false),
            Stage: Mutex::new(Phase::Scanning),
            BytesDone: AtomicU64::new(0),
            BytesTotal: AtomicU64::new(0),
            ItemsDone: AtomicU64::new(0),
            ItemsTotal: AtomicU64::new(0),
            Current: Mutex::new(String::new()),
            Decision: Mutex::new(None),
            Wake: Condvar::new(),
            LastReport: Mutex::new(None),
        });
        let WorkerState = State.clone();
        crate::Services::Workers::Spawn("File Operation", move || Execute(WorkerState, Request, Owner))?;
        Ok(Operation { Identifier, Title, State })
    }

    pub fn Cancel(&self) {
        self.State.Cancelled.store(true, Ordering::Release);
        let _Guard = Lock(&self.State.Decision);
        self.State.Wake.notify_all();
    }

    pub fn Resolve(&self, Choice: CollisionChoice, ApplyToAll: bool) {
        let mut Guard = Lock(&self.State.Decision);
        *Guard = Some((Choice, ApplyToAll));
        self.State.Wake.notify_all();
    }

    pub fn Snapshot(&self) -> ProgressSnapshot {
        let BytesDone = self.State.BytesDone.load(Ordering::Relaxed);
        let ItemsDone = self.State.ItemsDone.load(Ordering::Relaxed);
        ProgressSnapshot {
            Phase: *Lock(&self.State.Stage),
            BytesDone,
            BytesTotal: self.State.BytesTotal.load(Ordering::Relaxed).max(BytesDone),
            ItemsDone,
            ItemsTotal: self.State.ItemsTotal.load(Ordering::Relaxed).max(ItemsDone),
            Current: Lock(&self.State.Current).clone(),
        }
    }
}

fn Execute(State: Arc<Shared>, Work: Request, Owner: isize) {
    let Started = Instant::now();
    let Empty = Summary {
        Request: Work,
        Cancelled: false,
        Errors: Vec::new(),
        Created: Vec::new(),
        Moved: Vec::new(),
        Replaced: Vec::new(),
        Removed: Vec::new(),
        Elapsed: Duration::ZERO,
    };
    let mut Worker = Engine { State: State.clone(), ApplyAll: None, KeepBothAllowed: true, Result: Empty };
    let Work = Worker.Result.Request.clone();
    if catch_unwind(AssertUnwindSafe(|| Worker.Run(&Work, Owner))).is_err() {
        Worker.Result.Errors.push((PathBuf::new(), "The operation stopped because of an internal error".to_string()));
    }
    State.SetPhase(Phase::Finished);
    State.Done.store(true, Ordering::Release);
    let mut Result = Worker.Result;
    Result.Cancelled = State.IsCancelled();
    Result.Elapsed = Started.elapsed();
    (State.Notify)(Notification::Finished { Identifier: State.Identifier, Summary: Result });
}

#[derive(Clone, Copy, Default)]
struct Info {
    Attributes: u32,
    Size: u64,
    Modified: i64,
    Tag: u32,
}

impl Info {
    fn IsDirectory(&self) -> bool {
        self.Attributes & AttributeDirectory != 0
    }

    fn IsLink(&self) -> bool {
        self.Attributes & AttributeReparsePoint != 0 && self.Tag & NameSurrogateBit != 0
    }

    fn IsPlainDirectory(&self) -> bool {
        self.IsDirectory() && !self.IsLink()
    }

    fn New(Attributes: u32, SizeHigh: u32, SizeLow: u32, Time: FILETIME, Tag: u32) -> Info {
        let Modified = ((Time.dwHighDateTime as u64) << 32 | Time.dwLowDateTime as u64) as i64;
        let Tag = if Attributes & AttributeReparsePoint != 0 { Tag } else { 0 };
        Info { Attributes, Size: (SizeHigh as u64) << 32 | SizeLow as u64, Modified, Tag }
    }

    fn FromFind(Data: &WIN32_FIND_DATAW) -> Info {
        Info::New(Data.dwFileAttributes, Data.nFileSizeHigh, Data.nFileSizeLow, Data.ftLastWriteTime, Data.dwReserved0)
    }
}

struct OwnedHandle(HANDLE, bool);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = if self.1 { FindClose(self.0) } else { CloseHandle(self.0) };
        }
    }
}

fn HresultCode(Value: HRESULT) -> u32 {
    let Raw = Value.0 as u32;
    if Raw & 0xFFFF_0000 == 0x8007_0000 { Raw & 0xFFFF } else { Raw }
}

fn Code(Error: &windows::core::Error) -> u32 {
    HresultCode(Error.code())
}

fn FindFirst(Pattern: &[u16], Data: &mut WIN32_FIND_DATAW, Flags: FIND_FIRST_EX_FLAGS) -> Result<OwnedHandle, u32> {
    let Output = Data as *mut WIN32_FIND_DATAW as *mut c_void;
    unsafe { FindFirstFileExW(PCWSTR(Pattern.as_ptr()), FindExInfoBasic, Output, FindExSearchNameMatch, None, Flags) }
        .map(|Handle| OwnedHandle(Handle, true))
        .map_err(|Error| Code(&Error))
}

fn Inspect(Item: &Path) -> Result<Info, u32> {
    let Wide = ApiPath(Item);
    let mut Find = WIN32_FIND_DATAW::default();
    if FindFirst(&Wide, &mut Find, FIND_FIRST_EX_FLAGS(0)).is_ok() {
        return Ok(Info::FromFind(&Find));
    }
    let mut Data = WIN32_FILE_ATTRIBUTE_DATA::default();
    unsafe { GetFileAttributesExW(PCWSTR(Wide.as_ptr()), GetFileExInfoStandard, &mut Data as *mut WIN32_FILE_ATTRIBUTE_DATA as *mut c_void) }
        .map_err(|Error| Code(&Error))?;
    Ok(Info::New(Data.dwFileAttributes, Data.nFileSizeHigh, Data.nFileSizeLow, Data.ftLastWriteTime, 0))
}

fn Exists(Item: &Path) -> bool {
    Inspect(Item).is_ok()
}

fn Children(Directory: &Path) -> Result<Vec<(PathBuf, Info)>, u32> {
    let mut Data = WIN32_FIND_DATAW::default();
    let Handle = match FindFirst(&ApiPath(&Directory.join("*")), &mut Data, FIND_FIRST_EX_LARGE_FETCH) {
        Ok(Value) => Value,
        Err(Value) if Value == ERROR_FILE_NOT_FOUND.0 => return Ok(Vec::new()),
        Err(Value) => return Err(Value),
    };
    let mut Entries = Vec::new();
    loop {
        let Name = PathFromWide(&Data.cFileName);
        if Name.as_os_str() != "." && Name.as_os_str() != ".." {
            Entries.push((Name, Info::FromFind(&Data)));
        }
        if let Err(Error) = unsafe { FindNextFileW(Handle.0, &mut Data) } {
            let Value = Code(&Error);
            return if Value == ERROR_NO_MORE_FILES.0 { Ok(Entries) } else { Err(Value) };
        }
    }
}

fn OpenHandle(Item: &Path, Access: u32, Disposition: FILE_CREATION_DISPOSITION, Flags: u32) -> Result<OwnedHandle, u32> {
    let Wide = ApiPath(Item);
    let Share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    unsafe { CreateFileW(PCWSTR(Wide.as_ptr()), Access, Share, None, Disposition, FILE_FLAGS_AND_ATTRIBUTES(Flags), None) }
        .map(|Handle| OwnedHandle(Handle, false))
        .map_err(|Error| Code(&Error))
}

fn OpenReparse(Item: &Path, Access: u32) -> Result<OwnedHandle, u32> {
    OpenHandle(Item, Access, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
}

fn VolumeSerial(Item: &Path) -> Option<u32> {
    let Handle = OpenReparse(Item, FILE_READ_ATTRIBUTES.0).ok()?;
    let mut Data = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(Handle.0, &mut Data) }.ok()?;
    Some(Data.dwVolumeSerialNumber)
}

fn SetAttributes(Item: &Path, Attributes: u32) -> Result<(), u32> {
    let Wide = ApiPath(Item);
    let Value = Attributes & SettableAttributes;
    let Applied = if Value == 0 { FILE_ATTRIBUTE_NORMAL } else { FILE_FLAGS_AND_ATTRIBUTES(Value) };
    unsafe { SetFileAttributesW(PCWSTR(Wide.as_ptr()), Applied) }.map_err(|Error| Code(&Error))
}

fn RemoveItem(Item: &Path, Attributes: u32) -> Result<(), u32> {
    let Cleared = Attributes & AttributeReadOnly != 0 && SetAttributes(Item, Attributes & !AttributeReadOnly).is_ok();
    let Wide = ApiPath(Item);
    let Removed = if Attributes & AttributeDirectory != 0 { unsafe { RemoveDirectoryW(PCWSTR(Wide.as_ptr())) } } else { unsafe { DeleteFileW(PCWSTR(Wide.as_ptr())) } };
    Removed.map_err(|Error| {
        if Cleared {
            let _ = SetAttributes(Item, Attributes);
        }
        Code(&Error)
    })
}

fn Rename(Source: &Path, Target: &Path, Replacing: bool) -> Result<(), u32> {
    let SourceWide = ApiPath(Source);
    let TargetWide = ApiPath(Target);
    let Flags = if Replacing { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) };
    unsafe { MoveFileExW(PCWSTR(SourceWide.as_ptr()), PCWSTR(TargetWide.as_ptr()), Flags) }.map_err(|Error| Code(&Error))
}

fn ReadReparse(Item: &Path) -> Result<Vec<u8>, u32> {
    let Handle = OpenReparse(Item, FILE_READ_ATTRIBUTES.0)?;
    let mut Buffer = vec![0u8; MAXIMUM_REPARSE_DATA_BUFFER_SIZE as usize];
    let mut Returned = 0u32;
    let Output = Some(Buffer.as_mut_ptr() as *mut c_void);
    unsafe { DeviceIoControl(Handle.0, FSCTL_GET_REPARSE_POINT, None, 0, Output, Buffer.len() as u32, Some(&mut Returned), None) }.map_err(|Error| Code(&Error))?;
    Buffer.truncate(Returned as usize);
    Ok(Buffer)
}

fn ReadUnit(Data: &[u8], Offset: usize) -> Option<usize> {
    Data.get(Offset..Offset + 2).map(|Bytes| u16::from_le_bytes([Bytes[0], Bytes[1]]) as usize)
}

fn ReparseText(Data: &[u8], Base: usize, Offset: usize, Length: usize) -> Option<Vec<u16>> {
    let Start = Base + Offset;
    let Bytes = Data.get(Start..Start + Length)?;
    Some(Bytes.chunks_exact(2).map(|Pair| u16::from_le_bytes([Pair[0], Pair[1]])).collect())
}

fn SymbolicLinkTarget(Data: &[u8]) -> Option<Vec<u16>> {
    let Base = 20;
    let Print = ReparseText(Data, Base, ReadUnit(Data, 12)?, ReadUnit(Data, 14)?)?;
    let mut Text = if Print.is_empty() {
        let Substitute = String::from_utf16_lossy(&ReparseText(Data, Base, ReadUnit(Data, 8)?, ReadUnit(Data, 10)?)?);
        let Plain = match Substitute.strip_prefix("\\??\\UNC\\") {
            Some(Rest) => format!("\\\\{Rest}"),
            None => Substitute.strip_prefix("\\??\\").unwrap_or(&Substitute).to_string(),
        };
        Plain.encode_utf16().collect()
    } else {
        Print
    };
    Text.push(0);
    Some(Text)
}

fn CopyLink(Source: &Path, Details: &Info, Target: &Path) -> Result<(), u32> {
    let Wide = ApiPath(Target);
    if Details.Tag == IO_REPARSE_TAG_SYMLINK {
        let Text = SymbolicLinkTarget(&ReadReparse(Source)?).ok_or(ERROR_NOT_SUPPORTED.0)?;
        let Directory = if Details.IsDirectory() { SYMBOLIC_LINK_FLAG_DIRECTORY.0 } else { 0 };
        let Flags = SYMBOLIC_LINK_FLAGS(Directory | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE.0);
        let Created = unsafe { CreateSymbolicLinkW(PCWSTR(Wide.as_ptr()), PCWSTR(Text.as_ptr()), Flags) };
        return if Created { Ok(()) } else { Err(unsafe { GetLastError() }.0) };
    }
    if Details.Tag != IO_REPARSE_TAG_MOUNT_POINT {
        return Err(ERROR_NOT_SUPPORTED.0);
    }
    let Data = ReadReparse(Source)?;
    unsafe { CreateDirectoryW(PCWSTR(Wide.as_ptr()), None) }.map_err(|Error| Code(&Error))?;
    let Input = Some(Data.as_ptr() as *const c_void);
    let Applied = OpenReparse(Target, GENERIC_WRITE.0)
        .and_then(|Handle| unsafe { DeviceIoControl(Handle.0, FSCTL_SET_REPARSE_POINT, Input, Data.len() as u32, None, 0, None, None) }.map_err(|Error| Code(&Error)));
    if Applied.is_err() {
        let _ = unsafe { RemoveDirectoryW(PCWSTR(Wide.as_ptr())) };
    }
    Applied
}

struct CopyContext<'a> {
    State: &'a Shared,
    Base: u64,
}

unsafe extern "system" fn CopyProgress(
    _: i64,
    Transferred: i64,
    _: i64,
    _: i64,
    _: u32,
    _: LPPROGRESS_ROUTINE_CALLBACK_REASON,
    _: HANDLE,
    _: HANDLE,
    Data: *const c_void,
) -> COPYPROGRESSROUTINE_PROGRESS {
    let Context = unsafe { &*(Data as *const CopyContext) };
    if Context.State.IsCancelled() {
        return PROGRESS_CANCEL;
    }
    Context.State.BytesDone.store(Context.Base + Transferred.max(0) as u64, Ordering::Relaxed);
    Context.State.Report();
    PROGRESS_CONTINUE
}

fn CopyName(Directory: &Path, Name: &str, IsDirectory: bool) -> String {
    let (Stem, Extension) = match Name.rfind('.') {
        Some(Position) if !IsDirectory && Position > 0 => (&Name[..Position], &Name[Position..]),
        _ => (Name, ""),
    };
    UniqueName(Directory, &format!("{Stem} - Copy{Extension}"), IsDirectory, &|Candidate: &Path| Exists(Candidate))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Copy,
    Rename,
    Relocate,
}

enum Step {
    Item { Source: PathBuf, Details: Info, Target: PathBuf, Mode: Transfer },
    Close { Directory: PathBuf },
}

enum Outcome {
    Stopped,
    Done { Destination: PathBuf, Replaced: bool, Merged: bool },
}

struct Engine {
    State: Arc<Shared>,
    ApplyAll: Option<CollisionChoice>,
    KeepBothAllowed: bool,
    Result: Summary,
}

impl Engine {
    fn Run(&mut self, Work: &Request, Owner: isize) {
        match Work {
            Request::CopyInto { Sources, Destination } => self.Transfer(PairsInto(Sources, Destination), false),
            Request::MoveInto { Sources, Destination } => self.Transfer(PairsInto(Sources, Destination), true),
            Request::MovePairs(Pairs) => {
                self.KeepBothAllowed = false;
                self.Transfer(Pairs.clone(), true)
            }
            Request::Recycle(Items) => Recycle::Run(self, Items, Owner),
            Request::DeletePermanently(Items) => self.DeleteAll(Items),
            Request::CreateFolder(Item) => self.Create(Item, true),
            Request::CreateFile(Item) => self.Create(Item, false),
        }
    }

    fn Fail(&mut self, Item: &Path, Value: u32) {
        self.Result.Errors.push((Item.to_path_buf(), ErrorText(Value)));
    }

    fn FailText(&mut self, Item: &Path, Message: &str) {
        self.Result.Errors.push((Item.to_path_buf(), Message.to_string()));
    }

    fn Examine(&mut self, Item: &Path) -> Option<Info> {
        Inspect(Item).map_err(|Value| self.Fail(Item, Value)).ok()
    }

    fn ScanTree(&self, Root: &Path, Details: &Info, CountBytes: bool) {
        let Bytes = |Entry: &Info| if CountBytes && !Entry.IsDirectory() { Entry.Size } else { 0 };
        self.State.AddTotals(1, Bytes(Details));
        if !Details.IsPlainDirectory() {
            return;
        }
        let mut Pending = vec![Root.to_path_buf()];
        while let Some(Directory) = Pending.pop() {
            if self.State.IsCancelled() {
                return;
            }
            for (Name, Child) in Children(&Directory).unwrap_or_default() {
                self.State.AddTotals(1, Bytes(&Child));
                if Child.IsPlainDirectory() {
                    Pending.push(Directory.join(Name));
                }
            }
            self.State.Report();
        }
    }

    fn Ask(&mut self, Source: &Path, Details: &Info, Target: &Path, Existing: &Info) -> CollisionChoice {
        if let Some(Choice) = self.ApplyAll {
            return Choice;
        }
        let Query = CollisionQuery {
            Source: Source.to_path_buf(),
            Destination: Target.to_path_buf(),
            SourceSize: Details.Size,
            SourceModified: Details.Modified,
            DestinationSize: Existing.Size,
            DestinationModified: Existing.Modified,
            SourceIsDirectory: Details.IsDirectory(),
            DestinationIsDirectory: Existing.IsDirectory(),
        };
        *Lock(&self.State.Decision) = None;
        self.State.SetPhase(Phase::WaitingForUser);
        (self.State.Notify)(Notification::Collision { Identifier: self.State.Identifier, Query });
        let mut Guard = Lock(&self.State.Decision);
        let (Choice, ApplyToAll) = loop {
            if let Some(Answer) = Guard.take() {
                break Answer;
            }
            if self.State.IsCancelled() {
                break (CollisionChoice::Cancel, false);
            }
            Guard = self.State.Wake.wait(Guard).unwrap_or_else(|Poisoned| Poisoned.into_inner());
        };
        drop(Guard);
        self.State.SetPhase(Phase::Working);
        if Choice == CollisionChoice::Cancel {
            self.State.Cancelled.store(true, Ordering::Release);
        } else if ApplyToAll {
            self.ApplyAll = Some(Choice);
        }
        Choice
    }

    fn Transfer(&mut self, Pairs: Vec<(PathBuf, PathBuf)>, Moving: bool) {
        self.State.SetPhase(Phase::Scanning);
        let mut Plans = Vec::new();
        for (Source, Target) in Pairs {
            if self.State.IsCancelled() {
                break;
            }
            if Moving && SamePath(&Source, &Target) && LeafName(&Source) == LeafName(&Target) {
                continue;
            }
            let Some(Details) = self.Examine(&Source) else {
                continue;
            };
            if Details.IsDirectory() && IsSameOrInside(&Target, &Source) && !SamePath(&Target, &Source) {
                self.FailText(&Source, "The destination folder is inside the source folder");
                continue;
            }
            let Mode = if !Moving {
                Transfer::Copy
            } else {
                match (VolumeSerial(&Source), Target.parent().and_then(VolumeSerial)) {
                    (Some(Left), Some(Right)) if Left != Right => Transfer::Relocate,
                    _ => Transfer::Rename,
                }
            };
            if Mode == Transfer::Rename {
                self.State.AddTotals(1, 0);
            } else {
                self.ScanTree(&Source, &Details, true);
            }
            Plans.push((Source, Details, Target, Mode));
        }
        self.State.SetPhase(Phase::Working);
        self.State.Report();
        for (Source, Details, Target, Mode) in Plans {
            if self.State.IsCancelled() {
                break;
            }
            let Final = match Target.parent() {
                Some(Folder) if !Moving && SamePath(&Source, &Target) => Folder.join(CopyName(Folder, &LeafName(&Source), Details.IsDirectory())),
                _ => Target,
            };
            self.TransferTop(&Source, Details, Final, Mode);
        }
    }

    fn TransferTop(&mut self, Source: &Path, Details: Info, Target: PathBuf, Mode: Transfer) {
        let mut Stack = Vec::new();
        let Placed = self.ProcessItem(Source, Details, Target, Mode, &mut Stack);
        while let Some(Next) = Stack.pop() {
            if self.State.IsCancelled() {
                break;
            }
            match Next {
                Step::Item { Source: ChildSource, Details: ChildDetails, Target: ChildTarget, Mode: ChildMode } => {
                    self.ProcessItem(&ChildSource, ChildDetails, ChildTarget, ChildMode, &mut Stack);
                }
                Step::Close { Directory } => self.CloseDirectory(&Directory),
            }
        }
        if let Outcome::Done { Destination, Replaced, Merged } = Placed {
            if Mode == Transfer::Copy {
                if !Replaced && !Merged {
                    self.Result.Created.push(Destination);
                }
            } else if !Merged && (SamePath(Source, &Destination) || !Exists(Source)) {
                self.Result.Moved.push((Source.to_path_buf(), Destination));
            }
        }
    }

    fn SkipItem(&self, Details: &Info) -> Outcome {
        self.State.AddItems(1);
        if !Details.IsDirectory() {
            self.State.BytesDone.fetch_add(Details.Size, Ordering::Relaxed);
        }
        self.State.Report();
        Outcome::Stopped
    }

    fn ProcessItem(&mut self, Source: &Path, Details: Info, Target: PathBuf, Mode: Transfer, Stack: &mut Vec<Step>) -> Outcome {
        if self.State.IsCancelled() {
            return Outcome::Stopped;
        }
        self.State.SetCurrent(Source);
        let mut Target = Target;
        let mut Present = None;
        if let Some(Existing) = Inspect(&Target).ok().filter(|_| !SamePath(Source, &Target)) {
            if Details.IsPlainDirectory() && Existing.IsDirectory() {
                self.State.AddItems(1);
                self.Expand(Source, &Target, Mode, Stack);
                return Outcome::Done { Destination: Target, Replaced: false, Merged: true };
            }
            match self.Ask(Source, &Details, &Target, &Existing) {
                CollisionChoice::Replace if Existing.IsPlainDirectory() || Details.IsDirectory() != Existing.IsDirectory() => {
                    let Message =
                        if Existing.IsDirectory() { "The existing folder cannot be replaced by this item" } else { "The existing file cannot be replaced by a folder" };
                    self.FailText(&Target, Message);
                    return self.SkipItem(&Details);
                }
                CollisionChoice::Replace => Present = Some(Existing),
                CollisionChoice::Skip => return self.SkipItem(&Details),
                CollisionChoice::KeepBoth if !self.KeepBothAllowed => {
                    self.FailText(&Target, "An item with this name already exists");
                    return self.SkipItem(&Details);
                }
                CollisionChoice::KeepBoth => {
                    let Directory = Target.parent().map(Path::to_path_buf).unwrap_or_default();
                    let Name = UniqueName(&Directory, &LeafName(&Target), Details.IsDirectory(), &|Candidate: &Path| Exists(Candidate));
                    Target = Directory.join(Name);
                }
                CollisionChoice::Cancel => return Outcome::Stopped,
            }
        }
        let mut Restore = None;
        if let Some(Existing) = Present {
            let Protected = AttributeReadOnly | AttributeHidden | AttributeSystem;
            let Prepared = if Existing.IsDirectory() || Existing.IsLink() || Details.IsLink() {
                RemoveItem(&Target, Existing.Attributes)
            } else if Existing.Attributes & Protected != 0 {
                Restore = Some(Existing.Attributes);
                SetAttributes(&Target, Existing.Attributes & !Protected)
            } else {
                Ok(())
            };
            if let Err(Value) = Prepared {
                self.Fail(&Target, Value);
                return self.SkipItem(&Details);
            }
        }
        let Replacing = Present.is_some();
        let Attempt = match Mode {
            Transfer::Rename => match Rename(Source, &Target, Replacing) {
                Err(Value) if Value == ERROR_NOT_SAME_DEVICE.0 => {
                    self.ScanTree(Source, &Details, true);
                    self.Duplicate(Source, &Details, &Target, Transfer::Relocate, Replacing, Stack)
                }
                Other => Other.map(|()| self.State.AddItems(1)),
            },
            _ => self.Duplicate(Source, &Details, &Target, Mode, Replacing, Stack),
        };
        match Attempt {
            Ok(()) => {
                if Replacing {
                    self.Result.Replaced.push(Target.clone());
                }
                self.State.Report();
                Outcome::Done { Destination: Target, Replaced: Replacing, Merged: false }
            }
            Err(Value) => {
                if let Some(Attributes) = Restore.filter(|_| Exists(&Target)) {
                    let _ = SetAttributes(&Target, Attributes);
                }
                if Value == ERROR_REQUEST_ABORTED.0 && self.State.IsCancelled() {
                    return Outcome::Stopped;
                }
                self.Fail(Source, Value);
                self.State.AddItems(1);
                Outcome::Stopped
            }
        }
    }

    fn Expand(&mut self, Source: &Path, Target: &Path, Mode: Transfer, Stack: &mut Vec<Step>) {
        if Mode != Transfer::Copy {
            Stack.push(Step::Close { Directory: Source.to_path_buf() });
        }
        match Children(Source) {
            Ok(Entries) => {
                for (Name, Child) in Entries.into_iter().rev() {
                    Stack.push(Step::Item { Source: Source.join(&Name), Details: Child, Target: Target.join(&Name), Mode });
                }
            }
            Err(Value) => self.Fail(Source, Value),
        }
    }

    fn CloseDirectory(&mut self, Directory: &Path) {
        let Ok(Details) = Inspect(Directory) else {
            return;
        };
        if let Err(Value) = RemoveItem(Directory, Details.Attributes) {
            if Value != ERROR_DIR_NOT_EMPTY.0 {
                self.Fail(Directory, Value);
            }
        }
    }

    fn Duplicate(&mut self, Source: &Path, Details: &Info, Target: &Path, Mode: Transfer, Replacing: bool, Stack: &mut Vec<Step>) -> Result<(), u32> {
        if Details.IsPlainDirectory() {
            let SourceWide = ApiPath(Source);
            let TargetWide = ApiPath(Target);
            let Created = if Details.Attributes & AttributeReparsePoint == 0 {
                unsafe { CreateDirectoryExW(PCWSTR(SourceWide.as_ptr()), PCWSTR(TargetWide.as_ptr()), None) }
            } else {
                unsafe { CreateDirectoryW(PCWSTR(TargetWide.as_ptr()), None) }
            };
            Created.map_err(|Error| Code(&Error))?;
            self.State.AddItems(1);
            self.Expand(Source, Target, Mode, Stack);
            return Ok(());
        }
        if Details.IsLink() && (Details.IsDirectory() || Details.Tag == IO_REPARSE_TAG_SYMLINK) {
            CopyLink(Source, Details, Target)?;
        } else {
            self.CopyFile(Source, Details, Target, Replacing)?;
        }
        self.State.AddItems(1);
        if Mode != Transfer::Copy {
            if let Err(Value) = RemoveItem(Source, Details.Attributes) {
                self.Fail(Source, Value);
            }
        }
        Ok(())
    }

    fn CopyFile(&self, Source: &Path, Details: &Info, Target: &Path, Replacing: bool) -> Result<(), u32> {
        let Base = self.State.BytesDone.load(Ordering::Relaxed);
        let Context = CopyContext { State: &self.State, Base };
        let mut Flags = 0;
        if Details.IsLink() {
            Flags |= COPY_FILE_COPY_SYMLINK.0;
        }
        if !Replacing {
            Flags |= COPY_FILE_FAIL_IF_EXISTS.0;
        }
        let SourceWide = ApiPath(Source);
        let TargetWide = ApiPath(Target);
        let Data = Some(&Context as *const CopyContext as *const c_void);
        let Copied = unsafe { CopyFileExW(PCWSTR(SourceWide.as_ptr()), PCWSTR(TargetWide.as_ptr()), Some(CopyProgress), Data, None, COPYFILE_FLAGS(Flags)) };
        if !self.State.IsCancelled() {
            self.State.BytesDone.store(Base + Details.Size, Ordering::Relaxed);
        }
        Copied.map_err(|Error| Code(&Error))
    }

    fn DeleteAll(&mut self, Items: &[PathBuf]) {
        self.State.SetPhase(Phase::Scanning);
        let mut Plans = Vec::new();
        for Item in Items {
            if self.State.IsCancelled() {
                break;
            }
            if let Some(Details) = self.Examine(Item) {
                self.ScanTree(Item, &Details, false);
                Plans.push((Item.clone(), Details));
            }
        }
        self.State.SetPhase(Phase::Working);
        self.State.Report();
        for (Item, Details) in Plans {
            if self.State.IsCancelled() {
                break;
            }
            self.DeleteTree(&Item, Details);
            if !Exists(&Item) {
                self.Result.Removed.push(Item);
            }
        }
    }

    fn DeleteTree(&mut self, Root: &Path, Details: Info) {
        let mut Stack = vec![(Root.to_path_buf(), Details, false)];
        while let Some((Item, Entry, Expanded)) = Stack.pop() {
            if self.State.IsCancelled() {
                return;
            }
            self.State.SetCurrent(&Item);
            if Entry.IsPlainDirectory() && !Expanded {
                match Children(&Item) {
                    Ok(Entries) => {
                        Stack.push((Item.clone(), Entry, true));
                        Stack.extend(Entries.into_iter().rev().map(|(Name, Child)| (Item.join(Name), Child, false)));
                    }
                    Err(Value) => {
                        self.Fail(&Item, Value);
                        self.State.AddItems(1);
                    }
                }
                continue;
            }
            if let Err(Value) = RemoveItem(&Item, Entry.Attributes) {
                if !(Expanded && Value == ERROR_DIR_NOT_EMPTY.0) {
                    self.Fail(&Item, Value);
                }
            }
            self.State.AddItems(1);
            self.State.Report();
        }
    }

    fn Create(&mut self, Item: &Path, Folder: bool) {
        self.State.AddTotals(1, 0);
        self.State.SetPhase(Phase::Working);
        self.State.SetCurrent(Item);
        let Made = if Folder {
            let Wide = ApiPath(Item);
            unsafe { CreateDirectoryW(PCWSTR(Wide.as_ptr()), None) }.map_err(|Error| Code(&Error))
        } else {
            OpenHandle(Item, GENERIC_WRITE.0, CREATE_NEW, FILE_ATTRIBUTE_NORMAL.0).map(drop)
        };
        match Made {
            Ok(()) => self.Result.Created.push(Item.to_path_buf()),
            Err(Value) => self.Fail(Item, Value),
        }
        self.State.AddItems(1);
    }
}
