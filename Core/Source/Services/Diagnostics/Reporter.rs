use super::{Magic, PanicCode, ReportPath, SharedBlock};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_WRITE, HANDLE, HMODULE, SYSTEMTIME, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ};
use windows::Win32::System::Diagnostics::Debug::{
    AddrModeFlat, CONTEXT, CONTEXT_FULL_AMD64, EXCEPTION_POINTERS, EXCEPTION_RECORD, GetThreadContext, IMAGE_NT_HEADERS64, IMAGEHLP_LINEW64,
    IMAGEHLP_MODULEW64, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpScanMemory, MiniDumpWithFullMemory, MiniDumpWithFullMemoryInfo, MiniDumpWithHandleData,
    MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, ReadProcessMemory, STACKFRAME_EX,
    SYMBOL_INFOW, SYMFLAG_FRAMEREL, SYMFLAG_PARAMETER, SYMFLAG_REGISTER, SYMFLAG_REGREL, SYMOPT_DEFERRED_LOADS, SYMOPT_FAIL_CRITICAL_ERRORS,
    SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, StackWalkEx, SymCleanup, SymEnumSymbolsW, SymFromAddrW, SymFunctionTableAccess64, SymGetLineFromAddrW64,
    SymGetModuleBase64, SymGetModuleInfoW64, SymGetTypeInfo, SymInitializeW, SymSetOptions, SymSetScopeFromAddr, TI_GET_LENGTH,
};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile};
use windows::Win32::System::ProcessStatus::{EnumProcessModulesEx, GetModuleFileNameExW, GetModuleInformation, LIST_MODULES_ALL, MODULEINFO};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetNativeSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, OSVERSIONINFOEXW, SYSTEM_INFO};
use windows::Win32::System::SystemServices::IMAGE_DOS_HEADER;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, OpenThread, PROCESS_DUP_HANDLE, PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_VM_READ, SetEvent, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME, WaitForMultipleObjects,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONERROR, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW, SW_SHOWNORMAL};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

pub fn ExceptionName(Code: u32) -> &'static str {
    match Code {
        0xC000_0005 => "Access violation",
        0xC000_00FD => "Stack overflow",
        0xC000_001D => "Illegal instruction",
        0xC000_0094 => "Integer division by zero",
        0xC000_0095 => "Integer overflow",
        0xC000_0096 => "Privileged instruction",
        0xC000_0006 => "In-page I/O error",
        0xC000_0008 => "Invalid handle",
        0xC000_008C => "Array bounds exceeded",
        0xC000_008E => "Floating-point division by zero",
        0xC000_0374 => "Heap corruption",
        0xC000_0409 => "Stack buffer overrun / fail-fast",
        0xC000_0602 => "Fail-fast exception",
        0x8000_0003 => "Breakpoint",
        0x8000_0002 => "Datatype misalignment",
        0xE06D_7363 => "C++ exception",
        PanicCode => "Rust panic",
        _ => "Unknown exception",
    }
}

fn Read<T: Copy>(Process: HANDLE, Address: u64) -> Option<T> {
    let mut Value = std::mem::MaybeUninit::<T>::uninit();
    unsafe {
        ReadProcessMemory(Process, Address as *const _, Value.as_mut_ptr() as *mut _, std::mem::size_of::<T>(), None).ok()?;
        Some(Value.assume_init())
    }
}

fn Timestamp() -> String {
    let Time: SYSTEMTIME = unsafe { GetLocalTime() };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", Time.wYear, Time.wMonth, Time.wDay, Time.wHour, Time.wMinute, Time.wSecond)
}

fn WideText(Buffer: &[u16]) -> String {
    let End = Buffer.iter().position(|&Unit| Unit == 0).unwrap_or(Buffer.len());
    String::from_utf16_lossy(&Buffer[..End])
}

struct Symbols {
    Process: HANDLE,
    Buffer: Vec<u64>,
}

impl Symbols {
    fn New(Process: HANDLE) -> Symbols {
        let Executable = std::env::current_exe().ok().and_then(|Path| Path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let mut Search = Executable.display().to_string();
        if let Ok(Extra) = std::env::var("_NT_SYMBOL_PATH") {
            Search.push(';');
            Search.push_str(&Extra);
        }
        unsafe {
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES | SYMOPT_FAIL_CRITICAL_ERRORS);
            let _ = SymInitializeW(Process, &HSTRING::from(Search), true);
        }
        Symbols { Process, Buffer: vec![0u64; (std::mem::size_of::<SYMBOL_INFOW>() + 1024 * 2) / 8 + 1] }
    }

    fn Describe(&mut self, Address: u64) -> String {
        let mut Text = String::new();
        let mut Module = IMAGEHLP_MODULEW64 { SizeOfStruct: std::mem::size_of::<IMAGEHLP_MODULEW64>() as u32, ..Default::default() };
        let HasModule = unsafe { SymGetModuleInfoW64(self.Process, Address, &mut Module) }.is_ok();
        if HasModule {
            let _ = write!(Text, "{}", WideText(&Module.ModuleName));
        }
        let Symbol = self.Buffer.as_mut_ptr() as *mut SYMBOL_INFOW;
        unsafe {
            (*Symbol).SizeOfStruct = std::mem::size_of::<SYMBOL_INFOW>() as u32;
            (*Symbol).MaxNameLen = 1024;
        }
        let mut Displacement = 0u64;
        if unsafe { SymFromAddrW(self.Process, Address, Some(&mut Displacement), Symbol) }.is_ok() {
            let Name = unsafe { std::slice::from_raw_parts((*Symbol).Name.as_ptr(), (*Symbol).NameLen.min(1024) as usize) };
            let _ = write!(Text, "!{}+0x{:X}", String::from_utf16_lossy(Name), Displacement);
        } else if HasModule {
            let _ = write!(Text, "+0x{:X}", Address - Module.BaseOfImage);
        } else {
            Text.push_str("<unknown module>");
        }
        let mut Line = IMAGEHLP_LINEW64 { SizeOfStruct: std::mem::size_of::<IMAGEHLP_LINEW64>() as u32, ..Default::default() };
        let mut LineDisplacement = 0u32;
        if unsafe { SymGetLineFromAddrW64(self.Process, Address, &mut LineDisplacement, &mut Line) }.is_ok() && !Line.FileName.is_null() {
            let _ = write!(Text, " [{}:{}]", String::from_utf16_lossy(unsafe { Line.FileName.as_wide() }), Line.LineNumber);
        }
        Text
    }
}

impl Drop for Symbols {
    fn drop(&mut self) {
        unsafe {
            let _ = SymCleanup(self.Process);
        }
    }
}

struct Parameter {
    Name: String,
    Flags: u32,
    Register: u32,
    Offset: u64,
    TypeIndex: u32,
    ModuleBase: u64,
}

unsafe extern "system" fn CollectParameter(Symbol: *const SYMBOL_INFOW, _Size: u32, Context: *const core::ffi::c_void) -> BOOL {
    unsafe {
        let Output = &mut *(Context as *mut Vec<Parameter>);
        let Info = &*Symbol;
        if Info.Flags.0 & SYMFLAG_PARAMETER.0 != 0 && Output.len() < 16 {
            let Name = std::slice::from_raw_parts(Info.Name.as_ptr(), Info.NameLen.min(256) as usize);
            Output.push(Parameter { Name: String::from_utf16_lossy(Name), Flags: Info.Flags.0, Register: Info.Register, Offset: Info.Address, TypeIndex: Info.TypeIndex, ModuleBase: Info.ModBase });
        }
    }
    true.into()
}

fn RegisterValue(Context: &CONTEXT, Register: u32) -> Option<u64> {
    Some(match Register {
        328 => Context.Rax,
        329 => Context.Rbx,
        330 => Context.Rcx,
        331 => Context.Rdx,
        332 => Context.Rsi,
        333 => Context.Rdi,
        334 => Context.Rbp,
        335 => Context.Rsp,
        336 => Context.R8,
        337 => Context.R9,
        338 => Context.R10,
        339 => Context.R11,
        340 => Context.R12,
        341 => Context.R13,
        342 => Context.R14,
        343 => Context.R15,
        _ => return None,
    })
}

fn Parameters(Process: HANDLE, Frame: &STACKFRAME_EX, TopContext: Option<&CONTEXT>, Output: &mut String) {
    let mut Found: Vec<Parameter> = Vec::new();
    unsafe {
        if SymSetScopeFromAddr(Process, Frame.AddrPC.Offset).is_ok() {
            let _ = SymEnumSymbolsW(Process, 0, PCWSTR::null(), Some(CollectParameter), Some(&mut Found as *mut _ as *const _));
        }
    }
    if Found.is_empty() {
        let _ = writeln!(
            Output,
            "        raw stack slots (may not be arguments): 0x{:X} 0x{:X} 0x{:X} 0x{:X}",
            Frame.Params[0], Frame.Params[1], Frame.Params[2], Frame.Params[3]
        );
        return;
    }
    for Item in Found {
        let Value = if Item.Flags & SYMFLAG_REGISTER.0 != 0 {
            TopContext.and_then(|Context| RegisterValue(Context, Item.Register)).map(|Value| format!("0x{Value:X} (register)"))
        } else {
            let Base = if Item.Flags & SYMFLAG_FRAMEREL.0 != 0 {
                Some(Frame.AddrFrame.Offset)
            } else if Item.Flags & SYMFLAG_REGREL.0 != 0 {
                match (Item.Register, TopContext) {
                    (335, _) => Some(Frame.AddrStack.Offset),
                    (334, _) => Some(Frame.AddrFrame.Offset),
                    (Register, Some(Context)) => RegisterValue(Context, Register),
                    _ => None,
                }
            } else {
                None
            };
            Base.map(|Base| Base.wrapping_add(Item.Offset)).and_then(|Address| {
                let mut Length = 0u64;
                let Known = unsafe { SymGetTypeInfo(Process, Item.ModuleBase, Item.TypeIndex, TI_GET_LENGTH, &mut Length as *mut u64 as *mut _) }.is_ok();
                if Known && (Length == 0 || Length > 8) {
                    return Some(format!("<{Length}-byte value at 0x{Address:X}>"));
                }
                let Size = if Known { Length as u32 } else { 8 };
                let Raw: Option<u64> = Read::<u64>(Process, Address);
                Raw.map(|Raw| {
                    let Mask = if Size >= 8 { u64::MAX } else { (1u64 << (Size * 8)) - 1 };
                    format!("0x{:X}", Raw & Mask)
                })
            })
        };
        let _ = writeln!(Output, "        {} = {}", Item.Name, Value.unwrap_or_else(|| "<unavailable>".to_string()));
    }
}

unsafe extern "system" fn TableAccess(Process: HANDLE, Address: u64) -> *mut core::ffi::c_void {
    unsafe { SymFunctionTableAccess64(Process, Address) }
}

unsafe extern "system" fn ModuleBase(Process: HANDLE, Address: u64) -> u64 {
    unsafe { SymGetModuleBase64(Process, Address) }
}

fn WalkStack(Process: HANDLE, Thread: HANDLE, Start: &CONTEXT, Lookup: &mut Symbols, Output: &mut String) {
    let mut Context = AlignedContext(*Start);
    let Top = *Start;
    let mut Frame = STACKFRAME_EX { StackFrameSize: std::mem::size_of::<STACKFRAME_EX>() as u32, ..Default::default() };
    Frame.AddrPC.Offset = Context.0.Rip;
    Frame.AddrPC.Mode = AddrModeFlat;
    Frame.AddrFrame.Offset = Context.0.Rbp;
    Frame.AddrFrame.Mode = AddrModeFlat;
    Frame.AddrStack.Offset = Context.0.Rsp;
    Frame.AddrStack.Mode = AddrModeFlat;
    for Index in 0..128 {
        let Walked = unsafe {
            StackWalkEx(0x8664, Process, Thread, &mut Frame, &mut Context.0 as *mut CONTEXT as *mut _, None, Some(TableAccess), Some(ModuleBase), None, 0)
        };
        if !Walked.as_bool() || Frame.AddrPC.Offset == 0 {
            if Index == 0 {
                let _ = writeln!(Output, "    #0  0x{:016X}  {}", Start.Rip, Lookup.Describe(Start.Rip));
            }
            break;
        }
        let _ = writeln!(Output, "    #{:<2} 0x{:016X}  {}", Index, Frame.AddrPC.Offset, Lookup.Describe(Frame.AddrPC.Offset));
        Parameters(Process, &Frame, if Index == 0 { Some(&Top) } else { None }, Output);
        if Frame.AddrReturn.Offset == 0 {
            break;
        }
    }
}

fn Registers(Context: &CONTEXT, Output: &mut String) {
    let Values = [
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
    for Pair in Values.chunks(3) {
        let Line: Vec<String> = Pair.iter().map(|(Name, Value)| format!("{Name:>3} = 0x{Value:016X}")).collect();
        let _ = writeln!(Output, "    {}", Line.join("   "));
    }
    let _ = writeln!(Output, "    EFLAGS = 0x{:08X}   CS = 0x{:04X}   SS = 0x{:04X}   MXCSR = 0x{:08X}", Context.EFlags, Context.SegCs, Context.SegSs, Context.MxCsr);
}

fn SystemDescription(Output: &mut String) {
    unsafe {
        let mut Version = OSVERSIONINFOEXW { dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32, ..Default::default() };
        if let Ok(Ntdll) = GetModuleHandleW(w!("ntdll.dll")) {
            if let Some(Function) = GetProcAddress(Ntdll, windows::core::s!("RtlGetVersion")) {
                let Call: extern "system" fn(*mut OSVERSIONINFOEXW) -> i32 = std::mem::transmute(Function);
                Call(&mut Version);
            }
        }
        let _ = writeln!(Output, "Operating system: Windows {}.{} build {} (product type {})", Version.dwMajorVersion, Version.dwMinorVersion, Version.dwBuildNumber, Version.wProductType);
        let mut Info = SYSTEM_INFO::default();
        GetNativeSystemInfo(&mut Info);
        let _ = writeln!(Output, "Processors: {} · architecture {} · page size {}", Info.dwNumberOfProcessors, Info.Anonymous.Anonymous.wProcessorArchitecture.0, Info.dwPageSize);
        let mut Memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if GlobalMemoryStatusEx(&mut Memory).is_ok() {
            let _ = writeln!(Output, "Memory: {} MB total, {} MB available, load {}%", Memory.ullTotalPhys / 1048576, Memory.ullAvailPhys / 1048576, Memory.dwMemoryLoad);
        }
    }
}

fn BuildStamp() -> u32 {
    unsafe {
        let Ok(Module) = GetModuleHandleW(PCWSTR::null()) else { return 0 };
        let Dos = &*(Module.0 as *const IMAGE_DOS_HEADER);
        let Headers = &*((Module.0 as usize + Dos.e_lfanew as usize) as *const IMAGE_NT_HEADERS64);
        Headers.FileHeader.TimeDateStamp
    }
}

fn RemoteTimestamp(Process: HANDLE, Base: u64) -> u32 {
    let Some(Dos) = Read::<IMAGE_DOS_HEADER>(Process, Base) else { return 0 };
    Read::<IMAGE_NT_HEADERS64>(Process, Base + Dos.e_lfanew as u64).map(|Headers| Headers.FileHeader.TimeDateStamp).unwrap_or(0)
}

fn ModulesSection(Process: HANDLE, Output: &mut String) {
    let mut Modules = vec![HMODULE::default(); 1024];
    let mut Needed = 0u32;
    unsafe {
        if EnumProcessModulesEx(Process, Modules.as_mut_ptr(), (Modules.len() * std::mem::size_of::<HMODULE>()) as u32, &mut Needed, LIST_MODULES_ALL).is_err() {
            let _ = writeln!(Output, "    <module enumeration failed>");
            return;
        }
    }
    let Count = (Needed as usize / std::mem::size_of::<HMODULE>()).min(Modules.len());
    for Module in &Modules[..Count] {
        let mut Name = [0u16; 1024];
        let Length = unsafe { GetModuleFileNameExW(Some(Process), Some(*Module), &mut Name) } as usize;
        let mut Info = MODULEINFO::default();
        let _ = unsafe { GetModuleInformation(Process, *Module, &mut Info, std::mem::size_of::<MODULEINFO>() as u32) };
        let mut Symbols = IMAGEHLP_MODULEW64 { SizeOfStruct: std::mem::size_of::<IMAGEHLP_MODULEW64>() as u32, ..Default::default() };
        let SymbolText = if unsafe { SymGetModuleInfoW64(Process, Module.0 as u64, &mut Symbols) }.is_ok() {
            let Pdb = WideText(&Symbols.LoadedPdbName);
            let Kind = match Symbols.SymType.0 {
                0 => "no symbols",
                1 => "COFF",
                2 => "CodeView",
                3 => "PDB",
                4 => "exports",
                5 => "deferred",
                6 => "SYM",
                7 => "DIA",
                8 => "virtual",
                _ => "other",
            };
            let Stamp = if Symbols.TimeDateStamp != 0 { Symbols.TimeDateStamp } else { RemoteTimestamp(Process, Module.0 as u64) };
            format!("timestamp 0x{:08X} · {}{}", Stamp, Kind, if Pdb.is_empty() { String::new() } else { format!(" ({Pdb})") })
        } else {
            String::new()
        };
        let _ = writeln!(Output, "    0x{:016X} - 0x{:016X}  {}  {}", Module.0 as u64, Module.0 as u64 + Info.SizeOfImage as u64, String::from_utf16_lossy(&Name[..Length]), SymbolText);
    }
}

fn Uptime(Process: HANDLE) -> Option<f64> {
    let (mut Creation, mut Exit, mut Kernel, mut User) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(Process, &mut Creation, &mut Exit, &mut Kernel, &mut User) }.ok()?;
    let Created = ((Creation.dwHighDateTime as u64) << 32) | Creation.dwLowDateTime as u64;
    Some((crate::FileSystem::Search::NowFileTime() as u64).saturating_sub(Created) as f64 / 10_000_000.0)
}

struct ThreadHandle(HANDLE);

impl Drop for ThreadHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn SuspendOthers(ProcessId: u32, Crashed: u32) -> Vec<(u32, ThreadHandle)> {
    let mut Threads = Vec::new();
    let Ok(Snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else { return Threads };
    let _Guard = ThreadHandle(Snapshot);
    let mut Entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut Valid = unsafe { Thread32First(Snapshot, &mut Entry) }.is_ok();
    while Valid {
        if Entry.th32OwnerProcessID == ProcessId {
            let Access = THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION;
            if let Ok(Handle) = unsafe { OpenThread(Access, false, Entry.th32ThreadID) } {
                if Entry.th32ThreadID != Crashed {
                    unsafe { SuspendThread(Handle) };
                }
                Threads.push((Entry.th32ThreadID, ThreadHandle(Handle)));
            }
        }
        Valid = unsafe { Thread32Next(Snapshot, &mut Entry) }.is_ok();
    }
    Threads
}

fn WriteDump(Process: HANDLE, ProcessId: u32, Target: &Path, ThreadId: u32, Pointers: u64, Full: bool) -> Result<(), String> {
    let Wide = crate::FileSystem::WideNull(Target.as_os_str());
    let File = unsafe { CreateFileW(PCWSTR(Wide.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_READ, None, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None) }.map_err(|Error| Error.message())?;
    let Information = MINIDUMP_EXCEPTION_INFORMATION { ThreadId, ExceptionPointers: Pointers as *mut EXCEPTION_POINTERS, ClientPointers: true.into() };
    let Kind = if Full {
        MiniDumpWithFullMemory | MiniDumpWithFullMemoryInfo | MiniDumpWithHandleData | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules
    } else {
        MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules | MiniDumpWithHandleData | MiniDumpWithIndirectlyReferencedMemory | MiniDumpScanMemory
    };
    let Result = unsafe { MiniDumpWriteDump(Process, ProcessId, File, Kind, if Pointers != 0 { Some(&Information) } else { None }, None, None) };
    unsafe {
        let _ = CloseHandle(File);
    }
    Result.map_err(|Error| Error.message())
}

fn Report(Process: HANDLE, ProcessId: u32, Block: &SharedBlock, Threads: &[(u32, ThreadHandle)], DumpResult: &Result<(), String>, DumpPath: &Path) -> String {
    let mut Output = String::with_capacity(64 * 1024);
    let _ = writeln!(Output, "Flux crash report");
    let _ = writeln!(Output, "=================");
    let _ = writeln!(Output, "Application: Flux {} ({} build, {}, linker timestamp 0x{:08X})", env!("CARGO_PKG_VERSION"), if cfg!(debug_assertions) { "debug" } else { "release" }, std::env::consts::ARCH, BuildStamp());
    let _ = writeln!(Output, "Process ID: {ProcessId}");
    if let Some(Seconds) = Uptime(Process) {
        let _ = writeln!(Output, "Process uptime: {Seconds:.1} s");
    }
    let Activity = String::from_utf16_lossy(&Block.Activity[..(Block.ActivityLength as usize).min(Block.Activity.len())]);
    let _ = writeln!(Output, "Last activity: {}", if Activity.is_empty() { "<none recorded>" } else { &Activity });
    SystemDescription(&mut Output);
    match DumpResult {
        Ok(()) => {
            let _ = writeln!(Output, "Minidump: {}", DumpPath.display());
        }
        Err(Message) => {
            let _ = writeln!(Output, "Minidump: failed ({Message})");
        }
    }
    let _ = writeln!(Output);
    let Pointers: Option<EXCEPTION_POINTERS> = if Block.ExceptionPointers != 0 { Read(Process, Block.ExceptionPointers) } else { None };
    let Record: Option<EXCEPTION_RECORD> = Pointers.and_then(|Value| Read(Process, Value.ExceptionRecord as u64));
    let Context: Option<AlignedContext> = Pointers.and_then(|Value| Read::<CONTEXT>(Process, Value.ContextRecord as u64)).map(AlignedContext);
    let mut Lookup = Symbols::New(Process);
    let _ = writeln!(Output, "Exception");
    let _ = writeln!(Output, "---------");
    let _ = writeln!(Output, "Faulting thread ID: {}", Block.ThreadId);
    match &Record {
        Some(Record) => {
            let Code = Record.ExceptionCode.0 as u32;
            let _ = writeln!(Output, "Exception code: 0x{Code:08X} ({})", ExceptionName(Code));
            let _ = writeln!(Output, "Faulting instruction: 0x{:016X} {}", Record.ExceptionAddress as u64, Lookup.Describe(Record.ExceptionAddress as u64));
            if Code == 0xC000_0005 && Record.NumberParameters >= 2 {
                let Kind = match Record.ExceptionInformation[0] {
                    0 => "read from",
                    1 => "write to",
                    8 => "execute (DEP) at",
                    _ => "access at",
                };
                let _ = writeln!(Output, "Description: attempted {Kind} address 0x{:016X}", Record.ExceptionInformation[1]);
            }
            if Record.NumberParameters > 0 {
                let Parameters: Vec<String> = Record.ExceptionInformation[..(Record.NumberParameters as usize).min(15)].iter().map(|Value| format!("0x{Value:X}")).collect();
                let _ = writeln!(Output, "Exception parameters: {}", Parameters.join(", "));
            }
        }
        None => {
            let _ = writeln!(Output, "Exception record could not be read from the crashed process");
        }
    }
    if Block.PanicLength > 0 {
        let Text = String::from_utf8_lossy(&Block.Panic[..(Block.PanicLength as usize).min(Block.Panic.len())]);
        let _ = writeln!(Output, "\nPanic details\n-------------\n{Text}");
    }
    if let Some(Context) = &Context {
        let _ = writeln!(Output, "\nRegisters (faulting thread)\n---------------------------");
        Registers(&Context.0, &mut Output);
        let _ = writeln!(Output, "\nStack trace (faulting thread {}; arguments are best-effort and may be unavailable in optimized code)\n------------------------------------------------------------------------------------------", Block.ThreadId);
        let Faulting = Threads.iter().find(|(Identifier, _)| *Identifier == Block.ThreadId).map(|(_, Handle)| Handle.0).unwrap_or_default();
        WalkStack(Process, Faulting, &Context.0, &mut Lookup, &mut Output);
    }
    let _ = writeln!(Output, "\nOther threads (best effort)\n---------------------------");
    for (Identifier, Handle) in Threads {
        if *Identifier == Block.ThreadId {
            continue;
        }
        let mut Captured = AlignedContext(CONTEXT::default());
        Captured.0.ContextFlags = CONTEXT_FULL_AMD64;
        let _ = writeln!(Output, "\nThread {Identifier}:");
        if unsafe { GetThreadContext(Handle.0, &mut Captured.0) }.is_err() {
            let _ = writeln!(Output, "    <context unavailable>");
            continue;
        }
        WalkStack(Process, Handle.0, &Captured.0, &mut Lookup, &mut Output);
    }
    let _ = writeln!(Output, "\nLoaded modules\n--------------");
    ModulesSection(Process, &mut Output);
    Output
}

pub fn Run(Arguments: &[String]) -> i32 {
    if Arguments.len() < 8 {
        return 2;
    }
    let Number = |Index: usize| Arguments[Index].parse::<usize>().unwrap_or(0);
    let ProcessId = Number(2) as u32;
    let Mapping = HANDLE(Number(3) as *mut _);
    let CrashEvent = HANDLE(Number(4) as *mut _);
    let DoneEvent = HANDLE(Number(5) as *mut _);
    let Full = Number(6) == 1;
    let Directory = PathBuf::from(&Arguments[7]);
    let Access = PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_DUP_HANDLE | PROCESS_SYNCHRONIZE;
    let Ok(Process) = (unsafe { OpenProcess(Access, false, ProcessId) }) else { return 3 };
    let View = unsafe { MapViewOfFile(Mapping, FILE_MAP_READ, 0, 0, std::mem::size_of::<SharedBlock>()) };
    if View.Value.is_null() {
        return 4;
    }
    let Block = unsafe { &*(View.Value as *const SharedBlock) };
    if Block.Magic != Magic {
        return 5;
    }
    let Signaled = unsafe { WaitForMultipleObjects(&[CrashEvent, Process], false, u32::MAX) };
    if Signaled != WAIT_OBJECT_0 {
        return 0;
    }
    let Stamp = Timestamp();
    let _ = std::fs::create_dir_all(&Directory);
    let DumpPath = ReportPath(&Directory, &Stamp, ProcessId, "dmp");
    let TextPath = ReportPath(&Directory, &Stamp, ProcessId, "txt");
    let Threads = SuspendOthers(ProcessId, Block.ThreadId);
    let DumpResult = WriteDump(Process, ProcessId, &DumpPath, Block.ThreadId, Block.ExceptionPointers, Full);
    let Text = Report(Process, ProcessId, Block, &Threads, &DumpResult, &DumpPath);
    let Written = std::fs::write(&TextPath, Text.replace('\n', "\r\n"));
    drop(Threads);
    unsafe {
        let _ = SetEvent(DoneEvent);
    }
    let Message = match Written {
        Ok(()) => format!("Flux stopped unexpectedly.\n\nA crash report was saved to:\n{}\n\nOpen the crash reports folder?", TextPath.display()),
        Err(Error) => format!("Flux stopped unexpectedly, and the crash report could not be written: {Error}"),
    };
    let Answer = unsafe { MessageBoxW(None, &HSTRING::from(Message), w!("Flux"), MB_YESNO | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND) };
    if Answer == IDYES {
        let Parameters = HSTRING::from(format!("/select,\"{}\"", TextPath.display()));
        unsafe {
            ShellExecuteW(None, w!("open"), w!("explorer.exe"), &Parameters, PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
    unsafe {
        let _ = CloseHandle(Process);
    }
    0
}
