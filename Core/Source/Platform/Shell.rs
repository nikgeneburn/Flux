use crate::FileSystem::{AttributeDirectory, ShellPath, WideNull, WideString};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IDataObject};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_Videos,
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, ILFree, KF_FLAG_DEFAULT, OAIF_ALLOW_REGISTRATION,
    OAIF_EXEC, OPENASINFO, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SHCreateDataObject, SHCreateItemFromParsingName, SHELLEXECUTEINFOW,
    SHFILEINFOW, SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetKnownFolderPath, SHMultiFileProperties,
    SHOP_FILEPATH, SHObjectProperties, SHOpenWithDialog, SHParseDisplayName, SIGDN_FILESYSPATH, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, PCWSTR};

pub struct OwnedPidl(pub *mut ITEMIDLIST);

impl Drop for OwnedPidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ILFree(Some(self.0)) };
        }
    }
}

pub fn ParsePidl(Item: &Path) -> Option<OwnedPidl> {
    let Wide = ShellPath(Item);
    let mut Pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(Wide.as_ptr()), None, &mut Pidl, 0, None) }.ok()?;
    if Pidl.is_null() { None } else { Some(OwnedPidl(Pidl)) }
}

pub fn DataObject(Items: &[PathBuf]) -> Option<(IDataObject, Vec<OwnedPidl>)> {
    use windows::Win32::UI::Shell::{ILFindLastID, IShellFolder, SHBindToParent};
    let Pidls: Vec<OwnedPidl> = Items.iter().filter_map(|Item| ParsePidl(Item)).collect();
    if Pidls.is_empty() {
        return None;
    }
    let Shared = Items.first().and_then(|First| First.parent()).is_some_and(|Folder| Items.iter().all(|Item| Item.parent().is_some_and(|Other| crate::FileSystem::SamePath(Other, Folder))));
    if Shared && Pidls.len() == Items.len() {
        let mut Last: *mut ITEMIDLIST = std::ptr::null_mut();
        if let Ok(Parent) = unsafe { SHBindToParent::<IShellFolder>(Pidls[0].0, Some(&mut Last)) } {
            let Children: Vec<*const ITEMIDLIST> = Pidls.iter().map(|Pidl| unsafe { ILFindLastID(Pidl.0) } as *const _).collect();
            if let Ok(Object) = unsafe { Parent.GetUIObjectOf::<IDataObject>(HWND::default(), &Children, None) } {
                return Some((Object, Pidls));
            }
        }
    }
    let Raw: Vec<*const ITEMIDLIST> = Pidls.iter().map(|Pidl| Pidl.0 as *const _).collect();
    let Object: IDataObject = unsafe { SHCreateDataObject(None, Some(&Raw), None) }.ok()?;
    Some((Object, Pidls))
}

pub fn ShellItem(Item: &Path) -> Option<IShellItem> {
    let Wide = ShellPath(Item);
    unsafe { SHCreateItemFromParsingName(PCWSTR(Wide.as_ptr()), None) }.ok()
}

fn Execute(Window: HWND, Item: &Path, Verb: Option<&str>) -> Result<(), String> {
    let File = ShellPath(Item);
    let Directory = Item.parent().map(ShellPath);
    let VerbWide = Verb.map(WideString);
    let mut Info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_FLAG_NO_UI,
        hwnd: Window,
        lpVerb: VerbWide.as_ref().map(|Text| PCWSTR(Text.as_ptr())).unwrap_or(PCWSTR::null()),
        lpFile: PCWSTR(File.as_ptr()),
        lpDirectory: Directory.as_ref().map(|Text| PCWSTR(Text.as_ptr())).unwrap_or(PCWSTR::null()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut Info) }.map_err(|Error| Error.message())
}

pub fn OpenItem(Window: HWND, Item: &Path) -> Result<(), String> {
    Execute(Window, Item, None)
}

pub fn RunAsAdministrator(Window: HWND, Item: &Path) -> Result<(), String> {
    Execute(Window, Item, Some("runas"))
}

pub fn OpenWithDialog(Window: HWND, Item: &Path) -> Result<(), String> {
    let File = ShellPath(Item);
    let Info = OPENASINFO { pcszFile: PCWSTR(File.as_ptr()), pcszClass: PCWSTR::null(), oaifInFlags: OAIF_EXEC | OAIF_ALLOW_REGISTRATION };
    unsafe { SHOpenWithDialog(Some(Window), &Info) }.map_err(|Error| Error.message())
}

pub fn ShowProperties(Window: HWND, Items: &[PathBuf]) -> Result<(), String> {
    if Items.len() == 1 {
        let File = ShellPath(&Items[0]);
        let Shown = unsafe { SHObjectProperties(Some(Window), SHOP_FILEPATH, PCWSTR(File.as_ptr()), PCWSTR::null()) };
        return if Shown.as_bool() { Ok(()) } else { Err("The properties dialog could not be opened".to_string()) };
    }
    let (Object, _Pidls) = DataObject(Items).ok_or_else(|| "The selected items could not be resolved by the shell".to_string())?;
    unsafe { SHMultiFileProperties(&Object, 0) }.map_err(|Error| Error.message())
}

fn KnownFolder(Identifier: &GUID) -> Option<PathBuf> {
    unsafe {
        let Raw = SHGetKnownFolderPath(Identifier, KF_FLAG_DEFAULT, None).ok()?;
        let Path = PathBuf::from(OsString::from_wide(Raw.as_wide()));
        CoTaskMemFree(Some(Raw.0 as *const _));
        Some(Path)
    }
}

pub fn KnownFolders() -> Vec<(String, PathBuf)> {
    let Folders = [
        ("Home", FOLDERID_Profile),
        ("Desktop", FOLDERID_Desktop),
        ("Documents", FOLDERID_Documents),
        ("Downloads", FOLDERID_Downloads),
        ("Pictures", FOLDERID_Pictures),
        ("Music", FOLDERID_Music),
        ("Videos", FOLDERID_Videos),
    ];
    Folders.iter().filter_map(|(Name, Identifier)| KnownFolder(Identifier).map(|Path| (Name.to_string(), Path))).collect()
}

pub fn IconIndex(Item: &Path, Attributes: u32, AttributesOnly: bool) -> i32 {
    let Wide = if AttributesOnly { WideNull(Item.file_name().unwrap_or(Item.as_os_str())) } else { ShellPath(Item) };
    let mut Info = SHFILEINFOW::default();
    let Flags = if AttributesOnly { SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES } else { SHGFI_SYSICONINDEX };
    let Result = unsafe {
        SHGetFileInfoW(PCWSTR(Wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(Attributes & 0x3FFFF), Some(&mut Info), std::mem::size_of::<SHFILEINFOW>() as u32, Flags)
    };
    if Result == 0 { -1 } else { Info.iIcon }
}

pub fn ExtensionIconIndex(Extension: &[u16], IsDirectory: bool) -> i32 {
    let mut Name: Vec<u16> = "f".encode_utf16().collect();
    if !IsDirectory && !Extension.is_empty() {
        Name.push(b'.' as u16);
        Name.extend_from_slice(Extension);
    }
    let Attributes = if IsDirectory { AttributeDirectory } else { 0x80 };
    IconIndex(&PathBuf::from(OsString::from_wide(&Name)), Attributes, true)
}

pub fn TypeName(Extension: &[u16], IsDirectory: bool) -> String {
    let mut Name: Vec<u16> = "f".encode_utf16().collect();
    if !IsDirectory && !Extension.is_empty() {
        Name.push(b'.' as u16);
        Name.extend_from_slice(Extension);
    }
    Name.push(0);
    let mut Info = SHFILEINFOW::default();
    let Attributes = if IsDirectory { AttributeDirectory } else { 0x80 };
    let Result = unsafe {
        SHGetFileInfoW(PCWSTR(Name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(Attributes), Some(&mut Info), std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES)
    };
    let Length = Info.szTypeName.iter().position(|&Unit| Unit == 0).unwrap_or(0);
    if Result == 0 || Length == 0 {
        if IsDirectory {
            "File folder".to_string()
        } else if Extension.is_empty() {
            "File".to_string()
        } else {
            format!("{} File", String::from_utf16_lossy(Extension).to_uppercase())
        }
    } else {
        String::from_utf16_lossy(&Info.szTypeName[..Length])
    }
}

pub fn PickFolder(Window: HWND, Title: &str) -> Option<PathBuf> {
    unsafe {
        let Dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let Options = Dialog.GetOptions().ok()?;
        Dialog.SetOptions(Options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let TitleWide = WideString(Title);
        let _ = Dialog.SetTitle(PCWSTR(TitleWide.as_ptr()));
        Dialog.Show(Some(Window)).ok()?;
        let Item = Dialog.GetResult().ok()?;
        let Raw = Item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let Path = PathBuf::from(OsString::from_wide(Raw.as_wide()));
        CoTaskMemFree(Some(Raw.0 as *const _));
        Some(Path)
    }
}

fn FindOnPath(Program: &str) -> Option<PathBuf> {
    let Paths = std::env::var_os("PATH")?;
    std::env::split_paths(&Paths).map(|Directory| Directory.join(Program)).find(|Candidate| Candidate.is_file())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terminal {
    WindowsTerminal,
    PowerShellCore,
    PowerShell,
    CommandPrompt,
}

impl Terminal {
    pub fn Label(self) -> &'static str {
        match self {
            Terminal::WindowsTerminal => "Windows Terminal",
            Terminal::PowerShellCore => "PowerShell 7",
            Terminal::PowerShell => "Windows PowerShell",
            Terminal::CommandPrompt => "Command Prompt",
        }
    }

    fn Executable(self) -> Option<PathBuf> {
        match self {
            Terminal::WindowsTerminal => FindOnPath("wt.exe").or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|Local| PathBuf::from(Local).join("Microsoft\\WindowsApps\\wt.exe")).filter(|Candidate| Candidate.exists())
            }),
            Terminal::PowerShellCore => FindOnPath("pwsh.exe"),
            Terminal::PowerShell => FindOnPath("powershell.exe"),
            Terminal::CommandPrompt => FindOnPath("cmd.exe").or_else(|| Some(PathBuf::from("cmd.exe"))),
        }
    }
}

static Terminals: std::sync::LazyLock<Vec<(Terminal, PathBuf)>> = std::sync::LazyLock::new(|| {
    [Terminal::WindowsTerminal, Terminal::PowerShellCore, Terminal::PowerShell, Terminal::CommandPrompt]
        .into_iter()
        .filter_map(|Kind| Kind.Executable().map(|Program| (Kind, Program)))
        .collect()
});

pub fn AvailableTerminals() -> Vec<Terminal> {
    Terminals.iter().map(|(Kind, _)| *Kind).collect()
}

pub fn OpenTerminal(Directory: &Path, Kind: Terminal) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let Text = crate::FileSystem::PathFromWide(&ShellPath(Directory));
    let Program = Terminals.iter().find(|(Candidate, _)| *Candidate == Kind).map(|(_, Program)| Program.clone()).ok_or_else(|| format!("{} is not installed", Kind.Label()))?;
    let Quoted = Text.to_string_lossy().replace('\'', "''");
    let mut Process = std::process::Command::new(Program);
    match Kind {
        Terminal::WindowsTerminal => {
            Process.arg("-d").arg(&Text);
        }
        Terminal::PowerShellCore | Terminal::PowerShell => {
            Process.raw_arg(format!("-NoExit -Command \"Set-Location -LiteralPath '{Quoted}'\"")).creation_flags(0x00000010);
        }
        Terminal::CommandPrompt => {
            Process.raw_arg(format!("/K pushd \"{}\"", Text.display())).creation_flags(0x00000010);
        }
    }
    if !Text.to_string_lossy().starts_with("\\\\") {
        Process.current_dir(&Text);
    }
    Process.spawn().map(|_| ()).map_err(|Error| Error.to_string())
}

pub fn LaunchTerminal(Directory: &Path, Template: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let Text = crate::FileSystem::PathFromWide(&ShellPath(Directory));
    if let Some(Label) = Template.strip_prefix("terminal:") {
        if let Some(Kind) = AvailableTerminals().into_iter().find(|Kind| Kind.Label() == Label) {
            return OpenTerminal(Directory, Kind);
        }
    }
    if !Template.trim().is_empty() && !Template.starts_with("terminal:") {
        let CommandLine = Template.replace("{path}", &Text.to_string_lossy());
        return std::process::Command::new("cmd.exe")
            .raw_arg(format!("/C start \"\" {CommandLine}"))
            .current_dir(&Text)
            .creation_flags(0x08000000)
            .spawn()
            .map(|_| ())
            .map_err(|Error| Error.to_string());
    }
    let Preferred = AvailableTerminals().into_iter().next().unwrap_or(Terminal::CommandPrompt);
    OpenTerminal(Directory, Preferred)
}

pub fn OpenInExplorer(Item: &Path, Select: bool) -> Result<(), String> {
    let Text = crate::FileSystem::PathFromWide(&ShellPath(Item));
    let mut Process = std::process::Command::new("explorer.exe");
    if Select {
        use std::os::windows::process::CommandExt;
        Process.raw_arg(format!("/select,\"{}\"", Text.display()));
    } else {
        Process.arg(&Text);
    }
    Process.spawn().map(|_| ()).map_err(|Error| Error.to_string())
}

pub fn OpenInExplorerRaw(Text: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut Process = std::process::Command::new("explorer.exe");
    if !Text.trim().is_empty() {
        Process.raw_arg(format!("\"{}\"", Text.trim().trim_matches('"')));
    }
    Process.spawn().map(|_| ()).map_err(|Error| Error.to_string())
}

pub fn IsArchive(Item: &Path) -> bool {
    use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_STREAM};
    let Ok(Shell) = ShellItem(Item).ok_or(()) else { return false };
    let Wanted = SFGAO_FOLDER | SFGAO_STREAM;
    unsafe { Shell.GetAttributes(Wanted) }.is_ok_and(|Flags| Flags.0 & Wanted.0 == Wanted.0)
}

pub fn ShortcutTarget(Item: &Path) -> Option<PathBuf> {
    use windows::Win32::System::Com::{IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;
    unsafe {
        let Link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let File: IPersistFile = Link.cast().ok()?;
        let Wide = ShellPath(Item);
        File.Load(PCWSTR(Wide.as_ptr()), STGM_READ).ok()?;
        let mut Buffer = vec![0u16; 32768];
        Link.GetPath(&mut Buffer, std::ptr::null_mut(), 0).ok()?;
        let Target = crate::FileSystem::PathFromWide(&Buffer);
        if Target.as_os_str().is_empty() { None } else { Some(Target) }
    }
}
