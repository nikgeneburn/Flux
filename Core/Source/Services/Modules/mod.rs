pub mod Host;

use crate::Services::Config::{AppPaths, Ini};
use crate::Services::Workers::{EventSink, NewIdentifier, Spawn};
use FluxSdk::Utilities::{HashAlgorithm, Hasher, Hex};
use Host::{Invocation, InvocationData, LoadedModule};
use std::fs;
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Requirement {
    Nothing,
    Selection,
    Directory,
    OtherPane,
}

#[derive(Clone, Debug)]
pub struct ModuleCommand {
    pub Module: usize,
    pub Identifier: String,
    pub Title: String,
    pub Prompt: String,
    pub Requires: Requirement,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub Id: String,
    pub Name: String,
    pub Version: String,
    pub AbiVersion: u32,
    pub Library: String,
    pub Description: String,
    pub Author: String,
    pub DeclaredHash: Option<String>,
    pub Commands: Vec<ModuleCommand>,
    pub PreviewExtensions: Vec<String>,
    pub Directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModuleState {
    Untrusted,
    Enabled,
    Disabled,
    Modified,
    Incompatible(String),
    Failed(String),
}

pub struct ModuleEntry {
    pub Manifest: Manifest,
    pub State: ModuleState,
    pub Loaded: bool,
    Library: Option<Arc<LoadedModule>>,
}

pub struct InvocationContext {
    pub Directory: PathBuf,
    pub OtherDirectory: Option<PathBuf>,
    pub Argument: String,
    pub Selection: Vec<PathBuf>,
}

pub enum ModuleEvent {
    Progress,
    Finished { Title: String, Status: i32, Text: String, Paths: Vec<PathBuf>, Renames: Vec<(PathBuf, PathBuf)>, Elapsed: Duration },
    Preview { Token: u64, Text: Option<String> },
}

pub struct ModuleHost {
    Entries: Vec<ModuleEntry>,
    CommandList: Vec<ModuleCommand>,
    Trust: Ini,
    TrustFile: PathBuf,
    Directories: Vec<PathBuf>,
    InstallDirectory: PathBuf,
    Active: Arc<Mutex<Vec<Arc<Invocation>>>>,
}

fn ParseManifest(Directory: &Path) -> Result<Manifest, String> {
    let Data = Ini::Load(&Directory.join("Module.ini")).ok_or_else(|| "Module.ini could not be read".to_string())?;
    let Field = |Key: &str| Data.Get("Module", Key).unwrap_or("").trim().to_string();
    let Id = Field("Id");
    let Library = Field("Library");
    if Id.is_empty() || Library.is_empty() {
        return Err("Module.ini must define Id and Library".to_string());
    }
    if Library.contains(['\\', '/', ':']) || !Library.to_ascii_lowercase().ends_with(".dll") {
        return Err("Library must be a DLL file name located next to Module.ini".to_string());
    }
    let mut Commands = Vec::new();
    for (Section, Values) in &Data.Sections {
        let Some(Identifier) = Section.strip_prefix("Command.") else { continue };
        let Get = |Key: &str| Values.iter().find(|(Name, _)| Name.eq_ignore_ascii_case(Key)).map(|(_, Value)| Value.trim().to_string()).unwrap_or_default();
        let Requires = match Get("Requires").to_ascii_lowercase().as_str() {
            "selection" => Requirement::Selection,
            "directory" => Requirement::Directory,
            "otherpane" => Requirement::OtherPane,
            _ => Requirement::Nothing,
        };
        let Title = Get("Title");
        Commands.push(ModuleCommand { Module: 0, Identifier: Identifier.to_string(), Title: if Title.is_empty() { Identifier.to_string() } else { Title }, Prompt: Get("Prompt"), Requires });
    }
    let Name = Field("Name");
    Ok(Manifest {
        Name: if Name.is_empty() { Id.clone() } else { Name },
        Id,
        Version: Field("Version"),
        AbiVersion: Field("AbiVersion").parse().unwrap_or(0),
        Library,
        Description: Field("Description"),
        Author: Field("Author"),
        DeclaredHash: Some(Field("Sha256")).filter(|Value| Value.len() == 64),
        Commands,
        PreviewExtensions: Data.Get("Preview", "Extensions").unwrap_or("").split(',').map(|Part| Part.trim().trim_start_matches('.').to_ascii_lowercase()).filter(|Part| !Part.is_empty()).collect(),
        Directory: Directory.to_path_buf(),
    })
}

fn OpenLocked(Item: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().read(true).share_mode(0x1).open(Item)
}

fn HashOpen(File: &mut fs::File) -> std::io::Result<String> {
    let mut State = Hasher::New(HashAlgorithm::Sha256)?;
    let mut Buffer = vec![0u8; 256 * 1024];
    loop {
        let Count = File.read(&mut Buffer)?;
        if Count == 0 {
            break;
        }
        State.Update(&Buffer[..Count])?;
    }
    Ok(Hex(&State.Finish()?))
}

fn CopyTree(Source: &Path, Destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(Destination)?;
    for Item in fs::read_dir(Source)? {
        let Item = Item?;
        let Kind = Item.file_type()?;
        if Kind.is_symlink() {
            continue;
        }
        let Target = Destination.join(Item.file_name());
        if Kind.is_dir() {
            CopyTree(&Item.path(), &Target)?;
        } else {
            fs::copy(Item.path(), &Target)?;
        }
    }
    Ok(())
}

impl ModuleHost {
    pub fn New(Paths: &AppPaths) -> ModuleHost {
        let TrustFile = Paths.TrustFile();
        ModuleHost {
            Entries: Vec::new(),
            CommandList: Vec::new(),
            Trust: Ini::Load(&TrustFile).unwrap_or_default(),
            TrustFile,
            Directories: Paths.ModuleDirectories(),
            InstallDirectory: Paths.InstalledModulesDirectory(),
            Active: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn Discover(&mut self) {
        let Previous = std::mem::take(&mut self.Entries);
        let mut Entries: Vec<ModuleEntry> = Vec::new();
        for Directory in &self.Directories {
            let Ok(Reader) = fs::read_dir(Directory) else { continue };
            let mut Folders: Vec<PathBuf> = Reader.flatten().map(|Item| Item.path()).filter(|Item| Item.join("Module.ini").is_file()).collect();
            Folders.sort();
            for Folder in Folders {
                let Manifest = match ParseManifest(&Folder) {
                    Ok(Manifest) => Manifest,
                    Err(_) => continue,
                };
                if Entries.iter().any(|Existing| Existing.Manifest.Id.eq_ignore_ascii_case(&Manifest.Id)) {
                    continue;
                }
                let Kept = Previous.iter().find(|Old| Old.Manifest.Id == Manifest.Id && Old.Manifest.Directory == Manifest.Directory).and_then(|Old| Old.Library.clone());
                let State = if Manifest.AbiVersion != FluxSdk::AbiVersion {
                    ModuleState::Incompatible(format!("requires ABI {}, Flux provides {}", Manifest.AbiVersion, FluxSdk::AbiVersion))
                } else {
                    match self.Trust.Get(&Manifest.Id, "Sha256") {
                        None => ModuleState::Untrusted,
                        Some(_) if self.Trust.Bool(&Manifest.Id, "Enabled", true) => ModuleState::Enabled,
                        Some(_) => ModuleState::Disabled,
                    }
                };
                let Loaded = Kept.is_some();
                Entries.push(ModuleEntry { Manifest, State, Loaded, Library: Kept });
            }
        }
        self.Entries = Entries;
        self.CommandList = self
            .Entries
            .iter()
            .enumerate()
            .flat_map(|(Module, Entry)| Entry.Manifest.Commands.iter().map(move |Item| ModuleCommand { Module, ..Item.clone() }))
            .collect();
    }

    pub fn Entries(&self) -> &[ModuleEntry] {
        &self.Entries
    }

    pub fn Commands(&self) -> Vec<(u32, &ModuleCommand)> {
        self.CommandList.iter().enumerate().filter(|(_, Item)| self.Entries[Item.Module].State == ModuleState::Enabled).map(|(Index, Item)| (Index as u32, Item)).collect()
    }

    pub fn Command(&self, Index: u32) -> Option<&ModuleCommand> {
        self.CommandList.get(Index as usize).filter(|Item| self.Entries[Item.Module].State == ModuleState::Enabled)
    }

    fn LibraryPath(&self, Module: usize) -> PathBuf {
        let Manifest = &self.Entries[Module].Manifest;
        Manifest.Directory.join(&Manifest.Library)
    }

    pub fn ComputeHash(&self, Module: usize) -> Result<String, String> {
        let mut File = OpenLocked(&self.LibraryPath(Module)).map_err(|Error| Error.to_string())?;
        HashOpen(&mut File).map_err(|Error| Error.to_string())
    }

    fn SaveTrust(&mut self) {
        let _ = self.Trust.Save(&self.TrustFile);
    }

    pub fn Approve(&mut self, Module: usize) -> Result<String, String> {
        let Hash = self.ComputeHash(Module)?;
        let Manifest = &self.Entries[Module].Manifest;
        if let Some(Declared) = &Manifest.DeclaredHash {
            if !Declared.eq_ignore_ascii_case(&Hash) {
                return Err(format!("The library hash {Hash} does not match the declared hash {Declared}"));
            }
        }
        let Id = Manifest.Id.clone();
        self.Trust.Set(&Id, "Sha256", &Hash);
        self.Trust.Set(&Id, "Enabled", "true");
        self.Trust.Set(&Id, "Library", &self.LibraryPath(Module).to_string_lossy());
        self.SaveTrust();
        self.Entries[Module].Library = None;
        self.Entries[Module].Loaded = false;
        self.Entries[Module].State = ModuleState::Enabled;
        Ok(Hash)
    }

    pub fn Disable(&mut self, Module: usize) {
        let Id = self.Entries[Module].Manifest.Id.clone();
        self.Trust.Set(&Id, "Enabled", "false");
        self.SaveTrust();
        let Entry = &mut self.Entries[Module];
        Entry.Library = None;
        Entry.Loaded = false;
        Entry.State = ModuleState::Disabled;
    }

    pub fn Enable(&mut self, Module: usize) -> Result<(), String> {
        let Id = self.Entries[Module].Manifest.Id.clone();
        if self.Trust.Get(&Id, "Sha256").is_none() {
            return Err("The module has not been approved".to_string());
        }
        self.Trust.Set(&Id, "Enabled", "true");
        self.SaveTrust();
        self.Entries[Module].State = ModuleState::Enabled;
        Ok(())
    }

    pub fn Revoke(&mut self, Module: usize) {
        let Id = self.Entries[Module].Manifest.Id.clone();
        self.Trust.Sections.retain(|(Name, _)| !Name.eq_ignore_ascii_case(&Id));
        self.SaveTrust();
        let Entry = &mut self.Entries[Module];
        Entry.Library = None;
        Entry.Loaded = false;
        Entry.State = ModuleState::Untrusted;
    }

    fn Ensure(&mut self, Module: usize) -> Result<Arc<LoadedModule>, String> {
        if self.Entries[Module].State != ModuleState::Enabled {
            return Err("The module is not enabled".to_string());
        }
        if let Some(Library) = &self.Entries[Module].Library {
            return Ok(Library.clone());
        }
        let Id = self.Entries[Module].Manifest.Id.clone();
        let Trusted = self.Trust.Get(&Id, "Sha256").map(str::to_string).ok_or_else(|| "The module has not been approved".to_string())?;
        let Library = self.LibraryPath(Module);
        let mut Guard = OpenLocked(&Library).map_err(|Error| Error.to_string())?;
        let Actual = HashOpen(&mut Guard).map_err(|Error| Error.to_string())?;
        if !Actual.eq_ignore_ascii_case(&Trusted) {
            self.Entries[Module].State = ModuleState::Modified;
            return Err("The module library changed since it was approved; review and approve it again".to_string());
        }
        let Loaded = Host::Load(&Library);
        drop(Guard);
        match Loaded {
            Ok(Loaded) => {
                let Shared = Arc::new(Loaded);
                self.Entries[Module].Library = Some(Shared.clone());
                self.Entries[Module].Loaded = true;
                Ok(Shared)
            }
            Err(Message) => {
                self.Entries[Module].State = ModuleState::Failed(Message.clone());
                Err(Message)
            }
        }
    }

    fn Track(&self, State: &Arc<Invocation>) {
        if let Ok(mut Active) = self.Active.lock() {
            Active.push(State.clone());
        }
    }

    pub fn Invoke(&mut self, Index: u32, Context: InvocationContext, Sink: EventSink) -> Result<u64, String> {
        let Item = self.CommandList.get(Index as usize).cloned().ok_or_else(|| "Unknown module command".to_string())?;
        let Library = self.Ensure(Item.Module)?;
        let Identifier = NewIdentifier();
        let State = Invocation::New(Identifier, Item.Title.clone(), Sink);
        self.Track(&State);
        let Active = self.Active.clone();
        let Data = InvocationData { Command: Item.Identifier.clone(), Directory: Context.Directory, OtherDirectory: Context.OtherDirectory, Argument: Context.Argument, Selection: Context.Selection };
        let ThreadState = State.clone();
        Spawn("Module", move || {
            Host::RunInvocation(Library, ThreadState.clone(), Data);
            if let Ok(mut Active) = Active.lock() {
                Active.retain(|Running| !Arc::ptr_eq(Running, &ThreadState));
            }
        })
        .map_err(|Error| Error.to_string())?;
        Ok(Identifier)
    }

    pub fn PreviewProvider(&self, Extension: &str) -> Option<usize> {
        if Extension.is_empty() {
            return None;
        }
        self.Entries.iter().position(|Entry| Entry.State == ModuleState::Enabled && Entry.Manifest.PreviewExtensions.iter().any(|Candidate| Candidate == Extension))
    }

    pub fn Preview(&mut self, Module: usize, Item: PathBuf, Token: u64, Sink: EventSink) -> Result<(), String> {
        let Library = self.Ensure(Module)?;
        let State = Invocation::New(NewIdentifier(), String::new(), Sink);
        Spawn("ModulePreview", move || Host::RunPreview(Library, State, Item, Token)).map(|_| ()).map_err(|Error| Error.to_string())
    }

    pub fn Cancel(&self, Identifier: u64) {
        if let Ok(Active) = self.Active.lock() {
            for Running in Active.iter().filter(|Running| Running.Identifier == Identifier) {
                Running.Cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn Running(&self) -> Vec<(u64, String, u64, u64)> {
        self.Active
            .lock()
            .map(|Active| Active.iter().map(|Running| (Running.Identifier, Running.Title.clone(), Running.Done.load(Ordering::Relaxed), Running.Total.load(Ordering::Relaxed))).collect())
            .unwrap_or_default()
    }

    pub fn Install(&mut self, Source: &Path) -> Result<String, String> {
        let Manifest = ParseManifest(Source)?;
        if !Source.join(&Manifest.Library).is_file() {
            return Err(format!("{} was not found next to Module.ini", Manifest.Library));
        }
        let Safe: String = Manifest.Id.chars().map(|Character| if Character.is_alphanumeric() || Character == '.' || Character == '-' { Character } else { '_' }).collect();
        let Destination = self.InstallDirectory.join(Safe);
        if crate::FileSystem::SamePath(&Destination, Source) {
            self.Discover();
            return Ok(Manifest.Name);
        }
        if let Some(Index) = self.Entries.iter().position(|Entry| Entry.Manifest.Directory == Destination) {
            self.Entries[Index].Library = None;
        }
        if Destination.exists() {
            fs::remove_dir_all(&Destination).map_err(|Error| format!("The existing module folder could not be replaced: {Error}"))?;
        }
        CopyTree(Source, &Destination).map_err(|Error| Error.to_string())?;
        if self.Trust.Get(&Manifest.Id, "Sha256").is_some() {
            self.Trust.Set(&Manifest.Id, "Enabled", "false");
            self.Trust.Set(&Manifest.Id, "Sha256", "reinstalled");
            self.SaveTrust();
        }
        self.Discover();
        Ok(Manifest.Name)
    }
}
