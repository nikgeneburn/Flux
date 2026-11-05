use crate::Application::{App, Event};
use crate::Interface::Overlays::{Prompt, PromptPurpose};
use crate::Platform;
use crate::Services::Workers::Spawn;
use std::ffi::c_void;
use std::fs;
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::PCWSTR;

const LatestRelease: &str = "https://api.github.com/repos/nikgeneburn/Flux/releases/latest";
const AssetPrefix: &str = "Flux-";
const AssetSuffix: &str = "-windows-x64.zip";
const CheckInterval: u64 = 12 * 60 * 60;
const TickMilliseconds: u32 = 60 * 60 * 1000;
const MetadataLimit: usize = 4 << 20;
const PackageLimit: u64 = 512 << 20;
const CreateNoWindow: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Release {
    pub Version: String,
    pub Download: String,
    pub Digest: Option<String>,
}

pub enum UpdateEvent {
    Checked { Found: Option<Release>, Manual: bool },
    CheckFailed { Message: String, Manual: bool },
    Installed(PathBuf),
    InstallFailed(String),
}

pub fn CurrentVersion() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn Supported() -> bool {
    !cfg!(debug_assertions) && std::env::current_exe().is_ok_and(|Executable| !Executable.components().any(|Part| Part.as_os_str().eq_ignore_ascii_case("target")))
}

fn Now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|Elapsed| Elapsed.as_secs()).unwrap_or(0)
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn Wide(Text: &str) -> Vec<u16> {
    Text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn Opened(Value: *mut c_void) -> Result<Handle, String> {
    if Value.is_null() { Err(windows::core::Error::from_thread().message()) } else { Ok(Handle(Value)) }
}

fn Get(Url: &str, Accept: &str, Output: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    let Rest = Url.strip_prefix("https://").ok_or("Only HTTPS downloads are allowed")?;
    let (Host, Location) = match Rest.find('/') {
        Some(Index) => (&Rest[..Index], &Rest[Index..]),
        None => (Rest, "/"),
    };
    let Agent = Wide(&format!("Flux/{}", CurrentVersion()));
    let Session = Opened(unsafe { WinHttpOpen(PCWSTR(Agent.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0) })?;
    unsafe { WinHttpSetTimeouts(Session.0, 15000, 15000, 30000, 30000) }.map_err(|Error| Error.message())?;
    let HostName = Wide(Host);
    let Connection = Opened(unsafe { WinHttpConnect(Session.0, PCWSTR(HostName.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0) })?;
    let Verb = Wide("GET");
    let Object = Wide(Location);
    let Request = Opened(unsafe { WinHttpOpenRequest(Connection.0, PCWSTR(Verb.as_ptr()), PCWSTR(Object.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE) })?;
    let Headers: Vec<u16> = format!("Accept: {Accept}\r\n").encode_utf16().collect();
    unsafe { WinHttpSendRequest(Request.0, Some(&Headers), None, 0, 0, 0) }.map_err(|Error| Error.message())?;
    unsafe { WinHttpReceiveResponse(Request.0, std::ptr::null_mut()) }.map_err(|Error| Error.message())?;
    let mut Status = 0u32;
    let mut Length = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(Request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut Status as *mut u32 as *mut c_void), &mut Length, std::ptr::null_mut())
    }
    .map_err(|Error| Error.message())?;
    if Status != 200 {
        return Err(format!("GitHub answered with HTTP status {Status}"));
    }
    let mut Buffer = vec![0u8; 64 * 1024];
    loop {
        let mut Read = 0u32;
        unsafe { WinHttpReadData(Request.0, Buffer.as_mut_ptr() as *mut c_void, Buffer.len() as u32, &mut Read) }.map_err(|Error| Error.message())?;
        if Read == 0 {
            return Ok(());
        }
        Output(&Buffer[..Read as usize])?;
    }
}

enum Json {
    Null,
    Flag,
    Number,
    Text(String),
    List(Vec<Json>),
    Map(Vec<(String, Json)>),
}

impl Json {
    fn Field(&self, Name: &str) -> Option<&Json> {
        match self {
            Json::Map(Fields) => Fields.iter().find(|(Key, _)| Key == Name).map(|(_, Value)| Value),
            _ => None,
        }
    }

    fn AsText(&self) -> Option<&str> {
        match self {
            Json::Text(Value) => Some(Value),
            _ => None,
        }
    }
}

struct Parser<'a> {
    Bytes: &'a [u8],
    At: usize,
}

impl Parser<'_> {
    fn Blank(&mut self) {
        while self.Bytes.get(self.At).is_some_and(|Byte| matches!(Byte, b' ' | b'\t' | b'\r' | b'\n')) {
            self.At += 1;
        }
    }

    fn Eat(&mut self, Byte: u8) -> bool {
        self.Blank();
        if self.Bytes.get(self.At) == Some(&Byte) {
            self.At += 1;
            true
        } else {
            false
        }
    }

    fn Word(&mut self, Word: &str) -> Option<()> {
        self.Bytes[self.At..].starts_with(Word.as_bytes()).then(|| self.At += Word.len())
    }

    fn Value(&mut self, Depth: u32) -> Option<Json> {
        if Depth > 64 {
            return None;
        }
        self.Blank();
        match *self.Bytes.get(self.At)? {
            b'{' => {
                self.At += 1;
                let mut Fields = Vec::new();
                if self.Eat(b'}') {
                    return Some(Json::Map(Fields));
                }
                loop {
                    self.Blank();
                    let Key = self.String()?;
                    if !self.Eat(b':') {
                        return None;
                    }
                    Fields.push((Key, self.Value(Depth + 1)?));
                    if self.Eat(b'}') {
                        return Some(Json::Map(Fields));
                    }
                    if !self.Eat(b',') {
                        return None;
                    }
                }
            }
            b'[' => {
                self.At += 1;
                let mut Items = Vec::new();
                if self.Eat(b']') {
                    return Some(Json::List(Items));
                }
                loop {
                    Items.push(self.Value(Depth + 1)?);
                    if self.Eat(b']') {
                        return Some(Json::List(Items));
                    }
                    if !self.Eat(b',') {
                        return None;
                    }
                }
            }
            b'"' => self.String().map(Json::Text),
            b't' => self.Word("true").map(|()| Json::Flag),
            b'f' => self.Word("false").map(|()| Json::Flag),
            b'n' => self.Word("null").map(|()| Json::Null),
            _ => {
                let Start = self.At;
                while self.Bytes.get(self.At).is_some_and(|Byte| matches!(Byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.At += 1;
                }
                (self.At > Start).then_some(Json::Number)
            }
        }
    }

    fn Hex(&mut self) -> Option<u32> {
        let Digits = std::str::from_utf8(self.Bytes.get(self.At..self.At + 4)?).ok()?;
        self.At += 4;
        u32::from_str_radix(Digits, 16).ok()
    }

    fn String(&mut self) -> Option<String> {
        if self.Bytes.get(self.At) != Some(&b'"') {
            return None;
        }
        self.At += 1;
        let mut Output = Vec::new();
        loop {
            let Byte = *self.Bytes.get(self.At)?;
            self.At += 1;
            match Byte {
                b'"' => return String::from_utf8(Output).ok(),
                b'\\' => {
                    let Escape = *self.Bytes.get(self.At)?;
                    self.At += 1;
                    let Character = match Escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let High = self.Hex()?;
                            let Code = if (0xD800..0xDC00).contains(&High) && self.Word("\\u").is_some() {
                                let Low = self.Hex()?;
                                0x10000 + ((High - 0xD800) << 10) + (Low.checked_sub(0xDC00)? & 0x3FF)
                            } else {
                                High
                            };
                            char::from_u32(Code).unwrap_or('\u{FFFD}')
                        }
                        _ => return None,
                    };
                    let mut Encoded = [0u8; 4];
                    Output.extend_from_slice(Character.encode_utf8(&mut Encoded).as_bytes());
                }
                _ => Output.push(Byte),
            }
        }
    }
}

fn ParseRelease(Body: &[u8]) -> Result<Release, String> {
    let mut Reader = Parser { Bytes: Body, At: 0 };
    let Root = Reader.Value(0).ok_or("GitHub returned unreadable release information")?;
    let Tag = Root.Field("tag_name").and_then(Json::AsText).ok_or("The latest release has no version tag")?;
    let Version = Tag.trim_start_matches(['v', 'V']).to_string();
    let Assets = match Root.Field("assets") {
        Some(Json::List(Items)) => Items.as_slice(),
        _ => &[],
    };
    let Asset = Assets
        .iter()
        .find(|Asset| Asset.Field("name").and_then(Json::AsText).is_some_and(|Name| Name.starts_with(AssetPrefix) && Name.ends_with(AssetSuffix) && !Name.contains("symbols")))
        .ok_or_else(|| format!("Release {Tag} has no Windows package"))?;
    let Download = Asset.Field("browser_download_url").and_then(Json::AsText).ok_or("The Windows package has no download address")?.to_string();
    let Digest = Asset.Field("digest").and_then(Json::AsText).and_then(|Value| Value.strip_prefix("sha256:")).map(str::to_string);
    Ok(Release { Version, Download, Digest })
}

fn Parts(Version: &str) -> Vec<u64> {
    Version.split('.').map(|Part| Part.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect()
}

fn Newer(Candidate: &str, Installed: &str) -> bool {
    let (Left, Right) = (Parts(Candidate), Parts(Installed));
    for Index in 0..Left.len().max(Right.len()) {
        let (A, B) = (Left.get(Index).copied().unwrap_or(0), Right.get(Index).copied().unwrap_or(0));
        if A != B {
            return A > B;
        }
    }
    false
}

fn Latest() -> Result<Release, String> {
    let mut Body = Vec::new();
    Get(LatestRelease, "application/vnd.github+json", &mut |Chunk| {
        if Body.len() + Chunk.len() > MetadataLimit {
            return Err("GitHub returned too much release information".to_string());
        }
        Body.extend_from_slice(Chunk);
        Ok(())
    })?;
    ParseRelease(&Body)
}

fn Retired(Item: &Path) -> PathBuf {
    let mut Name = Item.as_os_str().to_owned();
    Name.push(".old");
    PathBuf::from(Name)
}

fn Collect(Root: &Path, Relative: &Path, Output: &mut Vec<PathBuf>) -> io::Result<()> {
    for Entry in fs::read_dir(Root.join(Relative))? {
        let Entry = Entry?;
        let Child = Relative.join(Entry.file_name());
        if Entry.file_type()?.is_dir() {
            Collect(Root, &Child, Output)?;
        } else {
            Output.push(Child);
        }
    }
    Ok(())
}

fn Stage(Item: &Release, Work: &Path) -> Result<PathBuf, String> {
    let Archive = Work.join("Package.zip");
    let mut Output = fs::File::create(&Archive).map_err(|Error| Error.to_string())?;
    let mut Written = 0u64;
    Get(&Item.Download, "application/octet-stream", &mut |Chunk| {
        Written += Chunk.len() as u64;
        if Written > PackageLimit {
            return Err("The download is larger than any Flux release".to_string());
        }
        Output.write_all(Chunk).map_err(|Error| Error.to_string())
    })?;
    Output.sync_all().map_err(|Error| Error.to_string())?;
    drop(Output);
    if let Some(Expected) = &Item.Digest {
        let mut Buffer = Vec::new();
        let Hash = FluxSdk::Utilities::HashFile(&Archive, FluxSdk::Utilities::HashAlgorithm::Sha256, &mut Buffer, &|| false, &mut |_| {})
            .map_err(|Error| Error.to_string())?
            .unwrap_or_default();
        let Actual: String = Hash.iter().map(|Byte| format!("{Byte:02x}")).collect();
        if !Actual.eq_ignore_ascii_case(Expected) {
            return Err("The download does not match the checksum published on GitHub".to_string());
        }
    }
    let Extracted = Work.join("Package");
    fs::create_dir_all(&Extracted).map_err(|Error| Error.to_string())?;
    let Tar = std::env::var_os("SystemRoot").map(|Root| PathBuf::from(Root).join("System32").join("tar.exe")).filter(|Tool| Tool.is_file()).ok_or("tar.exe is missing from System32")?;
    let Status = std::process::Command::new(Tar).arg("-xf").arg(&Archive).arg("-C").arg(&Extracted).creation_flags(CreateNoWindow).status().map_err(|Error| Error.to_string())?;
    if !Status.success() {
        return Err("The downloaded package could not be extracted".to_string());
    }
    let Root = Extracted.join("Flux");
    if !Root.join("Flux.exe").is_file() {
        return Err("The downloaded package does not contain Flux.exe".to_string());
    }
    Ok(Root)
}

fn Rollback(Moved: &[(PathBuf, Option<PathBuf>)]) {
    for (Target, Backup) in Moved.iter().rev() {
        let _ = fs::remove_file(Target);
        if let Some(Old) = Backup {
            let _ = fs::rename(Old, Target);
        }
    }
}

fn Replace(Source: &Path, Target: &Path) -> Result<(), String> {
    let mut Files = Vec::new();
    Collect(Source, Path::new(""), &mut Files).map_err(|Error| Error.to_string())?;
    let mut Moved: Vec<(PathBuf, Option<PathBuf>)> = Vec::with_capacity(Files.len());
    for Relative in &Files {
        let Destination = Target.join(Relative);
        let Step = || -> io::Result<Option<PathBuf>> {
            if let Some(Parent) = Destination.parent() {
                fs::create_dir_all(Parent)?;
            }
            let Backup = if Destination.exists() {
                let Old = Retired(&Destination);
                let _ = fs::remove_file(&Old);
                fs::rename(&Destination, &Old)?;
                Some(Old)
            } else {
                None
            };
            if let Err(Error) = fs::copy(Source.join(Relative), &Destination) {
                let _ = fs::remove_file(&Destination);
                if let Some(Old) = &Backup {
                    let _ = fs::rename(Old, &Destination);
                }
                return Err(Error);
            }
            Ok(Backup)
        };
        match Step() {
            Ok(Backup) => Moved.push((Destination, Backup)),
            Err(Error) => {
                Rollback(&Moved);
                return Err(format!("{}: {Error}. The installation was left unchanged.", Destination.display()));
            }
        }
    }
    Ok(())
}

fn Apply(Item: &Release) -> Result<PathBuf, String> {
    let Executable = std::env::current_exe().map_err(|Error| Error.to_string())?;
    let Directory = Executable.parent().ok_or("The installation folder is unknown")?.to_path_buf();
    let Work = std::env::temp_dir().join(format!("Flux-Update-{}-{}", Item.Version, std::process::id()));
    let _ = fs::remove_dir_all(&Work);
    fs::create_dir_all(&Work).map_err(|Error| Error.to_string())?;
    let Result = Stage(Item, &Work).and_then(|Root| Replace(&Root, &Directory));
    let _ = fs::remove_dir_all(&Work);
    Result.map(|()| Directory.join("Flux.exe"))
}

fn RemoveRetired(Directory: &Path) {
    let Ok(Entries) = fs::read_dir(Directory) else { return };
    for Entry in Entries.flatten() {
        let Item = Entry.path();
        if Entry.file_type().is_ok_and(|Kind| Kind.is_dir()) {
            RemoveRetired(&Item);
        } else if Item.extension().is_some_and(|Extension| Extension.eq_ignore_ascii_case("old")) && Item.with_extension("").is_file() {
            let _ = fs::remove_file(&Item);
        }
    }
}

impl App {
    pub fn StartUpdates(&mut self) {
        if !Supported() {
            return;
        }
        let Due = self.UpdateDue();
        if Due {
            self.Updating = true;
            self.Settings.LastUpdateCheck = Now();
            self.SaveSettings();
        }
        let Sink = self.Sink.clone();
        let Started = Spawn("Update", move || {
            if let Some(Directory) = std::env::current_exe().ok().and_then(|Executable| Executable.parent().map(Path::to_path_buf)) {
                RemoveRetired(&Directory);
            }
            if Due {
                Sink.Send(Event::Update(Check(false)));
            }
        });
        if Started.is_err() {
            self.Updating = false;
        }
        Platform::StartTimer(self.Window, Platform::TimerUpdate, TickMilliseconds);
    }

    fn UpdateDue(&self) -> bool {
        self.Settings.AutoUpdate && !self.Updating && Now().saturating_sub(self.Settings.LastUpdateCheck) >= CheckInterval
    }

    pub fn UpdateTick(&mut self) {
        if self.UpdateDue() {
            self.CheckForUpdates(false);
        }
    }

    pub fn CheckForUpdates(&mut self, Manual: bool) {
        if !Supported() {
            if Manual {
                self.Inform("Updates are available for installed release builds only");
            }
            return;
        }
        if self.Updating {
            if Manual {
                self.Inform("An update check is already running");
            }
            return;
        }
        self.Updating = true;
        self.Settings.LastUpdateCheck = Now();
        self.SaveSettings();
        if Manual {
            self.Inform("Checking for updates…");
        }
        let Sink = self.Sink.clone();
        if let Err(Error) = Spawn("Update", move || Sink.Send(Event::Update(Check(Manual)))) {
            self.Updating = false;
            if Manual {
                self.Fail("Could not check for updates", &Error.to_string());
            }
        }
    }

    pub fn OnUpdate(&mut self, Outcome: UpdateEvent) {
        match Outcome {
            UpdateEvent::Checked { Found: Some(Item), Manual } => {
                self.Updating = false;
                if Manual {
                    self.Notice = None;
                }
                if Manual || (self.Settings.AutoUpdate && self.Settings.SkippedVersion != Item.Version) {
                    self.OfferUpdate(Item, Manual);
                }
            }
            UpdateEvent::Checked { Found: None, Manual } => {
                self.Updating = false;
                if Manual {
                    self.Inform(&format!("Flux {} is up to date", CurrentVersion()));
                }
            }
            UpdateEvent::CheckFailed { Message, Manual } => {
                self.Updating = false;
                if Manual {
                    self.Fail("Could not check for updates", &Message);
                }
            }
            UpdateEvent::Installed(Executable) => {
                self.Relaunch = Some(Executable);
                Platform::Close(self.Window);
            }
            UpdateEvent::InstallFailed(Message) => {
                self.Updating = false;
                self.Fail("The update failed", &Message);
            }
        }
    }

    fn OfferUpdate(&mut self, Item: Release, Interrupt: bool) {
        if self.Overlay.is_some() && !Interrupt {
            self.PendingUpdate = Some(Item);
            return;
        }
        let Message = format!(
            "Flux {} is available. You are running {}.\n\nUpdate downloads it from GitHub, verifies it, replaces this installation and restarts Flux with your tabs restored.",
            Item.Version,
            CurrentVersion()
        );
        self.ShowPrompt(Prompt::New("Update available", &Message, &["Update", "Not now"], PromptPurpose::Update(Item)).WithToggle("Never ask again"));
    }

    pub fn ShowPendingUpdate(&mut self) {
        if self.Overlay.is_none() {
            if let Some(Item) = self.PendingUpdate.take() {
                self.OfferUpdate(Item, false);
            }
        }
    }

    pub fn AnswerUpdate(&mut self, Item: Release, Button: Option<usize>, Never: bool) {
        if Never {
            self.Settings.AutoUpdate = false;
        }
        if Button == Some(0) {
            self.Updating = true;
            self.Inform(&format!("Downloading Flux {}…", Item.Version));
            let Sink = self.Sink.clone();
            if let Err(Error) = Spawn("Update", move || {
                let Outcome = match Apply(&Item) {
                    Ok(Executable) => UpdateEvent::Installed(Executable),
                    Err(Message) => UpdateEvent::InstallFailed(Message),
                };
                Sink.Send(Event::Update(Outcome));
            }) {
                self.Updating = false;
                self.Fail("The update failed", &Error.to_string());
            }
        } else {
            self.Settings.SkippedVersion = Item.Version;
        }
        self.SaveSettings();
    }
}

fn Check(Manual: bool) -> UpdateEvent {
    match Latest() {
        Ok(Item) => UpdateEvent::Checked { Found: Newer(&Item.Version, CurrentVersion()).then_some(Item), Manual },
        Err(Message) => UpdateEvent::CheckFailed { Message, Manual },
    }
}
