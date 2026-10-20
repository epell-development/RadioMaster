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

/// What the music does while the microphone is on.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MicMode {
    /// lower it to the duck level
    #[default]
    Duck,
    /// silence it (it keeps running)
    Mute,
    /// pause it, and carry on when the mic goes off
    Pause,
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
    /// what the stream is encoded as
    pub format: StreamFormat,
    /// `StreamFormat::Custom`: command line that reads 16-bit 44.1 kHz stereo PCM on stdin and writes
    /// the encoded stream to stdout ({bitrate} = kbps)
    pub command: String,
    /// `StreamFormat::Custom`: the stream's MIME type, e.g. audio/aac
    pub content_type: String,
}

/// Stream encodings. MP3 is built in; the others are made by ffmpeg (or your own program).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum StreamFormat {
    #[default]
    Mp3,
    Aac,
    Opus,
    Vorbis,
    Flac,
    /// any program you like
    Custom,
}

impl StreamFormat {
    pub const ALL: [StreamFormat; 6] =
        [StreamFormat::Mp3, StreamFormat::Aac, StreamFormat::Opus, StreamFormat::Vorbis, StreamFormat::Flac, StreamFormat::Custom];
    pub fn label(self) -> &'static str {
        match self {
            StreamFormat::Mp3 => "MP3 (built in)",
            StreamFormat::Aac => "AAC (ffmpeg)",
            StreamFormat::Opus => "Opus in Ogg (ffmpeg)",
            StreamFormat::Vorbis => "Ogg Vorbis (ffmpeg)",
            StreamFormat::Flac => "FLAC in Ogg, lossless (ffmpeg)",
            StreamFormat::Custom => "Custom encoder (your command)",
        }
    }
    /// MIME type sent to the server
    pub fn content_type(self, custom: &str) -> String {
        match self {
            StreamFormat::Mp3 => "audio/mpeg".into(),
            StreamFormat::Aac => "audio/aac".into(),
            StreamFormat::Opus | StreamFormat::Vorbis | StreamFormat::Flac => "audio/ogg".into(),
            StreamFormat::Custom => {
                if custom.trim().is_empty() {
                    "audio/mpeg".into()
                } else {
                    custom.trim().to_string()
                }
            }
        }
    }
    /// usual mount point ending
    pub fn extension(self) -> &'static str {
        match self {
            StreamFormat::Mp3 => ".mp3",
            StreamFormat::Aac => ".aac",
            StreamFormat::Opus => ".opus",
            StreamFormat::Vorbis | StreamFormat::Flac => ".ogg",
            StreamFormat::Custom => "",
        }
    }
    /// ffmpeg arguments after the input (`-i -`) for the built-in presets
    pub fn ffmpeg_args(self, kbps: u32) -> Vec<String> {
        let b = format!("{kbps}k");
        let v: Vec<&str> = match self {
            StreamFormat::Aac => vec!["-c:a", "aac", "-b:a", &b, "-f", "adts"],
            StreamFormat::Opus => vec!["-c:a", "libopus", "-b:a", &b, "-application", "audio", "-f", "ogg"],
            StreamFormat::Vorbis => vec!["-c:a", "libvorbis", "-b:a", &b, "-f", "ogg"],
            StreamFormat::Flac => vec!["-c:a", "flac", "-f", "ogg"],
            StreamFormat::Mp3 | StreamFormat::Custom => vec![],
        };
        v.into_iter().map(String::from).collect()
    }
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
            format: StreamFormat::Mp3,
            command: String::new(),
            content_type: String::new(),
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

/// What sets an advert / jingle rule off.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Trigger {
    /// after every N songs
    #[default]
    Songs,
    /// every N minutes (or hours)
    Minutes,
    /// at set times of day
    Times,
}

/// One advert or jingle rule: what plays, how often, and when the rule is in force.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AdRule {
    /// stays the same when rules are reordered (keeps the song count with the rule)
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    /// adverts or jingles (label, colour, and how plays are reported)
    pub adverts: bool,
    pub trigger: Trigger,
    pub every_songs: u32,
    pub every_mins: u32,
    /// every N minutes counted from the top of the hour (:00, :30…) instead of from the last time
    pub on_clock: bool,
    /// (hour, minute) for `Trigger::Times`
    pub times: Vec<(u32, u32)>,
    /// adverts: take them from the Adverts list (each with its own dates and days)
    pub use_ad_list: bool,
    pub folder: String,
    /// chosen files; when not empty these are used instead of the folder
    pub files: Vec<String>,
    /// how many to play each time
    pub count: u32,
    pub pick: RotPick,
    /// position for `RotPick::InOrder`
    pub next: usize,
    /// in force from / to (YYYY-MM-DD, empty = no limit)
    pub from_date: String,
    pub to_date: String,
    /// Monday … Sunday
    pub days: [bool; 7],
    /// in force between these hours (from == to: all day; 22 → 6 runs over midnight)
    pub from_hour: u32,
    pub to_hour: u32,
}

impl Default for AdRule {
    fn default() -> Self {
        Self {
            id: 0,
            name: "Jingles".into(),
            enabled: false,
            adverts: false,
            trigger: Trigger::Songs,
            every_songs: 4,
            every_mins: 30,
            on_clock: true,
            times: vec![(12, 0)],
            use_ad_list: false,
            folder: String::new(),
            files: vec![],
            count: 1,
            pick: RotPick::Random,
            next: 0,
            from_date: String::new(),
            to_date: String::new(),
            days: [true; 7],
            from_hour: 0,
            to_hour: 0,
        }
    }
}

impl AdRule {
    /// Whether the rule is in force at `now` (dates, days and hours).
    pub fn in_force(&self, now: &DateTime<Local>) -> bool {
        use chrono::Timelike;
        let today = now.format("%Y-%m-%d").to_string();
        let h = now.hour();
        let hours_ok = if self.from_hour == self.to_hour {
            true
        } else if self.from_hour < self.to_hour {
            h >= self.from_hour && h < self.to_hour
        } else {
            h >= self.from_hour || h < self.to_hour
        };
        self.enabled
            && (self.from_date.trim().is_empty() || today.as_str() >= self.from_date.trim())
            && (self.to_date.trim().is_empty() || today.as_str() <= self.to_date.trim())
            && self.days[now.weekday().num_days_from_monday() as usize]
            && hours_ok
    }

    /// Whether a timed rule goes off in this minute.
    pub fn due_now(&self, now: &DateTime<Local>, last: Option<DateTime<Local>>) -> bool {
        use chrono::Timelike;
        if !self.in_force(now) {
            return false;
        }
        match self.trigger {
            Trigger::Songs => false,
            Trigger::Times => self.times.iter().any(|&(h, m)| h == now.hour() && m == now.minute()),
            Trigger::Minutes => {
                let n = self.every_mins.max(1) as i64;
                if self.on_clock {
                    (now.hour() as i64 * 60 + now.minute() as i64) % n == 0
                } else {
                    last.is_none_or(|l| (*now - l).num_minutes() >= n)
                }
            }
        }
    }

    /// One line saying what the rule does, e.g. "1 random jingle after every 4 songs".
    pub fn summary(&self) -> String {
        let what = if self.adverts { "advert" } else { "jingle" };
        let n = self.count.max(1);
        let pick = match self.pick {
            RotPick::All => format!("all the {what}s"),
            RotPick::Random => format!("{n} random {what}{}", if n == 1 { "" } else { "s" }),
            RotPick::InOrder => format!("{n} {what}{} in turn", if n == 1 { "" } else { "s" }),
        };
        let when = match self.trigger {
            Trigger::Songs => format!("after every {} song{}", self.every_songs.max(1), if self.every_songs == 1 { "" } else { "s" }),
            Trigger::Minutes => {
                let m = self.every_mins.max(1);
                let every = if m % 60 == 0 { format!("{} hour{}", m / 60, if m == 60 { "" } else { "s" }) } else { format!("{m} minutes") };
                format!("every {every}{}", if self.on_clock { " (on the clock)" } else { "" })
            }
            Trigger::Times => {
                let t: Vec<String> = self.times.iter().map(|(h, m)| format!("{h:02}:{m:02}")).collect();
                format!("at {}", if t.is_empty() { "—".to_string() } else { t.join(", ") })
            }
        };
        let mut limits = vec![];
        if !self.days.iter().all(|d| *d) {
            const D: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
            limits.push(D.iter().zip(self.days).filter(|(_, on)| *on).map(|(d, _)| *d).collect::<Vec<_>>().join(" "));
        }
        if self.from_hour != self.to_hour {
            limits.push(format!("{:02}:00–{:02}:00", self.from_hour, self.to_hour));
        }
        if !self.from_date.is_empty() || !self.to_date.is_empty() {
            limits.push(format!(
                "{} → {}",
                if self.from_date.is_empty() { "…" } else { &self.from_date },
                if self.to_date.is_empty() { "…" } else { &self.to_date }
            ));
        }
        let limits = if limits.is_empty() { String::new() } else { format!(" · {}", limits.join(" · ")) };
        format!("{pick} {when}{limits}")
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
    /// a category name, or "rule:<id>" for the files of an advert / jingle rule
    pub category: String,
    pub every_mins: u32,
    /// how many to insert each time
    pub count: u32,
}
impl Default for Marker {
    fn default() -> Self {
        Self { category: "Music".into(), every_mins: 60, count: 1 }
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
    /// what happens to the music while the mic is on
    pub mic_mode: MicMode,
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
    /// (before v0.2.5; moved into `ad_rules`)
    pub rotations: Vec<Rotation>,
    /// advert and jingle rules (every N songs / minutes / set times)
    pub ad_rules: Vec<AdRule>,
    /// 0 = made before `ad_rules` (migrated on load)
    #[serde(default)]
    pub ad_rules_v: u32,
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
    /// ffmpeg program for AAC / Opus / Vorbis / FLAC streams (empty = look for it)
    pub ffmpeg_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            check_updates: true,
            hour_lines: true,
            eq_presets: vec![],
            plugins_off: vec![],
            language: String::new(),
            ffmpeg_path: String::new(),
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
            mic_mode: MicMode::Duck,
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
            rotations: vec![],
            ad_rules: vec![AdRule { id: 1, ..Default::default() }],
            ad_rules_v: 1,
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
        c.migrate_ad_rules();
        c
    }

    /// v0.2.5: song-count rules and commercial breaks became one list of advert / jingle rules.
    pub fn migrate_ad_rules(&mut self) {
        if self.ad_rules_v >= 1 {
            return;
        }
        let mut rules: Vec<AdRule> = std::mem::take(&mut self.rotations)
            .into_iter()
            .map(|r| AdRule {
                name: r.name,
                enabled: r.enabled,
                adverts: r.adverts,
                trigger: Trigger::Songs,
                every_songs: r.every_n,
                folder: r.folder,
                files: r.files,
                count: r.count,
                pick: r.pick,
                next: r.next,
                ..Default::default()
            })
            .collect();
        for b in std::mem::take(&mut self.ad_breaks) {
            rules.push(AdRule {
                name: b.name,
                enabled: b.enabled,
                adverts: true,
                trigger: Trigger::Times,
                times: vec![(b.hour, b.minute)],
                days: b.days,
                use_ad_list: true,
                count: b.max_ads.max(1),
                pick: if b.max_ads == 0 { RotPick::All } else { RotPick::InOrder },
                ..Default::default()
            });
        }
        for (k, r) in rules.iter_mut().enumerate() {
            r.id = k as u64 + 1;
        }
        self.ad_rules.retain(|r| r.enabled || !r.folder.is_empty() || !r.files.is_empty()); // drop the untouched default
        self.ad_rules.extend(rules);
        for (k, r) in self.ad_rules.iter_mut().enumerate() {
            r.id = k as u64 + 1;
        }
        self.ad_rules_v = 1;
    }

    /// A rule id not used yet.
    pub fn new_rule_id(&self) -> u64 {
        self.ad_rules.iter().map(|r| r.id).max().unwrap_or(0) + 1
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn old_rules_and_breaks_become_one_list() {
        let old = r#"{"rotations":[{"name":"Jingles","enabled":true,"adverts":false,"folder":"J","every_n":3,"count":1}],
                      "ad_breaks":[{"name":"Break","enabled":true,"hour":15,"minute":45,"days":[true,true,true,true,true,false,false],"max_ads":0}]}"#;
        let mut c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.ad_rules_v, 0);
        c.migrate_ad_rules();
        assert_eq!(c.ad_rules.len(), 2, "the untouched default rule is dropped");
        let (j, b) = (&c.ad_rules[0], &c.ad_rules[1]);
        assert_eq!((j.trigger, j.every_songs, j.folder.as_str()), (Trigger::Songs, 3, "J"));
        assert_eq!((b.trigger, b.times.clone(), b.use_ad_list, b.pick), (Trigger::Times, vec![(15, 45)], true, RotPick::All));
        assert_ne!(j.id, b.id);
        assert!(c.rotations.is_empty() && c.ad_breaks.is_empty());
    }

    #[test]
    fn rules_follow_dates_days_hours_and_intervals() {
        // Tuesday 20 October 2026
        let r = AdRule {
            enabled: true,
            trigger: Trigger::Minutes,
            every_mins: 30,
            on_clock: true,
            from_date: "2026-10-01".into(),
            to_date: "2026-10-31".into(),
            days: [true, true, true, true, true, false, false],
            from_hour: 22,
            to_hour: 6,
            ..Default::default()
        };
        assert!(r.in_force(&at(2026, 10, 20, 23, 0)), "over midnight");
        assert!(r.in_force(&at(2026, 10, 20, 5, 59)));
        assert!(!r.in_force(&at(2026, 10, 20, 12, 0)), "outside the hours");
        assert!(!r.in_force(&at(2026, 10, 24, 23, 0)), "Saturday");
        assert!(!r.in_force(&at(2026, 11, 3, 23, 0)), "after the end date");
        assert!(r.due_now(&at(2026, 10, 20, 23, 30), None));
        assert!(!r.due_now(&at(2026, 10, 20, 23, 31), None));
        let r = AdRule { on_clock: false, every_mins: 120, from_hour: 0, to_hour: 0, ..r };
        assert!(r.due_now(&at(2026, 10, 20, 13, 7), None), "first time");
        assert!(!r.due_now(&at(2026, 10, 20, 14, 7), Some(at(2026, 10, 20, 13, 7))));
        assert!(r.due_now(&at(2026, 10, 20, 15, 7), Some(at(2026, 10, 20, 13, 7))));
        assert!(r.summary().contains("every 2 hours"));
        let t = AdRule { enabled: true, trigger: Trigger::Times, times: vec![(9, 0)], ..Default::default() };
        assert!(t.due_now(&at(2026, 10, 20, 9, 0), None) && !t.due_now(&at(2026, 10, 20, 9, 1), None));
    }
}
