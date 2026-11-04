use super::{ApiPath, AttributeDirectory, DriveDetails, Listing, RawEntry, WideNull, WideString};
use crate::Services::Workers::CancelToken;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FULL_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, FileFullDirectoryInfo, FileFullDirectoryRestartInfo, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetFileInformationByHandleEx, GetLogicalDrives, GetVolumeInformationW, NetShareEnum, OPEN_EXISTING,
    SHARE_INFO_1, STYPE_DISKTREE, WIN32_FIND_DATAW,
};
use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE};
use windows::core::{Owned, PCWSTR};

pub const BufferSize: usize = 64 * 1024;

pub fn Win32Code(Error: &windows::core::Error) -> u32 {
    let Code = Error.code().0 as u32;
    if (Code >> 16) & 0x1FFF == 7 { Code & 0xFFFF } else { Code }
}

fn FileTimeValue(Time: windows::Win32::Foundation::FILETIME) -> i64 {
    ((Time.dwHighDateTime as i64) << 32) | Time.dwLowDateTime as i64
}

fn IsDots(Name: &[u16]) -> bool {
    Name == [b'.' as u16] || Name == [b'.' as u16, b'.' as u16]
}

pub fn ForEach(Directory: &Path, Buffer: &mut Vec<u8>, Visit: &mut dyn FnMut(&RawEntry) -> bool) -> Result<(), u32> {
    let Wide = ApiPath(Directory);
    let Opened = unsafe {
        CreateFileW(
            PCWSTR(Wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let Handle = match Opened {
        Ok(Handle) => unsafe { Owned::new(Handle) },
        Err(Error) => return Err(Win32Code(&Error)),
    };
    if Buffer.len() < BufferSize {
        Buffer.resize(BufferSize, 0);
    }
    let mut First = true;
    loop {
        let Class = if First { FileFullDirectoryRestartInfo } else { FileFullDirectoryInfo };
        let Result = unsafe { GetFileInformationByHandleEx(*Handle, Class, Buffer.as_mut_ptr() as *mut _, Buffer.len() as u32) };
        if let Err(Error) = Result {
            let Code = Win32Code(&Error);
            if Code == ERROR_NO_MORE_FILES.0 {
                return Ok(());
            }
            if First && (Code == 1 || Code == 50 || Code == 87 || Code == 124) {
                drop(Handle);
                return ForEachFallback(Directory, Visit);
            }
            return Err(Code);
        }
        First = false;
        let mut Offset = 0usize;
        loop {
            let Record = unsafe { &*(Buffer.as_ptr().add(Offset) as *const FILE_FULL_DIR_INFO) };
            let Name = unsafe { std::slice::from_raw_parts(Record.FileName.as_ptr(), Record.FileNameLength as usize / 2) };
            if !IsDots(Name) {
                let Raw = RawEntry {
                    Name,
                    Attributes: Record.FileAttributes,
                    Size: if Record.FileAttributes & AttributeDirectory != 0 { 0 } else { Record.EndOfFile as u64 },
                    Modified: Record.LastWriteTime,
                    Created: Record.CreationTime,
                };
                if !Visit(&Raw) {
                    return Ok(());
                }
            }
            if Record.NextEntryOffset == 0 {
                break;
            }
            Offset += Record.NextEntryOffset as usize;
        }
    }
}

struct FindHandle(HANDLE);

impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = FindClose(self.0);
        }
    }
}

fn FindDataEntry(Data: &WIN32_FIND_DATAW) -> (usize, RawEntry<'_>) {
    let Length = Data.cFileName.iter().position(|&Unit| Unit == 0).unwrap_or(Data.cFileName.len());
    (
        Length,
        RawEntry {
            Name: &Data.cFileName[..Length],
            Attributes: Data.dwFileAttributes,
            Size: if Data.dwFileAttributes & AttributeDirectory != 0 { 0 } else { ((Data.nFileSizeHigh as u64) << 32) | Data.nFileSizeLow as u64 },
            Modified: FileTimeValue(Data.ftLastWriteTime),
            Created: FileTimeValue(Data.ftCreationTime),
        },
    )
}

fn ForEachFallback(Directory: &Path, Visit: &mut dyn FnMut(&RawEntry) -> bool) -> Result<(), u32> {
    let mut Pattern = ApiPath(Directory);
    Pattern.pop();
    if Pattern.last() != Some(&(b'\\' as u16)) {
        Pattern.push(b'\\' as u16);
    }
    Pattern.extend([b'*' as u16, 0]);
    let mut Data = WIN32_FIND_DATAW::default();
    let Handle = unsafe {
        FindFirstFileExW(PCWSTR(Pattern.as_ptr()), FindExInfoBasic, &mut Data as *mut _ as *mut _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH)
    }
    .map_err(|Error| Win32Code(&Error))?;
    let Guard = FindHandle(Handle);
    loop {
        let (_, Raw) = FindDataEntry(&Data);
        if !IsDots(Raw.Name) && !Visit(&Raw) {
            return Ok(());
        }
        if let Err(Error) = unsafe { FindNextFileW(Guard.0, &mut Data) } {
            let Code = Win32Code(&Error);
            return if Code == ERROR_NO_MORE_FILES.0 { Ok(()) } else { Err(Code) };
        }
    }
}

pub struct OwnedEntry {
    pub Name: Vec<u16>,
    pub Attributes: u32,
    pub Size: u64,
    pub Modified: i64,
    pub Created: i64,
}

pub fn Stat(Item: &Path) -> Option<OwnedEntry> {
    let Wide = ApiPath(Item);
    let mut Data = WIN32_FIND_DATAW::default();
    let Handle = unsafe { FindFirstFileExW(PCWSTR(Wide.as_ptr()), FindExInfoBasic, &mut Data as *mut _ as *mut _, FindExSearchNameMatch, None, Default::default()) }.ok()?;
    let _Guard = FindHandle(Handle);
    let (_, Raw) = FindDataEntry(&Data);
    Some(OwnedEntry { Name: Raw.Name.to_vec(), Attributes: Raw.Attributes, Size: Raw.Size, Modified: Raw.Modified, Created: Raw.Created })
}

pub fn Exists(Item: &Path) -> bool {
    Stat(Item).is_some()
}

pub fn LoadDirectory(
    Directory: &Path,
    Cancel: &CancelToken,
    Partial: &mut dyn FnMut(&Listing, usize),
) -> Result<Listing, u32> {
    let mut Data = Listing::New(Some(Directory.to_path_buf()));
    let mut Buffer = Vec::new();
    let Started = Instant::now();
    let mut Reported = 0usize;
    let mut NextReport = Duration::from_millis(60);
    let mut Cancelled = false;
    ForEach(Directory, &mut Buffer, &mut |Raw| {
        Data.Push(&[], Raw);
        if Data.Entries.len() & 1023 == 0 {
            if Cancel.IsCancelled() {
                Cancelled = true;
                return false;
            }
            let Elapsed = Started.elapsed();
            if Elapsed >= NextReport {
                Partial(&Data, Reported);
                Reported = Data.Entries.len();
                NextReport = Elapsed + Duration::from_millis(150);
            }
        }
        true
    })?;
    if Cancelled {
        return Err(1223);
    }
    Data.Text.shrink_to_fit();
    Data.Entries.shrink_to_fit();
    Ok(Data)
}

pub fn LoadComputer() -> Listing {
    let mut Data = Listing::New(None);
    let Mask = unsafe { GetLogicalDrives() };
    for Letter in 0..26u32 {
        if Mask & (1 << Letter) == 0 {
            continue;
        }
        let Root = format!("{}:\\", (b'A' + Letter as u8) as char);
        let RootWide = WideString(&Root);
        let Kind = unsafe { GetDriveTypeW(PCWSTR(RootWide.as_ptr())) };
        let mut Label = [0u16; 261];
        let mut FileSystem = [0u16; 261];
        let HasVolume = unsafe { GetVolumeInformationW(PCWSTR(RootWide.as_ptr()), Some(&mut Label), None, None, None, Some(&mut FileSystem)) }.is_ok();
        let (mut Free, mut Total) = (0u64, 0u64);
        if HasVolume {
            unsafe {
                let _ = GetDiskFreeSpaceExW(PCWSTR(RootWide.as_ptr()), Some(&mut Free), Some(&mut Total), None);
            }
        }
        let TextOf = |Buffer: &[u16]| String::from_utf16_lossy(&Buffer[..Buffer.iter().position(|&Unit| Unit == 0).unwrap_or(0)]);
        let VolumeLabel = TextOf(&Label);
        let KindName = match Kind {
            DRIVE_FIXED => "Local Disk",
            DRIVE_REMOVABLE => "Removable Disk",
            DRIVE_REMOTE => "Network Drive",
            DRIVE_CDROM => "Optical Drive",
            DRIVE_RAMDISK => "RAM Disk",
            _ => "Drive",
        };
        let Display = format!("{} ({})", if VolumeLabel.is_empty() { KindName } else { &VolumeLabel }, &Root[..2]);
        let Name: Vec<u16> = Root.encode_utf16().collect();
        Data.Push(&[], &RawEntry { Name: &Name, Attributes: AttributeDirectory, Size: Total, Modified: 0, Created: 0 });
        Data.Drives.push(DriveDetails { Label: Display, FileSystem: TextOf(&FileSystem), Total, Free });
    }
    Data
}

pub fn LoadServer(Server: &Path) -> Result<Listing, u32> {
    let ServerName = WideNull(Server.as_os_str());
    let mut Buffer: *mut u8 = std::ptr::null_mut();
    let (mut Read, mut Total) = (0u32, 0u32);
    let Status = unsafe { NetShareEnum(PCWSTR(ServerName.as_ptr()), 1, &mut Buffer, u32::MAX, &mut Read, &mut Total, None) };
    if Status != 0 && Status != 234 {
        return Err(Status);
    }
    let mut Data = Listing::New(Some(Server.to_path_buf()));
    if !Buffer.is_null() {
        let Shares = unsafe { std::slice::from_raw_parts(Buffer as *const SHARE_INFO_1, Read as usize) };
        for Share in Shares {
            if Share.shi1_type.0 & 0xFF != STYPE_DISKTREE.0 || Share.shi1_netname.is_null() {
                continue;
            }
            let Name = unsafe { Share.shi1_netname.as_wide() };
            if Name.last() == Some(&(b'$' as u16)) {
                continue;
            }
            Data.Push(&[], &RawEntry { Name, Attributes: AttributeDirectory, Size: 0, Modified: 0, Created: 0 });
        }
        unsafe {
            NetApiBufferFree(Some(Buffer as *const _));
        }
    }
    Ok(Data)
}

pub fn Subdirectories(Directory: &Path, IncludeHidden: bool, Limit: usize) -> Result<Vec<OsString>, u32> {
    let mut Names: Vec<Vec<u16>> = Vec::new();
    let mut Buffer = Vec::new();
    ForEach(Directory, &mut Buffer, &mut |Raw| {
        if Raw.Attributes & AttributeDirectory != 0 && (IncludeHidden || Raw.Attributes & super::AttributeHidden == 0) {
            Names.push(Raw.Name.to_vec());
        }
        Names.len() < Limit
    })?;
    Names.sort_by(|Left, Right| super::Sorting::CompareNatural(Left, Right));
    Ok(Names.iter().map(|Name| OsString::from_wide(Name)).collect())
}

pub fn FreeSpace(Item: &Path) -> Option<(u64, u64)> {
    let Wide = super::ShellPath(Item);
    let (mut Free, mut Total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(Wide.as_ptr()), Some(&mut Free), Some(&mut Total), None) }.ok()?;
    Some((Free, Total))
}

pub fn DirectorySize(Directory: &Path, Cancel: &CancelToken, Progress: &mut dyn FnMut(u64, u64)) -> Option<(u64, u64, u64)> {
    let mut Pending: Vec<PathBuf> = vec![Directory.to_path_buf()];
    let (mut Bytes, mut Files, mut Folders) = (0u64, 0u64, 0u64);
    let mut Buffer = Vec::new();
    let mut LastReport = Instant::now();
    while let Some(Current) = Pending.pop() {
        if Cancel.IsCancelled() {
            return None;
        }
        let _ = ForEach(&Current, &mut Buffer, &mut |Raw| {
            if Raw.Attributes & AttributeDirectory != 0 {
                Folders += 1;
                if Raw.Attributes & super::AttributeReparsePoint == 0 {
                    Pending.push(Current.join(OsString::from_wide(Raw.Name)));
                }
            } else {
                Files += 1;
                Bytes += Raw.Size;
            }
            true
        });
        if LastReport.elapsed() > Duration::from_millis(200) {
            Progress(Bytes, Files);
            LastReport = Instant::now();
        }
    }
    Some((Bytes, Files, Folders))
}
