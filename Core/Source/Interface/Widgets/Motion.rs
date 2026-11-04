use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Channel {
    Overlay,
    Content(usize),
    Hover(usize),
    Sidebar,
    Inspector,
    Notice,
    Section(u8),
    Tab(usize),
}

struct Track {
    Key: u64,
    Started: Instant,
    Length: Duration,
}

pub struct Motion {
    pub Enabled: bool,
    Tracks: HashMap<Channel, Track>,
    Now: Instant,
}

pub fn EaseOut(Value: f32) -> f32 {
    let Inverse = 1.0 - Value.clamp(0.0, 1.0);
    1.0 - Inverse * Inverse * Inverse
}

impl Motion {
    pub fn New(Enabled: bool) -> Motion {
        Motion { Enabled, Tracks: HashMap::new(), Now: Instant::now() }
    }

    pub fn BeginFrame(&mut self) {
        self.Now = Instant::now();
    }

    pub fn Watch(&mut self, Item: Channel, Key: u64, Milliseconds: u64, AnimateFirst: bool) -> f32 {
        let Now = self.Now;
        let Length = Duration::from_millis(Milliseconds);
        match self.Tracks.get_mut(&Item) {
            Some(Existing) if Existing.Key == Key => {}
            Some(Existing) => {
                Existing.Key = Key;
                Existing.Started = Now;
                Existing.Length = Length;
            }
            None => {
                let Started = if AnimateFirst { Now } else { Now.checked_sub(Length).unwrap_or(Now) };
                self.Tracks.insert(Item, Track { Key, Started, Length });
            }
        }
        if !self.Enabled {
            return 1.0;
        }
        let Track = &self.Tracks[&Item];
        if Track.Length.is_zero() {
            return 1.0;
        }
        EaseOut(Now.duration_since(Track.Started).as_secs_f32() / Track.Length.as_secs_f32())
    }

    pub fn Animating(&self) -> bool {
        let Now = Instant::now();
        self.Enabled && self.Tracks.values().any(|Track| Now.saturating_duration_since(Track.Started) < Track.Length)
    }
}
