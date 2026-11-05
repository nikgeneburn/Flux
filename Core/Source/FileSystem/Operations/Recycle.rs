use super::{Code, Engine, Exists, HresultCode, Phase, Request, Shared};
use crate::FileSystem::{ErrorText, InArchive, PathFromWide, SamePath, ShellPath};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{E_ABORT, E_NOINTERFACE, E_POINTER, HWND, MAX_PATH, S_OK};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Vtbl, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR};

const TooLong: &str = "The path is too long for the Recycle Bin; delete the item permanently instead";

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

#[repr(C)]
struct DeleteSink {
    Table: *const IFileOperationProgressSink_Vtbl,
    References: AtomicU32,
    State: Arc<Shared>,
    Requested: RefCell<Vec<PathBuf>>,
    Failures: RefCell<Vec<(PathBuf, String)>>,
}

static SinkTable: IFileOperationProgressSink_Vtbl = IFileOperationProgressSink_Vtbl {
    base__: IUnknown_Vtbl { QueryInterface: SinkQuery, AddRef: SinkAddRef, Release: SinkRelease },
    StartOperations: IgnoreEvent,
    FinishOperations: IgnoreFinish,
    PreRenameItem: IgnoreNamed,
    PostRenameItem: IgnorePostRename,
    PreMoveItem: IgnoreTransfer,
    PostMoveItem: IgnorePostTransfer,
    PreCopyItem: IgnoreTransfer,
    PostCopyItem: IgnorePostTransfer,
    PreDeleteItem: SinkPreDelete,
    PostDeleteItem: SinkPostDelete,
    PreNewItem: IgnoreNamed,
    PostNewItem: IgnorePostNew,
    UpdateProgress: SinkUpdate,
    ResetTimer: IgnoreEvent,
    PauseTimer: IgnoreEvent,
    ResumeTimer: IgnoreEvent,
};

unsafe fn SinkOf<'a>(This: *mut c_void) -> &'a DeleteSink {
    unsafe { &*(This as *const DeleteSink) }
}

unsafe extern "system" fn SinkQuery(This: *mut c_void, Identifier: *const GUID, Output: *mut *mut c_void) -> HRESULT {
    if Output.is_null() || Identifier.is_null() {
        return E_POINTER;
    }
    let Wanted = unsafe { *Identifier };
    if Wanted == IUnknown::IID || Wanted == IFileOperationProgressSink::IID {
        unsafe {
            SinkAddRef(This);
            *Output = This;
        }
        S_OK
    } else {
        unsafe { *Output = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn SinkAddRef(This: *mut c_void) -> u32 {
    unsafe { SinkOf(This) }.References.fetch_add(1, Ordering::AcqRel) + 1
}

unsafe extern "system" fn SinkRelease(This: *mut c_void) -> u32 {
    let Remaining = unsafe { SinkOf(This) }.References.fetch_sub(1, Ordering::AcqRel) - 1;
    if Remaining == 0 {
        drop(unsafe { Box::from_raw(This as *mut DeleteSink) });
    }
    Remaining
}

unsafe extern "system" fn IgnoreEvent(_: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnoreFinish(_: *mut c_void, _: HRESULT) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnoreNamed(_: *mut c_void, _: u32, _: *mut c_void, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnorePostRename(_: *mut c_void, _: u32, _: *mut c_void, _: PCWSTR, _: HRESULT, _: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnoreTransfer(_: *mut c_void, _: u32, _: *mut c_void, _: *mut c_void, _: PCWSTR) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnorePostTransfer(_: *mut c_void, _: u32, _: *mut c_void, _: *mut c_void, _: PCWSTR, _: HRESULT, _: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn IgnorePostNew(_: *mut c_void, _: u32, _: *mut c_void, _: PCWSTR, _: PCWSTR, _: u32, _: HRESULT, _: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn SinkPreDelete(This: *mut c_void, _: u32, Item: *mut c_void) -> HRESULT {
    let Sink = unsafe { SinkOf(This) };
    if Sink.State.IsCancelled() {
        return E_ABORT;
    }
    if let Some(Found) = unsafe { Sink.Matching(Item) } {
        Sink.State.SetCurrent(&Found);
        Sink.State.Report();
    }
    S_OK
}

unsafe extern "system" fn SinkPostDelete(This: *mut c_void, _: u32, Item: *mut c_void, Outcome: HRESULT, _: *mut c_void) -> HRESULT {
    let Sink = unsafe { SinkOf(This) };
    if let Some(Found) = unsafe { Sink.Matching(Item) } {
        Sink.State.AddItems(1);
        if Outcome.is_err() {
            Sink.Failures.borrow_mut().push((Found, ErrorText(HresultCode(Outcome))));
        }
        Sink.State.Report();
    }
    S_OK
}

unsafe extern "system" fn SinkUpdate(This: *mut c_void, _: u32, _: u32) -> HRESULT {
    let Sink = unsafe { SinkOf(This) };
    if Sink.State.IsCancelled() {
        return E_ABORT;
    }
    Sink.State.Report();
    S_OK
}

impl DeleteSink {
    unsafe fn Matching(&self, Item: *mut c_void) -> Option<PathBuf> {
        let Shell = unsafe { IShellItem::from_raw_borrowed(&Item) }?;
        let Name = unsafe { Shell.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
        let Found = PathFromWide(unsafe { Name.as_wide() });
        unsafe { CoTaskMemFree(Some(Name.0 as *const c_void)) };
        self.Requested.borrow().iter().find(|Candidate| SamePath(Candidate, &Found)).cloned()
    }
}

pub(super) fn Run(Worker: &mut Engine, Items: &[PathBuf], Owner: isize) {
    let State = Worker.State.clone();
    State.AddTotals(Items.len() as u64, 0);
    State.SetPhase(Phase::Working);
    State.Report();
    let Initialized = unsafe { CoInitializeEx(None, COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0)) };
    let _Apartment = Initialized.is_ok().then_some(Apartment);
    let Shell: IFileOperation = match unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) } {
        Ok(Value) => Value,
        Err(Error) => {
            for Item in Items {
                Worker.Fail(Item, Code(&Error));
            }
            return;
        }
    };
    let Flags = FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0 | FOF_NOCONFIRMATION.0 | FOF_WANTNUKEWARNING.0 | FOF_SILENT.0 | FOF_NOERRORUI.0;
    if let Err(Error) = unsafe { Shell.SetOperationFlags(FILEOPERATION_FLAGS(Flags)) } {
        for Item in Items {
            Worker.Fail(Item, Code(&Error));
        }
        return;
    }
    if Owner != 0 {
        let _ = unsafe { Shell.SetOwnerWindow(HWND(Owner as *mut c_void)) };
    }
    let Raw = Box::into_raw(Box::new(DeleteSink {
        Table: &SinkTable,
        References: AtomicU32::new(1),
        State: State.clone(),
        Requested: RefCell::new(Vec::new()),
        Failures: RefCell::new(Vec::new()),
    }));
    let Sink = unsafe { IFileOperationProgressSink::from_raw(Raw as *mut c_void) };
    let Data = unsafe { &*Raw };
    let Cookie = unsafe { Shell.Advise(&Sink) }.ok();
    for Item in Items {
        if State.IsCancelled() {
            break;
        }
        let Wide = ShellPath(Item);
        if Wide.len() > MAX_PATH as usize {
            Worker.FailText(Item, TooLong);
            continue;
        }
        let Queued =
            unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(Wide.as_ptr()), None) }.and_then(|Target| unsafe { Shell.DeleteItem(&Target, None) });
        match Queued {
            Ok(()) => Data.Requested.borrow_mut().push(Item.clone()),
            Err(Error) => Worker.Fail(Item, Code(&Error)),
        }
    }
    let Accepted = Data.Requested.borrow().clone();
    let Performed = if Accepted.is_empty() || State.IsCancelled() { Ok(()) } else { unsafe { Shell.PerformOperations() } };
    if let Some(Value) = Cookie {
        let _ = unsafe { Shell.Unadvise(Value) };
    }
    if !Accepted.is_empty() && unsafe { Shell.GetAnyOperationsAborted() }.is_ok_and(|Value| Value.as_bool()) {
        State.Cancelled.store(true, Ordering::Release);
    }
    let Failures = Data.Failures.take();
    drop(Sink);
    for Item in Accepted {
        if !Exists(&Item) {
            Worker.Result.Removed.push(Item);
            continue;
        }
        if let Some((_, Message)) = Failures.iter().find(|(Failed, _)| SamePath(Failed, &Item)) {
            Worker.Result.Errors.push((Item, Message.clone()));
        } else if !State.IsCancelled() {
            match &Performed {
                Err(Error) => Worker.Fail(&Item, Code(Error)),
                Ok(()) => Worker.FailText(&Item, "The item was not moved to the Recycle Bin"),
            }
        }
    }
}

fn ShellItemOf(Item: &Path) -> windows::core::Result<IShellItem> {
    let Wide = ShellPath(Item);
    unsafe { SHCreateItemFromParsingName(PCWSTR(Wide.as_ptr()), None) }
}

fn NameOf(Item: &Path) -> Vec<u16> {
    Item.file_name().map(|Name| Name.encode_wide().chain(std::iter::once(0)).collect()).unwrap_or_else(|| vec![0])
}

pub(super) fn Touches(Work: &Request) -> bool {
    let Inside = |Item: &Path| InArchive(Item);
    match Work {
        Request::CopyInto { Sources, .. } | Request::MoveInto { Sources, .. } => Sources.iter().any(|Item| Inside(Item)),
        Request::MovePairs(Pairs) => Pairs.iter().any(|(From, To)| Inside(From) || Inside(To)),
        Request::Recycle(Items) | Request::DeletePermanently(Items) => Items.iter().any(|Item| Inside(Item)),
        Request::CreateFolder(Item) | Request::CreateFile(Item) => Inside(Item),
    }
}

pub(super) fn Archive(Worker: &mut Engine, Work: &Request, Owner: isize) {
    let State = Worker.State.clone();
    State.SetPhase(Phase::Working);
    State.Report();
    let Initialized = unsafe { CoInitializeEx(None, COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0)) };
    let _Apartment = Initialized.is_ok().then_some(Apartment);
    let Affected: Vec<PathBuf> = match Work {
        Request::CopyInto { Sources, .. } | Request::MoveInto { Sources, .. } => Sources.clone(),
        Request::MovePairs(Pairs) => Pairs.iter().map(|(From, _)| From.clone()).collect(),
        Request::Recycle(Items) | Request::DeletePermanently(Items) => Items.clone(),
        Request::CreateFolder(Item) | Request::CreateFile(Item) => vec![Item.clone()],
    };
    State.AddTotals(Affected.len() as u64, 0);
    let Shell: IFileOperation = match unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) } {
        Ok(Value) => Value,
        Err(Error) => {
            for Item in &Affected {
                Worker.Fail(Item, Code(&Error));
            }
            return;
        }
    };
    let Recycling = matches!(Work, Request::Recycle(_));
    let Flags = FOF_ALLOWUNDO.0 | FOF_NOCONFIRMMKDIR.0 | if Recycling { FOFX_RECYCLEONDELETE.0 | FOF_WANTNUKEWARNING.0 } else { 0 };
    if let Err(Error) = unsafe { Shell.SetOperationFlags(FILEOPERATION_FLAGS(Flags)) } {
        for Item in &Affected {
            Worker.Fail(Item, Code(&Error));
        }
        return;
    }
    if Owner != 0 {
        let _ = unsafe { Shell.SetOwnerWindow(HWND(Owner as *mut c_void)) };
    }
    let mut Queued = 0usize;
    let mut Queue = |Item: &Path, Result: windows::core::Result<()>, Worker: &mut Engine| match Result {
        Ok(()) => Queued += 1,
        Err(Error) => Worker.Fail(Item, Code(&Error)),
    };
    match Work {
        Request::CopyInto { Sources, Destination } | Request::MoveInto { Sources, Destination } => {
            let Moving = matches!(Work, Request::MoveInto { .. });
            match ShellItemOf(Destination) {
                Ok(Folder) => {
                    for Source in Sources {
                        let Result = ShellItemOf(Source).and_then(|Item| unsafe {
                            if Moving { Shell.MoveItem(&Item, &Folder, PCWSTR::null(), None) } else { Shell.CopyItem(&Item, &Folder, PCWSTR::null(), None) }
                        });
                        Queue(Source, Result, Worker);
                    }
                }
                Err(Error) => {
                    for Source in Sources {
                        Worker.Fail(Source, Code(&Error));
                    }
                }
            }
        }
        Request::MovePairs(Pairs) => {
            for (From, To) in Pairs {
                let Name = NameOf(To);
                let Result = ShellItemOf(From).and_then(|Item| unsafe {
                    match (From.parent(), To.parent()) {
                        (Some(Before), Some(After)) if SamePath(Before, After) => Shell.RenameItem(&Item, PCWSTR(Name.as_ptr()), None),
                        (_, Some(After)) => ShellItemOf(After).and_then(|Folder| Shell.MoveItem(&Item, &Folder, PCWSTR(Name.as_ptr()), None)),
                        _ => Err(windows::core::Error::from(E_POINTER)),
                    }
                });
                Queue(From, Result, Worker);
            }
        }
        Request::Recycle(Items) | Request::DeletePermanently(Items) => {
            for Item in Items {
                let Result = ShellItemOf(Item).and_then(|Target| unsafe { Shell.DeleteItem(&Target, None) });
                Queue(Item, Result, Worker);
            }
        }
        Request::CreateFolder(Item) | Request::CreateFile(Item) => {
            let Attributes = if matches!(Work, Request::CreateFolder(_)) { 0x10 } else { 0x80 };
            let Name = NameOf(Item);
            let Result = Item
                .parent()
                .ok_or_else(|| windows::core::Error::from(E_POINTER))
                .and_then(ShellItemOf)
                .and_then(|Folder| unsafe { Shell.NewItem(&Folder, Attributes, PCWSTR(Name.as_ptr()), PCWSTR::null(), None) });
            Queue(Item, Result, Worker);
        }
    }
    if Queued == 0 || State.IsCancelled() {
        return;
    }
    if let Err(Error) = unsafe { Shell.PerformOperations() } {
        if Error.code() != E_ABORT {
            Worker.Result.Errors.push((Affected.first().cloned().unwrap_or_default(), ErrorText(Code(&Error))));
        }
    }
    if unsafe { Shell.GetAnyOperationsAborted() }.is_ok_and(|Value| Value.as_bool()) {
        State.Cancelled.store(true, Ordering::Release);
    }
    if let Request::CreateFolder(Item) | Request::CreateFile(Item) = Work {
        Worker.Result.Created.push(Item.clone());
    }
}
