use crate::Application::{App, Deferred, Event};
use crate::FileSystem::Enumerate;
use crate::FileSystem::{AttributeDirectory, AttributeReparsePoint, Location, ShellPath};
use crate::Interface::FileView::Columns::{AttributeText, FormatFileTime, TypeName, WithCommas};
use crate::Interface::{Align, Fit, Hit, Rect, Style};
use crate::Services::Workers::{CancelToken, NewIdentifier};
use FluxSdk::Utilities::FormatSize;
use std::io::Read;
use std::path::{Path, PathBuf};

pub enum PreviewContent {
    Text { Body: Vec<u16>, Truncated: bool, Encoding: &'static str },
    Binary,
    Error(String),
}

pub struct FolderSize {
    pub Bytes: u64,
    pub Files: u64,
    pub Folders: u64,
    pub Done: bool,
}

#[derive(Default)]
pub struct InspectorState {
    pub Token: u64,
    pub Target: Option<PathBuf>,
    pub Modified: i64,
    pub Content: Option<PreviewContent>,
    pub Scroll: f32,
    pub Size: Option<FolderSize>,
    pub SizeToken: u64,
    pub SizeCancel: Option<CancelToken>,
    pub ModuleText: Option<String>,
    pub LinkTarget: Option<String>,
}

const ImageExtensions: [&str; 22] = [
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "ico", "avif", "jxr", "dds", "raw", "cr2", "nef", "arw", "mp4", "mov", "mkv", "pdf",
];
const TextLimit: usize = 96 * 1024;

impl InspectorState {
    pub fn Accept(&mut self, Token: u64, Content: PreviewContent) {
        if Token == self.Token {
            self.Content = Some(Content);
        }
    }

    pub fn AcceptModuleText(&mut self, Token: u64, Text: Option<String>) {
        if Token == self.Token {
            self.ModuleText = Text;
        }
    }

    pub fn AcceptSize(&mut self, Token: u64, Bytes: u64, Files: u64, Folders: u64, Done: bool) {
        if Token == self.SizeToken {
            self.Size = Some(FolderSize { Bytes, Files, Folders, Done });
        }
    }

    pub fn ScrollBy(&mut self, Delta: f32) {
        self.Scroll = (self.Scroll + Delta).max(0.0);
    }
}

fn Decode(Bytes: &[u8], Truncated: bool) -> PreviewContent {
    if Bytes.len() >= 2 && (Bytes[0] == 0xFF && Bytes[1] == 0xFE) {
        let Units: Vec<u16> = Bytes[2..].chunks_exact(2).map(|Pair| u16::from_le_bytes([Pair[0], Pair[1]])).collect();
        return PreviewContent::Text { Body: Units, Truncated, Encoding: "UTF-16 LE" };
    }
    if Bytes.len() >= 2 && (Bytes[0] == 0xFE && Bytes[1] == 0xFF) {
        let Units: Vec<u16> = Bytes[2..].chunks_exact(2).map(|Pair| u16::from_be_bytes([Pair[0], Pair[1]])).collect();
        return PreviewContent::Text { Body: Units, Truncated, Encoding: "UTF-16 BE" };
    }
    let Body = if Bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { &Bytes[3..] } else { Bytes };
    let Sample = &Body[..Body.len().min(8192)];
    let Controls = Sample.iter().filter(|&&Byte| Byte == 0 || (Byte < 9) || (Byte > 13 && Byte < 32 && Byte != 27)).count();
    if Sample.contains(&0) || Controls * 50 > Sample.len().max(1) {
        return PreviewContent::Binary;
    }
    let (Text, Encoding) = match std::str::from_utf8(Body) {
        Ok(Text) => (Text.to_string(), "UTF-8"),
        Err(Error) if Truncated && Error.valid_up_to() + 4 >= Body.len() => (String::from_utf8_lossy(&Body[..Error.valid_up_to()]).into_owned(), "UTF-8"),
        Err(_) => (Body.iter().map(|&Byte| Byte as char).collect(), "ANSI"),
    };
    PreviewContent::Text { Body: Text.encode_utf16().collect(), Truncated, Encoding }
}

fn ReadPreview(Item: &Path) -> PreviewContent {
    let mut File = match std::fs::File::open(Item) {
        Ok(File) => File,
        Err(Error) => return PreviewContent::Error(Error.to_string()),
    };
    let mut Buffer = vec![0u8; TextLimit + 1];
    let mut Filled = 0;
    while Filled < Buffer.len() {
        match File.read(&mut Buffer[Filled..]) {
            Ok(0) => break,
            Ok(Count) => Filled += Count,
            Err(Error) => return PreviewContent::Error(Error.to_string()),
        }
    }
    let Truncated = Filled > TextLimit;
    Decode(&Buffer[..Filled.min(TextLimit)], Truncated)
}

fn LinkTarget(Item: &Path, Attributes: u32) -> Option<String> {
    if Attributes & AttributeReparsePoint != 0 {
        return std::fs::read_link(Item).ok().map(|Target| Target.display().to_string());
    }
    if Item.extension().is_some_and(|Extension| Extension.eq_ignore_ascii_case("lnk")) {
        return crate::Platform::Shell::ShortcutTarget(Item).map(|Target| Target.display().to_string());
    }
    None
}

impl App {
    pub fn RefreshInspector(&mut self) {
        if !self.Settings.ShowInspector {
            return;
        }
        let Pane = self.ActivePane;
        let Tab = self.Tab(Pane);
        let Target = Tab.Cursor.filter(|_| Tab.SelectedCount <= 1).map(|Index| (Tab.Listing.FullPath(Index), Tab.Listing.Entries[Index as usize]));
        let Same = match (&Target, &self.Inspector.Target) {
            (Some((Path, Item)), Some(Current)) => Path == Current && Item.Modified == self.Inspector.Modified,
            (None, None) => true,
            _ => false,
        };
        if Same {
            return;
        }
        if let Some(Cancel) = self.Inspector.SizeCancel.take() {
            Cancel.Cancel();
        }
        self.Inspector.Size = None;
        self.Inspector.Scroll = 0.0;
        self.Inspector.Content = None;
        self.Inspector.ModuleText = None;
        self.Inspector.Token = NewIdentifier();
        let Token = self.Inspector.Token;
        let Some((Item, Entry)) = Target else {
            self.Inspector.Target = None;
            self.Inspector.LinkTarget = None;
            return;
        };
        self.Inspector.Target = Some(Item.clone());
        self.Inspector.Modified = Entry.Modified;
        self.Inspector.LinkTarget = LinkTarget(&Item, Entry.Attributes);
        if Entry.Attributes & AttributeDirectory != 0 || matches!(self.Tab(Pane).Location, Location::Computer) {
            return;
        }
        let Extension = Item.extension().map(|Extension| Extension.to_string_lossy().to_lowercase()).unwrap_or_default();
        if let Some(Module) = self.Modules.PreviewProvider(&Extension) {
            if let Err(Message) = self.Modules.Preview(Module, Item.clone(), Token, self.Sink.clone()) {
                self.Inspector.ModuleText = Some(format!("Preview module unavailable: {Message}"));
            }
            return;
        }
        if ImageExtensions.contains(&Extension.as_str()) || Entry.Size == 0 {
            return;
        }
        let Sink = self.Sink.clone();
        self.Background.Submit(move || {
            let Content = ReadPreview(&Item);
            Sink.Send(Event::Preview { Token, Content });
        });
    }

    pub fn StartFolderSize(&mut self) {
        let Pane = self.ActivePane;
        let Tab = self.Tab(Pane);
        let Directory = match Tab.Cursor.filter(|&Index| Tab.Listing.IsDirectory(Index)) {
            Some(Index) => Tab.Listing.FullPath(Index),
            None => match self.CurrentDirectory(Pane) {
                Some(Directory) => Directory,
                None => return,
            },
        };
        if let Some(Cancel) = self.Inspector.SizeCancel.take() {
            Cancel.Cancel();
        }
        let Cancel = CancelToken::New();
        self.Inspector.SizeCancel = Some(Cancel.clone());
        self.Inspector.SizeToken = NewIdentifier();
        self.Inspector.Size = Some(FolderSize { Bytes: 0, Files: 0, Folders: 0, Done: false });
        let Token = self.Inspector.SizeToken;
        let Sink = self.Sink.clone();
        let _ = crate::Services::Workers::Spawn("FolderSize", move || {
            let ProgressSink = Sink.clone();
            let Result = Enumerate::DirectorySize(&Directory, &Cancel, &mut |Bytes, Files| {
                ProgressSink.Send(Event::FolderSize { Token, Bytes, Files, Folders: 0, Done: false });
            });
            if let Some((Bytes, Files, Folders)) = Result {
                Sink.Send(Event::FolderSize { Token, Bytes, Files, Folders, Done: true });
            }
        });
    }
}

pub fn Action(App: &mut App, Code: u8) {
    let Target = App.Inspector.Target.clone();
    match Code {
        0 => App.StartFolderSize(),
        1 => {
            if let Some(Item) = Target {
                App.Deferred.push(Deferred::Open(Item));
            }
        }
        2 => {
            if let Some(Item) = Target {
                crate::Platform::Clipboard::SetText(App.Window, &Item.display().to_string());
                App.Inform("Path copied");
            }
        }
        3 => {
            if let Some(Item) = Target {
                App.Deferred.push(Deferred::Properties(vec![Item]));
            }
        }
        _ => {}
    }
}

fn Row(App: &mut App, Area: &mut Rect, Label: &str, Value: &str) {
    if Value.is_empty() {
        return;
    }
    let Colors = App.Theme.Colors.clone();
    let Line = Area.TakeTop(20.0);
    App.Draw.Label(Label, Style::Small, Rect { Right: Line.Left + 86.0, ..Line }, Colors.TextFaint, Align::Left);
    App.Draw.Label(Value, Style::Small, Rect { Left: Line.Left + 90.0, ..Line }, Colors.Text, Align::Left);
}

fn ActionButton(App: &mut App, Area: Rect, Label: &str, Code: u8) {
    let Colors = App.Theme.Colors.clone();
    let Target = Hit::InspectorAction(Code);
    App.Draw.FillRounded(Area, App.Theme.Sizes.Radius, if App.Pointer.Hover == Target { Colors.Hover.Mix(Colors.Input, 0.4) } else { Colors.Input });
    App.Draw.StrokeRounded(Area, App.Theme.Sizes.Radius, Colors.Border, 1.0);
    App.Draw.Label(Label, Style::Small, Area, Colors.Text, Align::Center);
    App.Hits.Push(Area, Target);
}

pub fn DrawInspector(App: &mut App, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    App.Draw.Fill(Area, Colors.Sidebar);
    App.Hits.Push(Area, Hit::Inspector);
    App.RefreshInspector();
    let Pane = App.ActivePane;
    let mut Remaining = Area.Inset(14.0, 12.0);
    App.Draw.PushClip(Area);
    if let Some(Report) = App.Tab(Pane).Report.as_ref().map(|Report| (Report.Title.clone(), Report.Text.clone())) {
        let (Title, Text) = Report;
        App.Draw.Label(&Title, Style::Strong, Remaining.TakeTop(22.0), Colors.Text, Align::Left);
        Remaining.TakeTop(6.0);
        let Wide: Vec<u16> = Text.encode_utf16().collect();
        let Height = App.Draw.WrappedHeight(&Wide, Remaining.Width(), true);
        App.Inspector.Scroll = App.Inspector.Scroll.min((Height - Remaining.Height() + 20.0).max(0.0));
        let Scroll = App.Inspector.Scroll;
        App.Draw.PushClip(Remaining);
        App.Draw.Wrapped(&Wide, Rect { Top: Remaining.Top - Scroll, Bottom: Remaining.Top - Scroll + Height + 4.0, ..Remaining }, Colors.TextMuted, true);
        App.Draw.PopClip();
        App.Draw.PopClip();
        return;
    }
    let Tab = App.Tab(Pane);
    if Tab.SelectedCount > 1 {
        let Count = Tab.SelectedCount;
        let Bytes = Tab.SelectedBytes();
        let Folders = Tab.SelectedIndices().iter().filter(|&&Index| Tab.Listing.IsDirectory(Index)).count();
        App.Draw.Label(&format!("{} items selected", WithCommas(Count as u64)), Style::Heading, Remaining.TakeTop(30.0), Colors.Text, Align::Left);
        Row(App, &mut Remaining, "Files", &WithCommas((Count - Folders) as u64));
        Row(App, &mut Remaining, "Folders", &WithCommas(Folders as u64));
        Row(App, &mut Remaining, "File size", &FormatSize(Bytes));
        App.Draw.PopClip();
        return;
    }
    let Some(Index) = Tab.Cursor.filter(|Index| Tab.Order.contains(Index)) else {
        let Message = match &Tab.Location {
            Location::Directory(Item) => Item.display().to_string(),
            Other => Other.Title(),
        };
        App.Draw.Label("No selection", Style::Strong, Remaining.TakeTop(22.0), Colors.TextMuted, Align::Left);
        App.Draw.Label(&Message, Style::Small, Remaining.TakeTop(18.0), Colors.TextFaint, Align::Left);
        App.Draw.PopClip();
        return;
    };
    let Item = Tab.Listing.Entries[Index as usize];
    let Name = Tab.Listing.NameString(Index);
    let Full = Tab.Listing.FullPath(Index);
    let Kind = if !Tab.Listing.Drives.is_empty() && Tab.Listing.Root.is_none() { "Drive".to_string() } else { TypeName(&Tab.Listing, Index).to_string() };
    let IsDirectory = Item.Attributes & AttributeDirectory != 0;
    let Scale = App.Scale();
    let PreviewHeight = (Remaining.Width() * 0.75).min(260.0);
    let PreviewArea = Remaining.TakeTop(PreviewHeight);
    let Extension = Full.extension().map(|Extension| Extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    let WantsThumbnail = ImageExtensions.contains(&Extension.as_str());
    let Pixel = (PreviewArea.Height().max(PreviewArea.Width()) * Scale) as u32;
    let Shown = if WantsThumbnail { App.Media.Thumbnail(&Full, Pixel, true) } else { None };
    match Shown {
        Some((Image, Width, Height)) => {
            let Target = Fit(Width as f32 / Scale, Height as f32 / Scale, PreviewArea);
            App.Draw.Bitmap(&Image, Target, 1.0);
        }
        None => {
            let Listing = &mut App.Panes[Pane].CurrentMut().Listing;
            let IconIndex = App.Media.IconIndex(Listing, Index);
            if let Some(Image) = App.Media.IconBitmap(&App.Draw, IconIndex, (96.0 * Scale) as u32) {
                let Size = 72.0;
                App.Draw.Bitmap(&Image, Rect::New(PreviewArea.Left + (PreviewArea.Width() - Size) / 2.0, PreviewArea.Top + (PreviewArea.Height() - Size) / 2.0, Size, Size), 1.0);
            }
        }
    }
    Remaining.TakeTop(10.0);
    App.Draw.Label(&Name, Style::Strong, Remaining.TakeTop(22.0), Colors.Text, Align::Left);
    App.Draw.Label(&Kind, Style::Small, Remaining.TakeTop(18.0), Colors.TextMuted, Align::Left);
    Remaining.TakeTop(8.0);
    if !IsDirectory {
        let Size = format!("{} ({} bytes)", FormatSize(Item.Size), WithCommas(Item.Size));
        Row(App, &mut Remaining, "Size", &Size);
    } else if let Some(Total) = &App.Inspector.Size {
        let Folders = if Total.Done { format!(" · {} folders", WithCommas(Total.Folders)) } else { String::new() };
        let Text = format!("{}{} · {} files{}", FormatSize(Total.Bytes), if Total.Done { "" } else { "…" }, WithCommas(Total.Files), Folders);
        Row(App, &mut Remaining, "Size", &Text);
    }
    Row(App, &mut Remaining, "Modified", &FormatFileTime(Item.Modified));
    Row(App, &mut Remaining, "Created", &FormatFileTime(Item.Created));
    let mut Attributes = String::new();
    AttributeText(&mut Attributes, Item.Attributes);
    Row(App, &mut Remaining, "Attributes", &Attributes);
    if let Some(Link) = App.Inspector.LinkTarget.clone() {
        Row(App, &mut Remaining, "Target", &Link);
    }
    let Location = String::from_utf16_lossy(&ShellPath(&Full));
    let Location = Location.trim_end_matches('\0').to_string();
    Row(App, &mut Remaining, "Path", &Location);
    Remaining.TakeTop(8.0);
    let mut Buttons = Remaining.TakeTop(26.0);
    let Width = (Buttons.Width() - 12.0) / 3.0;
    ActionButton(App, Buttons.TakeLeft(Width), if IsDirectory { "Calculate size" } else { "Open" }, if IsDirectory { 0 } else { 1 });
    Buttons.TakeLeft(6.0);
    ActionButton(App, Buttons.TakeLeft(Width), "Copy path", 2);
    Buttons.TakeLeft(6.0);
    ActionButton(App, Buttons, "Properties", 3);
    Remaining.TakeTop(12.0);
    let Body = match (&App.Inspector.ModuleText, &App.Inspector.Content) {
        (Some(Text), _) => Some((Text.encode_utf16().collect::<Vec<u16>>(), "Module preview".to_string())),
        (None, Some(PreviewContent::Text { Body, Truncated, Encoding })) => {
            let Shown = &Body[..Body.len().min(24 * 1024)];
            Some((Shown.to_vec(), format!("{Encoding}{}", if *Truncated || Shown.len() < Body.len() { " · first part" } else { "" })))
        }
        (None, Some(PreviewContent::Binary)) => {
            App.Draw.Label("Binary file", Style::Small, Remaining.TakeTop(18.0), Colors.TextFaint, Align::Left);
            None
        }
        (None, Some(PreviewContent::Error(Message))) => {
            App.Draw.Label(&format!("Preview unavailable: {Message}"), Style::Small, Remaining.TakeTop(18.0), Colors.Danger, Align::Left);
            None
        }
        _ => None,
    };
    if let Some((Text, Caption)) = Body {
        App.Draw.HorizontalLine(Remaining.Left, Remaining.Right, Remaining.Top, Colors.Border);
        Remaining.TakeTop(6.0);
        App.Draw.Label(&Caption, Style::Small, Remaining.TakeTop(18.0), Colors.TextFaint, Align::Left);
        let Height = App.Draw.WrappedHeight(&Text, Remaining.Width(), true);
        App.Inspector.Scroll = App.Inspector.Scroll.min((Height - Remaining.Height() + 20.0).max(0.0));
        let Scroll = App.Inspector.Scroll;
        App.Draw.PushClip(Remaining);
        App.Draw.Wrapped(&Text, Rect { Top: Remaining.Top - Scroll, Bottom: Remaining.Top - Scroll + Height + 4.0, ..Remaining }, Colors.Text.Mix(Colors.TextMuted, 0.3), true);
        App.Draw.PopClip();
    }
    App.Draw.PopClip();
}
