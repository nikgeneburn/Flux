use super::{AttributeDirectory, FlagRemoved, Listing};
use std::cmp::Ordering;
use std::sync::LazyLock;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal, LCMAP_LOWERCASE, LCMapStringEx, LOCALE_NAME_INVARIANT};

static FoldTable: LazyLock<Box<[u16]>> = LazyLock::new(BuildFoldTable);

fn BuildFoldTable() -> Box<[u16]> {
    let mut Table: Vec<u16> = (0..=0xFFFFu32).map(|Unit| Unit as u16).collect();
    let Ranges = [(0x80usize, 0xD800usize), (0xE000usize, 0x10000usize)];
    for (Start, End) in Ranges {
        let Source: Vec<u16> = (Start..End).map(|Unit| Unit as u16).collect();
        let mut Output = vec![0u16; Source.len()];
        let Written = unsafe { LCMapStringEx(LOCALE_NAME_INVARIANT, LCMAP_LOWERCASE, &Source, Some(&mut Output), None, None, LPARAM(0)) };
        if Written as usize == Source.len() {
            Table[Start..End].copy_from_slice(&Output);
        }
    }
    for Unit in b'A'..=b'Z' {
        Table[Unit as usize] = (Unit + 32) as u16;
    }
    Table.into_boxed_slice()
}

#[inline]
pub fn Fold(Unit: u16) -> u16 {
    if Unit < 0x80 {
        if (b'A' as u16..=b'Z' as u16).contains(&Unit) { Unit + 32 } else { Unit }
    } else {
        FoldTable[Unit as usize]
    }
}

pub fn FoldString(Text: &str) -> Vec<u16> {
    Text.encode_utf16().map(Fold).collect()
}

pub fn EqualIgnoreCase(Left: &[u16], Right: &[u16]) -> bool {
    if Left.len() != Right.len() {
        return false;
    }
    if Left.iter().zip(Right).all(|(&A, &B)| A == B || (A < 0x80 && B < 0x80 && Fold(A) == Fold(B))) {
        return true;
    }
    unsafe { CompareStringOrdinal(Left, Right, true) == CSTR_EQUAL }
}

pub fn StartsWithFolded(Text: &[u16], FoldedPrefix: &[u16]) -> bool {
    Text.len() >= FoldedPrefix.len() && Text.iter().zip(FoldedPrefix).all(|(&A, &B)| Fold(A) == B)
}

pub fn ContainsFolded(Text: &[u16], FoldedNeedle: &[u16]) -> bool {
    if FoldedNeedle.is_empty() {
        return true;
    }
    if Text.len() < FoldedNeedle.len() {
        return false;
    }
    let First = FoldedNeedle[0];
    let Last = Text.len() - FoldedNeedle.len();
    let mut Start = 0;
    while Start <= Last {
        if Fold(Text[Start]) == First && Text[Start + 1..Start + FoldedNeedle.len()].iter().zip(&FoldedNeedle[1..]).all(|(&A, &B)| Fold(A) == B) {
            return true;
        }
        Start += 1;
    }
    false
}

#[inline]
fn IsDigit(Unit: u16) -> bool {
    (b'0' as u16..=b'9' as u16).contains(&Unit)
}

pub fn CompareNatural(Left: &[u16], Right: &[u16]) -> Ordering {
    let (mut I, mut J) = (0usize, 0usize);
    while I < Left.len() && J < Right.len() {
        let (A, B) = (Left[I], Right[J]);
        if IsDigit(A) && IsDigit(B) {
            let StartA = I;
            let StartB = J;
            while I < Left.len() && Left[I] == b'0' as u16 {
                I += 1;
            }
            while J < Right.len() && Right[J] == b'0' as u16 {
                J += 1;
            }
            let SignificantA = I;
            let SignificantB = J;
            while I < Left.len() && IsDigit(Left[I]) {
                I += 1;
            }
            while J < Right.len() && IsDigit(Right[J]) {
                J += 1;
            }
            let LengthA = I - SignificantA;
            let LengthB = J - SignificantB;
            if LengthA != LengthB {
                return LengthA.cmp(&LengthB);
            }
            let Digits = Left[SignificantA..I].cmp(&Right[SignificantB..J]);
            if Digits != Ordering::Equal {
                return Digits;
            }
            let Zeros = (SignificantB - StartB).cmp(&(SignificantA - StartA));
            if Zeros != Ordering::Equal {
                return Zeros;
            }
            continue;
        }
        let (FoldedA, FoldedB) = (Fold(A), Fold(B));
        if FoldedA != FoldedB {
            return FoldedA.cmp(&FoldedB);
        }
        I += 1;
        J += 1;
    }
    (Left.len() - I).cmp(&(Right.len() - J)).then_with(|| Left.cmp(Right))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Created,
    Type,
    Folder,
}

impl SortKey {
    pub fn Label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Modified => "Modified",
            SortKey::Created => "Created",
            SortKey::Type => "Type",
            SortKey::Folder => "Folder",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SortSpec {
    pub Key: SortKey,
    pub Descending: bool,
    pub FoldersFirst: bool,
}

pub fn CompareEntries(Data: &Listing, Spec: SortSpec, Left: u32, Right: u32) -> Ordering {
    let A = &Data.Entries[Left as usize];
    let B = &Data.Entries[Right as usize];
    if Spec.FoldersFirst {
        let DirectoryA = A.Attributes & AttributeDirectory != 0;
        let DirectoryB = B.Attributes & AttributeDirectory != 0;
        if DirectoryA != DirectoryB {
            return if DirectoryA { Ordering::Less } else { Ordering::Greater };
        }
    }
    let Primary = match Spec.Key {
        SortKey::Name => Ordering::Equal,
        SortKey::Size => A.Size.cmp(&B.Size),
        SortKey::Modified => A.Modified.cmp(&B.Modified),
        SortKey::Created => A.Created.cmp(&B.Created),
        SortKey::Type => CompareNatural(Data.Extension(Left), Data.Extension(Right)),
        SortKey::Folder => CompareNatural(Data.Folder(Left), Data.Folder(Right)),
    };
    let Combined = Primary.then_with(|| CompareNatural(Data.Name(Left), Data.Name(Right))).then_with(|| Left.cmp(&Right));
    if Spec.Descending { Combined.reverse() } else { Combined }
}

pub fn SortOrder(Data: &Listing, Order: &mut [u32], Spec: SortSpec) {
    let Workers = std::thread::available_parallelism().map(|Count| Count.get()).unwrap_or(1).min(8);
    if Order.len() < 20_000 || Workers < 2 {
        Order.sort_unstable_by(|&Left, &Right| CompareEntries(Data, Spec, Left, Right));
        return;
    }
    let ChunkLength = Order.len().div_ceil(Workers);
    std::thread::scope(|Scope| {
        for Chunk in Order.chunks_mut(ChunkLength) {
            Scope.spawn(move || Chunk.sort_unstable_by(|&Left, &Right| CompareEntries(Data, Spec, Left, Right)));
        }
    });
    let mut Buffer = vec![0u32; Order.len()];
    let mut Width = ChunkLength;
    while Width < Order.len() {
        std::thread::scope(|Scope| {
            for (Source, Target) in Order.chunks(Width * 2).zip(Buffer.chunks_mut(Width * 2)) {
                Scope.spawn(move || Merge(Data, Spec, Source, Width.min(Source.len()), Target));
            }
        });
        Order.copy_from_slice(&Buffer);
        Width *= 2;
    }
}

fn Merge(Data: &Listing, Spec: SortSpec, Source: &[u32], Middle: usize, Target: &mut [u32]) {
    let (Left, Right) = Source.split_at(Middle);
    let (mut First, mut Second, mut Output) = (0usize, 0usize, 0usize);
    while First < Left.len() && Second < Right.len() {
        if CompareEntries(Data, Spec, Right[Second], Left[First]) == Ordering::Less {
            Target[Output] = Right[Second];
            Second += 1;
        } else {
            Target[Output] = Left[First];
            First += 1;
        }
        Output += 1;
    }
    let Remaining = Left.len() - First;
    Target[Output..Output + Remaining].copy_from_slice(&Left[First..]);
    Target[Output + Remaining..].copy_from_slice(&Right[Second..]);
}

pub fn InsertSorted(Data: &Listing, Order: &mut Vec<u32>, Spec: SortSpec, Index: u32) -> usize {
    let Position = Order.partition_point(|&Existing| CompareEntries(Data, Spec, Existing, Index) == Ordering::Less);
    Order.insert(Position, Index);
    Position
}

pub fn LiveIndices(Data: &Listing) -> Vec<u32> {
    (0..Data.Entries.len() as u32).filter(|&Index| Data.Entries[Index as usize].Flags & FlagRemoved == 0).collect()
}
