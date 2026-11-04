use FluxSdk::Utilities::{HashAlgorithm, Hasher, Hex};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

pub const Extension: &str = "idx";
const Magic: &[u8; 8] = b"FLUXIDX1";
const CountOffset: u64 = 16;
const FlagDirectory: u8 = 1;
const UnixEpochTicks: u64 = 116_444_736_000_000_000;

pub struct Entry {
    pub Relative: Box<[u16]>,
    pub Lower: Box<str>,
    pub Size: u64,
    pub IsDirectory: bool,
}

pub struct Header {
    pub Root: PathBuf,
    pub BuiltAt: u64,
    pub Count: u64,
}

pub struct IndexData {
    pub Header: Header,
    pub RootLower: String,
    pub Entries: Vec<Entry>,
}

struct CacheEntry {
    Modified: SystemTime,
    Length: u64,
    Data: Arc<IndexData>,
}

static Cache: LazyLock<Mutex<HashMap<PathBuf, CacheEntry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn LockCache() -> MutexGuard<'static, HashMap<PathBuf, CacheEntry>> {
    Cache.lock().unwrap_or_else(|Poisoned| Poisoned.into_inner())
}

pub fn NowTicks() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|Elapsed| (Elapsed.as_nanos() / 100) as u64 + UnixEpochTicks)
        .unwrap_or(UnixEpochTicks)
}

pub fn TicksToUnixSeconds(Ticks: u64) -> i64 {
    (Ticks as i64 - UnixEpochTicks as i64).div_euclid(10_000_000)
}

pub fn IndexDirectory() -> io::Result<PathBuf> {
    let Base = env::var_os("LOCALAPPDATA").filter(|Value| !Value.is_empty()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "the LOCALAPPDATA environment variable is not set")
    })?;
    Ok(PathBuf::from(Base).join("Flux").join("Index"))
}

pub fn NormalizeRoot(Root: &Path) -> PathBuf {
    let Text = Root.as_os_str().encode_wide().collect::<Vec<u16>>();
    let mut Length = Text.len();
    while Length > 3 && (Text[Length - 1] == b'\\' as u16 || Text[Length - 1] == b'/' as u16) {
        Length -= 1;
    }
    PathBuf::from(OsString::from_wide(&Text[..Length]))
}

pub fn IndexFile(Directory: &Path, Root: &Path) -> io::Result<PathBuf> {
    let Key = Root.to_string_lossy().to_lowercase();
    let mut State = Hasher::New(HashAlgorithm::Sha256)?;
    State.Update(Key.as_bytes())?;
    let Digest = State.Finish()?;
    Ok(Directory.join(format!("{}.{}", Hex(&Digest), Extension)))
}

pub struct Writer {
    Output: BufWriter<File>,
    Count: u64,
}

impl Writer {
    pub fn Create(Target: &Path, Root: &Path, BuiltAt: u64) -> io::Result<Self> {
        let mut Output = BufWriter::with_capacity(1 << 20, File::create(Target)?);
        let RootWide: Vec<u16> = Root.as_os_str().encode_wide().collect();
        let RootLength = u32::try_from(RootWide.len()).map_err(|_| io::Error::other("root path is too long"))?;
        Output.write_all(Magic)?;
        Output.write_all(&BuiltAt.to_le_bytes())?;
        Output.write_all(&0u64.to_le_bytes())?;
        Output.write_all(&RootLength.to_le_bytes())?;
        for Unit in RootWide {
            Output.write_all(&Unit.to_le_bytes())?;
        }
        Ok(Self { Output, Count: 0 })
    }

    pub fn Add(&mut self, Relative: &[u16], Size: u64, Modified: u64, IsDirectory: bool) -> io::Result<()> {
        let Length = u16::try_from(Relative.len()).map_err(|_| io::Error::other("path is too long"))?;
        self.Output.write_all(&[if IsDirectory { FlagDirectory } else { 0 }])?;
        self.Output.write_all(&Length.to_le_bytes())?;
        for Unit in Relative {
            self.Output.write_all(&Unit.to_le_bytes())?;
        }
        self.Output.write_all(&Size.to_le_bytes())?;
        self.Output.write_all(&Modified.to_le_bytes())?;
        self.Count += 1;
        Ok(())
    }

    pub fn Finish(self) -> io::Result<u64> {
        let mut Target = self.Output.into_inner().map_err(|Error| Error.into_error())?;
        Target.seek(SeekFrom::Start(CountOffset))?;
        Target.write_all(&self.Count.to_le_bytes())?;
        Target.sync_all()?;
        Ok(self.Count)
    }
}

struct Cursor<'a> {
    Data: &'a [u8],
    Position: usize,
}

impl<'a> Cursor<'a> {
    fn Take(&mut self, Count: usize) -> io::Result<&'a [u8]> {
        let End = self.Position.checked_add(Count).filter(|End| *End <= self.Data.len());
        let End = End.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "index file is truncated"))?;
        let Slice = &self.Data[self.Position..End];
        self.Position = End;
        Ok(Slice)
    }

    fn U8(&mut self) -> io::Result<u8> {
        Ok(self.Take(1)?[0])
    }

    fn U16(&mut self) -> io::Result<u16> {
        let Bytes = self.Take(2)?;
        Ok(u16::from_le_bytes([Bytes[0], Bytes[1]]))
    }

    fn U32(&mut self) -> io::Result<u32> {
        let Bytes = self.Take(4)?;
        Ok(u32::from_le_bytes([Bytes[0], Bytes[1], Bytes[2], Bytes[3]]))
    }

    fn U64(&mut self) -> io::Result<u64> {
        let mut Bytes = [0u8; 8];
        Bytes.copy_from_slice(self.Take(8)?);
        Ok(u64::from_le_bytes(Bytes))
    }

    fn Wide(&mut self, Units: usize) -> io::Result<Vec<u16>> {
        let Bytes = self.Take(Units.checked_mul(2).ok_or_else(|| io::Error::other("length overflow"))?)?;
        Ok(Bytes.chunks_exact(2).map(|Pair| u16::from_le_bytes([Pair[0], Pair[1]])).collect())
    }
}

fn ParseHeader(Reader: &mut Cursor) -> io::Result<Header> {
    if Reader.Take(Magic.len())? != Magic {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a Flux index file"));
    }
    let BuiltAt = Reader.U64()?;
    let Count = Reader.U64()?;
    let RootLength = Reader.U32()? as usize;
    let Root = PathBuf::from(OsString::from_wide(&Reader.Wide(RootLength)?));
    Ok(Header { Root, BuiltAt, Count })
}

pub fn ReadHeader(Item: &Path) -> io::Result<Header> {
    let mut Source = BufReader::new(File::open(Item)?);
    let mut Fixed = [0u8; 28];
    Source.read_exact(&mut Fixed)?;
    let RootLength = u32::from_le_bytes([Fixed[24], Fixed[25], Fixed[26], Fixed[27]]) as usize;
    let mut Bytes = Fixed.to_vec();
    Bytes.resize(Fixed.len() + RootLength * 2, 0);
    Source.read_exact(&mut Bytes[Fixed.len()..])?;
    ParseHeader(&mut Cursor { Data: &Bytes, Position: 0 })
}

fn Load(Item: &Path) -> io::Result<IndexData> {
    let Bytes = fs::read(Item)?;
    let mut Reader = Cursor { Data: &Bytes, Position: 0 };
    let Header = ParseHeader(&mut Reader)?;
    let Capacity = usize::try_from(Header.Count).unwrap_or(0).min(Bytes.len() / 19);
    let mut Entries = Vec::with_capacity(Capacity);
    for _ in 0..Header.Count {
        let Flags = Reader.U8()?;
        let Length = Reader.U16()? as usize;
        let Relative = Reader.Wide(Length)?;
        let Size = Reader.U64()?;
        Reader.Take(8)?;
        let Lower = String::from_utf16_lossy(&Relative).to_lowercase().into_boxed_str();
        Entries.push(Entry { Relative: Relative.into_boxed_slice(), Lower, Size, IsDirectory: Flags & FlagDirectory != 0 });
    }
    if Reader.Position != Bytes.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "index file has trailing data"));
    }
    let RootLower = Header.Root.to_string_lossy().to_lowercase();
    Ok(IndexData { Header, RootLower, Entries })
}

pub fn IndexFiles(Directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut Files = Vec::new();
    let Reader = match fs::read_dir(Directory) {
        Ok(Reader) => Reader,
        Err(Error) if Error.kind() == io::ErrorKind::NotFound => return Ok(Files),
        Err(Error) => return Err(Error),
    };
    for Item in Reader {
        let ItemPath = Item?.path();
        if ItemPath.extension().is_some_and(|Value| Value.eq_ignore_ascii_case(Extension)) && ItemPath.is_file() {
            Files.push(ItemPath);
        }
    }
    Files.sort();
    Ok(Files)
}

pub fn Cached(Item: &Path) -> io::Result<Arc<IndexData>> {
    let Data = fs::metadata(Item)?;
    let Modified = Data.modified()?;
    let Length = Data.len();
    if let Some(Existing) = LockCache().get(Item)
        && Existing.Modified == Modified
        && Existing.Length == Length
    {
        return Ok(Existing.Data.clone());
    }
    let Loaded = Arc::new(Load(Item)?);
    LockCache().insert(Item.to_path_buf(), CacheEntry { Modified, Length, Data: Loaded.clone() });
    Ok(Loaded)
}

pub fn Retain(Files: &[PathBuf]) {
    LockCache().retain(|Key, _| Files.contains(Key));
}

pub fn Forget(Item: &Path) {
    LockCache().remove(Item);
}

pub fn Clear() {
    LockCache().clear();
}
