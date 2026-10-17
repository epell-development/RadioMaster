//! Station brain: playlists (tabs), every playlist item type, auto-advance + crossfade, hour
//! markers, rotations (jingles/ads), scheduler, encoders and history. Runs on its own thread so
//! automation keeps working even when the window is minimized.

use crate::azuracast::{self, AzuraState};
use crate::config::{data_dir, AdBreak, AzuraConfig, Config};
use crate::dsp::DspConfig;
use crate::engine::{CueSpec, Engine, Status};
use crate::fade::{smart_cues, Cues, FadeMode, Waveform};
pub use crate::items::{ItemKind, PlEntry};
use crate::library::{self, Track};
use crate::live::{self, LogSink};
use crate::scheduler::{ActionKind, Event};
use crate::stream::{self, EncoderHandle};
use chrono::{DateTime, Duration as CDur, Local, NaiveDate, TimeZone};
use parking_lot::Mutex;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::Instant,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

pub struct HistoryItem {
    pub time: String,
    pub text: String,
}

#[derive(Clone)]
pub struct LogLine {
    pub time: String,
    pub text: String,
    pub error: bool,
}

/// A playlist tab. The tab that is on air keeps its rows in `Playout::playlist` / `current`;
/// its own slot here stays empty until another tab goes on air.
#[derive(Clone, Default)]
pub struct PlTab {
    pub name: String,
    pub entries: Vec<PlEntry>,
    pub current: Option<usize>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedTab {
    name: String,
    entries: Vec<PlEntry>,
    current: Option<usize>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedTabs {
    tabs: Vec<SavedTab>,
    playing: usize,
}

enum WriteJob {
    Write(PathBuf, String),
    Flush(mpsc::Sender<()>),
}

fn writer() -> Option<std::sync::MutexGuard<'static, mpsc::Sender<WriteJob>>> {
    use std::sync::OnceLock;
    static TX: OnceLock<std::sync::Mutex<mpsc::Sender<WriteJob>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<WriteJob>();
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                match job {
                    WriteJob::Write(p, d) => {
                        let tmp = p.with_extension("tmp");
                        if std::fs::write(&tmp, d).is_ok() {
                            let _ = std::fs::rename(&tmp, &p);
                        }
                    }
                    WriteJob::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        std::sync::Mutex::new(tx)
    })
    .lock()
    .ok()
}

/// Write a file off the playout lock (the disk can be slow; playback and the UI must not wait).
/// Writes go through one thread, in order, and land atomically via a temporary file.
fn write_in_background(path: PathBuf, data: String) {
    if let Some(tx) = writer() {
        if tx.send(WriteJob::Write(path.clone(), data.clone())).is_ok() {
            return;
        }
    }
    let _ = std::fs::write(path, data);
}

/// Wait until every queued write has reached the disk (used when the program closes).
pub fn flush_writes() {
    let (tx, rx) = mpsc::channel();
    if let Some(w) = writer() {
        if w.send(WriteJob::Flush(tx)).is_err() {
            return;
        }
    }
    let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
}

/// Projected air time of a playlist row.
#[derive(Clone, Copy, Debug)]
pub struct Start {
    pub at: DateTime<Local>,
    /// will be skipped because an hour marker comes first
    pub skipped: bool,
}

enum ScanMsg {
    Progress(usize),
    Done(Vec<Track>),
}

/// The date-time a marker for hour:minute refers to, as seen from `reference`
/// (the nearest such time, so 0:00 seen from 23:50 is midnight tomorrow).
pub fn marker_target(hour: u32, minute: u32, reference: DateTime<Local>) -> DateTime<Local> {
    let naive = reference.date_naive().and_hms_opt(hour.min(23), minute.min(59), 0).unwrap_or_default();
    let mut t = Local.from_local_datetime(&naive).earliest().unwrap_or(reference);
    if t < reference - CDur::hours(12) {
        t += CDur::days(1);
    } else if t > reference + CDur::hours(12) {
        t -= CDur::days(1);
    }
    t
}

/// File paths listed in an .m3u/.m3u8 file (relative paths resolved against its folder).
pub fn m3u_paths(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let p = PathBuf::from(l);
            if p.is_absolute() {
                p
            } else {
                base.join(p)
            }
        })
        .collect()
}

pub struct Playout {
    pub cfg: Config,
    pub engine: Engine,
    pub library: Vec<Track>,
    pub library_ver: u64,
    lib_index: HashMap<PathBuf, usize>,
    pub scanning: bool,
    pub scan_progress: usize,
    scan_rx: Option<mpsc::Receiver<ScanMsg>>,
    /// rows of the playlist tab that is on air (or, inside `with_tab`, the tab being worked on)
    pub playlist: Vec<PlEntry>,
    pub current: Option<usize>,
    pub state: PlayState,
    pub status: Status,
    triggered_next: bool,
    pub handles: Vec<Option<EncoderHandle>>,
    pub plugins: crate::plugins::Plugins,
    /// hour of the last "hour" plugin event
    plugin_hour: u32,
    /// last state of each encoder, for "encoder" plugin events
    enc_states: Vec<String>,
    pub now_text: Arc<Mutex<String>>,
    pub azura: Arc<Mutex<AzuraState>>,
    azura_shared: Arc<Mutex<AzuraConfig>>,
    pub history: Vec<HistoryItem>,
    fired: HashMap<usize, String>,
    songs_since: Vec<u32>,
    pub notify: Arc<Mutex<String>>,
    last_save: Instant,
    dsp_shared: DspConfig,
    fired_ads: HashMap<usize, String>,
    auto_check: Instant,
    /// cue points per file (manual from the track editor, or computed by smart fading)
    pub cues: HashMap<PathBuf, Cues>,
    cues_dirty: bool,
    /// waveforms analysed so far (most recent kept)
    pub waves: HashMap<PathBuf, Arc<Waveform>>,
    wave_order: Vec<PathBuf>,
    pending: std::collections::HashSet<PathBuf>,
    analyze_tx: mpsc::Sender<PathBuf>,
    analyze_rx: mpsc::Receiver<(PathBuf, Option<Waveform>)>,
    /// cues of the track on air (what the smart trigger follows)
    pub active_cues: Option<Cues>,
    // ---- playlists
    pub tabs: Vec<PlTab>,
    /// index of the tab that is on air
    pub playing_tab: usize,
    /// set when playback starts from a list swapped in by `with_tab`: that tab goes on air
    adopt: bool,
    pending_tab: Option<usize>,
    // ---- what is on air
    /// file on air (None for streams, line input, pauses…)
    pub on_air: Option<Track>,
    /// text shown in the On Air box
    pub on_air_label: String,
    /// waiting (timed pause / early hour marker) until this time
    pub wait_until: Option<DateTime<Local>>,
    pub wait_label: String,
    /// the rest of a container that is playing
    sub_queue: VecDeque<Track>,
    /// a soft hour marker fired: continue from this row after the current track
    pending_jump: Option<usize>,
    marker_fired: HashMap<(u32, u32), NaiveDate>,
    /// last time each file / artist was played (unix seconds) for repeat protection and "Last played"
    pub plays: HashMap<PathBuf, i64>,
    artist_plays: HashMap<String, i64>,
    plays_dirty: bool,
    // ---- log
    pub log: Vec<LogLine>,
    pub log_sink: LogSink,
}

impl Playout {
    pub fn new(engine: Engine, cfg: Config) -> Self {
        engine.set_dsp(&cfg.dsp);
        let dsp_shared = cfg.dsp.clone();
        let azura_shared = Arc::new(Mutex::new(cfg.azuracast.clone()));
        let azura = Arc::new(Mutex::new(AzuraState::default()));
        azuracast::spawn_poller(azura_shared.clone(), azura.clone());
        let log_sink: LogSink = Arc::new(Mutex::new(vec![]));
        let mut plugins = crate::plugins::Plugins::new(log_sink.clone());
        if !cfg.plugins_off.is_empty() {
            plugins.reload(&cfg.plugins_off);
        }
        let library: Vec<Track> =
            std::fs::read_to_string(data_dir().join("library.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let n_enc = cfg.encoders.len();
        // background analyser: decodes files and builds waveforms for smart fading / the editor
        let (analyze_tx, job_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, analyze_rx) = mpsc::channel::<(PathBuf, Option<Waveform>)>();
        thread::spawn(move || {
            while let Ok(path) = job_rx.recv() {
                let w = Waveform::analyze(&path).ok();
                if res_tx.send((path, w)).is_err() {
                    break;
                }
            }
        });
        let cues: HashMap<PathBuf, Cues> = std::fs::read_to_string(data_dir().join("cues.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<(PathBuf, Cues)>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        let plays: HashMap<PathBuf, i64> =
            std::fs::read_to_string(data_dir().join("plays.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut p = Playout {
            cfg,
            engine,
            library,
            library_ver: 1,
            lib_index: HashMap::new(),
            scanning: false,
            scan_progress: 0,
            scan_rx: None,
            playlist: vec![],
            current: None,
            state: PlayState::Stopped,
            status: Status::default(),
            triggered_next: false,
            handles: (0..n_enc).map(|_| None).collect(),
            now_text: Arc::new(Mutex::new(String::new())),
            azura,
            azura_shared,
            history: vec![],
            fired: HashMap::new(),
            songs_since: vec![],
            notify: Arc::new(Mutex::new(String::new())),
            last_save: Instant::now(),
            plugin_hour: chrono::Timelike::hour(&Local::now()),
            enc_states: vec![],
            dsp_shared,
            fired_ads: HashMap::new(),
            auto_check: Instant::now(),
            cues,
            cues_dirty: false,
            waves: HashMap::new(),
            wave_order: vec![],
            pending: Default::default(),
            analyze_tx,
            analyze_rx,
            active_cues: None,
            tabs: vec![PlTab { name: "Main".into(), ..Default::default() }],
            playing_tab: 0,
            adopt: false,
            pending_tab: None,
            on_air: None,
            on_air_label: String::new(),
            wait_until: None,
            wait_label: String::new(),
            sub_queue: VecDeque::new(),
            pending_jump: None,
            marker_fired: HashMap::new(),
            plays,
            artist_plays: HashMap::new(),
            plays_dirty: false,
            log: vec![],
            log_sink,
            plugins,
        };
        p.reindex();
        p.load_tabs();
        for i in 0..n_enc {
            if p.cfg.encoders[i].start_with_app {
                p.start_encoder(i);
            }
        }
        p
    }

    /// Status-bar message; also written to the log.
    pub fn say(&self, s: impl Into<String>) {
        let s: String = s.into();
        *self.notify.lock() = s.clone();
        self.log_sink.lock().push(s);
    }

    fn drain_log(&mut self) {
        let msgs: Vec<String> = std::mem::take(&mut *self.log_sink.lock());
        for m in msgs {
            let l = m.to_lowercase();
            let error = ["fail", "error", "cannot", "could not", "unsupported", "unknown command"].iter().any(|k| l.contains(k));
            self.log.push(LogLine { time: Local::now().format("%H:%M:%S").to_string(), text: m, error });
        }
        let n = self.log.len();
        if n > 500 {
            self.log.drain(0..n - 500);
        }
    }

    // ---------------------------------------------------------------- tick
    pub fn tick(&mut self) {
        self.drain_scan();
        self.drain_log();
        {
            let mut sh = self.azura_shared.lock();
            if *sh != self.cfg.azuracast {
                *sh = self.cfg.azuracast.clone();
            }
        }
        let st = self.engine.status();
        let ended = self.engine.take_ended();
        self.status = st.clone();

        self.drain_analysis();

        if let Some(t) = self.pending_tab.take() {
            self.put_tab_on_air(t);
        }

        if self.state == PlayState::Playing {
            let xf = self.cfg.crossfade_secs;
            let teaser = self.cfg.teaser_secs;
            if let Some(until) = self.wait_until {
                // timed pause or early hour marker: silence until the time comes
                if Local::now() >= until {
                    self.wait_until = None;
                    if !self.advance(0.0) {
                        self.state = PlayState::Stopped;
                    }
                }
            } else if ended {
                if !self.advance(0.0) {
                    self.state = PlayState::Stopped;
                }
            } else if let (Some(c), false, true) = (self.active_cues, self.triggered_next, teaser <= 0.0) {
                // cue-driven (smart or hand-edited) transition: bring the next track in at the mix point;
                // the outgoing track finishes on its own fade / cue out
                if st.pos >= c.mix && c.mix < c.cue_out - 0.01 {
                    self.triggered_next = true;
                    self.advance_cued();
                }
            } else if teaser > 0.0 && !self.triggered_next && st.pos >= teaser {
                self.triggered_next = true;
                self.advance(xf.min(2.0));
            } else if self.active_cues.is_none() && xf > 0.05 && !self.triggered_next && st.total > xf + 1.0 && st.total - st.pos <= xf {
                self.triggered_next = true;
                self.advance(xf);
            }
            if self.state == PlayState::Playing && self.wait_until.is_none() {
                self.check_markers();
            }
        }

        // scheduler
        let now = Local::now();
        let key = now.format("%Y%m%d%H%M").to_string();
        let due: Vec<(usize, Event)> = self
            .cfg
            .events
            .iter()
            .enumerate()
            .filter(|(i, e)| e.due(&now) && self.fired.get(i) != Some(&key))
            .map(|(i, e)| (i, e.clone()))
            .collect();
        for (i, e) in due {
            self.fired.insert(i, key.clone());
            self.exec(&e);
        }

        // ad breaks
        let breaks: Vec<(usize, AdBreak)> = self
            .cfg
            .ad_breaks
            .iter()
            .enumerate()
            .filter(|(i, b)| b.due(&now) && self.fired_ads.get(i) != Some(&key))
            .map(|(i, b)| (i, b.clone()))
            .collect();
        for (i, b) in breaks {
            self.fired_ads.insert(i, key.clone());
            self.fire_ad_break(&b, &now);
        }

        // DSP settings → engine
        if self.dsp_shared != self.cfg.dsp {
            self.dsp_shared = self.cfg.dsp.clone();
            self.engine.set_dsp(&self.dsp_shared);
        }

        // auto-playlist: keep the queue filled
        if self.cfg.auto_enabled && self.auto_check.elapsed().as_secs() >= 2 {
            self.auto_check = Instant::now();
            let start = self.current.map(|c| c + 1).unwrap_or(0);
            let mut rem: f32 = self.playlist.iter().skip(start).map(|e| e.est_secs()).sum();
            if self.state == PlayState::Playing {
                rem += (self.status.total - self.status.pos).max(0.0);
            }
            if rem < self.cfg.auto_min_mins as f32 * 60.0 && !self.library.is_empty() {
                let name = self.cfg.auto_template.clone();
                if self.generate_playlist(&name, true) == 0 {
                    self.cfg.auto_enabled = false;
                } else {
                    self.say("Auto-playlist: queue topped up");
                }
            }
        }

        // plugins: commands they sent, the hourly event, encoder changes
        for (from, cmd) in self.plugins.take_commands() {
            self.say(format!("{from}: {cmd}"));
            self.run_command(&cmd);
        }
        let h = chrono::Timelike::hour(&now);
        if h != self.plugin_hour {
            self.plugin_hour = h;
            self.plugin_event("hour", json!({ "hour": h }));
        }
        let states: Vec<String> = self
            .handles
            .iter()
            .map(|h| match h {
                Some(h) => match &h.status.lock().state {
                    crate::stream::EncState::Live => "connected".to_string(),
                    crate::stream::EncState::Error(e) => format!("error: {e}"),
                    crate::stream::EncState::Connecting => "connecting".to_string(),
                    crate::stream::EncState::Idle => "stopped".to_string(),
                },
                None => "stopped".to_string(),
            })
            .collect();
        if states != self.enc_states {
            for (i, st) in states.iter().enumerate() {
                if self.enc_states.get(i) != Some(st) && st != "connecting" && !(self.enc_states.get(i).is_none() && st == "stopped") {
                    let name = self.cfg.encoders.get(i).map(|e| e.name.clone()).unwrap_or_default();
                    self.plugin_event("encoder", json!({ "encoder": i + 1, "name": name, "state": st }));
                }
            }
            self.enc_states = states;
        }

        if self.last_save.elapsed().as_secs() >= 60 {
            self.save_all();
        }
    }

    pub fn save_all(&mut self) {
        self.cfg.save();
        if self.cues_dirty {
            let v: Vec<(&PathBuf, &Cues)> = self.cues.iter().collect();
            if let Ok(j) = serde_json::to_string(&v) {
                write_in_background(data_dir().join("cues.json"), j);
            }
            self.cues_dirty = false;
        }
        if self.plays_dirty {
            if let Ok(j) = serde_json::to_string(&self.plays) {
                write_in_background(data_dir().join("plays.json"), j);
            }
            self.plays_dirty = false;
        }
        self.save_tabs();
        self.last_save = Instant::now();
    }

    // ------------------------------------------------------- playlist tabs
    fn load_tabs(&mut self) {
        let saved: Option<SavedTabs> = std::fs::read_to_string(data_dir().join("playlists.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .filter(|t: &SavedTabs| !t.tabs.is_empty());
        let Some(saved) = saved else {
            // first run with tabs: the old single playlist becomes "Main"
            self.load_m3u(&data_dir().join("last.m3u"));
            return;
        };
        let mut tabs: Vec<PlTab> = saved
            .tabs
            .into_iter()
            .map(|t| {
                let current = t.current.filter(|&c| c < t.entries.len());
                PlTab { name: t.name, entries: t.entries, current }
            })
            .collect();
        let playing = saved.playing.min(tabs.len() - 1);
        self.playlist = std::mem::take(&mut tabs[playing].entries);
        self.current = tabs[playing].current.take();
        self.tabs = tabs;
        self.playing_tab = playing;
    }

    pub fn save_tabs(&self) {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (entries, current) = if i == self.playing_tab { (&self.playlist, self.current) } else { (&t.entries, t.current) };
                SavedTab { name: t.name.clone(), entries: entries.clone(), current }
            })
            .collect();
        if let Ok(j) = serde_json::to_string(&SavedTabs { tabs, playing: self.playing_tab }) {
            write_in_background(data_dir().join("playlists.json"), j);
        }
    }

    fn swap_tab(&mut self, v: usize) {
        std::mem::swap(&mut self.playlist, &mut self.tabs[v].entries);
        std::mem::swap(&mut self.current, &mut self.tabs[v].current);
    }

    /// Run `f` with tab `v` temporarily in place of the on-air list (so every playlist edit works on
    /// any tab). If `f` starts playback from it, tab `v` becomes the on-air tab.
    pub fn with_tab<R>(&mut self, v: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if v == self.playing_tab || v >= self.tabs.len() {
            self.adopt = false;
            return f(self);
        }
        let old = self.playing_tab;
        self.swap_tab(v);
        self.adopt = false;
        let r = f(self);
        if self.adopt {
            // tab v is now on air; the previous on-air list moves back into its own slot
            let e = std::mem::take(&mut self.tabs[v].entries);
            let c = self.tabs[v].current.take();
            self.tabs[old].entries = e;
            self.tabs[old].current = c;
            self.playing_tab = v;
        } else {
            self.swap_tab(v);
        }
        self.adopt = false;
        r
    }

    /// Rows of any tab (read only).
    pub fn tab_rows(&self, i: usize) -> &[PlEntry] {
        if i == self.playing_tab {
            &self.playlist
        } else {
            self.tabs.get(i).map(|t| t.entries.as_slice()).unwrap_or(&[])
        }
    }

    pub fn tab_len(&self, i: usize) -> usize {
        self.tab_rows(i).len()
    }

    pub fn tab_secs(&self, i: usize) -> f32 {
        self.tab_rows(i).iter().map(|x| x.est_secs()).sum()
    }

    pub fn new_tab(&mut self, name: Option<String>) -> usize {
        let used: std::collections::HashSet<String> = self.tabs.iter().map(|t| t.name.clone()).collect();
        let name = name.unwrap_or_else(|| (1..).map(|n| format!("PL{n}")).find(|n| !used.contains(n)).unwrap());
        self.tabs.push(PlTab { name, ..Default::default() });
        self.tabs.len() - 1
    }

    pub fn duplicate_tab(&mut self, i: usize) -> usize {
        let entries: Vec<PlEntry> = self.tab_rows(i).iter().map(|e| e.fresh()).collect();
        let name = format!("{} copy", self.tabs[i].name);
        self.tabs.push(PlTab { name, entries, current: None });
        self.tabs.len() - 1
    }

    /// Close a tab. The on-air tab and the last remaining tab can't be closed.
    pub fn close_tab(&mut self, i: usize) -> bool {
        if i == self.playing_tab || self.tabs.len() <= 1 || i >= self.tabs.len() {
            return false;
        }
        self.tabs.remove(i);
        if i < self.playing_tab {
            self.playing_tab -= 1;
        }
        true
    }

    /// Append rows to any tab (the on-air one included).
    pub fn append_to_tab(&mut self, i: usize, rows: Vec<PlEntry>) {
        if i == self.playing_tab {
            self.playlist.extend(rows);
        } else if let Some(t) = self.tabs.get_mut(i) {
            t.entries.extend(rows);
        }
    }

    /// Move tab `from` to position `to` in the tab strip.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        let p = self.playing_tab;
        self.playing_tab = if p == from {
            to
        } else if from < p && to >= p {
            p - 1
        } else if from > p && to <= p {
            p + 1
        } else {
            p
        };
    }

    /// Put another playlist on air, continuing from its first unplayed row.
    pub fn put_tab_on_air(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let xf = if self.state == PlayState::Playing { self.cfg.crossfade_secs } else { 0.0 };
        self.with_tab(i, |c| {
            let from = c.current.filter(|&r| r < c.playlist.len()).unwrap_or(0);
            let start = (from..c.playlist.len())
                .find(|&r| !c.playlist[r].played)
                .or_else(|| (0..c.playlist.len()).find(|&r| !c.playlist[r].played))
                .unwrap_or(0);
            c.play_index(start, xf);
        });
    }

    // ------------------------------------------------------------ transport
    fn next_index(&mut self) -> Option<usize> {
        let n = self.playlist.len();
        if let Some(m) = self.pending_jump.take() {
            if m < n {
                return Some(m);
            }
        }
        let nxt = match self.current {
            Some(c) if c + 1 < n => Some(c + 1),
            None if n > 0 => Some(0),
            _ => None,
        };
        if nxt.is_some() {
            return nxt;
        }
        if self.cfg.loop_playlist && n > 0 {
            self.playlist.iter_mut().for_each(|e| e.played = false);
            return Some(0);
        }
        if self.cfg.random_fill {
            if let Some(t) = self.library.choose(&mut rand::thread_rng()).cloned() {
                self.playlist.push(PlEntry::file(t));
                return Some(self.playlist.len() - 1);
            }
        }
        None
    }

    /// Move to the next playable item. Returns false if nothing could be started.
    pub fn advance(&mut self, xfade: f32) -> bool {
        self.advance_inner(Some(xfade))
    }

    /// Advance at a cue mix point: the outgoing track keeps its own fade.
    fn advance_cued(&mut self) -> bool {
        self.advance_inner(None)
    }

    fn advance_inner(&mut self, old_fade: Option<f32>) -> bool {
        self.wait_until = None;
        // the rest of a container comes first
        while let Some(t) = self.sub_queue.pop_front() {
            if self.play_file(&t, old_fade) {
                return true;
            }
        }
        // comments, commands and late hour markers are passed over without playing anything
        for _ in 0..256 {
            match self.next_index() {
                Some(i) => {
                    if self.start_index(i, old_fade) {
                        return true;
                    }
                }
                None => return false,
            }
        }
        false
    }

    /// Play a row now (double-click / Play).
    pub fn play_index(&mut self, i: usize, xfade: f32) -> bool {
        self.sub_queue.clear();
        self.pending_jump = None;
        self.wait_until = None;
        if self.start_index(i, Some(xfade)) {
            return true;
        }
        // the row itself plays nothing (comment, command…): carry on with what follows it
        self.advance(xfade)
    }

    fn start_index(&mut self, i: usize, old_fade: Option<f32>) -> bool {
        if i >= self.playlist.len() {
            return false;
        }
        self.current = Some(i);
        self.playlist[i].played = true;
        let e = self.playlist[i].clone();
        let xf = old_fade.unwrap_or(0.0);
        let ok = match e.kind.clone() {
            ItemKind::Track | ItemKind::VoiceTrack => {
                if self.play_file(&e.track, old_fade) {
                    self.apply_rotations(i, &e.track);
                    if let Some(n) = self.playlist.get(i + 1).filter(|x| x.is_file()).map(|x| x.track.path.clone()) {
                        self.request_analysis(&n); // pre-analyse the next track
                    }
                    true
                } else {
                    false
                }
            }
            ItemKind::Url { url, secs } => {
                let src = live::url_source(&url, self.log_sink.clone());
                let spec = CueSpec { cue_out: (secs > 0.0).then_some(secs), ..Default::default() };
                self.engine.play_source(Box::new(src), None, secs, spec, Some(xf), true);
                self.started_other(e.title());
                true
            }
            ItemKind::LineIn { secs } => {
                let src = live::line_source(&self.cfg.audio_devices.input, self.log_sink.clone());
                let spec = CueSpec { cue_out: (secs > 0.0).then_some(secs), ..Default::default() };
                self.engine.play_source(Box::new(src), None, secs, spec, Some(xf), true);
                self.started_other(e.title());
                true
            }
            ItemKind::Command(c) => {
                self.run_command(&c);
                false
            }
            ItemKind::Pause(secs) => {
                self.engine.stop(xf.max(0.3));
                self.start_wait(Local::now() + CDur::milliseconds((secs.max(0.0) * 1000.0) as i64), e.title());
                true
            }
            ItemKind::Teaser { count, secs } => {
                let parts: Vec<(PathBuf, f32)> = self
                    .playlist
                    .iter()
                    .skip(i + 1)
                    .filter(|x| x.is_file())
                    .take(count.max(1) as usize)
                    .map(|x| (x.track.path.clone(), x.track.duration * 0.35))
                    .collect();
                match live::teaser_source(&parts, secs) {
                    Ok(src) => {
                        self.engine.play_source(src, None, parts.len() as f32 * secs, CueSpec::default(), Some(xf), false);
                        self.started_other(e.title());
                        true
                    }
                    Err(err) => {
                        self.say(format!("Teaser skipped: {err}"));
                        false
                    }
                }
            }
            ItemKind::TimeAnnounce => {
                let text = live::announce_text(&self.cfg.announce.template, &Local::now());
                let a = &self.cfg.announce;
                let src = live::announce_source(
                    text.clone(),
                    a.folder.clone(),
                    a.voice.clone(),
                    a.rate,
                    data_dir().join("tmp").join("time.wav"),
                    self.log_sink.clone(),
                );
                self.engine.play_source(Box::new(src), None, 0.0, CueSpec::default(), Some(xf), false);
                self.started_other(format!("Time announcement — {text}"));
                true
            }
            ItemKind::Comment(_) | ItemKind::HourInfo { .. } => false,
            ItemKind::Tts { text, voice, rate } => {
                let said = self.fill_speech(&text, i);
                let voice = if voice.is_empty() { self.cfg.announce.voice.clone() } else { voice };
                let src = live::announce_source(
                    said.clone(),
                    String::new(),
                    voice,
                    rate,
                    data_dir().join("tmp").join(format!("speech{}.wav", i % 4)),
                    self.log_sink.clone(),
                );
                self.engine.play_source(Box::new(src), None, 0.0, CueSpec::default(), Some(xf), false);
                self.started_other(format!("Speech — {said}"));
                true
            }
            ItemKind::Container { items, random } => {
                let mut files: Vec<PathBuf> = items.into_iter().filter(|p| p.exists()).collect();
                if files.is_empty() {
                    self.say(format!("{}: no playable files", e.title()));
                    false
                } else if random {
                    let pick = files.choose(&mut rand::thread_rng()).cloned().unwrap();
                    let t = self.entry_for(&pick).track;
                    self.play_file(&t, old_fade)
                } else {
                    let first = files.remove(0);
                    let t = self.entry_for(&first).track;
                    if self.play_file(&t, old_fade) {
                        self.sub_queue = files.iter().map(|p| self.entry_for(p).track).collect();
                        true
                    } else {
                        false
                    }
                }
            }
            ItemKind::TrackList { source } => match self.pick_from_list(&source) {
                Some(t) => self.play_file(&t, old_fade),
                None => {
                    self.say(format!("{}: nothing to play", e.title()));
                    false
                }
            },
            ItemKind::HourMarker { hour, minute, wait, .. } => {
                let now = Local::now();
                let target = marker_target(hour, minute, now);
                self.marker_fired.insert((hour, minute), target.date_naive());
                if wait && now < target {
                    self.engine.stop(xf.max(0.5));
                    self.start_wait(target, format!("Waiting for {}", e.title()));
                    true
                } else {
                    false
                }
            }
        };
        if ok {
            self.adopt = true; // started: if this list was swapped in by `with_tab`, it is now on air
        }
        ok
    }

    /// Start an audio file with its cue points (smart or hand-edited).
    fn play_file(&mut self, t: &Track, old_fade: Option<f32>) -> bool {
        let cues = self.cues_for(&t.path);
        let spec = match cues {
            Some(c) => CueSpec {
                cue_in: c.cue_in,
                cue_out: Some(c.cue_out),
                fade_in: c.fade_in,
                fade_out_at: Some(c.fade_out),
                gain_db: c.gain_db,
            },
            None => CueSpec { fade_in: old_fade.unwrap_or(0.0), ..Default::default() },
        };
        // a manual "play" (old_fade = Some) still lets the previous track fade normally
        if let Err(e) = self.engine.play_cued(&t.path, t.duration, spec, old_fade) {
            self.say(format!("Cannot play {}: {e}", t.path.display()));
            return false;
        }
        self.active_cues = cues;
        self.request_analysis(&t.path);
        let text = t.label();
        self.on_air = Some(t.clone());
        self.after_start(text.clone());
        self.push_metadata(&text);
        self.export_now(t);
        let now = Local::now().timestamp();
        self.plays.insert(t.path.clone(), now);
        if !t.artist.trim().is_empty() {
            self.artist_plays.insert(t.artist.trim().to_lowercase(), now);
        }
        self.plays_dirty = true;
        let data = json!({
            "title": t.title, "artist": t.artist, "album": t.album, "genre": t.genre, "year": t.year,
            "path": t.path.to_string_lossy(), "duration": t.duration, "text": text,
        });
        self.plugin_event("track_started", data);
        true
    }

    /// Send an event to the plugins.
    pub fn plugin_event(&mut self, event: &str, data: serde_json::Value) {
        let off = self.cfg.plugins_off.clone();
        self.plugins.emit(event, data, &off);
    }

    /// Something other than a file started (stream, line input, teaser…).
    fn started_other(&mut self, label: String) {
        self.active_cues = None;
        self.on_air = None;
        self.after_start(label.clone());
        self.push_metadata(&label);
        self.plugin_event("item_started", json!({ "text": label, "title": label }));
    }

    fn after_start(&mut self, label: String) {
        self.state = PlayState::Playing;
        self.triggered_next = false;
        self.wait_until = None;
        self.on_air_label = label.clone();
        *self.now_text.lock() = label.clone();
        self.log_history(&label);
    }

    fn start_wait(&mut self, until: DateTime<Local>, label: String) {
        self.active_cues = None;
        self.on_air = None;
        self.on_air_label = String::new();
        self.state = PlayState::Playing;
        self.triggered_next = false;
        self.wait_until = Some(until);
        self.wait_label = label;
    }

    /// Hour markers: when a marker's time arrives while earlier rows are still playing,
    /// a hard marker cuts in now and a soft one continues from the marker after the current track.
    fn check_markers(&mut self) {
        let now = Local::now();
        let from = self.current.map(|c| c + 1).unwrap_or(0);
        let Some((j, hour, minute, hard)) = (from..self.playlist.len()).find_map(|j| match self.playlist[j].kind {
            ItemKind::HourMarker { hour, minute, hard, .. } => Some((j, hour, minute, hard)),
            _ => None,
        }) else {
            return;
        };
        let target = marker_target(hour, minute, now);
        let late = now - target;
        if late < CDur::zero() || late > CDur::minutes(60) || self.marker_fired.get(&(hour, minute)) == Some(&target.date_naive()) {
            return;
        }
        self.marker_fired.insert((hour, minute), target.date_naive());
        if hard {
            self.say(format!("Hour marker {hour}:{minute:02}: cutting in on time"));
            self.sub_queue.clear();
            self.current = Some(j);
            self.playlist[j].played = true;
            let xf = self.cfg.crossfade_secs.max(1.5);
            if !self.advance(xf) {
                self.stop();
            }
        } else {
            self.say(format!("Hour marker {hour}:{minute:02}: continuing from the marker after this track"));
            self.sub_queue.clear();
            self.pending_jump = Some(j);
        }
    }

    /// Expected time from a row's start until the next row starts.
    pub fn row_span(&self, e: &PlEntry) -> f32 {
        match e.kind {
            ItemKind::Track | ItemKind::VoiceTrack => self.span_of(&e.track),
            _ => e.est_secs(),
        }
    }

    /// Approximate air time of every row of the list currently in `playlist`
    /// (`on_air` = it is the list that is playing). Hour markers wait or skip rows.
    pub fn projected_starts(&self, on_air: bool) -> Vec<Option<Start>> {
        let n = self.playlist.len();
        let mut out: Vec<Option<Start>> = vec![None; n];
        if n == 0 {
            return out;
        }
        let now = Local::now();
        let ms = |s: f32| CDur::milliseconds((s.max(0.0) * 1000.0) as i64);
        let st = &self.status;
        let live = on_air && matches!(self.state, PlayState::Playing | PlayState::Paused);
        let xf = self.cfg.crossfade_secs;
        let (first, mut t) = match self.current {
            Some(c) if live && c < n => {
                if let Some(u) = self.wait_until {
                    out[c] = Some(Start { at: u, skipped: false });
                    (c + 1, u.max(now))
                } else if st.has_track {
                    out[c] = Some(Start { at: now - ms(st.pos), skipped: false });
                    let next_at = match self.active_cues {
                        Some(cu) => cu.mix - st.pos,
                        None if st.total > 0.0 => st.total - st.pos - xf.min(st.total * 0.5),
                        None => self.playlist[c].est_secs() - st.pos,
                    };
                    let rest: f32 = self.sub_queue.iter().map(|t| self.span_of(t)).sum();
                    (c + 1, now + ms(next_at + rest))
                } else {
                    (c + 1, now)
                }
            }
            Some(c) if c < n => (c, now), // stopped: Play resumes here
            _ => (0, now),
        };
        let mut since = first;
        for i in first..n {
            if let ItemKind::HourMarker { hour, minute, hard, wait } = self.playlist[i].kind {
                let target = marker_target(hour, minute, t);
                let late = (since..i).find(|&r| out[r].is_some_and(|s| s.at > target) && self.row_span(&self.playlist[r]) > 0.0);
                if let Some(r0) = late {
                    // rows that would start after the marker time are skipped
                    let resume = if hard { target } else { out[r0].unwrap().at };
                    for s in out[r0..i].iter_mut().flatten() {
                        s.skipped = true;
                    }
                    t = resume;
                } else if wait && t < target {
                    t = target;
                }
                out[i] = Some(Start { at: t, skipped: false });
                since = i + 1;
            } else {
                out[i] = Some(Start { at: t, skipped: false });
                t += ms(self.row_span(&self.playlist[i]));
            }
        }
        out
    }

    /// Where a row for time `h:m` belongs: before the first upcoming row that starts at or after it.
    pub fn row_for_time(&self, h: u32, m: u32, on_air: bool) -> usize {
        let starts = self.projected_starts(on_air);
        let target = marker_target(h, m, Local::now());
        let from = if on_air { self.current.map(|c| c + 1).unwrap_or(0) } else { 0 };
        (from..self.playlist.len()).find(|&i| starts[i].is_some_and(|s| !s.skipped && s.at >= target)).unwrap_or(self.playlist.len())
    }

    /// Keep automatic "next hour" rows where each new hour starts. Returns, when something moved,
    /// the new index of every old row (None = an automatic row that was removed).
    pub fn refresh_hour_info(&mut self, on_air: bool) -> Option<Vec<Option<usize>>> {
        let is_auto = |e: &PlEntry| matches!(e.kind, ItemKind::HourInfo { auto: true, .. });
        let starts = self.projected_starts(on_air);
        let from = if on_air && self.state != PlayState::Stopped { self.current.map(|c| c + 1).unwrap_or(0) } else { 0 };
        // wanted: (index of the real row it goes before, hour)
        let mut want: Vec<(usize, u32)> = vec![];
        let mut prev: Option<DateTime<Local>> = if on_air && self.state != PlayState::Stopped {
            self.current.and_then(|c| starts.get(c).copied().flatten()).map(|s| s.at)
        } else {
            None
        };
        for i in from..self.playlist.len() {
            let e = &self.playlist[i];
            if is_auto(e) || matches!(e.kind, ItemKind::HourMarker { .. } | ItemKind::HourInfo { .. }) {
                continue;
            }
            let Some(st) = starts[i].filter(|s| !s.skipped) else { continue };
            if let Some(p) = prev {
                let hour_of = |t: DateTime<Local>| t.format("%Y%m%d%H").to_string();
                if hour_of(p) != hour_of(st.at) {
                    // only one row per hour change, and none right after a manual marker for that hour
                    let manual =
                        i > 0 && matches!(self.playlist[i - 1].kind, ItemKind::HourMarker { .. } | ItemKind::HourInfo { auto: false, .. });
                    if !manual {
                        want.push((i, chrono::Timelike::hour(&st.at)));
                    }
                }
            }
            prev = Some(st.at);
        }
        // what is there now
        let mut have: Vec<(usize, u32)> = vec![];
        for (i, e) in self.playlist.iter().enumerate() {
            if let ItemKind::HourInfo { hour, auto: true } = e.kind {
                if let Some(next) = (i + 1..self.playlist.len()).find(|&j| !is_auto(&self.playlist[j])) {
                    have.push((next, hour));
                }
            }
        }
        let stray = self.playlist.iter().enumerate().any(|(i, e)| is_auto(e) && i < from);
        if want == have && !stray {
            return None;
        }
        // rebuild: drop automatic rows, then insert the wanted ones
        let old = std::mem::take(&mut self.playlist);
        let mut map: Vec<Option<usize>> = vec![None; old.len()];
        let mut out: Vec<PlEntry> = Vec::with_capacity(old.len() + want.len());
        let mut w = want.iter().peekable();
        for (i, e) in old.into_iter().enumerate() {
            if is_auto(&e) {
                continue;
            }
            while let Some(&&(at, hour)) = w.peek() {
                if at == i {
                    out.push(PlEntry::special(ItemKind::HourInfo { hour, auto: true }, "", 0.0));
                    w.next();
                } else {
                    break;
                }
            }
            map[i] = Some(out.len());
            out.push(e);
        }
        self.playlist = out;
        self.current = self.current.and_then(|c| map.get(c).copied().flatten());
        self.pending_jump = self.pending_jump.and_then(|j| map.get(j).copied().flatten());
        Some(map)
    }

    /// Remove every automatic hour row (when the option is turned off).
    pub fn clear_hour_info(&mut self) -> Option<Vec<Option<usize>>> {
        if !self.playlist.iter().any(|e| matches!(e.kind, ItemKind::HourInfo { auto: true, .. })) {
            return None;
        }
        let old = std::mem::take(&mut self.playlist);
        let mut map = vec![None; old.len()];
        for (i, e) in old.into_iter().enumerate() {
            if !matches!(e.kind, ItemKind::HourInfo { auto: true, .. }) {
                map[i] = Some(self.playlist.len());
                self.playlist.push(e);
            }
        }
        self.current = self.current.and_then(|c| map.get(c).copied().flatten());
        self.pending_jump = self.pending_jump.and_then(|j| map.get(j).copied().flatten());
        Some(map)
    }

    /// Move the rows in `sel` so they sit just before row `to` (an index in the list before the
    /// move; `len` = the end). Returns the rows' new indices.
    pub fn move_rows(&mut self, sel: &BTreeSet<usize>, to: usize) -> BTreeSet<usize> {
        let n = self.playlist.len();
        let picked: Vec<usize> = sel.iter().copied().filter(|&i| i < n).collect();
        if picked.is_empty() {
            return BTreeSet::new();
        }
        let mut order: Vec<usize> = (0..n).filter(|i| !sel.contains(i)).collect();
        let at = order.iter().position(|&i| i >= to).unwrap_or(order.len());
        order.splice(at..at, picked.iter().copied());
        let mut slots: Vec<Option<PlEntry>> = std::mem::take(&mut self.playlist).into_iter().map(Some).collect();
        self.playlist = order.iter().map(|&i| slots[i].take().unwrap()).collect();
        let pos = |o: usize| order.iter().position(|&i| i == o);
        self.current = self.current.and_then(pos);
        self.pending_jump = self.pending_jump.and_then(pos);
        (at..at + picked.len()).collect()
    }

    /// Seconds until the next hour marker in the on-air list (if any).
    pub fn secs_to_next_marker(&self) -> Option<i64> {
        let starts = self.projected_starts(true);
        let from = self.current.map(|c| c + 1).unwrap_or(0);
        (from..self.playlist.len())
            .find(|&i| matches!(self.playlist[i].kind, ItemKind::HourMarker { .. }))
            .and_then(|i| starts[i])
            .map(|s| (s.at - Local::now()).num_seconds().max(0))
    }

    /// Fill in a speech row's text: the time placeholders plus {next}, {next_artist}, {next_title},
    /// {last}, {last_artist}, {last_title} and {station}.
    pub fn fill_speech(&self, text: &str, row: usize) -> String {
        let now = Local::now();
        let mut out = live::announce_text(text, &now);
        let next = self.playlist.iter().skip(row + 1).find(|e| e.is_file()).map(|e| e.track.clone());
        let last = self.playlist[..row.min(self.playlist.len())].iter().rev().find(|e| e.is_file()).map(|e| e.track.clone());
        let parts = |t: &Option<Track>| t.as_ref().map(|t| (t.label(), t.artist.clone(), t.title.clone())).unwrap_or_default();
        let (n, na, nt) = parts(&next);
        let (l, la, lt) = parts(&last);
        let station = self.cfg.encoders.first().map(|e| e.stream_name.clone()).unwrap_or_default();
        for (k, v) in [
            ("{next}", n),
            ("{next_artist}", na),
            ("{next_title}", nt),
            ("{last}", l),
            ("{last_artist}", la),
            ("{last_title}", lt),
            ("{station}", station),
        ] {
            out = out.replace(k, &v);
        }
        out
    }

    /// Text for the On Air box.
    pub fn on_air_text(&self) -> String {
        if let Some(u) = self.wait_until {
            let left = (u - Local::now()).num_seconds().max(0);
            return format!("{} ({:02}:{:02}:{:02})", self.wait_label, left / 3600, (left / 60) % 60, left % 60);
        }
        if self.state == PlayState::Stopped || !self.status.has_track {
            return String::new();
        }
        self.on_air_label.clone()
    }

    // ------------------------------------------------------------- commands
    /// Station commands for "Add command…" rows:
    /// STOP, PAUSE, VOLUME <0-100>, ENCODER START <n>, ENCODER STOP <n>, LOAD <file.m3u>,
    /// PLAYLIST <tab name>, RUN <program and arguments>.
    pub fn run_command(&mut self, cmd: &str) {
        let c = cmd.trim();
        let up = c.to_uppercase();
        let words: Vec<&str> = c.split_whitespace().collect();
        let arg = |n: usize| words.get(n).copied().unwrap_or("");
        let rest = |n: usize| -> String { words.iter().skip(n).copied().collect::<Vec<_>>().join(" ") };
        self.say(format!("Command: {c}"));
        match words.first().map(|w| w.to_uppercase()).as_deref() {
            Some("STOP") => self.stop(),
            Some("PAUSE") => {
                if self.state == PlayState::Playing {
                    self.pause_toggle();
                }
            }
            Some("VOLUME") => match arg(1).trim_end_matches('%').parse::<f32>() {
                Ok(v) => self.set_volume((v / 100.0).clamp(0.0, 1.0)),
                Err(_) => self.say("Unknown command: VOLUME needs a number 0-100"),
            },
            Some("ENCODER") => {
                let n = arg(2).parse::<usize>().unwrap_or(1).max(1) - 1;
                if up.starts_with("ENCODER START") {
                    self.start_encoder(n);
                } else if up.starts_with("ENCODER STOP") {
                    self.stop_encoder(n);
                } else {
                    self.say("Unknown command: use ENCODER START <n> or ENCODER STOP <n>");
                }
            }
            Some("LOAD") => {
                let p = PathBuf::from(rest(1));
                if p.exists() {
                    self.load_m3u(&p); // replaces this list; playback continues from its first row
                } else {
                    self.say(format!("LOAD: file not found: {}", p.display()));
                }
            }
            Some("PLAYLIST") => {
                let name = rest(1);
                match self.tabs.iter().position(|t| t.name.eq_ignore_ascii_case(name.trim())) {
                    Some(t) => self.pending_tab = Some(t),
                    None => self.say(format!("PLAYLIST: no playlist called \"{name}\"")),
                }
            }
            Some("RUN") => {
                let line = rest(1);
                thread::spawn(move || {
                    #[cfg(windows)]
                    let _ = std::process::Command::new("cmd").args(["/C", &line]).status();
                    #[cfg(not(windows))]
                    let _ = std::process::Command::new("sh").args(["-c", &line]).status();
                });
            }
            _ => self.say(format!("Unknown command: {c}")),
        }
    }

    // ------------------------------------------------------------ track lists
    /// Pick a track from a folder or .m3u for a Track List row, honouring repeat protection.
    fn pick_from_list(&mut self, source: &str) -> Option<Track> {
        let p = Path::new(source.trim());
        let files: Vec<PathBuf> = if p.is_dir() {
            library::audio_files_in(p)
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8")) {
            m3u_paths(p).into_iter().filter(|f| f.exists()).collect()
        } else {
            vec![]
        };
        if files.is_empty() {
            return None;
        }
        let now = Local::now().timestamp();
        let tmin = self.cfg.repeat.track_mins as i64 * 60;
        let amin = self.cfg.repeat.artist_mins as i64 * 60;
        let mut cands: Vec<&PathBuf> = files.iter().filter(|f| self.plays.get(*f).is_none_or(|&t| now - t >= tmin)).collect();
        let mut rng = rand::thread_rng();
        if cands.is_empty() {
            // everything played recently: take the least recently played ones
            let mut all: Vec<&PathBuf> = files.iter().collect();
            all.sort_by_key(|f| self.plays.get(*f).copied().unwrap_or(0));
            cands = all.into_iter().take(5).collect();
        }
        cands.shuffle(&mut rng);
        for f in cands.iter().take(60) {
            let t = self.entry_for(f).track;
            let a = t.artist.trim().to_lowercase();
            if a.is_empty() || self.artist_plays.get(&a).is_none_or(|&x| now - x >= amin) {
                return Some(t);
            }
        }
        cands.first().map(|f| self.entry_for(f).track)
    }

    /// (times played this session, last played) for a track, from the play history.
    pub fn last_played_of(&self, path: &Path) -> (usize, Option<String>) {
        let label = self
            .lib_index
            .get(path)
            .map(|&i| self.library[i].label())
            .or_else(|| self.playlist.iter().find(|e| e.track.path == path).map(|e| e.track.label()));
        let Some(label) = label else { return (0, None) };
        let hits: Vec<&HistoryItem> = self.history.iter().filter(|h| h.text == label).collect();
        (hits.len(), hits.last().map(|h| h.time.clone()))
    }

    // ------------------------------------------------------------ fading
    /// Cue points to use for a file: hand-edited ones always, analysed ones in Smart mode.
    pub fn cues_for(&self, path: &Path) -> Option<Cues> {
        let c = self.cues.get(path)?;
        (c.manual || self.cfg.fade.mode == FadeMode::Smart).then_some(*c)
    }

    /// Seconds from a track's start until the next one starts (used for start-time estimates).
    pub fn span_of(&self, t: &Track) -> f32 {
        match self.cues_for(&t.path) {
            Some(c) => c.span(),
            None => {
                let d = t.duration;
                (d - self.cfg.crossfade_secs.min(d * 0.5)).max(0.0)
            }
        }
    }

    pub fn request_analysis(&mut self, path: &Path) {
        if self.waves.contains_key(path) || self.pending.contains(path) || path.as_os_str().is_empty() {
            return;
        }
        self.pending.insert(path.to_path_buf());
        let _ = self.analyze_tx.send(path.to_path_buf());
    }

    fn on_air_path(&self) -> Option<PathBuf> {
        self.on_air.as_ref().map(|t| t.path.clone())
    }

    fn drain_analysis(&mut self) {
        while let Ok((path, wave)) = self.analyze_rx.try_recv() {
            self.pending.remove(&path);
            let Some(w) = wave else { continue };
            if !self.cues.get(&path).is_some_and(|c| c.manual) {
                let c = smart_cues(&w, &self.cfg.fade);
                self.cues.insert(path.clone(), c);
                self.cues_dirty = true;
            }
            self.waves.insert(path.clone(), Arc::new(w));
            self.wave_order.retain(|p| p != &path);
            self.wave_order.push(path.clone());
            while self.wave_order.len() > 48 {
                let old = self.wave_order.remove(0);
                self.waves.remove(&old);
            }
            // the track on air was analysed after it started: hand its cues to the engine now
            if self.on_air_path().as_deref() == Some(path.as_path()) && self.active_cues.is_none() && !self.triggered_next {
                if let Some(c) = self.cues_for(&path) {
                    self.engine.set_active_cues(
                        &path,
                        CueSpec {
                            cue_in: c.cue_in,
                            cue_out: Some(c.cue_out),
                            fade_in: 0.0,
                            fade_out_at: Some(c.fade_out),
                            gain_db: c.gain_db,
                        },
                    );
                    self.active_cues = Some(c);
                }
            }
        }
    }

    /// Store hand-edited cues (from the track editor).
    pub fn set_manual_cues(&mut self, path: &Path, mut c: Cues, duration: f32) {
        c.manual = true;
        c.clamp(duration);
        self.cues.insert(path.to_path_buf(), c);
        self.cues_dirty = true;
        self.refresh_active_cues(path);
    }

    /// Forget hand-edited cues; smart analysis takes over again.
    pub fn clear_manual_cues(&mut self, path: &Path) {
        if let Some(w) = self.waves.get(path).cloned() {
            self.cues.insert(path.to_path_buf(), smart_cues(&w, &self.cfg.fade));
        } else {
            self.cues.remove(path);
        }
        self.cues_dirty = true;
        self.refresh_active_cues(path);
    }

    /// Recompute every analysed (non-manual) track after the smart settings changed.
    pub fn reanalyse_cues(&mut self) {
        let fc = self.cfg.fade.clone();
        for (p, w) in &self.waves {
            if !self.cues.get(p).is_some_and(|c| c.manual) {
                self.cues.insert(p.clone(), smart_cues(w, &fc));
            }
        }
        self.cues_dirty = true;
    }

    fn refresh_active_cues(&mut self, path: &Path) {
        if self.on_air_path().as_deref() == Some(path) && !self.triggered_next {
            self.active_cues = self.cues_for(path);
            if let Some(c) = self.active_cues {
                self.engine.set_active_cues(
                    path,
                    CueSpec { cue_in: c.cue_in, cue_out: Some(c.cue_out), fade_in: 0.0, fade_out_at: Some(c.fade_out), gain_db: c.gain_db },
                );
            }
        }
    }

    /// The files a song-count rule plays from.
    fn rotation_pool(r: &crate::config::Rotation) -> Vec<PathBuf> {
        if !r.files.is_empty() {
            r.files.iter().map(PathBuf::from).filter(|p| p.exists()).collect()
        } else if !r.folder.trim().is_empty() {
            library::audio_files_in(Path::new(&r.folder))
        } else {
            vec![]
        }
    }

    /// Song-count rules: after every N songs, insert adverts / jingles after the row that just started.
    fn apply_rotations(&mut self, idx: usize, t: &Track) {
        use crate::config::RotPick;
        let n_rules = self.cfg.rotations.len();
        if self.songs_since.len() < n_rules {
            self.songs_since.resize(n_rules, 0);
        }
        // an advert or jingle doesn't count as a song
        let is_rot = self
            .cfg
            .rotations
            .iter()
            .any(|r| (!r.folder.is_empty() && t.path.starts_with(&r.folder)) || r.files.iter().any(|f| Path::new(f) == t.path));
        if is_rot {
            return;
        }
        let mut insert: Vec<PlEntry> = vec![];
        for k in 0..n_rules {
            let r = self.cfg.rotations[k].clone();
            if !r.enabled {
                continue;
            }
            self.songs_since[k] += 1;
            if self.songs_since[k] < r.every_n.max(1) {
                continue;
            }
            let pool = Self::rotation_pool(&r);
            if pool.is_empty() {
                continue;
            }
            self.songs_since[k] = 0;
            let want = (r.count.max(1) as usize).min(pool.len());
            let picked: Vec<PathBuf> = match r.pick {
                RotPick::All => pool.clone(),
                RotPick::Random => pool.choose_multiple(&mut rand::thread_rng(), want).cloned().collect(),
                RotPick::InOrder => {
                    let start = r.next % pool.len();
                    let v: Vec<PathBuf> = (0..want).map(|j| pool[(start + j) % pool.len()].clone()).collect();
                    self.cfg.rotations[k].next = (start + want) % pool.len();
                    v
                }
            };
            self.say(format!("{}: {} after this song", r.name, picked.len()));
            insert.extend(picked.iter().map(|p| self.entry_for(p)));
        }
        if !insert.is_empty() {
            self.insert_rows(idx + 1, insert);
        }
    }

    pub fn play_pressed(&mut self, selected: Option<usize>) {
        match self.state {
            PlayState::Paused => {
                self.engine.set_paused(false);
                self.state = PlayState::Playing;
            }
            _ => {
                let i = selected.or(self.current).unwrap_or(0);
                if !self.play_index(i, 0.0) {
                    self.say("Nothing to play — add tracks to the playlist");
                }
            }
        }
    }

    pub fn pause_toggle(&mut self) {
        match self.state {
            PlayState::Playing => {
                self.engine.set_paused(true);
                self.state = PlayState::Paused;
            }
            PlayState::Paused => {
                self.engine.set_paused(false);
                self.state = PlayState::Playing;
            }
            _ => {}
        }
    }

    pub fn stop(&mut self) {
        self.engine.stop(1.0);
        if self.state != PlayState::Stopped {
            self.plugin_event("playback_stopped", json!({}));
        }
        self.state = PlayState::Stopped;
        self.wait_until = None;
        self.sub_queue.clear();
        self.pending_jump = None;
    }

    pub fn next(&mut self) {
        let xf = self.cfg.crossfade_secs;
        if !self.advance(xf) {
            self.stop();
        }
    }

    pub fn prev(&mut self) {
        if let Some(c) = self.current {
            if c > 0 {
                let xf = self.cfg.crossfade_secs;
                self.play_index(c - 1, xf);
            }
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.cfg.volume = v;
        self.engine.set_volume(v);
    }

    /// Fire a cart pad; pressing a pad that is already playing stops it.
    pub fn trigger_cart(&self, idx: usize) {
        if let Some(p) = self.cfg.carts.get(idx).map(|c| c.path.clone()) {
            if !p.is_empty() {
                if self.engine.stop_carts(Some(Path::new(&p)), 0.4) > 0 {
                    return;
                }
                if let Err(e) = self.engine.trigger_cart(Path::new(&p)) {
                    self.say(format!("Cart error: {e}"));
                }
            }
        }
    }

    /// Stop every jingle that is playing (quick fade).
    pub fn stop_carts(&self) {
        let n = self.engine.stop_carts(None, 0.4);
        if n > 0 {
            self.say(format!("Stopped {n} jingle(s)"));
        }
    }

    // ------------------------------------------------------------- playlist
    pub fn entry_for(&self, p: &Path) -> PlEntry {
        let track = match self.lib_index.get(p) {
            Some(&i) => self.library[i].clone(),
            None => Track::from_path(p),
        };
        PlEntry::file(track)
    }

    fn expand(paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut out = vec![];
        for p in paths {
            if p.is_dir() {
                out.extend(library::audio_files_in(&p));
            } else if library::is_audio(&p) {
                out.push(p);
            }
        }
        out
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        for p in Self::expand(paths) {
            let e = self.entry_for(&p);
            self.playlist.push(e);
        }
    }

    pub fn insert_next(&mut self, paths: Vec<PathBuf>) {
        let at = self.current.map(|c| c + 1).unwrap_or(0);
        let rows: Vec<PlEntry> = Self::expand(paths).iter().map(|p| self.entry_for(p)).collect();
        self.insert_rows(at, rows);
    }

    /// Insert rows at `at` (clamped), keeping the on-air row pointer right.
    pub fn insert_rows(&mut self, at: usize, rows: Vec<PlEntry>) {
        let at = at.min(self.playlist.len());
        let n = rows.len();
        self.playlist.splice(at..at, rows);
        if let Some(c) = self.current {
            if at <= c {
                self.current = Some(c + n);
            }
        }
        if let Some(j) = self.pending_jump {
            if at <= j {
                self.pending_jump = Some(j + n);
            }
        }
    }

    pub fn remove(&mut self, sel: &BTreeSet<usize>) {
        for &i in sel.iter().rev() {
            if i < self.playlist.len() {
                self.playlist.remove(i);
                self.current = match self.current {
                    Some(c) if c == i => None,
                    Some(c) if c > i => Some(c - 1),
                    o => o,
                };
                self.pending_jump = match self.pending_jump {
                    Some(j) if j == i => None,
                    Some(j) if j > i => Some(j - 1),
                    o => o,
                };
            }
        }
    }

    pub fn move_sel(&mut self, sel: &BTreeSet<usize>, up: bool) -> BTreeSet<usize> {
        let mut new = BTreeSet::new();
        let idx: Vec<usize> = if up { sel.iter().cloned().collect() } else { sel.iter().rev().cloned().collect() };
        let n = self.playlist.len();
        for i in idx {
            let j = if up {
                i.checked_sub(1)
            } else if i + 1 < n {
                Some(i + 1)
            } else {
                None
            };
            match j {
                Some(j) if !new.contains(&j) => {
                    self.playlist.swap(i, j);
                    if self.current == Some(i) {
                        self.current = Some(j);
                    } else if self.current == Some(j) {
                        self.current = Some(i);
                    }
                    new.insert(j);
                }
                _ => {
                    new.insert(i);
                }
            }
        }
        new
    }

    pub fn clear_playlist(&mut self) {
        self.playlist.clear();
        self.current = None;
        self.pending_jump = None;
    }

    pub fn shuffle(&mut self) {
        let start = self.current.map(|c| c + 1).unwrap_or(0).min(self.playlist.len());
        self.playlist[start..].shuffle(&mut rand::thread_rng());
    }

    /// Keep the on-air row pointing at the same row after the list was reordered.
    fn keep_current<F: FnOnce(&mut Vec<PlEntry>)>(&mut self, f: F) {
        let cur = self.current.and_then(|c| self.playlist.get(c)).map(|e| (e.track.path.clone(), e.title()));
        f(&mut self.playlist);
        self.current = cur.and_then(|(p, t)| self.playlist.iter().position(|e| e.track.path == p && e.title() == t));
        self.pending_jump = None;
    }

    pub fn sort_by(&mut self, by: &str) {
        let by = by.to_string();
        self.keep_current(|pl| match by.as_str() {
            "artist" => pl.sort_by_key(|e| (e.track.artist.to_lowercase(), e.track.title.to_lowercase())),
            "title" => pl.sort_by_key(|e| e.track.title.to_lowercase()),
            _ => pl.sort_by(|a, b| a.est_secs().total_cmp(&b.est_secs())),
        });
    }

    pub fn reverse(&mut self) {
        self.keep_current(|pl| pl.reverse());
    }

    /// Remove repeated files (keeps the first of each). Returns how many were removed.
    pub fn remove_duplicates(&mut self) -> usize {
        let before = self.playlist.len();
        let mut seen = std::collections::HashSet::new();
        self.keep_current(|pl| pl.retain(|e| !e.is_file() || seen.insert(e.track.path.clone())));
        before - self.playlist.len()
    }

    /// Remove rows that have already played (not the one on air).
    pub fn remove_played(&mut self) -> usize {
        let cur = self.current;
        let sel: BTreeSet<usize> = self.playlist.iter().enumerate().filter(|(i, e)| e.played && Some(*i) != cur).map(|(i, _)| i).collect();
        self.remove(&sel);
        sel.len()
    }

    /// Remove file rows whose file no longer exists.
    pub fn remove_missing(&mut self) -> usize {
        let sel: BTreeSet<usize> =
            self.playlist.iter().enumerate().filter(|(_, e)| e.is_file() && !e.track.path.exists()).map(|(i, _)| i).collect();
        self.remove(&sel);
        sel.len()
    }

    pub fn total_secs(&self) -> f32 {
        self.playlist.iter().map(|e| e.est_secs()).sum()
    }

    pub fn save_m3u(&self, path: &Path) {
        let mut s = String::from("#EXTM3U\n");
        for e in &self.playlist {
            s += &e.to_m3u();
        }
        let _ = std::fs::write(path, s);
    }

    /// Rows stored in an M3U (RadioMaster special rows included).
    pub fn read_m3u(&self, path: &Path) -> Vec<PlEntry> {
        let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut out = vec![];
        let mut voice = false;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line == "#RADIOMASTER-VOICE" {
                voice = true;
                continue;
            }
            if let Some(e) = crate::items::from_m3u_comment(line) {
                out.push(e);
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let p = PathBuf::from(line);
            let p = if p.is_absolute() { p } else { base.join(p) };
            if p.exists() {
                let mut e = self.entry_for(&p);
                if voice {
                    e.kind = ItemKind::VoiceTrack;
                }
                out.push(e);
            }
            voice = false;
        }
        out
    }

    pub fn load_m3u(&mut self, path: &Path) {
        let rows = self.read_m3u(path);
        if rows.is_empty() && !path.exists() {
            return;
        }
        self.playlist = rows;
        self.current = None;
        self.pending_jump = None;
    }

    // -------------------------------------------------------------- library
    fn reindex(&mut self) {
        self.lib_index = self.library.iter().enumerate().map(|(i, t)| (t.path.clone(), i)).collect();
        self.library_ver += 1;
    }

    pub fn start_scan(&mut self) {
        if self.scanning {
            return;
        }
        let folders = self.cfg.library_folders.clone();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.scanning = true;
        self.scan_progress = 0;
        thread::spawn(move || {
            let mut out = vec![];
            for f in folders {
                for p in library::audio_files_in(Path::new(&f)) {
                    out.push(Track::from_path(&p));
                    if out.len() % 25 == 0 {
                        let _ = tx.send(ScanMsg::Progress(out.len()));
                    }
                }
            }
            let _ = tx.send(ScanMsg::Done(out));
        });
    }

    fn drain_scan(&mut self) {
        let mut done = None;
        if let Some(rx) = &self.scan_rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    ScanMsg::Progress(n) => self.scan_progress = n,
                    ScanMsg::Done(v) => done = Some(v),
                }
            }
        }
        if let Some(v) = done {
            if let Ok(s) = serde_json::to_string(&v) {
                let _ = std::fs::write(data_dir().join("library.json"), s);
            }
            self.library = v;
            self.reindex();
            self.scanning = false;
            self.scan_rx = None;
            self.say(format!("Library scan complete: {} tracks", self.library.len()));
        }
    }

    pub fn update_tags(&mut self, t: Track) {
        match library::write_tags(&t) {
            Ok(()) => {
                if let Some(&i) = self.lib_index.get(&t.path) {
                    self.library[i] = t.clone();
                    self.library_ver += 1;
                }
                for e in self.playlist.iter_mut().filter(|e| e.is_file() && e.track.path == t.path) {
                    e.track = t.clone();
                }
                for tab in self.tabs.iter_mut() {
                    for e in tab.entries.iter_mut().filter(|e| e.is_file() && e.track.path == t.path) {
                        e.track = t.clone();
                    }
                }
                self.say("Tags saved");
            }
            Err(e) => self.say(format!("Tag write failed: {e}")),
        }
    }

    // ------------------------------------------------------------- encoders
    pub fn start_encoder(&mut self, i: usize) {
        if i >= self.cfg.encoders.len() {
            return;
        }
        if self.handles[i].as_ref().map(|h| h.is_running()).unwrap_or(false) {
            return;
        }
        self.handles[i] = Some(EncoderHandle::start(self.cfg.encoders[i].clone(), self.engine.clone(), self.now_text.clone()));
    }

    pub fn stop_encoder(&mut self, i: usize) {
        if let Some(Some(h)) = self.handles.get(i) {
            h.stop();
        }
    }

    pub fn add_encoder(&mut self) {
        self.cfg.encoders.push(Default::default());
        self.handles.push(None);
    }

    pub fn remove_encoder(&mut self, i: usize) {
        self.stop_encoder(i);
        self.cfg.encoders.remove(i);
        self.handles.remove(i);
    }

    fn push_metadata(&self, text: &str) {
        for (i, h) in self.handles.iter().enumerate() {
            if let Some(h) = h {
                if h.is_running() && self.cfg.encoders[i].send_metadata {
                    let (c, t) = (self.cfg.encoders[i].clone(), text.to_string());
                    thread::spawn(move || {
                        let _ = stream::send_metadata(&c, &t);
                    });
                }
            }
        }
    }

    // ------------------------------------------------- generator / ads / misc
    pub fn generate_playlist(&mut self, name: &str, append: bool) -> usize {
        let Some(tpl) = self.cfg.templates.iter().find(|t| t.name == name).cloned() else {
            self.say(format!("Template '{name}' not found"));
            return 0;
        };
        let tracks = crate::generator::generate(&tpl, &self.cfg.categories, &self.library);
        let n = tracks.len();
        if n == 0 {
            self.say("Generator produced nothing — scan the library and check category filters");
            return 0;
        }
        if !append {
            self.clear_playlist();
        }
        // when the new rows would start, so hour markers land on the right hours
        let starts = self.projected_starts(true);
        let mut t = match self.playlist.len() {
            0 => Local::now(),
            k => {
                let last = &self.playlist[k - 1];
                starts[k - 1].map(|s| s.at).unwrap_or_else(Local::now) + CDur::milliseconds((self.row_span(last) * 1000.0) as i64)
            }
        };
        for tr in tracks {
            let span = self.span_of(&tr);
            if tpl.hour_markers {
                use chrono::Timelike;
                let after = t + CDur::milliseconds((span * 1000.0) as i64);
                if after.hour() != t.hour() || after.date_naive() != t.date_naive() {
                    // this track would run over the top of the hour: the next hour starts with a marker
                    self.playlist.push(PlEntry::file(tr));
                    let h = after.hour();
                    self.playlist.push(PlEntry::special(ItemKind::HourMarker { hour: h, minute: 0, hard: false, wait: true }, "", 0.0));
                    t = marker_target(h, 0, after);
                    continue;
                }
            }
            t += CDur::milliseconds((span * 1000.0) as i64);
            self.playlist.push(PlEntry::file(tr));
        }
        n
    }

    fn fire_ad_break(&mut self, br: &AdBreak, now: &DateTime<Local>) {
        let limit = if br.max_ads == 0 { usize::MAX } else { br.max_ads as usize };
        let paths: Vec<PathBuf> =
            self.cfg.ads.iter().filter(|a| a.active(now) && !a.path.is_empty()).map(|a| PathBuf::from(&a.path)).take(limit).collect();
        if paths.is_empty() {
            return;
        }
        self.say(format!("Ad break '{}': {} spot(s) queued", br.name, paths.len()));
        self.insert_next(paths);
    }

    fn export_now(&self, t: &Track) {
        let e = self.cfg.export.clone();
        if e.text_file.is_empty() && e.xml_file.is_empty() && e.http_url.is_empty() {
            return;
        }
        let (a, ti) = (t.artist.clone(), t.title.clone());
        thread::spawn(move || {
            let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            if !e.text_file.is_empty() {
                let _ = std::fs::write(&e.text_file, format!("{a} - {ti}"));
            }
            if !e.xml_file.is_empty() {
                let _ = std::fs::write(
                    &e.xml_file,
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<nowplaying><artist>{}</artist><title>{}</title></nowplaying>\n",
                        esc(&a),
                        esc(&ti)
                    ),
                );
            }
            if !e.http_url.is_empty() {
                let url = e.http_url.replace("{artist}", &urlencoding::encode(&a)).replace("{title}", &urlencoding::encode(&ti));
                let _ = ureq::get(&url).timeout(std::time::Duration::from_secs(6)).call();
            }
        });
    }

    pub fn seek(&mut self, secs: f32) {
        if let Err(e) = self.engine.seek(secs) {
            self.say(format!("Seek failed: {e}"));
        }
    }

    pub fn render_playlist(&self, out: PathBuf) {
        let paths: Vec<PathBuf> = self.playlist.iter().filter(|e| e.is_file()).map(|e| e.track.path.clone()).collect();
        let notify = self.notify.clone();
        thread::spawn(move || {
            let n2 = notify.clone();
            let res = crate::engine::render_wav(&paths, &out, &move |k, n| {
                *n2.lock() = format!("Rendering playlist… track {}/{}", k + 1, n);
            });
            *notify.lock() = match res {
                Ok(()) => format!("Rendered playlist to {}", out.display()),
                Err(e) => format!("Render failed: {e}"),
            };
        });
    }

    /// Playlist as CSV (time, type, title, length).
    pub fn export_csv(&self, out: &Path) -> std::io::Result<()> {
        let starts = self.projected_starts(true);
        let mut s = String::from("Start,Type,Artist,Title,Length,File\n");
        let q = |x: &str| format!("\"{}\"", x.replace('"', "'"));
        for (i, e) in self.playlist.iter().enumerate() {
            let at = starts.get(i).copied().flatten().map(|s| s.at.format("%H:%M:%S").to_string()).unwrap_or_default();
            s += &format!(
                "{},{},{},{},{},{}\n",
                at,
                e.type_label(),
                q(&e.track.artist),
                q(&if e.is_file() { e.track.title.clone() } else { e.title() }),
                e.length_text(),
                q(&if e.is_file() { e.track.path.display().to_string() } else { String::new() })
            );
        }
        std::fs::write(out, s)
    }

    // ------------------------------------------------------ history/events
    fn log_history(&mut self, text: &str) {
        let time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let line = format!("\"{}\",\"{}\"\n", time, text.replace('"', "'"));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("history.csv")) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
        self.history.push(HistoryItem { time, text: text.to_string() });
        if self.history.len() > 500 {
            self.history.remove(0);
        }
        self.log_sink.lock().push(format!("Started: {text}"));
    }

    fn exec(&mut self, e: &Event) {
        let xf = self.cfg.crossfade_secs;
        self.say(format!("Scheduler: {}", e.name));
        match e.action {
            ActionKind::LoadPlaylistAndPlay => {
                self.load_m3u(Path::new(&e.path));
                self.play_index(0, xf);
            }
            ActionKind::PlayFile => {
                self.insert_next(vec![PathBuf::from(&e.path)]);
                self.next();
            }
            ActionKind::InsertNext => self.insert_next(vec![PathBuf::from(&e.path)]),
            ActionKind::Stop => self.stop(),
            ActionKind::StartEncoder => self.start_encoder(e.arg as usize),
            ActionKind::StopEncoder => self.stop_encoder(e.arg as usize),
            ActionKind::SetVolume => self.set_volume(e.arg.clamp(0.0, 1.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn playout() -> Playout {
        // keep test files out of the real settings folder
        std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join("rm_test_cfg"));
        Playout::new(Engine::new_detached(), Config::default())
    }

    fn file(name: &str, secs: f32) -> PlEntry {
        PlEntry::file(Track {
            path: PathBuf::from(format!("/nonexistent/{name}.mp3")),
            title: name.into(),
            duration: secs,
            ..Default::default()
        })
    }

    fn marker(t: DateTime<Local>, hard: bool, wait: bool) -> PlEntry {
        PlEntry::special(ItemKind::HourMarker { hour: t.hour(), minute: t.minute(), hard, wait }, "", 0.0)
    }

    #[test]
    fn adverts_every_n_songs_in_order() {
        use crate::config::{RotPick, Rotation};
        let mut p = playout();
        let dir = std::env::temp_dir().join("rm_test_ads");
        let _ = std::fs::create_dir_all(&dir);
        let ads: Vec<String> = (0..3)
            .map(|i| {
                let f = dir.join(format!("ad{i}.wav"));
                crate::engine::write_wav(&f, &vec![0.0; 4410 * 2], 2, 44100).unwrap();
                f.to_string_lossy().to_string()
            })
            .collect();
        p.cfg.rotations = vec![Rotation {
            name: "Adverts".into(),
            enabled: true,
            adverts: true,
            files: ads.clone(),
            every_n: 2,
            count: 2,
            pick: RotPick::InOrder,
            ..Default::default()
        }];
        for i in 0..4 {
            p.playlist.push(file(&format!("song{i}"), 200.0));
        }
        let song = p.playlist[0].track.clone();
        p.apply_rotations(0, &song); // 1st song: nothing yet
        assert_eq!(p.playlist.len(), 4);
        p.apply_rotations(1, &song); // 2nd song: two adverts after row 1
        let names: Vec<String> = p.playlist.iter().map(|e| e.track.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(&names[2..4], ["ad0.wav", "ad1.wav"]);
        // an advert starting doesn't count as a song
        let ad = p.playlist[2].track.clone();
        p.apply_rotations(2, &ad);
        p.apply_rotations(3, &ad);
        assert_eq!(p.playlist.len(), 6);
        // next time it carries on where it stopped
        p.apply_rotations(4, &song);
        p.apply_rotations(5, &song);
        let names: Vec<String> = p.playlist.iter().map(|e| e.track.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(&names[6..8], ["ad2.wav", "ad0.wav"]);
    }

    #[test]
    fn dragging_rows_keeps_the_on_air_row() {
        let mut p = playout();
        for i in 0..6 {
            p.playlist.push(file(&format!("t{i}"), 60.0));
        }
        p.current = Some(1);
        let sel: BTreeSet<usize> = [3, 4].into_iter().collect();
        let new = p.move_rows(&sel, 1);
        let names: Vec<String> = p.playlist.iter().map(|e| e.track.title.clone()).collect();
        assert_eq!(names, ["t0", "t3", "t4", "t1", "t2", "t5"]);
        assert_eq!(new, [1, 2].into_iter().collect());
        assert_eq!(p.current, Some(3), "the playing row moved down with the insert");
        // to the end
        let sel: BTreeSet<usize> = [0].into_iter().collect();
        p.move_rows(&sel, 6);
        assert_eq!(p.playlist.last().unwrap().track.title, "t0");
    }

    #[test]
    fn hour_lines_go_where_the_hour_changes() {
        let mut p = playout();
        // 30 rows of 5 minutes = 2.5 hours from now (stopped: starts are projected from now)
        for i in 0..30 {
            p.playlist.push(file(&format!("t{i}"), 300.0 + p.cfg.crossfade_secs.min(150.0)));
        }
        let map = p.refresh_hour_info(false).expect("lines added");
        assert_eq!(map.len(), 30);
        let lines: Vec<usize> = p
            .playlist
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.kind, ItemKind::HourInfo { auto: true, .. }))
            .map(|(i, _)| i)
            .collect();
        assert!(lines.len() >= 2 && lines.len() <= 3, "{lines:?}");
        // each line's following row starts in a new hour
        let starts = p.projected_starts(false);
        for &l in &lines {
            let (a, b) = (starts[l - 1].unwrap().at, starts[l + 1].unwrap().at);
            assert_ne!(a.format("%H").to_string(), b.format("%H").to_string());
        }
        // nothing to do the second time
        assert!(p.refresh_hour_info(false).is_none());
        assert!(p.clear_hour_info().is_some());
        assert_eq!(p.playlist.len(), 30);
    }

    #[test]
    #[ignore]
    fn bench_projected_starts() {
        let mut p = playout();
        let now = Local::now();
        for i in 0..2000 {
            p.playlist.push(file(&format!("t{i}"), 200.0));
            if i % 15 == 0 {
                p.playlist.push(marker(now + chrono::Duration::minutes(i as i64 * 3), false, true));
            }
        }
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(p.projected_starts(true));
        }
        eprintln!("projected_starts x{} rows: {:?} each", p.playlist.len(), t.elapsed() / 20);
    }

    #[test]
    fn marker_target_is_the_nearest_time() {
        let base = Local.with_ymd_and_hms(2026, 10, 17, 23, 50, 0).unwrap();
        assert_eq!(marker_target(0, 0, base), Local.with_ymd_and_hms(2026, 10, 18, 0, 0, 0).unwrap());
        let early = Local.with_ymd_and_hms(2026, 10, 17, 0, 10, 0).unwrap();
        assert_eq!(marker_target(23, 0, early), Local.with_ymd_and_hms(2026, 10, 16, 23, 0, 0).unwrap());
    }

    #[test]
    fn markers_wait_skip_and_cut() {
        let mut p = playout();
        p.cfg.crossfade_secs = 3.0;
        p.cfg.fade.mode = FadeMode::Fixed;
        let now = Local::now();
        // a marker 4–5 minutes ahead (whole minute)
        let m = now + CDur::minutes(5);
        let target = marker_target(m.hour(), m.minute(), now);

        // early: the marker waits for its time
        p.playlist = vec![file("A", 60.0), marker(target, false, true), file("B", 60.0)];
        let s = p.projected_starts(false);
        assert_eq!(s[1].unwrap().at, target);
        assert_eq!(s[2].unwrap().at, target, "the row after a waiting marker starts on the hour");

        // late, soft marker: rows starting after the marker time are skipped, the marker
        // follows the track that is playing when the time comes
        let rows = |hard| {
            vec![file("A", 120.0), file("B", 120.0), file("C", 120.0), file("D", 120.0), marker(target, hard, true), file("E", 60.0)]
        };
        p.playlist = rows(false);
        let s = p.projected_starts(false);
        assert!(s[3].unwrap().skipped && !s[2].unwrap().skipped, "only D starts after the marker time");
        assert_eq!(s[4].unwrap().at, s[3].unwrap().at, "soft marker: after C finishes");

        // hard marker: cuts in exactly on time
        p.playlist = rows(true);
        let s = p.projected_starts(false);
        assert!(s[3].unwrap().skipped);
        assert_eq!(s[4].unwrap().at, target);
        assert_eq!(s[5].unwrap().at, target);
    }

    #[test]
    fn playing_from_another_tab_puts_it_on_air() {
        let mut p = playout();
        let wav = std::env::temp_dir().join("rm_tab_test.wav");
        let tone: Vec<f32> = vec![0.1; crate::engine::SR as usize * 2];
        crate::engine::write_wav(&wav, &tone, 2, crate::engine::SR).unwrap();
        p.playlist = vec![file("main1", 60.0)];
        let t = p.new_tab(Some("Evening".into()));
        p.with_tab(t, |c| c.add_paths(vec![wav.clone()]));
        assert_eq!(p.playing_tab, 0);
        assert_eq!(p.playlist.len(), 1, "editing another tab leaves the on-air list alone");
        assert_eq!(p.tab_len(t), 1);
        // a row that fails to play does not take the tab on air
        p.with_tab(t, |c| c.playlist.insert(0, file("missing", 30.0)));
        assert_eq!(p.playing_tab, 0);
        p.put_tab_on_air(t);
        assert_eq!(p.playing_tab, t, "playing a row of the tab puts that tab on air");
        assert_eq!(p.on_air.as_ref().map(|x| x.path.clone()), Some(wav));
        assert_eq!(p.tab_rows(0).len(), 1, "the old list is kept in its own tab");
    }

    #[test]
    fn station_commands() {
        let mut p = playout();
        p.run_command("VOLUME 40");
        assert!((p.cfg.volume - 0.4).abs() < 1e-6);
        let t = p.new_tab(Some("Night".into()));
        p.run_command("playlist night");
        assert_eq!(p.pending_tab, Some(t));
        p.run_command("FLY TO THE MOON");
        p.drain_log();
        assert!(p.log.iter().any(|l| l.error && l.text.contains("Unknown command")));
    }
}
