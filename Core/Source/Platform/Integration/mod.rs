use crate::FileSystem::{Location, WideString};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HWND, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, FindWindowExW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA};
use windows::core::{Owned, PCWSTR, PWSTR, w};

pub const ForwardSignature: usize = 0x464C5558;

const BackupKey: &str = "Software\\Flux\\ShellBackup";
const DirectoryShell: &str = "Software\\Classes\\Directory\\shell";
const DirectoryCommand: &str = "Software\\Classes\\Directory\\shell\\open\\command";
const DriveShell: &str = "Software\\Classes\\Drive\\shell";
const DriveCommand: &str = "Software\\Classes\\Drive\\shell\\open\\command";
const NewWindowCommand: &str = "Software\\Classes\\CLSID\\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\\shell\\opennewwindow\\command";
const NewWindowVerb: &str = "Software\\Classes\\CLSID\\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\\shell\\opennewwindow";
const BackgroundVerb: &str = "Software\\Classes\\Directory\\Background\\shell\\Flux";
const BackgroundCommand: &str = "Software\\Classes\\Directory\\Background\\shell\\Flux\\command";
const NewWindowShell: &str = "Software\\Classes\\CLSID\\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\\shell";
const NewWindowClass: &str = "Software\\Classes\\CLSID\\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}";

const TakenValues: [(&str, &str); 7] = [
    (DirectoryShell, ""),
    (DirectoryCommand, ""),
    (DirectoryCommand, "DelegateExecute"),
    (DriveShell, ""),
    (DriveCommand, ""),
    (DriveCommand, "DelegateExecute"),
    (NewWindowCommand, ""),
];

fn ValueName(Name: &str) -> Option<Vec<u16>> {
    if Name.is_empty() { None } else { Some(WideString(Name)) }
}

fn ReadString(Key: &str, Name: &str) -> Result<Option<String>, String> {
    let KeyText = WideString(Key);
    let NameText = ValueName(Name);
    let NamePointer = NameText.as_ref().map_or(PCWSTR::null(), |Text| PCWSTR(Text.as_ptr()));
    let mut Bytes = 0u32;
    let Status = unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr()), NamePointer, RRF_RT_REG_SZ, None, None, Some(&mut Bytes)) };
    if Status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if Status != ERROR_SUCCESS {
        return Err(Describe(Key, Status));
    }
    let mut Buffer = vec![0u16; Bytes as usize / 2 + 1];
    let mut Size = (Buffer.len() * 2) as u32;
    let Status = unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr()), NamePointer, RRF_RT_REG_SZ, None, Some(Buffer.as_mut_ptr() as *mut _), Some(&mut Size)) };
    if Status != ERROR_SUCCESS {
        return Err(Describe(Key, Status));
    }
    let Length = Buffer.iter().position(|&Unit| Unit == 0).unwrap_or(Buffer.len());
    Ok(Some(String::from_utf16_lossy(&Buffer[..Length])))
}

fn WriteString(Key: &str, Name: &str, Text: &str) -> Result<(), String> {
    let KeyText = WideString(Key);
    let NameText = ValueName(Name);
    let NamePointer = NameText.as_ref().map_or(PCWSTR::null(), |Value| PCWSTR(Value.as_ptr()));
    let Data = WideString(Text);
    let Status = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr()), NamePointer, REG_SZ.0, Some(Data.as_ptr() as *const _), (Data.len() * 2) as u32) };
    if Status == ERROR_SUCCESS { Ok(()) } else { Err(Describe(Key, Status)) }
}

fn DeleteValue(Key: &str, Name: &str) -> Result<(), String> {
    let KeyText = WideString(Key);
    let NameText = ValueName(Name);
    let NamePointer = NameText.as_ref().map_or(PCWSTR::null(), |Value| PCWSTR(Value.as_ptr()));
    let Status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr()), NamePointer) };
    if Status == ERROR_SUCCESS || Status == ERROR_FILE_NOT_FOUND { Ok(()) } else { Err(Describe(Key, Status)) }
}

pub fn ApplyTaskbarIdentity() {
    let Registered = ReadString(DirectoryCommand, "").ok().flatten().unwrap_or_default().to_ascii_lowercase();
    if Registered.contains("\\flux.exe\"") {
        let _ = unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(w!("Microsoft.Windows.Explorer")) };
    }
}

fn DeleteIfEmpty(Key: &str) {
    let KeyText = WideString(Key);
    let mut Handle = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr()), None, KEY_READ, &mut Handle) } != ERROR_SUCCESS {
        return;
    }
    let (mut Subkeys, mut Values) = (0u32, 0u32);
    let Status = unsafe { RegQueryInfoKeyW(Handle, None, None, None, Some(&mut Subkeys), None, None, Some(&mut Values), None, None, None, None) };
    let _ = unsafe { RegCloseKey(Handle) };
    if Status == ERROR_SUCCESS && Subkeys == 0 && Values == 0 {
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr())) };
    }
}

fn DeleteTree(Key: &str) -> Result<(), String> {
    let KeyText = WideString(Key);
    let Status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr())) };
    if Status == ERROR_SUCCESS || Status == ERROR_FILE_NOT_FOUND {
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(KeyText.as_ptr())) };
        Ok(())
    } else {
        Err(Describe(Key, Status))
    }
}

fn Describe(Key: &str, Status: WIN32_ERROR) -> String {
    format!("HKCU\\{Key}: {}", windows::core::Error::from(Status.to_hresult()).message())
}

fn BackupName(Key: &str, Name: &str) -> String {
    format!("{Key}|{Name}")
}

fn CommandFor(Executable: &Path, Argument: &str) -> String {
    format!("\"{}\" \"{Argument}\"", Executable.display())
}

pub fn IsRegistered(Executable: &Path) -> bool {
    let Expected = CommandFor(Executable, "%1").to_lowercase();
    matches!(ReadString(DirectoryCommand, ""), Ok(Some(Current)) if Current.to_lowercase() == Expected)
}

pub fn Register(Executable: &Path) -> Result<(), String> {
    if ReadString(BackupKey, "Registered")?.is_none() {
        for (Key, Name) in TakenValues {
            let Saved = match ReadString(Key, Name)? {
                Some(Text) => format!("1{Text}"),
                None => "0".to_string(),
            };
            WriteString(BackupKey, &BackupName(Key, Name), &Saved)?;
        }
        WriteString(BackupKey, "NewWindowVerbExisted", if ReadString(NewWindowVerb, "")?.is_some() || ReadString(NewWindowCommand, "")?.is_some() { "1" } else { "0" })?;
        WriteString(BackupKey, "Registered", "1")?;
    }
    let Open = CommandFor(Executable, "%1");
    WriteString(DirectoryShell, "", "open")?;
    WriteString(DirectoryCommand, "", &Open)?;
    WriteString(DirectoryCommand, "DelegateExecute", "")?;
    WriteString(DriveShell, "", "open")?;
    WriteString(DriveCommand, "", &Open)?;
    WriteString(DriveCommand, "DelegateExecute", "")?;
    WriteString(NewWindowCommand, "", &format!("\"{}\"", Executable.display()))?;
    WriteString(NewWindowCommand, "DelegateExecute", "")?;
    WriteString(BackgroundVerb, "", "Open in Flux")?;
    WriteString(BackgroundVerb, "Icon", &Executable.display().to_string())?;
    WriteString(BackgroundCommand, "", &CommandFor(Executable, "%V"))?;
    NotifyShell();
    Ok(())
}

pub fn Unregister() -> Result<(), String> {
    if ReadString(BackupKey, "Registered")?.is_some() {
        for (Key, Name) in TakenValues {
            match ReadString(BackupKey, &BackupName(Key, Name))? {
                Some(Saved) if Saved.starts_with('1') => WriteString(Key, Name, &Saved[1..])?,
                _ => DeleteValue(Key, Name)?,
            }
        }
        if ReadString(BackupKey, "NewWindowVerbExisted")?.as_deref() != Some("1") {
            DeleteTree(NewWindowVerb)?;
        } else {
            DeleteValue(NewWindowCommand, "DelegateExecute")?;
        }
        DeleteTree(BackupKey)?;
    } else {
        for (Key, Name) in TakenValues {
            DeleteValue(Key, Name)?;
        }
        DeleteTree(NewWindowVerb)?;
    }
    for Key in [DirectoryCommand, DriveCommand] {
        if ReadString(Key, "")?.is_none() {
            DeleteValue(Key, "DelegateExecute")?;
        }
    }
    DeleteTree(BackgroundVerb)?;
    for Key in [DirectoryCommand, "Software\\Classes\\Directory\\shell\\open", DriveCommand, "Software\\Classes\\Drive\\shell\\open", NewWindowCommand, NewWindowVerb, NewWindowShell, NewWindowClass, DirectoryShell, DriveShell] {
        DeleteIfEmpty(Key);
    }
    NotifyShell();
    Ok(())
}

fn NotifyShell() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn ProcessImage(Process: u32) -> Option<PathBuf> {
    let Handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, Process) }.ok()?;
    let Handle = unsafe { Owned::new(Handle) };
    let mut Buffer = vec![0u16; 32768];
    let mut Length = Buffer.len() as u32;
    unsafe { QueryFullProcessImageNameW(*Handle, PROCESS_NAME_WIN32, PWSTR(Buffer.as_mut_ptr()), &mut Length) }.ok()?;
    Some(crate::FileSystem::PathFromWide(&Buffer[..Length as usize]))
}

fn SameInstanceWindow() -> Option<(HWND, u32)> {
    let Own = std::env::current_exe().ok()?;
    let mut After: Option<HWND> = None;
    loop {
        let Window = unsafe { FindWindowExW(None, After, w!("FluxMainWindow"), PCWSTR::null()) }.ok()?;
        let mut Process = 0u32;
        unsafe { GetWindowThreadProcessId(Window, Some(&mut Process)) };
        if Process != 0 && ProcessImage(Process).is_some_and(|Image| crate::FileSystem::SamePath(&Image, &Own)) {
            return Some((Window, Process));
        }
        After = Some(Window);
    }
}

pub fn ForwardToRunningInstance(Target: &str) -> bool {
    let Some((Window, Process)) = SameInstanceWindow() else { return false };
    let _ = unsafe { AllowSetForegroundWindow(Process) };
    let Payload: Vec<u16> = Target.encode_utf16().collect();
    let Data = COPYDATASTRUCT { dwData: ForwardSignature, cbData: (Payload.len() * 2) as u32, lpData: Payload.as_ptr() as *mut _ };
    let mut Answer = 0usize;
    let Sent = unsafe { SendMessageTimeoutW(Window, WM_COPYDATA, WPARAM(0), LPARAM(&Data as *const _ as isize), SMTO_ABORTIFHUNG, 3000, Some(&mut Answer)) };
    Sent.0 != 0 && Answer == 1
}

pub fn DecodeForward(LParameter: LPARAM) -> Option<String> {
    let Data = unsafe { (LParameter.0 as *const COPYDATASTRUCT).as_ref()? };
    if Data.dwData != ForwardSignature {
        return None;
    }
    if Data.cbData == 0 || Data.lpData.is_null() {
        return Some(String::new());
    }
    let Units = unsafe { std::slice::from_raw_parts(Data.lpData as *const u16, Data.cbData as usize / 2) };
    Some(String::from_utf16_lossy(Units))
}

pub fn StartupTarget(Arguments: &[String]) -> Option<String> {
    let mut Index = 1;
    while Index < Arguments.len() {
        let Item = &Arguments[Index];
        if Item.starts_with("--") {
            Index += if matches!(Item.as_str(), "--bench-ui" | "--output") { 2 } else { 1 };
            continue;
        }
        return Some(Item.trim_matches('"').to_string());
    }
    None
}

pub fn ForwardText(Arguments: &[String]) -> String {
    match StartupTarget(Arguments).and_then(|Text| ResolveTarget(&Text)) {
        Some((Location::Computer, _)) => "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}".to_string(),
        Some((Location::Directory(_), Some(Item))) => format!("/select,{}", Item.display()),
        Some((Location::Directory(Folder), None)) => Folder.display().to_string(),
        _ => String::new(),
    }
}

pub fn ResolveTarget(Text: &str) -> Option<(Location, Option<PathBuf>)> {
    let Trimmed = Text.trim().trim_matches('"');
    if Trimmed.to_ascii_uppercase().contains("20D04FE0-3AEA-1069-A2D8-08002B30309D") {
        return Some((Location::Computer, None));
    }
    if Trimmed.is_empty() || Trimmed.starts_with("::") || Trimmed.starts_with("shell:") {
        return None;
    }
    let Selected = Trimmed.strip_prefix("/select,").map(|Rest| Rest.trim().trim_matches('"'));
    let mut Text = Selected.unwrap_or(Trimmed).to_string();
    if Text.len() == 2 && Text.ends_with(':') {
        Text.push('\\');
    }
    let Candidate = PathBuf::from(Text);
    let Absolute = std::path::absolute(&Candidate).unwrap_or(Candidate);
    if Selected.is_some() || Absolute.is_file() {
        let Parent = Absolute.parent()?.to_path_buf();
        return Some((Location::Directory(Parent), Some(Absolute)));
    }
    Some((Location::Directory(Absolute), None))
}
