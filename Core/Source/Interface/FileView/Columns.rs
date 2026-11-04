use crate::FileSystem::{
    AttributeArchive, AttributeCompressed, AttributeDirectory, AttributeEncrypted, AttributeHidden, AttributeOffline, AttributeReadOnly,
    AttributeReparsePoint, AttributeSystem, Listing,
};
use crate::Services::Config::{ColumnKind, ColumnSetting};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::core::PCWSTR;

thread_local! {
    static TypeCache: RefCell<HashMap<(Vec<u16>, bool), Rc<str>>> = RefCell::new(HashMap::new());
}

pub fn TypeName(Data: &Listing, Index: u32) -> Rc<str> {
    let IsDirectory = Data.IsDirectory(Index);
    let Key: Vec<u16> = Data.Extension(Index).iter().map(|&Unit| crate::FileSystem::Sorting::Fold(Unit)).collect();
    TypeCache.with(|Cache| {
        let mut Cache = Cache.borrow_mut();
        if let Some(Found) = Cache.get(&(Key.clone(), IsDirectory)) {
            return Found.clone();
        }
        let Name: Rc<str> = Rc::from(crate::Platform::Shell::TypeName(&Key, IsDirectory).as_str());
        if Cache.len() > 4096 {
            Cache.clear();
        }
        Cache.insert((Key, IsDirectory), Name.clone());
        Name
    })
}

pub fn FormatFileTime(Value: i64) -> String {
    let mut Output = String::new();
    FormatFileTimeInto(&mut Output, Value);
    Output
}

pub fn FormatFileTimeInto(Output: &mut String, Value: i64) {
    if Value <= 0 {
        return;
    }
    let Time = FILETIME { dwLowDateTime: Value as u32, dwHighDateTime: (Value >> 32) as u32 };
    let mut Universal = SYSTEMTIME::default();
    let mut Local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&Time, &mut Universal).is_err() || SystemTimeToTzSpecificLocalTime(None, &Universal, &mut Local).is_err() {
            return;
        }
    }
    let mut Buffer = [0u16; 64];
    let DateLength = unsafe { GetDateFormatEx(PCWSTR::null(), DATE_SHORTDATE, Some(&Local), PCWSTR::null(), Some(&mut Buffer), PCWSTR::null()) } as usize;
    if DateLength > 1 {
        Output.push_str(&String::from_utf16_lossy(&Buffer[..DateLength - 1]));
    }
    let TimeLength = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&Local), PCWSTR::null(), Some(&mut Buffer)) } as usize;
    if TimeLength > 1 {
        Output.push(' ');
        Output.push_str(&String::from_utf16_lossy(&Buffer[..TimeLength - 1]));
    }
}

pub fn AttributeText(Output: &mut String, Attributes: u32) {
    let Flags = [
        (AttributeReadOnly, 'R'),
        (AttributeHidden, 'H'),
        (AttributeSystem, 'S'),
        (AttributeArchive, 'A'),
        (AttributeCompressed, 'C'),
        (AttributeEncrypted, 'E'),
        (AttributeOffline, 'O'),
        (AttributeReparsePoint, 'L'),
    ];
    for (Flag, Letter) in Flags {
        if Attributes & Flag != 0 {
            Output.push(Letter);
        }
    }
}

pub fn CellText(Output: &mut String, Data: &Listing, Index: u32, Kind: ColumnKind) {
    Output.clear();
    let Item = &Data.Entries[Index as usize];
    let IsDrive = !Data.Drives.is_empty() && Data.Root.is_none();
    match Kind {
        ColumnKind::Name => {
            if IsDrive {
                if let Some(Drive) = Data.Drives.get(Index as usize) {
                    Output.push_str(&Drive.Label);
                    return;
                }
            }
            Output.push_str(&String::from_utf16_lossy(Data.Name(Index)));
        }
        ColumnKind::Type => {
            if IsDrive {
                if let Some(Drive) = Data.Drives.get(Index as usize) {
                    Output.push_str(if Drive.FileSystem.is_empty() { "Drive" } else { &Drive.FileSystem });
                    if Drive.Total > 0 {
                        Output.push_str(" · ");
                        FluxSdk::Utilities::FormatSizeInto(Output, Drive.Free);
                        Output.push_str(" free of ");
                        FluxSdk::Utilities::FormatSizeInto(Output, Drive.Total);
                    }
                    return;
                }
            }
            Output.push_str(&TypeName(Data, Index));
        }
        ColumnKind::Size => {
            if IsDrive && Data.Drives.get(Index as usize).is_some() {
                return;
            }
            if Item.Attributes & AttributeDirectory == 0 {
                FluxSdk::Utilities::FormatSizeInto(Output, Item.Size);
            } else if Item.Flags & crate::FileSystem::FlagSizeKnown != 0 {
                let Partial = Item.Flags & crate::FileSystem::FlagSizePartial != 0;
                if !Partial || Item.Size > 0 {
                    FluxSdk::Utilities::FormatSizeInto(Output, Item.Size);
                }
                if Partial {
                    Output.push('…');
                }
            }
        }
        ColumnKind::Modified => FormatFileTimeInto(Output, Item.Modified),
        ColumnKind::Created => FormatFileTimeInto(Output, Item.Created),
        ColumnKind::Attributes => AttributeText(Output, Item.Attributes),
        ColumnKind::Folder => {
            let Folder = Data.Folder(Index);
            if Folder.is_empty() {
                if Data.Root.is_some() {
                    Output.push('.');
                }
            } else {
                Output.push_str(&String::from_utf16_lossy(Folder));
            }
        }
    }
}

pub fn Visible(Columns: &[ColumnSetting], ShowFolder: bool) -> Vec<ColumnSetting> {
    Columns.iter().copied().filter(|Column| Column.Visible && (Column.Kind != ColumnKind::Folder || ShowFolder)).collect()
}

pub fn WithCommas(Value: u64) -> String {
    let Digits = Value.to_string();
    let mut Output = String::with_capacity(Digits.len() + Digits.len() / 3);
    for (Index, Character) in Digits.chars().enumerate() {
        if Index > 0 && (Digits.len() - Index) % 3 == 0 {
            Output.push(',');
        }
        Output.push(Character);
    }
    Output
}

pub fn CountText(Count: usize, Singular: &str) -> String {
    let mut Output = WithCommas(Count as u64);
    let _ = write!(Output, " {}{}", Singular, if Count == 1 { "" } else { "s" });
    Output
}
