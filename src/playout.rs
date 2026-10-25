//! Station brain: playlists (tabs), every playlist item type, auto-advance + crossfade, hour
//! markers, rotations (jingles/ads), scheduler, encoders and history. Runs on its own thread so
//! automation keeps working even when the window is minimized.

use crate::azuracast::{self, AzuraState};
use crate::config::{data_dir, AzuraConfig, Config};
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

pub struct HistoryItem {
    pub time: String,
    pub text: String,
    /// song, advert, jingle, voice, speech, time, stream, live, teaser (see `report::KINDS`)
    pub kind: &'static str,
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

type Encode = Box<dyn FnOnce() -> Option<String> + Send>;

enum WriteJob {
    Write(PathBuf, String),
    /// turned into text on the writer thread (big playlists take a while to serialize)
    Encode(PathBuf, Encode),
    Append(PathBuf, String),
    Flush(mpsc::Sender<()>),
}

fn writer() -> Option<std::sync::MutexGuard<'static, mpsc::Sender<WriteJob>>> {
    use std::sync::OnceLock;
    static TX: OnceLock<std::sync::Mutex<mpsc::Sender<WriteJob>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<WriteJob>();
        thread::spawn(move || {
            // what each file was last given: an unchanged file isn't written again (the periodic save)
            let mut last: HashMap<PathBuf, u64> = HashMap::new();
            let mut write = |p: PathBuf, d: String| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                d.hash(&mut h);
                let h = h.finish();
                if last.get(&p) == Some(&h) && p.exists() {
                    return;
                }
                let tmp = p.with_extension("tmp");
                if std::fs::write(&tmp, d).is_ok() && std::fs::rename(&tmp, &p).is_ok() {
                    last.insert(p, h);
                }
            };
            while let Ok(job) = rx.recv() {
                match job {
                    WriteJob::Write(p, d) => write(p, d),
                    WriteJob::Encode(p, f) => {
                        if let Some(d) = f() {
                            write(p, d);
                        }
                    }
                    WriteJob::Append(p, d) => {
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
                            use std::io::Write;
                            let _ = f.write_all(d.as_bytes());
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
pub(crate) fn write_in_background(path: PathBuf, data: String) {
    let job = match writer() {
        Some(tx) => match tx.send(WriteJob::Write(path, data)) {
            Ok(()) => return,
            Err(mpsc::SendError(job)) => job,
        },
        None => WriteJob::Write(path, data),
    };
    if let WriteJob::Write(path, data) = job {
        let _ = std::fs::write(path, data);
    }
}

/// Like `write_in_background`, but `value` is also turned into JSON on the writer thread, so a
/// big playlist or config never holds up the UI or playback while it is serialized.
pub(crate) fn write_json_in_background<T: Serialize + Send + 'static>(path: PathBuf, value: T, pretty: bool) {
    let encode: Encode = Box::new(move || if pretty { serde_json::to_string_pretty(&value) } else { serde_json::to_string(&value) }.ok());
    let job = match writer() {
        Some(tx) => match tx.send(WriteJob::Encode(path, encode)) {
            Ok(()) => return,
            Err(mpsc::SendError(job)) => job,
        },
        None => WriteJob::Encode(path, encode),
    };
    if let WriteJob::Encode(path, f) = job {
        if let Some(d) = f() {
            let _ = std::fs::write(path, d);
        }
    }
}

/// Add to the end of a file on the background writer (after any writes already queued).
fn append_in_background(path: PathBuf, data: String) {
    let job = match writer() {
        Some(tx) => match tx.send(WriteJob::Append(path, data)) {
            Ok(()) => return,
            Err(mpsc::SendError(job)) => job,
        },
        None => WriteJob::Append(path, data),
    };
    if let WriteJob::Append(path, data) = job {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = f.write_all(data.as_bytes());
        }
    }
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

/// A line of an M3U: a RadioMaster special row, or a file that exists (and whether it is a voice track).
pub enum M3uRow {
    Row(PlEntry),
    File(PathBuf, bool),
}

/// Read an M3U and check which of its files exist (reads the disk; call it off the UI thread).
pub fn parse_m3u(path: &Path) -> Vec<M3uRow> {
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
            out.push(M3uRow::Row(e));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let p = PathBuf::from(line);
        let p = if p.is_absolute() { p } else { base.join(p) };
        if p.exists() {
            out.push(M3uRow::File(p, voice));
        }
        voice = false;
    }
    out
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
    /// kind of the item being started (for the play history), when the file alone can't tell
    kind_hint: Option<&'static str>,
    fired: HashMap<usize, String>,
    /// songs since each song-count rule last went off (by rule id)
    songs_since: HashMap<u64, u32>,
    pub notify: Arc<Mutex<String>>,
    last_save: Instant,
    dsp_shared: DspConfig,
    /// minute each timed rule last went off (so it fires once), and when
    rule_fired: HashMap<u64, String>,
    rule_last: HashMap<u64, DateTime<Local>>,
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
    /// background tag writes: (new tags, old tags, result)
    tag_tx: mpsc::Sender<(Track, Track, Result<(), String>)>,
    tag_rx: mpsc::Receiver<(Track, Track, Result<(), String>)>,
    /// cues of the track on air (what the smart trigger follows)
    pub active_cues: Option<Cues>,
    // ---- playlists
    pub tabs: Vec<PlTab>,
    /// index of the tab that is on air
    pub playing_tab: usize,
    /// set when playback starts from a list swapped in by `with_tab`: that tab goes on air
    adopt: bool,
    /// tab whose rows `with_tab` has swapped into `playlist` (its own slot holds the on-air rows meanwhile)
    swapped: Option<usize>,
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
    /// speech row already rendered ahead (row|text)
    speech_ready: String,
    // ---- RadioMaster Server
    /// set while this playout mirrors a RadioMaster Server: actions go to the server and nothing is saved here
    pub link: Option<crossbeam_channel::Sender<sync::Cmd>>,
    /// jingles playing on the server
    mirror_carts: Vec<PathBuf>,
    /// lines ever written to the log (how the server's clients follow it)
    pub log_total: u64,
}

#[path = "sync.rs"]
pub mod sync;

impl Playout {
    pub fn new(engine: Engine, cfg: Config) -> Self {
        std::thread::spawn(live::clean_speech_files);
        let log_sink: LogSink = Arc::new(Mutex::new(vec![]));
        let mut plugins = crate::plugins::Plugins::new(log_sink.clone());
        if !cfg.plugins_off.is_empty() {
            plugins.reload(&cfg.plugins_off);
        }
        let library: Vec<Track> =
            std::fs::read_to_string(data_dir().join("library.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let cues: HashMap<PathBuf, Cues> = std::fs::read_to_string(data_dir().join("cues.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<(PathBuf, Cues)>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        let plays: HashMap<PathBuf, i64> =
            std::fs::read_to_string(data_dir().join("plays.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let n_enc = cfg.encoders.len();
        let mut p = Self::build(engine, cfg, library, cues, plays, plugins, log_sink);
        p.reindex();
        p.load_tabs();
        for i in 0..n_enc {
            if p.cfg.encoders[i].start_with_app {
                p.start_encoder(i);
            }
        }
        p
    }

    fn build(
        engine: Engine,
        cfg: Config,
        library: Vec<Track>,
        cues: HashMap<PathBuf, Cues>,
        plays: HashMap<PathBuf, i64>,
        plugins: crate::plugins::Plugins,
        log_sink: LogSink,
    ) -> Self {
        engine.set_dsp(&cfg.dsp);
        let dsp_shared = cfg.dsp.clone();
        let azura_shared = Arc::new(Mutex::new(cfg.azuracast.clone()));
        let azura = Arc::new(Mutex::new(AzuraState::default()));
        azuracast::spawn_poller(azura_shared.clone(), azura.clone());
        let n_enc = cfg.encoders.len();
        // background analyser: decodes files and builds waveforms for smart fading / the editor
        let (analyze_tx, job_rx) = mpsc::channel::<PathBuf>();
        let (res_tx, analyze_rx) = mpsc::channel::<(PathBuf, Option<Waveform>)>();
        let (tag_tx, tag_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(path) = job_rx.recv() {
                let w = Waveform::analyze(&path).ok();
                if res_tx.send((path, w)).is_err() {
                    break;
                }
            }
        });
        Playout {
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
            kind_hint: None,
            fired: HashMap::new(),
            songs_since: HashMap::new(),
            notify: Arc::new(Mutex::new(String::new())),
            last_save: Instant::now(),
            plugin_hour: chrono::Timelike::hour(&Local::now()),
            enc_states: vec![],
            dsp_shared,
            rule_fired: HashMap::new(),
            rule_last: HashMap::new(),
            auto_check: Instant::now(),
            cues,
            cues_dirty: false,
            waves: HashMap::new(),
            wave_order: vec![],
            pending: Default::default(),
            analyze_tx,
            analyze_rx,
            tag_tx,
            tag_rx,
            active_cues: None,
            tabs: vec![PlTab { name: "Main".into(), ..Default::default() }],
            playing_tab: 0,
            adopt: false,
            swapped: None,
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
            speech_ready: String::new(),
            plugins,
            link: None,
            mirror_carts: vec![],
            log_total: 0,
        }
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
            self.log_total += 1;
        }
        let n = self.log.len();
        if n > 500 {
            self.log.drain(0..n - 500);
        }
    }

    // ---------------------------------------------------------------- tick
    pub fn tick(&mut self) {
        if self.link.is_some() {
            // mirroring a server: the server plays and schedules, this copy only shows it
            self.drain_log();
            return;
        }
        self.drain_scan();
        self.drain_tags();
        self.drain_tag_writes();
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
                self.prepare_next_speech();
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
            self.run_event(&e);
        }

        // timed advert / jingle rules (every N minutes, set times)
        let timed: Vec<crate::config::AdRule> = self
            .cfg
            .ad_rules
            .iter()
            .filter(|r| self.rule_fired.get(&r.id) != Some(&key) && r.due_now(&now, self.rule_last.get(&r.id).copied()))
            .cloned()
            .collect();
        for r in timed {
            self.rule_fired.insert(r.id, key.clone());
            self.rule_last.insert(r.id, now);
            self.fire_rule(r.id, None);
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
        if self.link.is_some() {
            return;
        }
        self.cfg.save();
        if self.cues_dirty {
            let v: Vec<(PathBuf, Cues)> = self.cues.iter().map(|(p, c)| (p.clone(), *c)).collect();
            write_json_in_background(data_dir().join("cues.json"), v, false);
            self.cues_dirty = false;
        }
        if self.plays_dirty {
            write_json_in_background(data_dir().join("plays.json"), self.plays.clone(), false);
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
        if self.link.is_some() {
            return;
        }
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                // `slot` keeps this right while `with_tab` has another tab swapped in
                let (entries, current) = match self.slot(i) {
                    None => (&self.playlist, self.current),
                    Some(k) => (&self.tabs[k].entries, self.tabs[k].current),
                };
                SavedTab { name: t.name.clone(), entries: entries.clone(), current }
            })
            .collect();
        write_json_in_background(data_dir().join("playlists.json"), SavedTabs { tabs, playing: self.playing_tab }, false);
    }

    fn swap_tab(&mut self, v: usize) {
        std::mem::swap(&mut self.playlist, &mut self.tabs[v].entries);
        std::mem::swap(&mut self.current, &mut self.tabs[v].current);
    }

    /// Run `f` with tab `v` temporarily in place of the on-air list (so every playlist edit works on
    /// any tab). If `f` starts playback from it, tab `v` becomes the on-air tab. Calls may nest: a
    /// nested call for the tab already swapped in runs in place, any other tab is swapped in properly.
    pub fn with_tab<R>(&mut self, v: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if v >= self.tabs.len() {
            return f(self);
        }
        match self.swapped {
            // already working on tab v (e.g. a dialog drawn inside `with_tab` inserting rows)
            Some(s) if s == v => return f(self),
            Some(s) => {
                // put the on-air rows back where they belong, work on v, then swap s in again
                self.swap_tab(s);
                self.swapped = None;
                let r = self.with_tab(v, f);
                if s != self.playing_tab && s < self.tabs.len() {
                    self.swap_tab(s);
                    self.swapped = Some(s);
                }
                return r;
            }
            None => {}
        }
        if v == self.playing_tab {
            self.adopt = false;
            return f(self);
        }
        let old = self.playing_tab;
        self.swap_tab(v);
        self.swapped = Some(v);
        self.adopt = false;
        let r = f(self);
        self.swapped = None;
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

    /// Slot in `tabs` that holds tab `i`'s rows, or None when they are in `playlist`.
    fn slot(&self, i: usize) -> Option<usize> {
        match self.swapped {
            Some(s) if i == s => None,
            Some(s) if i == self.playing_tab => Some(s),
            _ if i == self.playing_tab => None,
            _ => Some(i),
        }
    }

    /// Rows of any tab (read only).
    pub fn tab_rows(&self, i: usize) -> &[PlEntry] {
        match self.slot(i) {
            None => &self.playlist,
            Some(k) => self.tabs.get(k).map(|t| t.entries.as_slice()).unwrap_or(&[]),
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
        match self.slot(i) {
            None => self.playlist.extend(rows),
            Some(k) => {
                if let Some(t) = self.tabs.get_mut(k) {
                    t.entries.extend(rows);
                }
            }
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
        if self.forward(|_| sync::Cmd::TabOnAir { tab: i }) {
            return;
        }
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
        if self.forward(|tab| sync::Cmd::PlayIndex { tab, row: i, xfade }) {
            return true;
        }
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
        self.kind_hint = None;
        let ok = match e.kind.clone() {
            ItemKind::Track | ItemKind::VoiceTrack => {
                self.kind_hint = matches!(e.kind, ItemKind::VoiceTrack).then_some("voice");
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
                self.kind_hint = Some("stream");
                self.started_other(e.title());
                true
            }
            ItemKind::LineIn { secs } => {
                let src = live::line_source(&self.cfg.audio_devices.input, self.log_sink.clone());
                let spec = CueSpec { cue_out: (secs > 0.0).then_some(secs), ..Default::default() };
                self.engine.play_source(Box::new(src), None, secs, spec, Some(xf), true);
                self.kind_hint = Some("live");
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
                        self.kind_hint = Some("teaser");
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
                self.engine.play_source(Box::new(src), None, 0.0, CueSpec::default(), Some(xf), true);
                self.kind_hint = Some("time");
                self.started_other(format!("Time announcement — {text}"));
                true
            }
            ItemKind::Comment(_) | ItemKind::HourInfo { .. } => false,
            ItemKind::Event { action, path, arg } => self.row_event(i, action, &path, arg, &e.title()),
            ItemKind::Tts { text, voice, rate, opts } => {
                let (said, v) = self.speech_for(&text, &voice, rate, &opts, i);
                let src = live::speech_source(said.clone(), v, opts, self.log_sink.clone());
                // speech has pauses between lines: never cut it short with silence trimming
                self.engine.play_source(Box::new(src), None, e.est_secs(), CueSpec::default(), Some(xf), true);
                self.kind_hint = Some("speech");
                self.started_other(format!("Speech — {}", said.lines().next().unwrap_or("")));
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
        let kind = self.kind_hint.take().unwrap_or_else(|| self.kind_of(&t.path));
        self.after_start(text.clone());
        self.log_history(&text, kind, Some(&t.path), t.duration);
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
        let kind = self.kind_hint.take().unwrap_or("other");
        self.log_history(&label, kind, None, 0.0);
        self.push_metadata(&label);
        self.plugin_event("item_started", json!({ "text": label, "title": label }));
    }

    fn after_start(&mut self, label: String) {
        self.state = PlayState::Playing;
        self.triggered_next = false;
        self.wait_until = None;
        self.on_air_label = label.clone();
        *self.now_text.lock() = label.clone();
    }

    /// Whether a file is an advert, a jingle or a song (for the play history and reports).
    pub fn kind_of(&self, path: &Path) -> &'static str {
        let in_rule =
            |adverts: bool| {
                self.cfg.rotations.iter().filter(|r| r.adverts == adverts).any(|r| {
                    (!r.folder.trim().is_empty() && path.starts_with(r.folder.trim())) || r.files.iter().any(|f| Path::new(f) == path)
                })
            };
        if self.cfg.ads.iter().any(|a| Path::new(&a.path) == path) || in_rule(true) {
            "advert"
        } else if self.cfg.carts.iter().any(|c| Path::new(&c.path) == path) || in_rule(false) {
            "jingle"
        } else {
            "song"
        }
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
    /// What a speech row will say (placeholders filled in) and with which voice settings.
    pub fn speech_for(&self, text: &str, voice: &str, rate: i32, opts: &crate::items::SpeechOpts, row: usize) -> (String, live::Voice) {
        let mut said = self.fill_speech(text, row);
        if opts.spell_numbers {
            said = live::spell_digits(&said);
        }
        let name = if voice.is_empty() { self.cfg.announce.voice.clone() } else { voice.to_string() };
        (said, live::Voice { name, rate, pitch: opts.pitch, line_pause_ms: opts.line_pause_ms })
    }

    /// Render the next speech row ahead of time so it starts without a gap.
    fn prepare_next_speech(&mut self) {
        let Some(c) = self.current else { return };
        let Some((i, e)) =
            self.playlist.iter().enumerate().skip(c + 1).find(|(_, e)| !matches!(e.kind, ItemKind::Comment(_) | ItemKind::HourInfo { .. }))
        else {
            return;
        };
        let ItemKind::Tts { text, voice, rate, opts } = &e.kind else { return };
        if self.status.total <= 0.0 || self.status.total - self.status.pos > 25.0 {
            return;
        }
        let (said, v) = self.speech_for(text, voice, *rate, opts, i);
        let key = format!("{i}|{said}");
        if self.speech_ready == key {
            return;
        }
        self.speech_ready = key;
        let sink = self.log_sink.clone();
        std::thread::spawn(move || {
            if let Err(e) = live::speech_file(&said, &v) {
                sink.lock().push(format!("Text to speech: {e}"));
            }
        });
    }

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
        if self.forward(|tab| sync::Cmd::Command { tab, text: cmd.to_string() }) {
            return;
        }
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
            Some("GENERATE") | Some("APPEND") => {
                // GENERATE <template> = new playlist, APPEND <template> = add after the playlist
                let append = words.first().is_some_and(|w| w.eq_ignore_ascii_case("APPEND"));
                let name = rest(1);
                match self.cfg.templates.iter().find(|t| t.name.eq_ignore_ascii_case(name.trim())).map(|t| t.name.clone()) {
                    Some(n) => {
                        let k = self.generate_playlist(&n, append);
                        self.say(format!("{k} tracks from template '{n}'"));
                    }
                    None => self.say(format!("{}: no template called \"{name}\"", if append { "APPEND" } else { "GENERATE" })),
                }
            }
            Some("JINGLES") | Some("ADVERTS") | Some("RULE") => {
                // RULE <name>: queue that advert / jingle rule's files after the song on air
                let name = rest(1);
                match self.cfg.ad_rules.iter().find(|r| r.name.eq_ignore_ascii_case(name.trim())).map(|r| r.id) {
                    Some(id) => {
                        self.fire_rule(id, None);
                    }
                    None => self.say(format!("RULE: no advert / jingle rule called \"{name}\"")),
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
            library::audio_files_cached(p)
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
        if self.forward(|_| sync::Cmd::SetCues { path: path.to_path_buf(), cues: c, duration }) {
            return;
        }
        c.manual = true;
        c.clamp(duration);
        self.cues.insert(path.to_path_buf(), c);
        self.cues_dirty = true;
        self.refresh_active_cues(path);
    }

    /// Whether this file has cue points set by hand.
    pub fn manual_cues_for(&self, path: &Path) -> bool {
        self.cues.get(path).is_some_and(|c| c.manual)
    }

    /// Forget hand-edited cues; smart analysis takes over again.
    pub fn clear_manual_cues(&mut self, path: &Path) {
        if self.forward(|_| sync::Cmd::ClearCues { path: path.to_path_buf() }) {
            return;
        }
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
        if self.forward(|_| sync::Cmd::ReanalyseCues) {
            return;
        }
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
    /// Files a rule can choose from: the Adverts list (only adverts in their dates and days),
    /// the hand-picked files, or the folder.
    fn rule_pool(&self, r: &crate::config::AdRule) -> Vec<PathBuf> {
        if r.adverts && r.use_ad_list {
            let now = Local::now();
            self.cfg
                .ads
                .iter()
                .filter(|a| a.active(&now) && !a.path.is_empty())
                .map(|a| PathBuf::from(&a.path))
                .filter(|p| p.exists())
                .collect()
        } else if !r.files.is_empty() {
            r.files.iter().map(PathBuf::from).filter(|p| p.exists()).collect()
        } else if !r.folder.trim().is_empty() {
            library::audio_files_cached(Path::new(&r.folder))
        } else {
            vec![]
        }
    }

    /// Whether a file belongs to an advert / jingle rule or the Adverts list (those don't count as songs).
    fn is_rule_file(&self, path: &Path) -> bool {
        self.cfg.ads.iter().any(|a| Path::new(&a.path) == path)
            || self
                .cfg
                .ad_rules
                .iter()
                .any(|r| (!r.folder.trim().is_empty() && path.starts_with(r.folder.trim())) || r.files.iter().any(|f| Path::new(f) == path))
    }

    /// Run rule `id` now: pick its files and put them after `after` (or after the on-air row).
    /// Returns how many were queued.
    pub fn fire_rule(&mut self, id: u64, after: Option<usize>) -> usize {
        if self.forward(|tab| sync::Cmd::FireRule { tab, id, after }) {
            return 0;
        }
        use crate::config::RotPick;
        let Some(k) = self.cfg.ad_rules.iter().position(|r| r.id == id) else { return 0 };
        let r = self.cfg.ad_rules[k].clone();
        let pool = self.rule_pool(&r);
        if pool.is_empty() {
            self.say(format!("{}: nothing to play (check its folder / files)", r.name));
            return 0;
        }
        let want = (r.count.max(1) as usize).min(pool.len());
        let picked: Vec<PathBuf> = match r.pick {
            RotPick::All => pool.clone(),
            RotPick::Random => pool.choose_multiple(&mut rand::thread_rng(), want).cloned().collect(),
            RotPick::InOrder => {
                let start = r.next % pool.len();
                let v: Vec<PathBuf> = (0..want).map(|j| pool[(start + j) % pool.len()].clone()).collect();
                self.cfg.ad_rules[k].next = (start + want) % pool.len();
                v
            }
        };
        let n = picked.len();
        let rows: Vec<PlEntry> = picked.iter().map(|p| self.entry_for(p)).collect();
        let at = after.or(self.current).map(|c| c + 1).unwrap_or(0);
        self.insert_rows(at, rows);
        self.say(format!("{}: {n} {} queued", r.name, if r.adverts { "advert(s)" } else { "jingle(s)" }));
        n
    }

    /// Song-count rules: after every N songs, insert adverts / jingles after the row that just started.
    fn apply_rotations(&mut self, idx: usize, t: &Track) {
        use crate::config::Trigger;
        // an advert or jingle doesn't count as a song
        if self.is_rule_file(&t.path) {
            return;
        }
        let now = Local::now();
        let due: Vec<u64> = self
            .cfg
            .ad_rules
            .iter()
            .filter(|r| r.trigger == Trigger::Songs && r.in_force(&now))
            .filter_map(|r| {
                let c = self.songs_since.entry(r.id).or_insert(0);
                *c += 1;
                (*c >= r.every_songs.max(1)).then_some(r.id)
            })
            .collect();
        // later rules go after earlier ones
        let mut after = idx;
        for id in due {
            let n = self.fire_rule(id, Some(after));
            if n > 0 {
                self.songs_since.insert(id, 0);
                after += n;
            }
        }
    }

    pub fn play_pressed(&mut self, selected: Option<usize>) {
        if self.forward(|tab| sync::Cmd::PlayPressed { tab, selected }) {
            return;
        }
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
        if self.forward(|_| sync::Cmd::PauseToggle) {
            return;
        }
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
        if self.forward(|_| sync::Cmd::Stop) {
            return;
        }
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
        if self.forward(|_| sync::Cmd::Next) {
            return;
        }
        let xf = self.cfg.crossfade_secs;
        if !self.advance(xf) {
            self.stop();
        }
    }

    pub fn prev(&mut self) {
        if self.forward(|_| sync::Cmd::Prev) {
            return;
        }
        if let Some(c) = self.current {
            if c > 0 {
                let xf = self.cfg.crossfade_secs;
                self.play_index(c - 1, xf);
            }
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        if self.forward(|_| sync::Cmd::Volume { v }) {
            self.cfg.volume = v;
            return;
        }
        self.cfg.volume = v;
        self.engine.set_volume(v);
    }

    /// Fire a cart pad; pressing a pad that is already playing stops it.
    pub fn trigger_cart(&mut self, idx: usize) {
        if self.forward(|_| sync::Cmd::Cart { idx }) {
            return;
        }
        if let Some(p) = self.cfg.carts.get(idx).map(|c| c.path.clone()) {
            if !p.is_empty() {
                if self.engine.stop_carts(Some(Path::new(&p)), 0.4) > 0 {
                    return;
                }
                match self.engine.trigger_cart(Path::new(&p)) {
                    Err(e) => self.say(format!("Cart error: {e}")),
                    Ok(()) => {
                        // jingles from the cart wall go into the play history (and the jingle report)
                        let t = self.entry_for(Path::new(&p)).track;
                        self.log_history(&t.label(), "jingle", Some(&t.path), t.duration);
                    }
                }
            }
        }
    }

    /// Stop every jingle that is playing (quick fade).
    pub fn stop_carts(&self) {
        if self.forward(|_| sync::Cmd::StopCarts) {
            return;
        }
        let n = self.engine.stop_carts(None, 0.4);
        if n > 0 {
            self.say(format!("Stopped {n} jingle(s)"));
        }
    }

    // ------------------------------------------------------------- playlist
    /// A row for a file with its tags (from the library, or read now).
    pub fn entry_for(&self, p: &Path) -> PlEntry {
        let track = match self.lib_index.get(p) {
            Some(&i) => self.library[i].clone(),
            None => library::read_track(p),
        };
        PlEntry::file(track)
    }

    /// A row for a file without waiting for the disk: files outside the library show their file
    /// name until their tags have been read in the background (`drain_tags` fills them in).
    pub fn entry_quick(&self, p: &Path) -> PlEntry {
        match self.lib_index.get(p) {
            Some(&i) => PlEntry::file(self.library[i].clone()),
            None => {
                library::read_tags_later(p);
                PlEntry::file(Track::placeholder(p))
            }
        }
    }

    /// Put tags read in the background into the rows still showing just a file name.
    fn drain_tags(&mut self) {
        let read = library::tags_read();
        if read.is_empty() {
            return;
        }
        let by_path: HashMap<&PathBuf, &Track> = read.iter().map(|t| (&t.path, t)).collect();
        let tabs = self.tabs.iter_mut().map(|t| &mut t.entries);
        for e in std::iter::once(&mut self.playlist).chain(tabs).flatten() {
            if e.is_file() && e.track.is_placeholder() {
                if let Some(t) = by_path.get(&e.track.path) {
                    e.track = (*t).clone();
                }
            }
        }
    }

    /// Library tracks as they are; folders become their audio files (remembered between calls).
    fn expand(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut out = vec![];
        for p in paths {
            if self.lib_index.contains_key(&p) {
                out.push(p);
            } else if p.is_dir() {
                out.extend(library::audio_files_cached(&p));
            } else if library::is_audio(&p) {
                out.push(p);
            }
        }
        out
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        for p in self.expand(paths) {
            let e = self.entry_quick(&p);
            self.playlist.push(e);
        }
    }

    pub fn insert_next(&mut self, paths: Vec<PathBuf>) {
        let at = self.current.map(|c| c + 1).unwrap_or(0);
        let rows: Vec<PlEntry> = self.expand(paths).iter().map(|p| self.entry_quick(p)).collect();
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
        self.m3u_rows(parse_m3u(path))
    }

    /// Playlist rows for what `parse_m3u` read.
    pub fn m3u_rows(&self, rows: Vec<M3uRow>) -> Vec<PlEntry> {
        rows.into_iter()
            .map(|r| match r {
                M3uRow::Row(e) => e,
                M3uRow::File(p, voice) => {
                    let mut e = self.entry_quick(&p);
                    if voice {
                        e.kind = ItemKind::VoiceTrack;
                    }
                    e
                }
            })
            .collect()
    }

    pub fn load_m3u(&mut self, path: &Path) {
        let rows = self.read_m3u(path);
        if rows.is_empty() && !path.exists() {
            return;
        }
        self.replace_rows(rows);
    }

    /// Replace the whole list (as loading an M3U does).
    pub fn replace_rows(&mut self, rows: Vec<PlEntry>) {
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
        if self.forward(|_| sync::Cmd::Rescan) {
            return;
        }
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
            self.library = v;
            self.reindex();
            self.save_library();
            self.scanning = false;
            self.scan_rx = None;
            self.say(format!("Library scan complete: {} tracks", self.library.len()));
        }
    }

    /// Save edited tags: the library, every playlist row and the on-air track show them at once, the
    /// file's own tags are written on a background thread, and library.json is saved so the edit
    /// survives a restart. If the file can't be written, the old tags come back.
    pub fn update_tags(&mut self, t: Track) {
        if self.forward(|_| sync::Cmd::WriteTags { track: t.clone() }) {
            return;
        }
        let old = self
            .lib_index
            .get(&t.path)
            .map(|&i| self.library[i].clone())
            .or_else(|| self.playlist_track(&t.path))
            .unwrap_or_else(|| Track::from_path(&t.path));
        self.apply_track(&t);
        let tx = self.tag_tx.clone();
        thread::spawn(move || {
            let r = library::write_tags(&t).map_err(|e| e.to_string());
            let _ = tx.send((t, old, r));
        });
    }

    /// Results of background tag writes.
    fn drain_tag_writes(&mut self) {
        while let Ok((t, old, r)) = self.tag_rx.try_recv() {
            let name = t.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match r {
                Ok(()) => self.say(format!("Tags saved to {name}")),
                Err(e) => {
                    self.apply_track(&old);
                    self.say(format!("Tag write failed for {name}: {e}"));
                }
            }
        }
    }

    /// The track of a playlist row (any tab) that plays this file.
    fn playlist_track(&self, p: &Path) -> Option<Track> {
        std::iter::once(&self.playlist)
            .chain(self.tabs.iter().map(|t| &t.entries))
            .flatten()
            .find(|e| e.is_file() && e.track.path == p)
            .map(|e| e.track.clone())
    }

    /// Show a track's tags everywhere the file is used, and save the library and playlists.
    fn apply_track(&mut self, t: &Track) {
        if let Some(&i) = self.lib_index.get(&t.path) {
            self.library[i] = t.clone();
            self.library_ver += 1;
            self.save_library();
        }
        let tabs = self.tabs.iter_mut().map(|t| &mut t.entries);
        for e in std::iter::once(&mut self.playlist).chain(tabs).flatten() {
            if e.is_file() && e.track.path == t.path {
                e.track = t.clone();
            }
        }
        if let Some(o) = self.on_air.as_mut().filter(|o| o.path == t.path) {
            *o = t.clone();
        }
        self.save_tabs();
    }

    // ------------------------------------------------------- library cleanup
    /// Write library.json on the background writer.
    pub fn save_library(&self) {
        if self.link.is_some() {
            return;
        }
        if let Ok(s) = serde_json::to_string(&self.library) {
            write_in_background(data_dir().join("library.json"), s);
        }
    }

    /// Every file row in every playlist tab (the playing one included), as (path, label).
    pub fn playlist_files(&self) -> Vec<(PathBuf, String)> {
        let tabs = self.tabs.iter().map(|t| &t.entries);
        std::iter::once(&self.playlist)
            .chain(tabs)
            .flatten()
            .filter(|e| e.is_file())
            .map(|e| (e.track.path.clone(), e.track.label()))
            .collect()
    }

    /// Point everything that uses a file (library, playlist rows in every tab, cue points, play
    /// counts) at another one. `pairs`: old path, and the new file with its tags (read off the UI
    /// thread). When the new file is already in the library the old entry goes.
    pub fn relink(&mut self, pairs: Vec<(PathBuf, Track)>) {
        let tracks: HashMap<PathBuf, Track> = pairs
            .iter()
            .map(|(_, t)| (t.path.clone(), self.lib_index.get(&t.path).map(|&i| self.library[i].clone()).unwrap_or_else(|| t.clone())))
            .collect();
        let pairs: Vec<(PathBuf, PathBuf)> = pairs.into_iter().map(|(from, t)| (from, t.path)).collect();
        let map: HashMap<&PathBuf, &PathBuf> = pairs.iter().map(|(a, b)| (a, b)).collect();
        let in_lib: std::collections::HashSet<PathBuf> =
            pairs.iter().filter(|(_, to)| self.lib_index.contains_key(to)).map(|(_, to)| to.clone()).collect();
        self.library.retain(|t| map.get(&t.path).is_none_or(|to| !in_lib.contains(*to)));
        for t in self.library.iter_mut() {
            if let Some(to) = map.get(&t.path) {
                t.path = (*to).clone();
            }
        }
        self.reindex();
        let tabs = self.tabs.iter_mut().map(|t| &mut t.entries);
        for e in std::iter::once(&mut self.playlist).chain(tabs).flatten().filter(|e| e.is_file()) {
            if let Some(to) = map.get(&e.track.path) {
                e.track = tracks[*to].clone();
            }
        }
        for (from, to) in &pairs {
            if !self.cues.contains_key(to) {
                if let Some(c) = self.cues.remove(from) {
                    self.cues.insert(to.clone(), c);
                    self.cues_dirty = true;
                }
            }
            if let Some(n) = self.plays.remove(from) {
                let e = self.plays.entry(to.clone()).or_insert(n);
                *e = (*e).max(n);
                self.plays_dirty = true;
            }
        }
        self.save_library();
        self.save_all();
    }

    /// Take files out of the library (the files themselves are left alone).
    pub fn forget_tracks(&mut self, paths: &std::collections::HashSet<PathBuf>) {
        self.library.retain(|t| !paths.contains(&t.path));
        self.reindex();
        self.save_library();
    }

    /// Remove the playlist rows (in every tab) that play these files. Returns how many went.
    pub fn remove_rows_for(&mut self, paths: &std::collections::HashSet<PathBuf>) -> usize {
        let gone = |e: &PlEntry| e.is_file() && paths.contains(&e.track.path);
        let before: usize = self.playlist.len() + self.tabs.iter().map(|t| t.entries.len()).sum::<usize>();
        let sel: BTreeSet<usize> =
            self.playlist.iter().enumerate().filter(|(i, e)| gone(e) && Some(*i) != self.current).map(|(i, _)| i).collect();
        self.remove(&sel);
        for t in self.tabs.iter_mut() {
            let cur = t.current.and_then(|c| t.entries.get(c)).map(|e| (e.track.path.clone(), e.title()));
            t.entries.retain(|e| !gone(e));
            t.current = cur.and_then(|(p, n)| t.entries.iter().position(|e| e.track.path == p && e.title() == n));
        }
        let after: usize = self.playlist.len() + self.tabs.iter().map(|t| t.entries.len()).sum::<usize>();
        self.save_all();
        before - after
    }

    /// Tags the cleanup tool has written: show them in the library and the playlists.
    pub fn tags_written(&mut self, tracks: Vec<Track>) {
        for t in tracks {
            if let Some(&i) = self.lib_index.get(&t.path) {
                self.library[i] = t.clone();
            }
            let tabs = self.tabs.iter_mut().map(|t| &mut t.entries);
            for e in std::iter::once(&mut self.playlist).chain(tabs).flatten().filter(|e| e.is_file() && e.track.path == t.path) {
                e.track = t.clone();
            }
        }
        self.library_ver += 1;
        self.save_library();
        self.save_all();
    }

    // ------------------------------------------------------------- encoders
    pub fn start_encoder(&mut self, i: usize) {
        if self.forward(|_| sync::Cmd::EncoderStart { i }) {
            return;
        }
        if i >= self.cfg.encoders.len() {
            return;
        }
        if self.handles[i].as_ref().map(|h| h.is_running()).unwrap_or(false) {
            return;
        }
        self.handles[i] = Some(EncoderHandle::start(
            self.cfg.encoders[i].clone(),
            self.engine.clone(),
            self.now_text.clone(),
            self.cfg.ffmpeg_path.clone(),
        ));
    }

    pub fn stop_encoder(&mut self, i: usize) {
        if self.forward(|_| sync::Cmd::EncoderStop { i }) {
            return;
        }
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
        if self.forward(|tab| sync::Cmd::Generate { tab, name: name.to_string(), append }) {
            return 0;
        }
        let Some(tpl) = self.cfg.templates.iter().find(|t| t.name == name).cloned() else {
            self.say(format!("Template '{name}' not found"));
            return 0;
        };
        // advert / jingle rules used as pattern steps or markers: their files join the pool
        let mut lib = self.library.clone();
        let mut preset: HashMap<String, Vec<usize>> = HashMap::new();
        for src in tpl.pattern.iter().map(|p| &p.category).chain(tpl.markers.iter().map(|m| &m.category)) {
            let Some(id) = src.strip_prefix("rule:").and_then(|s| s.parse::<u64>().ok()) else { continue };
            if preset.contains_key(src) {
                continue;
            }
            let Some(r) = self.cfg.ad_rules.iter().find(|r| r.id == id).cloned() else { continue };
            let start = lib.len();
            for p in self.rule_pool(&r) {
                lib.push(self.entry_for(&p).track);
            }
            preset.insert(src.clone(), (start..lib.len()).collect());
        }
        let tracks = crate::generator::generate(&tpl, &self.cfg.categories, &lib, &preset);
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
        if self.forward(|_| sync::Cmd::Seek { secs }) {
            return;
        }
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
    fn log_history(&mut self, text: &str, kind: &'static str, path: Option<&Path>, secs: f32) {
        let time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let q = |s: &str| s.replace('"', "'").replace(['\n', '\r'], " ");
        let path = path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let line = format!("\"{}\",\"{}\",\"{kind}\",\"{}\",\"{:.1}\"\n", time, q(text), q(&path), secs.max(0.0));
        append_in_background(data_dir().join("history.csv"), line);
        self.history.push(HistoryItem { time, text: text.to_string(), kind });
        if self.history.len() > 500 {
            self.history.remove(0);
        }
        self.log_sink.lock().push(format!("Started: {text}"));
    }

    /// Playback reached an Event row (`i`). Returns true when playback must not carry on to the
    /// next row: the event put another playlist on air, or stopped.
    fn row_event(&mut self, i: usize, action: ActionKind, path: &str, arg: f32, title: &str) -> bool {
        let path = path.trim();
        self.say(format!("Playlist {}", title.replacen("Event:", "event:", 1)));
        match action {
            ActionKind::LoadPlaylistAndPlay => {
                if Path::new(path).exists() {
                    self.load_m3u(Path::new(path)); // carries on from the new list's first row
                } else {
                    self.say(format!("Event: file not found: {path}"));
                }
            }
            ActionKind::PlayFile | ActionKind::InsertNext => {
                if Path::new(path).exists() {
                    let rows: Vec<PlEntry> = self.expand(vec![PathBuf::from(path)]).iter().map(|p| self.entry_quick(p)).collect();
                    self.insert_rows(i + 1, rows);
                } else {
                    self.say(format!("Event: file not found: {path}"));
                }
            }
            ActionKind::Stop => {
                self.stop();
                return true;
            }
            ActionKind::StartEncoder => self.start_encoder(arg as usize),
            ActionKind::StopEncoder => self.stop_encoder(arg as usize),
            ActionKind::SetVolume => self.set_volume(arg.clamp(0.0, 1.0)),
            ActionKind::GenerateReplace | ActionKind::GenerateAppend => {
                // a new playlist carries on from its first row; added tracks follow on after the list
                let n = self.generate_playlist(path, action == ActionKind::GenerateAppend);
                if n > 0 {
                    self.say(format!("Event: {n} tracks from template '{path}'"));
                }
            }
            ActionKind::RunRule => {
                self.fire_rule(arg as u64, Some(i));
            }
            ActionKind::SwitchPlaylist => self.run_command(&format!("PLAYLIST {path}")),
            ActionKind::Command => self.run_command(path),
            ActionKind::RestartPlaylist => {
                self.playlist.iter_mut().for_each(|e| e.played = false);
                self.pending_jump = Some(0);
            }
        }
        // another playlist goes on air next tick: don't start this list's next row in between
        self.pending_tab.is_some() || self.state == PlayState::Stopped
    }

    pub fn run_event(&mut self, e: &Event) {
        if self.forward(|_| sync::Cmd::Event { event: e.clone() }) {
            return;
        }
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
            ActionKind::GenerateReplace | ActionKind::GenerateAppend => {
                let append = e.action == ActionKind::GenerateAppend;
                let was_playing = self.state == PlayState::Playing;
                let n = self.generate_playlist(e.path.trim(), append);
                if n > 0 {
                    self.say(format!("Scheduler: {n} tracks from template '{}'", e.path.trim()));
                    if !append && was_playing {
                        self.play_index(0, xf); // a fresh playlist carries on playing
                    }
                }
            }
            ActionKind::RunRule => {
                self.fire_rule(e.arg as u64, None);
            }
            ActionKind::SwitchPlaylist => self.run_command(&format!("PLAYLIST {}", e.path.trim())),
            ActionKind::Command => self.run_command(e.path.trim()),
            ActionKind::RestartPlaylist => {
                self.playlist.iter_mut().for_each(|r| r.played = false);
                self.play_index(0, xf);
            }
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
        use crate::config::{AdRule, RotPick};
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
        p.cfg.ad_rules = vec![AdRule {
            id: 7,
            name: "Adverts".into(),
            enabled: true,
            adverts: true,
            files: ads.clone(),
            every_songs: 2,
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
    fn rows_added_without_tags_get_them_in_the_background() {
        let dir = std::env::temp_dir().join("rm_test_quick_rows");
        let _ = std::fs::create_dir_all(&dir);
        let wav = dir.join("two_seconds.wav");
        crate::engine::write_wav(&wav, &vec![0.0; 44100 * 2 * 2], 2, 44100).unwrap();
        std::fs::write(dir.join("list.m3u"), "#EXTM3U\ntwo_seconds.wav\nmissing.wav\n").unwrap();
        let mut p = playout();
        let rows = p.m3u_rows(parse_m3u(&dir.join("list.m3u")));
        assert_eq!(rows.len(), 1, "files that don't exist are left out");
        assert!(rows[0].track.is_placeholder());
        assert_eq!(rows[0].track.title, "two_seconds");
        p.insert_rows(0, rows);
        let t0 = Instant::now();
        while p.playlist[0].track.duration == 0.0 && t0.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            p.drain_tags();
            // tests run side by side and another test's playout may have taken the result: ask again
            library::read_tags_later(&wav);
        }
        assert!((p.playlist[0].track.duration - 2.0).abs() < 0.1, "got {}", p.playlist[0].track.duration);
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
    fn nested_with_tab_edits_the_viewed_tab() {
        let mut p = playout();
        p.playlist = vec![file("main1", 60.0)];
        let t = p.new_tab(Some("Evening".into()));
        let u = p.new_tab(Some("Night".into()));
        // a dialog drawn inside `with_tab` inserts into the same tab with another `with_tab`
        p.with_tab(t, |c| {
            assert_eq!(c.tab_len(t), 0);
            assert_eq!(c.tab_len(0), 1, "the on-air tab is still readable while another is swapped in");
            c.with_tab(t, |c| c.insert_rows(0, vec![file("eve1", 60.0)]));
            c.append_to_tab(0, vec![file("main2", 60.0)]);
            c.with_tab(u, |c| c.insert_rows(0, vec![file("night1", 60.0)]));
            c.with_tab(0, |c| c.insert_rows(0, vec![file("main0", 60.0)]));
            assert_eq!(c.tab_len(t), 1);
        });
        assert_eq!(p.playing_tab, 0);
        assert_eq!(p.playlist.iter().map(|e| e.title()).collect::<Vec<_>>(), ["main0", "main1", "main2"]);
        assert_eq!(p.tab_rows(t).iter().map(|e| e.title()).collect::<Vec<_>>(), ["eve1"]);
        assert_eq!(p.tab_rows(u).iter().map(|e| e.title()).collect::<Vec<_>>(), ["night1"]);
    }

    #[test]
    fn track_editor_tags_reach_the_file_and_library() {
        let mut p = playout();
        let wav = std::env::temp_dir().join("rm_tag_test.wav");
        crate::engine::write_wav(&wav, &vec![0.1; crate::engine::SR as usize], 2, crate::engine::SR).unwrap();
        p.library = vec![Track::from_path(&wav)];
        p.reindex();
        let t = p.new_tab(Some("Evening".into()));
        p.with_tab(t, |c| c.add_paths(vec![wav.clone()]));
        let mut edited = p.library[0].clone();
        (edited.artist, edited.title, edited.album) = ("Some Artist".into(), "New Title".into(), "Album".into());
        p.update_tags(edited);
        // shown everywhere straight away
        assert_eq!(p.library[0].title, "New Title");
        assert_eq!(p.tab_rows(t)[0].track.title, "New Title");
        let wait = || std::thread::sleep(std::time::Duration::from_millis(20));
        for _ in 0..250 {
            p.drain_tag_writes();
            if p.notify.lock().contains("Tags saved") {
                break;
            }
            wait();
        }
        let back = Track::from_path(&wav);
        assert_eq!((back.artist.as_str(), back.title.as_str(), back.album.as_str()), ("Some Artist", "New Title", "Album"));

        // a file that can't be written gets its old tags back
        let gone = Track { path: PathBuf::from("/nonexistent/rm_gone.mp3"), title: "Old".into(), ..Default::default() };
        p.library.push(gone.clone());
        p.reindex();
        p.update_tags(Track { title: "New".into(), ..gone });
        for _ in 0..250 {
            p.drain_tag_writes();
            if p.notify.lock().contains("Tag write failed") {
                break;
            }
            wait();
        }
        assert_eq!(p.library.last().unwrap().title, "Old");
    }

    #[test]
    fn playlist_events_return_to_main_and_loop() {
        let mut p = playout();
        let dir = std::env::temp_dir().join("rm_test_events");
        let _ = std::fs::create_dir_all(&dir);
        let wav = |name: &str| {
            let f = dir.join(format!("{name}.wav"));
            crate::engine::write_wav(&f, &vec![0.1; crate::engine::SR as usize * 2], 2, crate::engine::SR).unwrap();
            p.entry_for(&f)
        };
        let ev = |action, path: &str| PlEntry::special(ItemKind::Event { action, path: path.into(), arg: 0.0 }, "", 0.0);
        let (m1, m2, e1) = (wav("main1"), wav("main2"), wav("evening1"));
        p.playlist = vec![m1.clone(), m2.clone(), ev(ActionKind::RestartPlaylist, "")];
        let evening = p.new_tab(Some("Evening".into()));
        p.with_tab(evening, |c| c.playlist = vec![e1.clone(), ev(ActionKind::SwitchPlaylist, "Main")]);
        let on_air = |p: &Playout| p.on_air.as_ref().map(|t| t.path.clone());

        // main is playing its first track when the scheduler puts the evening show on air
        assert!(p.play_index(0, 0.0));
        p.put_tab_on_air(evening);
        assert_eq!(p.playing_tab, evening);
        assert_eq!(on_air(&p), Some(e1.track.path.clone()));

        // the evening show ends on its event row: main comes back instead of dead air
        assert!(p.advance(0.0), "the event row holds playback for the switch");
        assert_eq!(p.pending_tab, Some(0));
        p.tick();
        assert_eq!(p.playing_tab, 0);
        assert!(p.state == PlayState::Playing);
        assert_eq!(on_air(&p), Some(m2.track.path.clone()), "main carries on where it left off");

        // main's last row plays it again from the top
        assert!(p.advance(0.0));
        assert_eq!(p.current, Some(0));
        assert_eq!(on_air(&p), Some(m1.track.path.clone()));
        assert!(!p.playlist[1].played, "a fresh loop");

        // a file event queues its file right after the row
        p.playlist = vec![m1.clone(), ev(ActionKind::InsertNext, &e1.track.path.to_string_lossy()), m2.clone()];
        assert!(p.play_index(0, 0.0));
        assert!(p.advance(0.0));
        assert_eq!(on_air(&p), Some(e1.track.path.clone()));
        assert_eq!(p.playlist.len(), 4);

        // a stop event ends playback
        p.playlist = vec![m1.clone(), ev(ActionKind::Stop, "")];
        assert!(p.play_index(0, 0.0));
        p.advance(0.0);
        assert!(p.state == PlayState::Stopped);
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
