#![allow(non_snake_case, non_upper_case_globals)]

mod Store;

use FluxSdk::Utilities::{FormatSize, Walk, WalkControl};
use FluxSdk::{Host, Invocation, Module, Outcome};
use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

struct Index;

static Instance: Index = Index;

FluxSdk::ExportModule!(Instance);

const MaximumResults: usize = 20_000;
const ProgressStep: u64 = 4096;
const ListedErrors: usize = 20;

struct Query {
    Terms: Vec<String>,
    Extensions: Vec<String>,
    Folders: Vec<String>,
}

fn CivilDate(Seconds: i64) -> (i64, i64, i64, i64, i64) {
    let Days = Seconds.div_euclid(86400);
    let Remainder = Seconds.rem_euclid(86400);
    let Shifted = Days + 719468;
    let Era = Shifted.div_euclid(146097);
    let DayOfEra = Shifted - Era * 146097;
    let YearOfEra = (DayOfEra - DayOfEra / 1460 + DayOfEra / 36524 - DayOfEra / 146096) / 365;
    let DayOfYear = DayOfEra - (365 * YearOfEra + YearOfEra / 4 - YearOfEra / 100);
    let MonthIndex = (5 * DayOfYear + 2) / 153;
    let Day = DayOfYear - (153 * MonthIndex + 2) / 5 + 1;
    let Month = if MonthIndex < 10 { MonthIndex + 3 } else { MonthIndex - 9 };
    let Year = YearOfEra + Era * 400 + if Month <= 2 { 1 } else { 0 };
    (Year, Month, Day, Remainder / 3600, Remainder / 60 % 60)
}

fn FormatBuilt(Ticks: u64) -> String {
    let (Year, Month, Day, Hour, Minute) = CivilDate(Store::TicksToUnixSeconds(Ticks));
    let Age = Store::NowTicks().saturating_sub(Ticks) / 10_000_000;
    let AgeText = match Age {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", Age / 60),
        3600..86400 => format!("{} h ago", Age / 3600),
        _ => format!("{} day(s) ago", Age / 86400),
    };
    format!("{Year:04}-{Month:02}-{Day:02} {Hour:02}:{Minute:02} UTC, {AgeText}")
}

fn Build(Host: &Host, Directory: &Path) -> Outcome {
    let Started = Instant::now();
    let Root = Store::NormalizeRoot(Directory);
    if !Root.is_dir() {
        return Outcome::Failed(format!("{} is not a folder", Root.display()));
    }
    let Folder = match Store::IndexDirectory() {
        Ok(Folder) => Folder,
        Err(Error) => return Outcome::Failed(format!("Cannot locate the index folder: {Error}")),
    };
    if let Err(Error) = fs::create_dir_all(&Folder) {
        return Outcome::Failed(format!("Cannot create {}: {}", Folder.display(), Error));
    }
    let Target = match Store::IndexFile(&Folder, &Root) {
        Ok(Target) => Target,
        Err(Error) => return Outcome::Failed(format!("Cannot name the index file: {Error}")),
    };
    let Temporary = Target.with_extension(format!("{}.{}.tmp", Store::Extension, process::id()));
    let mut Output = match Store::Writer::Create(&Temporary, &Root, Store::NowTicks()) {
        Ok(Output) => Output,
        Err(Error) => return Outcome::Failed(format!("Cannot create {}: {}", Temporary.display(), Error)),
    };
    let mut Files = 0u64;
    let mut Folders = 0u64;
    let mut Bytes = 0u64;
    let mut Stopped = false;
    let mut WriteError = None;
    let WalkErrors = Walk(&Root, &mut |Item, Data, _| {
        if Host.Cancelled() {
            Stopped = true;
            return WalkControl::Stop;
        }
        let Relative: Vec<u16> = match Item.strip_prefix(&Root) {
            Ok(Relative) => Relative.as_os_str().encode_wide().collect(),
            Err(_) => return WalkControl::Continue,
        };
        let IsDirectory = Data.is_dir();
        if let Err(Error) = Output.Add(&Relative, Data.len(), Data.last_write_time(), IsDirectory) {
            WriteError = Some(Error);
            return WalkControl::Stop;
        }
        if IsDirectory {
            Folders += 1;
        } else {
            Files += 1;
            Bytes += Data.len();
        }
        if (Files + Folders) % ProgressStep == 0 {
            Host.Progress(Files + Folders, 0);
        }
        WalkControl::Continue
    });
    let Finished = if Stopped || WriteError.is_some() { None } else { Some(Output.Finish()) };
    let Count = match (Finished, WriteError) {
        (Some(Ok(Count)), _) => Count,
        (Some(Err(Error)), _) | (None, Some(Error)) => {
            let _ = fs::remove_file(&Temporary);
            return Outcome::Failed(format!("Cannot write {}: {}", Temporary.display(), Error));
        }
        (None, None) => {
            let _ = fs::remove_file(&Temporary);
            return Outcome::Cancelled;
        }
    };
    if let Err(Error) = fs::rename(&Temporary, &Target) {
        let _ = fs::remove_file(&Temporary);
        return Outcome::Failed(format!("Cannot replace {}: {}", Target.display(), Error));
    }
    Store::Forget(&Target);
    let FileSize = fs::metadata(&Target).map(|Data| Data.len()).unwrap_or(0);
    Host.Line(&format!("Indexed {}", Root.display()));
    Host.Line(&format!(
        "{} entries: {} file(s), {} folder(s), {} of data",
        Count,
        Files,
        Folders,
        FormatSize(Bytes)
    ));
    Host.Line(&format!("Index file: {} ({})", Target.display(), FormatSize(FileSize)));
    if !WalkErrors.is_empty() {
        Host.Line(&format!("{} location(s) could not be read:", WalkErrors.len()));
        for (Item, Error) in WalkErrors.iter().take(ListedErrors) {
            Host.Line(&format!("  {}: {}", Item.display(), Error));
        }
        if WalkErrors.len() > ListedErrors {
            Host.Line(&format!("  ... and {} more", WalkErrors.len() - ListedErrors));
        }
    }
    Host.Line(&format!("Completed in {:.2} s", Started.elapsed().as_secs_f64()));
    Outcome::Completed
}

fn ParseQuery(Text: &str) -> Query {
    let mut Result = Query { Terms: Vec::new(), Extensions: Vec::new(), Folders: Vec::new() };
    for Token in Text.split_whitespace() {
        let Lower = Token.to_lowercase();
        if let Some(List) = Lower.strip_prefix("ext:") {
            let Values = List.split(',').map(|Value| Value.trim().trim_start_matches('.')).filter(|Value| !Value.is_empty());
            Result.Extensions.extend(Values.map(str::to_string));
        } else if let Some(Folder) = Lower.strip_prefix("folder:") {
            if !Folder.is_empty() {
                Result.Folders.push(Folder.to_string());
            }
        } else {
            Result.Terms.push(Lower);
        }
    }
    Result
}

fn Matches(Request: &Query, Data: &Store::IndexData, Item: &Store::Entry, Scratch: &mut String) -> bool {
    if !Request.Terms.iter().all(|Term| Item.Lower.contains(Term.as_str())) {
        return false;
    }
    let Separator = Item.Lower.rfind('\\');
    if !Request.Extensions.is_empty() {
        if Item.IsDirectory {
            return false;
        }
        let Name = &Item.Lower[Separator.map(|Position| Position + 1).unwrap_or(0)..];
        let Extension = match Name.rfind('.') {
            Some(Dot) if Dot > 0 => &Name[Dot + 1..],
            _ => "",
        };
        if !Request.Extensions.iter().any(|Wanted| Wanted == Extension) {
            return false;
        }
    }
    if !Request.Folders.is_empty() {
        Scratch.clear();
        Scratch.push_str(&Data.RootLower);
        if let Some(Position) = Separator {
            if !Scratch.ends_with('\\') {
                Scratch.push('\\');
            }
            Scratch.push_str(&Item.Lower[..Position]);
        }
        if !Request.Folders.iter().all(|Folder| Scratch.contains(Folder.as_str())) {
            return false;
        }
    }
    true
}

fn Search(Host: &Host, Argument: &str) -> Outcome {
    let Started = Instant::now();
    let Request = ParseQuery(Argument);
    if Request.Terms.is_empty() && Request.Extensions.is_empty() && Request.Folders.is_empty() {
        return Outcome::Failed("Enter at least one search term, ext: filter or folder: filter.".to_string());
    }
    let Folder = match Store::IndexDirectory() {
        Ok(Folder) => Folder,
        Err(Error) => return Outcome::Failed(format!("Cannot locate the index folder: {Error}")),
    };
    let Files = match Store::IndexFiles(&Folder) {
        Ok(Files) => Files,
        Err(Error) => return Outcome::Failed(format!("Cannot list {}: {}", Folder.display(), Error)),
    };
    Store::Retain(&Files);
    if Files.is_empty() {
        Host.Line("No folders are indexed yet. Use Index: Build Index of Current Folder first.");
        return Outcome::Completed;
    }
    let mut Loaded = Vec::with_capacity(Files.len());
    for Item in &Files {
        if Host.Cancelled() {
            return Outcome::Cancelled;
        }
        match Store::Cached(Item) {
            Ok(Data) => Loaded.push(Data),
            Err(Error) => Host.Line(&format!("Skipped unreadable index {}: {}", Item.display(), Error)),
        }
    }
    Loaded.sort_by(|Left, Right| Left.RootLower.cmp(&Right.RootLower));
    let LoadTime = Started.elapsed();
    let Total: u64 = Loaded.iter().map(|Data| Data.Entries.len() as u64).sum();
    let mut Scratch = String::new();
    let mut Found = 0usize;
    let mut FoundBytes = 0u64;
    let mut Examined = 0u64;
    let mut PerIndex = Vec::with_capacity(Loaded.len());
    for Data in &Loaded {
        let mut Local = 0usize;
        for Item in &Data.Entries {
            Examined += 1;
            if Examined % 65536 == 0 {
                if Host.Cancelled() {
                    return Outcome::Cancelled;
                }
                Host.Progress(Examined, Total);
            }
            if !Matches(&Request, Data, Item, &mut Scratch) {
                continue;
            }
            if Found < MaximumResults {
                Host.Path(&Data.Header.Root.join(OsString::from_wide(&Item.Relative)));
            }
            Found += 1;
            Local += 1;
            FoundBytes += Item.Size;
        }
        PerIndex.push(Local);
    }
    Host.Progress(Total, Total);
    Host.Line(&format!("Query: {}", Argument.trim()));
    Host.Line(&format!(
        "{} match(es) ({} in files) in {} entries of {} index(es); {:.1} ms (loading {:.1} ms)",
        Found,
        FormatSize(FoundBytes),
        Total,
        Loaded.len(),
        Started.elapsed().as_secs_f64() * 1000.0,
        LoadTime.as_secs_f64() * 1000.0
    ));
    if Found > MaximumResults {
        Host.Line(&format!("Showing the first {MaximumResults} results; refine the query to see the rest."));
    }
    Host.Line("");
    for (Data, Local) in Loaded.iter().zip(PerIndex) {
        Host.Line(&format!(
            "  {}: {} match(es), {} entries, built {}",
            Data.Header.Root.display(),
            Local,
            Data.Entries.len(),
            FormatBuilt(Data.Header.BuiltAt)
        ));
    }
    Outcome::Completed
}

fn List(Host: &Host) -> Outcome {
    let Folder = match Store::IndexDirectory() {
        Ok(Folder) => Folder,
        Err(Error) => return Outcome::Failed(format!("Cannot locate the index folder: {Error}")),
    };
    let Files = match Store::IndexFiles(&Folder) {
        Ok(Files) => Files,
        Err(Error) => return Outcome::Failed(format!("Cannot list {}: {}", Folder.display(), Error)),
    };
    let mut Headers: Vec<(Store::Header, PathBuf)> = Vec::new();
    let mut Broken = Vec::new();
    for Item in Files {
        match Store::ReadHeader(&Item) {
            Ok(Header) => Headers.push((Header, Item)),
            Err(Error) => Broken.push((Item, Error)),
        }
    }
    Headers.sort_by(|Left, Right| Left.0.Root.cmp(&Right.0.Root));
    Host.Line(&format!("Index folder: {}", Folder.display()));
    Host.Line(&format!("{} indexed folder(s)", Headers.len()));
    let mut TotalEntries = 0u64;
    let mut TotalBytes = 0u64;
    for (Header, Item) in &Headers {
        let FileSize = fs::metadata(Item).map(|Data| Data.len()).unwrap_or(0);
        TotalEntries += Header.Count;
        TotalBytes += FileSize;
        Host.Line("");
        Host.Line(&format!("  {}", Header.Root.display()));
        Host.Line(&format!(
            "    {} entries, index {}, built {}",
            Header.Count,
            FormatSize(FileSize),
            FormatBuilt(Header.BuiltAt)
        ));
    }
    for (Item, Error) in &Broken {
        Host.Line(&format!("  Unreadable index {}: {}", Item.display(), Error));
    }
    Host.Line("");
    Host.Line(&format!("Total: {} entries, {} on disk", TotalEntries, FormatSize(TotalBytes)));
    Outcome::Completed
}

fn Remove(Host: &Host, Directory: &Path) -> Outcome {
    let Root = Store::NormalizeRoot(Directory);
    let Folder = match Store::IndexDirectory() {
        Ok(Folder) => Folder,
        Err(Error) => return Outcome::Failed(format!("Cannot locate the index folder: {Error}")),
    };
    let Target = match Store::IndexFile(&Folder, &Root) {
        Ok(Target) => Target,
        Err(Error) => return Outcome::Failed(format!("Cannot name the index file: {Error}")),
    };
    Store::Forget(&Target);
    match fs::remove_file(&Target) {
        Ok(()) => {
            Host.Line(&format!("Removed the index of {}", Root.display()));
            Outcome::Completed
        }
        Err(Error) if Error.kind() == std::io::ErrorKind::NotFound => {
            Host.Line(&format!("{} is not indexed.", Root.display()));
            Outcome::Completed
        }
        Err(Error) => Outcome::Failed(format!("Cannot delete {}: {}", Target.display(), Error)),
    }
}

impl Module for Index {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        match Request.Command.as_str() {
            "Build" => Build(Host, &Request.Directory),
            "Search" => Search(Host, &Request.Argument),
            "List" => List(Host),
            "Remove" => Remove(Host, &Request.Directory),
            _ => Outcome::Unsupported,
        }
    }

    fn Shutdown(&self) {
        Store::Clear();
    }
}
