pub mod Extensions;

use super::Shell::{OwnedPidl, ParsePidl};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, IShellFolder, ILFindLastID,
    SHBindToParent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, HMENU, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_DRAWITEM, WM_INITMENUPOPUP,
    WM_MEASUREITEM, WM_MENUCHAR,
};
use windows::core::{Interface, PCSTR, PCWSTR, PSTR, w};

struct MenuHandle(HMENU);

impl Drop for MenuHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

thread_local! {
    static ActiveMenu: RefCell<Option<(Option<IContextMenu2>, Option<IContextMenu3>)>> = const { RefCell::new(None) };
}

pub fn RouteMenuMessage(Message: u32, WParameter: WPARAM, LParameter: LPARAM) -> Option<LRESULT> {
    if !matches!(Message, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    ActiveMenu.with(|Slot| {
        let Slot = Slot.borrow();
        let (Second, Third) = Slot.as_ref()?;
        if let Some(Menu) = Third {
            let mut Result = LRESULT(0);
            if unsafe { Menu.HandleMenuMsg2(Message, WParameter, LParameter, Some(&mut Result)) }.is_ok() {
                return Some(Result);
            }
        }
        if let Some(Menu) = Second {
            if unsafe { Menu.HandleMenuMsg(Message, WParameter, LParameter) }.is_ok() {
                return Some(LRESULT(0));
            }
        }
        None
    })
}

pub enum ContextOutcome {
    Nothing,
    Rename,
    Delete,
    Paste,
    Copy,
    Cut,
    Invoked,
    Failed(String),
}

pub enum ContextTarget<'a> {
    Items(&'a [PathBuf]),
    Background(&'a Path),
}

const ShellFirst: u32 = 0x1000;
const ShellLast: u32 = 0x6FFF;

fn ItemsMenu(Window: HWND, Items: &[PathBuf]) -> Option<(IContextMenu, Vec<OwnedPidl>)> {
    let Pidls: Vec<OwnedPidl> = Items.iter().filter_map(|Item| ParsePidl(Item)).collect();
    if Pidls.is_empty() {
        return None;
    }
    let mut LastIdentifier: *mut ITEMIDLIST = std::ptr::null_mut();
    let Parent: IShellFolder = unsafe { SHBindToParent(Pidls[0].0, Some(&mut LastIdentifier)) }.ok()?;
    let Children: Vec<*const ITEMIDLIST> = Pidls.iter().map(|Pidl| unsafe { ILFindLastID(Pidl.0) } as *const _).collect();
    let Menu: IContextMenu = unsafe { Parent.GetUIObjectOf(Window, &Children, None) }.ok()?;
    Some((Menu, Pidls))
}

fn BackgroundMenu(Window: HWND, Directory: &Path) -> Option<(IContextMenu, Vec<OwnedPidl>)> {
    let Pidl = ParsePidl(Directory)?;
    let mut LastIdentifier: *mut ITEMIDLIST = std::ptr::null_mut();
    let Parent: IShellFolder = unsafe { SHBindToParent(Pidl.0, Some(&mut LastIdentifier)) }.ok()?;
    let Folder: IShellFolder = unsafe { Parent.BindToObject(LastIdentifier, None) }.ok()?;
    let Menu: IContextMenu = unsafe { Folder.CreateViewObject(Window) }.ok()?;
    Some((Menu, vec![Pidl]))
}

fn VerbOf(Menu: &IContextMenu, Offset: u32) -> String {
    let mut Buffer = [0u16; 128];
    let Result = unsafe { Menu.GetCommandString(Offset as usize, GCS_VERBW, None, PSTR(Buffer.as_mut_ptr() as *mut u8), Buffer.len() as u32) };
    if Result.is_err() {
        return String::new();
    }
    let Length = Buffer.iter().position(|&Unit| Unit == 0).unwrap_or(0);
    String::from_utf16_lossy(&Buffer[..Length]).to_ascii_lowercase()
}

pub fn ShowContextMenu(Window: HWND, Target: ContextTarget, ScreenX: i32, ScreenY: i32, Extended: bool) -> ContextOutcome {
    let Menu = match unsafe { CreatePopupMenu() } {
        Ok(Menu) => MenuHandle(Menu),
        Err(Error) => return ContextOutcome::Failed(Error.message()),
    };
    let Shell = match Target {
        ContextTarget::Items(Items) => ItemsMenu(Window, Items),
        ContextTarget::Background(Directory) => BackgroundMenu(Window, Directory),
    };
    let Some((Context, _)) = &Shell else { return ContextOutcome::Failed("The Windows shell menu is not available for this location".to_string()) };
    let Flags = CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | if Extended { CMF_EXTENDEDVERBS } else { 0 };
    let _ = unsafe { Context.QueryContextMenu(Menu.0, 0, ShellFirst, ShellLast, Flags) };
    let Second = Context.cast::<IContextMenu2>().ok();
    let Third = Context.cast::<IContextMenu3>().ok();
    ActiveMenu.with(|Slot| *Slot.borrow_mut() = Some((Second, Third)));
    let Chosen = unsafe { TrackPopupMenuEx(Menu.0, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, ScreenX, ScreenY, Window, None) }.0 as u32;
    ActiveMenu.with(|Slot| *Slot.borrow_mut() = None);
    if Chosen == 0 {
        return ContextOutcome::Nothing;
    }
    if Chosen < ShellFirst {
        return ContextOutcome::Nothing;
    }
    let Some((Context, _Pidls)) = Shell else { return ContextOutcome::Nothing };
    let Offset = Chosen - ShellFirst;
    match VerbOf(&Context, Offset).as_str() {
        "rename" => return ContextOutcome::Rename,
        "delete" => return ContextOutcome::Delete,
        "paste" => return ContextOutcome::Paste,
        "copy" => return ContextOutcome::Copy,
        "cut" => return ContextOutcome::Cut,
        _ => {}
    }
    let (Control, ShiftKey, _) = super::Modifiers();
    let mut Mask = 0x4000 | CMIC_MASK_PTINVOKE;
    if Control {
        Mask |= CMIC_MASK_CONTROL_DOWN;
    }
    if ShiftKey {
        Mask |= CMIC_MASK_SHIFT_DOWN;
    }
    let Info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: Mask,
        hwnd: Window,
        lpVerb: PCSTR(Offset as usize as *const u8),
        lpVerbW: PCWSTR(Offset as usize as *const u16),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: POINT { x: ScreenX, y: ScreenY },
        ..Default::default()
    };
    match unsafe { Context.InvokeCommand(&Info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) } {
        Ok(()) => ContextOutcome::Invoked,
        Err(Error) if Error.code().0 as u32 == 0x800704C7 => ContextOutcome::Nothing,
        Err(Error) => ContextOutcome::Failed(Error.message()),
    }
}

pub fn ApplyMenuTheme(Dark: bool) {
    unsafe {
        let Ok(Library) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(SetMode) = GetProcAddress(Library, PCSTR(135 as *const u8)) {
            let Function: extern "system" fn(i32) -> i32 = std::mem::transmute(SetMode);
            Function(if Dark { 2 } else { 3 });
        }
        if let Some(Flush) = GetProcAddress(Library, PCSTR(136 as *const u8)) {
            let Function: extern "system" fn() = std::mem::transmute(Flush);
            Function();
        }
    }
}
