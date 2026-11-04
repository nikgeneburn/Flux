pub mod Motion;

use super::Theme::Theme;
use super::{Align, Rect, Renderer, Style};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_DELETE, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditOutcome {
    Unhandled,
    Handled,
    Changed,
    Submit,
    Cancel,
}

#[derive(Clone, Default, Debug)]
pub struct LineEdit {
    pub Text: Vec<u16>,
    pub Caret: usize,
    pub Anchor: usize,
    pub Offset: f32,
}

fn IsWordUnit(Unit: u16) -> bool {
    char::from_u32(Unit as u32).is_some_and(|Character| Character.is_alphanumeric()) || Unit == b'_' as u16 || Unit >= 0x80
}

impl LineEdit {
    pub fn New() -> LineEdit {
        LineEdit::default()
    }

    pub fn SetText(&mut self, Text: &str) {
        self.Text = Text.encode_utf16().collect();
        self.Caret = self.Text.len();
        self.Anchor = self.Caret;
        self.Offset = 0.0;
    }

    pub fn Value(&self) -> String {
        String::from_utf16_lossy(&self.Text)
    }

    pub fn IsEmpty(&self) -> bool {
        self.Text.is_empty()
    }

    pub fn Clear(&mut self) {
        self.Text.clear();
        self.Caret = 0;
        self.Anchor = 0;
        self.Offset = 0.0;
    }

    pub fn SelectAll(&mut self) {
        self.Anchor = 0;
        self.Caret = self.Text.len();
    }

    pub fn SelectRange(&mut self, Start: usize, End: usize) {
        self.Anchor = Start.min(self.Text.len());
        self.Caret = End.min(self.Text.len());
    }

    pub fn Selection(&self) -> (usize, usize) {
        (self.Caret.min(self.Anchor), self.Caret.max(self.Anchor))
    }

    pub fn HasSelection(&self) -> bool {
        self.Caret != self.Anchor
    }

    fn DeleteSelection(&mut self) -> bool {
        let (Start, End) = self.Selection();
        if Start == End {
            return false;
        }
        self.Text.drain(Start..End);
        self.Caret = Start;
        self.Anchor = Start;
        true
    }

    pub fn Insert(&mut self, Units: &[u16]) {
        self.DeleteSelection();
        let Clean: Vec<u16> = Units.iter().copied().filter(|&Unit| Unit >= 32 && Unit != 127).collect();
        self.Text.splice(self.Caret..self.Caret, Clean.iter().copied());
        self.Caret += Clean.len();
        self.Anchor = self.Caret;
    }

    fn PreviousBoundary(&self, From: usize) -> usize {
        let mut Position = From;
        while Position > 0 && !IsWordUnit(self.Text[Position - 1]) {
            Position -= 1;
        }
        while Position > 0 && IsWordUnit(self.Text[Position - 1]) {
            Position -= 1;
        }
        Position
    }

    fn NextBoundary(&self, From: usize) -> usize {
        let mut Position = From;
        while Position < self.Text.len() && IsWordUnit(self.Text[Position]) {
            Position += 1;
        }
        while Position < self.Text.len() && !IsWordUnit(self.Text[Position]) {
            Position += 1;
        }
        Position
    }

    fn StepLeft(&self, From: usize) -> usize {
        let mut Position = From.saturating_sub(1);
        if Position > 0 && (0xDC00..0xE000).contains(&self.Text[Position]) {
            Position -= 1;
        }
        Position
    }

    fn StepRight(&self, From: usize) -> usize {
        let mut Position = (From + 1).min(self.Text.len());
        if Position < self.Text.len() && (0xDC00..0xE000).contains(&self.Text[Position]) {
            Position += 1;
        }
        Position
    }

    pub fn HandleChar(&mut self, Unit: u16) -> EditOutcome {
        if Unit < 32 || Unit == 127 {
            return EditOutcome::Unhandled;
        }
        self.Insert(&[Unit]);
        EditOutcome::Changed
    }

    pub fn HandleKey(&mut self, Key: u16, Control: bool, Shift: bool, Window: HWND) -> EditOutcome {
        let Move = |Edit: &mut LineEdit, Position: usize| {
            Edit.Caret = Position;
            if !Shift {
                Edit.Anchor = Position;
            }
            EditOutcome::Handled
        };
        match Key {
            Key if Key == VK_RETURN.0 => EditOutcome::Submit,
            Key if Key == VK_ESCAPE.0 => EditOutcome::Cancel,
            Key if Key == VK_LEFT.0 => {
                if !Shift && self.HasSelection() {
                    let (Start, _) = self.Selection();
                    return Move(self, Start);
                }
                let Target = if Control { self.PreviousBoundary(self.Caret) } else { self.StepLeft(self.Caret) };
                Move(self, Target)
            }
            Key if Key == VK_RIGHT.0 => {
                if !Shift && self.HasSelection() {
                    let (_, End) = self.Selection();
                    return Move(self, End);
                }
                let Target = if Control { self.NextBoundary(self.Caret) } else { self.StepRight(self.Caret) };
                Move(self, Target)
            }
            Key if Key == VK_HOME.0 => Move(self, 0),
            Key if Key == VK_END.0 => {
                let End = self.Text.len();
                Move(self, End)
            }
            Key if Key == VK_BACK.0 => {
                if !self.DeleteSelection() {
                    let Start = if Control { self.PreviousBoundary(self.Caret) } else { self.StepLeft(self.Caret) };
                    if Start == self.Caret {
                        return EditOutcome::Handled;
                    }
                    self.Text.drain(Start..self.Caret);
                    self.Caret = Start;
                    self.Anchor = Start;
                }
                EditOutcome::Changed
            }
            Key if Key == VK_DELETE.0 => {
                if !self.DeleteSelection() {
                    let End = if Control { self.NextBoundary(self.Caret) } else { self.StepRight(self.Caret) };
                    if End == self.Caret {
                        return EditOutcome::Handled;
                    }
                    self.Text.drain(self.Caret..End);
                }
                EditOutcome::Changed
            }
            Key if Control && Key == b'A' as u16 => {
                self.SelectAll();
                EditOutcome::Handled
            }
            Key if Control && (Key == b'C' as u16 || Key == b'X' as u16) => {
                let (Start, End) = self.Selection();
                if Start != End {
                    crate::Platform::Clipboard::SetText(Window, &String::from_utf16_lossy(&self.Text[Start..End]));
                    if Key == b'X' as u16 {
                        self.DeleteSelection();
                        return EditOutcome::Changed;
                    }
                }
                EditOutcome::Handled
            }
            Key if Control && Key == b'V' as u16 => {
                if let Some(Pasted) = crate::Platform::Clipboard::GetText(Window) {
                    let Line: Vec<u16> = Pasted.lines().next().unwrap_or("").encode_utf16().collect();
                    self.Insert(&Line);
                    return EditOutcome::Changed;
                }
                EditOutcome::Handled
            }
            _ => EditOutcome::Unhandled,
        }
    }

    pub fn ClickAt(&mut self, Draw: &Renderer, Area: Rect, X: f32, Extend: bool) {
        let Offsets = Draw.CaretOffsets(&self.Text, Style::Body);
        let Local = X - Area.Left - 6.0 + self.Offset;
        let mut Best = 0usize;
        let mut BestDistance = f32::MAX;
        for (Index, Position) in Offsets.iter().enumerate() {
            let Distance = (Position - Local).abs();
            if Distance < BestDistance {
                BestDistance = Distance;
                Best = Index;
            }
        }
        self.Caret = Best;
        if !Extend {
            self.Anchor = Best;
        }
    }

    pub fn SelectWordAt(&mut self, Draw: &Renderer, Area: Rect, X: f32) {
        self.ClickAt(Draw, Area, X, false);
        let Start = if self.Caret > 0 && IsWordUnit(self.Text[self.Caret - 1]) { self.PreviousBoundary(self.Caret) } else { self.Caret };
        let mut End = self.Caret;
        while End < self.Text.len() && IsWordUnit(self.Text[End]) {
            End += 1;
        }
        self.Anchor = Start;
        self.Caret = End;
    }

    pub fn Draw(&mut self, Draw: &mut Renderer, Look: &Theme, Area: Rect, Focused: bool, Placeholder: &str) {
        let Colors = &Look.Colors;
        Draw.FillRounded(Area, Look.Sizes.Radius, Colors.Input);
        Draw.StrokeRounded(Area, Look.Sizes.Radius, if Focused { Colors.Accent } else { Colors.Border }, 1.0);
        let Inner = Area.Inset(6.0, 0.0);
        if self.Text.is_empty() {
            if !Placeholder.is_empty() {
                Draw.Label(Placeholder, Style::Body, Inner, Colors.TextFaint, Align::Left);
            }
            if Focused {
                Draw.Fill(Rect::New(Inner.Left, Inner.Top + 5.0, 1.0, Inner.Height() - 10.0), Colors.Text);
            }
            return;
        }
        if !Focused {
            self.Offset = 0.0;
            Draw.Text(&self.Text, Style::Body, Inner, Colors.Text, Align::Left);
            return;
        }
        let Offsets = Draw.CaretOffsets(&self.Text, Style::Body);
        let CaretX = Offsets.get(self.Caret).copied().unwrap_or(0.0);
        let Visible = Inner.Width() - 2.0;
        if CaretX - self.Offset > Visible {
            self.Offset = CaretX - Visible;
        }
        if CaretX < self.Offset {
            self.Offset = CaretX;
        }
        let Total = Offsets.last().copied().unwrap_or(0.0);
        if Total - self.Offset < Visible {
            self.Offset = (Total - Visible).max(0.0);
        }
        Draw.PushClip(Inner);
        let (Start, End) = self.Selection();
        if Start != End {
            let Left = Inner.Left + Offsets[Start] - self.Offset;
            let Right = Inner.Left + Offsets[End] - self.Offset;
            Draw.Fill(Rect { Left, Top: Inner.Top + 4.0, Right, Bottom: Inner.Bottom - 4.0 }, Colors.Selection.WithAlpha((Colors.Selection.A * 2.2).min(0.6)));
        }
        let TextArea = Rect { Left: Inner.Left - self.Offset, Right: Inner.Left - self.Offset + Total + 50.0, ..Inner };
        Draw.Text(&self.Text, Style::Body, TextArea, Colors.Text, Align::Left);
        Draw.Fill(Rect::New(Inner.Left + CaretX - self.Offset, Inner.Top + 5.0, 1.0, Inner.Height() - 10.0), Colors.Text);
        Draw.PopClip();
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Scroller {
    pub Offset: f32,
    pub Target: f32,
}

impl Scroller {
    pub fn Clamp(&mut self, Maximum: f32) {
        let Limit = Maximum.max(0.0);
        self.Offset = self.Offset.clamp(0.0, Limit);
        self.Target = self.Target.clamp(0.0, Limit);
    }

    pub fn Jump(&mut self, Value: f32) {
        self.Offset = Value;
        self.Target = Value;
    }

    pub fn Step(&mut self) -> bool {
        let Distance = self.Target - self.Offset;
        if Distance.abs() < 0.5 {
            self.Offset = self.Target;
            return false;
        }
        self.Offset += Distance * 0.35;
        true
    }

    pub fn IsAnimating(&self) -> bool {
        (self.Target - self.Offset).abs() >= 0.5
    }
}

pub fn ScrollbarThumb(Track: Rect, Offset: f32, Content: f32, Viewport: f32) -> Option<Rect> {
    if Content <= Viewport + 0.5 || Track.Height() <= 0.0 {
        return None;
    }
    let Height = (Track.Height() * Viewport / Content).max(24.0).min(Track.Height());
    let Range = Track.Height() - Height;
    let Position = if Content - Viewport > 0.0 { Offset / (Content - Viewport) * Range } else { 0.0 };
    Some(Rect::New(Track.Left, Track.Top + Position.clamp(0.0, Range), Track.Width(), Height))
}

pub fn DrawScrollbar(Draw: &Renderer, Look: &Theme, Track: Rect, Offset: f32, Content: f32, Viewport: f32, Active: bool) -> Option<Rect> {
    let Thumb = ScrollbarThumb(Track, Offset, Content, Viewport)?;
    let Width = if Active { Track.Width() - 2.0 } else { 4.0 };
    let Visible = Rect::New(Thumb.Right - Width - 2.0, Thumb.Top + 1.0, Width, Thumb.Height() - 2.0);
    let Color = if Active { Look.Colors.TextMuted.WithAlpha(0.55) } else { Look.Colors.TextMuted.WithAlpha(0.32) };
    Draw.FillRounded(Visible, Width / 2.0, Color);
    Some(Thumb)
}

pub fn OffsetForThumb(Track: Rect, ThumbTop: f32, Content: f32, Viewport: f32) -> f32 {
    let Height = (Track.Height() * Viewport / Content).max(24.0).min(Track.Height());
    let Range = (Track.Height() - Height).max(1.0);
    ((ThumbTop - Track.Top) / Range).clamp(0.0, 1.0) * (Content - Viewport).max(0.0)
}

pub fn Button(Draw: &mut Renderer, Look: &Theme, Area: Rect, Glyph: u16, Hovered: bool, Enabled: bool, Active: bool) {
    if Active {
        Draw.FillRounded(Area, Look.Sizes.Radius, Look.Colors.Pressed);
    } else if Hovered && Enabled {
        Draw.FillRounded(Area, Look.Sizes.Radius, Look.Colors.Hover);
    }
    let Color = if !Enabled {
        Look.Colors.TextFaint.WithAlpha(0.5)
    } else if Active {
        Look.Colors.Accent
    } else {
        Look.Colors.TextMuted
    };
    Draw.Glyph(Glyph, Style::Icon, Area, Color);
}
