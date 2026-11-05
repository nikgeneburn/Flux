mod Automation;

use super::Shell::{OwnedPidl, ParsePidl};
use crate::Application::Event;
use crate::Services::Workers::EventSink;
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, E_POINTER, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CoCreateInstance, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream, ITypeInfo,
};
use windows::Win32::System::Ole::{IOleWindow_Impl, OLEMENUGROUPWIDTHS};
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERSETTINGS, GPFIDL_DEFAULT, ILCombine, ILFree, ILGetSize, IShellBrowser, IShellBrowser_Impl, IShellView, IShellView_Impl, IShellWindows, IFolderView, IWebBrowser2, SHGetPathFromIDListEx, SVSI_DESELECTOTHERS,
    SID_STopLevelBrowser, SWC_BROWSER, ShellWindows, _SVGIO,
};
use windows::Win32::UI::WindowsAndMessaging::{HMENU, MSG};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, implement};

#[implement(IWebBrowser2, IServiceProvider, IShellBrowser, IShellView)]
struct Browser {
    Window: HWND,
    Sink: EventSink,
    Folder: Rc<RefCell<Option<PathBuf>>>,
}

struct ShellWindow {
    Windows: IShellWindows,
    Window: HWND,
    Cookie: std::cell::Cell<i32>,
    Folder: Rc<RefCell<Option<PathBuf>>>,
    Object: IDispatch,
}

thread_local! {
    static Registered: RefCell<Option<ShellWindow>> = const { RefCell::new(None) };
    static Reporting: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static Missed: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Report {
    Done,
    Later,
    Busy,
}

pub fn Register(Window: HWND, Sink: EventSink) -> bool {
    let Some(Windows) = (unsafe { CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL) }).ok() else { return false };
    let Folder = Rc::new(RefCell::new(None));
    let Automation: IWebBrowser2 = Browser { Window, Sink, Folder: Folder.clone() }.into();
    let Object: IDispatch = Automation.into();
    let Ok(Cookie) = (unsafe { Windows.Register(&Object, Window.0 as isize as i32, SWC_BROWSER) }) else { return false };
    Registered.with(|Slot| *Slot.borrow_mut() = Some(ShellWindow { Windows, Window, Cookie: std::cell::Cell::new(Cookie), Folder, Object }));
    true
}

pub fn Unregister() {
    let Previous = Registered.with(|Slot| Slot.borrow_mut().take());
    if let Some(Shell) = Previous {
        let _ = unsafe { Shell.Windows.Revoke(Shell.Cookie.get()) };
    }
}

pub fn Navigate(Folder: Option<&Path>, Fresh: bool) -> Report {
    if Reporting.get() {
        Missed.set(true);
        return Report::Busy;
    }
    if unsafe { windows::Win32::UI::WindowsAndMessaging::InSendMessage() }.as_bool() {
        return Report::Later;
    }
    Reporting.set(true);
    Missed.set(false);
    Send(Folder, Fresh);
    Reporting.set(false);
    if Missed.replace(false) { Report::Later } else { Report::Done }
}

fn Send(Folder: Option<&Path>, Fresh: bool) {
    let Target = Registered.with(|Slot| {
        let Slot = Slot.borrow();
        let Shell = Slot.as_ref()?;
        (Fresh || Shell.Folder.borrow().as_deref() != Folder).then(|| (Shell.Windows.clone(), Shell.Window, Shell.Cookie.get(), Shell.Folder.clone(), Shell.Object.clone()))
    });
    let Some((Windows, Window, Cookie, Reported, Object)) = Target else { return };
    *Reported.borrow_mut() = Folder.map(Path::to_path_buf);
    let Some(Pidl) = Folder.and_then(ParsePidl) else { return };
    let Size = unsafe { ILGetSize(Some(Pidl.0)) };
    let Ok(Location) = (unsafe { InitVariantFromBuffer(Pidl.0 as *const c_void, Size) }) else { return };
    let mut Current = Cookie;
    if Fresh {
        let Root = VARIANT::default();
        let Thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() } as i32;
        let _ = unsafe { Windows.Revoke(Cookie) };
        let Pending = unsafe { Windows.RegisterPending(Thread, &Location, &Root, SWC_BROWSER) };
        if let Ok(Renewed) = unsafe { Windows.Register(&Object, Window.0 as isize as i32, SWC_BROWSER) } {
            Current = Renewed;
            if let Ok(Stale) = Pending {
                if Stale != Renewed {
                    let _ = unsafe { Windows.Revoke(Stale) };
                }
            }
        }
        Registered.with(|Slot| {
            if let Some(Shell) = Slot.borrow().as_ref() {
                Shell.Cookie.set(Current);
            }
        });
    }
    let _ = unsafe { Windows.OnNavigate(Current, &Location as *const VARIANT) };
}

fn ChildPath(Folder: &Path, Child: *const ITEMIDLIST) -> Option<PathBuf> {
    let Parent: OwnedPidl = ParsePidl(Folder)?;
    let Full = unsafe { ILCombine(Some(Parent.0), Some(Child)) };
    if Full.is_null() {
        return None;
    }
    let mut Buffer = vec![0u16; 32768];
    let Found = unsafe { SHGetPathFromIDListEx(Full, &mut Buffer, GPFIDL_DEFAULT) }.as_bool();
    unsafe { ILFree(Some(Full)) };
    Found.then(|| crate::FileSystem::PathFromWide(&Buffer))
}

impl IDispatch_Impl for Browser_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _: u32, _: u32) -> windows::core::Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(&self, _: *const GUID, _: *const PCWSTR, _: u32, _: u32, _: *mut i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(&self, _: i32, _: *const GUID, _: u32, _: DISPATCH_FLAGS, _: *const DISPPARAMS, _: *mut VARIANT, _: *mut EXCEPINFO, _: *mut u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IServiceProvider_Impl for Browser_Impl {
    fn QueryService(&self, Service: *const GUID, Wanted: *const GUID, Output: *mut *mut c_void) -> windows::core::Result<()> {
        if Service.is_null() || Wanted.is_null() || Output.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *Output = std::ptr::null_mut() };
        let Known = unsafe { *Service == SID_STopLevelBrowser || *Service == IShellBrowser::IID || *Service == IFolderView::IID };
        if !Known {
            return Err(E_NOINTERFACE.into());
        }
        let Unknown: IUnknown = self.to_interface();
        unsafe { Unknown.query(Wanted, Output) }.ok()
    }
}

impl IOleWindow_Impl for Browser_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        Ok(self.Window)
    }

    fn ContextSensitiveHelp(&self, _: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IShellBrowser_Impl for Browser_Impl {
    fn InsertMenusSB(&self, _: HMENU, _: *mut OLEMENUGROUPWIDTHS) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn SetMenuSB(&self, _: HMENU, _: isize, _: HWND) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn RemoveMenusSB(&self, _: HMENU) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn SetStatusTextSB(&self, _: &PCWSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnableModelessSB(&self, _: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn TranslateAcceleratorSB(&self, _: *const MSG, _: u16) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn BrowseObject(&self, _: *const ITEMIDLIST, _: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetViewStateStream(&self, _: u32) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }

    fn GetControlWindow(&self, _: u32) -> windows::core::Result<HWND> {
        Err(E_NOTIMPL.into())
    }

    fn SendControlMsg(&self, _: u32, _: u32, _: WPARAM, _: LPARAM, _: *mut LRESULT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryActiveShellView(&self) -> windows::core::Result<IShellView> {
        Ok(self.to_interface())
    }

    fn OnViewWindowActive(&self, _: Ref<IShellView>) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetToolbarItems(&self, _: *const TBBUTTON, _: u32, _: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IShellView_Impl for Browser_Impl {
    fn TranslateAccelerator(&self, _: *const MSG) -> HRESULT {
        windows::Win32::Foundation::S_FALSE
    }

    fn EnableModeless(&self, _: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn UIActivate(&self, _: u32) -> windows::core::Result<()> {
        Ok(())
    }

    fn Refresh(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn CreateViewWindow(&self, _: Ref<IShellView>, _: *const FOLDERSETTINGS, _: Ref<IShellBrowser>, _: *const RECT) -> windows::core::Result<HWND> {
        Err(E_NOTIMPL.into())
    }

    fn DestroyViewWindow(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetCurrentInfo(&self) -> windows::core::Result<FOLDERSETTINGS> {
        Err(E_NOTIMPL.into())
    }

    fn AddPropertySheetPages(&self, _: u32, _: LPFNSVADDPROPSHEETPAGE, _: LPARAM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn SaveViewState(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn SelectItem(&self, Child: *const ITEMIDLIST, Flags: u32) -> windows::core::Result<()> {
        if Child.is_null() {
            return Err(E_POINTER.into());
        }
        let Folder = self.Folder.borrow().clone().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        let Item = ChildPath(&Folder, Child).ok_or(windows::core::Error::from(E_NOINTERFACE))?;
        self.Sink.Send(Event::Reveal(Item, Flags & SVSI_DESELECTOTHERS.0 as u32 == 0));
        Ok(())
    }

    fn GetItemObject(&self, _: &_SVGIO, _: *const GUID, _: *mut *mut c_void) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}
