#![allow(non_snake_case, non_upper_case_globals)]

pub mod Utilities;

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const AbiVersion: u32 = 1;
pub const InitializeSymbol: &[u8] = b"FluxModuleInitialize\0";

pub const StatusOk: i32 = 0;
pub const StatusFailed: i32 = 1;
pub const StatusCancelled: i32 = 2;
pub const StatusUnsupported: i32 = 3;
pub const StatusIncompatible: i32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WideText {
    pub Data: *const u16,
    pub Length: usize,
}

impl WideText {
    pub fn FromSlice(Slice: &[u16]) -> Self {
        Self { Data: Slice.as_ptr(), Length: Slice.len() }
    }

    pub fn Empty() -> Self {
        Self { Data: std::ptr::null(), Length: 0 }
    }

    pub unsafe fn AsSlice<'a>(&self) -> &'a [u16] {
        if self.Data.is_null() || self.Length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.Data, self.Length) }
        }
    }

    pub unsafe fn ToStringLossy(&self) -> String {
        String::from_utf16_lossy(unsafe { self.AsSlice() })
    }

    pub unsafe fn ToPathBuf(&self) -> PathBuf {
        PathBuf::from(OsString::from_wide(unsafe { self.AsSlice() }))
    }
}

pub type InitializeFunction = unsafe extern "C" fn(HostAbiVersion: u32, Api: *mut ModuleApi) -> i32;

#[repr(C)]
pub struct HostApi {
    pub AbiVersion: u32,
    pub Size: u32,
    pub Context: *mut c_void,
    pub EmitText: unsafe extern "C" fn(Context: *mut c_void, Text: WideText),
    pub EmitPath: unsafe extern "C" fn(Context: *mut c_void, Path: WideText),
    pub EmitRename: unsafe extern "C" fn(Context: *mut c_void, From: WideText, To: WideText),
    pub SetProgress: unsafe extern "C" fn(Context: *mut c_void, Done: u64, Total: u64),
    pub IsCancelled: unsafe extern "C" fn(Context: *mut c_void) -> u32,
}

#[repr(C)]
pub struct InvocationRequest {
    pub Size: u32,
    pub Command: WideText,
    pub Directory: WideText,
    pub OtherDirectory: WideText,
    pub Argument: WideText,
    pub Selection: *const WideText,
    pub SelectionCount: usize,
}

#[repr(C)]
pub struct PreviewRequest {
    pub Size: u32,
    pub Path: WideText,
    pub MaxBytes: u64,
}

#[repr(C)]
pub struct ModuleApi {
    pub AbiVersion: u32,
    pub Size: u32,
    pub Invoke: Option<unsafe extern "C" fn(Host: *const HostApi, Request: *const InvocationRequest) -> i32>,
    pub Preview: Option<unsafe extern "C" fn(Host: *const HostApi, Request: *const PreviewRequest) -> i32>,
    pub Shutdown: Option<unsafe extern "C" fn()>,
}

pub enum Outcome {
    Completed,
    Cancelled,
    Failed(String),
    Unsupported,
}

pub struct Host {
    Api: *const HostApi,
}

impl Host {
    fn Api(&self) -> &HostApi {
        unsafe { &*self.Api }
    }

    pub fn Text(&self, Text: &str) {
        let Wide: Vec<u16> = Text.encode_utf16().collect();
        unsafe { (self.Api().EmitText)(self.Api().Context, WideText::FromSlice(&Wide)) }
    }

    pub fn Line(&self, Text: &str) {
        let mut Wide: Vec<u16> = Text.encode_utf16().collect();
        Wide.push(b'\n' as u16);
        unsafe { (self.Api().EmitText)(self.Api().Context, WideText::FromSlice(&Wide)) }
    }

    pub fn Path(&self, Item: &Path) {
        let Wide: Vec<u16> = Item.as_os_str().encode_wide().collect();
        unsafe { (self.Api().EmitPath)(self.Api().Context, WideText::FromSlice(&Wide)) }
    }

    pub fn Rename(&self, From: &Path, To: &Path) {
        let FromWide: Vec<u16> = From.as_os_str().encode_wide().collect();
        let ToWide: Vec<u16> = To.as_os_str().encode_wide().collect();
        unsafe { (self.Api().EmitRename)(self.Api().Context, WideText::FromSlice(&FromWide), WideText::FromSlice(&ToWide)) }
    }

    pub fn Progress(&self, Done: u64, Total: u64) {
        unsafe { (self.Api().SetProgress)(self.Api().Context, Done, Total) }
    }

    pub fn Cancelled(&self) -> bool {
        unsafe { (self.Api().IsCancelled)(self.Api().Context) != 0 }
    }
}

pub struct Invocation {
    pub Command: String,
    pub Directory: PathBuf,
    pub OtherDirectory: Option<PathBuf>,
    pub Argument: String,
    pub Selection: Vec<PathBuf>,
}

impl Invocation {
    pub fn TargetsOrDirectory(&self) -> Vec<PathBuf> {
        if self.Selection.is_empty() { vec![self.Directory.clone()] } else { self.Selection.clone() }
    }
}

pub struct PreviewInput {
    pub Path: PathBuf,
    pub MaxBytes: u64,
}

pub trait Module: Sync + Send + 'static {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome;

    fn Preview(&self, _Host: &Host, _Request: &PreviewInput) -> Outcome {
        Outcome::Unsupported
    }

    fn Shutdown(&self) {}
}

static Registered: OnceLock<&'static dyn Module> = OnceLock::new();

fn OutcomeStatus(Host: &Host, Result: std::thread::Result<Outcome>) -> i32 {
    match Result {
        Ok(Outcome::Completed) => StatusOk,
        Ok(Outcome::Cancelled) => StatusCancelled,
        Ok(Outcome::Unsupported) => StatusUnsupported,
        Ok(Outcome::Failed(Message)) => {
            Host.Line(&Message);
            StatusFailed
        }
        Err(Payload) => {
            let Message = Payload
                .downcast_ref::<&str>()
                .map(|Text| Text.to_string())
                .or_else(|| Payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Host.Line(&format!("Module failure: {Message}"));
            StatusFailed
        }
    }
}

unsafe extern "C" fn InvokeTrampoline(HostPointer: *const HostApi, RequestPointer: *const InvocationRequest) -> i32 {
    let Some(Instance) = Registered.get() else { return StatusFailed };
    if HostPointer.is_null() || RequestPointer.is_null() {
        return StatusFailed;
    }
    let HostWrapper = Host { Api: HostPointer };
    let Request = unsafe { &*RequestPointer };
    let Selection = if Request.Selection.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(Request.Selection, Request.SelectionCount) }
            .iter()
            .map(|Item| unsafe { Item.ToPathBuf() })
            .collect()
    };
    let OtherDirectory = unsafe { Request.OtherDirectory.ToPathBuf() };
    let Data = Invocation {
        Command: unsafe { Request.Command.ToStringLossy() },
        Directory: unsafe { Request.Directory.ToPathBuf() },
        OtherDirectory: if OtherDirectory.as_os_str().is_empty() { None } else { Some(OtherDirectory) },
        Argument: unsafe { Request.Argument.ToStringLossy() },
        Selection,
    };
    let Result = catch_unwind(AssertUnwindSafe(|| Instance.Invoke(&HostWrapper, &Data)));
    OutcomeStatus(&HostWrapper, Result)
}

unsafe extern "C" fn PreviewTrampoline(HostPointer: *const HostApi, RequestPointer: *const PreviewRequest) -> i32 {
    let Some(Instance) = Registered.get() else { return StatusFailed };
    if HostPointer.is_null() || RequestPointer.is_null() {
        return StatusFailed;
    }
    let HostWrapper = Host { Api: HostPointer };
    let Request = unsafe { &*RequestPointer };
    let Data = PreviewInput { Path: unsafe { Request.Path.ToPathBuf() }, MaxBytes: Request.MaxBytes };
    let Result = catch_unwind(AssertUnwindSafe(|| Instance.Preview(&HostWrapper, &Data)));
    OutcomeStatus(&HostWrapper, Result)
}

unsafe extern "C" fn ShutdownTrampoline() {
    if let Some(Instance) = Registered.get() {
        let _ = catch_unwind(AssertUnwindSafe(|| Instance.Shutdown()));
    }
}

pub unsafe fn Initialize(HostAbiVersion: u32, Api: *mut ModuleApi, Instance: &'static dyn Module) -> i32 {
    if HostAbiVersion != AbiVersion || Api.is_null() {
        return StatusIncompatible;
    }
    let _ = Registered.set(Instance);
    unsafe {
        Api.write(ModuleApi {
            AbiVersion,
            Size: std::mem::size_of::<ModuleApi>() as u32,
            Invoke: Some(InvokeTrampoline),
            Preview: Some(PreviewTrampoline),
            Shutdown: Some(ShutdownTrampoline),
        });
    }
    StatusOk
}

#[macro_export]
macro_rules! ExportModule {
    ($Instance:path) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn FluxModuleInitialize(HostAbiVersion: u32, Api: *mut $crate::ModuleApi) -> i32 {
            unsafe { $crate::Initialize(HostAbiVersion, Api, &$Instance) }
        }
    };
}
