use crate::dsp::DspConfig;
use crate::scheduler::Event;
use chrono::{DateTime, Datelike, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Settings, library cache, playlists and history live here (per Windows user).
pub fn data_dir() -> PathBuf {
    static MIGRATED: std::sync::Once = std::sync::Once::new();
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("RadioMaster");
    MIGRATED.call_once(|| {
        // first run after the rename: bring the old OpenBoss settings across (the old folder is left as a backup)
        let old = base.join("OpenBoss");
        if !d.exists() && old.is_dir() {
            let _ = copy_dir(&old, &d);
        }
    });
    let _ = std::fs::create_dir_all(&d);
    d
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

/// Spoken / recorded time announcements.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct AnnounceCfg {
    /// e.g. "It's {h12}:{mm} {ampm}" ({h24}, {h12}, {mm}, {ampm}, {time})
    pub template: String,
    /// Folder of recordings H00…H23 / M00…M59; empty = use the computer's speech voice.
    pub folder: String,
    /// Speech voice name (empty = default voice).
    pub voice: String,
    /// Speaking speed, -10 … 10.
    pub rate: i32,
}
impl Default for AnnounceCfg {
    fn default() -> Self {
        Self { template: "It's {h12}:{mm} {ampm}".into(), folder: String::new(), voice: String::new(), rate: 0 }
    }
}

/// Track list repeat protection.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(default)]
pub struct RepeatCfg {
    /// don't play the same track again within this many minutes
    pub track_mins: u32,
    /// don't play the same artist again within this many minutes
    pub artist_mins: u32,
}
impl Default for RepeatCfg {
    fn default() -> Self {
        Self { track_mins: 180, artist_mins: 60 }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServerKind {
    Icecast,
    Shoutcast,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct EncoderConfig {
    pub name: String,
    pub start_with_app: bool,
    pub kind: ServerKind,
    pub host: String,
    pub port: u16,
    pub mount: String,
    pub username: String,
    pub password: String,
    pub bitrate_kbps: u32,
    pub stream_name: String,
    pub description: String,
    pub genre: String,
    pub url: String,
    pub public: bool,
    pub send_metadata: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            name: "Encoder".into(),
            start_with_app: false,
            kind: ServerKind::Icecast,
            host: "localhost".into(),
            port: 8000,
            mount: "/radio.mp3".into(),
            username: "source".into(),
            password: String::new(),
            bitrate_kbps: 128,
            stream_name: "RadioMaster".into(),
            description: String::new(),
            genre: "Various".into(),
            url: String::new(),
            public: false,
            send_metadata: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct AzuraConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    /// Station short name or numeric ID.
    pub station: String,
    pub poll_secs: u32,
    /// Remote folder used by "Upload to AzuraCast".
    pub upload_dir: String,
}

impl Default for AzuraConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            station: String::new(),
            poll_secs: 5,
            upload_dir: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CartPad {
    pub label: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Rotation {
    pub name: String,
    pub enabled: bool,
    /// adverts or jingles (only changes the label and the log)
    pub adverts: bool,
    pub folder: String,
    /// chosen files; when not empty these are used instead of the folder
    pub files: Vec<String>,
    pub every_n: u32,
    /// how many to play each time
    pub count: u32,
    pub pick: RotPick,
    /// position for `RotPick::InOrder`
    pub next: usize,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum RotPick {
    /// different random ones each time
    #[default]
    Random,
    /// one after another through the list, starting again at the end
    InOrder,
    /// every file, each time
    All,
}

impl Default for Rotation {
    fn default() -> Self {
        Self {
            name: "Jingles".into(),
            enabled: false,
            adverts: false,
            folder: String::new(),
            files: vec![],
            every_n: 4,
            count: 1,
            pick: RotPick::Random,
            next: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Category {
    pub name: String,
    pub folder: String,
    pub genre: String,
    pub min_secs: f32,
    /// 0 = no maximum
    pub max_secs: f32,
}
impl Default for Category {
    fn default() -> Self {
        Self { name: "Music".into(), folder: String::new(), genre: String::new(), min_secs: 0.0, max_secs: 0.0 }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct PatternItem {
    pub category: String,
    pub count: u32,
}
impl Default for PatternItem {
    fn default() -> Self {
        Self { category: "Music".into(), count: 1 }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Marker {
    pub category: String,
    pub every_mins: u32,
}
impl Default for Marker {
    fn default() -> Self {
        Self { category: "Music".into(), every_mins: 60 }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub minutes: u32,
    pub pattern: Vec<PatternItem>,
    pub markers: Vec<Marker>,
    pub artist_gap: u32,
    /// put an hour marker at the top of every hour
    pub hour_markers: bool,
}
impl Default for Template {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            minutes: 120,
            pattern: vec![PatternItem::default()],
            markers: vec![],
            artist_gap: 5,
            hour_markers: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Ad {
    pub name: String,
    pub enabled: bool,
    pub path: String,
    /// YYYY-MM-DD, empty = no limit
    pub start_date: String,
    pub end_date: String,
    pub days: [bool; 7],
}
impl Default for Ad {
    fn default() -> Self {
        Self {
            name: "New ad".into(),
            enabled: true,
            path: String::new(),
            start_date: String::new(),
            end_date: String::new(),
            days: [true; 7],
        }
    }
}
impl Ad {
    pub fn active(&self, now: &DateTime<Local>) -> bool {
        let today = now.format("%Y-%m-%d").to_string();
        self.enabled
            && (self.start_date.is_empty() || today >= self.start_date)
            && (self.end_date.is_empty() || today <= self.end_date)
            && self.days[now.weekday().num_days_from_monday() as usize]
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AdBreak {
    pub name: String,
    pub enabled: bool,
    pub hour: u32,
    pub minute: u32,
    pub days: [bool; 7],
    /// 0 = all active ads
    pub max_ads: u32,
}
impl Default for AdBreak {
    fn default() -> Self {
        Self { name: "Break".into(), enabled: true, hour: 12, minute: 0, days: [true; 7], max_ads: 0 }
    }
}
impl AdBreak {
    pub fn due(&self, now: &DateTime<Local>) -> bool {
        use chrono::Timelike;
        self.enabled && self.hour == now.hour() && self.minute == now.minute() && self.days[now.weekday().num_days_from_monday() as usize]
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ExportCfg {
    pub text_file: String,
    pub xml_file: String,
    /// URL with {artist} and {title} placeholders
    pub http_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct RemoteCfg {
    pub enabled: bool,
    pub port: u16,
    pub password: String,
}
impl Default for RemoteCfg {
    fn default() -> Self {
        Self { enabled: false, port: 9258, password: String::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct OutputSel {
    pub name: String,
    pub enabled: bool,
    pub gain: f32,
}
impl Default for OutputSel {
    fn default() -> Self {
        Self { name: String::new(), enabled: false, gain: 1.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AudioDevices {
    /// Order matters: the first enabled device that opens is the primary output.
    pub outputs: Vec<OutputSel>,
    /// Empty = system default input.
    pub input: String,
    /// Where the fade editor's preview plays (empty = system default). Never goes to the stream.
    pub preview: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub audio_devices: AudioDevices,
    pub announce: AnnounceCfg,
    pub repeat: RepeatCfg,
    pub dsp: DspConfig,
    pub categories: Vec<Category>,
    pub templates: Vec<Template>,
    pub auto_enabled: bool,
    pub auto_template: String,
    pub auto_min_mins: u32,
    pub ads: Vec<Ad>,
    pub ad_breaks: Vec<AdBreak>,
    pub teaser_secs: f32,
    pub export: ExportCfg,
    pub remote: RemoteCfg,
    pub mic_gain: f32,
    pub mic_duck: f32,
    pub library_folders: Vec<String>,
    pub crossfade_secs: f32,
    pub fade: crate::fade::SmartFadeConfig,
    pub volume: f32,
    pub loop_playlist: bool,
    pub random_fill: bool,
    pub encoders: Vec<EncoderConfig>,
    pub azuracast: AzuraConfig,
    pub carts: Vec<CartPad>,
    pub events: Vec<Event>,
    pub rotations: Vec<Rotation>,
    /// Look for a newer RadioMaster release on GitHub at start-up.
    pub check_updates: bool,
    /// automatic "next hour" lines in the playlist
    pub hour_lines: bool,
    /// saved EQ curves (name, gains)
    pub eq_presets: Vec<(String, [f32; 12])>,
    /// plugin folders that are switched off
    pub plugins_off: Vec<String>,
    /// interface language (a file in the lang folder); empty = English
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check_updates: true,
            hour_lines: true,
            eq_presets: vec![],
            plugins_off: vec![],
            language: String::new(),
            audio_devices: AudioDevices::default(),
            announce: AnnounceCfg::default(),
            repeat: RepeatCfg::default(),
            dsp: DspConfig::default(),
            categories: vec![Category::default()],
            templates: vec![Template::default()],
            auto_enabled: false,
            auto_template: "Default".into(),
            auto_min_mins: 30,
            ads: vec![],
            ad_breaks: vec![],
            teaser_secs: 0.0,
            export: ExportCfg::default(),
            remote: RemoteCfg::default(),
            mic_gain: 1.0,
            mic_duck: 0.35,
            library_folders: vec![],
            crossfade_secs: 3.0,
            fade: crate::fade::SmartFadeConfig::default(),
            volume: 0.9,
            loop_playlist: false,
            random_fill: false,
            encoders: vec![EncoderConfig::default()],
            azuracast: AzuraConfig::default(),
            carts: vec![CartPad::default(); 24],
            events: vec![],
            rotations: vec![Rotation::default()],
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        data_dir().join("config.json")
    }
    pub fn load() -> Self {
        let mut c: Config = std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        c.carts.resize(24, CartPad::default());
        // the remote API's default port moved from 9000 to 9258; follow it unless someone turned it on
        if !c.remote.enabled && c.remote.port == 9000 {
            c.remote.port = 9258;
        }
        c
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}
