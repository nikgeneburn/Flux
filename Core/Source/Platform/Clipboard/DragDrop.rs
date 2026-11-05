use super::FilesFromDrop;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{GlobalFree, HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, HDROP, IDropTargetHelper, IShellFolder, SHBindToParent, SHDoDragDrop};
use windows::core::{Ref, implement};

pub struct DropAnswer {
    pub Effect: u32,
    pub Shell: Option<PathBuf>,
}

thread_local! {
    static Probe: RefCell<Option<(PathBuf, Option<IDropTarget>)>> = const { RefCell::new(None) };
}

fn ItemDropTarget(Window: HWND, Item: &Path) -> Option<IDropTarget> {
    let Pidl = crate::Platform::Shell::ParsePidl(Item)?;
    let mut Last: *mut ITEMIDLIST = std::ptr::null_mut();
    let Parent: IShellFolder = unsafe { SHBindToParent(Pidl.0, Some(&mut Last)) }.ok()?;
    unsafe { Parent.GetUIObjectOf::<IDropTarget>(Window, &[Last as *const ITEMIDLIST], None) }.ok()
}

pub fn FolderDropTarget(Window: HWND, Item: &Path) -> Option<IDropTarget> {
    ItemDropTarget(Window, Item)
}

fn Probed(Window: HWND, Item: &Path) -> Option<IDropTarget> {
    Probe.with(|Slot| {
        let mut Slot = Slot.borrow_mut();
        if let Some((Known, Target)) = Slot.as_ref() {
            if Known == Item {
                return Target.clone();
            }
        }
        let Target = ItemDropTarget(Window, Item);
        *Slot = Some((Item.to_path_buf(), Target.clone()));
        Target
    })
}

pub fn AcceptsDrops(Window: HWND, Item: &Path) -> bool {
    Probed(Window, Item).is_some()
}

fn Forget() {
    Probe.with(|Slot| Slot.borrow_mut().take());
}

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
    Data: RefCell<Option<IDataObject>>,
    Entered: RefCell<Option<(PathBuf, IDropTarget)>>,
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
        if Effect.is_null() {
            return;
        }
        let mut Client = POINT { x: Point.x, y: Point.y };
        unsafe {
            let _ = ScreenToClient(self.Window, &mut Client);
        }
        let Allowed = unsafe { (*Effect).0 };
        let Items = self.Items.borrow().clone();
        if Items.is_empty() && self.Data.borrow().is_none() {
            unsafe { *Effect = DROPEFFECT_NONE };
            return;
        }
        let Request = DropRequest { Phase, X: Client.x, Y: Client.y, Keys: Keys.0, Allowed, Items: &Items };
        let Answer = crate::Application::DropFeedback(&Request);
        if self.Forward(Phase, Answer.Shell, Keys, Point, Effect, Allowed) {
            return;
        }
        let Chosen = if Phase == DropPhase::Drop { crate::Application::DropCommit(&Request) } else { Answer.Effect };
        unsafe { *Effect = DROPEFFECT(Chosen & Allowed) };
    }

    fn Forward(&self, Phase: DropPhase, Shell: Option<PathBuf>, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT, Allowed: u32) -> bool {
        let Some(Item) = Shell else {
            self.LeaveForward();
            return false;
        };
        let Some(Data) = self.Data.borrow().clone() else { return false };
        let Current = self.Entered.borrow().as_ref().filter(|(Known, _)| *Known == Item).map(|(_, Target)| Target.clone());
        unsafe { *Effect = DROPEFFECT(Allowed) };
        let Target = match Current {
            Some(Target) => {
                if Phase != DropPhase::Drop {
                    let _ = unsafe { Target.DragOver(Keys, *Point, Effect) };
                }
                Target
            }
            None => {
                self.LeaveForward();
                let Some(Target) = Probed(self.Window, &Item) else { return false };
                let _ = unsafe { Target.DragEnter(&Data, Keys, *Point, Effect) };
                *self.Entered.borrow_mut() = Some((Item, Target.clone()));
                Target
            }
        };
        if Phase == DropPhase::Drop {
            self.Entered.borrow_mut().take();
            unsafe { *Effect = DROPEFFECT(Allowed) };
            if unsafe { Target.Drop(&Data, Keys, *Point, Effect) }.is_err() {
                unsafe { *Effect = DROPEFFECT_NONE };
            }
            crate::Application::AfterShellDrop();
        }
        true
    }

    fn LeaveForward(&self) {
        let Previous = self.Entered.borrow_mut().take();
        if let Some((_, Target)) = Previous {
            let _ = unsafe { Target.DragLeave() };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, Object: Ref<IDataObject>, Keys: MODIFIERKEYS_FLAGS, Point: &POINTL, Effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        *self.Items.borrow_mut() = Object.ok().map(ReadItems).unwrap_or_default();
        *self.Data.borrow_mut() = Object.ok().ok().cloned();
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
        self.LeaveForward();
        Forget();
        *self.Data.borrow_mut() = None;
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
            *self.Data.borrow_mut() = Some(Data.clone());
        }
        self.Ask(DropPhase::Drop, Keys, Point, Effect);
        if let (Some(Helper), Ok(Data)) = (&self.Helper, Object.ok()) {
            let Screen = POINT { x: Point.x, y: Point.y };
            unsafe {
                let _ = Helper.Drop(Data, &Screen, if Effect.is_null() { DROPEFFECT_NONE } else { *Effect });
            }
        }
        self.Items.borrow_mut().clear();
        *self.Data.borrow_mut() = None;
        Forget();
        Ok(())
    }
}

pub fn Register(Window: HWND) -> bool {
    let Helper: Option<IDropTargetHelper> = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
    let Target: IDropTarget = DropTarget { Window, Helper, Items: RefCell::new(Vec::new()), Data: RefCell::new(None), Entered: RefCell::new(None) }.into();
    unsafe { RegisterDragDrop(Window, &Target) }.is_ok()
}

pub fn Revoke(Window: HWND) {
    unsafe {
        let _ = RevokeDragDrop(Window);
    }
}

pub fn StartDrag(Window: HWND, Items: &[PathBuf]) -> Option<u32> {
    let (Object, _Pidls) = crate::Platform::Shell::DataObject(Items)?;
    if !Items.iter().any(|Item| crate::FileSystem::InArchive(Item)) {
        AttachFileList(&Object, Items);
    }
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
