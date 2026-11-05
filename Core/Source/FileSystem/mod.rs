pub mod Enumerate;
pub mod Operations;
pub mod Search;
pub mod Sizes;
pub mod Sorting;
pub mod Watcher;

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::core::PCWSTR;

pub const AttributeReadOnly: u32 = 0x1;
pub const AttributeHidden: u32 = 0x2;
pub const AttributeSystem: u32 = 0x4;
pub const AttributeDirectory: u32 = 0x10;
pub const AttributeArchive: u32 = 0x20;
pub const AttributeCompressed: u32 = 0x800;
pub const AttributeOffline: u32 = 0x1000;
pub const AttributeEncrypted: u32 = 0x4000;
pub const AttributeReparsePoint: u32 = 0x400;

pub const FlagSelected: u8 = 1;
pub const FlagRemoved: u8 = 2;
pub const FlagSizeKnown: u8 = 4;
pub const FlagSizePartial: u8 = 8;

#[derive(Clone, PartialEq, Debug)]
pub enum Location {
    Computer,
    Directory(PathBuf),
    Search { Root: PathBuf, Query: String },
    Results { Title: String, Identifier: u64 },
}

impl Location {
    pub fn DirectoryPath(&self) -> Option<&Path> {
        match self {
            Location::Directory(Item) => Some(Item),
            Location::Search { Root, .. } => Some(Root),
            _ => None,
        }
    }

    pub fn Title(&self) -> String {
        match self {
            Location::Computer => "This PC".to_string(),
            Location::Directory(Item) => LeafName(Item),
            Location::Search { Query, .. } => format!("Search: {Query}"),
            Location::Results { Title, .. } => Title.clone(),
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Entry {
    pub PathStart: u32,
    pub PathLength: u16,
    pub NameOffset: u16,
    pub ExtensionOffset: u16,
    pub Flags: u8,
    pub Attributes: u32,
    pub Size: u64,
    pub Modified: i64,
    pub Created: i64,
    pub Icon: i32,
}

#[derive(Clone, Default, Debug)]
pub struct DriveDetails {
    pub Label: String,
    pub FileSystem: String,
    pub Total: u64,
    pub Free: u64,
}

#[derive(Default)]
pub struct Listing {
    pub Root: Option<PathBuf>,
    pub Text: Vec<u16>,
    pub Entries: Vec<Entry>,
    pub Drives: Vec<DriveDetails>,
    pub RemovedCount: usize,
    pub Archive: Option<PathBuf>,
}

pub struct RawEntry<'a> {
    pub Name: &'a [u16],
    pub Attributes: u32,
    pub Size: u64,
    pub Modified: i64,
    pub Created: i64,
}

impl Listing {
    pub fn New(Root: Option<PathBuf>) -> Self {
        Self { Root, ..Default::default() }
    }

    pub fn Len(&self) -> usize {
        self.Entries.len()
    }

    pub fn Push(&mut self, Prefix: &[u16], Raw: &RawEntry) -> u32 {
        let PathStart = self.Text.len() as u32;
        let mut NameOffset = 0usize;
        if !Prefix.is_empty() {
            self.Text.extend_from_slice(Prefix);
            if Prefix.last() != Some(&(b'\\' as u16)) {
                self.Text.push(b'\\' as u16);
            }
            NameOffset = self.Text.len() - PathStart as usize;
        }
        self.Text.extend_from_slice(Raw.Name);
        let PathLength = (self.Text.len() - PathStart as usize) as u16;
        let ExtensionOffset = if Raw.Attributes & AttributeDirectory != 0 {
            Raw.Name.len()
        } else {
            match Raw.Name.iter().rposition(|&Unit| Unit == b'.' as u16) {
                Some(Position) => Position + 1,
                None => Raw.Name.len(),
            }
        } as u16;
        self.Entries.push(Entry {
            PathStart,
            PathLength,
            NameOffset: NameOffset as u16,
            ExtensionOffset,
            Flags: 0,
            Attributes: Raw.Attributes,
            Size: Raw.Size,
            Modified: Raw.Modified,
            Created: Raw.Created,
            Icon: -1,
        });
        (self.Entries.len() - 1) as u32
    }

    pub fn RelativePath(&self, Index: u32) -> &[u16] {
        let Item = &self.Entries[Index as usize];
        &self.Text[Item.PathStart as usize..Item.PathStart as usize + Item.PathLength as usize]
    }

    pub fn Name(&self, Index: u32) -> &[u16] {
        let Item = &self.Entries[Index as usize];
        &self.RelativePath(Index)[Item.NameOffset as usize..]
    }

    pub fn Folder(&self, Index: u32) -> &[u16] {
        let Item = &self.Entries[Index as usize];
        let Relative = self.RelativePath(Index);
        let End = (Item.NameOffset as usize).saturating_sub(1);
        &Relative[..End]
    }

    pub fn Extension(&self, Index: u32) -> &[u16] {
        let Item = &self.Entries[Index as usize];
        let Name = self.Name(Index);
        &Name[(Item.ExtensionOffset as usize).min(Name.len())..]
    }

    pub fn IsDirectory(&self, Index: u32) -> bool {
        self.Entries[Index as usize].Attributes & AttributeDirectory != 0
    }

    pub fn FullPath(&self, Index: u32) -> PathBuf {
        let Relative = PathBuf::from(OsString::from_wide(self.RelativePath(Index)));
        match &self.Root {
            Some(Root) => Root.join(Relative),
            None => Relative,
        }
    }

    pub fn NameString(&self, Index: u32) -> String {
        String::from_utf16_lossy(self.Name(Index))
    }

    pub fn Find(&self, Relative: &[u16]) -> Option<u32> {
        (0..self.Entries.len() as u32)
            .find(|&Index| self.Entries[Index as usize].Flags & FlagRemoved == 0 && Sorting::EqualIgnoreCase(self.RelativePath(Index), Relative))
    }

    pub fn Remove(&mut self, Index: u32) {
        let Item = &mut self.Entries[Index as usize];
        if Item.Flags & FlagRemoved == 0 {
            Item.Flags = FlagRemoved;
            self.RemovedCount += 1;
        }
    }

    pub fn Compact(&mut self) {
        if self.RemovedCount == 0 {
            return;
        }
        let mut Text = Vec::with_capacity(self.Text.len());
        let mut Entries = Vec::with_capacity(self.Entries.len() - self.RemovedCount);
        for Item in &self.Entries {
            if Item.Flags & FlagRemoved != 0 {
                continue;
            }
            let Start = Item.PathStart as usize;
            let mut Copy = *Item;
            Copy.PathStart = Text.len() as u32;
            Text.extend_from_slice(&self.Text[Start..Start + Item.PathLength as usize]);
            Entries.push(Copy);
        }
        self.Text = Text;
        self.Entries = Entries;
        self.RemovedCount = 0;
    }
}

pub fn WideNull(Text: &OsStr) -> Vec<u16> {
    Text.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn WideString(Text: &str) -> Vec<u16> {
    Text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn ApiPath(Item: &Path) -> Vec<u16> {
    let Wide: Vec<u16> = Item.as_os_str().encode_wide().collect();
    let Backslash = b'\\' as u16;
    let mut Output = Vec::with_capacity(Wide.len() + 9);
    let IsVerbatim = Wide.len() >= 4 && Wide[0] == Backslash && Wide[1] == Backslash && (Wide[2] == b'?' as u16 || Wide[2] == b'.' as u16) && Wide[3] == Backslash;
    if IsVerbatim {
        Output.extend_from_slice(&Wide);
    } else if Wide.len() >= 2 && Wide[0] == Backslash && Wide[1] == Backslash {
        Output.extend("\\\\?\\UNC\\".encode_utf16());
        Output.extend_from_slice(&Wide[2..]);
    } else if Wide.len() >= 2 && Wide[1] == b':' as u16 {
        Output.extend("\\\\?\\".encode_utf16());
        Output.extend_from_slice(&Wide);
        if Output.len() == 6 {
            Output.push(Backslash);
        }
    } else {
        Output.extend_from_slice(&Wide);
    }
    Output.push(0);
    Output
}

pub fn ShellPath(Item: &Path) -> Vec<u16> {
    let Wide: Vec<u16> = Item.as_os_str().encode_wide().collect();
    let Text = String::from_utf16_lossy(&Wide);
    let Stripped = if let Some(Rest) = Text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{Rest}")
    } else if let Some(Rest) = Text.strip_prefix("\\\\?\\") {
        Rest.to_string()
    } else {
        Text
    };
    WideString(&Stripped)
}

pub fn ExpandEnvironment(Text: &str) -> String {
    if !Text.contains('%') {
        return Text.to_string();
    }
    let Source = WideString(Text);
    let mut Buffer = vec![0u16; 4096];
    let Length = unsafe { ExpandEnvironmentStringsW(PCWSTR(Source.as_ptr()), Some(&mut Buffer)) } as usize;
    if Length == 0 || Length > Buffer.len() {
        return Text.to_string();
    }
    String::from_utf16_lossy(&Buffer[..Length.saturating_sub(1)])
}

pub fn IsServerPath(Item: &Path) -> bool {
    let Text = Item.to_string_lossy();
    match Text.strip_prefix("\\\\") {
        Some(Rest) => {
            let Trimmed = Rest.trim_end_matches('\\');
            !Trimmed.is_empty() && !Trimmed.contains('\\') && !Rest.starts_with('?') && !Rest.starts_with('.')
        }
        None => false,
    }
}

pub fn Resolve(Input: &str, Base: Option<&Path>) -> Option<PathBuf> {
    let Trimmed = Input.trim().trim_matches('"').trim();
    if Trimmed.is_empty() {
        return None;
    }
    let mut Text = ExpandEnvironment(Trimmed).replace('/', "\\");
    if Text == "~" || Text.starts_with("~\\") {
        if let Ok(Profile) = std::env::var("USERPROFILE") {
            Text = format!("{}{}", Profile, &Text[1..]);
        }
    }
    let IsUnc = Text.starts_with("\\\\");
    let IsDrive = Text.len() >= 2 && Text.as_bytes()[1] == b':' && Text.as_bytes()[0].is_ascii_alphabetic();
    let Combined = if IsUnc || IsDrive {
        Text
    } else {
        let Base = Base?;
        if Text.starts_with('\\') {
            let Root = RootOf(Base)?;
            format!("{}{}", Root.trim_end_matches('\\'), Text)
        } else {
            format!("{}\\{}", Base.to_string_lossy().trim_end_matches('\\'), Text)
        }
    };
    Some(Normalize(&Combined))
}

fn RootOf(Item: &Path) -> Option<String> {
    let Text = Item.to_string_lossy();
    if Text.len() >= 2 && Text.as_bytes()[1] == b':' {
        return Some(Text[..2].to_string());
    }
    if let Some(Rest) = Text.strip_prefix("\\\\") {
        let Parts: Vec<&str> = Rest.splitn(3, '\\').collect();
        if Parts.len() >= 2 {
            return Some(format!("\\\\{}\\{}", Parts[0], Parts[1]));
        }
    }
    None
}

pub fn ErrorText(Code: u32) -> String {
    use windows::Win32::System::Diagnostics::Debug::{FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW};
    let mut Buffer = [0u16; 512];
    let Length = unsafe {
        FormatMessageW(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, None, Code, 0, windows::core::PWSTR(Buffer.as_mut_ptr()), Buffer.len() as u32, None)
    } as usize;
    let Text = String::from_utf16_lossy(&Buffer[..Length.min(Buffer.len())]);
    let Trimmed = Text.trim_end_matches(['\r', '\n', ' ', '.']);
    if Trimmed.is_empty() { format!("Error {Code}") } else { format!("{Trimmed} ({Code})") }
}

pub fn Normalize(Text: &str) -> PathBuf {
    let (Prefix, Rest) = if let Some(Rest) = Text.strip_prefix("\\\\?\\UNC\\") {
        let Parts: Vec<&str> = Rest.splitn(3, '\\').collect();
        if Parts.len() < 2 {
            return PathBuf::from(format!("\\\\{}", Rest.trim_end_matches('\\')));
        }
        (format!("\\\\{}\\{}", Parts[0], Parts[1]), Parts.get(2).copied().unwrap_or("").to_string())
    } else if let Some(Rest) = Text.strip_prefix("\\\\?\\") {
        return Normalize(Rest);
    } else if let Some(Rest) = Text.strip_prefix("\\\\") {
        let Parts: Vec<&str> = Rest.splitn(3, '\\').filter(|Part| !Part.is_empty()).collect();
        if Parts.len() < 2 {
            return PathBuf::from(format!("\\\\{}", Parts.first().copied().unwrap_or("")));
        }
        (format!("\\\\{}\\{}", Parts[0], Parts[1]), Parts.get(2).copied().unwrap_or("").to_string())
    } else if Text.len() >= 2 && Text.as_bytes()[1] == b':' {
        (Text[..2].to_ascii_uppercase(), Text[2..].to_string())
    } else {
        (String::new(), Text.to_string())
    };
    let mut Parts: Vec<&str> = Vec::new();
    for Part in Rest.split('\\') {
        match Part {
            "" | "." => {}
            ".." => {
                Parts.pop();
            }
            Other => Parts.push(Other.trim_end_matches(' ')),
        }
    }
    let mut Output = Prefix;
    Output.push('\\');
    Output.push_str(&Parts.join("\\"));
    if Output.len() > 3 && Output.ends_with('\\') && !Output.starts_with("\\\\") {
        Output.pop();
    }
    PathBuf::from(Output)
}

pub fn Parent(Item: &Path) -> Option<PathBuf> {
    if IsServerPath(Item) {
        return None;
    }
    let Text = Item.to_string_lossy();
    if let Some(Rest) = Text.strip_prefix("\\\\") {
        let Trimmed = Rest.trim_end_matches('\\');
        let Parts: Vec<&str> = Trimmed.split('\\').collect();
        if Parts.len() == 2 {
            return Some(PathBuf::from(format!("\\\\{}", Parts[0])));
        }
        if Parts.len() == 3 {
            return Some(PathBuf::from(format!("\\\\{}\\{}\\", Parts[0], Parts[1])));
        }
    }
    Item.parent().map(Path::to_path_buf)
}

pub fn LeafName(Item: &Path) -> String {
    if IsServerPath(Item) {
        return Item.to_string_lossy().trim_end_matches('\\').to_string();
    }
    match Item.file_name() {
        Some(Name) => Name.to_string_lossy().into_owned(),
        None => {
            let Text = Item.to_string_lossy();
            let Trimmed = Text.trim_end_matches('\\');
            match Trimmed.strip_prefix("\\\\") {
                Some(Rest) => Rest.rsplit('\\').next().unwrap_or(Rest).to_string(),
                None => Trimmed.to_string(),
            }
        }
    }
}

pub struct Segment {
    pub Label: String,
    pub Target: PathBuf,
}

pub fn Segments(Item: &Path) -> Vec<Segment> {
    let mut Output = Vec::new();
    let mut Current = Some(Item.to_path_buf());
    while let Some(Path) = Current {
        let Label = LeafName(&Path);
        Current = Parent(&Path);
        Output.push(Segment { Label, Target: Path });
    }
    Output.reverse();
    Output
}

pub fn IsSameOrInside(Item: &Path, Container: &Path) -> bool {
    let Left: Vec<u16> = Item.as_os_str().encode_wide().collect();
    let Right: Vec<u16> = Container.as_os_str().encode_wide().collect();
    let Trimmed = if Right.last() == Some(&(b'\\' as u16)) { &Right[..Right.len() - 1] } else { &Right[..] };
    if Left.len() < Trimmed.len() || !Sorting::EqualIgnoreCase(&Left[..Trimmed.len()], Trimmed) {
        return false;
    }
    Left.len() == Trimmed.len() || Left[Trimmed.len()] == b'\\' as u16
}

pub fn SamePath(Left: &Path, Right: &Path) -> bool {
    let A: Vec<u16> = Left.as_os_str().encode_wide().collect();
    let B: Vec<u16> = Right.as_os_str().encode_wide().collect();
    let TrimA = if A.len() > 3 && A.last() == Some(&(b'\\' as u16)) { &A[..A.len() - 1] } else { &A[..] };
    let TrimB = if B.len() > 3 && B.last() == Some(&(b'\\' as u16)) { &B[..B.len() - 1] } else { &B[..] };
    Sorting::EqualIgnoreCase(TrimA, TrimB)
}

pub fn ArchiveRoot(Item: &Path) -> Option<PathBuf> {
    let Chain: Vec<&Path> = Item.ancestors().collect();
    for Prefix in Chain.into_iter().rev().skip(1) {
        let Wide = ApiPath(Prefix);
        let Attributes = unsafe { windows::Win32::Storage::FileSystem::GetFileAttributesW(PCWSTR(Wide.as_ptr())) };
        if Attributes == u32::MAX {
            return None;
        }
        if Attributes & AttributeDirectory == 0 {
            return Some(Prefix.to_path_buf());
        }
    }
    None
}

pub fn InArchive(Item: &Path) -> bool {
    ArchiveRoot(Item).is_some_and(|Root| !SamePath(&Root, Item))
}

pub fn PathFromWide(Wide: &[u16]) -> PathBuf {
    let End = Wide.iter().position(|&Unit| Unit == 0).unwrap_or(Wide.len());
    PathBuf::from(OsString::from_wide(&Wide[..End]))
}

pub fn ValidateName(Name: &str) -> Result<(), &'static str> {
    let Trimmed = Name.trim_end_matches([' ', '.']);
    if Trimmed.is_empty() {
        return Err("Name cannot be empty");
    }
    if Name.chars().any(|Character| matches!(Character, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (Character as u32) < 32) {
        return Err("Name contains characters that are not allowed: < > : \" / \\ | ? *");
    }
    let Stem = Trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let Reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];
    if Reserved.contains(&Stem.as_str()) {
        return Err("Name is reserved by Windows");
    }
    Ok(())
}

pub fn UniqueName(Directory: &Path, Name: &str, IsDirectory: bool, Exists: &dyn Fn(&Path) -> bool) -> String {
    if !Exists(&Directory.join(Name)) {
        return Name.to_string();
    }
    let (Stem, Extension) = match Name.rfind('.') {
        Some(Position) if !IsDirectory && Position > 0 => (&Name[..Position], &Name[Position..]),
        _ => (Name, ""),
    };
    (2..u32::MAX)
        .map(|Number| format!("{Stem} ({Number}){Extension}"))
        .find(|Candidate| !Exists(&Directory.join(Candidate)))
        .unwrap_or_else(|| Name.to_string())
}
