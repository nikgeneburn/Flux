use super::{BackgroundMenu, ItemsMenu, MenuHandle, ShellFirst, ShellLast, VerbOf};
use crate::Application::Event;
use crate::Platform::Imaging::{MenuBitmapPixels, Pixels};
use crate::Platform::Shell::OwnedPidl;
use crate::Services::Workers::{EventSink, Spawn};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO,
    CMINVOKECOMMANDINFOEX, IContextMenu, IContextMenu2, IContextMenu3,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DispatchMessageW, GetMenuItemCount, GetMenuItemInfoW, GetMessageW, HMENU, MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED,
    MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SW_SHOWNORMAL, TranslateMessage, WM_APP, WM_INITMENUPOPUP, WM_QUIT,
};
use windows::core::{Interface, PCSTR, PCWSTR, PWSTR};

pub struct ShellEntry {
    pub Label: String,
    pub Verb: String,
    pub Command: Option<u32>,
    pub Enabled: bool,
    pub Checked: bool,
    pub Image: Option<Pixels>,
    pub Children: Vec<ShellEntry>,
}

impl ShellEntry {
    pub fn IsSeparator(&self) -> bool {
        self.Label.is_empty()
    }
}

pub enum ShellTarget {
    Items(Vec<PathBuf>),
    Background(PathBuf),
}

pub enum ShellMenuUpdate {
    Ready { Token: u64, Entries: Vec<ShellEntry> },
    Failed(String),
}

enum Request {
    Build { Token: u64, Target: ShellTarget, Extended: bool },
    Invoke { Token: u64, Command: u32, X: i32, Y: i32, Control: bool, Shift: bool },
}

struct Built {
    Token: u64,
    Context: IContextMenu,
    _Menu: MenuHandle,
    _Pidls: Vec<OwnedPidl>,
}

pub struct ShellMenus {
    Requests: Sender<Request>,
    Thread: u32,
}

impl ShellMenus {
    pub fn Start(Window: HWND, Sink: EventSink) -> Option<ShellMenus> {
        let (Requests, Incoming) = channel();
        let (Ready, Started) = channel::<u32>();
        let Owner = Window.0 as isize;
        Spawn("ShellMenu", move || Run(HWND(Owner as *mut _), Sink, Incoming, Ready)).ok()?;
        let Thread = Started.recv().ok()?;
        Some(ShellMenus { Requests, Thread })
    }

    fn Post(&self, Item: Request) {
        if self.Requests.send(Item).is_ok() {
            let _ = unsafe { PostThreadMessageW(self.Thread, WM_APP, WPARAM(0), LPARAM(0)) };
        }
    }

    pub fn Build(&self, Token: u64, Target: ShellTarget, Extended: bool) {
        self.Post(Request::Build { Token, Target, Extended });
    }

    pub fn Invoke(&self, Token: u64, Command: u32, X: i32, Y: i32) {
        let (Control, Shift, _) = super::super::Modifiers();
        self.Post(Request::Invoke { Token, Command, X, Y, Control, Shift });
    }
}

impl Drop for ShellMenus {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.Thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

fn Run(Window: HWND, Sink: EventSink, Incoming: Receiver<Request>, Ready: Sender<u32>) {
    let Initialized = unsafe { OleInitialize(None) }.is_ok();
    let mut Message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut Message, None, WM_APP, WM_APP, PM_NOREMOVE);
    }
    if Ready.send(unsafe { GetCurrentThreadId() }).is_err() {
        return;
    }
    let mut Active: Option<Built> = None;
    loop {
        if unsafe { GetMessageW(&mut Message, None, 0, 0) }.0 <= 0 {
            break;
        }
        if Message.hwnd.is_invalid() && Message.message == WM_APP {
            let Pending: Vec<Request> = Incoming.try_iter().collect();
            let LastBuild = Pending.iter().rposition(|Item| matches!(Item, Request::Build { .. }));
            for (Position, Item) in Pending.into_iter().enumerate() {
                match Item {
                    Request::Build { Token, Target, Extended } if Some(Position) == LastBuild => {
                        drop(Active.take());
                        Active = Build(Window, &Sink, Token, Target, Extended);
                    }
                    Request::Build { .. } => {}
                    Request::Invoke { Token, Command, X, Y, Control, Shift } => {
                        if let Some(Current) = Active.as_ref().filter(|Current| Current.Token == Token) {
                            Invoke(Window, &Sink, &Current.Context, Command, X, Y, Control, Shift);
                        }
                    }
                }
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&Message);
            DispatchMessageW(&Message);
        }
    }
    drop(Active);
    if Initialized {
        unsafe { OleUninitialize() };
    }
}

fn Build(Window: HWND, Sink: &EventSink, Token: u64, Target: ShellTarget, Extended: bool) -> Option<Built> {
    let Shell = match &Target {
        ShellTarget::Items(Items) => ItemsMenu(Window, Items),
        ShellTarget::Background(Directory) => BackgroundMenu(Window, Directory),
    };
    let Some((Context, Pidls)) = Shell else {
        Sink.Send(Event::ShellMenu(ShellMenuUpdate::Ready { Token, Entries: Vec::new() }));
        return None;
    };
    let Menu = MenuHandle(unsafe { CreatePopupMenu() }.ok()?);
    let Flags = CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | if Extended { CMF_EXTENDEDVERBS } else { 0 };
    if unsafe { Context.QueryContextMenu(Menu.0, 0, ShellFirst, ShellLast, Flags) }.is_err() {
        Sink.Send(Event::ShellMenu(ShellMenuUpdate::Ready { Token, Entries: Vec::new() }));
        return None;
    }
    let Second = Context.cast::<IContextMenu2>().ok();
    let Third = Context.cast::<IContextMenu3>().ok();
    let Entries = Walk(&Context, Second.as_ref(), Third.as_ref(), Menu.0, 0);
    Sink.Send(Event::ShellMenu(ShellMenuUpdate::Ready { Token, Entries }));
    Some(Built { Token, Context, _Menu: Menu, _Pidls: Pidls })
}

fn Walk(Context: &IContextMenu, Second: Option<&IContextMenu2>, Third: Option<&IContextMenu3>, Menu: HMENU, Depth: u32) -> Vec<ShellEntry> {
    let Count = unsafe { GetMenuItemCount(Some(Menu)) };
    let mut Entries: Vec<ShellEntry> = Vec::with_capacity(Count.max(0) as usize);
    for Position in 0..Count.max(0) as u32 {
        let mut Info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_BITMAP | MIIM_STRING,
            ..Default::default()
        };
        if unsafe { GetMenuItemInfoW(Menu, Position, true, &mut Info) }.is_err() {
            continue;
        }
        if Info.fType.0 & MFT_SEPARATOR.0 != 0 {
            if Entries.last().is_some_and(|Last| !Last.IsSeparator()) {
                Entries.push(Separator());
            }
            continue;
        }
        let Label = CleanLabel(&ItemText(Menu, Position, Info.cch));
        if Label.is_empty() {
            continue;
        }
        let HasChildren = !Info.hSubMenu.is_invalid();
        let Children = if HasChildren && Depth < 2 {
            Populate(Second, Third, Info.hSubMenu, Position);
            Walk(Context, Second, Third, Info.hSubMenu, Depth + 1)
        } else {
            Vec::new()
        };
        if HasChildren && Children.is_empty() {
            continue;
        }
        let Command = (!HasChildren && (ShellFirst..=ShellLast).contains(&Info.wID)).then(|| Info.wID - ShellFirst);
        Entries.push(ShellEntry {
            Verb: Command.map(|Offset| VerbOf(Context, Offset)).unwrap_or_default(),
            Label,
            Command,
            Enabled: Info.fState.0 & MFS_DISABLED.0 == 0,
            Checked: Info.fState.0 & MFS_CHECKED.0 != 0,
            Image: MenuImage(Info.hbmpItem),
            Children,
        });
    }
    while Entries.last().is_some_and(ShellEntry::IsSeparator) {
        Entries.pop();
    }
    Entries
}

fn Separator() -> ShellEntry {
    ShellEntry { Label: String::new(), Verb: String::new(), Command: None, Enabled: false, Checked: false, Image: None, Children: Vec::new() }
}

fn ItemText(Menu: HMENU, Position: u32, Length: u32) -> String {
    if Length == 0 {
        return String::new();
    }
    let mut Buffer = vec![0u16; Length as usize + 1];
    let mut Info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_STRING,
        dwTypeData: PWSTR(Buffer.as_mut_ptr()),
        cch: Length + 1,
        ..Default::default()
    };
    if unsafe { GetMenuItemInfoW(Menu, Position, true, &mut Info) }.is_err() {
        return String::new();
    }
    let End = Buffer.iter().position(|&Unit| Unit == 0).unwrap_or(Buffer.len());
    String::from_utf16_lossy(&Buffer[..End])
}

fn CleanLabel(Text: &str) -> String {
    let Visible = Text.split('\t').next().unwrap_or("");
    let mut Output = String::with_capacity(Visible.len());
    let mut Characters = Visible.chars().peekable();
    while let Some(Character) = Characters.next() {
        if Character == '&' {
            if Characters.peek() == Some(&'&') {
                Output.push('&');
                Characters.next();
            }
            continue;
        }
        Output.push(Character);
    }
    Output.trim().to_string()
}

fn Populate(Second: Option<&IContextMenu2>, Third: Option<&IContextMenu3>, Child: HMENU, Position: u32) {
    let WParameter = WPARAM(Child.0 as usize);
    let LParameter = LPARAM(Position as isize);
    if let Some(Menu) = Third {
        let mut Result = LRESULT(0);
        if unsafe { Menu.HandleMenuMsg2(WM_INITMENUPOPUP, WParameter, LParameter, Some(&mut Result)) }.is_ok() {
            return;
        }
    }
    if let Some(Menu) = Second {
        let _ = unsafe { Menu.HandleMenuMsg(WM_INITMENUPOPUP, WParameter, LParameter) };
    }
}

fn MenuImage(Bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<Pixels> {
    let Value = Bitmap.0 as isize;
    if Value <= 11 {
        return None;
    }
    MenuBitmapPixels(Bitmap)
}

fn Invoke(Window: HWND, Sink: &EventSink, Context: &IContextMenu, Command: u32, X: i32, Y: i32, Control: bool, Shift: bool) {
    let mut Mask = 0x4000 | CMIC_MASK_PTINVOKE;
    if Control {
        Mask |= CMIC_MASK_CONTROL_DOWN;
    }
    if Shift {
        Mask |= CMIC_MASK_SHIFT_DOWN;
    }
    let Info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: Mask,
        hwnd: Window,
        lpVerb: PCSTR(Command as usize as *const u8),
        lpVerbW: PCWSTR(Command as usize as *const u16),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: POINT { x: X, y: Y },
        ..Default::default()
    };
    if let Err(Error) = unsafe { Context.InvokeCommand(&Info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO) } {
        if Error.code().0 as u32 != 0x800704C7 {
            Sink.Send(Event::ShellMenu(ShellMenuUpdate::Failed(Error.message())));
        }
    }
}
