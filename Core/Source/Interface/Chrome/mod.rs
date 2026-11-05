pub mod Sidebar;

use super::FileView::Columns::{CountText, WithCommas};
use super::Widgets::Button;
use super::{Align, Hit, Rect, Style};
use crate::Application::{App, Focus};
use crate::Control::Commands::Command;
use crate::Control::Panes::LoadState;
use crate::FileSystem::{Location, Segments};
use crate::FileSystem::Operations::Phase;
use crate::Platform;
use FluxSdk::Utilities::FormatSize;

const GlyphBack: u16 = 0xE72B;
const GlyphForward: u16 = 0xE72A;
const GlyphUp: u16 = 0xE74A;
const GlyphRefresh: u16 = 0xE72C;
const GlyphSearch: u16 = 0xE721;
const GlyphClose: u16 = 0xE8BB;
const GlyphAdd: u16 = 0xE710;
const GlyphChevron: u16 = 0xE76C;
const GlyphMinimize: u16 = 0xE921;
const GlyphMaximize: u16 = 0xE922;
const GlyphRestore: u16 = 0xE923;
const GlyphSidebar: u16 = 0xE700;
const GlyphDual: u16 = 0xE8A9;
const GlyphInspector: u16 = 0xE8A1;
const GlyphMore: u16 = 0xE713;
const GlyphComputer: u16 = 0xE977;

pub fn DrawTitleBar(App: &mut App, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    App.Draw.Fill(Area, Colors.Chrome);
    App.Hits.Push(Area, Hit::Caption);
    let mut Remaining = Area;
    let Mark = Rect::New(Area.Left + 10.0, Area.Top + (Area.Height() - 20.0) / 2.0, 20.0, 20.0);
    let Pixel = (20.0 * App.Scale()).round() as u32;
    let Generation = App.Draw.Generation;
    if !App.Logo.as_ref().is_some_and(|(Cached, Size, _)| *Cached == Generation && *Size == Pixel) {
        App.Logo = crate::Platform::Imaging::ApplicationIcon(Pixel)
            .and_then(|Image| App.Draw.CreateBitmap(Image.Width, Image.Height, &Image.Data))
            .map(|Image| (Generation, Pixel, Image));
    }
    match &App.Logo {
        Some((_, _, Image)) => App.Draw.Bitmap(Image, Mark, 1.0),
        None => {
            App.Draw.FillRounded(Mark, 4.0, Colors.Accent);
            App.Draw.Label("F", Style::Strong, Mark, Colors.AccentText, Align::Center);
        }
    }
    Remaining.Left = Mark.Right + 10.0;
    let Title = App.Tab(App.ActivePane).Location.Title();
    App.Draw.Label(&Title, Style::Small, Rect::New(Remaining.Left, Area.Top, 220.0, Area.Height()), Colors.TextMuted, Align::Left);
    let Controls = [(GlyphMinimize, Hit::Minimize), (if Platform::IsMaximized(App.Window) { GlyphRestore } else { GlyphMaximize }, Hit::Maximize), (GlyphClose, Hit::Close)];
    for (Glyph, Target) in Controls.iter().rev() {
        let Cell = Remaining.TakeRight(46.0);
        let Hovered = App.Pointer.Hover == *Target;
        if Hovered {
            App.Draw.Fill(Cell, if *Target == Hit::Close { Colors.Danger } else { Colors.Hover });
        }
        let Color = if Hovered && *Target == Hit::Close { crate::Interface::Theme::Color::Hex(0xFFFFFF) } else { Colors.TextMuted };
        App.Draw.Glyph(*Glyph, Style::IconSmall, Cell, Color);
        App.Hits.Push(Cell, *Target);
    }
    Remaining.TakeRight(8.0);
    let Toggles = [
        (GlyphMore, Command::ViewMenu, false),
        (GlyphInspector, Command::ToggleInspector, App.Settings.ShowInspector),
        (GlyphDual, Command::ToggleDualPane, App.Settings.DualPane),
        (GlyphSidebar, Command::ToggleSidebar, App.Settings.ShowSidebar),
    ];
    for (Glyph, Item, Active) in Toggles {
        let Cell = Remaining.TakeRight(32.0).Inset(2.0, 4.0);
        Button(&mut App.Draw, &Look, Cell, Glyph, App.Pointer.Hover == Hit::Run(Item), true, Active);
        App.Hits.Push(Cell, Hit::Run(Item));
    }
    let Width = (Area.Width() * 0.36).clamp(160.0, 420.0);
    let Center = Rect::New(Area.Left + (Area.Width() - Width) / 2.0, Area.Top + 5.0, Width, Area.Height() - 10.0);
    if Center.Left > Mark.Right + 180.0 && Center.Right < Remaining.Right - 8.0 {
        let Hovered = App.Pointer.Hover == Hit::Run(Command::Palette);
        App.Draw.FillRounded(Center, Look.Sizes.Radius, if Hovered { Colors.Input.Mix(Colors.Text, 0.05) } else { Colors.Input });
        App.Draw.StrokeRounded(Center, Look.Sizes.Radius, Colors.Border, 1.0);
        App.Draw.Glyph(GlyphSearch, Style::IconSmall, Rect::New(Center.Left + 6.0, Center.Top, 20.0, Center.Height()), Colors.TextFaint);
        App.Draw.Label("Search commands and folders", Style::Small, Rect { Left: Center.Left + 30.0, ..Center }, Colors.TextFaint, Align::Left);
        let Shortcut = App.Keys.ShortcutText(Command::Palette).to_string();
        App.Draw.Label(&Shortcut, Style::Small, Rect { Right: Center.Right - 10.0, ..Center }, Colors.TextFaint, Align::Right);
        App.Hits.Push(Center, Hit::Run(Command::Palette));
    }
    App.Draw.HorizontalLine(Area.Left, Area.Right, Area.Bottom - 1.0, Colors.Border);
}

pub fn CrumbTarget(App: &App, Pane: usize, Index: usize) -> Option<Location> {
    if Index == 0 {
        return Some(Location::Computer);
    }
    let Root = App.Tab(Pane).Location.DirectoryPath()?.to_path_buf();
    Segments(&Root).into_iter().nth(Index - 1).map(|Segment| Location::Directory(Segment.Target))
}

fn DrawTabs(App: &mut App, Pane: usize, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    App.Draw.Fill(Area, Colors.Chrome);
    App.Hits.Push(Area, Hit::TabStrip { Pane: Pane as u8 });
    let Count = App.Panes[Pane].Tabs.len();
    let Available = Area.Width() - 44.0;
    let Titles: Vec<String> = App.Panes[Pane].Tabs.iter().map(|Tab| Tab.Title()).collect();
    let Natural: Vec<f32> = Titles.iter().map(|Title| (App.Draw.MeasureString(Title, Style::Small) + 46.0).clamp(96.0, 220.0)).collect();
    let Total: f32 = Natural.iter().sum();
    let Shrink = if Total > Available { Available / Total } else { 1.0 };
    let mut X = Area.Left + 6.0;
    let ActivePane = App.ActivePane == Pane;
    let TabKey = App.Panes[Pane].Tabs[App.Panes[Pane].Active.min(Count.saturating_sub(1))].Identifier;
    let TabReveal = App.Motion.Watch(crate::Interface::Widgets::Motion::Channel::Tab(Pane), TabKey, 180, false);
    for Index in 0..Count {
        let Width = (Natural[Index] * Shrink).max(56.0);
        let Cell = Rect::New(X, Area.Top + 4.0, Width, Area.Height() - 4.0);
        X += Width + 2.0;
        if Cell.Left > Area.Right - 40.0 {
            break;
        }
        let Active = App.Panes[Pane].Active == Index;
        let Target = Hit::Tab { Pane: Pane as u8, Index: Index as u16 };
        let Hovered = App.Pointer.Hover == Target || matches!(App.Pointer.Hover, Hit::TabClose { Pane: P, Index: I } if P as usize == Pane && I as usize == Index);
        if Active {
            App.Draw.FillRounded(Rect { Bottom: Cell.Bottom + 4.0, ..Cell }, App.Theme.Sizes.Radius, Colors.Surface.Mix(Colors.Chrome, 1.0 - TabReveal));
            if ActivePane {
                let Line = (Cell.Width() - 2.0) * TabReveal;
                App.Draw.Fill(Rect::New(Cell.Left + (Cell.Width() - Line) / 2.0, Cell.Top, Line, 2.0), Colors.Accent);
            }
        } else if Hovered {
            App.Draw.FillRounded(Cell.Inset(0.0, 2.0), App.Theme.Sizes.Radius, Colors.Hover);
        }
        let Loading = App.Panes[Pane].Tabs[Index].State == LoadState::Loading;
        let Label = Rect { Left: Cell.Left + 12.0, Right: Cell.Right - 26.0, ..Cell };
        let Color = if Active { Colors.Text } else { Colors.TextMuted };
        App.Draw.Label(&Titles[Index], Style::Small, Label, Color, Align::Left);
        if Loading {
            App.Draw.Fill(Rect::New(Cell.Left + 6.0, Cell.Bottom - 3.0, Cell.Width() - 12.0, 1.0), Colors.Accent.WithAlpha(0.6));
        }
        App.Hits.Push(Cell, Target);
        if Active || Hovered {
            let Close = Rect::New(Cell.Right - 24.0, Cell.Top + (Cell.Height() - 18.0) / 2.0, 18.0, 18.0);
            let CloseTarget = Hit::TabClose { Pane: Pane as u8, Index: Index as u16 };
            if App.Pointer.Hover == CloseTarget {
                App.Draw.FillRounded(Close, 2.0, Colors.Pressed);
            }
            App.Draw.Glyph(GlyphClose, Style::IconSmall, Close, Colors.TextMuted);
            App.Hits.Push(Close, CloseTarget);
        }
    }
    let Plus = Rect::New(X.min(Area.Right - 34.0), Area.Top + 5.0, 28.0, Area.Height() - 8.0);
    let PlusTarget = Hit::TabNew { Pane: Pane as u8 };
    if App.Pointer.Hover == PlusTarget {
        App.Draw.FillRounded(Plus, App.Theme.Sizes.Radius, Colors.Hover);
    }
    App.Draw.Glyph(GlyphAdd, Style::IconSmall, Plus, Colors.TextMuted);
    App.Hits.Push(Plus, PlusTarget);
}

fn DrawBreadcrumbs(App: &mut App, Pane: usize, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Radius = App.Theme.Sizes.Radius;
    let AddressTarget = Hit::Address { Pane: Pane as u8 };
    let Hovered = App.Pointer.Hover == AddressTarget;
    App.Draw.FillRounded(Area, Radius, if Hovered { Colors.Input.Mix(Colors.Text, 0.03) } else { Colors.Input });
    App.Draw.StrokeRounded(Area, Radius, Colors.Border, 1.0);
    App.Hits.Push(Area, AddressTarget);
    let Location = App.Tab(Pane).Location.clone();
    let mut Labels: Vec<(String, Option<usize>)> = vec![("This PC".to_string(), Some(0))];
    if let Some(Root) = Location.DirectoryPath() {
        for (Index, Segment) in Segments(Root).into_iter().enumerate() {
            Labels.push((Segment.Label, Some(Index + 1)));
        }
    }
    match &Location {
        Location::Search { Query, .. } => Labels.push((format!("Search “{Query}”"), None)),
        Location::Results { Title, .. } => Labels.push((Title.clone(), None)),
        _ => {}
    }
    let Sole = Labels.len() == 1;
    let Widths: Vec<f32> = Labels
        .iter()
        .enumerate()
        .map(|(Index, (Label, _))| match Index {
            0 if Sole => App.Draw.MeasureString(Label, Style::Body) + 36.0,
            0 => 30.0,
            _ => App.Draw.MeasureString(Label, Style::Body) + 14.0,
        })
        .collect();
    let Chevron = 16.0;
    let Inner = Area.Inset(4.0, 2.0);
    let mut Start = 0;
    let mut Total: f32 = Widths.iter().map(|Width| Width + Chevron).sum();
    while Start + 1 < Labels.len() && Total > Inner.Width() - 24.0 {
        Total -= Widths[Start] + Chevron;
        Start += 1;
    }
    let mut X = Inner.Left;
    if Start > 0 {
        App.Draw.Label("…", Style::Body, Rect::New(X, Inner.Top, 18.0, Inner.Height()), Colors.TextFaint, Align::Center);
        X += 18.0;
    }
    App.Draw.PushClip(Inner);
    for Index in Start..Labels.len() {
        let (Label, Target) = &Labels[Index];
        let Width = Widths[Index].min(Inner.Right - X - 4.0).max(30.0);
        let Cell = Rect::New(X, Inner.Top + 1.0, Width, Inner.Height() - 2.0);
        let Last = Index + 1 == Labels.len();
        if let Some(Segment) = Target {
            let CrumbHit = Hit::Crumb { Pane: Pane as u8, Index: *Segment as u16 };
            let DropOver = App.DropHover == Some(crate::Application::DropHover::Crumb { Pane, Index: *Segment as u16 });
            if DropOver {
                App.Draw.FillRounded(Cell, Radius, Colors.Selection);
                App.Draw.StrokeRounded(Cell, Radius, Colors.Accent, 1.0);
            } else if App.Pointer.Hover == CrumbHit {
                App.Draw.FillRounded(Cell, Radius, Colors.Hover);
            }
            App.Hits.Push(Cell, CrumbHit);
        }
        if Index == 0 {
            let Color = if Last { Colors.Text } else { Colors.TextMuted };
            App.Draw.Glyph(GlyphComputer, Style::IconSmall, Rect { Right: Cell.Left + 28.0, ..Cell }, Color);
            if Sole {
                App.Draw.Label(Label, Style::Body, Rect { Left: Cell.Left + 28.0, ..Cell }, Color, Align::Left);
            }
        } else {
            App.Draw.Label(Label, Style::Body, Cell, if Last { Colors.Text } else { Colors.TextMuted }, Align::Center);
        }
        X += Width;
        if let (Some(Segment), false) = (Target, Last && Target.is_some() && *Target == Some(0) && Labels.len() == 1) {
            let Arrow = Rect::New(X, Inner.Top + 3.0, Chevron, Inner.Height() - 6.0);
            let MenuHit = Hit::CrumbMenu { Pane: Pane as u8, Index: *Segment as u16 };
            if *Segment > 0 || Labels.len() > 1 {
                if App.Pointer.Hover == MenuHit {
                    App.Draw.FillRounded(Arrow, Radius, Colors.Hover);
                }
                App.Draw.Glyph(GlyphChevron, Style::IconSmall, Arrow, Colors.TextFaint);
                if *Segment > 0 {
                    App.Hits.Push(Arrow, MenuHit);
                }
                X += Chevron;
            }
        }
    }
    App.Draw.PopClip();
}

fn DrawToolbar(App: &mut App, Pane: usize, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    let Look = App.Theme.clone();
    App.Draw.Fill(Area, Colors.Surface);
    App.Draw.HorizontalLine(Area.Left, Area.Right, Area.Bottom - 1.0, Colors.Border);
    let mut Remaining = Area.Inset(6.0, 4.0);
    let Tab = App.Tab(Pane);
    let Enabled = [
        Tab.CanGoBack(),
        Tab.CanGoForward(),
        !matches!(Tab.Location, Location::Computer),
        true,
    ];
    let Buttons = [(GlyphBack, Command::Back), (GlyphForward, Command::Forward), (GlyphUp, Command::Up), (GlyphRefresh, Command::Refresh)];
    for ((Glyph, Item), Allowed) in Buttons.into_iter().zip(Enabled) {
        let Cell = Remaining.TakeLeft(28.0);
        let Hovered = App.Pointer.Hover == Hit::Run(Item) && App.ActivePane == Pane;
        Button(&mut App.Draw, &Look, Cell, Glyph, Hovered, Allowed, false);
        if Allowed {
            App.Hits.Push(Cell, Hit::Run(Item));
        }
        Remaining.TakeLeft(2.0);
    }
    Remaining.TakeLeft(6.0);
    let FilterWidth = (Remaining.Width() * 0.3).clamp(120.0, 260.0);
    let FilterArea = Remaining.TakeRight(FilterWidth);
    Remaining.TakeRight(6.0);
    let Editing = App.EditingAddress(Pane);
    if Editing {
        let Focused = Editing;
        App.Panes[Pane].Address.Draw(&mut App.Draw, &Look, Remaining, Focused, "Type a path, %VARIABLE% or \\\\server\\share");
        App.Hits.Push(Remaining, Hit::Address { Pane: Pane as u8 });
    } else {
        DrawBreadcrumbs(App, Pane, Remaining);
    }
    let FilterFocused = App.Focus == Focus::Filter && App.ActivePane == Pane;
    let SearchMode = matches!(App.Tab(Pane).Location, Location::Search { .. });
    let Placeholder = if SearchMode { "Refine search" } else { "Filter · Enter searches subfolders" };
    let Edit = &mut App.Panes[Pane].CurrentMut().Filter;
    let Inner = Rect { Left: FilterArea.Left, ..FilterArea };
    Edit.Draw(&mut App.Draw, &Look, Inner, FilterFocused, Placeholder);
    App.Hits.Push(FilterArea, Hit::Filter { Pane: Pane as u8 });
    let HasText = !App.Tab(Pane).Filter.IsEmpty() || SearchMode;
    if HasText {
        let Clear = Rect::New(FilterArea.Right - 24.0, FilterArea.Top + 3.0, 20.0, FilterArea.Height() - 6.0);
        App.Draw.Fill(Clear, Colors.Input);
        let ClearTarget = Hit::FilterClear { Pane: Pane as u8 };
        if App.Pointer.Hover == ClearTarget {
            App.Draw.FillRounded(Clear, 2.0, Colors.Hover);
        }
        App.Draw.Glyph(GlyphClose, Style::IconSmall, Clear, Colors.TextMuted);
        App.Hits.Push(Clear, ClearTarget);
    } else if !FilterFocused {
        App.Draw.Glyph(GlyphSearch, Style::IconSmall, Rect::New(FilterArea.Right - 24.0, FilterArea.Top, 20.0, FilterArea.Height()), Colors.TextFaint);
    }
}

pub fn DrawPaneHeader(App: &mut App, Pane: usize, Area: Rect) {
    let mut Remaining = Area;
    let Tabs = Remaining.TakeTop(App.Theme.Sizes.TabHeight);
    DrawTabs(App, Pane, Tabs);
    DrawToolbar(App, Pane, Remaining);
}

pub fn DrawStatusBar(App: &mut App, Area: Rect) {
    let Colors = App.Theme.Colors.clone();
    App.Draw.Fill(Area, Colors.Chrome);
    App.Draw.HorizontalLine(Area.Left, Area.Right, Area.Top, Colors.Border);
    let mut Remaining = Area.Inset(10.0, 0.0);
    let Pane = App.ActivePane;
    let Tab = App.Tab(Pane);
    let mut Summary = CountText(Tab.Order.len(), "item");
    if Tab.Order.len() != Tab.Listing.Len() - Tab.Listing.RemovedCount && Tab.State == LoadState::Ready {
        Summary.push_str(&format!(" (of {})", WithCommas((Tab.Listing.Len() - Tab.Listing.RemovedCount) as u64)));
    }
    if Tab.SelectedCount > 0 {
        let Bytes = Tab.SelectedBytes();
        Summary.push_str(&format!("  ·  {} selected", WithCommas(Tab.SelectedCount as u64)));
        if Bytes > 0 {
            Summary.push_str(&format!(" ({})", FormatSize(Bytes)));
        }
    }
    if let Some((Free, Total)) = App.Panes[Pane].FreeSpace {
        if Total > 0 {
            Summary.push_str(&format!("  ·  {} free of {}", FormatSize(Free), FormatSize(Total)));
        }
    }
    if let Some(Status) = &Tab.Status {
        Summary.push_str("  ·  ");
        Summary.push_str(Status);
    }
    let SummaryWidth = App.Draw.MeasureString(&Summary, Style::Small).min(Remaining.Width() * 0.55);
    let Left = Remaining.TakeLeft(SummaryWidth + 8.0);
    App.Draw.Label(&Summary, Style::Small, Left, Colors.TextMuted, Align::Left);
    if App.Settings.ShowPerformance {
        let Text = App.Performance.Summary();
        let Width = App.Draw.MeasureString(&Text, Style::Small) + 12.0;
        let Right = Remaining.TakeRight(Width);
        App.Draw.Label(&Text, Style::Small, Right, Colors.Accent, Align::Right);
    }
    let Running: Vec<(u64, String, f32, String)> = App
        .Operations
        .iter()
        .map(|Running| {
            let Snapshot = Running.Snapshot();
            let Fraction = if Snapshot.BytesTotal > 0 {
                Snapshot.BytesDone as f32 / Snapshot.BytesTotal as f32
            } else if Snapshot.ItemsTotal > 0 {
                Snapshot.ItemsDone as f32 / Snapshot.ItemsTotal as f32
            } else {
                0.0
            };
            let Detail = match Snapshot.Phase {
                Phase::Scanning => "scanning".to_string(),
                Phase::WaitingForUser => "waiting for decision".to_string(),
                _ if Snapshot.BytesTotal > 0 => format!("{} of {} · {}", FormatSize(Snapshot.BytesDone), FormatSize(Snapshot.BytesTotal), Snapshot.Current),
                _ => format!("{} of {} · {}", Snapshot.ItemsDone, Snapshot.ItemsTotal, Snapshot.Current),
            };
            (Running.Identifier, Running.Title.clone(), Fraction.clamp(0.0, 1.0), Detail)
        })
        .chain(App.Modules.Running().into_iter().map(|(Identifier, Title, Done, Total)| {
            let Fraction = if Total > 0 { Done as f32 / Total as f32 } else { 0.0 };
            (Identifier, Title, Fraction.clamp(0.0, 1.0), if Total > 0 { format!("{Done} of {Total}") } else { "running".to_string() })
        }))
        .collect();
    if let Some((Identifier, Title, Fraction, Detail)) = Running.first() {
        let Block = Remaining.TakeRight(Remaining.Width().min(460.0));
        let Cancel = Rect::New(Block.Right - 20.0, Block.Top + 3.0, 18.0, Block.Height() - 6.0);
        let CancelTarget = Hit::OperationCancel(*Identifier);
        if App.Pointer.Hover == CancelTarget {
            App.Draw.FillRounded(Cancel, 2.0, Colors.Hover);
        }
        App.Draw.Glyph(GlyphClose, Style::IconSmall, Cancel, Colors.TextMuted);
        App.Hits.Push(Cancel, CancelTarget);
        let Bar = Rect::New(Cancel.Left - 106.0, Block.Top + Block.Height() / 2.0 - 2.0, 100.0, 4.0);
        App.Draw.FillRounded(Bar, 2.0, Colors.Border);
        App.Draw.FillRounded(Rect { Right: Bar.Left + Bar.Width() * Fraction, ..Bar }, 2.0, Colors.Accent);
        let More = if Running.len() > 1 { format!(" (+{})", Running.len() - 1) } else { String::new() };
        let Text = format!("{Title}{More} · {Detail}");
        App.Draw.Label(&Text, Style::Small, Rect { Left: Block.Left, Right: Bar.Left - 8.0, ..Block }, Colors.Text, Align::Right);
    } else if let Some(Notice) = &App.Notice {
        let Text = Notice.Text.clone();
        let Color = if Notice.Error { Colors.Danger } else { Colors.TextMuted };
        let HasDetail = Notice.Detail.is_some();
        let Key = Text.bytes().fold(1469598103934665603u64, |Hash, Byte| (Hash ^ Byte as u64).wrapping_mul(1099511628211));
        let Reveal = App.Motion.Watch(crate::Interface::Widgets::Motion::Channel::Notice, Key, 260, true);
        let Slid = Rect { Right: Remaining.Right - (1.0 - Reveal) * 16.0, ..Remaining };
        App.Draw.Label(&Text, Style::Small, Slid, Color.WithAlpha(Color.A * Reveal), Align::Right);
        if HasDetail {
            App.Hits.Push(Remaining, Hit::StatusMessage);
        }
    }
}
