use super::Browser_Impl;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, SHANDLE_PTR, VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::System::Ole::{OLECMDEXECOPT, OLECMDF, OLECMDID, READYSTATE, READYSTATE_COMPLETE};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{IWebBrowser_Impl, IWebBrowser2, IWebBrowser2_Impl, IWebBrowserApp_Impl};
use windows::core::{BSTR, IUnknownImpl};

impl IWebBrowser_Impl for Browser_Impl {
    fn GoBack(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GoForward(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GoHome(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GoSearch(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Navigate(&self, _: &BSTR, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Refresh(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Refresh2(&self, _: *const VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Stop(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Application(&self) -> windows::core::Result<IDispatch> {
        Err(E_NOTIMPL.into())
    }

    fn Parent(&self) -> windows::core::Result<IDispatch> {
        Err(E_NOTIMPL.into())
    }

    fn Container(&self) -> windows::core::Result<IDispatch> {
        Err(E_NOTIMPL.into())
    }

    fn Document(&self) -> windows::core::Result<IDispatch> {
        Ok(self.to_interface::<IWebBrowser2>().into())
    }

    fn TopLevelContainer(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }

    fn Type(&self) -> windows::core::Result<BSTR> {
        Err(E_NOTIMPL.into())
    }

    fn Left(&self) -> windows::core::Result<i32> {
        Err(E_NOTIMPL.into())
    }

    fn SetLeft(&self, _: i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Top(&self) -> windows::core::Result<i32> {
        Err(E_NOTIMPL.into())
    }

    fn SetTop(&self, _: i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Width(&self) -> windows::core::Result<i32> {
        Err(E_NOTIMPL.into())
    }

    fn SetWidth(&self, _: i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Height(&self) -> windows::core::Result<i32> {
        Err(E_NOTIMPL.into())
    }

    fn SetHeight(&self, _: i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn LocationName(&self) -> windows::core::Result<BSTR> {
        self.Folder.borrow().as_deref().and_then(Path::file_name).map(|Name| BSTR::from_wide(&Name.encode_wide().collect::<Vec<u16>>())).ok_or_else(|| E_FAIL.into())
    }

    fn LocationURL(&self) -> windows::core::Result<BSTR> {
        self.Folder.borrow().as_deref().map(|Folder| BSTR::from_wide(&Folder.as_os_str().encode_wide().collect::<Vec<u16>>())).ok_or_else(|| E_FAIL.into())
    }

    fn Busy(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for Browser_Impl {
    fn Quit(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ClientToWindow(&self, _: *mut i32, _: *mut i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn PutProperty(&self, _: &BSTR, _: &VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetProperty(&self, _: &BSTR) -> windows::core::Result<VARIANT> {
        Err(E_NOTIMPL.into())
    }

    fn Name(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("Flux"))
    }

    fn HWND(&self) -> windows::core::Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.Window.0 as isize))
    }

    fn FullName(&self) -> windows::core::Result<BSTR> {
        std::env::current_exe().map(|Executable| BSTR::from_wide(&Executable.as_os_str().encode_wide().collect::<Vec<u16>>())).map_err(|_| E_FAIL.into())
    }

    fn Path(&self) -> windows::core::Result<BSTR> {
        Err(E_NOTIMPL.into())
    }

    fn Visible(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }

    fn SetVisible(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn StatusBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetStatusBar(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn StatusText(&self) -> windows::core::Result<BSTR> {
        Err(E_NOTIMPL.into())
    }

    fn SetStatusText(&self, _: &BSTR) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ToolBar(&self) -> windows::core::Result<i32> {
        Err(E_NOTIMPL.into())
    }

    fn SetToolBar(&self, _: i32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn MenuBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetMenuBar(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn FullScreen(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetFullScreen(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IWebBrowser2_Impl for Browser_Impl {
    fn Navigate2(&self, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryStatusWB(&self, _: OLECMDID) -> windows::core::Result<OLECMDF> {
        Err(E_NOTIMPL.into())
    }

    fn ExecWB(&self, _: OLECMDID, _: OLECMDEXECOPT, _: *const VARIANT, _: *mut VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ShowBrowserBar(&self, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn ReadyState(&self) -> windows::core::Result<READYSTATE> {
        Ok(READYSTATE_COMPLETE)
    }

    fn Offline(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetOffline(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Silent(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetSilent(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn RegisterAsBrowser(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetRegisterAsBrowser(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn RegisterAsDropTarget(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetRegisterAsDropTarget(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn TheaterMode(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetTheaterMode(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn AddressBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetAddressBar(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Resizable(&self) -> windows::core::Result<VARIANT_BOOL> {
        Err(E_NOTIMPL.into())
    }

    fn SetResizable(&self, _: VARIANT_BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}
