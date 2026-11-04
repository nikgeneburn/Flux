use super::ApiPath;
use super::Enumerate::{OwnedEntry, Stat};
use crate::Application::Event;
use crate::Services::Workers::{EventSink, Spawn};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED, PostQueuedCompletionStatus};
use windows::core::{Owned, PCWSTR};

pub struct ChangeRecord {
    pub Name: Vec<u16>,
    pub Entry: Option<OwnedEntry>,
}

pub struct ChangeBatch {
    pub Watch: u64,
    pub Records: Vec<ChangeRecord>,
    pub Overflow: bool,
}

enum Command {
    Add(u64, PathBuf),
    Remove(u64),
    Stop,
}

struct WatchState {
    Path: PathBuf,
    Handle: Owned<HANDLE>,
    Overlapped: OVERLAPPED,
    Buffer: Vec<u32>,
    Pending: HashMap<Vec<u16>, bool>,
    Overflow: bool,
    Closing: bool,
    Active: bool,
}

struct Port(HANDLE);

unsafe impl Send for Port {}
unsafe impl Sync for Port {}

impl Drop for Port {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub struct Watcher {
    Port: Arc<Port>,
    Commands: Arc<Mutex<Vec<Command>>>,
    NextIdentifier: u64,
}

const CommandKey: usize = 0;
const Debounce: Duration = Duration::from_millis(120);

impl Watcher {
    pub fn New(Sink: EventSink) -> Option<Self> {
        let Handle = unsafe { CreateIoCompletionPort(HANDLE(-1isize as *mut _), None, 0, 1) }.ok()?;
        let PortHandle = Arc::new(Port(Handle));
        let Commands: Arc<Mutex<Vec<Command>>> = Arc::new(Mutex::new(Vec::new()));
        let ThreadPort = PortHandle.clone();
        let ThreadCommands = Commands.clone();
        Spawn("Watcher", move || WatchLoop(ThreadPort, ThreadCommands, Sink)).ok()?;
        Some(Self { Port: PortHandle, Commands, NextIdentifier: 1 })
    }

    fn Post(&self, Item: Command) {
        if let Ok(mut Queue) = self.Commands.lock() {
            Queue.push(Item);
        }
        unsafe {
            let _ = PostQueuedCompletionStatus(self.Port.0, 0, CommandKey, None);
        }
    }

    pub fn Watch(&mut self, Directory: PathBuf) -> u64 {
        let Identifier = self.NextIdentifier;
        self.NextIdentifier += 1;
        self.Post(Command::Add(Identifier, Directory));
        Identifier
    }

    pub fn Unwatch(&self, Identifier: u64) {
        self.Post(Command::Remove(Identifier));
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.Post(Command::Stop);
    }
}

fn Issue(State: &mut WatchState) -> bool {
    State.Overlapped = OVERLAPPED::default();
    let Filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION;
    let Result = unsafe {
        ReadDirectoryChangesW(
            *State.Handle,
            State.Buffer.as_mut_ptr() as *mut _,
            (State.Buffer.len() * 4) as u32,
            false,
            Filter,
            None,
            Some(&mut State.Overlapped),
            None,
        )
    };
    State.Active = Result.is_ok();
    State.Active
}

fn Open(CompletionPort: HANDLE, Identifier: u64, Directory: PathBuf) -> Option<Box<WatchState>> {
    let Wide = ApiPath(&Directory);
    let Handle = unsafe {
        CreateFileW(
            PCWSTR(Wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )
    }
    .ok()?;
    let Handle = unsafe { Owned::new(Handle) };
    unsafe { CreateIoCompletionPort(*Handle, Some(CompletionPort), Identifier as usize, 1) }.ok()?;
    let mut State = Box::new(WatchState {
        Path: Directory,
        Handle,
        Overlapped: OVERLAPPED::default(),
        Buffer: vec![0u32; 16 * 1024],
        Pending: HashMap::new(),
        Overflow: false,
        Closing: false,
        Active: false,
    });
    if Issue(&mut State) { Some(State) } else { None }
}

fn Parse(State: &mut WatchState, Bytes: usize) {
    if Bytes == 0 {
        State.Overflow = true;
        State.Pending.clear();
        return;
    }
    let Base = State.Buffer.as_ptr() as *const u8;
    let mut Offset = 0usize;
    loop {
        if Offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() > Bytes {
            break;
        }
        let Record = unsafe { &*(Base.add(Offset) as *const FILE_NOTIFY_INFORMATION) };
        let Name = unsafe { std::slice::from_raw_parts(Record.FileName.as_ptr(), Record.FileNameLength as usize / 2) };
        if !Name.contains(&(b'\\' as u16)) {
            let Removed = Record.Action == FILE_ACTION_REMOVED || Record.Action == FILE_ACTION_RENAMED_OLD_NAME;
            State.Pending.insert(Name.to_vec(), Removed);
        }
        if Record.NextEntryOffset == 0 {
            break;
        }
        Offset += Record.NextEntryOffset as usize;
    }
}

fn Flush(Identifier: u64, State: &mut WatchState, Sink: &EventSink) {
    if !State.Overflow && State.Pending.is_empty() {
        return;
    }
    let mut Records = Vec::with_capacity(State.Pending.len());
    if !State.Overflow {
        for (Name, _) in State.Pending.drain() {
            let Entry = Stat(&State.Path.join(OsString::from_wide(&Name)));
            Records.push(ChangeRecord { Name, Entry });
        }
    }
    let Overflow = State.Overflow;
    State.Overflow = false;
    State.Pending.clear();
    Sink.Send(Event::DirectoryChanged(ChangeBatch { Watch: Identifier, Records, Overflow }));
}

fn WatchLoop(PortHandle: Arc<Port>, Commands: Arc<Mutex<Vec<Command>>>, Sink: EventSink) {
    let mut Watches: HashMap<u64, Box<WatchState>> = HashMap::new();
    let mut Deadline: Option<Instant> = None;
    loop {
        let Timeout = match Deadline {
            Some(When) => RemainingMilliseconds(When),
            None => u32::MAX,
        };
        let mut Bytes = 0u32;
        let mut Key = 0usize;
        let mut Overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        let Result = unsafe { GetQueuedCompletionStatus(PortHandle.0, &mut Bytes, &mut Key, &mut Overlapped, Timeout) };
        if Result.is_err() && Overlapped.is_null() {
            for (Identifier, State) in Watches.iter_mut() {
                Flush(*Identifier, State, &Sink);
            }
            Deadline = None;
            continue;
        }
        if Key == CommandKey && Overlapped.is_null() {
            let Drained: Vec<Command> = Commands.lock().map(|mut Queue| std::mem::take(&mut *Queue)).unwrap_or_default();
            for Item in Drained {
                match Item {
                    Command::Add(Identifier, Directory) => {
                        if let Some(State) = Open(PortHandle.0, Identifier, Directory) {
                            Watches.insert(Identifier, State);
                        }
                    }
                    Command::Remove(Identifier) => {
                        if let Some(State) = Watches.get_mut(&Identifier) {
                            if State.Active {
                                State.Closing = true;
                                unsafe {
                                    let _ = CancelIoEx(*State.Handle, Some(&State.Overlapped));
                                }
                            } else {
                                Watches.remove(&Identifier);
                            }
                        }
                    }
                    Command::Stop => {
                        for State in Watches.values_mut() {
                            if State.Active {
                                unsafe {
                                    let _ = CancelIoEx(*State.Handle, Some(&State.Overlapped));
                                }
                            }
                        }
                        let Started = Instant::now();
                        while Watches.values().any(|State| State.Active) && Started.elapsed() < Duration::from_secs(2) {
                            let mut Ignored = 0u32;
                            let mut DoneKey = 0usize;
                            let mut DoneOverlapped: *mut OVERLAPPED = std::ptr::null_mut();
                            let _ = unsafe { GetQueuedCompletionStatus(PortHandle.0, &mut Ignored, &mut DoneKey, &mut DoneOverlapped, 200) };
                            if let Some(State) = Watches.get_mut(&(DoneKey as u64)) {
                                if !DoneOverlapped.is_null() {
                                    State.Active = false;
                                }
                            }
                        }
                        if Watches.values().any(|State| State.Active) {
                            std::mem::forget(Watches);
                        }
                        return;
                    }
                }
            }
            continue;
        }
        let Identifier = Key as u64;
        let Some(State) = Watches.get_mut(&Identifier) else { continue };
        State.Active = false;
        if State.Closing {
            Watches.remove(&Identifier);
            continue;
        }
        if Result.is_err() {
            State.Overflow = true;
            Flush(Identifier, State, &Sink);
            Watches.remove(&Identifier);
            continue;
        }
        Parse(State, Bytes as usize);
        if !Issue(State) {
            State.Overflow = true;
            Flush(Identifier, State, &Sink);
            Watches.remove(&Identifier);
            continue;
        }
        if Deadline.is_none() {
            Deadline = Some(Instant::now() + Debounce);
        }
    }
}

fn RemainingMilliseconds(When: Instant) -> u32 {
    When.saturating_duration_since(Instant::now()).as_millis() as u32
}
