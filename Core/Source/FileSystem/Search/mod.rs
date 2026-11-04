use super::Enumerate::ForEach;
use super::Sorting::{ContainsFolded, Fold, FoldString};
use super::{AttributeDirectory, AttributeHidden, AttributeReparsePoint, AttributeSystem, Listing, RawEntry};
use crate::Services::Workers::CancelToken;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
enum Term {
    Contains(Vec<u16>),
    Wildcard(Vec<u16>),
    Fuzzy(Vec<u16>),
    Exclude(Vec<u16>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KindFilter {
    Any,
    Files,
    Folders,
}

#[derive(Clone, Debug)]
pub struct Query {
    Terms: Vec<Term>,
    Extensions: Vec<Vec<u16>>,
    pub Kind: KindFilter,
    MinimumSize: Option<u64>,
    MaximumSize: Option<u64>,
    NewerThan: Option<i64>,
    OlderThan: Option<i64>,
}

pub fn NowFileTime() -> i64 {
    let Since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (Since.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}

fn ParseSize(Text: &str) -> Option<u64> {
    let Lower = Text.trim().to_ascii_lowercase();
    let Split = Lower.find(|Character: char| !(Character.is_ascii_digit() || Character == '.')).unwrap_or(Lower.len());
    let Number: f64 = Lower[..Split].parse().ok()?;
    let Multiplier = match Lower[Split..].trim() {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((Number * Multiplier) as u64)
}

fn ParseAge(Text: &str) -> Option<i64> {
    let Lower = Text.trim().to_ascii_lowercase();
    let Split = Lower.find(|Character: char| !Character.is_ascii_digit()).unwrap_or(Lower.len());
    let Number: i64 = Lower[..Split].parse().ok()?;
    let Seconds = match &Lower[Split..] {
        "m" | "min" => 60,
        "h" => 3600,
        "" | "d" => 86400,
        "w" => 7 * 86400,
        "mo" => 30 * 86400,
        "y" => 365 * 86400,
        _ => return None,
    };
    Some(Number * Seconds * 10_000_000)
}

impl Query {
    pub fn Parse(Text: &str) -> Query {
        let mut Output = Query {
            Terms: Vec::new(),
            Extensions: Vec::new(),
            Kind: KindFilter::Any,
            MinimumSize: None,
            MaximumSize: None,
            NewerThan: None,
            OlderThan: None,
        };
        let Now = NowFileTime();
        for Token in Text.split_whitespace() {
            let Lower = Token.to_ascii_lowercase();
            if let Some(Value) = Lower.strip_prefix("ext:") {
                Output.Extensions.extend(Value.split(',').filter(|Part| !Part.is_empty()).map(|Part| FoldString(Part.trim_start_matches('.'))));
            } else if let Some(Value) = Lower.strip_prefix("type:") {
                Output.Kind = match Value {
                    "file" | "files" => KindFilter::Files,
                    "folder" | "folders" | "dir" | "dirs" => KindFilter::Folders,
                    _ => KindFilter::Any,
                };
            } else if let Some(Value) = Lower.strip_prefix("size:>") {
                Output.MinimumSize = ParseSize(Value);
            } else if let Some(Value) = Lower.strip_prefix("size:<") {
                Output.MaximumSize = ParseSize(Value);
            } else if let Some(Value) = Lower.strip_prefix("modified:<") {
                Output.NewerThan = ParseAge(Value).map(|Age| Now - Age);
            } else if let Some(Value) = Lower.strip_prefix("modified:>") {
                Output.OlderThan = ParseAge(Value).map(|Age| Now - Age);
            } else if let Some(Value) = Token.strip_prefix('~') {
                if !Value.is_empty() {
                    Output.Terms.push(Term::Fuzzy(FoldString(Value)));
                }
            } else if let Some(Value) = Token.strip_prefix('-') {
                if !Value.is_empty() {
                    Output.Terms.push(Term::Exclude(FoldString(Value)));
                }
            } else if Token.contains('*') || Token.contains('?') {
                Output.Terms.push(Term::Wildcard(FoldString(Token)));
            } else {
                Output.Terms.push(Term::Contains(FoldString(Token.trim_matches('"'))));
            }
        }
        Output
    }

    pub fn Matches(&self, Name: &[u16], Extension: &[u16], Attributes: u32, Size: u64, Modified: i64) -> bool {
        let IsDirectory = Attributes & AttributeDirectory != 0;
        match self.Kind {
            KindFilter::Files if IsDirectory => return false,
            KindFilter::Folders if !IsDirectory => return false,
            _ => {}
        }
        if !self.Extensions.is_empty() && (IsDirectory || !self.Extensions.iter().any(|Wanted| Wanted.len() == Extension.len() && Extension.iter().zip(Wanted).all(|(&A, &B)| Fold(A) == B))) {
            return false;
        }
        if self.MinimumSize.is_some_and(|Minimum| IsDirectory || Size < Minimum) || self.MaximumSize.is_some_and(|Maximum| IsDirectory || Size > Maximum) {
            return false;
        }
        if self.NewerThan.is_some_and(|Limit| Modified < Limit) || self.OlderThan.is_some_and(|Limit| Modified > Limit) {
            return false;
        }
        self.Terms.iter().all(|Item| match Item {
            Term::Contains(Needle) => ContainsFolded(Name, Needle),
            Term::Exclude(Needle) => !ContainsFolded(Name, Needle),
            Term::Wildcard(Pattern) => WildcardMatch(Pattern, Name),
            Term::Fuzzy(Pattern) => FuzzyScore(Pattern, Name).is_some(),
        })
    }
}

pub fn WildcardMatch(Pattern: &[u16], Text: &[u16]) -> bool {
    let (mut P, mut T) = (0usize, 0usize);
    let (mut Star, mut Mark) = (usize::MAX, 0usize);
    while T < Text.len() {
        if P < Pattern.len() && (Pattern[P] == b'?' as u16 || Pattern[P] == Fold(Text[T])) {
            P += 1;
            T += 1;
        } else if P < Pattern.len() && Pattern[P] == b'*' as u16 {
            Star = P;
            Mark = T;
            P += 1;
        } else if Star != usize::MAX {
            P = Star + 1;
            Mark += 1;
            T = Mark;
        } else {
            return false;
        }
    }
    while P < Pattern.len() && Pattern[P] == b'*' as u16 {
        P += 1;
    }
    P == Pattern.len()
}

fn IsBoundary(Text: &[u16], Index: usize) -> bool {
    if Index == 0 {
        return true;
    }
    let Previous = Text[Index - 1];
    let Current = Text[Index];
    let Separator = [b' ', b'_', b'-', b'.', b'\\', b'/', b'(', b'['].iter().any(|&Unit| Unit as u16 == Previous);
    let CaseChange = (b'a' as u16..=b'z' as u16).contains(&Previous) && (b'A' as u16..=b'Z' as u16).contains(&Current);
    Separator || CaseChange
}

pub fn FuzzyScore(Pattern: &[u16], Text: &[u16]) -> Option<i32> {
    if Pattern.is_empty() {
        return Some(0);
    }
    let mut Score = 0i32;
    let mut PatternIndex = 0usize;
    let mut Previous: Option<usize> = None;
    let mut FirstMatch: Option<usize> = None;
    for (Index, &Unit) in Text.iter().enumerate() {
        if PatternIndex == Pattern.len() {
            break;
        }
        if Fold(Unit) != Pattern[PatternIndex] {
            continue;
        }
        let mut Gain = 10;
        if IsBoundary(Text, Index) {
            Gain += 18;
        }
        match Previous {
            Some(Last) if Last + 1 == Index => Gain += 15,
            Some(Last) => Gain -= ((Index - Last - 1) as i32).min(12),
            None => Gain -= (Index as i32).min(15),
        }
        if Unit == Pattern[PatternIndex] && Unit != Fold(Unit) {
            Gain += 1;
        }
        FirstMatch.get_or_insert(Index);
        Score += Gain;
        Previous = Some(Index);
        PatternIndex += 1;
    }
    if PatternIndex < Pattern.len() {
        return None;
    }
    Score -= (Text.len() as i32 - Pattern.len() as i32).max(0) / 4;
    if FirstMatch == Some(0) {
        Score += 10;
    }
    Some(Score)
}

pub struct SearchStatistics {
    pub Directories: u64,
    pub Scanned: u64,
    pub Matches: u64,
    pub Errors: u64,
    pub Elapsed: Duration,
    pub Truncated: bool,
}

pub const MaximumResults: usize = 2_000_000;

pub fn Run(
    Root: &Path,
    Search: &Query,
    IncludeHidden: bool,
    Cancel: &CancelToken,
    Emit: &mut dyn FnMut(Listing),
) -> SearchStatistics {
    let Started = Instant::now();
    let mut Statistics = SearchStatistics { Directories: 0, Scanned: 0, Matches: 0, Errors: 0, Elapsed: Duration::ZERO, Truncated: false };
    let mut Pending: Vec<(PathBuf, Vec<u16>)> = vec![(Root.to_path_buf(), Vec::new())];
    let mut Batch = Listing::New(Some(Root.to_path_buf()));
    let mut Buffer = Vec::new();
    let mut LastEmit = Instant::now();
    let mut Children: Vec<(Vec<u16>, u32)> = Vec::new();
    while let Some((Directory, Relative)) = Pending.pop() {
        if Cancel.IsCancelled() {
            break;
        }
        Statistics.Directories += 1;
        Children.clear();
        let Result = ForEach(&Directory, &mut Buffer, &mut |Raw: &RawEntry| {
            Statistics.Scanned += 1;
            if !IncludeHidden && Raw.Attributes & (AttributeHidden | AttributeSystem) != 0 {
                return true;
            }
            let IsDirectory = Raw.Attributes & AttributeDirectory != 0;
            if IsDirectory && Raw.Attributes & AttributeReparsePoint == 0 {
                Children.push((Raw.Name.to_vec(), Raw.Attributes));
            }
            let Extension = if IsDirectory {
                &Raw.Name[Raw.Name.len()..]
            } else {
                match Raw.Name.iter().rposition(|&Unit| Unit == b'.' as u16) {
                    Some(Position) => &Raw.Name[Position + 1..],
                    None => &Raw.Name[Raw.Name.len()..],
                }
            };
            if Search.Matches(Raw.Name, Extension, Raw.Attributes, Raw.Size, Raw.Modified) {
                Batch.Push(&Relative, Raw);
                Statistics.Matches += 1;
            }
            true
        });
        if Result.is_err() {
            Statistics.Errors += 1;
        }
        for (Name, _) in Children.iter().rev() {
            let mut ChildRelative = Relative.clone();
            if !ChildRelative.is_empty() {
                ChildRelative.push(b'\\' as u16);
            }
            ChildRelative.extend_from_slice(Name);
            Pending.push((Directory.join(OsString::from_wide(Name)), ChildRelative));
        }
        if Statistics.Matches as usize >= MaximumResults {
            Statistics.Truncated = true;
            break;
        }
        if !Batch.Entries.is_empty() && (LastEmit.elapsed() > Duration::from_millis(80) || Batch.Entries.len() >= 4096) {
            Emit(std::mem::replace(&mut Batch, Listing::New(Some(Root.to_path_buf()))));
            LastEmit = Instant::now();
        }
    }
    if !Batch.Entries.is_empty() {
        Emit(Batch);
    }
    Statistics.Elapsed = Started.elapsed();
    Statistics
}
