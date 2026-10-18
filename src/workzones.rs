//! Work zones (RadioBOSS-style): 1, 2 or 3 tabbed panels around the playlist, plus any number of
//! Additional windows, each with its own tabbed panel. The layout is saved automatically.
use crate::config::data_dir;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Tab {
    Library,
    Scheduler,
    Generator,
    Ads,
    Dsp,
    Streaming,
    AzuraCast,
    Reports,
    History,
    Carts,
    Clock,
    Segment,
    Queue,
    Search,
    Statistics,
    WebPlayout,
    WebClock,
    WebAzura,
    WebCustom,
    FileExplorer,
    Log,
    SongRequests,
    AuxPlaylist,
    TrackInfo,
    /// Web panel number n (0-based; the first four are WebPlayout…WebCustom).
    Web(u16),
}
impl Tab {
    /// Tabs that can go in a panel, in the order of the "+" menu.
    pub const PANEL: [Tab; 13] = [
        Tab::AuxPlaylist,
        Tab::TrackInfo,
        Tab::Carts,
        Tab::Search,
        Tab::Scheduler,
        Tab::FileExplorer,
        Tab::Segment,
        Tab::Statistics,
        Tab::SongRequests,
        Tab::Queue,
        Tab::Log,
        Tab::Library,
        Tab::Clock,
    ];
    /// The tab for web panel `i`.
    pub fn web(i: usize) -> Tab {
        match i {
            0 => Tab::WebPlayout,
            1 => Tab::WebAzura,
            2 => Tab::WebClock,
            3 => Tab::WebCustom,
            n => Tab::Web(n.min(u16::MAX as usize) as u16),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Tab::Library => "Library",
            Tab::Scheduler => "Scheduler",
            Tab::Generator => "Generator",
            Tab::Ads => "Ads",
            Tab::Dsp => "Audio FX",
            Tab::Streaming => "Streams",
            Tab::AzuraCast => "AzuraCast",
            Tab::Reports => "Reports",
            Tab::History => "History",
            Tab::Carts => "Cart wall",
            Tab::Clock => "Clock",
            Tab::Segment => "Notepad",
            Tab::Queue => "Playback Queue",
            Tab::Search => "Search",
            Tab::Statistics => "Statistics",
            Tab::WebPlayout => "Web 1",
            Tab::WebAzura => "Web 2",
            Tab::WebClock => "Web 3",
            Tab::WebCustom => "Web 4",
            Tab::FileExplorer => "File Explorer",
            Tab::Log => "Log",
            Tab::SongRequests => "Song Requests",
            Tab::AuxPlaylist => "AUX Playlist",
            Tab::TrackInfo => "Track Info",
            Tab::Web(_) => "Web",
        }
    }
    /// Tools that live in their own window instead of a panel.
    pub fn is_window_tool(self) -> bool {
        matches!(self, Tab::Generator | Tab::Ads | Tab::Dsp | Tab::Streaming | Tab::AzuraCast | Tab::Reports | Tab::History)
    }
    pub fn is_panel(self) -> bool {
        !self.is_window_tool()
    }
    /// Which web panel this tab is.
    pub fn web_index(self) -> Option<usize> {
        match self {
            Tab::WebPlayout => Some(0),
            Tab::WebAzura => Some(1),
            Tab::WebClock => Some(2),
            Tab::WebCustom => Some(3),
            Tab::Web(n) => Some(n as usize),
            _ => None,
        }
    }
    pub fn is_web(self) -> bool {
        self.web_index().is_some()
    }
}

/// A tabbed panel: in the main window or in an Additional window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Left,
    Top,
    Bottom,
    Aux(u64),
}
impl Pane {
    pub fn label(self) -> String {
        match self {
            Pane::Left => "Left panel".into(),
            Pane::Top => "Right panel".into(),
            Pane::Bottom => "Lower right panel".into(),
            Pane::Aux(id) => format!("Additional window {id}"),
        }
    }
    /// Key of the native window this pane lives in ("" = main window).
    pub fn window_key(self) -> String {
        match self {
            Pane::Aux(id) => aux_title(id),
            _ => String::new(),
        }
    }
    /// Unique key for per-pane state (web view, scroll position…).
    pub fn key(self) -> String {
        match self {
            Pane::Left => "left".into(),
            Pane::Top => "top".into(),
            Pane::Bottom => "bottom".into(),
            Pane::Aux(id) => format!("aux{id}"),
        }
    }
}

/// Title of an Additional window (also used to find its native handle for web views).
pub fn aux_title(id: u64) -> String {
    format!("RadioMaster — Additional {id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct PaneTabs {
    pub tabs: Vec<Tab>,
    pub active: usize,
}
impl PaneTabs {
    fn of(tabs: Vec<Tab>) -> Self {
        PaneTabs { tabs, active: 0 }
    }
    pub fn selected(&self) -> Option<Tab> {
        self.tabs.get(self.active).copied()
    }
    pub fn add(&mut self, tab: Tab) {
        match self.tabs.iter().position(|t| *t == tab) {
            Some(i) => self.active = i,
            None => {
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
            }
        }
    }
    pub fn remove(&mut self, i: usize) -> Option<Tab> {
        if i >= self.tabs.len() {
            return None;
        }
        let t = self.tabs.remove(i);
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        Some(t)
    }
    fn repair(&mut self) {
        self.tabs.retain(|t| t.is_panel());
        self.tabs.dedup();
        self.tabs.truncate(24);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }
}

/// An Additional window: its own tabbed panel on any screen.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct AuxWindow {
    pub id: u64,
    pub pane: PaneTabs,
    /// last position / size (points)
    pub pos: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    /// playlist shown by an AUX Playlist tab in this window
    pub pl_tab: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct WebSite {
    pub name: String,
    pub url: String,
}

/// Start page of the first web panel on a fresh install.
pub const DEFAULT_WEB: &str = "https://radiomanager.dovafm.com";

fn default_web() -> Vec<WebSite> {
    let mut v: Vec<WebSite> = (1..=4).map(|i| WebSite { name: format!("Web {i}"), url: String::new() }).collect();
    v[0].url = DEFAULT_WEB.into();
    v
}

/// Layout from before work zones became panel counts (read once to migrate).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LegacyZone {
    pub left: Vec<Tab>,
    pub top: Vec<Tab>,
    pub bottom: Vec<Tab>,
    pub left_active: usize,
    pub top_active: usize,
    pub bottom_active: usize,
    pub show_left: bool,
    pub show_right: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkzoneSet {
    /// layout format version (missing in files written before work zones became panel counts)
    #[serde(default)]
    pub v: u32,
    /// work zone 1, 2 or 3: number of panels (1 = left; 2 = left + right; 3 = left + right + lower right)
    pub panels: u8,
    pub show_left: bool,
    pub left: PaneTabs,
    pub top: PaneTabs,
    pub bottom: PaneTabs,
    pub split: f32,
    pub aux: Vec<AuxWindow>,
    pub next_aux: u64,
    pub dark: bool,
    /// the playlist scrolls along with the on-air track
    pub follow: bool,
    pub scale: f32,
    /// Four user-defined web panels (name + address). Nothing is built in.
    pub web: Vec<WebSite>,
    // ---- legacy fields, read once to migrate older layouts
    #[serde(skip_serializing)]
    pub zones: Vec<LegacyZone>,
    #[serde(skip_serializing)]
    pub active: usize,
    #[serde(skip_serializing)]
    pub custom_url: String,
    #[serde(skip_serializing)]
    pub azura_url: String,
    #[serde(skip_serializing)]
    pub clock_url: String,
}

impl Default for WorkzoneSet {
    fn default() -> Self {
        Self {
            v: 3,
            panels: 3,
            show_left: true,
            left: PaneTabs::of(vec![Tab::Library, Tab::FileExplorer, Tab::Search, Tab::Scheduler, Tab::Log]),
            top: PaneTabs::of(vec![Tab::WebPlayout, Tab::Segment, Tab::Carts, Tab::Queue]),
            bottom: PaneTabs::of(vec![Tab::Statistics, Tab::Clock, Tab::Search]),
            split: 0.5,
            aux: vec![],
            next_aux: 1,
            dark: false,
            follow: true,
            scale: 1.0,
            web: default_web(),
            zones: vec![],
            active: 0,
            custom_url: String::new(),
            azura_url: String::new(),
            clock_url: String::new(),
        }
    }
}

impl WorkzoneSet {
    fn path() -> std::path::PathBuf {
        data_dir().join("workzones.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let backup = path.with_extension("json.bak");
        let s: Option<Self> =
            std::fs::read(&path).ok().or_else(|| std::fs::read(backup).ok()).and_then(|v| serde_json::from_slice(&v).ok());
        let mut s = s.unwrap_or_default();
        s.migrate();
        s
    }

    /// Bring older layouts up to date and repair anything out of range.
    pub fn migrate(&mut self) {
        if self.v < 2 {
            // named work zones (On Air / Production / …): keep the tabs of the one that was in use
            if let Some(z) = self.zones.get(self.active.min(self.zones.len().saturating_sub(1))).cloned() {
                self.left = PaneTabs { tabs: z.left, active: z.left_active };
                self.top = PaneTabs { tabs: z.top, active: z.top_active };
                self.bottom = PaneTabs { tabs: z.bottom, active: z.bottom_active };
                self.show_left = z.show_left || !z.show_right;
                self.panels = if z.show_right { 3 } else { 1 };
            } else {
                let d = Self::default();
                self.left = d.left;
                self.top = d.top;
                self.bottom = d.bottom;
                self.panels = 3;
                self.show_left = true;
            }
            // the old fixed web panels become user-named Web 2–4
            for (i, u) in [(1, &self.azura_url), (2, &self.clock_url), (3, &self.custom_url)] {
                if !u.trim().is_empty() {
                    if self.web.len() <= i {
                        self.web.resize(4, WebSite::default());
                    }
                    self.web[i].url = u.trim().to_string();
                }
            }
            self.v = 2;
        }
        if self.v < 3 {
            // the old start pages (listen.dovafm.com, clocktab) make way for Radio Manager
            let old = |u: &str| {
                let u = u.trim().to_ascii_lowercase();
                u.contains("listen.dovafm.com") || u.contains("clocktab.com")
            };
            let mut first = true;
            for w in self.web.iter_mut().filter(|w| old(&w.url)) {
                w.url = if first { DEFAULT_WEB.into() } else { String::new() };
                if first && (w.name.to_ascii_lowercase().contains("clock") || w.name.to_ascii_lowercase().contains("listen")) {
                    w.name = "Radio Manager".into();
                }
                first = false;
            }
            self.v = 3;
        }
        self.zones.clear();
        if self.web.is_empty() {
            self.web = default_web();
            self.web[1].url = self.azura_url.trim().to_string();
            self.web[2].url = self.clock_url.trim().to_string();
            self.web[3].url = self.custom_url.trim().to_string();
        }
        if self.web.len() < 4 {
            self.web.resize(4, WebSite::default());
        }
        self.web.truncate(64);
        for (i, w) in self.web.iter_mut().enumerate() {
            if w.name.trim().is_empty() {
                w.name = format!("Web {}", i + 1);
            }
        }
        self.panels = self.panels.clamp(1, 3);
        self.scale = if self.scale.is_finite() { self.scale.clamp(0.8, 1.7) } else { 1.0 };
        self.split = if self.split.is_finite() { self.split.clamp(0.2, 0.8) } else { 0.5 };
        for p in [&mut self.left, &mut self.top, &mut self.bottom] {
            p.repair();
        }
        self.aux.truncate(32);
        for a in self.aux.iter_mut() {
            a.pane.repair();
        }
        let max_id = self.aux.iter().map(|a| a.id).max().unwrap_or(0);
        self.next_aux = self.next_aux.max(max_id + 1).max(1);
    }

    pub fn pane(&self, p: Pane) -> Option<&PaneTabs> {
        match p {
            Pane::Left => Some(&self.left),
            Pane::Top => Some(&self.top),
            Pane::Bottom => Some(&self.bottom),
            Pane::Aux(id) => self.aux.iter().find(|a| a.id == id).map(|a| &a.pane),
        }
    }

    pub fn pane_mut(&mut self, p: Pane) -> Option<&mut PaneTabs> {
        match p {
            Pane::Left => Some(&mut self.left),
            Pane::Top => Some(&mut self.top),
            Pane::Bottom => Some(&mut self.bottom),
            Pane::Aux(id) => self.aux.iter_mut().find(|a| a.id == id).map(|a| &mut a.pane),
        }
    }

    /// Panes the user can currently see (for "Move to…").
    pub fn visible_panes(&self) -> Vec<Pane> {
        let mut v = vec![];
        if self.show_left {
            v.push(Pane::Left);
        }
        if self.panels >= 2 {
            v.push(Pane::Top);
        }
        if self.panels >= 3 {
            v.push(Pane::Bottom);
        }
        v.extend(self.aux.iter().map(|a| Pane::Aux(a.id)));
        v
    }

    pub fn move_tab(&mut self, from: Pane, i: usize, to: Pane) {
        if from == to {
            return;
        }
        let Some(tab) = self.pane_mut(from).and_then(|p| p.remove(i)) else { return };
        if let Some(p) = self.pane_mut(to) {
            p.add(tab);
        }
    }

    /// Open a new Additional window (with one tab to start with). Returns its id.
    pub fn new_aux(&mut self, first: Tab) -> u64 {
        let id = self.next_aux.max(1);
        self.next_aux = id + 1;
        self.aux.push(AuxWindow { id, pane: PaneTabs::of(vec![first]), ..Default::default() });
        id
    }

    pub fn close_aux(&mut self, id: u64) {
        self.aux.retain(|a| a.id != id);
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let target = Self::path();
        // Write-then-rename avoids a truncated configuration after a crash.
        let temp = target.with_extension("json.tmp");
        std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        let backup = target.with_extension("json.bak");
        if target.exists() {
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(&target, &backup).map_err(|e| e.to_string())?;
        }
        match std::fs::rename(&temp, &target) {
            Ok(()) => {
                let _ = std::fs::remove_file(backup);
                Ok(())
            }
            Err(e) => {
                if backup.exists() {
                    let _ = std::fs::rename(backup, &target);
                }
                Err(e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_named_workzones_migrate_to_panels() {
        // a layout saved by the previous version: four named work zones, "Production" in use
        let old = r#"{"zones":[{"name":"On Air","left":["Library"],"top":["Carts"],"bottom":["Clock"],"show_left":true,"show_right":true},
                                 {"name":"Production","left":["Search","Generator"],"top":["Segment"],"bottom":["Queue"],"show_left":true,"show_right":true}],
                      "active":1,"dark":true,"scale":1.2,"azura_url":"https://radio.example.com/login"}"#;
        let mut s: WorkzoneSet = serde_json::from_str(old).unwrap();
        s.migrate();
        assert_eq!(s.left.tabs, vec![Tab::Search], "tools that are windows now are dropped from panels");
        assert_eq!(s.top.tabs, vec![Tab::Segment]);
        assert_eq!(s.panels, 3);
        assert!(s.dark);
        assert_eq!(s.web[1].url, "https://radio.example.com/login");
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("Production"), "named zones are gone");
    }

    #[test]
    fn additional_windows_and_moving_tabs() {
        let mut s = WorkzoneSet::default();
        let a = s.new_aux(Tab::Carts);
        let b = s.new_aux(Tab::Log);
        assert_ne!(a, b);
        s.move_tab(Pane::Top, 0, Pane::Aux(b));
        assert_eq!(s.pane(Pane::Aux(b)).unwrap().tabs, vec![Tab::Log, Tab::WebPlayout]);
        assert_eq!(s.pane(Pane::Aux(b)).unwrap().selected(), Some(Tab::WebPlayout));
        s.close_aux(a);
        let mut back: WorkzoneSet = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        back.migrate();
        assert_eq!(back.aux.len(), 1);
        assert!(back.next_aux > b, "ids are never reused");
    }

    #[test]
    fn remove_tab_repairs_selection() {
        let mut p = PaneTabs::of(vec![Tab::Log, Tab::Clock, Tab::Carts]);
        p.active = 2;
        p.remove(2);
        assert_eq!(p.active, 1);
        p.remove(0);
        assert_eq!(p.selected(), Some(Tab::Clock));
    }
}
