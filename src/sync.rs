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
    PlayPressed { tab: usize, selected: Option<usize> },
    PlayIndex { tab: usize, row: usize, xfade: f32 },
    PauseToggle,
    Stop,
    Next,
    Prev,
    Volume { v: f32 },
    Seek { secs: f32 },
    Cart { idx: usize },
    StopCarts,
    TabOnAir { tab: usize },
    EncoderStart { i: usize },
    EncoderStop { i: usize },
    Command { tab: usize, text: String },
    Event { event: Event },
    Generate { tab: usize, name: String, append: bool },
    FireRule { tab: usize, id: u64, after: Option<usize> },
    Rescan,
    WriteTags { track: Track },
    SetCues { path: PathBuf, cues: Cues, duration: f32 },
    ClearCues { path: PathBuf },
    ReanalyseCues,
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
}

/// The server's own settings, as the app's Server settings window shows them.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ServerInfo {
    pub name: String,
    pub user: String,
    pub version: String,
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
        }
    }

    pub fn apply_live(&mut self, l: Live) {
        self.state = match l.state.as_str() {
            "playing" => PlayState::Playing,
            "paused" => PlayState::Paused,
            _ => PlayState::Stopped,
        };
        self.status = Status { has_track: l.has_track, pos: l.pos, total: l.total, peak_l: l.peak_l, peak_r: l.peak_r };
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
