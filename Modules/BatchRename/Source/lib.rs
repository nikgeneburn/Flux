#![allow(non_snake_case, non_upper_case_globals)]

use FluxSdk::{Host, Invocation, Module, Outcome};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

struct BatchRename;

static Instance: BatchRename = BatchRename;

FluxSdk::ExportModule!(Instance);

const InvalidCharacters: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
const ReservedNames: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹",
    "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];
const MaximumNameLength: usize = 255;
const MaximumCounterWidth: usize = 20;
const UnixEpochTicks: u128 = 116_444_736_000_000_000;

enum Piece {
    Literal(String),
    Stem,
    Extension,
    Counter { Width: usize, Start: i64 },
    Parent,
    Date,
}

enum Transform {
    Template(Vec<Piece>),
    Replace { Old: Vec<char>, New: String },
    Lower,
    Upper,
}

struct Item {
    From: PathBuf,
    To: PathBuf,
    OldName: String,
    NewName: String,
}

struct Source<'a> {
    Path: &'a Path,
    Name: &'a str,
    IsDirectory: bool,
    Modified: Option<SystemTime>,
}

fn ParseToken(Token: &str) -> Result<Piece, String> {
    let Lower = Token.trim().to_lowercase();
    match Lower.as_str() {
        "name" => return Ok(Piece::Stem),
        "ext" => return Ok(Piece::Extension),
        "parent" => return Ok(Piece::Parent),
        "date" => return Ok(Piece::Date),
        "n" => return Ok(Piece::Counter { Width: 1, Start: 1 }),
        _ => {}
    }
    let Some(Options) = Lower.strip_prefix("n:") else {
        return Err(format!("Unknown token {{{Token}}}"));
    };
    let mut Parts = Options.split(':');
    let WidthText = Parts.next().unwrap_or("").trim();
    let Width = if WidthText.is_empty() {
        1
    } else {
        match WidthText.parse::<usize>() {
            Ok(Value) if (1..=MaximumCounterWidth).contains(&Value) => Value,
            _ => return Err(format!("Counter width in {{{Token}}} must be between 1 and {MaximumCounterWidth}")),
        }
    };
    let Start = match Parts.next().map(str::trim) {
        None | Some("") => 1,
        Some(StartText) => StartText.parse::<i64>().map_err(|_| format!("Counter start in {{{Token}}} is not a number"))?,
    };
    if Parts.next().is_some() {
        return Err(format!("Too many options in {{{Token}}}"));
    }
    Ok(Piece::Counter { Width, Start })
}

fn ParseTemplate(Text: &str) -> Result<Vec<Piece>, String> {
    let mut Pieces = Vec::new();
    let mut Literal = String::new();
    let mut Rest = Text;
    while let Some(Open) = Rest.find('{') {
        Literal.push_str(&Rest[..Open]);
        let After = &Rest[Open + 1..];
        let Close = After.find('}').ok_or_else(|| "Pattern has a '{' without a matching '}'".to_string())?;
        let Token = ParseToken(&After[..Close])?;
        if !Literal.is_empty() {
            Pieces.push(Piece::Literal(std::mem::take(&mut Literal)));
        }
        Pieces.push(Token);
        Rest = &After[Close + 1..];
    }
    Literal.push_str(Rest);
    if !Literal.is_empty() {
        Pieces.push(Piece::Literal(Literal));
    }
    Ok(Pieces)
}

fn ParseTransform(Argument: &str) -> Result<Transform, String> {
    if let Some((Old, New)) = Argument.split_once("=>") {
        if Old.is_empty() {
            return Err("The text to replace (before =>) is empty".to_string());
        }
        return Ok(Transform::Replace { Old: Old.chars().collect(), New: New.to_string() });
    }
    let Trimmed = Argument.trim();
    if Trimmed.is_empty() {
        return Err("Enter a pattern such as {name}_{n:3}.{ext} or old=>new".to_string());
    }
    ParseTemplate(Trimmed).map(Transform::Template)
}

fn LocalDate(Time: Option<SystemTime>) -> String {
    let Fallback = "0000-00-00".to_string();
    let Some(Ticks) = Time.and_then(|Value| Value.duration_since(UNIX_EPOCH).ok()).map(|Value| Value.as_nanos() / 100 + UnixEpochTicks)
    else {
        return Fallback;
    };
    let File = FILETIME { dwLowDateTime: Ticks as u32, dwHighDateTime: (Ticks >> 32) as u32 };
    let mut Universal = SYSTEMTIME::default();
    let mut Local = SYSTEMTIME::default();
    let Converted = unsafe {
        FileTimeToSystemTime(&File, &mut Universal).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &Universal, &mut Local).is_ok()
    };
    if !Converted {
        return Fallback;
    }
    format!("{:04}-{:02}-{:02}", Local.wYear, Local.wMonth, Local.wDay)
}

fn SplitName(Name: &str, IsDirectory: bool) -> (&str, &str) {
    if IsDirectory {
        return (Name, "");
    }
    match Name.rfind('.') {
        Some(Dot) if Dot > 0 => (&Name[..Dot], &Name[Dot + 1..]),
        _ => (Name, ""),
    }
}

fn CharactersMatch(Left: char, Right: char) -> bool {
    Left == Right || Left.to_lowercase().eq(Right.to_lowercase())
}

fn ReplaceIgnoreCase(Name: &str, Old: &[char], New: &str) -> String {
    let Characters: Vec<char> = Name.chars().collect();
    let mut Output = String::with_capacity(Name.len());
    let mut Position = 0;
    while Position < Characters.len() {
        let End = Position + Old.len();
        if End <= Characters.len() && Characters[Position..End].iter().zip(Old).all(|(A, B)| CharactersMatch(*A, *B)) {
            Output.push_str(New);
            Position = End;
        } else {
            Output.push(Characters[Position]);
            Position += 1;
        }
    }
    Output
}

fn FormatCounter(Value: i64, Width: usize) -> String {
    if Value < 0 { format!("-{:0Width$}", Value.unsigned_abs()) } else { format!("{Value:0Width$}") }
}

fn Render(Pieces: &[Piece], Entry: &Source, Index: usize) -> String {
    let (Stem, Extension) = SplitName(Entry.Name, Entry.IsDirectory);
    let mut Output = String::new();
    for Part in Pieces {
        match Part {
            Piece::Literal(Text) => Output.push_str(Text),
            Piece::Stem => Output.push_str(Stem),
            Piece::Extension => {
                if Extension.is_empty() {
                    if Output.ends_with('.') {
                        Output.pop();
                    }
                } else {
                    Output.push_str(Extension);
                }
            }
            Piece::Counter { Width, Start } => {
                Output.push_str(&FormatCounter(Start.saturating_add(Index as i64), *Width));
            }
            Piece::Parent => {
                let ParentName = Entry.Path.parent().and_then(Path::file_name).map(|Value| Value.to_string_lossy());
                Output.push_str(ParentName.as_deref().unwrap_or(""));
            }
            Piece::Date => Output.push_str(&LocalDate(Entry.Modified)),
        }
    }
    Output
}

fn Apply(Rule: &Transform, Entry: &Source, Index: usize) -> String {
    match Rule {
        Transform::Template(Pieces) => Render(Pieces, Entry, Index),
        Transform::Replace { Old, New } => ReplaceIgnoreCase(Entry.Name, Old, New),
        Transform::Lower => Entry.Name.to_lowercase(),
        Transform::Upper => Entry.Name.to_uppercase(),
    }
}

fn NameProblem(Name: &str) -> Option<String> {
    if Name.is_empty() {
        return Some("new name is empty".to_string());
    }
    if let Some(Bad) = Name.chars().find(|Symbol| InvalidCharacters.contains(Symbol) || (*Symbol as u32) < 32) {
        return Some(if (Bad as u32) < 32 {
            "contains a control character".to_string()
        } else {
            format!("contains the invalid character '{Bad}'")
        });
    }
    if Name.ends_with('.') || Name.ends_with(' ') {
        return Some("ends with a dot or space".to_string());
    }
    if Name.encode_utf16().count() > MaximumNameLength {
        return Some(format!("is longer than {MaximumNameLength} characters"));
    }
    let Base = Name.split('.').next().unwrap_or("").trim_end().to_uppercase();
    if ReservedNames.contains(&Base.as_str()) {
        return Some(format!("uses the reserved device name {Base}"));
    }
    None
}

fn PathKey(Item: &Path) -> String {
    Item.to_string_lossy().to_lowercase()
}

fn OrderRenames(Items: &[Item], Problems: &mut Vec<String>) -> Vec<usize> {
    let Sources: HashMap<String, usize> = Items.iter().enumerate().map(|(Index, Entry)| (PathKey(&Entry.From), Index)).collect();
    let Prerequisite: Vec<Option<usize>> = Items
        .iter()
        .enumerate()
        .map(|(Index, Entry)| Sources.get(&PathKey(&Entry.To)).copied().filter(|Other| *Other != Index))
        .collect();
    let mut State = vec![0u8; Items.len()];
    let mut Order = Vec::with_capacity(Items.len());
    for Start in 0..Items.len() {
        let mut Chain = Vec::new();
        let mut Current = Some(Start);
        while let Some(Position) = Current {
            if State[Position] == 2 {
                break;
            }
            if State[Position] == 1 {
                let Names: Vec<&str> = Chain.iter().map(|Index: &usize| Items[*Index].OldName.as_str()).collect();
                Problems.push(format!("{} form a rename cycle; rename them in two steps", Names.join(", ")));
                return Vec::new();
            }
            State[Position] = 1;
            Chain.push(Position);
            Current = Prerequisite[Position];
        }
        for Position in Chain.into_iter().rev() {
            State[Position] = 2;
            Order.push(Position);
        }
    }
    Order
}

fn Plan(Host: &Host, Request: &Invocation, Rule: &Transform) -> Outcome {
    if Request.Selection.is_empty() {
        return Outcome::Failed("Select the items to rename.".to_string());
    }
    let mut Problems = Vec::new();
    let mut Items = Vec::with_capacity(Request.Selection.len());
    let mut Selected = HashSet::new();
    for (Index, From) in Request.Selection.iter().enumerate() {
        if Host.Cancelled() {
            return Outcome::Cancelled;
        }
        Host.Progress(Index as u64, Request.Selection.len() as u64);
        if !Selected.insert(PathKey(From)) {
            continue;
        }
        let Data = match fs::symlink_metadata(From) {
            Ok(Data) => Data,
            Err(Error) => {
                Problems.push(format!("{}: {}", From.display(), Error));
                continue;
            }
        };
        let (Some(Name), Some(Parent)) = (From.file_name().and_then(|Value| Value.to_str()), From.parent()) else {
            Problems.push(format!("{}: name cannot be renamed by pattern", From.display()));
            continue;
        };
        let Entry = Source { Path: From, Name, IsDirectory: Data.is_dir(), Modified: Data.modified().ok() };
        let NewName = Apply(Rule, &Entry, Items.len());
        if let Some(Reason) = NameProblem(&NewName) {
            Problems.push(format!("{} -> {}: {}", Name, NewName, Reason));
        }
        Items.push(Item { From: From.clone(), To: Parent.join(&NewName), OldName: Name.to_string(), NewName });
    }
    let mut Targets: HashMap<String, Vec<usize>> = HashMap::new();
    for (Index, Entry) in Items.iter().enumerate() {
        Targets.entry(PathKey(&Entry.To)).or_default().push(Index);
    }
    let mut Clashes: Vec<&Vec<usize>> = Targets.values().filter(|Indices| Indices.len() > 1).collect();
    Clashes.sort();
    for Indices in Clashes {
        let Names: Vec<&str> = Indices.iter().map(|Index| Items[*Index].OldName.as_str()).collect();
        Problems.push(format!("{} would all be named {}", Names.join(", "), Items[Indices[0]].NewName));
    }
    for Entry in &Items {
        if Entry.NewName != Entry.OldName && !Selected.contains(&PathKey(&Entry.To)) && fs::symlink_metadata(&Entry.To).is_ok() {
            Problems.push(format!("{} -> {}: an item with that name already exists", Entry.OldName, Entry.NewName));
        }
    }
    let Changed: Vec<Item> = Items.into_iter().filter(|Entry| Entry.NewName != Entry.OldName).collect();
    let Order = if Problems.is_empty() { OrderRenames(&Changed, &mut Problems) } else { Vec::new() };
    if !Problems.is_empty() {
        Host.Line(&format!("Rename aborted: {} problem(s) found. No items were renamed.", Problems.len()));
        Host.Line("");
        for Problem in &Problems {
            Host.Line(&format!("  {Problem}"));
        }
        return Outcome::Failed("Rename aborted; fix the pattern and try again.".to_string());
    }
    let Unchanged = Selected.len() - Changed.len();
    Host.Line(&format!("{} item(s) selected, {} to rename, {} unchanged", Selected.len(), Changed.len(), Unchanged));
    if Changed.is_empty() {
        return Outcome::Completed;
    }
    let Width = Changed.iter().map(|Entry| Entry.OldName.chars().count()).max().unwrap_or(0).min(60);
    Host.Line("");
    for Position in &Order {
        let Entry = &Changed[*Position];
        Host.Line(&format!("  {:<Width$}  ->  {}", Entry.OldName, Entry.NewName));
    }
    for Position in Order {
        Host.Rename(&Changed[Position].From, &Changed[Position].To);
    }
    Host.Progress(Selected.len() as u64, Selected.len() as u64);
    Outcome::Completed
}

impl Module for BatchRename {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        let Rule = match Request.Command.as_str() {
            "Rename" => match ParseTransform(&Request.Argument) {
                Ok(Rule) => Rule,
                Err(Message) => return Outcome::Failed(Message),
            },
            "Lowercase" => Transform::Lower,
            "Uppercase" => Transform::Upper,
            _ => return Outcome::Unsupported,
        };
        Plan(Host, Request, &Rule)
    }
}
