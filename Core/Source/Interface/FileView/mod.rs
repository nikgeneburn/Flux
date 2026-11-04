pub mod Columns;

use super::Theme::Metrics;
use super::Widgets::{DrawScrollbar, ScrollbarThumb};
use super::{Align, Fit, Hit, Rect, Style};
use super::Widgets::Motion::Channel;
use crate::Application::{App, DragState, Focus};
use crate::Control::Panes::{LoadState, Tab};
use crate::FileSystem::{AttributeHidden, FlagSelected, Location};
use crate::FileSystem::Sorting::SortKey;
use crate::Services::Config::{ColumnKind, ColumnSetting, ViewMode};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP};

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub Mode: ViewMode,
    pub Header: Rect,
    pub Content: Rect,
    pub ItemWidth: f32,
    pub ItemHeight: f32,
    pub PerLine: usize,
    pub Count: usize,
    pub Extent: f32,
    pub Horizontal: bool,
    pub Inset: f32,
}

pub fn Compute(Item: &Tab, Area: Rect, Sizes: &Metrics) -> Geometry {
    let Count = Item.Order.len();
    let mut Content = Area;
    let Header = if Item.View == ViewMode::Details { Content.TakeTop(Sizes.HeaderHeight) } else { Rect { Bottom: Area.Top, ..Area } };
    match Item.View {
        ViewMode::Details => Geometry {
            Mode: Item.View,
            Header,
            Content,
            ItemWidth: Content.Width(),
            ItemHeight: Sizes.RowHeight,
            PerLine: 1,
            Count,
            Extent: Count as f32 * Sizes.RowHeight + Sizes.RowHeight * 0.5,
            Horizontal: false,
            Inset: 0.0,
        },
        ViewMode::List => {
            let Inset = 4.0;
            let PerLine = (((Content.Height() - Inset * 2.0 - 12.0) / Sizes.RowHeight).floor() as usize).max(1);
            let ItemWidth = 240.0 + (Sizes.FontSize - 12.0) * 12.0;
            let Lines = Count.div_ceil(PerLine);
            Geometry { Mode: Item.View, Header, Content, ItemWidth, ItemHeight: Sizes.RowHeight, PerLine, Count, Extent: Lines as f32 * ItemWidth + Inset * 2.0, Horizontal: true, Inset }
        }
        ViewMode::Grid | ViewMode::Thumbnails => {
            let Inset = 8.0;
            let (BaseWidth, ItemHeight) = if Item.View == ViewMode::Grid {
                (104.0, 48.0 + 14.0 + Sizes.FontSize * 1.5 + 12.0)
            } else {
                (Item.ItemSize + 20.0, Item.ItemSize + 18.0 + Sizes.FontSize * 1.5 + 10.0)
            };
            let Usable = (Content.Width() - Inset * 2.0 - 8.0).max(BaseWidth);
            let PerLine = ((Usable / BaseWidth).floor() as usize).max(1);
            let ItemWidth = Usable / PerLine as f32;
            let Lines = Count.div_ceil(PerLine);
            Geometry { Mode: Item.View, Header, Content, ItemWidth, ItemHeight, PerLine, Count, Extent: Lines as f32 * ItemHeight + Inset * 2.0, Horizontal: false, Inset }
        }
    }
}

impl Geometry {
    pub fn Viewport(&self) -> f32 {
        if self.Horizontal { self.Content.Width() } else { self.Content.Height() }
    }

    pub fn MaxScroll(&self) -> f32 {
        (self.Extent - self.Viewport()).max(0.0)
    }

    pub fn ItemLocal(&self, Position: usize) -> Rect {
        match self.Mode {
            ViewMode::Details => Rect::New(0.0, Position as f32 * self.ItemHeight, self.ItemWidth, self.ItemHeight),
            ViewMode::List => {
                let Column = Position / self.PerLine;
                let Row = Position % self.PerLine;
                Rect::New(self.Inset + Column as f32 * self.ItemWidth, self.Inset + Row as f32 * self.ItemHeight, self.ItemWidth - 6.0, self.ItemHeight)
            }
            ViewMode::Grid | ViewMode::Thumbnails => {
                let Row = Position / self.PerLine;
                let Column = Position % self.PerLine;
                Rect::New(self.Inset + Column as f32 * self.ItemWidth + 2.0, self.Inset + Row as f32 * self.ItemHeight + 2.0, self.ItemWidth - 4.0, self.ItemHeight - 4.0)
            }
        }
    }

    pub fn ToWindow(&self, Local: Rect, Scroll: f32, ScrollX: f32) -> Rect {
        let (DeltaX, DeltaY) = if self.Horizontal { (Scroll, 0.0) } else { (ScrollX, Scroll) };
        Rect { Left: self.Content.Left + Local.Left - DeltaX, Top: self.Content.Top + Local.Top - DeltaY, Right: self.Content.Left + Local.Right - DeltaX, Bottom: self.Content.Top + Local.Bottom - DeltaY }
    }

    pub fn VisibleRange(&self, Scroll: f32) -> (usize, usize) {
        if self.Count == 0 {
            return (0, 0);
        }
        let Start = (Scroll - self.Inset).max(0.0);
        let End = Scroll + self.Viewport();
        let (FirstLine, LastLine, Size) = match self.Mode {
            ViewMode::List => (Start / self.ItemWidth, End / self.ItemWidth, self.PerLine),
            ViewMode::Details => (Start / self.ItemHeight, End / self.ItemHeight, 1),
            _ => (Start / self.ItemHeight, End / self.ItemHeight, self.PerLine),
        };
        let First = (FirstLine.floor() as usize) * Size;
        let Last = ((LastLine.ceil() as usize + 1) * Size).min(self.Count);
        (First.min(self.Count), Last)
    }

    pub fn PositionAt(&self, LocalX: f32, LocalY: f32) -> Option<usize> {
        if LocalX < 0.0 || LocalY < 0.0 {
            return None;
        }
        let Position = match self.Mode {
            ViewMode::Details => (LocalY / self.ItemHeight) as usize,
            ViewMode::List => {
                let Column = ((LocalX - self.Inset) / self.ItemWidth).floor();
                let Row = ((LocalY - self.Inset) / self.ItemHeight).floor();
                if Column < 0.0 || Row < 0.0 || Row as usize >= self.PerLine {
                    return None;
                }
                Column as usize * self.PerLine + Row as usize
            }
            _ => {
                let Column = ((LocalX - self.Inset) / self.ItemWidth).floor();
                let Row = ((LocalY - self.Inset) / self.ItemHeight).floor();
                if Column < 0.0 || Row < 0.0 || Column as usize >= self.PerLine {
                    return None;
                }
                Row as usize * self.PerLine + Column as usize
            }
        };
        if Position >= self.Count {
            return None;
        }
        let Local = self.ItemLocal(Position);
        if self.Mode != ViewMode::Details && !Local.Contains(LocalX, LocalY) {
            return None;
        }
        Some(Position)
    }
}

fn GeometryOf(App: &App, Pane: usize) -> Geometry {
    Compute(App.Tab(Pane), App.ViewAreas[Pane], &App.Theme.Sizes)
}

pub fn Reveal(Item: &mut Tab, Area: Rect, Sizes: &Metrics, Position: usize) {
    let Shape = Compute(Item, Area, Sizes);
    let Local = Shape.ItemLocal(Position);
    let (Start, End) = if Shape.Horizontal { (Local.Left - Shape.Inset, Local.Right + Shape.Inset) } else { (Local.Top - Shape.Inset, Local.Bottom + Shape.Inset) };
    let Viewport = Shape.Viewport();
    let mut Target = Item.Scroll.Target;
    if Start < Target {
        Target = Start;
    } else if End > Target + Viewport {
        Target = End - Viewport;
    }
    let Target = Target.clamp(0.0, Shape.MaxScroll());
    if (Target - Item.Scroll.Target).abs() > 0.1 {
        Item.Scroll.Jump(Target);
    }
}

pub fn ItemArea(App: &App, Pane: usize, Position: usize) -> Option<Rect> {
    let Shape = GeometryOf(App, Pane);
    if Position >= Shape.Count {
        return None;
    }
    let Tab = App.Tab(Pane);
    Some(Shape.ToWindow(Shape.ItemLocal(Position), Tab.Scroll.Offset, Tab.ScrollX))
}

pub fn ToContent(App: &App, Pane: usize, X: f32, Y: f32) -> (f32, f32) {
    let Shape = GeometryOf(App, Pane);
    let Tab = App.Tab(Pane);
    let (DeltaX, DeltaY) = if Shape.Horizontal { (Tab.Scroll.Offset, 0.0) } else { (Tab.ScrollX, Tab.Scroll.Offset) };
    (X - Shape.Content.Left + DeltaX, Y - Shape.Content.Top + DeltaY)
}

pub fn HitItem(App: &App, Pane: usize, X: f32, Y: f32) -> Option<usize> {
    let Shape = GeometryOf(App, Pane);
    if !Shape.Content.Contains(X, Y) {
        return None;
    }
    let (LocalX, LocalY) = ToContent(App, Pane, X, Y);
    let Position = Shape.PositionAt(LocalX, LocalY)?;
    if Shape.Mode == ViewMode::Details {
        let NameWidth = App.Settings.Columns.iter().find(|Column| Column.Kind == ColumnKind::Name).map(|Column| Column.Width).unwrap_or(300.0);
        let TotalWidth: f32 = VisibleColumns(App, Pane).iter().map(|Column| Column.Width).sum();
        if LocalX > TotalWidth.max(NameWidth) {
            return None;
        }
    }
    Some(Position)
}

pub fn ItemsInContentRect(App: &App, Pane: usize, Left: f32, Top: f32, Right: f32, Bottom: f32) -> Vec<usize> {
    let Shape = GeometryOf(App, Pane);
    let Area = Rect { Left, Top, Right, Bottom };
    let (Start, End) = if Shape.Horizontal { (Left, Right) } else { (Top, Bottom) };
    let Scroll = (Start - Shape.Viewport()).max(0.0);
    let (First, _) = Shape.VisibleRange(Scroll);
    let mut Output = Vec::new();
    for Position in First..Shape.Count {
        let Local = Shape.ItemLocal(Position);
        let Leading = if Shape.Horizontal { Local.Left } else { Local.Top };
        if Leading > End {
            break;
        }
        let Probe = if Shape.Mode == ViewMode::Details { Rect { Right: Local.Left + 400.0, ..Local } } else { Local };
        if Probe.Intersects(&Area) {
            Output.push(Position);
        }
    }
    Output
}

pub fn Step(App: &App, Pane: usize, From: usize, Key: u16) -> usize {
    let Shape = GeometryOf(App, Pane);
    let Last = Shape.Count.saturating_sub(1);
    let Page = match Shape.Mode {
        ViewMode::Details => ((Shape.Content.Height() / Shape.ItemHeight).floor() as usize).max(1) - 1,
        ViewMode::List => Shape.PerLine * ((Shape.Content.Width() / Shape.ItemWidth).floor() as usize).max(1),
        _ => Shape.PerLine * ((Shape.Content.Height() / Shape.ItemHeight).floor() as usize).max(1),
    }
    .max(1);
    let (Back, Forward) = match Shape.Mode {
        ViewMode::Details => (0, 0),
        ViewMode::List => (Shape.PerLine, Shape.PerLine),
        _ => (1, 1),
    };
    let (Up, Down) = match Shape.Mode {
        ViewMode::Grid | ViewMode::Thumbnails => (Shape.PerLine, Shape.PerLine),
        _ => (1, 1),
    };
    match Key {
        Key if Key == VK_UP.0 => From.saturating_sub(Up),
        Key if Key == VK_DOWN.0 => (From + Down).min(Last),
        Key if Key == VK_LEFT.0 => From.saturating_sub(Back),
        Key if Key == VK_RIGHT.0 => (From + Forward).min(Last),
        Key if Key == VK_PRIOR.0 => From.saturating_sub(Page),
        Key if Key == VK_NEXT.0 => (From + Page).min(Last),
        Key if Key == VK_HOME.0 => 0,
        Key if Key == VK_END.0 => Last,
        _ => From,
    }
}

pub fn PageExtent(App: &App, Pane: usize) -> f32 {
    GeometryOf(App, Pane).Viewport() * 0.9
}

pub fn Extents(App: &App, Pane: usize) -> (f32, f32) {
    let Shape = GeometryOf(App, Pane);
    (Shape.Extent, Shape.Viewport())
}

pub fn TrackArea(App: &App, Pane: usize) -> Option<Rect> {
    let Shape = GeometryOf(App, Pane);
    if Shape.Horizontal {
        return None;
    }
    Some(Rect::New(Shape.Content.Right - App.Theme.Sizes.ScrollbarWidth, Shape.Content.Top + 2.0, App.Theme.Sizes.ScrollbarWidth, Shape.Content.Height() - 4.0))
}

pub fn ScrollBy(App: &mut App, Pane: usize, Delta: f32, Immediate: bool) {
    let Shape = GeometryOf(App, Pane);
    let Tab = App.TabMut(Pane);
    let Target = (Tab.Scroll.Target + Delta).clamp(0.0, Shape.MaxScroll());
    if Immediate {
        Tab.Scroll.Jump(Target);
    } else {
        Tab.Scroll.Target = Target;
    }
    App.EnsureScrollTimer();
}

pub fn ScrollHorizontal(App: &mut App, Pane: usize, Delta: f32) {
    let Shape = GeometryOf(App, Pane);
    if Shape.Horizontal {
        ScrollBy(App, Pane, Delta, true);
        return;
    }
    if Shape.Mode == ViewMode::Details {
        let Total: f32 = VisibleColumns(App, Pane).iter().map(|Column| Column.Width).sum();
        let Limit = (Total - Shape.Content.Width() + 20.0).max(0.0);
        let Tab = App.TabMut(Pane);
        Tab.ScrollX = (Tab.ScrollX + Delta).clamp(0.0, Limit);
    }
}

pub fn VisibleColumns(App: &App, Pane: usize) -> Vec<ColumnSetting> {
    let ShowFolder = matches!(App.Tab(Pane).Location, Location::Search { .. } | Location::Results { .. });
    Columns::Visible(&App.Settings.Columns, ShowFolder)
}

fn SortKeyFor(Kind: ColumnKind) -> Option<SortKey> {
    Some(match Kind {
        ColumnKind::Name => SortKey::Name,
        ColumnKind::Size => SortKey::Size,
        ColumnKind::Modified => SortKey::Modified,
        ColumnKind::Created => SortKey::Created,
        ColumnKind::Type => SortKey::Type,
        ColumnKind::Folder => SortKey::Folder,
        ColumnKind::Attributes => return None,
    })
}

fn DrawHeader(App: &mut App, Pane: usize, Shape: &Geometry) {
    let Colors = App.Theme.Colors.clone();
    let Area = Shape.Header;
    App.Draw.Fill(Area, Colors.Surface);
    App.Draw.HorizontalLine(Area.Left, Area.Right, Area.Bottom - 1.0, Colors.Border);
    let Sort = App.Tab(Pane).Sort;
    let ScrollX = App.Tab(Pane).ScrollX;
    let ShowFolder = matches!(App.Tab(Pane).Location, Location::Search { .. } | Location::Results { .. });
    let Hover = App.Pointer.Hover;
    App.Draw.PushClip(Area);
    let mut X = Area.Left - ScrollX;
    for (SettingIndex, Column) in App.Settings.Columns.clone().iter().enumerate() {
        if !Column.Visible || (Column.Kind == ColumnKind::Folder && !ShowFolder) {
            continue;
        }
        let Cell = Rect::New(X, Area.Top, Column.Width, Area.Height());
        let Target = Hit::Header { Pane: Pane as u8, Column: SettingIndex as u8 };
        if Hover == Target {
            App.Draw.Fill(Cell, Colors.Hover);
        }
        let Sorted = SortKeyFor(Column.Kind) == Some(Sort.Key);
        let Label = Cell.Inset(10.0, 0.0);
        let Alignment = if Column.Kind.RightAligned() { Align::Right } else { Align::Left };
        let LabelArea = if Sorted && Alignment == Align::Right { Rect { Right: Label.Right - 14.0, ..Label } } else { Label };
        App.Draw.Label(Column.Kind.Name(), Style::Small, LabelArea, if Sorted { Colors.Text } else { Colors.TextMuted }, Alignment);
        if Sorted {
            let Glyph = if Sort.Descending { 0xE70D } else { 0xE70E };
            let GlyphArea = if Alignment == Align::Right { Rect::New(Label.Right - 10.0, Cell.Top, 10.0, Cell.Height()) } else { Rect::New(Cell.Right - 20.0, Cell.Top, 12.0, Cell.Height()) };
            App.Draw.Glyph(Glyph, Style::IconSmall, GlyphArea, Colors.Accent);
        }
        App.Draw.VerticalLine(Cell.Right - 1.0, Cell.Top + 6.0, Cell.Bottom - 6.0, Colors.Border);
        App.Hits.Push(Cell, Target);
        App.Hits.Push(Rect::New(Cell.Right - 4.0, Cell.Top, 8.0, Cell.Height()), Hit::HeaderResize { Pane: Pane as u8, Column: SettingIndex as u8 });
        X += Column.Width;
    }
    App.Draw.PopClip();
}

fn EmptyMessage(Item: &Tab) -> Option<(String, bool)> {
    match &Item.State {
        LoadState::Failed(Message) => Some((Message.clone(), true)),
        LoadState::Loading if Item.Order.is_empty() => {
            let Waited = Item.LoadStarted.map(|Start| Start.elapsed().as_millis()).unwrap_or(0);
            if Waited > 120 { Some(("Loading…".to_string(), false)) } else { None }
        }
        _ if Item.Order.is_empty() => {
            if Item.FilterQuery.is_some() {
                Some(("No items match the filter".to_string(), false))
            } else if matches!(Item.Location, Location::Search { .. }) && Item.State == LoadState::Ready {
                Some(("No matches".to_string(), false))
            } else if Item.State == LoadState::Ready {
                Some(("This folder is empty".to_string(), false))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn DrawFileView(App: &mut App, Pane: usize, Area: Rect) {
    let Sizes = App.Theme.Sizes.clone();
    let Colors = App.Theme.Colors.clone();
    App.Draw.Fill(Area, Colors.Background);
    let Shape = Compute(App.Tab(Pane), Area, &Sizes);
    {
        let Tab = App.TabMut(Pane);
        Tab.Scroll.Clamp(Shape.MaxScroll());
    }
    if Shape.Mode == ViewMode::Details {
        DrawHeader(App, Pane, &Shape);
    }
    App.Hits.Push(Shape.Content, Hit::List { Pane: Pane as u8 });
    if let Some((Message, Error)) = EmptyMessage(App.Tab(Pane)) {
        let Box = Rect::New(Shape.Content.Left, Shape.Content.Top + 36.0, Shape.Content.Width(), 40.0);
        if Error {
            App.Draw.Glyph(0xE783, Style::Icon, Rect::New(Box.Left, Box.Top - 26.0, Box.Width(), 24.0), Colors.Danger);
        }
        App.Draw.Label(&Message, Style::Body, Box, if Error { Colors.Danger } else { Colors.TextMuted }, Align::Center);
    }
    let Focused = App.Focus == Focus::List && App.ActivePane == Pane && App.Active;
    let SelectionColor = if Focused || App.Focus == Focus::Rename { Colors.Selection } else { Colors.SelectionInactive };
    let Hovered = if matches!(App.Pointer.Drag, DragState::Idle) && Shape.Content.Contains(App.Pointer.X, App.Pointer.Y) && App.Overlay.is_none() {
        HitItem(App, Pane, App.Pointer.X, App.Pointer.Y)
    } else {
        None
    };
    let HoverKey = Hovered.map(|Position| Position as u64 + 1).unwrap_or(0) ^ (App.Tab(Pane).Identifier << 40);
    let HoverFade = App.Motion.Watch(Channel::Hover(Pane), HoverKey, 120, true);
    let DriveListing = !App.Tab(Pane).Listing.Drives.is_empty() && App.Tab(Pane).Listing.Root.is_none();
    let DropTarget = match App.DropHover {
        Some(crate::Application::DropHover::List { Pane: Over, Target }) if Over == Pane => Some(Target),
        _ => None,
    };
    let Visible = if Shape.Mode == ViewMode::Details { Columns::Visible(&App.Settings.Columns, matches!(App.Tab(Pane).Location, Location::Search { .. } | Location::Results { .. })) } else { Vec::new() };
    let Scale = App.Scale();
    let (Scroll, ScrollX) = (App.Tab(Pane).Scroll.Offset, App.Tab(Pane).ScrollX);
    let (First, Last) = Shape.VisibleRange(Scroll);
    let RenameIndex = App.Rename.as_ref().filter(|State| State.Pane == Pane && State.TabIdentifier == App.Tab(Pane).Identifier).map(|State| State.Index);
    let mut Text = String::with_capacity(128);
    let mut RenameArea: Option<Rect> = None;
    App.Draw.PushClip(Shape.Content);
    for Position in First..Last {
        let Index = App.Tab(Pane).Order[Position];
        let Item = App.Tab(Pane).Listing.Entries[Index as usize];
        let Area = Shape.ToWindow(Shape.ItemLocal(Position), Scroll, ScrollX);
        let Row = if Shape.Mode == ViewMode::Details { Rect { Left: Shape.Content.Left, Right: Shape.Content.Right, ..Area } } else { Area };
        let Selected = Item.Flags & FlagSelected != 0;
        let IsCursor = App.Tab(Pane).Cursor == Some(Index);
        let Faded = Item.Attributes & AttributeHidden != 0;
        let Radius = if Shape.Mode == ViewMode::Details { 0.0 } else { Sizes.Radius };
        if Selected {
            App.Draw.FillRounded(Row, Radius, SelectionColor);
        } else if Hovered == Some(Position) {
            App.Draw.FillRounded(Row, Radius, Colors.Hover.WithAlpha(Colors.Hover.A * HoverFade));
        }
        if IsCursor && Focused && !Selected {
            App.Draw.StrokeRounded(Row, Radius, Colors.TextFaint, 1.0);
        }
        if IsCursor && Focused && Selected && Shape.Mode == ViewMode::Details {
            App.Draw.Fill(Rect::New(Row.Left, Row.Top, 2.0, Row.Height()), Colors.Accent);
        }
        if DropTarget == Some(Some(Index)) {
            App.Draw.StrokeRounded(Row, Radius, Colors.Accent, 1.5);
        }
        let TextColor = if Faded { Colors.TextMuted } else { Colors.Text };
        let IconOpacity = if Faded { 0.55 } else { 1.0 };
        match Shape.Mode {
            ViewMode::Details => {
                let mut X = Shape.Content.Left - ScrollX;
                for Column in &Visible {
                    let Cell = Rect::New(X, Area.Top, Column.Width, Area.Height());
                    X += Column.Width;
                    if Cell.Right < Shape.Content.Left || Cell.Left > Shape.Content.Right {
                        continue;
                    }
                    if Column.Kind == ColumnKind::Name {
                        let IconArea = Rect::New(Cell.Left + 8.0, Cell.Top + (Cell.Height() - 16.0) / 2.0, 16.0, 16.0);
                        DrawIcon(App, Pane, Index, IconArea, (16.0 * Scale) as u32, IconOpacity);
                        let NameArea = Rect { Left: IconArea.Right + 7.0, Right: Cell.Right - 6.0, ..Cell };
                        if RenameIndex == Some(Index) {
                            RenameArea = Some(Rect { Left: NameArea.Left - 4.0, Top: Cell.Top + 1.0, Right: NameArea.Right.max(NameArea.Left + 160.0), Bottom: Cell.Bottom - 1.0 });
                            continue;
                        }
                        Columns::CellText(&mut Text, &App.Tab(Pane).Listing, Index, Column.Kind);
                        App.Draw.Label(&Text, Style::Body, NameArea, TextColor, Align::Left);
                    } else {
                        Columns::CellText(&mut Text, &App.Tab(Pane).Listing, Index, Column.Kind);
                        let Alignment = if Column.Kind.RightAligned() { Align::Right } else { Align::Left };
                        App.Draw.Label(&Text, Style::Body, Cell.Inset(10.0, 0.0), Colors.TextMuted, Alignment);
                        if DriveListing && Column.Kind == ColumnKind::Size {
                            if let Some(Drive) = App.Tab(Pane).Listing.Drives.get(Index as usize).filter(|Drive| Drive.Total > 0) {
                                let Bar = Rect::New(Cell.Left + 10.0, Cell.Top + (Cell.Height() - 6.0) / 2.0, Cell.Width() - 20.0, 6.0);
                                App.Draw.UsageBar(Bar, 1.0 - Drive.Free as f32 / Drive.Total as f32, Colors.Border);
                            }
                        }
                    }
                }
            }
            ViewMode::List => {
                let IconArea = Rect::New(Area.Left + 6.0, Area.Top + (Area.Height() - 16.0) / 2.0, 16.0, 16.0);
                DrawIcon(App, Pane, Index, IconArea, (16.0 * Scale) as u32, IconOpacity);
                let NameArea = Rect { Left: IconArea.Right + 6.0, Right: Area.Right - 4.0, ..Area };
                if RenameIndex == Some(Index) {
                    RenameArea = Some(Rect { Left: NameArea.Left - 4.0, ..NameArea });
                    continue;
                }
                Columns::CellText(&mut Text, &App.Tab(Pane).Listing, Index, ColumnKind::Name);
                App.Draw.Label(&Text, Style::Body, NameArea, TextColor, Align::Left);
            }
            ViewMode::Grid | ViewMode::Thumbnails => {
                let LabelHeight = Sizes.FontSize * 1.5 + 4.0;
                let Visual = Rect { Bottom: Area.Bottom - LabelHeight - 2.0, ..Area }.Inset(6.0, 6.0);
                if Shape.Mode == ViewMode::Grid {
                    let IconArea = Rect::New(Visual.Left + (Visual.Width() - 48.0) / 2.0, Visual.Top + (Visual.Height() - 48.0) / 2.0, 48.0, 48.0);
                    DrawIcon(App, Pane, Index, IconArea, (48.0 * Scale) as u32, IconOpacity);
                } else {
                    let Path = App.Tab(Pane).Listing.FullPath(Index);
                    let Pixel = (Visual.Height().min(Visual.Width()) * Scale) as u32;
                    match App.Media.Thumbnail(&Path, Pixel, false) {
                        Some((Image, Width, Height)) => {
                            let Target = Fit(Width as f32 / Scale, Height as f32 / Scale, Visual);
                            App.Draw.Bitmap(&Image, Target, IconOpacity);
                        }
                        None => {
                            let Size = (Visual.Height().min(Visual.Width()) * 0.5).min(96.0);
                            let IconArea = Rect::New(Visual.Left + (Visual.Width() - Size) / 2.0, Visual.Top + (Visual.Height() - Size) / 2.0, Size, Size);
                            DrawIcon(App, Pane, Index, IconArea, (Size * Scale) as u32, IconOpacity * 0.7);
                        }
                    }
                }
                let LabelArea = Rect::New(Area.Left + 4.0, Area.Bottom - LabelHeight - 2.0, Area.Width() - 8.0, LabelHeight);
                if DriveListing {
                    if let Some(Drive) = App.Tab(Pane).Listing.Drives.get(Index as usize).filter(|Drive| Drive.Total > 0) {
                        let Bar = Rect::New(Area.Left + 12.0, LabelArea.Top - 6.0, Area.Width() - 24.0, 5.0);
                        App.Draw.UsageBar(Bar, 1.0 - Drive.Free as f32 / Drive.Total as f32, Colors.Border);
                    }
                }
                if RenameIndex == Some(Index) {
                    RenameArea = Some(Rect { Left: LabelArea.Left - 20.0, Right: LabelArea.Right + 20.0, ..LabelArea });
                    continue;
                }
                Columns::CellText(&mut Text, &App.Tab(Pane).Listing, Index, ColumnKind::Name);
                App.Draw.Label(&Text, Style::Body, LabelArea, TextColor, Align::Center);
            }
        }
    }
    if let DragState::Rubber { Pane: RubberPane, StartX, StartY, X, Y, .. } = &App.Pointer.Drag {
        if *RubberPane == Pane {
            let Local = Rect { Left: StartX.min(*X), Top: StartY.min(*Y), Right: StartX.max(*X), Bottom: StartY.max(*Y) };
            let Band = Shape.ToWindow(Local, Scroll, ScrollX);
            App.Draw.Fill(Band, Colors.Accent.WithAlpha(0.12));
            App.Draw.Stroke(Band, Colors.Accent.WithAlpha(0.7));
        }
    }
    let Tab = App.Tab(Pane);
    let ContentKey = Tab.Identifier.wrapping_mul(31).wrapping_add(Tab.Token).wrapping_mul(2).wrapping_add((Tab.State == LoadState::Ready) as u64);
    let Reveal = App.Motion.Watch(Channel::Content(Pane), ContentKey, 170, true);
    if Reveal < 1.0 {
        App.Draw.Fill(Shape.Content, Colors.Background.WithAlpha((1.0 - Reveal) * 0.9));
    }
    App.Draw.PopClip();
    if let Some(Field) = RenameArea {
        let Look = App.Theme.clone();
        if let Some(State) = &mut App.Rename {
            App.Draw.Fill(Field.Inset(-1.0, -1.0), Colors.Background);
            State.Edit.Draw(&mut App.Draw, &Look, Field, true, "");
        }
    }
    if DropTarget == Some(None) {
        App.Draw.StrokeRounded(Shape.Content.Inset(1.0, 1.0), 0.0, Colors.Accent, 2.0);
    }
    if App.Tab(Pane).State == LoadState::Loading && !App.Tab(Pane).Order.is_empty() {
        let Badge = format!("Loading… {}", Columns::WithCommas(App.Tab(Pane).Listing.Len() as u64));
        let Width = App.Draw.MeasureString(&Badge, Style::Small) + 20.0;
        let Pill = Rect::New(Shape.Content.Right - Width - 16.0, Shape.Content.Bottom - 30.0, Width, 22.0);
        App.Draw.FillRounded(Pill, Sizes.Radius, Colors.Raised);
        App.Draw.StrokeRounded(Pill, Sizes.Radius, Colors.Border, 1.0);
        App.Draw.Label(&Badge, Style::Small, Pill, Colors.TextMuted, Align::Center);
    }
    if Shape.Horizontal {
        if Shape.Extent > Shape.Viewport() {
            let Track = Rect::New(Shape.Content.Left + 4.0, Shape.Content.Bottom - 6.0, Shape.Content.Width() - 8.0, 4.0);
            let Ratio = Shape.Viewport() / Shape.Extent;
            let Width = (Track.Width() * Ratio).max(24.0);
            let Left = Track.Left + (Track.Width() - Width) * (Scroll / Shape.MaxScroll().max(1.0));
            App.Draw.FillRounded(Rect::New(Left, Track.Top, Width, 4.0), 2.0, Colors.TextMuted.WithAlpha(0.35));
        }
    } else if let Some(Track) = TrackArea(App, Pane) {
        let Active = matches!(App.Pointer.Hover, Hit::ScrollThumb { Pane: Over } | Hit::ScrollTrack { Pane: Over } if Over as usize == Pane)
            || matches!(App.Pointer.Drag, DragState::Thumb { Pane: Over, .. } if Over == Pane);
        if ScrollbarThumb(Track, Scroll, Shape.Extent, Shape.Viewport()).is_some() {
            App.Hits.Push(Track, Hit::ScrollTrack { Pane: Pane as u8 });
            let Look = App.Theme.clone();
            if let Some(Thumb) = DrawScrollbar(&App.Draw, &Look, Track, Scroll, Shape.Extent, Shape.Viewport(), Active) {
                App.Hits.Push(Thumb, Hit::ScrollThumb { Pane: Pane as u8 });
            }
        }
    }
}

fn DrawIcon(App: &mut App, Pane: usize, Index: u32, Area: Rect, PixelSize: u32, Opacity: f32) {
    let Listing = &mut App.Panes[Pane].CurrentMut().Listing;
    let IconIndex = App.Media.IconIndex(Listing, Index);
    if let Some(Image) = App.Media.IconBitmap(&App.Draw, IconIndex, PixelSize) {
        App.Draw.Bitmap(&Image, Area, Opacity);
    }
}
