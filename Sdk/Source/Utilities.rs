use std::fmt::Write as FmtWrite;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_MD5_ALG_HANDLE, BCRYPT_SHA1_ALG_HANDLE, BCRYPT_SHA256_ALG_HANDLE,
    BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
};

pub const AttributeReparsePoint: u32 = 0x400;
pub const AttributeDirectory: u32 = 0x10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
}

impl HashAlgorithm {
    pub fn Name(self) -> &'static str {
        match self {
            HashAlgorithm::Md5 => "MD5",
            HashAlgorithm::Sha1 => "SHA-1",
            HashAlgorithm::Sha256 => "SHA-256",
        }
    }

    fn Handle(self) -> BCRYPT_ALG_HANDLE {
        match self {
            HashAlgorithm::Md5 => BCRYPT_MD5_ALG_HANDLE,
            HashAlgorithm::Sha1 => BCRYPT_SHA1_ALG_HANDLE,
            HashAlgorithm::Sha256 => BCRYPT_SHA256_ALG_HANDLE,
        }
    }

    fn Length(self) -> usize {
        match self {
            HashAlgorithm::Md5 => 16,
            HashAlgorithm::Sha1 => 20,
            HashAlgorithm::Sha256 => 32,
        }
    }
}

pub struct Hasher {
    Handle: BCRYPT_HASH_HANDLE,
    Algorithm: HashAlgorithm,
}

fn StatusResult(Status: windows::Win32::Foundation::NTSTATUS) -> io::Result<()> {
    if Status.0 >= 0 { Ok(()) } else { Err(io::Error::other(format!("BCrypt status 0x{:08X}", Status.0 as u32))) }
}

impl Hasher {
    pub fn New(Algorithm: HashAlgorithm) -> io::Result<Self> {
        let mut Handle = BCRYPT_HASH_HANDLE::default();
        StatusResult(unsafe { BCryptCreateHash(Algorithm.Handle(), &mut Handle, None, None, 0) })?;
        Ok(Self { Handle, Algorithm })
    }

    pub fn Update(&mut self, Data: &[u8]) -> io::Result<()> {
        StatusResult(unsafe { BCryptHashData(self.Handle, Data, 0) })
    }

    pub fn Finish(self) -> io::Result<Vec<u8>> {
        let mut Output = vec![0u8; self.Algorithm.Length()];
        StatusResult(unsafe { BCryptFinishHash(self.Handle, &mut Output, 0) })?;
        Ok(Output)
    }
}

impl Drop for Hasher {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.Handle);
        }
    }
}

pub fn HashFile(
    Item: &Path,
    Algorithm: HashAlgorithm,
    Buffer: &mut Vec<u8>,
    Cancelled: &dyn Fn() -> bool,
    Progress: &mut dyn FnMut(u64),
) -> io::Result<Option<Vec<u8>>> {
    if Buffer.len() < 1 << 20 {
        Buffer.resize(1 << 20, 0);
    }
    let mut Source = File::open(Item)?;
    let mut State = Hasher::New(Algorithm)?;
    loop {
        if Cancelled() {
            return Ok(None);
        }
        let Count = match Source.read(Buffer) {
            Ok(Count) => Count,
            Err(Error) if Error.kind() == io::ErrorKind::Interrupted => continue,
            Err(Error) => return Err(Error),
        };
        if Count == 0 {
            break;
        }
        State.Update(&Buffer[..Count])?;
        Progress(Count as u64);
    }
    State.Finish().map(Some)
}

pub fn Hex(Bytes: &[u8]) -> String {
    let mut Text = String::with_capacity(Bytes.len() * 2);
    for Byte in Bytes {
        let _ = write!(Text, "{Byte:02x}");
    }
    Text
}

pub fn IsReparsePoint(Data: &Metadata) -> bool {
    Data.file_attributes() & AttributeReparsePoint != 0
}

pub enum WalkControl {
    Continue,
    SkipChildren,
    Stop,
}

pub fn Walk(
    Root: &Path,
    Visit: &mut dyn FnMut(&Path, &Metadata, usize) -> WalkControl,
) -> Vec<(PathBuf, io::Error)> {
    let mut Errors = Vec::new();
    let mut Pending: Vec<(PathBuf, usize)> = vec![(Root.to_path_buf(), 0)];
    while let Some((Directory, Depth)) = Pending.pop() {
        let Reader = match fs::read_dir(&Directory) {
            Ok(Reader) => Reader,
            Err(Error) => {
                Errors.push((Directory, Error));
                continue;
            }
        };
        for Item in Reader {
            let Item = match Item {
                Ok(Item) => Item,
                Err(Error) => {
                    Errors.push((Directory.clone(), Error));
                    continue;
                }
            };
            let ItemPath = Item.path();
            let Data = match fs::symlink_metadata(&ItemPath) {
                Ok(Data) => Data,
                Err(Error) => {
                    Errors.push((ItemPath, Error));
                    continue;
                }
            };
            match Visit(&ItemPath, &Data, Depth) {
                WalkControl::Stop => return Errors,
                WalkControl::SkipChildren => continue,
                WalkControl::Continue => {}
            }
            if Data.is_dir() && !IsReparsePoint(&Data) {
                Pending.push((ItemPath, Depth + 1));
            }
        }
    }
    Errors
}

pub fn FormatSizeInto(Output: &mut String, Size: u64) {
    const Units: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    if Size < 1024 {
        let _ = write!(Output, "{Size} B");
        return;
    }
    let mut Value = Size as f64 / 1024.0;
    let mut Unit = 0;
    while Value >= 1024.0 && Unit + 1 < Units.len() {
        Value /= 1024.0;
        Unit += 1;
    }
    if Value >= 100.0 {
        let _ = write!(Output, "{:.0} {}", Value, Units[Unit]);
    } else if Value >= 10.0 {
        let _ = write!(Output, "{:.1} {}", Value, Units[Unit]);
    } else {
        let _ = write!(Output, "{:.2} {}", Value, Units[Unit]);
    }
}

pub fn FormatSize(Size: u64) -> String {
    let mut Output = String::new();
    FormatSizeInto(&mut Output, Size);
    Output
}

pub fn ToWide(Text: &str) -> Vec<u16> {
    Text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn EqualIgnoreCase(Left: &str, Right: &str) -> bool {
    Left.len() == Right.len() && Left.chars().zip(Right.chars()).all(|(A, B)| A.to_lowercase().eq(B.to_lowercase()))
}
