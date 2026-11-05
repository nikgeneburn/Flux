pub mod Browser;
pub mod Clipboard;
pub mod Imaging;
pub mod Integration;
pub mod Menus;
pub mod Shell;

use crate::Interface::CursorShape;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, GetMonitorInfoW, ScreenToClient};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub const TimerSmoothScroll: usize = 1;
pub const TimerPerformance: usize = 2;
pub const TimerUpdate: usize = 3;

pub fn KeyDown(Key: u16) -> bool {
    unsafe { (GetKeyState(Key as i32) as u16 & 0x8000) != 0 }
}

pub fn Modifiers() -> (bool, bool, bool) {
    (KeyDown(VK_CONTROL.0), KeyDown(VK_SHIFT.0), KeyDown(VK_MENU.0))
}

pub fn WindowDpi(Window: HWND) -> u32 {
    let Dpi = unsafe { GetDpiForWindow(Window) };
    if Dpi == 0 { 96 } else { Dpi }
}

pub fn ClientSize(Window: HWND) -> (u32, u32) {
    let mut Area = RECT::default();
    unsafe {
        let _ = GetClientRect(Window, &mut Area);
    }
    ((Area.right - Area.left).max(0) as u32, (Area.bottom - Area.top).max(0) as u32)
}

pub fn Invalidate(Window: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(Window), None, false);
    }
}

pub fn SetCursorShape(Shape: CursorShape) {
    let Name = match Shape {
        CursorShape::Arrow => IDC_ARROW,
        CursorShape::Text => IDC_IBEAM,
        CursorShape::ResizeHorizontal => IDC_SIZEWE,
    };
    unsafe {
        if let Ok(Cursor) = LoadCursorW(None, Name) {
            SetCursor(Some(Cursor));
        }
    }
}

pub fn Capture(Window: HWND) {
    unsafe {
        SetCapture(Window);
    }
}

pub fn Release() {
    unsafe {
        let _ = ReleaseCapture();
    }
}

pub fn StartTimer(Window: HWND, Identifier: usize, Milliseconds: u32) {
    unsafe {
        SetTimer(Some(Window), Identifier, Milliseconds, None);
    }
}

pub fn StopTimer(Window: HWND, Identifier: usize) {
    unsafe {
        let _ = KillTimer(Some(Window), Identifier);
    }
}

pub fn IsMaximized(Window: HWND) -> bool {
    unsafe { IsZoomed(Window).as_bool() }
}

pub fn Minimize(Window: HWND) {
    unsafe {
        let _ = ShowWindow(Window, SW_MINIMIZE);
    }
}

pub fn ToggleMaximize(Window: HWND) {
    unsafe {
        let _ = ShowWindow(Window, if IsMaximized(Window) { SW_RESTORE } else { SW_MAXIMIZE });
    }
}

pub fn Close(Window: HWND) {
    unsafe {
        let _ = PostMessageW(Some(Window), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn CursorClientPosition(Window: HWND) -> (i32, i32) {
    let mut Point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut Point);
        let _ = ScreenToClient(Window, &mut Point);
    }
    (Point.x, Point.y)
}

pub fn ScreenPoint(Window: HWND, X: i32, Y: i32) -> (i32, i32) {
    let mut Point = POINT { x: X, y: Y };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(Window, &mut Point);
    }
    (Point.x, Point.y)
}

pub fn ApplyDarkFrame(Window: HWND, Dark: bool) {
    let Value: i32 = Dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(Window, DWMWA_USE_IMMERSIVE_DARK_MODE, &Value as *const i32 as *const _, 4);
    }
}

pub struct Placement {
    pub Left: i32,
    pub Top: i32,
    pub Width: i32,
    pub Height: i32,
    pub Maximized: bool,
}

pub fn CurrentPlacement(Window: HWND) -> Option<Placement> {
    let mut Data = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    unsafe { GetWindowPlacement(Window, &mut Data) }.ok()?;
    let Normal = Data.rcNormalPosition;
    Some(Placement {
        Left: Normal.left,
        Top: Normal.top,
        Width: Normal.right - Normal.left,
        Height: Normal.bottom - Normal.top,
        Maximized: Data.showCmd == SW_SHOWMAXIMIZED.0 as u32 || IsMaximized(Window),
    })
}

pub fn ShowError(Window: Option<HWND>, Title: &str, Message: &str) {
    let TitleWide = crate::FileSystem::WideString(Title);
    let MessageWide = crate::FileSystem::WideString(Message);
    unsafe {
        MessageBoxW(Window, PCWSTR(MessageWide.as_ptr()), PCWSTR(TitleWide.as_ptr()), MB_ICONERROR | MB_OK);
    }
}

pub fn CreateMainWindow(Bounds: Option<&Placement>) -> windows::core::Result<HWND> {
    unsafe {
        let Instance = GetModuleHandleW(None)?;
        let LoadSized = |Size: i32| {
            LoadImageW(Some(Instance.into()), PCWSTR(1 as *const u16), IMAGE_ICON, Size, Size, LR_DEFAULTCOLOR).ok().map(|Handle| HICON(Handle.0))
        };
        let Large = LoadSized(GetSystemMetrics(SM_CXICON)).or_else(|| LoadIconW(None, IDI_APPLICATION).ok());
        let Small = LoadSized(GetSystemMetrics(SM_CXSMICON));
        let Class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(WindowProcedure),
            hInstance: Instance.into(),
            hIcon: Large.unwrap_or_default(),
            hIconSm: Small.unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("FluxMainWindow"),
            ..Default::default()
        };
        RegisterClassExW(&Class);
        let (X, Y, Width, Height) = match Bounds {
            Some(Area) if Area.Width >= 400 && Area.Height >= 300 => {
                let Requested = RECT { left: Area.Left, top: Area.Top, right: Area.Left + Area.Width, bottom: Area.Top + Area.Height };
                let Monitor = windows::Win32::Graphics::Gdi::MonitorFromRect(&Requested, MONITOR_DEFAULTTONEAREST);
                let mut Info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                if GetMonitorInfoW(Monitor, &mut Info).as_bool() {
                    let Work = Info.rcWork;
                    let Width = Area.Width.min(Work.right - Work.left);
                    let Height = Area.Height.min(Work.bottom - Work.top);
                    let Left = Area.Left.clamp(Work.left, Work.right - Width);
                    let Top = Area.Top.clamp(Work.top, Work.bottom - Height);
                    (Left, Top, Width, Height)
                } else {
                    (Area.Left, Area.Top, Area.Width, Area.Height)
                }
            }
            _ => (CW_USEDEFAULT, CW_USEDEFAULT, 1280, 800),
        };
        let Window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("FluxMainWindow"),
            w!("Flux"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            X,
            Y,
            Width,
            Height,
            None,
            None,
            Some(Instance.into()),
            None,
        )?;
        let Margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        let _ = DwmExtendFrameIntoClientArea(Window, &Margins);
        let _ = SetWindowPos(Window, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        Ok(Window)
    }
}

pub fn Show(Window: HWND, Maximized: bool) {
    unsafe {
        let _ = ShowWindow(Window, if Maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL });
        let _ = SetForegroundWindow(Window);
    }
}

pub fn Activate(Window: HWND) {
    unsafe {
        if IsIconic(Window).as_bool() {
            let _ = ShowWindow(Window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(Window);
    }
}

fn FrameThickness(Window: HWND) -> (i32, i32) {
    let Dpi = WindowDpi(Window);
    unsafe {
        let Padding = windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXPADDEDBORDER, Dpi);
        (
            windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXFRAME, Dpi) + Padding,
            windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYFRAME, Dpi) + Padding,
        )
    }
}

fn BorderHit(Window: HWND, X: i32, Y: i32) -> Option<u32> {
    if IsMaximized(Window) {
        return None;
    }
    let (Width, Height) = ClientSize(Window);
    let (Width, Height) = (Width as i32, Height as i32);
    let Border = (6 * WindowDpi(Window) / 96) as i32;
    let Left = X < Border;
    let Right = X >= Width - Border;
    let Top = Y < Border;
    let Bottom = Y >= Height - Border;
    Some(match (Left, Right, Top, Bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    })
}

unsafe extern "system" fn WindowProcedure(Window: HWND, Message: u32, WParameter: WPARAM, LParameter: LPARAM) -> LRESULT {
    match Message {
        WM_NCCALCSIZE => {
            if IsMaximized(Window) {
                let Area = if WParameter.0 != 0 {
                    let Parameters = unsafe { &mut *(LParameter.0 as *mut NCCALCSIZE_PARAMS) };
                    &mut Parameters.rgrc[0]
                } else {
                    unsafe { &mut *(LParameter.0 as *mut RECT) }
                };
                let (Horizontal, Vertical) = FrameThickness(Window);
                Area.left += Horizontal;
                Area.right -= Horizontal;
                Area.top += Vertical;
                Area.bottom -= Vertical;
                let Monitor = unsafe { MonitorFromWindow(Window, MONITOR_DEFAULTTONEAREST) };
                let mut Info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                if unsafe { GetMonitorInfoW(Monitor, &mut Info) }.as_bool() {
                    let Work = Info.rcWork;
                    Area.left = Area.left.max(Work.left);
                    Area.top = Area.top.max(Work.top);
                    Area.right = Area.right.min(Work.right);
                    Area.bottom = Area.bottom.min(Work.bottom);
                }
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            let ScreenX = (LParameter.0 & 0xFFFF) as i16 as i32;
            let ScreenY = ((LParameter.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut Point = POINT { x: ScreenX, y: ScreenY };
            unsafe {
                let _ = ScreenToClient(Window, &mut Point);
            }
            if let Some(Code) = BorderHit(Window, Point.x, Point.y) {
                return LRESULT(Code as isize);
            }
            let Code = crate::Application::CaptionTest(Window, Point.x, Point.y);
            LRESULT(Code as isize)
        }
        WM_NCACTIVATE => unsafe { DefWindowProcW(Window, Message, WParameter, LPARAM(-1)) },
        WM_ERASEBKGND => LRESULT(1),
        WM_GETMINMAXINFO => {
            let Info = unsafe { &mut *(LParameter.0 as *mut MINMAXINFO) };
            let Scale = WindowDpi(Window) as i32;
            Info.ptMinTrackSize = POINT { x: 640 * Scale / 96, y: 420 * Scale / 96 };
            LRESULT(0)
        }
        _ => match crate::Application::Dispatch(Window, Message, WParameter, LParameter) {
            Some(Result) => Result,
            None => unsafe { DefWindowProcW(Window, Message, WParameter, LParameter) },
        },
    }
}

pub fn RunMessageLoop() -> i32 {
    let mut Message = MSG::default();
    unsafe {
        loop {
            let Result = GetMessageW(&mut Message, None, 0, 0);
            if Result.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&Message);
            DispatchMessageW(&Message);
        }
    }
    Message.wParam.0 as i32
}
