pub mod Reporter;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, WriteFile};
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT, EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
    RaiseException, RtlLookupFunctionEntry, RtlVirtualUnwind, SetUnhandledExceptionFilter, UNW_FLAG_NHANDLER,
};
use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetCurrentProcessId,
    GetCurrentThreadId, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW, SetEvent, SetThreadStackGuarantee, UpdateProcThreadAttribute, WaitForMultipleObjects,
};
use windows::core::{PCWSTR, PWSTR};

pub const MonitorArgument: &str = "--crash-monitor";
pub const PanicCode: u32 = 0xE046_4C58;
pub const Magic: u32 = 0x4655_4C58;

pub struct CrashOptions {
    pub CrashDirectory: PathBuf,
    pub UseMonitor: bool,
    pub FullMemoryDump: bool,
}

#[repr(C)]
pub struct SharedBlock {
    pub Magic: u32,
    pub ProcessId: u32,
    pub ThreadId: u32,
    pub Flags: u32,
    pub ExceptionPointers: u64,
    pub ActivityLength: u32,
    pub Activity: [u16; 1024],
    pub PanicLength: u32,
    pub Panic: [u8; 16384],
}

struct Installed {
    Block: *mut SharedBlock,
    CrashEvent: HANDLE,
    DoneEvent: HANDLE,
    Monitor: Option<HANDLE>,
    Directory: Vec<u16>,
    FullDump: bool,
}

unsafe impl Send for Installed {}
unsafe impl Sync for Installed {}

static Global: OnceLock<Installed> = OnceLock::new();
static Crashing: AtomicBool = AtomicBool::new(false);

pub fn SetActivity(Text: &str) {
    let Some(State) = Global.get() else { return };
    let Block = unsafe { &mut *State.Block };
    let mut Length = 0usize;
    for (Slot, Unit) in Block.Activity.iter_mut().zip(Text.encode_utf16()) {
        *Slot = Unit;
        Length += 1;
    }
    Block.ActivityLength = Length as u32;
}

pub fn TriggerTestCrash() -> ! {
    unsafe {
        RaiseException(0xC000_0005, 1, Some(&[1, 0x10]));
    }
    std::process::abort()
}

fn Inheritable() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: true.into() }
}

fn SpawnMonitor(Mapping: HANDLE, CrashEvent: HANDLE, DoneEvent: HANDLE, Options: &CrashOptions) -> Option<HANDLE> {
    let Executable = std::env::current_exe().ok()?;
    let CommandLine = format!(
        "\"{}\" {} {} {} {} {} {} \"{}\"",
        Executable.display(),
        MonitorArgument,
        unsafe { GetCurrentProcessId() },
        Mapping.0 as usize,
        CrashEvent.0 as usize,
        DoneEvent.0 as usize,
        Options.FullMemoryDump as u8,
        Options.CrashDirectory.display()
    );
    let mut CommandWide: Vec<u16> = CommandLine.encode_utf16().chain(std::iter::once(0)).collect();
    let ApplicationWide = crate::FileSystem::WideNull(Executable.as_os_str());
    let Handles = [Mapping, CrashEvent, DoneEvent];
    unsafe {
        let mut Size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut Size);
        let mut Storage = vec![0u8; Size];
        let List = LPPROC_THREAD_ATTRIBUTE_LIST(Storage.as_mut_ptr() as *mut _);
        InitializeProcThreadAttributeList(Some(List), 1, None, &mut Size).ok()?;
        let Updated = UpdateProcThreadAttribute(List, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, Some(Handles.as_ptr() as *const _), std::mem::size_of_val(&Handles), None, None);
        if Updated.is_err() {
            DeleteProcThreadAttributeList(List);
            return None;
        }
        let Startup = STARTUPINFOEXW { StartupInfo: STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOEXW>() as u32, ..Default::default() }, lpAttributeList: List };
        let mut Information = PROCESS_INFORMATION::default();
        let Created = CreateProcessW(
            PCWSTR(ApplicationWide.as_ptr()),
            Some(PWSTR(CommandWide.as_mut_ptr())),
            None,
            None,
            true,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &Startup.StartupInfo,
            &mut Information,
        );
        DeleteProcThreadAttributeList(List);
        Created.ok()?;
        let _ = CloseHandle(Information.hThread);
        Some(Information.hProcess)
    }
}

pub fn Install(Options: CrashOptions) {
    let _ = std::fs::create_dir_all(&Options.CrashDirectory);
    unsafe {
        let mut Guarantee = 64 * 1024u32;
        let _ = SetThreadStackGuarantee(&mut Guarantee);
    }
    let Attributes = Inheritable();
    let Size = std::mem::size_of::<SharedBlock>() as u32;
    let Mapping = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&Attributes), PAGE_READWRITE, 0, Size, PCWSTR::null()) };
    let Ok(Mapping) = Mapping else { return };
    let View = unsafe { MapViewOfFile(Mapping, FILE_MAP_ALL_ACCESS, 0, 0, Size as usize) };
    if View.Value.is_null() {
        return;
    }
    let Block = View.Value as *mut SharedBlock;
    unsafe {
        (*Block).Magic = Magic;
        (*Block).ProcessId = GetCurrentProcessId();
    }
    let CrashEvent = unsafe { CreateEventW(Some(&Attributes), false, false, PCWSTR::null()) };
    let DoneEvent = unsafe { CreateEventW(Some(&Attributes), true, false, PCWSTR::null()) };
    let (Ok(CrashEvent), Ok(DoneEvent)) = (CrashEvent, DoneEvent) else { return };
    let Monitor = if Options.UseMonitor { SpawnMonitor(Mapping, CrashEvent, DoneEvent, &Options) } else { None };
    let DirectoryWide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(Options.CrashDirectory.as_os_str()).collect();
    let _ = Global.set(Installed { Block, CrashEvent, DoneEvent, Monitor, Directory: DirectoryWide, FullDump: Options.FullMemoryDump });
    unsafe {
        SetUnhandledExceptionFilter(Some(Filter));
    }
    std::panic::set_hook(Box::new(|Info| {
        let Location = Info.location().map(|Place| format!("{}:{}:{}", Place.file(), Place.line(), Place.column())).unwrap_or_default();
        let Message = Info.payload().downcast_ref::<&str>().map(|Text| Text.to_string()).or_else(|| Info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "non-string panic payload".to_string());
        let Thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let Trace = std::backtrace::Backtrace::force_capture();
        let Text = format!("Panic on thread '{Thread}' at {Location}\n{Message}\n\nRust backtrace:\n{Trace}");
        if let Some(State) = Global.get() {
            let Block = unsafe { &mut *State.Block };
            let Bytes = Text.as_bytes();
            let Length = Bytes.len().min(Block.Panic.len());
            Block.Panic[..Length].copy_from_slice(&Bytes[..Length]);
            Block.PanicLength = Length as u32;
        }
        unsafe {
            RaiseException(PanicCode, 1, None);
        }
        std::process::abort();
    }));
}

unsafe extern "system" fn Filter(Pointers: *const EXCEPTION_POINTERS) -> i32 {
    if Crashing.swap(true, Ordering::SeqCst) {
        return 1;
    }
    let Some(State) = Global.get() else { return 0 };
    unsafe {
        let Block = &mut *State.Block;
        Block.ThreadId = GetCurrentThreadId();
        Block.ExceptionPointers = Pointers as u64;
        if let Some(Monitor) = State.Monitor {
            if SetEvent(State.CrashEvent).is_ok() {
                let Result = WaitForMultipleObjects(&[State.DoneEvent, Monitor], false, 120_000);
                if Result == WAIT_OBJECT_0 {
                    return 1;
                }
            }
        }
        Fallback(State, Pointers);
    }
    1
}

struct FixedText {
    Bytes: [u8; 16384],
    Length: usize,
}

impl FixedText {
    fn Push(&mut self, Text: &[u8]) {
        let Available = self.Bytes.len() - self.Length;
        let Count = Text.len().min(Available);
        self.Bytes[self.Length..self.Length + Count].copy_from_slice(&Text[..Count]);
        self.Length += Count;
    }

    fn Hex(&mut self, Value: u64, Digits: usize) {
        let mut Buffer = [b'0'; 16];
        let Table = b"0123456789ABCDEF";
        for Index in 0..Digits.min(16) {
            Buffer[Digits - 1 - Index] = Table[((Value >> (Index * 4)) & 0xF) as usize];
        }
        self.Push(b"0x");
        self.Push(&Buffer[..Digits.min(16)]);
    }

    fn Decimal(&mut self, Value: u64) {
        let mut Buffer = [0u8; 20];
        let mut Position = Buffer.len();
        let mut Remaining = Value;
        loop {
            Position -= 1;
            Buffer[Position] = b'0' + (Remaining % 10) as u8;
            Remaining /= 10;
            if Remaining == 0 {
                break;
            }
        }
        self.Push(&Buffer[Position..]);
    }

    fn Wide(&mut self, Text: &[u16]) {
        for &Unit in Text {
            if Unit == 0 {
                break;
            }
            self.Push(&[if Unit < 128 { Unit as u8 } else { b'?' }]);
        }
    }
}

unsafe fn OpenOutput(Directory: &[u16], Suffix: &[u16]) -> Option<HANDLE> {
    let mut Name = [0u16; 1024];
    let Prefix: &[u16] = &[b'\\' as u16, b'F' as u16, b'l' as u16, b'u' as u16, b'x' as u16, b'-' as u16, b'f' as u16, b'a' as u16, b'l' as u16, b'l' as u16, b'b' as u16, b'a' as u16, b'c' as u16, b'k' as u16, b'-' as u16];
    let mut Position = 0;
    for &Unit in Directory.iter().chain(Prefix.iter()) {
        if Position + 40 >= Name.len() {
            return None;
        }
        Name[Position] = Unit;
        Position += 1;
    }
    let mut Value = unsafe { GetCurrentProcessId() } as u64 * 100000 + (unsafe { windows::Win32::System::SystemInformation::GetTickCount() } as u64 % 100000);
    let mut Digits = [0u16; 20];
    let mut Count = 0;
    loop {
        Digits[Count] = b'0' as u16 + (Value % 10) as u16;
        Value /= 10;
        Count += 1;
        if Value == 0 {
            break;
        }
    }
    for Index in (0..Count).rev() {
        Name[Position] = Digits[Index];
        Position += 1;
    }
    for &Unit in Suffix {
        Name[Position] = Unit;
        Position += 1;
    }
    Name[Position] = 0;
    unsafe { CreateFileW(PCWSTR(Name.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_READ, None, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None) }.ok()
}

unsafe fn Fallback(State: &Installed, Pointers: *const EXCEPTION_POINTERS) {
    unsafe {
        let ThreadId = GetCurrentThreadId();
        let Dump: [u16; 4] = [b'.' as u16, b'd' as u16, b'm' as u16, b'p' as u16];
        if let Some(File) = OpenOutput(&State.Directory, &Dump) {
            let Information = MINIDUMP_EXCEPTION_INFORMATION { ThreadId, ExceptionPointers: Pointers as *mut _, ClientPointers: false.into() };
            let Kind = if State.FullDump { windows::Win32::System::Diagnostics::Debug::MiniDumpWithFullMemory } else { MiniDumpNormal };
            let _ = MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), File, Kind | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules, Some(&Information), None, None);
            let _ = CloseHandle(File);
        }
        let Report: [u16; 4] = [b'.' as u16, b't' as u16, b'x' as u16, b't' as u16];
        let Some(File) = OpenOutput(&State.Directory, &Report) else { return };
        let mut Text = FixedText { Bytes: [0; 16384], Length: 0 };
        Text.Push(b"Flux crash report (in-process emergency fallback; the crash monitor was unavailable)\r\nVersion: ");
        Text.Push(env!("CARGO_PKG_VERSION").as_bytes());
        Text.Push(b"\r\nThread: ");
        Text.Decimal(ThreadId as u64);
        if !Pointers.is_null() && !(*Pointers).ExceptionRecord.is_null() {
            let Record = &*(*Pointers).ExceptionRecord;
            Text.Push(b"\r\nException code: ");
            Text.Hex(Record.ExceptionCode.0 as u32 as u64, 8);
            Text.Push(b" (");
            Text.Push(Reporter::ExceptionName(Record.ExceptionCode.0 as u32).as_bytes());
            Text.Push(b")\r\nAddress: ");
            Text.Hex(Record.ExceptionAddress as u64, 16);
        }
        let Block = &*State.Block;
        if Block.PanicLength > 0 {
            Text.Push(b"\r\n\r\n");
            Text.Push(&Block.Panic[..(Block.PanicLength as usize).min(Block.Panic.len())]);
        }
        if !Pointers.is_null() && !(*Pointers).ContextRecord.is_null() {
            let mut Context: CONTEXT = *(*Pointers).ContextRecord;
            let Registers = [
                ("RAX", Context.Rax),
                ("RBX", Context.Rbx),
                ("RCX", Context.Rcx),
                ("RDX", Context.Rdx),
                ("RSI", Context.Rsi),
                ("RDI", Context.Rdi),
                ("RBP", Context.Rbp),
                ("RSP", Context.Rsp),
                ("R8", Context.R8),
                ("R9", Context.R9),
                ("R10", Context.R10),
                ("R11", Context.R11),
                ("R12", Context.R12),
                ("R13", Context.R13),
                ("R14", Context.R14),
                ("R15", Context.R15),
                ("RIP", Context.Rip),
            ];
            Text.Push(b"\r\n\r\nRegisters:\r\n");
            for (Name, Value) in Registers {
                Text.Push(Name.as_bytes());
                Text.Push(b" = ");
                Text.Hex(Value, 16);
                Text.Push(b"\r\n");
            }
            Text.Push(b"\r\nStack (unwound in-process):\r\n");
            for Frame in 0..64 {
                if Context.Rip == 0 {
                    break;
                }
                Text.Push(b"#");
                Text.Decimal(Frame);
                Text.Push(b" ");
                Text.Hex(Context.Rip, 16);
                let mut Module = HMODULE::default();
                if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(Context.Rip as *const u16), &mut Module).is_ok() {
                    let mut Path = [0u16; 260];
                    let Length = GetModuleFileNameW(Some(Module), &mut Path) as usize;
                    let Slash = Path[..Length].iter().rposition(|&Unit| Unit == b'\\' as u16).map(|Position| Position + 1).unwrap_or(0);
                    Text.Push(b" ");
                    Text.Wide(&Path[Slash..Length]);
                    Text.Push(b"+");
                    Text.Hex(Context.Rip - Module.0 as u64, 8);
                }
                Text.Push(b"\r\n");
                let mut ImageBase = 0u64;
                let Entry = RtlLookupFunctionEntry(Context.Rip, &mut ImageBase, None);
                if Entry.is_null() {
                    if Context.Rsp == 0 {
                        break;
                    }
                    Context.Rip = std::ptr::read_unaligned(Context.Rsp as *const u64);
                    Context.Rsp += 8;
                } else {
                    let mut HandlerData: *mut core::ffi::c_void = std::ptr::null_mut();
                    let mut Establisher = 0u64;
                    let _ = RtlVirtualUnwind(UNW_FLAG_NHANDLER, ImageBase, Context.Rip, Entry, &mut Context, &mut HandlerData, &mut Establisher, None);
                }
            }
        }
        let _ = WriteFile(File, Some(&Text.Bytes[..Text.Length]), None, None);
        let _ = CloseHandle(File);
    }
}

pub fn ReportPath(Directory: &Path, Stamp: &str, ProcessId: u32, Extension: &str) -> PathBuf {
    Directory.join(format!("Flux-{Stamp}-{ProcessId}.{Extension}"))
}

pub fn RunMonitor(Arguments: &[String]) -> i32 {
    Reporter::Run(Arguments)
}
