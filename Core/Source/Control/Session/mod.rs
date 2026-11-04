pub mod Undo;

use crate::FileSystem::{Location, SamePath};
use crate::Platform::Placement;
use crate::Services::Config::Ini;
use std::path::{Path, PathBuf};

pub const RecentLimit: usize = 24;

pub struct Session {
    pub Favorites: Vec<PathBuf>,
    pub Recent: Vec<PathBuf>,
    pub Tabs: [Vec<Location>; 2],
    pub ActiveTabs: [usize; 2],
    pub ActivePane: usize,
    pub Window: Option<Placement>,
}

fn LocationText(Item: &Location) -> Option<String> {
    match Item {
        Location::Computer => Some("::Computer".to_string()),
        Location::Directory(Path) => Some(Path.to_string_lossy().into_owned()),
        Location::Search { Root, .. } => Some(Root.to_string_lossy().into_owned()),
        Location::Results { .. } => None,
    }
}

fn ParseLocation(Text: &str) -> Option<Location> {
    if Text == "::Computer" {
        return Some(Location::Computer);
    }
    if Text.is_empty() {
        return None;
    }
    Some(Location::Directory(PathBuf::from(Text)))
}

impl Session {
    pub fn Load(Item: &Path) -> Session {
        let Data = Ini::Load(Item);
        let Fresh = Data.is_none();
        let Data = Data.unwrap_or_default();
        let List = |Section: &str| -> Vec<PathBuf> {
            Data.Section(Section).unwrap_or(&[]).iter().filter(|(_, Value)| !Value.is_empty()).map(|(_, Value)| PathBuf::from(Value)).collect()
        };
        let mut Favorites = List("Favorites");
        if Fresh {
            Favorites = crate::Platform::Shell::KnownFolders().into_iter().filter(|(Name, _)| Name != "Music" && Name != "Videos").map(|(_, Path)| Path).collect();
        }
        let Recent = List("Recent").into_iter().take(RecentLimit).collect();
        let Tabs = [0usize, 1].map(|Pane| {
            Data.Section(&format!("Pane{Pane}")).unwrap_or(&[]).iter().filter(|(Key, _)| Key.starts_with("Tab")).filter_map(|(_, Value)| ParseLocation(Value)).collect::<Vec<_>>()
        });
        let ActiveTabs = [0usize, 1].map(|Pane| Data.Get(&format!("Pane{Pane}"), "Active").and_then(|Value| Value.parse().ok()).unwrap_or(0));
        let ActivePane = Data.Get("Window", "ActivePane").and_then(|Value| Value.parse().ok()).unwrap_or(0usize).min(1);
        let Number = |Key: &str| Data.Get("Window", Key).and_then(|Value| Value.parse::<i32>().ok());
        let Window = match (Number("Left"), Number("Top"), Number("Width"), Number("Height")) {
            (Some(Left), Some(Top), Some(Width), Some(Height)) => Some(Placement { Left, Top, Width, Height, Maximized: Data.Bool("Window", "Maximized", false) }),
            _ => None,
        };
        Session { Favorites, Recent, Tabs, ActiveTabs, ActivePane, Window }
    }

    pub fn Save(&self, Item: &Path) -> std::io::Result<()> {
        let mut Data = Ini::default();
        for (Index, Favorite) in self.Favorites.iter().enumerate() {
            Data.Push("Favorites", &format!("Item{Index}"), &Favorite.to_string_lossy());
        }
        for (Index, Recent) in self.Recent.iter().enumerate() {
            Data.Push("Recent", &format!("Item{Index}"), &Recent.to_string_lossy());
        }
        for Pane in 0..2 {
            let Section = format!("Pane{Pane}");
            Data.Set(&Section, "Active", &self.ActiveTabs[Pane].to_string());
            for (Index, Tab) in self.Tabs[Pane].iter().filter_map(LocationText).enumerate() {
                Data.Push(&Section, &format!("Tab{Index}"), &Tab);
            }
        }
        Data.Set("Window", "ActivePane", &self.ActivePane.to_string());
        if let Some(Place) = &self.Window {
            Data.Set("Window", "Left", &Place.Left.to_string());
            Data.Set("Window", "Top", &Place.Top.to_string());
            Data.Set("Window", "Width", &Place.Width.to_string());
            Data.Set("Window", "Height", &Place.Height.to_string());
            Data.Set("Window", "Maximized", if Place.Maximized { "true" } else { "false" });
        }
        Data.Save(Item)
    }

    pub fn AddRecent(&mut self, Item: &Path) {
        self.Recent.retain(|Existing| !SamePath(Existing, Item));
        self.Recent.insert(0, Item.to_path_buf());
        self.Recent.truncate(RecentLimit);
    }

    pub fn IsFavorite(&self, Item: &Path) -> bool {
        self.Favorites.iter().any(|Existing| SamePath(Existing, Item))
    }

    pub fn Pin(&mut self, Item: &Path) -> bool {
        if self.IsFavorite(Item) {
            return false;
        }
        self.Favorites.push(Item.to_path_buf());
        true
    }

    pub fn Unpin(&mut self, Item: &Path) -> bool {
        let Before = self.Favorites.len();
        self.Favorites.retain(|Existing| !SamePath(Existing, Item));
        Before != self.Favorites.len()
    }

    pub fn MoveFavorite(&mut self, From: usize, To: usize) {
        if From < self.Favorites.len() && To < self.Favorites.len() && From != To {
            let Item = self.Favorites.remove(From);
            self.Favorites.insert(To, Item);
        }
    }
}
