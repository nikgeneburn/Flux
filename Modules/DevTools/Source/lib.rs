#![allow(non_snake_case, non_upper_case_globals)]

mod Git;

use FluxSdk::Utilities::{FormatSize, HashAlgorithm, HashFile, Hex, IsReparsePoint, Walk, WalkControl};
use FluxSdk::{Host, Invocation, Module, Outcome, PreviewInput};
use std::fmt::Write as FmtWrite;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct DevTools;

static Instance: DevTools = DevTools;

FluxSdk::ExportModule!(Instance);

const PreviewLimit: u64 = 4096;
const BytesPerRow: usize = 16;

struct HashTarget {
    Path: PathBuf,
    Size: u64,
}

enum Collected {
    Finished,
    Stopped,
}

fn CollectTargets(
    Host: &Host,
    Selection: &[PathBuf],
    Targets: &mut Vec<HashTarget>,
    Errors: &mut Vec<(PathBuf, io::Error)>,
) -> Collected {
    for Item in Selection {
        if Host.Cancelled() {
            return Collected::Stopped;
        }
        let Data = match fs::symlink_metadata(Item) {
            Ok(Data) => Data,
            Err(Error) => {
                Errors.push((Item.clone(), Error));
                continue;
            }
        };
        if Data.is_dir() && !IsReparsePoint(&Data) {
            let mut Found = Vec::new();
            let mut Stopped = false;
            let WalkErrors = Walk(Item, &mut |Entry, EntryData, _| {
                if Host.Cancelled() {
                    Stopped = true;
                    return WalkControl::Stop;
                }
                if EntryData.is_file() {
                    Found.push(HashTarget { Path: Entry.to_path_buf(), Size: EntryData.len() });
                }
                WalkControl::Continue
            });
            if Stopped {
                return Collected::Stopped;
            }
            Errors.extend(WalkErrors);
            Found.sort_by(|Left, Right| Left.Path.cmp(&Right.Path));
            Targets.extend(Found);
            continue;
        }
        match fs::metadata(Item) {
            Ok(Target) if Target.is_file() => Targets.push(HashTarget { Path: Item.clone(), Size: Target.len() }),
            Ok(_) => Errors.push((Item.clone(), io::Error::other("not a regular file"))),
            Err(Error) => Errors.push((Item.clone(), Error)),
        }
    }
    Collected::Finished
}

fn DisplayPath(Item: &Path, Base: &Path) -> String {
    match Item.strip_prefix(Base) {
        Ok(Relative) if !Relative.as_os_str().is_empty() => Relative.display().to_string(),
        _ => Item.display().to_string(),
    }
}

fn HashSelection(Host: &Host, Request: &Invocation, Algorithm: HashAlgorithm) -> Outcome {
    let Started = Instant::now();
    let mut Targets = Vec::new();
    let mut Errors = Vec::new();
    if let Collected::Stopped = CollectTargets(Host, &Request.Selection, &mut Targets, &mut Errors) {
        return Outcome::Cancelled;
    }
    let Total: u64 = Targets.iter().map(|Target| Target.Size).sum();
    Host.Line(&format!("{} of {} file(s), {}", Algorithm.Name(), Targets.len(), FormatSize(Total)));
    Host.Line("");
    let mut Buffer = Vec::new();
    let mut Done = 0u64;
    let mut Hashed = 0usize;
    let mut HashedBytes = 0u64;
    let CancelCheck = || Host.Cancelled();
    Host.Progress(0, Total);
    for Target in &Targets {
        let mut Advance = |Count: u64| {
            Done += Count;
            Host.Progress(Done, Total);
        };
        match HashFile(&Target.Path, Algorithm, &mut Buffer, &CancelCheck, &mut Advance) {
            Ok(Some(Digest)) => {
                Host.Line(&format!("{}  {}", Hex(&Digest), DisplayPath(&Target.Path, &Request.Directory)));
                Hashed += 1;
                HashedBytes += Target.Size;
            }
            Ok(None) => return Outcome::Cancelled,
            Err(Error) => Errors.push((Target.Path.clone(), Error)),
        }
    }
    Host.Line("");
    if Targets.is_empty() {
        Host.Line("No files to hash.");
    }
    if !Errors.is_empty() {
        Host.Line(&format!("Failed ({}):", Errors.len()));
        for (Item, Error) in &Errors {
            Host.Line(&format!("  {}: {}", Item.display(), Error));
        }
    }
    Host.Line(&format!(
        "Hashed {} of {} file(s), {} in {:.2} s",
        Hashed,
        Targets.len(),
        FormatSize(HashedBytes),
        Started.elapsed().as_secs_f64()
    ));
    Outcome::Completed
}

fn AppendHexRow(Output: &mut String, Offset: usize, Row: &[u8]) {
    let _ = write!(Output, "{Offset:08x}  ");
    for Column in 0..BytesPerRow {
        match Row.get(Column) {
            Some(Byte) => {
                let _ = write!(Output, "{Byte:02x} ");
            }
            None => Output.push_str("   "),
        }
        if Column == BytesPerRow / 2 - 1 {
            Output.push(' ');
        }
    }
    Output.push_str(" |");
    for Byte in Row {
        Output.push(if Byte.is_ascii_graphic() || *Byte == b' ' { *Byte as char } else { '.' });
    }
    Output.push_str("|\n");
}

fn HexPreview(Host: &Host, Request: &PreviewInput) -> Outcome {
    let Data = match fs::metadata(&Request.Path) {
        Ok(Data) => Data,
        Err(Error) => return Outcome::Failed(format!("Cannot read {}: {}", Request.Path.display(), Error)),
    };
    if !Data.is_file() {
        return Outcome::Unsupported;
    }
    let Limit = if Request.MaxBytes == 0 { PreviewLimit } else { Request.MaxBytes.min(PreviewLimit) };
    let mut Bytes = Vec::with_capacity(Limit as usize);
    let Result = File::open(&Request.Path).and_then(|Source| Source.take(Limit).read_to_end(&mut Bytes));
    if let Err(Error) = Result {
        return Outcome::Failed(format!("Cannot read {}: {}", Request.Path.display(), Error));
    }
    let Name = Request.Path.file_name().map(|Name| Name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut Output = String::with_capacity(Bytes.len() * 5 + 256);
    let _ = writeln!(Output, "{Name}");
    let _ = writeln!(Output, "Size: {} ({} bytes)", FormatSize(Data.len()), Data.len());
    let _ = writeln!(Output, "Showing first {} byte(s)", Bytes.len());
    Output.push('\n');
    for (Index, Row) in Bytes.chunks(BytesPerRow).enumerate() {
        AppendHexRow(&mut Output, Index * BytesPerRow, Row);
    }
    if (Bytes.len() as u64) < Data.len() {
        let _ = writeln!(Output, "{:08x}  ...", Bytes.len());
    }
    Host.Text(&Output);
    Outcome::Completed
}

impl Module for DevTools {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        match Request.Command.as_str() {
            "Sha256" => HashSelection(Host, Request, HashAlgorithm::Sha256),
            "Sha1" => HashSelection(Host, Request, HashAlgorithm::Sha1),
            "Md5" => HashSelection(Host, Request, HashAlgorithm::Md5),
            "GitStatus" => Git::Status(Host, &Request.Directory),
            _ => Outcome::Unsupported,
        }
    }

    fn Preview(&self, Host: &Host, Request: &PreviewInput) -> Outcome {
        HexPreview(Host, Request)
    }
}
