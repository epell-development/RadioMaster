//! RadioMaster Server sync: what a headless server sends its desktop apps and what they send back.
//!
//! A desktop app connected to a server swaps its own playout for a mirror. The server's playlists,
//! settings, library and status are copied into the mirror; playlist and settings edits made in the
//! app go back as whole lists; playback actions (play, next, carts, encoders…) are forwarded as
//! commands. The mirror never plays, schedules or saves anything itself.

// shared by the app and the server, which each use their own half
#![allow(dead_code)]

use super::*;
use std::hash::{Hash, Hasher};

/// Bumped when the server API changes in a way older apps can't follow.
pub const PROTOCOL: u32 = 1;

/// The server's default TCP port (the remote-control API uses 9258).
pub const DEFAULT_PORT: u16 = 9259;

/// The web portal's default port.
pub const DEFAULT_PORTAL_PORT: u16 = 9260;

/// An action forwarded from a desktop app to the server. `tab` is the playlist tab it applies to.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Cmd {
    PlayPressed {
        tab: usize,
        selected: Option<usize>,
    },
    PlayIndex {
        tab: usize,
        row: usize,
        xfade: f32,
    },
    PauseToggle,
    Stop,
    Next,
    Prev,
    Volume {
        v: f32,
    },
    Seek {
        secs: f32,
    },
    Cart {
        idx: usize,
    },
    StopCarts,
    TabOnAir {
        tab: usize,
    },
    EncoderStart {
        i: usize,
    },
    EncoderStop {
        i: usize,
    },
    Command {
        tab: usize,
        text: String,
    },
    Event {
        event: Event,
    },
    Generate {
        tab: usize,
        name: String,
        append: bool,
    },
    FireRule {
        tab: usize,
        id: u64,
        after: Option<usize>,
    },
    Rescan,
    WriteTags {
        track: Track,
    },
    SetCues {
        path: PathBuf,
        cues: Cues,
        duration: f32,
    },
    ClearCues {
        path: PathBuf,
    },
    ReanalyseCues,
    /// files (or folders) on the server into a playlist at `at` (None: at the end)
    Insert {
        tab: usize,
        paths: Vec<PathBuf>,
        at: Option<usize>,
    },
    /// take rows out of a playlist
    RemoveRows {
        tab: usize,
        rows: Vec<usize>,
    },
}

impl Cmd {
    /// Whether an account with these permissions may send this action.
    pub fn allowed(&self, p: &crate::auth::Perms) -> bool {
        use crate::auth::tool;
        match self {
            Cmd::PlayPressed { .. }
            | Cmd::PlayIndex { .. }
            | Cmd::PauseToggle
            | Cmd::Stop
            | Cmd::Next
            | Cmd::Prev
            | Cmd::Volume { .. }
            | Cmd::Seek { .. }
            | Cmd::TabOnAir { .. } => p.playback,
            Cmd::Cart { .. } | Cmd::StopCarts => p.jingles,
            Cmd::EncoderStart { .. } | Cmd::EncoderStop { .. } => p.streaming,
            Cmd::Command { .. } | Cmd::Insert { .. } | Cmd::RemoveRows { .. } => p.edit_playlists,
            Cmd::Event { .. } => p.tool(tool::SCHEDULER),
            Cmd::Generate { .. } => p.edit_playlists && p.tool(tool::GENERATOR),
            Cmd::FireRule { .. } => p.edit_playlists && p.tool(tool::ADS),
            Cmd::Rescan => p.settings || p.tool(tool::LIBRARY),
            Cmd::WriteTags { .. } | Cmd::SetCues { .. } | Cmd::ClearCues { .. } | Cmd::ReanalyseCues => p.tool(tool::TRACK_EDITOR),
        }
    }
}

/// The first part of the station settings that differs between `old` and `new` and that an account
/// with `p` may not change (None: it may make the whole change).
pub fn cfg_denied(old: &Config, new: &Config, p: &crate::auth::Perms) -> Option<String> {
    use crate::auth::tool;
    let (Ok(serde_json::Value::Object(a)), Ok(serde_json::Value::Object(b))) = (serde_json::to_value(old), serde_json::to_value(new))
    else {
        return Some("settings".into());
    };
    let allowed = |key: &str| match key {
        "categories" | "templates" | "auto_enabled" | "auto_template" | "auto_min_mins" | "repeat" => p.tool(tool::GENERATOR),
        "ads" | "ad_breaks" | "ad_rules" | "ad_rules_v" | "rotations" | "teaser_secs" => p.tool(tool::ADS),
        "events" => p.tool(tool::SCHEDULER),
        "dsp" | "eq_presets" => p.tool(tool::AUDIO_FX),
        "azuracast" => p.tool(tool::AZURACAST),
        "carts" => p.jingles,
        "encoders" => p.streaming,
        "mic_gain" | "mic_duck" | "mic_mode" | "volume" => p.playback,
        "loop_playlist" | "random_fill" | "hour_lines" | "announce" => p.edit_playlists,
        _ => p.settings,
    };
    let null = serde_json::Value::Null;
    a.keys().chain(b.keys()).find(|k| a.get(*k).unwrap_or(&null) != b.get(*k).unwrap_or(&null) && !allowed(k)).cloned()
}

/// A server account as an administrator sees it (never with its password).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UserWire {
    pub name: String,
    pub admin: bool,
    pub perms: crate::auth::Perms,
    pub disabled: bool,
}

/// A change to the server's accounts (administrators only).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum UserOp {
    Add { name: String, password: String, admin: bool, perms: crate::auth::Perms },
    Update { name: String, admin: bool, perms: crate::auth::Perms, disabled: bool },
    Password { name: String, password: String },
    Delete { name: String },
}

/// A folder on the server, as its file browser shows it.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct FsList {
    /// the folder listed (where the browser starts when none was asked for)
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    /// drives (Windows) or / , the library folders and the home folder
    pub places: Vec<PathBuf>,
    pub entries: Vec<FsEntry>,
    /// why the folder couldn't be read (empty when it could)
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct FsEntry {
    pub path: PathBuf,
    pub dir: bool,
    pub size: u64,
}

/// List a folder for a file browser (reads the disk): its folders and audio files (`all`: every
/// file). An empty path starts at the first library folder, else the home folder.
pub fn list_folder(path: PathBuf, all: bool, library_folders: &[String]) -> FsList {
    let home = dirs::home_dir();
    let start = || library_folders.first().map(PathBuf::from).or_else(|| home.clone()).unwrap_or_else(|| PathBuf::from("/"));
    let path = if path.as_os_str().is_empty() { start() } else { path };
    let mut places = crate::library::drive_roots().to_vec();
    places.extend(library_folders.iter().map(PathBuf::from));
    places.extend(home.clone());
    let mut out = FsList { parent: path.parent().map(|p| p.to_path_buf()), places, ..Default::default() };
    match std::fs::read_dir(&path) {
        Ok(rd) => {
            out.entries = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    // (follows links, so a linked folder opens like any other)
                    let p = e.path();
                    let m = std::fs::metadata(&p).ok()?;
                    let dir = m.is_dir();
                    (dir || all || crate::library::is_audio(&p)).then_some(FsEntry { dir, size: if dir { 0 } else { m.len() }, path: p })
                })
                .collect();
            out.entries.sort_by(|a, b| {
                b.dir.cmp(&a.dir).then_with(|| {
                    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    name(&a.path).cmp(&name(&b.path))
                })
            });
        }
        Err(e) => out.error = e.to_string(),
    }
    out.path = path;
    out
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct TabWire {
    pub name: String,
    pub entries: Vec<PlEntry>,
    pub current: Option<usize>,
}

/// Every playlist tab, the on-air one included.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct TabsWire {
    pub tabs: Vec<TabWire>,
    pub playing: usize,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct LogWire {
    pub time: String,
    pub text: String,
    pub error: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct HistWire {
    pub time: String,
    pub text: String,
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct EncWire {
    /// idle, connecting, live or error
    pub state: String,
    pub error: String,
    pub bytes: u64,
    pub reconnects: u32,
}

/// What changes all the time: sent with every poll.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Live {
    /// playing, paused or stopped
    pub state: String,
    pub has_track: bool,
    pub pos: f32,
    pub total: f32,
    pub peak_l: f32,
    pub peak_r: f32,
    pub on_air: Option<Track>,
    pub on_air_label: String,
    /// unix seconds
    pub wait_until: Option<i64>,
    pub wait_label: String,
    /// one per encoder; None = not running
    pub encoders: Vec<Option<EncWire>>,
    pub carts: Vec<PathBuf>,
    pub scanning: bool,
    pub scan_progress: usize,
    pub notify: String,
    /// the server's clock when this was sent (unix ms; 0 from older servers)
    pub now_ms: i64,
}

/// One poll's answer. The lists come only when they changed since the revision the app has.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Snapshot {
    pub live: Live,
    pub rev: u64,
    pub tabs: Option<TabsWire>,
    pub cfg_rev: u64,
    pub cfg: Option<Config>,
    pub log_rev: u64,
    pub log: Option<Vec<LogWire>>,
    pub hist_rev: u64,
    pub history: Option<Vec<HistWire>>,
    /// the library is fetched on its own when this changes
    pub lib_ver: u64,
    /// playlist files the server can't find (sent when the list changed)
    pub missing_rev: u64,
    pub missing: Option<Vec<PathBuf>>,
}

/// A waveform for the track editor of an app (levels 0..1 scaled to 0..65535), with the cues the
/// server uses for that file.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct WaveWire {
    pub peaks: Vec<u16>,
    pub rms: Vec<u16>,
    pub cues: Option<Cues>,
}

impl WaveWire {
    pub fn of(w: &Waveform, cues: Option<Cues>) -> WaveWire {
        let q = |v: &[f32]| v.iter().map(|x| (x.clamp(0.0, 1.0) * 65535.0).round() as u16).collect();
        WaveWire { peaks: q(&w.peaks), rms: q(&w.rms), cues }
    }

    pub fn waveform(&self) -> Waveform {
        let f = |v: &[u16]| v.iter().map(|&x| x as f32 / 65535.0).collect();
        Waveform { peaks: f(&self.peaks), rms: f(&self.rms) }
    }
}

/// The answer to an app asking for a waveform: it, or "still analysing, ask again".
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WaveReply {
    pub wave: Option<WaveWire>,
    pub pending: bool,
}

/// The server's own settings, as the app's Server settings window shows them.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ServerInfo {
    pub name: String,
    /// the account signed in
    pub user: String,
    /// it is an administrator (only they change these settings, accounts and updates)
    pub admin: bool,
    pub version: String,
    /// a newer RadioMaster Server release (empty: up to date, or not checked yet)
    pub latest: String,
    /// that release has a download for the server's system, so it can update itself
    pub can_update: bool,
    /// what an update or restart is doing now (empty: nothing)
    pub update_state: String,
    pub port: u16,
    /// start playing when the server starts
    pub autoplay: bool,
    /// play through the server's sound cards (off: streaming only)
    pub audio: bool,
    /// sound cards the server has
    pub devices: Vec<String>,
    /// the ones chosen (none enabled: the default card)
    pub outputs: Vec<crate::config::OutputSel>,
    /// (device, status) as last opened
    pub report: Vec<(String, String)>,
    /// the server was started with --no-audio, which wins over `audio`
    pub forced_silent: bool,
    /// HTTPS, the web portal and its port as saved (they take effect when the server restarts)
    pub https: bool,
    /// SHA-256 of the certificate in use (empty without HTTPS)
    pub fingerprint: String,
    pub portal: bool,
    pub portal_port: u16,
    /// the saved HTTPS / portal settings differ from what is running
    pub restart_needed: bool,
    /// the newer release's page
    pub release_page: String,
    /// when the certificate in use runs out (unix seconds; 0 without HTTPS)
    pub cert_expires: i64,
    /// Let's Encrypt: on, the domain names (comma-separated), contact email, staging server
    pub acme: bool,
    pub acme_domains: String,
    pub acme_email: String,
    pub acme_staging: bool,
    /// how getting the certificate went: "", working, ok or error, and what happened
    pub acme_state: String,
    pub acme_message: String,
    /// Cloudflare Tunnel: on, and whether a tunnel token is saved (the token itself is never sent back)
    pub tunnel: bool,
    pub tunnel_token_set: bool,
    /// "", starting, running or error; the public address (quick tunnels); what went wrong
    pub tunnel_state: String,
    pub tunnel_url: String,
    pub tunnel_message: String,
}

/// Changes from the Server settings window.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ServerUpdate {
    pub name: String,
    pub autoplay: bool,
    pub audio: bool,
    pub outputs: Vec<crate::config::OutputSel>,
    /// to change the password: the current one and the new one
    pub password: Option<(String, String)>,
    /// saved for the next start of the server
    pub https: bool,
    pub portal: bool,
    pub portal_port: u16,
    /// Let's Encrypt and Cloudflare Tunnel (None: leave as they are, which is what older apps send)
    pub acme: Option<bool>,
    pub acme_domains: Option<String>,
    pub acme_email: Option<String>,
    pub acme_staging: Option<bool>,
    pub tunnel: Option<bool>,
    /// a new tunnel token (an empty one removes it: a quick tunnel)
    pub tunnel_token: Option<String>,
}

// ------------------------------------------------------------ station profiles onto a server

/// The text looks like a full path on Windows (C:\\…, \\\\server\\…) or elsewhere (/…).
fn is_abs_path(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() > 1 && b[0] == b'/' && b[1] != b'/')
        || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || s.starts_with("\\\\")
}

/// "C:\\Music\\x.mp3" → "c:/music/x.mp3" for Windows-style paths (which ignore case), "/srv/x" as is.
fn path_key(s: &str) -> String {
    let unified = s.replace('\\', "/");
    if s.contains('\\') || s.as_bytes().get(1) == Some(&b':') {
        unified.to_lowercase()
    } else {
        unified
    }
}

fn each_string(v: &serde_json::Value, f: &mut dyn FnMut(&str)) {
    match v {
        serde_json::Value::String(s) => f(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| each_string(x, f)),
        serde_json::Value::Object(o) => o.values().for_each(|x| each_string(x, f)),
        _ => {}
    }
}

/// The folders a station profile's file paths are under, as few as cover them all (a file's own
/// folder, or the path itself when it has no extension), for mapping them to the server's folders.
pub fn path_roots(v: &serde_json::Value) -> Vec<String> {
    let mut dirs: Vec<String> = vec![];
    each_string(v, &mut |s| {
        if !is_abs_path(s) {
            return;
        }
        let unified = s.trim_end_matches(['/', '\\']).replace('\\', "/");
        let last = unified.rsplit('/').next().unwrap_or("");
        let dir =
            if last.contains('.') { unified.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or(unified.clone()) } else { unified };
        // back in the profile's own style
        let dir = if s.contains('\\') { dir.replace('/', "\\") } else { dir };
        if !dir.is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    });
    dirs.sort_by_key(|d| d.len());
    let mut roots: Vec<String> = vec![];
    for d in dirs {
        let k = path_key(&d);
        if !roots.iter().any(|r| {
            let rk = path_key(r);
            k == rk || k.starts_with(&format!("{}/", rk.trim_end_matches('/')))
        }) {
            roots.push(d);
        }
    }
    roots
}

/// `path` moved from under `from` to under `to` (None: it isn't under `from`). The rest of the path
/// takes `to`'s separators.
pub fn map_path(path: &str, from: &str, to: &str) -> Option<String> {
    let (pk, fk) = (path_key(path), path_key(from.trim_end_matches(['/', '\\'])));
    if fk.is_empty() || !(pk == fk || pk.starts_with(&format!("{fk}/"))) {
        return None;
    }
    let rest = path.replace('\\', "/")[fk.len()..].to_string();
    let windows = to.contains('\\') || to.as_bytes().get(1) == Some(&b':');
    let rest = if windows { rest.replace('/', "\\") } else { rest };
    Some(format!("{}{rest}", to.trim_end_matches(['/', '\\'])))
}

/// Move every path in a station profile with the first mapping (from, to) it fits. Returns how
/// many paths changed.
pub fn map_paths(v: &mut serde_json::Value, maps: &[(String, String)]) -> usize {
    match v {
        serde_json::Value::String(s) if is_abs_path(s) => {
            match maps
                .iter()
                .filter(|(f, t)| !f.trim().is_empty() && !t.trim().is_empty())
                .find_map(|(f, t)| map_path(s, f.trim(), t.trim()))
            {
                Some(n) => {
                    *s = n;
                    1
                }
                None => 0,
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().map(|x| map_paths(x, maps)).sum(),
        serde_json::Value::Object(o) => o.values_mut().map(|x| map_paths(x, maps)).sum(),
        _ => 0,
    }
}

/// Playlist files that are missing (on the server, which is the machine that plays them).
pub fn missing_in(t: &TabsWire) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = t
        .tabs
        .iter()
        .flat_map(|t| t.entries.iter())
        .filter(|e| e.is_file() && library::file_missing(&e.track.path))
        .map(|e| e.track.path.clone())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// How many log lines and history items a client is sent.
pub const TAIL: usize = 300;

/// A content fingerprint (same value for the same JSON).
pub fn digest<T: Serialize>(v: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(v).unwrap_or_default().hash(&mut h);
    h.finish()
}

/// The settings that belong to one machine and are never copied between the server and its apps:
/// sound cards, the remote-control API, interface language, update checks, plugins and ffmpeg.
pub fn keep_local(new: &mut Config, here: &Config) {
    new.audio_devices = here.audio_devices.clone();
    new.remote = here.remote.clone();
    new.language = here.language.clone();
    new.check_updates = here.check_updates;
    new.plugins_off = here.plugins_off.clone();
    new.ffmpeg_path = here.ffmpeg_path.clone();
    new.hear_server = here.hear_server;
}

/// The station part of the settings (what is shared with the server).
pub fn station_cfg(cfg: &Config) -> Config {
    let mut c = cfg.clone();
    keep_local(&mut c, &Config::default());
    c
}

fn static_kind(k: &str) -> &'static str {
    const KINDS: [&str; 10] = ["song", "advert", "jingle", "voice", "speech", "time", "stream", "live", "teaser", "other"];
    KINDS.iter().find(|x| **x == k).copied().unwrap_or("other")
}

fn enc_wire(h: &EncoderHandle) -> EncWire {
    let s = h.status.lock();
    let (state, error) = match &s.state {
        crate::stream::EncState::Idle => ("idle", String::new()),
        crate::stream::EncState::Connecting => ("connecting", String::new()),
        crate::stream::EncState::Live => ("live", String::new()),
        crate::stream::EncState::Error(e) => ("error", e.clone()),
    };
    EncWire { state: state.into(), error, bytes: s.bytes, reconnects: s.reconnects }
}

fn enc_state(w: &EncWire) -> crate::stream::EncState {
    match w.state.as_str() {
        "connecting" => crate::stream::EncState::Connecting,
        "live" => crate::stream::EncState::Live,
        "error" => crate::stream::EncState::Error(w.error.clone()),
        _ => crate::stream::EncState::Idle,
    }
}

impl Playout {
    /// A mirror of a server for this desktop app: starts empty (the first poll fills it), keeps this
    /// machine's own settings and shares the status-bar message with `local`.
    pub fn mirror_of(local: &Playout, link: crossbeam_channel::Sender<Cmd>) -> Playout {
        let log_sink: LogSink = Arc::new(Mutex::new(vec![]));
        let plugins = crate::plugins::Plugins::none(log_sink.clone());
        let mut cfg = local.cfg.clone();
        cfg.encoders.clear();
        let mut p = Self::build(Engine::new_detached(), cfg, vec![], HashMap::new(), HashMap::new(), plugins, log_sink);
        p.notify = local.notify.clone();
        p.link = Some(link);
        p.reindex();
        p
    }

    /// Send an action to the server instead of doing it here. False when this playout isn't a mirror.
    pub(super) fn forward(&self, cmd: impl FnOnce(usize) -> Cmd) -> bool {
        match &self.link {
            Some(tx) => {
                let _ = tx.send(cmd(self.swapped.unwrap_or(self.playing_tab)));
                true
            }
            None => false,
        }
    }

    /// Jingles playing now (on the server when this is a mirror).
    pub fn carts_playing(&self) -> Vec<PathBuf> {
        if self.link.is_some() {
            self.mirror_carts.clone()
        } else {
            self.engine.carts_playing()
        }
    }

    // ------------------------------------------------------------ playlists
    pub fn tabs_wire(&self) -> TabsWire {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (entries, current) = if i == self.playing_tab { (&self.playlist, self.current) } else { (&t.entries, t.current) };
                TabWire { name: t.name.clone(), entries: entries.clone(), current }
            })
            .collect();
        TabsWire { tabs, playing: self.playing_tab }
    }

    /// Replace every playlist tab. The track on air keeps playing; only the lists change.
    pub fn set_tabs(&mut self, w: TabsWire) {
        if w.tabs.is_empty() {
            return;
        }
        let mut tabs: Vec<PlTab> = w
            .tabs
            .into_iter()
            .map(|t| {
                let current = t.current.filter(|&c| c < t.entries.len());
                PlTab { name: t.name, entries: t.entries, current }
            })
            .collect();
        let playing = w.playing.min(tabs.len() - 1);
        self.playlist = std::mem::take(&mut tabs[playing].entries);
        self.current = tabs[playing].current.take();
        self.tabs = tabs;
        self.playing_tab = playing;
        if self.pending_jump.is_some_and(|j| j >= self.playlist.len()) {
            self.pending_jump = None;
        }
    }

    // ------------------------------------------------------------ settings
    /// Take the station settings from elsewhere, keeping this machine's own (`keep_local`).
    pub fn set_station_cfg(&mut self, mut new: Config) {
        keep_local(&mut new, &self.cfg);
        new.carts.resize(24, Default::default());
        if self.link.is_none() {
            if new.encoders.len() != self.cfg.encoders.len() {
                // an encoder was added or removed: the others keep running (matched by their settings)
                let key = |e: &crate::config::EncoderConfig| serde_json::to_string(e).unwrap_or_default();
                let mut old: Vec<(String, Option<EncoderHandle>)> =
                    self.cfg.encoders.iter().map(key).zip(std::mem::take(&mut self.handles)).collect();
                self.handles = new
                    .encoders
                    .iter()
                    .map(|e| {
                        let k = key(e);
                        old.iter_mut().find(|(o, h)| *o == k && h.is_some()).and_then(|(_, h)| h.take())
                    })
                    .collect();
                for (_, h) in old.into_iter() {
                    if let Some(h) = h {
                        h.stop();
                    }
                }
            }
            if (new.volume - self.cfg.volume).abs() > f32::EPSILON {
                self.engine.set_volume(new.volume);
            }
        } else {
            self.handles.resize_with(new.encoders.len(), || None);
        }
        self.cfg = new;
    }

    // ------------------------------------------------------------ library
    pub fn set_library(&mut self, library: Vec<Track>) {
        self.library = library;
        self.reindex();
    }

    // ------------------------------------------------------------ live status
    pub fn live_wire(&self) -> Live {
        Live {
            state: match self.state {
                PlayState::Playing => "playing",
                PlayState::Paused => "paused",
                PlayState::Stopped => "stopped",
            }
            .into(),
            has_track: self.status.has_track,
            pos: self.status.pos,
            total: self.status.total,
            peak_l: self.status.peak_l,
            peak_r: self.status.peak_r,
            on_air: self.on_air.clone(),
            on_air_label: self.on_air_label.clone(),
            wait_until: self.wait_until.map(|t| t.timestamp()),
            wait_label: self.wait_label.clone(),
            encoders: self.handles.iter().map(|h| h.as_ref().filter(|h| h.is_running()).map(enc_wire)).collect(),
            carts: self.engine.carts_playing(),
            scanning: self.scanning,
            scan_progress: self.scan_progress,
            notify: self.notify.lock().clone(),
            now_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    pub fn apply_live(&mut self, l: Live) {
        self.state = match l.state.as_str() {
            "playing" => PlayState::Playing,
            "paused" => PlayState::Paused,
            _ => PlayState::Stopped,
        };
        // the position is counted on here between polls, so it ticks with the clocks instead of in
        // uneven steps; it only jumps when it is more than a moment out (a seek or a new track)
        let playing = self.state == PlayState::Playing && l.has_track;
        let age = if l.now_ms > 0 { ((self.now().timestamp_millis() - l.now_ms).clamp(0, 2000)) as f32 / 1000.0 } else { 0.0 };
        let est = if playing { (l.pos + age).min(l.total.max(l.pos)) } else { l.pos };
        let same = self.on_air.as_ref().map(|t| &t.path) == l.on_air.as_ref().map(|t| &t.path);
        self.mirror_pos = match self.mirror_pos {
            Some((p, t)) if playing && same && (p + t.elapsed().as_secs_f32() - est).abs() < 0.3 => Some((p, t)),
            _ if playing => Some((est, Instant::now())),
            _ => None,
        };
        self.status = Status { has_track: l.has_track, pos: est, total: l.total, peak_l: l.peak_l, peak_r: l.peak_r };
        self.follow_mirror_pos();
        self.on_air = l.on_air;
        self.on_air_label = l.on_air_label;
        self.wait_until = l.wait_until.and_then(|t| Local.timestamp_opt(t, 0).single());
        self.wait_label = l.wait_label;
        self.handles.resize_with(self.cfg.encoders.len(), || None);
        for (i, slot) in self.handles.iter_mut().enumerate() {
            match l.encoders.get(i).cloned().flatten() {
                Some(w) => match slot {
                    Some(h) => {
                        let mut s = h.status.lock();
                        s.state = enc_state(&w);
                        s.bytes = w.bytes;
                        s.reconnects = w.reconnects;
                    }
                    None => *slot = Some(EncoderHandle::mirror(enc_state(&w), w.bytes, w.reconnects)),
                },
                None => *slot = None,
            }
        }
        self.mirror_carts = l.carts;
        self.scanning = l.scanning;
        self.scan_progress = l.scan_progress;
    }

    /// Move a mirror's on-air position on to now.
    pub(super) fn follow_mirror_pos(&mut self) {
        if let Some((p, t)) = self.mirror_pos {
            if self.state == PlayState::Playing {
                self.status.pos = (p + t.elapsed().as_secs_f32()).min(self.status.total.max(p));
            }
        }
    }

    // ------------------------------------------------------------ files on the server
    pub fn set_missing(&mut self, v: Vec<PathBuf>) {
        self.mirror_missing = v.into_iter().collect();
    }

    /// A file in the library or a playlist (apps may only ask about those).
    pub fn knows_file(&self, p: &Path) -> bool {
        self.lib_index.contains_key(p)
            || self.playlist.iter().chain(self.tabs.iter().flat_map(|t| t.entries.iter())).any(|e| e.track.path == p)
    }

    /// The waveform and cues of a file, if it has been analysed.
    pub fn wave_wire(&self, p: &Path) -> Option<WaveWire> {
        self.waves.get(p).map(|w| WaveWire::of(w, self.cues.get(p).copied()))
    }

    /// From now on this mirror asks the server for waveforms: the paths it wants arrive on the
    /// returned channel, for `link::Link::start`.
    pub fn wave_requests(&mut self) -> crossbeam_channel::Receiver<PathBuf> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.remote_waves = Some(tx);
        self.pending.clear();
        rx
    }

    /// A waveform the server sent, or why it couldn't.
    pub fn put_remote_wave(&mut self, path: PathBuf, r: Result<WaveWire, String>) {
        self.pending.remove(&path);
        match r {
            Ok(w) => {
                // cues set by hand here meanwhile win; otherwise the server's (or worked out here)
                let wave = w.waveform();
                if !self.cues.get(&path).is_some_and(|c| c.manual) {
                    let c = w.cues.unwrap_or_else(|| smart_cues(&wave, &self.cfg.fade));
                    self.cues.insert(path.clone(), c);
                }
                self.keep_wave(path, wave);
            }
            Err(e) => {
                self.wave_errors.insert(path, e);
            }
        }
    }

    pub fn log_wire(&self) -> Vec<LogWire> {
        let from = self.log.len().saturating_sub(TAIL);
        self.log[from..].iter().map(|l| LogWire { time: l.time.clone(), text: l.text.clone(), error: l.error }).collect()
    }

    pub fn apply_log(&mut self, v: Vec<LogWire>) {
        self.log = v.into_iter().map(|l| LogLine { time: l.time, text: l.text, error: l.error }).collect();
    }

    pub fn history_wire(&self) -> Vec<HistWire> {
        let from = self.history.len().saturating_sub(TAIL);
        self.history[from..].iter().map(|h| HistWire { time: h.time.clone(), text: h.text.clone(), kind: h.kind.to_string() }).collect()
    }

    pub fn apply_history(&mut self, v: Vec<HistWire>) {
        self.history = v.into_iter().map(|h| HistoryItem { time: h.time, text: h.text, kind: static_kind(&h.kind) }).collect();
    }

    // ------------------------------------------------------------ server side
    /// Carry out an action an app sent.
    pub fn run_cmd(&mut self, c: Cmd) {
        let tab_ok = |p: &Self, t: usize| t < p.tabs.len();
        match c {
            Cmd::PlayPressed { tab, selected } if tab_ok(self, tab) => self.with_tab(tab, |p| p.play_pressed(selected)),
            Cmd::PlayIndex { tab, row, xfade } if tab_ok(self, tab) => {
                self.with_tab(tab, |p| {
                    if row < p.playlist.len() {
                        p.play_index(row, xfade);
                    }
                });
            }
            Cmd::PauseToggle => self.pause_toggle(),
            Cmd::Stop => self.stop(),
            Cmd::Next => self.next(),
            Cmd::Prev => self.prev(),
            Cmd::Volume { v } => self.set_volume(v.clamp(0.0, 1.0)),
            Cmd::Seek { secs } => self.seek(secs.max(0.0)),
            Cmd::Cart { idx } => self.trigger_cart(idx),
            Cmd::StopCarts => self.stop_carts(),
            Cmd::TabOnAir { tab } => self.put_tab_on_air(tab),
            Cmd::EncoderStart { i } => self.start_encoder(i),
            Cmd::EncoderStop { i } => self.stop_encoder(i),
            Cmd::Command { tab, text } if tab_ok(self, tab) => self.with_tab(tab, |p| p.run_command(&text)),
            Cmd::Event { event } => self.run_event(&event),
            Cmd::Generate { tab, name, append } if tab_ok(self, tab) => {
                let n = self.with_tab(tab, |p| p.generate_playlist(&name, append));
                if n > 0 {
                    self.say(format!("Generated {n} tracks from '{name}'"));
                }
            }
            Cmd::FireRule { tab, id, after } if tab_ok(self, tab) => {
                self.with_tab(tab, |p| p.fire_rule(id, after));
            }
            Cmd::Rescan => self.start_scan(),
            Cmd::WriteTags { track } => self.update_tags(track),
            Cmd::SetCues { path, cues, duration } => self.set_manual_cues(&path, cues, duration),
            Cmd::ClearCues { path } => self.clear_manual_cues(&path),
            Cmd::ReanalyseCues => self.reanalyse_cues(),
            Cmd::Insert { tab, paths, at } if tab_ok(self, tab) => {
                self.with_tab(tab, |p| {
                    let rows: Vec<PlEntry> = p.expand(paths).iter().map(|f| p.entry_quick(f)).collect();
                    p.insert_rows(at.unwrap_or(usize::MAX), rows);
                });
                self.save_tabs();
            }
            Cmd::RemoveRows { tab, rows } if tab_ok(self, tab) => {
                self.with_tab(tab, |p| p.remove(&rows.into_iter().collect()));
                self.save_tabs();
            }
            other => self.say(format!("Server: ignored {other:?} (that playlist is gone)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> PlEntry {
        PlEntry::file(Track { path: PathBuf::from(format!("/music/{name}.mp3")), title: name.into(), ..Default::default() })
    }

    #[test]
    fn tabs_round_trip() {
        let mut p = Playout::new(Engine::new_detached(), Config::default());
        p.set_tabs(TabsWire {
            tabs: vec![
                TabWire { name: "Main".into(), entries: vec![row("a"), row("b")], current: Some(1) },
                TabWire { name: "Show".into(), entries: vec![row("c")], current: None },
            ],
            playing: 1,
        });
        assert_eq!(p.playing_tab, 1);
        assert_eq!(p.playlist.len(), 1);
        assert_eq!(p.tab_len(0), 2);
        let w = p.tabs_wire();
        assert_eq!(w.playing, 1);
        assert_eq!(w.tabs[0].current, Some(1));
        assert_eq!(w.tabs[1].entries[0].track.title, "c");
        assert_eq!(digest(&w), digest(&p.tabs_wire()));
    }

    #[test]
    fn mirror_forwards_actions_to_the_tab_in_use() {
        let local = Playout::new(Engine::new_detached(), Config::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut m = Playout::mirror_of(&local, tx);
        m.set_tabs(TabsWire {
            tabs: vec![
                TabWire { name: "Main".into(), entries: vec![row("a")], current: None },
                TabWire { name: "PL1".into(), entries: vec![row("b")], current: None },
            ],
            playing: 0,
        });
        m.with_tab(1, |c| {
            assert!(c.play_index(0, 0.0));
        });
        m.next();
        assert!(matches!(rx.try_recv(), Ok(Cmd::PlayIndex { tab: 1, row: 0, .. })));
        assert!(matches!(rx.try_recv(), Ok(Cmd::Next)));
        // nothing played or moved here
        assert_eq!(m.state, PlayState::Stopped);
        assert_eq!(m.playing_tab, 0);
    }

    #[test]
    fn profile_paths_map_to_the_server() {
        let v = serde_json::json!({
            "library_folders": ["C:\\Users\\DJ\\Music"],
            "carts": [{"path": "C:\\Users\\DJ\\Music\\Jingles\\id.mp3"}, {"path": ""}],
            "ads": [{"path": "D:\\Ads\\spring.mp3"}],
            "azuracast": {"base_url": "https://radio.example.com/api"},
            "_session": {"tabs": [{"entries": [{"track": {"path": "/home/dj/show/a.flac"}}]}]}
        });
        let roots = path_roots(&v);
        assert_eq!(roots, vec!["D:\\Ads".to_string(), "/home/dj/show".into(), "C:\\Users\\DJ\\Music".into()]);
        let mut m = v.clone();
        let maps = vec![("c:\\users\\dj\\music".to_string(), "/srv/music".to_string()), ("D:\\Ads".into(), "E:\\Station\\Ads".into())];
        assert_eq!(map_paths(&mut m, &maps), 3);
        assert_eq!(m["library_folders"][0], "/srv/music");
        assert_eq!(m["carts"][0]["path"], "/srv/music/Jingles/id.mp3");
        assert_eq!(m["ads"][0]["path"], "E:\\Station\\Ads\\spring.mp3");
        // not mapped, and not paths at all, stay as they were
        assert_eq!(m["_session"]["tabs"][0]["entries"][0]["track"]["path"], "/home/dj/show/a.flac");
        assert_eq!(m["azuracast"]["base_url"], "https://radio.example.com/api");
        // a folder whose name only starts the same isn't under it
        assert_eq!(map_path("/srv/musicals/x.mp3", "/srv/music", "/data"), None);
    }

    #[test]
    fn permissions_limit_what_an_account_changes() {
        let p = crate::auth::Perms::presenter();
        let old = Config::default();
        let mut new = old.clone();
        new.carts.resize(24, Default::default());
        new.carts[0].path = "/srv/jingles/id.mp3".into();
        assert_eq!(cfg_denied(&old, &new, &p), None, "presenters may set up jingles");
        new.crossfade_secs = 9.0;
        assert_eq!(cfg_denied(&old, &new, &p).as_deref(), Some("crossfade_secs"));
        assert_eq!(cfg_denied(&old, &new, &crate::auth::Perms::default()), None);
        assert!(Cmd::Next.allowed(&p) && !Cmd::EncoderStart { i: 0 }.allowed(&p));
        assert!(!Cmd::Generate { tab: 0, name: "Hits".into(), append: false }.allowed(&p));
    }

    #[test]
    fn waveforms_survive_the_trip() {
        let w = Waveform { peaks: vec![0.0, 0.5, 1.0, 2.0], rms: vec![0.0, 0.25, 0.7, 0.001] };
        let back = serde_json::from_str::<WaveWire>(&serde_json::to_string(&WaveWire::of(&w, None)).unwrap()).unwrap().waveform();
        assert_eq!(back.peaks.len(), 4);
        for (a, b) in w.rms.iter().zip(&back.rms) {
            assert!((a - b).abs() < 1e-4);
        }
        assert_eq!(back.peaks[3], 1.0); // (levels are 0..1)
    }

    #[test]
    fn mirror_counts_the_position_on_between_polls() {
        let local = Playout::new(Engine::new_detached(), Config::default());
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut m = Playout::mirror_of(&local, tx);
        let track = Track { path: PathBuf::from("/music/a.mp3"), ..Default::default() };
        let live = |pos: f32| Live {
            state: "playing".into(),
            has_track: true,
            pos,
            total: 200.0,
            on_air: Some(track.clone()),
            now_ms: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };
        m.apply_live(live(10.0));
        std::thread::sleep(std::time::Duration::from_millis(300));
        m.tick();
        assert!(m.status.pos > 10.2 && m.status.pos < 10.6, "{}", m.status.pos);
        // a poll that is a little behind doesn't pull it back (no stutter)…
        let before = m.status.pos;
        m.apply_live(live(10.2));
        assert!(m.status.pos >= before);
        // …but a seek does
        m.apply_live(live(90.0));
        assert!((m.status.pos - 90.0).abs() < 0.1);
        // paused: shown as the server has it
        let mut p = live(42.0);
        p.state = "paused".into();
        m.apply_live(p);
        m.tick();
        assert_eq!(m.status.pos, 42.0);
    }

    #[test]
    fn mirror_keeps_hand_set_cues_it_sent() {
        let local = Playout::new(Engine::new_detached(), Config::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut m = Playout::mirror_of(&local, tx);
        let p = PathBuf::from("/music/a.mp3");
        m.set_manual_cues(&p, Cues::plain(100.0), 100.0);
        assert!(matches!(rx.try_recv(), Ok(Cmd::SetCues { .. })));
        assert!(m.manual_cues_for(&p));
        // a waveform arriving later doesn't overwrite them
        let w = Waveform { peaks: vec![0.5; 100], rms: vec![0.3; 100] };
        m.put_remote_wave(p.clone(), Ok(WaveWire::of(&w, None)));
        assert!(m.manual_cues_for(&p));
        assert!(m.waves.contains_key(&p));
        m.put_remote_wave(PathBuf::from("/music/b.mp3"), Err("nope".into()));
        assert_eq!(m.wave_error(Path::new("/music/b.mp3")), Some("nope"));
    }

    #[test]
    fn local_settings_stay_put() {
        let mut here = Config::default();
        here.language = "de".into();
        here.remote.port = 1234;
        let mut theirs = Config::default();
        theirs.crossfade_secs = 7.0;
        theirs.language = "fr".into();
        keep_local(&mut theirs, &here);
        assert_eq!(theirs.language, "de");
        assert_eq!(theirs.remote.port, 1234);
        assert_eq!(theirs.crossfade_secs, 7.0);
        assert_eq!(digest(&station_cfg(&here)), digest(&station_cfg(&Config::default())));
    }
}
