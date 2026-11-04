use super::{MenuAction, MenuEntry};
use crate::Application::App;
use crate::Control::Commands::Command;
use crate::Interface::Overlays::Overlay;
use crate::Interface::Renderer;
use crate::Platform::Menus::Extensions::{ShellEntry, ShellMenuUpdate, ShellMenus, ShellTarget};
use crate::Services::Workers::NewIdentifier;
use std::cell::RefCell;
use crate::Services::Config::Ini;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;

const SkippedVerbs: [&str; 21] = [
    "open", "opennewwindow", "opennewprocess", "opennewtab", "explore", "find", "openas", "runas", "runasuser", "cut", "copy", "paste", "pastelink", "delete",
    "rename", "properties", "pintohome", "unpinfromhome", "copyaspath", "refresh", "undo",
];

const SkippedLabels: [&str; 13] = [
    "view", "sort by", "group by", "new", "open with", "open in new window", "open in new tab", "open in new process", "pin to quick access",
    "unpin from quick access", "copy as path", "open in terminal", "paste shortcut",
];

#[derive(Debug)]
pub struct MenuImage {
    Width: u32,
    Height: u32,
    Data: Vec<u8>,
    Cached: RefCell<Option<(u64, ID2D1Bitmap)>>,
}

impl MenuImage {
    pub fn Bitmap(&self, Draw: &Renderer) -> Option<ID2D1Bitmap> {
        let mut Cached = self.Cached.borrow_mut();
        if let Some((Generation, Bitmap)) = Cached.as_ref() {
            if *Generation == Draw.Generation {
                return Some(Bitmap.clone());
            }
        }
        let Bitmap = Draw.CreateBitmap(self.Width, self.Height, &self.Data)?;
        *Cached = Some((Draw.Generation, Bitmap.clone()));
        Some(Bitmap)
    }
}

impl App {
    pub fn RequestShellEntries(&mut self, Target: ShellTarget) {
        if self.ShellMenus.is_none() {
            self.ShellMenus = ShellMenus::Start(self.Window, self.Sink.clone());
        }
        let Token = NewIdentifier();
        let Some(Overlay::Menu(State)) = &mut self.Overlay else { return };
        State.Token = Token;
        if let Some(Service) = &self.ShellMenus {
            Service.Build(Token, Target, crate::Platform::KeyDown(0x10));
        }
    }

    pub fn InvokeShellCommand(&mut self, Token: u64, Command: u32, X: i32, Y: i32) {
        if let Some(Service) = &self.ShellMenus {
            Service.Invoke(Token, Command, X, Y);
        }
    }

    pub fn AcceptShellMenu(&mut self, Update: ShellMenuUpdate) {
        let (Token, Entries) = match Update {
            ShellMenuUpdate::Failed(Message) => {
                self.Fail("The shell command could not be run", &Message);
                return;
            }
            ShellMenuUpdate::Ready { Token, Entries } => (Token, Entries),
        };
        let Room = self.Height - 8.0;
        let Inline = self.Settings.ShellActionsInline;
        self.ShellUsage.Ensure(self.Paths.MenuUsageFile());
        let Usage = &self.ShellUsage;
        let Some(Overlay::Menu(State)) = &mut self.Overlay else { return };
        if Token == 0 || State.Token != Token {
            return;
        }
        let Existing = OwnLabels(&State.Entries);
        let Extra = Rank(Convert(Entries, Token, &Existing, 0, ""), Usage);
        if Extra.is_empty() {
            return;
        }
        let Anchor = State.Entries.iter().position(|Entry| matches!(Entry.Action, Some(MenuAction::Run(Command::Properties)))).unwrap_or(State.Entries.len());
        let Insert = if Anchor > 0 && State.Entries[Anchor - 1].IsSeparator() { Anchor - 1 } else { Anchor };
        let Height = |Entries: &[MenuEntry]| Entries.iter().map(|Entry| if Entry.IsSeparator() { super::SeparatorHeight } else { super::RowHeight }).sum::<f32>();
        let mut Free = Room - 8.0 - Height(&State.Entries) - super::SeparatorHeight - super::RowHeight;
        let mut Block = vec![MenuEntry::Separator()];
        let mut Rest = Extra.into_iter().peekable();
        let mut Shown = 0usize;
        while Inline {
            let Some(Next) = Rest.peek() else { break };
            let Separator = Next.IsSeparator();
            let Cost = if Separator { super::SeparatorHeight } else { super::RowHeight };
            if Cost > Free || (!Separator && Shown >= InlineLimit) {
                break;
            }
            Free -= Cost;
            Shown += usize::from(!Separator);
            Block.extend(Rest.next());
        }
        let Overflow: Vec<MenuEntry> = Rest.skip_while(MenuEntry::IsSeparator).collect();
        while Block.len() > 1 && Block.last().is_some_and(MenuEntry::IsSeparator) {
            Block.pop();
        }
        if !Overflow.is_empty() {
            Block.push(MenuEntry::Group(if Inline { "More" } else { "App actions" }, 0xE712, Overflow));
        }
        let Added = Block.len();
        State.Entries.splice(Insert..Insert, Block);
        if let Some(Value) = State.Selected.first_mut().and_then(Option::as_mut).filter(|Value| **Value >= Insert) {
            *Value += Added;
        }
        if let Some(Value) = State.Open.first_mut().filter(|Value| **Value >= Insert) {
            *Value += Added;
        }
        self.Invalidate();
    }
}

fn Normalize(Label: &str) -> String {
    Label.trim().trim_end_matches('…').trim_end_matches("...").trim().to_lowercase()
}

fn OwnLabels(Entries: &[MenuEntry]) -> HashSet<String> {
    let mut Labels = HashSet::new();
    for Entry in Entries {
        if !Entry.IsSeparator() {
            Labels.insert(Normalize(&Entry.Label));
        }
        for Child in &Entry.Children {
            if !Child.IsSeparator() {
                Labels.insert(Normalize(&Child.Label));
            }
        }
    }
    Labels
}

fn Skipped(Entry: &ShellEntry, Existing: &HashSet<String>) -> bool {
    let Label = Normalize(&Entry.Label);
    Existing.contains(&Label)
        || SkippedVerbs.contains(&Entry.Verb.as_str())
        || SkippedLabels.contains(&Label.as_str())
        || Label.starts_with("undo ")
        || Label.starts_with("redo ")
}

fn Image(Entry: &mut ShellEntry) -> Option<Rc<MenuImage>> {
    let Pixels = Entry.Image.take()?;
    Some(Rc::new(MenuImage { Width: Pixels.Width, Height: Pixels.Height, Data: Pixels.Data, Cached: RefCell::new(None) }))
}

fn Convert(Entries: Vec<ShellEntry>, Token: u64, Existing: &HashSet<String>, Depth: u32, Usage: &str) -> Vec<MenuEntry> {
    let mut Output: Vec<MenuEntry> = Vec::new();
    for mut Entry in Entries {
        if Entry.IsSeparator() {
            if Output.last().is_some_and(|Last| !Last.IsSeparator()) {
                Output.push(MenuEntry::Separator());
            }
            continue;
        }
        if Depth == 0 && Skipped(&Entry, Existing) {
            continue;
        }
        let Key = if Depth == 0 { Normalize(&Entry.Label) } else { Usage.to_string() };
        let Picture = Image(&mut Entry);
        if !Entry.Children.is_empty() {
            let Children = Convert(std::mem::take(&mut Entry.Children), Token, Existing, Depth + 1, &Key);
            if Children.iter().all(MenuEntry::IsSeparator) {
                continue;
            }
            let mut Group = MenuEntry::Group(&Entry.Label, 0, Children);
            Group.Image = Picture;
            Output.push(Group);
            continue;
        }
        let Some(Command) = Entry.Command else { continue };
        let mut Item = MenuEntry::Item(&Entry.Label, 0, MenuAction::Shell { Token, Command, Usage: Key }).Enable(Entry.Enabled);
        if Entry.Checked {
            Item = Item.Check(true);
        }
        Item.Image = Picture;
        Output.push(Item);
    }
    while Output.first().is_some_and(MenuEntry::IsSeparator) {
        Output.remove(0);
    }
    while Output.last().is_some_and(MenuEntry::IsSeparator) {
        Output.pop();
    }
    Output
}

#[derive(Default)]
pub struct ShellUsage {
    Loaded: bool,
    File: PathBuf,
    Counts: HashMap<String, (u32, u64)>,
}

const UsageSection: &str = "Usage";
const InlineLimit: usize = 8;

impl ShellUsage {
    fn Ensure(&mut self, File: PathBuf) {
        if self.Loaded {
            return;
        }
        self.Loaded = true;
        if let Some(Values) = Ini::Load(&File).as_ref().and_then(|Data| Data.Section(UsageSection).map(<[(String, String)]>::to_vec)) {
            for (Key, Value) in Values {
                let mut Parts = Value.split(',');
                let Count = Parts.next().and_then(|Text| Text.trim().parse().ok()).unwrap_or(0);
                let Last = Parts.next().and_then(|Text| Text.trim().parse().ok()).unwrap_or(0);
                self.Counts.insert(Key.to_lowercase(), (Count, Last));
            }
        }
        self.File = File;
    }

    fn Score(&self, Entry: &MenuEntry) -> (u32, u64) {
        self.Counts.get(&Normalize(&Entry.Label)).copied().unwrap_or((0, 0))
    }

    fn Record(&mut self, Key: &str) {
        if Key.is_empty() {
            return;
        }
        let Now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|Elapsed| Elapsed.as_secs()).unwrap_or(0);
        let Slot = self.Counts.entry(Key.replace(['=', '[', ']'], " ")).or_insert((0, 0));
        Slot.0 = Slot.0.saturating_add(1);
        Slot.1 = Now;
        let mut Data = Ini::Load(&self.File).unwrap_or_default();
        for (Name, (Count, Last)) in &self.Counts {
            Data.Set(UsageSection, Name, &format!("{Count},{Last}"));
        }
        let _ = Data.Save(&self.File);
    }
}

impl App {
    pub fn RecordShellUsage(&mut self, Key: &str) {
        self.ShellUsage.Ensure(self.Paths.MenuUsageFile());
        self.ShellUsage.Record(Key);
    }
}

fn Rank(Extra: Vec<MenuEntry>, Usage: &ShellUsage) -> Vec<MenuEntry> {
    if !Extra.iter().any(|Entry| Usage.Score(Entry).0 > 0) {
        return Extra;
    }
    let mut Items: Vec<MenuEntry> = Extra.into_iter().filter(|Entry| !Entry.IsSeparator()).collect();
    Items.sort_by_key(|Entry| std::cmp::Reverse(Usage.Score(Entry)));
    Items
}
