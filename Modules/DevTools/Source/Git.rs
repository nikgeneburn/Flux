use FluxSdk::{Host, Outcome};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const CreateNoWindow: u32 = 0x0800_0000;
const PollInterval: Duration = Duration::from_millis(15);

enum GitFailure {
    Missing,
    Cancelled,
    Io(io::Error),
    Exit(String),
}

struct Change {
    Code: char,
    Path: String,
    Original: Option<String>,
}

#[derive(Default)]
struct Report {
    Oid: String,
    Head: String,
    Upstream: Option<String>,
    Ahead: Option<String>,
    Behind: Option<String>,
    Staged: Vec<Change>,
    Unstaged: Vec<Change>,
    Unmerged: Vec<(String, String)>,
    Untracked: Vec<String>,
}

type Reader = JoinHandle<io::Result<Vec<u8>>>;

fn ReadAll<Source: Read>(Pipe: Option<Source>) -> io::Result<Vec<u8>> {
    let mut Bytes = Vec::new();
    if let Some(mut Stream) = Pipe {
        Stream.read_to_end(&mut Bytes)?;
    }
    Ok(Bytes)
}

fn Abandon(Process: &mut Child, OutputReader: Reader, ErrorReader: Reader) {
    let _ = Process.kill();
    let _ = Process.wait();
    let _ = OutputReader.join();
    let _ = ErrorReader.join();
}

fn Collect(ReaderThread: Reader) -> io::Result<Vec<u8>> {
    ReaderThread.join().unwrap_or_else(|_| Err(io::Error::other("pipe reader failed")))
}

fn RunGit(Host: &Host, Directory: &Path, Arguments: &[&str]) -> Result<Vec<u8>, GitFailure> {
    let Spawned = Command::new("git")
        .args(Arguments)
        .current_dir(Directory)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CreateNoWindow)
        .spawn();
    let mut Process = match Spawned {
        Ok(Process) => Process,
        Err(Error) if Error.kind() == io::ErrorKind::NotFound => return Err(GitFailure::Missing),
        Err(Error) => return Err(GitFailure::Io(Error)),
    };
    let OutputPipe = Process.stdout.take();
    let ErrorPipe = Process.stderr.take();
    let OutputReader: Reader = thread::spawn(move || ReadAll(OutputPipe));
    let ErrorReader: Reader = thread::spawn(move || ReadAll(ErrorPipe));
    let ExitStatus = loop {
        if Host.Cancelled() {
            Abandon(&mut Process, OutputReader, ErrorReader);
            return Err(GitFailure::Cancelled);
        }
        match Process.try_wait() {
            Ok(Some(ExitStatus)) => break ExitStatus,
            Ok(None) => thread::sleep(PollInterval),
            Err(Error) => {
                Abandon(&mut Process, OutputReader, ErrorReader);
                return Err(GitFailure::Io(Error));
            }
        }
    };
    let Output = Collect(OutputReader).map_err(GitFailure::Io)?;
    let ErrorText = Collect(ErrorReader).unwrap_or_default();
    if !ExitStatus.success() {
        return Err(GitFailure::Exit(String::from_utf8_lossy(&ErrorText).trim().to_string()));
    }
    Ok(Output)
}

fn ChangeName(Code: char) -> &'static str {
    match Code {
        'M' => "modified",
        'T' => "type changed",
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        _ => "changed",
    }
}

fn ConflictName(Code: &str) -> &'static str {
    match Code {
        "DD" => "both deleted",
        "AU" => "added by us",
        "UD" => "deleted by them",
        "UA" => "added by them",
        "DU" => "deleted by us",
        "AA" => "both added",
        "UU" => "both modified",
        _ => "unmerged",
    }
}

fn PushChange(Report: &mut Report, Codes: &str, ItemPath: &str, Original: Option<&str>) {
    let mut Letters = Codes.chars();
    let Index = Letters.next().unwrap_or('.');
    let Worktree = Letters.next().unwrap_or('.');
    if Index != '.' {
        Report.Staged.push(Change { Code: Index, Path: ItemPath.to_string(), Original: Original.map(str::to_string) });
    }
    if Worktree != '.' {
        Report.Unstaged.push(Change { Code: Worktree, Path: ItemPath.to_string(), Original: None });
    }
}

fn ParseHeader(Report: &mut Report, Header: &str) {
    if let Some(Value) = Header.strip_prefix("branch.oid ") {
        Report.Oid = Value.to_string();
    } else if let Some(Value) = Header.strip_prefix("branch.head ") {
        Report.Head = Value.to_string();
    } else if let Some(Value) = Header.strip_prefix("branch.upstream ") {
        Report.Upstream = Some(Value.to_string());
    } else if let Some(Value) = Header.strip_prefix("branch.ab ") {
        let mut Parts = Value.split(' ');
        Report.Ahead = Parts.next().map(|Part| Part.trim_start_matches('+').to_string());
        Report.Behind = Parts.next().map(|Part| Part.trim_start_matches('-').to_string());
    }
}

fn ParseStatus(Output: &[u8]) -> Report {
    let Text = String::from_utf8_lossy(Output);
    let mut Tokens = Text.split('\0').filter(|Token| !Token.is_empty());
    let mut Report = Report::default();
    while let Some(Token) = Tokens.next() {
        if let Some(Header) = Token.strip_prefix("# ") {
            ParseHeader(&mut Report, Header);
        } else if let Some(Rest) = Token.strip_prefix("1 ") {
            let Fields: Vec<&str> = Rest.splitn(8, ' ').collect();
            if Fields.len() == 8 {
                PushChange(&mut Report, Fields[0], Fields[7], None);
            }
        } else if let Some(Rest) = Token.strip_prefix("2 ") {
            let Fields: Vec<&str> = Rest.splitn(9, ' ').collect();
            let Original = Tokens.next();
            if Fields.len() == 9 {
                PushChange(&mut Report, Fields[0], Fields[8], Original);
            }
        } else if let Some(Rest) = Token.strip_prefix("u ") {
            let Fields: Vec<&str> = Rest.splitn(10, ' ').collect();
            if Fields.len() == 10 {
                Report.Unmerged.push((Fields[0].to_string(), Fields[9].to_string()));
            }
        } else if let Some(Rest) = Token.strip_prefix("? ") {
            Report.Untracked.push(Rest.to_string());
        }
    }
    Report
}

fn ShortOid(Oid: &str) -> &str {
    if Oid.len() > 10 && Oid.is_ascii() { &Oid[..10] } else { Oid }
}

fn BranchLine(Report: &Report) -> String {
    if Report.Head == "(detached)" {
        format!("Branch: detached HEAD at {}", ShortOid(&Report.Oid))
    } else if Report.Oid == "(initial)" {
        format!("Branch: {} (no commits yet)", Report.Head)
    } else {
        format!("Branch: {} at {}", Report.Head, ShortOid(&Report.Oid))
    }
}

fn UpstreamLine(Report: &Report) -> String {
    match &Report.Upstream {
        None => "Upstream: none".to_string(),
        Some(Upstream) => match (&Report.Ahead, &Report.Behind) {
            (Some(Ahead), Some(Behind)) => format!("Upstream: {Upstream}, ahead {Ahead}, behind {Behind}"),
            _ => format!("Upstream: {Upstream} (gone)"),
        },
    }
}

fn FullPath(Top: &Path, Relative: &str) -> PathBuf {
    Top.join(Relative.trim_end_matches('/').replace('/', "\\"))
}

fn EmitChanges(Host: &Host, Title: &str, Changes: &[Change]) {
    if Changes.is_empty() {
        return;
    }
    Host.Line("");
    Host.Line(&format!("{Title} ({}):", Changes.len()));
    for Item in Changes {
        match &Item.Original {
            Some(Original) => Host.Line(&format!("  {:<13}{} -> {}", ChangeName(Item.Code), Original, Item.Path)),
            None => Host.Line(&format!("  {:<13}{}", ChangeName(Item.Code), Item.Path)),
        }
    }
}

fn EmitPaths(Host: &Host, Top: &Path, Report: &Report) {
    let mut Seen = HashSet::new();
    let Relatives = Report
        .Staged
        .iter()
        .chain(Report.Unstaged.iter())
        .map(|Item| Item.Path.as_str())
        .chain(Report.Unmerged.iter().map(|(_, Item)| Item.as_str()))
        .chain(Report.Untracked.iter().map(String::as_str));
    for Relative in Relatives {
        if !Seen.insert(Relative.to_lowercase()) {
            continue;
        }
        let Item = FullPath(Top, Relative);
        if fs::symlink_metadata(&Item).is_ok() {
            Host.Path(&Item);
        }
    }
}

pub fn Status(Host: &Host, Directory: &Path) -> Outcome {
    let Top = match RunGit(Host, Directory, &["rev-parse", "--show-toplevel"]) {
        Ok(Output) => PathBuf::from(String::from_utf8_lossy(&Output).trim().replace('/', "\\")),
        Err(GitFailure::Missing) => {
            Host.Line("Git is not installed or git.exe is not on PATH.");
            return Outcome::Completed;
        }
        Err(GitFailure::Cancelled) => return Outcome::Cancelled,
        Err(GitFailure::Io(Error)) => return Outcome::Failed(format!("Cannot run git: {Error}")),
        Err(GitFailure::Exit(Message)) => {
            Host.Line(&format!("{} is not inside a Git working tree.", Directory.display()));
            if !Message.is_empty() {
                Host.Line(&format!("git: {Message}"));
            }
            return Outcome::Completed;
        }
    };
    let Output = match RunGit(Host, Directory, &["status", "--porcelain=v2", "--branch", "-z"]) {
        Ok(Output) => Output,
        Err(GitFailure::Missing) => return Outcome::Failed("git.exe disappeared from PATH".to_string()),
        Err(GitFailure::Cancelled) => return Outcome::Cancelled,
        Err(GitFailure::Io(Error)) => return Outcome::Failed(format!("Cannot run git: {Error}")),
        Err(GitFailure::Exit(Message)) => return Outcome::Failed(format!("git status failed: {Message}")),
    };
    let Report = ParseStatus(&Output);
    Host.Line(&format!("Repository: {}", Top.display()));
    Host.Line(&BranchLine(&Report));
    Host.Line(&UpstreamLine(&Report));
    let Total = Report.Staged.len() + Report.Unstaged.len() + Report.Unmerged.len() + Report.Untracked.len();
    if Total == 0 {
        Host.Line("");
        Host.Line("Working tree clean.");
        return Outcome::Completed;
    }
    if !Report.Unmerged.is_empty() {
        Host.Line("");
        Host.Line(&format!("Unmerged ({}):", Report.Unmerged.len()));
        for (Code, Item) in &Report.Unmerged {
            Host.Line(&format!("  {:<16}{}", ConflictName(Code), Item));
        }
    }
    EmitChanges(Host, "Staged changes", &Report.Staged);
    EmitChanges(Host, "Unstaged changes", &Report.Unstaged);
    if !Report.Untracked.is_empty() {
        Host.Line("");
        Host.Line(&format!("Untracked ({}):", Report.Untracked.len()));
        for Item in &Report.Untracked {
            Host.Line(&format!("  {Item}"));
        }
    }
    EmitPaths(Host, &Top, &Report);
    Outcome::Completed
}
