#![allow(non_snake_case, non_upper_case_globals)]

use FluxSdk::Utilities::{FormatSize, HashAlgorithm, HashFile, Walk, WalkControl};
use FluxSdk::{Host, Invocation, Module, Outcome};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct FolderCompare;

static Instance: FolderCompare = FolderCompare;

FluxSdk::ExportModule!(Instance);

const TimeTolerance: Duration = Duration::from_secs(2);
const ListedErrors: usize = 50;

struct Entry {
    Relative: PathBuf,
    IsDirectory: bool,
    Size: u64,
    Modified: Option<SystemTime>,
}

struct Difference {
    Key: String,
    Reason: String,
}

struct Side {
    Root: PathBuf,
    Entries: HashMap<String, Entry>,
}

#[derive(Default)]
struct Summary {
    OnlyLeft: Vec<(String, usize)>,
    OnlyRight: Vec<(String, usize)>,
    Different: Vec<Difference>,
    Identical: usize,
    TimestampOnly: usize,
    Directories: usize,
}

fn Scan(Host: &Host, Root: &Path, Errors: &mut Vec<(PathBuf, io::Error)>) -> Option<Side> {
    let mut Entries = HashMap::new();
    let mut Stopped = false;
    let mut Count = 0u64;
    let WalkErrors = Walk(Root, &mut |Item, Data, _| {
        if Host.Cancelled() {
            Stopped = true;
            return WalkControl::Stop;
        }
        let Relative = Item.strip_prefix(Root).map(Path::to_path_buf).unwrap_or_else(|_| Item.to_path_buf());
        let Key = Relative.to_string_lossy().to_lowercase();
        Entries.insert(
            Key,
            Entry { Relative, IsDirectory: Data.is_dir(), Size: Data.len(), Modified: Data.modified().ok() },
        );
        Count += 1;
        if Count % 1024 == 0 {
            Host.Progress(Count, 0);
        }
        WalkControl::Continue
    });
    Errors.extend(WalkErrors);
    if Stopped { None } else { Some(Side { Root: Root.to_path_buf(), Entries }) }
}

fn CivilDate(Seconds: i64) -> (i64, i64, i64, i64, i64, i64) {
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
    (Year, Month, Day, Remainder / 3600, Remainder / 60 % 60, Remainder % 60)
}

fn FormatTime(Time: Option<SystemTime>) -> String {
    let Some(Time) = Time else { return "unknown time".to_string() };
    let Seconds = match Time.duration_since(UNIX_EPOCH) {
        Ok(Elapsed) => Elapsed.as_secs() as i64,
        Err(Error) => -(Error.duration().as_secs_f64().ceil() as i64),
    };
    let (Year, Month, Day, Hour, Minute, Second) = CivilDate(Seconds);
    format!("{Year:04}-{Month:02}-{Day:02} {Hour:02}:{Minute:02}:{Second:02} UTC")
}

fn TimesDiffer(Left: Option<SystemTime>, Right: Option<SystemTime>) -> bool {
    match (Left, Right) {
        (Some(LeftTime), Some(RightTime)) => {
            let Gap = LeftTime.duration_since(RightTime).or_else(|_| RightTime.duration_since(LeftTime));
            Gap.map(|Value| Value > TimeTolerance).unwrap_or(true)
        }
        (None, None) => false,
        _ => true,
    }
}

fn CollapsedAncestor<'a>(Key: &'a str, Collapsed: &HashSet<String>) -> Option<&'a str> {
    let mut Position = 0;
    while let Some(Offset) = Key[Position..].find('\\') {
        let Prefix = &Key[..Position + Offset];
        if Collapsed.contains(Prefix) {
            return Some(Prefix);
        }
        Position += Offset + 1;
    }
    None
}

fn OnlyIn(Own: &Side, Other: &Side, Mismatched: &HashSet<String>) -> Vec<(String, usize)> {
    let mut Keys: Vec<&String> = Own.Entries.keys().filter(|Key| !Other.Entries.contains_key(*Key)).collect();
    Keys.sort();
    let Collapsed: HashSet<String> = Keys
        .iter()
        .filter(|Key| Own.Entries[**Key].IsDirectory)
        .map(|Key| (*Key).clone())
        .chain(Mismatched.iter().filter(|Key| Own.Entries[*Key].IsDirectory).cloned())
        .collect();
    let mut Counts: HashMap<&str, usize> = HashMap::new();
    let mut Reported = Vec::new();
    for Key in &Keys {
        match CollapsedAncestor(Key, &Collapsed) {
            Some(Ancestor) => *Counts.entry(Ancestor).or_default() += 1,
            None => Reported.push(Key.as_str()),
        }
    }
    Reported.into_iter().map(|Key| (Key.to_string(), Counts.get(Key).copied().unwrap_or(0))).collect()
}

fn Compare(Host: &Host, Left: &Side, Right: &Side, ByContent: bool, Errors: &mut Vec<(PathBuf, io::Error)>) -> Option<Summary> {
    let mut Result = Summary::default();
    let mut Mismatched = HashSet::new();
    let mut Pending: Vec<&String> = Vec::new();
    let mut Shared: Vec<&String> = Left.Entries.keys().filter(|Key| Right.Entries.contains_key(*Key)).collect();
    Shared.sort();
    for Key in Shared {
        let LeftEntry = &Left.Entries[Key];
        let RightEntry = &Right.Entries[Key];
        if LeftEntry.IsDirectory != RightEntry.IsDirectory {
            let Reason = if LeftEntry.IsDirectory { "folder vs file" } else { "file vs folder" };
            Mismatched.insert(Key.clone());
            Result.Different.push(Difference { Key: Key.clone(), Reason: Reason.to_string() });
        } else if LeftEntry.IsDirectory {
            Result.Directories += 1;
        } else if LeftEntry.Size != RightEntry.Size {
            let Reason = format!("size {} vs {}", FormatSize(LeftEntry.Size), FormatSize(RightEntry.Size));
            Result.Different.push(Difference { Key: Key.clone(), Reason });
        } else if ByContent && LeftEntry.Size > 0 {
            Pending.push(Key);
        } else if TimesDiffer(LeftEntry.Modified, RightEntry.Modified) {
            if ByContent {
                Result.Identical += 1;
                Result.TimestampOnly += 1;
            } else {
                let Reason = format!("modified {} vs {}", FormatTime(LeftEntry.Modified), FormatTime(RightEntry.Modified));
                Result.Different.push(Difference { Key: Key.clone(), Reason });
            }
        } else {
            Result.Identical += 1;
        }
    }
    let Total: u64 = Pending.iter().map(|Key| Left.Entries[*Key].Size * 2).sum();
    let mut Done = 0u64;
    let mut Buffer = Vec::new();
    let CancelCheck = || Host.Cancelled();
    Host.Progress(0, Total);
    for Key in Pending {
        let mut Digests = Vec::with_capacity(2);
        for Owner in [Left, Right] {
            let Full = Owner.Root.join(&Owner.Entries[Key].Relative);
            let mut Advance = |Count: u64| {
                Done += Count;
                Host.Progress(Done, Total);
            };
            match HashFile(&Full, HashAlgorithm::Sha256, &mut Buffer, &CancelCheck, &mut Advance) {
                Ok(Some(Digest)) => Digests.push(Digest),
                Ok(None) => return None,
                Err(Error) => Errors.push((Full, Error)),
            }
        }
        if Digests.len() != 2 {
            Result.Different.push(Difference { Key: Key.clone(), Reason: "could not be read".to_string() });
        } else if Digests[0] != Digests[1] {
            Result.Different.push(Difference { Key: Key.clone(), Reason: "content differs".to_string() });
        } else {
            Result.Identical += 1;
            if TimesDiffer(Left.Entries[Key].Modified, Right.Entries[Key].Modified) {
                Result.TimestampOnly += 1;
            }
        }
    }
    Result.Different.sort_by(|First, Second| First.Key.cmp(&Second.Key));
    Result.OnlyLeft = OnlyIn(Left, Right, &Mismatched);
    Result.OnlyRight = OnlyIn(Right, Left, &Mismatched);
    Some(Result)
}

fn ListOnly(Host: &Host, Title: &str, Owner: &Side, Items: &[(String, usize)]) {
    if Items.is_empty() {
        return;
    }
    Host.Line("");
    Host.Line(&format!("{Title} ({}):", Items.len()));
    for (Key, Contained) in Items {
        let Item = &Owner.Entries[Key];
        if Item.IsDirectory {
            Host.Line(&format!("  {}\\ (folder, {} item(s) inside)", Item.Relative.display(), Contained));
        } else {
            Host.Line(&format!("  {} ({})", Item.Relative.display(), FormatSize(Item.Size)));
        }
    }
}

fn Run(Host: &Host, Request: &Invocation, ByContent: bool) -> Outcome {
    let Started = Instant::now();
    let Some(OtherDirectory) = &Request.OtherDirectory else {
        return Outcome::Failed("Folder compare needs the other pane to show a folder.".to_string());
    };
    let mut Errors = Vec::new();
    let Some(Left) = Scan(Host, &Request.Directory, &mut Errors) else { return Outcome::Cancelled };
    let Some(Right) = Scan(Host, OtherDirectory, &mut Errors) else { return Outcome::Cancelled };
    let Some(Result) = Compare(Host, &Left, &Right, ByContent, &mut Errors) else { return Outcome::Cancelled };
    Host.Line(&format!("Left:  {}", Left.Root.display()));
    Host.Line(&format!("Right: {}", Right.Root.display()));
    Host.Line(if ByContent {
        "Mode: size and SHA-256 content"
    } else {
        "Mode: size and modified time (2 s tolerance)"
    });
    Host.Line("");
    Host.Line(&format!(
        "Only in left: {}   Only in right: {}   Different: {}   Identical files: {}   Common folders: {}",
        Result.OnlyLeft.len(),
        Result.OnlyRight.len(),
        Result.Different.len(),
        Result.Identical,
        Result.Directories
    ));
    if ByContent && Result.TimestampOnly > 0 {
        Host.Line(&format!("{} identical file(s) have different modified times", Result.TimestampOnly));
    }
    ListOnly(Host, "Only in left", &Left, &Result.OnlyLeft);
    ListOnly(Host, "Only in right", &Right, &Result.OnlyRight);
    if !Result.Different.is_empty() {
        Host.Line("");
        Host.Line(&format!("Different ({}):", Result.Different.len()));
        for Item in &Result.Different {
            Host.Line(&format!("  {}: {}", Left.Entries[&Item.Key].Relative.display(), Item.Reason));
        }
    }
    if !Errors.is_empty() {
        Host.Line("");
        Host.Line(&format!("Errors ({}):", Errors.len()));
        for (Item, Error) in Errors.iter().take(ListedErrors) {
            Host.Line(&format!("  {}: {}", Item.display(), Error));
        }
        if Errors.len() > ListedErrors {
            Host.Line(&format!("  ... and {} more", Errors.len() - ListedErrors));
        }
    }
    Host.Line("");
    Host.Line(&format!("Completed in {:.2} s", Started.elapsed().as_secs_f64()));
    for (Key, _) in &Result.OnlyLeft {
        Host.Path(&Left.Root.join(&Left.Entries[Key].Relative));
    }
    for (Key, _) in &Result.OnlyRight {
        Host.Path(&Right.Root.join(&Right.Entries[Key].Relative));
    }
    for Item in &Result.Different {
        Host.Path(&Left.Root.join(&Left.Entries[&Item.Key].Relative));
        Host.Path(&Right.Root.join(&Right.Entries[&Item.Key].Relative));
    }
    Outcome::Completed
}

impl Module for FolderCompare {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        match Request.Command.as_str() {
            "Compare" => Run(Host, Request, false),
            "CompareContent" => Run(Host, Request, true),
            _ => Outcome::Unsupported,
        }
    }
}
