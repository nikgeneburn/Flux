#![allow(non_snake_case, non_upper_case_globals)]

use FluxSdk::Utilities::{FormatSize, HashAlgorithm, HashFile, Hasher, Hex, IsReparsePoint, Walk, WalkControl};
use FluxSdk::{Host, Invocation, Module, Outcome};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Duplicates;

static Instance: Duplicates = Duplicates;

FluxSdk::ExportModule!(Instance);

const PrefixLength: usize = 64 * 1024;
const ScanProgressStep: u64 = 1024;
const ListedErrors: usize = 50;

struct Group {
    Size: u64,
    Hash: Vec<u8>,
    Members: Vec<PathBuf>,
}

struct Scan {
    Files: u64,
    Bytes: u64,
    BySize: HashMap<u64, Vec<PathBuf>>,
}

fn ParseSize(Text: &str) -> Option<u64> {
    let Trimmed = Text.trim();
    let Split = Trimmed.find(|Symbol: char| !(Symbol.is_ascii_digit() || Symbol == '.')).unwrap_or(Trimmed.len());
    let (Number, Unit) = Trimmed.split_at(Split);
    let Value: f64 = Number.parse().ok()?;
    let Multiplier: u64 = match Unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return None,
    };
    let Bytes = Value * Multiplier as f64;
    if !Bytes.is_finite() || Bytes < 0.0 || Bytes >= u64::MAX as f64 {
        return None;
    }
    Some(Bytes.round() as u64)
}

fn Roots(Request: &Invocation) -> Vec<PathBuf> {
    let Folders: Vec<PathBuf> = Request
        .Selection
        .iter()
        .filter(|Item| fs::symlink_metadata(Item).map(|Data| Data.is_dir() && !IsReparsePoint(&Data)).unwrap_or(false))
        .cloned()
        .collect();
    if Folders.is_empty() { vec![Request.Directory.clone()] } else { Folders }
}

fn ScanRoots(Host: &Host, Roots: &[PathBuf], Minimum: u64, Errors: &mut Vec<(PathBuf, io::Error)>) -> Option<Scan> {
    let mut Result = Scan { Files: 0, Bytes: 0, BySize: HashMap::new() };
    let mut Seen = HashSet::new();
    let mut Stopped = false;
    for Root in Roots {
        let WalkErrors = Walk(Root, &mut |Item, Data, _| {
            if Host.Cancelled() {
                Stopped = true;
                return WalkControl::Stop;
            }
            if IsReparsePoint(Data) {
                return WalkControl::SkipChildren;
            }
            if !Data.is_file() {
                return WalkControl::Continue;
            }
            let Size = Data.len();
            if Size == 0 || Size < Minimum || !Seen.insert(Item.to_string_lossy().to_lowercase()) {
                return WalkControl::Continue;
            }
            Result.Files += 1;
            Result.Bytes += Size;
            Result.BySize.entry(Size).or_default().push(Item.to_path_buf());
            if Result.Files % ScanProgressStep == 0 {
                Host.Progress(Result.Files, 0);
            }
            WalkControl::Continue
        });
        Errors.extend(WalkErrors);
        if Stopped {
            return None;
        }
    }
    Some(Result)
}

fn HashPrefix(Item: &Path, Buffer: &mut [u8], Expected: usize) -> io::Result<Vec<u8>> {
    let mut Source = File::open(Item)?;
    let mut Filled = 0;
    while Filled < Buffer.len() {
        match Source.read(&mut Buffer[Filled..]) {
            Ok(0) => break,
            Ok(Count) => Filled += Count,
            Err(Error) if Error.kind() == io::ErrorKind::Interrupted => continue,
            Err(Error) => return Err(Error),
        }
    }
    if Filled != Expected {
        return Err(io::Error::other("file changed during the scan"));
    }
    let mut State = Hasher::New(HashAlgorithm::Sha256)?;
    State.Update(&Buffer[..Filled])?;
    State.Finish()
}

fn SplitByHash(Members: Vec<(Vec<u8>, PathBuf)>) -> Vec<(Vec<u8>, Vec<PathBuf>)> {
    let mut ByHash: HashMap<Vec<u8>, Vec<PathBuf>> = HashMap::new();
    for (Digest, Item) in Members {
        ByHash.entry(Digest).or_default().push(Item);
    }
    ByHash.into_iter().filter(|(_, Items)| Items.len() > 1).collect()
}

fn FindGroups(
    Host: &Host,
    Candidates: Vec<(u64, Vec<PathBuf>)>,
    Errors: &mut Vec<(PathBuf, io::Error)>,
) -> Option<Vec<Group>> {
    let PrefixTotal: u64 = Candidates.iter().map(|(Size, Items)| (*Size).min(PrefixLength as u64) * Items.len() as u64).sum();
    let mut PrefixBuffer = vec![0u8; PrefixLength];
    let mut Done = 0u64;
    let mut Groups = Vec::new();
    let mut FullPending: Vec<(u64, Vec<PathBuf>)> = Vec::new();
    Host.Progress(0, PrefixTotal);
    for (Size, Items) in Candidates {
        let Expected = Size.min(PrefixLength as u64) as usize;
        let mut Hashed = Vec::with_capacity(Items.len());
        for Item in Items {
            if Host.Cancelled() {
                return None;
            }
            match HashPrefix(&Item, &mut PrefixBuffer[..Expected], Expected) {
                Ok(Digest) => Hashed.push((Digest, Item)),
                Err(Error) => Errors.push((Item, Error)),
            }
            Done += Expected as u64;
            Host.Progress(Done, PrefixTotal);
        }
        for (Digest, Members) in SplitByHash(Hashed) {
            if Size as usize <= PrefixLength {
                Groups.push(Group { Size, Hash: Digest, Members });
            } else {
                FullPending.push((Size, Members));
            }
        }
    }
    let FullTotal: u64 = FullPending.iter().map(|(Size, Items)| *Size * Items.len() as u64).sum();
    let mut Buffer = Vec::new();
    let CancelCheck = || Host.Cancelled();
    Done = 0;
    Host.Progress(0, FullTotal);
    for (Size, Items) in FullPending {
        let mut Hashed = Vec::with_capacity(Items.len());
        for Item in Items {
            let mut Advance = |Count: u64| {
                Done += Count;
                Host.Progress(Done, FullTotal);
            };
            match HashFile(&Item, HashAlgorithm::Sha256, &mut Buffer, &CancelCheck, &mut Advance) {
                Ok(Some(Digest)) => Hashed.push((Digest, Item)),
                Ok(None) => return None,
                Err(Error) => Errors.push((Item, Error)),
            }
        }
        for (Digest, Members) in SplitByHash(Hashed) {
            Groups.push(Group { Size, Hash: Digest, Members });
        }
    }
    Some(Groups)
}

fn FindDuplicates(Host: &Host, Request: &Invocation, Minimum: u64) -> Outcome {
    let Started = Instant::now();
    let Roots = Roots(Request);
    let mut Errors = Vec::new();
    let Some(Scanned) = ScanRoots(Host, &Roots, Minimum, &mut Errors) else { return Outcome::Cancelled };
    let mut Candidates: Vec<(u64, Vec<PathBuf>)> =
        Scanned.BySize.into_iter().filter(|(_, Items)| Items.len() > 1).collect();
    Candidates.sort_by(|Left, Right| Right.0.cmp(&Left.0));
    let CandidateCount: usize = Candidates.iter().map(|(_, Items)| Items.len()).sum();
    let Some(mut Groups) = FindGroups(Host, Candidates, &mut Errors) else { return Outcome::Cancelled };
    for Entry in &mut Groups {
        Entry.Members.sort();
    }
    Groups.sort_by(|Left, Right| {
        let LeftWaste = Left.Size * (Left.Members.len() as u64 - 1);
        let RightWaste = Right.Size * (Right.Members.len() as u64 - 1);
        RightWaste.cmp(&LeftWaste).then_with(|| Left.Members[0].cmp(&Right.Members[0]))
    });
    let Reclaimable: u64 = Groups.iter().map(|Entry| Entry.Size * (Entry.Members.len() as u64 - 1)).sum();
    let DuplicateFiles: usize = Groups.iter().map(|Entry| Entry.Members.len()).sum();
    for Root in &Roots {
        Host.Line(&format!("Scanned: {}", Root.display()));
    }
    if Minimum > 0 {
        Host.Line(&format!("Minimum size: {}", FormatSize(Minimum)));
    }
    Host.Line(&format!(
        "{} file(s), {} examined; {} share a size with another file",
        Scanned.Files,
        FormatSize(Scanned.Bytes),
        CandidateCount
    ));
    Host.Line(&format!(
        "Found {} duplicate group(s) with {} file(s); reclaimable space {}",
        Groups.len(),
        DuplicateFiles,
        FormatSize(Reclaimable)
    ));
    for (Index, Entry) in Groups.iter().enumerate() {
        let Hash = Hex(&Entry.Hash);
        Host.Line("");
        Host.Line(&format!(
            "Group {}: {} files x {}, reclaimable {}, SHA-256 {}",
            Index + 1,
            Entry.Members.len(),
            FormatSize(Entry.Size),
            FormatSize(Entry.Size * (Entry.Members.len() as u64 - 1)),
            &Hash[..16]
        ));
        for Member in &Entry.Members {
            Host.Line(&format!("  {}", Member.display()));
        }
    }
    if !Errors.is_empty() {
        Host.Line("");
        Host.Line(&format!("Skipped {} item(s) because of errors:", Errors.len()));
        for (Item, Error) in Errors.iter().take(ListedErrors) {
            Host.Line(&format!("  {}: {}", Item.display(), Error));
        }
        if Errors.len() > ListedErrors {
            Host.Line(&format!("  ... and {} more", Errors.len() - ListedErrors));
        }
    }
    Host.Line("");
    Host.Line(&format!("Completed in {:.2} s", Started.elapsed().as_secs_f64()));
    for Entry in &Groups {
        for Member in &Entry.Members {
            Host.Path(Member);
        }
    }
    Outcome::Completed
}

impl Module for Duplicates {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        match Request.Command.as_str() {
            "Find" => FindDuplicates(Host, Request, 0),
            "FindLarge" => match ParseSize(&Request.Argument) {
                Some(Minimum) => FindDuplicates(Host, Request, Minimum),
                None => Outcome::Failed(format!(
                    "Invalid minimum size \"{}\"; use a number with an optional unit such as 500KB, 1MB or 2.5GB",
                    Request.Argument.trim()
                )),
            },
            _ => Outcome::Unsupported,
        }
    }
}
