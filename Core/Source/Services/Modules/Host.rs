use super::ModuleEvent;
use crate::Application::Event;
use crate::Services::Workers::EventSink;
use FluxSdk::{HostApi, InitializeFunction, InvocationRequest, ModuleApi, PreviewRequest, WideText};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW};
use windows::core::{PCSTR, PCWSTR};

pub struct LoadedModule {
    Handle: HMODULE,
    Api: ModuleApi,
}

unsafe impl Send for LoadedModule {}
unsafe impl Sync for LoadedModule {}

impl Drop for LoadedModule {
    fn drop(&mut self) {
        unsafe {
            if let Some(Shutdown) = self.Api.Shutdown {
                Shutdown();
            }
            let _ = FreeLibrary(self.Handle);
        }
    }
}

pub fn Load(Library: &Path) -> Result<LoadedModule, String> {
    let Wide = crate::FileSystem::WideNull(Library.as_os_str());
    let Handle = unsafe { LoadLibraryExW(PCWSTR(Wide.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) }.map_err(|Error| Error.message())?;
    let Symbol = unsafe { GetProcAddress(Handle, PCSTR(FluxSdk::InitializeSymbol.as_ptr())) };
    let Some(Symbol) = Symbol else {
        unsafe {
            let _ = FreeLibrary(Handle);
        }
        return Err("The library does not export FluxModuleInitialize".to_string());
    };
    let Initialize: InitializeFunction = unsafe { std::mem::transmute(Symbol) };
    let mut Api = ModuleApi { AbiVersion: 0, Size: 0, Invoke: None, Preview: None, Shutdown: None };
    let Status = unsafe { Initialize(FluxSdk::AbiVersion, &mut Api) };
    if Status != FluxSdk::StatusOk || Api.AbiVersion != FluxSdk::AbiVersion {
        unsafe {
            let _ = FreeLibrary(Handle);
        }
        return Err(format!("Initialization failed (status {Status}, ABI {})", Api.AbiVersion));
    }
    Ok(LoadedModule { Handle, Api })
}

pub struct Invocation {
    pub Identifier: u64,
    pub Title: String,
    pub Done: AtomicU64,
    pub Total: AtomicU64,
    pub Cancel: AtomicBool,
    Text: Mutex<String>,
    Paths: Mutex<Vec<PathBuf>>,
    Renames: Mutex<Vec<(PathBuf, PathBuf)>>,
    LastReport: Mutex<Instant>,
    Sink: EventSink,
}

impl Invocation {
    pub fn New(Identifier: u64, Title: String, Sink: EventSink) -> Arc<Invocation> {
        Arc::new(Invocation {
            Identifier,
            Title,
            Done: AtomicU64::new(0),
            Total: AtomicU64::new(0),
            Cancel: AtomicBool::new(false),
            Text: Mutex::new(String::new()),
            Paths: Mutex::new(Vec::new()),
            Renames: Mutex::new(Vec::new()),
            LastReport: Mutex::new(Instant::now()),
            Sink,
        })
    }
}

const TextLimit: usize = 8 * 1024 * 1024;
const PathLimit: usize = 1_000_000;

unsafe extern "C" fn EmitText(Context: *mut c_void, Text: WideText) {
    let State = unsafe { &*(Context as *const Invocation) };
    if let Ok(mut Buffer) = State.Text.lock() {
        if Buffer.len() < TextLimit {
            Buffer.push_str(&unsafe { Text.ToStringLossy() });
        }
    }
}

unsafe extern "C" fn EmitPath(Context: *mut c_void, Item: WideText) {
    let State = unsafe { &*(Context as *const Invocation) };
    if let Ok(mut Paths) = State.Paths.lock() {
        if Paths.len() < PathLimit {
            Paths.push(unsafe { Item.ToPathBuf() });
        }
    }
}

unsafe extern "C" fn EmitRename(Context: *mut c_void, From: WideText, To: WideText) {
    let State = unsafe { &*(Context as *const Invocation) };
    if let Ok(mut Renames) = State.Renames.lock() {
        Renames.push((unsafe { From.ToPathBuf() }, unsafe { To.ToPathBuf() }));
    }
}

unsafe extern "C" fn SetProgress(Context: *mut c_void, Done: u64, Total: u64) {
    let State = unsafe { &*(Context as *const Invocation) };
    State.Done.store(Done, Ordering::Relaxed);
    State.Total.store(Total, Ordering::Relaxed);
    if let Ok(mut Last) = State.LastReport.lock() {
        if Last.elapsed() > Duration::from_millis(150) {
            *Last = Instant::now();
            State.Sink.Send(Event::Module(ModuleEvent::Progress));
        }
    }
}

unsafe extern "C" fn IsCancelled(Context: *mut c_void) -> u32 {
    let State = unsafe { &*(Context as *const Invocation) };
    State.Cancel.load(Ordering::Relaxed) as u32
}

fn HostFor(State: &Arc<Invocation>) -> HostApi {
    HostApi {
        AbiVersion: FluxSdk::AbiVersion,
        Size: std::mem::size_of::<HostApi>() as u32,
        Context: Arc::as_ptr(State) as *mut c_void,
        EmitText,
        EmitPath,
        EmitRename,
        SetProgress,
        IsCancelled,
    }
}

fn Wide(Item: &Path) -> Vec<u16> {
    Item.as_os_str().encode_wide().collect()
}

pub struct InvocationData {
    pub Command: String,
    pub Directory: PathBuf,
    pub OtherDirectory: Option<PathBuf>,
    pub Argument: String,
    pub Selection: Vec<PathBuf>,
}

pub fn RunInvocation(Module: Arc<LoadedModule>, State: Arc<Invocation>, Data: InvocationData) {
    let Started = Instant::now();
    let Command: Vec<u16> = Data.Command.encode_utf16().collect();
    let Directory = Wide(&Data.Directory);
    let Other = Data.OtherDirectory.as_deref().map(Wide).unwrap_or_default();
    let Argument: Vec<u16> = Data.Argument.encode_utf16().collect();
    let SelectionWide: Vec<Vec<u16>> = Data.Selection.iter().map(|Item| Wide(Item)).collect();
    let Selection: Vec<WideText> = SelectionWide.iter().map(|Item| WideText::FromSlice(Item)).collect();
    let Request = InvocationRequest {
        Size: std::mem::size_of::<InvocationRequest>() as u32,
        Command: WideText::FromSlice(&Command),
        Directory: WideText::FromSlice(&Directory),
        OtherDirectory: if Other.is_empty() { WideText::Empty() } else { WideText::FromSlice(&Other) },
        Argument: WideText::FromSlice(&Argument),
        Selection: Selection.as_ptr(),
        SelectionCount: Selection.len(),
    };
    let Host = HostFor(&State);
    let Status = match Module.Api.Invoke {
        Some(Invoke) => unsafe { Invoke(&Host, &Request) },
        None => FluxSdk::StatusUnsupported,
    };
    let Text = State.Text.lock().map(|Buffer| Buffer.clone()).unwrap_or_default();
    let Paths = State.Paths.lock().map(|Paths| Paths.clone()).unwrap_or_default();
    let Renames = State.Renames.lock().map(|Renames| Renames.clone()).unwrap_or_default();
    State.Sink.Send(Event::Module(ModuleEvent::Finished {
        Title: State.Title.clone(),
        Status,
        Text,
        Paths,
        Renames,
        Elapsed: Started.elapsed(),
    }));
}

pub fn RunPreview(Module: Arc<LoadedModule>, State: Arc<Invocation>, Item: PathBuf, Token: u64) {
    let PathWide = Wide(&Item);
    let Request = PreviewRequest { Size: std::mem::size_of::<PreviewRequest>() as u32, Path: WideText::FromSlice(&PathWide), MaxBytes: 64 * 1024 };
    let Host = HostFor(&State);
    let Status = match Module.Api.Preview {
        Some(Preview) => unsafe { Preview(&Host, &Request) },
        None => FluxSdk::StatusUnsupported,
    };
    let Text = State.Text.lock().map(|Buffer| Buffer.clone()).unwrap_or_default();
    let Result = if Status == FluxSdk::StatusOk || !Text.is_empty() { Some(Text) } else { None };
    State.Sink.Send(Event::Module(ModuleEvent::Preview { Token, Text: Result }));
}
