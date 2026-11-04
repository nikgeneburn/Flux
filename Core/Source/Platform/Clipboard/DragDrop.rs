use super::FilesFromDrop;
use std::cell::RefCell;
use std::path::PathBuf;
use windows::Win32::Foundation::{GlobalFree, HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, HDROP, IDropTargetHelper, SHDoDragDrop};
use windows::core::{Ref, implement};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropPhase {
    Enter,
    Over,
    Leave,
    Drop,
}

pub struct DropRequest<'a> {
    pub Phase: DropPhase,
    pub X: i32,
    pub Y: i32,
    pub Keys: u32,
    pub Allowed: u32,
    pub Items: &'a [PathBuf],
}

#[implement(IDropTarget)]
struct DropTarget {
    Window: HWND,
    Helper: Option<IDropTargetHelper>,
    Items: RefCell<Vec<PathBuf>>,
}

fn ReadItems(Object: &IDataObject) -> Vec<PathBuf> {
    let Format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let Ok(mut Medium) = (unsafe { Object.GetData(&Format) }) else { return Vec::new() };
    let Memory = unsafe { Medium.u.hGlobal };
    let Pointer = unsafe { GlobalLock(Memory) };
    let Items = if Pointer.is_null() { Vec::new() } else { FilesFromDrop(HDROP(Pointer)) };
    unsafe {
        if !Pointer.is_null() {
            let _ = GlobalUnlock(Memory);
        }
        ReleaseStgMedium(&mut Medium);
    }
    Items
}

impl DropTarget {
    fn Ask(&self, Phase: DropPhase, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT) {
        let mut Client = POINT { x: Point.x, y: Point.y };
        unsafe {
            let _ = ScreenToClient(self.Window, &mut Client);
        }
        let Allowed = if Effect.is_null() { 0 } else { unsafe { (*Effect).0 } };
        let Items = self.Items.borrow();
        let Request = DropRequest { Phase, X: Client.x, Y: Client.y, Keys: Keys.0, Allowed, Items: &Items };
        let Chosen = if Items.is_empty() { 0 } else { crate::Application::DropFeedback(&Request) };
        if !Effect.is_null() {
            unsafe { *Effect = DROPEFFECT(Chosen & Allowed) };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, Object: Ref<IDataObject>, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        *self.Items.borrow_mut() = Object.ok().map(ReadItems).unwrap_or_default();
        self.Ask(DropPhase::Enter, Keys, Point, Effect);
        if let (Some(Helper), Ok(Data)) = (&self.Helper, Object.ok()) {
            let Screen = POINT { x: Point.x, y: Point.y };
            unsafe {
                let _ = Helper.DragEnter(self.Window, Data, &Screen, if Effect.is_null() { DROPEFFECT_NONE } else { *Effect });
            }
        }
        Ok(())
    }

    fn DragOver(&self, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        self.Ask(DropPhase::Over, Keys, Point, Effect);
        if let Some(Helper) = &self.Helper {
            let Screen = POINT { x: Point.x, y: Point.y };
            unsafe {
                let _ = Helper.DragOver(&Screen, if Effect.is_null() { DROPEFFECT_NONE } else { *Effect });
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        let Items = std::mem::take(&mut *self.Items.borrow_mut());
        let Request = DropRequest { Phase: DropPhase::Leave, X: 0, Y: 0, Keys: 0, Allowed: 0, Items: &Items };
        crate::Application::DropFeedback(&Request);
        if let Some(Helper) = &self.Helper {
            unsafe {
                let _ = Helper.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(&self, Object: Ref<IDataObject>, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        if let Ok(Data) = Object.ok() {
            *self.Items.borrow_mut() = ReadItems(Data);
        }
        self.Ask(DropPhase::Drop, Keys, Point, Effect);
        if let (Some(Helper), Ok(Data)) = (&self.Helper, Object.ok()) {
            let Screen = POINT { x: Point.x, y: Point.y };
            unsafe {
                let _ = Helper.Drop(Data, &Screen, if Effect.is_null() { DROPEFFECT_NONE } else { *Effect });
            }
        }
        self.Items.borrow_mut().clear();
        Ok(())
    }
}

pub fn Register(Window: HWND) -> bool {
    let Helper: Option<IDropTargetHelper> = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
    let Target: IDropTarget = DropTarget { Window, Helper, Items: RefCell::new(Vec::new()) }.into();
    unsafe { RegisterDragDrop(Window, &Target) }.is_ok()
}

pub fn Revoke(Window: HWND) {
    unsafe {
        let _ = RevokeDragDrop(Window);
    }
}

pub fn StartDrag(Window: HWND, Items: &[PathBuf]) -> Option<u32> {
    let (Object, _Pidls) = crate::Platform::Shell::DataObject(Items)?;
    AttachFileList(&Object, Items);
    let Allowed = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);
    unsafe { SHDoDragDrop(Some(Window), &Object, None, Allowed) }.ok().map(|Effect| Effect.0)
}

fn AttachFileList(Object: &IDataObject, Items: &[PathBuf]) {
    let Format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    if unsafe { Object.QueryGetData(&Format) } == S_OK {
        return;
    }
    let Some(Memory) = super::GlobalFromBytes(&super::DropFilesBytes(Items)) else { return };
    let Medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: Memory }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    if unsafe { Object.SetData(&Format, &Medium, true) }.is_err() {
        let _ = unsafe { GlobalFree(Some(Memory)) };
    }
}
