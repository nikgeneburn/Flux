pub mod DragDrop;

use crate::FileSystem::{PathFromWide, ShellPath};
use std::path::PathBuf;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};
use windows::core::{BOOL, w};

pub const EffectCopy: u32 = 1;
pub const EffectMove: u32 = 2;
pub const EffectLink: u32 = 4;

struct ClipboardSession;

impl ClipboardSession {
    fn Open(Window: HWND) -> Option<ClipboardSession> {
        for _ in 0..8 {
            if unsafe { OpenClipboard(Some(Window)) }.is_ok() {
                return Some(ClipboardSession);
            }
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        None
    }
}

impl Drop for ClipboardSession {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn DropEffectFormat() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

pub fn GlobalFromBytes(Bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let Memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, Bytes.len().max(1)).ok()?;
        let Pointer = GlobalLock(Memory) as *mut u8;
        if Pointer.is_null() {
            let _ = GlobalFree(Some(Memory));
            return None;
        }
        std::ptr::copy_nonoverlapping(Bytes.as_ptr(), Pointer, Bytes.len());
        let _ = GlobalUnlock(Memory);
        Some(Memory)
    }
}

fn Place(Format: u32, Bytes: &[u8]) -> bool {
    let Some(Memory) = GlobalFromBytes(Bytes) else { return false };
    if unsafe { SetClipboardData(Format, Some(HANDLE(Memory.0))) }.is_err() {
        unsafe {
            let _ = GlobalFree(Some(Memory));
        }
        return false;
    }
    true
}

pub fn DropFilesBytes(Items: &[PathBuf]) -> Vec<u8> {
    let Header = DROPFILES { pFiles: std::mem::size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: BOOL(0), fWide: BOOL(1) };
    let mut Bytes = Vec::with_capacity(64 + Items.len() * 64);
    Bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(&Header as *const DROPFILES as *const u8, std::mem::size_of::<DROPFILES>()) });
    for Item in Items {
        for Unit in ShellPath(Item) {
            Bytes.extend_from_slice(&Unit.to_le_bytes());
        }
    }
    Bytes.extend_from_slice(&[0, 0]);
    Bytes
}

pub fn SetFiles(Window: HWND, Items: &[PathBuf], Cut: bool) -> bool {
    if Items.is_empty() {
        return false;
    }
    let Some(_Session) = ClipboardSession::Open(Window) else { return false };
    unsafe {
        let _ = EmptyClipboard();
    }
    let Effect: u32 = if Cut { EffectMove } else { EffectCopy | EffectLink };
    Place(CF_HDROP.0 as u32, &DropFilesBytes(Items)) && Place(DropEffectFormat(), &Effect.to_le_bytes())
}

pub fn FilesFromDrop(Drop: HDROP) -> Vec<PathBuf> {
    let Count = unsafe { DragQueryFileW(Drop, u32::MAX, None) };
    let mut Items = Vec::with_capacity(Count as usize);
    for Index in 0..Count {
        let Length = unsafe { DragQueryFileW(Drop, Index, None) } as usize;
        let mut Buffer = vec![0u16; Length + 1];
        let Copied = unsafe { DragQueryFileW(Drop, Index, Some(&mut Buffer)) } as usize;
        if Copied > 0 {
            Items.push(PathFromWide(&Buffer[..Copied]));
        }
    }
    Items
}

pub fn HasFiles() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok()
}

pub fn GetFiles(Window: HWND) -> Option<(Vec<PathBuf>, bool)> {
    let _Session = ClipboardSession::Open(Window)?;
    let Handle = unsafe { GetClipboardData(CF_HDROP.0 as u32) }.ok()?;
    let Memory = HGLOBAL(Handle.0);
    let Locked = unsafe { GlobalLock(Memory) };
    if Locked.is_null() {
        return None;
    }
    let Items = FilesFromDrop(HDROP(Locked));
    unsafe {
        let _ = GlobalUnlock(Memory);
    }
    let mut Cut = false;
    if let Ok(EffectHandle) = unsafe { GetClipboardData(DropEffectFormat()) } {
        let EffectMemory = HGLOBAL(EffectHandle.0);
        let Pointer = unsafe { GlobalLock(EffectMemory) } as *const u32;
        if !Pointer.is_null() {
            Cut = unsafe { Pointer.read_unaligned() } & EffectMove != 0;
            unsafe {
                let _ = GlobalUnlock(EffectMemory);
            }
        }
    }
    if Items.is_empty() { None } else { Some((Items, Cut)) }
}

pub fn ClearIfCut(Window: HWND) {
    let Some(_Session) = ClipboardSession::Open(Window) else { return };
    unsafe {
        let _ = EmptyClipboard();
    }
}

pub fn SetText(Window: HWND, Text: &str) -> bool {
    let Some(_Session) = ClipboardSession::Open(Window) else { return false };
    unsafe {
        let _ = EmptyClipboard();
    }
    let Bytes: Vec<u8> = Text.encode_utf16().chain(std::iter::once(0)).flat_map(|Unit| Unit.to_le_bytes()).collect();
    Place(CF_UNICODETEXT.0 as u32, &Bytes)
}

pub fn GetText(Window: HWND) -> Option<String> {
    let _Session = ClipboardSession::Open(Window)?;
    let Handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    let Memory = HGLOBAL(Handle.0);
    let Pointer = unsafe { GlobalLock(Memory) } as *const u16;
    if Pointer.is_null() {
        return None;
    }
    let mut Length = 0usize;
    while unsafe { *Pointer.add(Length) } != 0 && Length < 1 << 24 {
        Length += 1;
    }
    let Text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(Pointer, Length) });
    unsafe {
        let _ = GlobalUnlock(Memory);
    }
    Some(Text)
}
