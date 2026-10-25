//! Audio engine: a single master bus (what listeners hear) fed by two crossfading
//! decks plus cart-wall slots. The same mixed PCM is tapped for the stream encoders,
//! so what you hear is exactly what goes on air.

use crate::dsp::{Dsp, DspConfig};
use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use parking_lot::Mutex;
use rodio::{source::UniformSourceIterator, Decoder, Source};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::{
    collections::VecDeque,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const SR: u32 = 44_100;
const BLOCK_FRAMES: usize = 1024;
const CART_SLOTS: usize = 8;

/// Interleaved stereo f32 at `SR` Hz.
pub type Pcm = Box<dyn Iterator<Item = f32> + Send>;

/// Samples per decoded block (2048 stereo frames ≈ 46 ms).
const AHEAD_BLOCK: usize = 4096;
/// Blocks decoded ahead of the sound card (≈ 3 s): a slow disk or a busy CPU is ridden out.
const AHEAD_BLOCKS: usize = 64;
/// Blocks decoded before `Ahead::start` returns, so a track starts on the very next callback.
const AHEAD_FIRST: usize = 2;

/// A file decoded ahead on its own thread. The sound card callback only copies samples out of
/// memory: it never reads the disk or runs a decoder, and never waits for either.
pub struct Ahead {
    rx: Receiver<Vec<f32>>,
    /// used blocks go back to the decoder thread, so the audio thread never frees memory
    back: Sender<Vec<f32>>,
    cur: Vec<f32>,
    pos: usize,
}

enum Pull {
    Frame(f32, f32),
    /// the decoder hasn't caught up (very slow disk): play silence for now, don't end the track
    Wait,
    End,
}

impl Ahead {
    pub fn start(mut src: Pcm) -> Ahead {
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(AHEAD_BLOCKS);
        let (back, back_rx) = crossbeam_channel::bounded::<Vec<f32>>(AHEAD_BLOCKS);
        let fill = |src: &mut Pcm, mut b: Vec<f32>| {
            b.clear();
            // a plain loop: rodio's converters can panic in `size_hint`, which `extend` would call
            while b.len() < AHEAD_BLOCK {
                match src.next() {
                    Some(x) => b.push(x),
                    None => break,
                }
            }
            if b.len() % 2 == 1 {
                b.push(0.0); // keep L/R aligned
            }
            b
        };
        let mut ended = false;
        for _ in 0..AHEAD_FIRST {
            let b = fill(&mut src, Vec::with_capacity(AHEAD_BLOCK));
            ended = b.len() < AHEAD_BLOCK;
            if !b.is_empty() {
                let _ = tx.try_send(b);
            }
            if ended {
                break;
            }
        }
        if !ended {
            std::thread::Builder::new()
                .name("decoder".into())
                .spawn(move || loop {
                    let b = fill(&mut src, back_rx.try_recv().unwrap_or_else(|_| Vec::with_capacity(AHEAD_BLOCK)));
                    let last = b.len() < AHEAD_BLOCK;
                    // blocks while the buffer is full; fails once the deck is gone
                    if (!b.is_empty() && tx.send(b).is_err()) || last {
                        return; // the decoder and the file are closed here, not on the audio thread
                    }
                })
                .ok();
        }
        Ahead { rx, back, cur: Vec::new(), pos: 0 }
    }

    #[inline]
    fn pull(&mut self) -> Pull {
        if self.pos + 1 >= self.cur.len() {
            match self.rx.try_recv() {
                Ok(b) => {
                    let old = std::mem::replace(&mut self.cur, b);
                    let _ = self.back.try_send(old);
                    self.pos = 0;
                    if self.cur.len() < 2 {
                        return Pull::End;
                    }
                }
                Err(TryRecvError::Empty) => return Pull::Wait,
                Err(TryRecvError::Disconnected) => return Pull::End,
            }
        }
        let f = Pull::Frame(self.cur[self.pos], self.cur[self.pos + 1]);
        self.pos += 2;
        f
    }
}

/// Where a deck's samples come from.
enum Input {
    /// files: decoded ahead on another thread
    Ahead(Ahead),
    /// live feeds (streams, line input, speech) already buffer on their own thread
    Direct(Pcm),
}

impl Input {
    #[inline]
    fn pull(&mut self) -> Pull {
        match self {
            Input::Ahead(a) => a.pull(),
            Input::Direct(src) => match (src.next(), src.next()) {
                (Some(l), Some(r)) => Pull::Frame(l, r),
                _ => Pull::End,
            },
        }
    }
}

#[derive(Default)]
struct Deck {
    src: Option<Input>,
    gain: f32,
    target: f32,
    step: f32,
    frames: u64,
    total_secs: f32,
    fading_out: bool,
    path: Option<PathBuf>,
    heard: bool,
    skipped: u32,
    silent_run: u32,
    /// absolute frame at which playback ends (cue out)
    end_frame: Option<u64>,
    /// (start frame, end frame) of a scheduled fade-out (cue fade out → cue out)
    auto_fade: Option<(u64, u64)>,
    /// per-track playback level (linear)
    level: f32,
    /// live source (stream, line input): never silence-trimmed
    live: bool,
}

/// How a track should be started.
#[derive(Clone, Copy, Debug, Default)]
pub struct CueSpec {
    pub cue_in: f32,
    pub cue_out: Option<f32>,
    pub fade_in: f32,
    pub fade_out_at: Option<f32>,
    /// per-track playback level in dB
    pub gain_db: f32,
}

const SKIP_MAX: u32 = SR * 10;

impl Deck {
    fn new(src: Input, total_secs: f32, gain: f32) -> Self {
        Deck {
            src: Some(src),
            gain,
            target: gain,
            step: 0.0,
            frames: 0,
            total_secs,
            fading_out: false,
            path: None,
            heard: false,
            skipped: 0,
            silent_run: 0,
            end_frame: None,
            auto_fade: None,
            level: 1.0,
            live: false,
        }
    }

    fn fade_to(&mut self, target: f32, secs: f32) {
        let n = (secs * SR as f32).max(1.0);
        self.target = target;
        self.step = (target - self.gain) / n;
        self.fading_out = target <= 0.0;
    }

    /// Adds this deck into `out`. Returns true if the source ran dry (natural end).
    /// `trim` = (silence threshold, max trailing silent frames).
    fn apply_cues(&mut self, spec: &CueSpec) {
        self.level = 10f32.powf(spec.gain_db.clamp(-24.0, 24.0) / 20.0);
        self.end_frame = spec.cue_out.map(|s| (s.max(0.0) * SR as f32) as u64);
        self.auto_fade = match (spec.fade_out_at, spec.cue_out) {
            (Some(a), Some(b)) if b > a => Some(((a.max(0.0) * SR as f32) as u64, (b * SR as f32) as u64)),
            _ => None,
        };
    }

    fn mix(&mut self, out: &mut [f32], trim: Option<(f32, u32)>) -> bool {
        if self.src.is_none() {
            return false;
        }
        // a pause in a live stream or on a microphone is not the end of the item
        let trim = if self.live { None } else { trim };
        let mut ended = false;
        for frame in out.chunks_exact_mut(2) {
            if self.end_frame.is_some_and(|e| self.frames >= e) {
                ended = true; // reached cue out
                break;
            }
            if let Some((a, b)) = self.auto_fade {
                if self.frames >= a {
                    self.auto_fade = None;
                    if !self.fading_out {
                        self.fade_to(0.0, (b.saturating_sub(self.frames) as f32 / SR as f32).max(0.01));
                    }
                }
            }
            let src = self.src.as_mut().unwrap();
            let mut waiting = false;
            let (l, r) = loop {
                let (l, r) = match src.pull() {
                    Pull::Frame(l, r) => (l, r),
                    Pull::Wait => {
                        waiting = true;
                        break (0.0, 0.0);
                    }
                    Pull::End => {
                        ended = true;
                        break (0.0, 0.0);
                    }
                };
                if let Some((thr, _)) = trim {
                    if !self.heard && l.abs().max(r.abs()) < thr && self.skipped < SKIP_MAX {
                        self.skipped += 1; // skip leading silence
                        continue;
                    }
                }
                break (l, r);
            };
            if ended || waiting {
                break;
            }
            if let Some((thr, max_run)) = trim {
                if l.abs().max(r.abs()) >= thr {
                    self.heard = true;
                    self.silent_run = 0;
                } else {
                    self.silent_run += 1;
                    if self.heard && self.silent_run >= max_run {
                        ended = true; // trailing silence reached
                        break;
                    }
                }
            }
            if self.gain != self.target {
                self.gain += self.step;
                if (self.step >= 0.0 && self.gain >= self.target) || (self.step < 0.0 && self.gain <= self.target) {
                    self.gain = self.target;
                }
            }
            let g = self.gain * self.level;
            frame[0] += l * g;
            frame[1] += r * g;
            self.frames += 1;
        }
        if self.fading_out && self.gain <= 0.0 && self.end_frame.is_some() && !ended {
            // a cue fade reached silence: treat it as the natural end of the track
            ended = true;
        }
        if ended || (self.fading_out && self.gain <= 0.0) {
            self.src = None;
        }
        ended
    }
}

struct Shared {
    decks: [Deck; 2],
    carts: Vec<Deck>,
    active: usize,
    master: f32,
    paused: bool,
    ended: bool,
    peak_l: f32,
    peak_r: f32,
    taps: Vec<Sender<Arc<Vec<i16>>>>,
    dsp: Dsp,
    trim: Option<(f32, u32)>,
    mic_on: bool,
    mic_gain: f32,
    duck: f32,
    /// music level right now (follows `duck` smoothly)
    duck_now: f32,
    mic_buf: VecDeque<f32>,
    monitors: Vec<Sender<Arc<Vec<f32>>>>,
    /// mics of apps connected to RadioMaster Server (summed with the local mic while the mic is on)
    net_mics: Vec<(u64, NetSource)>,
    next_mic: u64,
    /// a connected app: the server's on-air mix, played after the volume and processing
    feed: Option<NetSource>,
}

#[derive(Clone, Default, Debug)]
pub struct Status {
    pub has_track: bool,
    pub pos: f32,
    pub total: f32,
    pub peak_l: f32,
    pub peak_r: f32,
}

pub struct Bus {
    shared: Arc<Mutex<Shared>>,
    buf: Vec<f32>,
    pos: usize,
}

impl Bus {
    fn refill(&mut self) {
        let mut s = self.shared.lock();
        self.buf.iter_mut().for_each(|x| *x = 0.0);
        let trim = s.trim;
        if !s.paused {
            let active = s.active;
            for i in 0..2 {
                let ended = s.decks[i].mix(&mut self.buf, trim);
                if ended && i == active {
                    s.ended = true;
                }
            }
        }
        // the music level glides to the duck level (and back) instead of jumping: no clicks
        let target = if s.mic_on { s.duck } else { 1.0 };
        if s.duck_now != target || target != 1.0 {
            let step = 1.0 / (0.12 * SR as f32);
            let mut g = s.duck_now;
            for f in self.buf.chunks_exact_mut(2) {
                g = if g < target { (g + step).min(target) } else { (g - step).max(target) };
                f[0] *= g;
                f[1] *= g;
            }
            s.duck_now = g;
        }
        if !s.mic_on && !s.mic_buf.is_empty() {
            s.mic_buf.clear();
        }
        for c in s.carts.iter_mut() {
            c.mix(&mut self.buf, None);
        }
        let s = &mut *s;
        if s.mic_on {
            let g = s.mic_gain;
            for f in self.buf.chunks_exact_mut(2) {
                let (mut l, mut r) = (s.mic_buf.pop_front().unwrap_or(0.0), s.mic_buf.pop_front().unwrap_or(0.0));
                for (_, m) in s.net_mics.iter_mut() {
                    let (a, b) = m.frame();
                    l += a;
                    r += b;
                }
                f[0] += l * g;
                f[1] += r * g;
            }
        }
        s.dsp.process(&mut self.buf);
        let vol = s.master;
        let (mut pl, mut pr) = (0f32, 0f32);
        for f in self.buf.chunks_exact_mut(2) {
            let (a, b) = s.feed.as_mut().map_or((0.0, 0.0), NetSource::frame);
            f[0] = (f[0] * vol + a).clamp(-1.0, 1.0);
            f[1] = (f[1] * vol + b).clamp(-1.0, 1.0);
            pl = pl.max(f[0].abs());
            pr = pr.max(f[1].abs());
        }
        s.peak_l = pl.max(s.peak_l * 0.88);
        s.peak_r = pr.max(s.peak_r * 0.88);
        if !s.monitors.is_empty() {
            let blk = Arc::new(self.buf.clone());
            s.monitors.retain(|t| !matches!(t.try_send(blk.clone()), Err(TrySendError::Disconnected(_))));
        }
        if !s.taps.is_empty() {
            let pcm: Arc<Vec<i16>> = Arc::new(self.buf.iter().map(|x| (x * 32767.0) as i16).collect());
            s.taps.retain(|t| !matches!(t.try_send(pcm.clone()), Err(TrySendError::Disconnected(_))));
        }
    }
}

impl Iterator for Bus {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() {
            self.refill();
            self.pos = 0;
        }
        let v = self.buf[self.pos];
        self.pos += 1;
        Some(v)
    }
}

impl Source for Bus {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[derive(Clone)]
pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    /// mirrors `Shared::mic_on` without the lock, for the studio monitor gates
    mic_live: Arc<AtomicBool>,
    /// the last `set_mic` values (a lock of its own, so the audio thread is never waited on)
    mic_set: Arc<Mutex<Option<(bool, f32, f32)>>>,
}

fn open(path: &Path) -> Result<(Pcm, f32)> {
    open_seek(path, 0.0)
}

pub fn open_at(path: &Path, secs: f32) -> Result<Pcm> {
    Ok(open_seek(path, secs)?.0)
}

/// Opens a file at `secs` seconds; also gives its length (0 = unknown) without opening it twice.
fn open_seek(path: &Path, secs: f32) -> Result<(Pcm, f32)> {
    // a bigger read buffer means fewer, larger disk reads (kind to slow and network drives)
    let mut dec = Decoder::new(BufReader::with_capacity(256 * 1024, File::open(path)?))?;
    let dur = dec.total_duration().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    if secs > 0.0 {
        let _ = dec.try_seek(Duration::from_secs_f32(secs));
    }
    Ok((Box::new(UniformSourceIterator::<_, f32>::new(dec.convert_samples::<f32>(), 2, SR)), dur))
}

/// Write interleaved f32 samples as a 16-bit PCM WAV file.
pub fn write_wav(path: &Path, samples: &[f32], channels: u16, rate: u32) -> Result<()> {
    use std::io::Write;
    let bytes = (samples.len() * 2) as u32;
    let mut f = std::io::BufWriter::new(File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&36u32.saturating_add(bytes).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * channels as u32 * 2).to_le_bytes())?;
    f.write_all(&(channels * 2).to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&bytes.to_le_bytes())?;
    for x in samples {
        f.write_all(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    f.flush()?;
    Ok(())
}

/// Render a list of files back-to-back into one 16-bit stereo WAV.
pub fn render_wav(paths: &[PathBuf], out: &Path, progress: &dyn Fn(usize, usize)) -> Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(File::create(out)?);
    f.write_all(&[0u8; 44])?;
    let mut bytes: u32 = 0;
    for (k, p) in paths.iter().enumerate() {
        progress(k, paths.len());
        if let Ok((src, _)) = open(p) {
            for x in src {
                f.write_all(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
                bytes = bytes.saturating_add(2);
            }
        }
    }
    let mut file = f.into_inner().map_err(|e| anyhow!("{e}"))?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36u32.saturating_add(bytes)).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&SR.to_le_bytes());
    h.extend_from_slice(&(SR * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&bytes.to_le_bytes());
    file.write_all(&h)?;
    Ok(())
}

impl Engine {
    pub fn new_detached() -> Engine {
        let shared = Arc::new(Mutex::new(Shared {
            decks: [Deck::default(), Deck::default()],
            carts: (0..CART_SLOTS).map(|_| Deck::default()).collect(),
            active: 0,
            master: 0.9,
            paused: false,
            ended: false,
            peak_l: 0.0,
            peak_r: 0.0,
            taps: vec![],
            dsp: Dsp::new(),
            trim: None,
            mic_on: false,
            mic_gain: 1.0,
            duck: 0.35,
            duck_now: 1.0,
            mic_buf: VecDeque::new(),
            monitors: vec![],
            net_mics: vec![],
            next_mic: 0,
            feed: None,
        }));
        Engine { shared, mic_live: Arc::new(AtomicBool::new(false)), mic_set: Arc::new(Mutex::new(None)) }
    }

    /// The master bus as an audio source. Exactly one primary output (or the null driver) should pull it.
    pub fn primary_source(&self) -> Bus {
        Bus { shared: self.shared.clone(), buf: vec![0.0; BLOCK_FRAMES * 2], pos: BLOCK_FRAMES * 2 }
    }

    /// A copy of the master mix for an additional output device (monitor / headphones / second card).
    pub fn monitor_source(&self) -> MonitorSource {
        let (tx, rx) = crossbeam_channel::bounded::<Arc<Vec<f32>>>(24);
        self.shared.lock().monitors.push(tx);
        MonitorSource { rx, cur: Arc::new(Vec::new()), pos: 0 }
    }

    /// Wraps an output's source so it goes quiet while the mic is on (studio monitor speakers),
    /// which stops the mic picking the speakers up. The stream and other outputs are not affected.
    pub fn studio_gate<S: Source<Item = f32>>(&self, inner: S) -> StudioGate<S> {
        StudioGate { inner, mic: self.mic_live.clone(), g: 1.0, n: 0 }
    }

    /// Real-time clock that drives the bus when no sound device is available (streaming still works).
    pub fn start_null_driver(&self) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let mut bus = self.primary_source();
        std::thread::spawn(move || {
            let block = Duration::from_secs_f64(BLOCK_FRAMES as f64 / SR as f64);
            let mut next = std::time::Instant::now();
            while !s2.load(Ordering::Relaxed) {
                bus.refill();
                next += block;
                let now = std::time::Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        });
        stop
    }

    /// Start a track on the idle deck honouring its cue points.
    /// `old_fade`: `Some(secs)` fades the previous track out over `secs` (0 = cut it),
    /// `None` lets it finish on its own cue fade / cue out (smart fading).
    pub fn play_cued(&self, path: &Path, duration_hint: f32, spec: CueSpec, old_fade: Option<f32>) -> Result<()> {
        let (src, dur) = open_seek(path, spec.cue_in)?;
        let total = if duration_hint > 0.0 { duration_hint } else { dur };
        self.start_deck(Input::Ahead(Ahead::start(src)), Some(path.to_path_buf()), total, spec, old_fade, false);
        Ok(())
    }

    /// Start any source (internet stream, line input, teaser, time announcement…) on the idle deck.
    /// `total` = expected length in seconds (0 = unknown). With `spec.cue_out` set, it ends there.
    /// Live sources play as they come; anything else (teasers, previews) is decoded ahead.
    pub fn play_source(&self, src: Pcm, label: Option<PathBuf>, total: f32, spec: CueSpec, old_fade: Option<f32>, live: bool) {
        let input = if live { Input::Direct(src) } else { Input::Ahead(Ahead::start(src)) };
        self.start_deck(input, label, total, spec, old_fade, live);
    }

    fn start_deck(&self, src: Input, path: Option<PathBuf>, total: f32, spec: CueSpec, old_fade: Option<f32>, live: bool) {
        let mut s = self.shared.lock();
        let old = s.active;
        let new = 1 - old;
        let overlapping = s.decks[old].src.is_some() && !s.paused;
        match old_fade {
            Some(x) if x > 0.0 && overlapping => s.decks[old].fade_to(0.0, x),
            None if overlapping && s.decks[old].end_frame.is_some() => {} // finishes on its own cues
            _ => s.decks[old] = Deck::default(),
        }
        // a plain fixed crossfade only fades in when it overlaps something; cued tracks always honour their fade-in
        let fade_in = if overlapping || spec.cue_out.is_some() { spec.fade_in } else { 0.0 };
        let mut d = Deck::new(src, total, if fade_in > 0.0 { 0.0 } else { 1.0 });
        if fade_in > 0.0 {
            d.fade_to(1.0, fade_in);
            d.fading_out = false;
        }
        d.path = path;
        d.live = live;
        d.frames = (spec.cue_in.max(0.0) * SR as f32) as u64;
        if spec.cue_in > 0.0 || live {
            d.heard = true;
        }
        d.apply_cues(&spec);
        s.decks[new] = d;
        s.active = new;
        s.paused = false;
        s.ended = false;
    }

    /// File on the active deck.
    pub fn current_path(&self) -> Option<PathBuf> {
        let s = self.shared.lock();
        s.decks[s.active].path.clone()
    }

    /// Update the cue points of the track currently on air (e.g. analysis finished after it started).
    pub fn set_active_cues(&self, path: &Path, spec: CueSpec) {
        let mut s = self.shared.lock();
        let a = s.active;
        if s.decks[a].path.as_deref() == Some(path) && s.decks[a].src.is_some() {
            s.decks[a].apply_cues(&spec);
        }
    }

    pub fn stop(&self, fade_secs: f32) {
        let mut s = self.shared.lock();
        for d in s.decks.iter_mut() {
            if d.src.is_some() {
                if fade_secs > 0.0 {
                    d.fade_to(0.0, fade_secs);
                } else {
                    *d = Deck::default();
                }
            }
        }
        s.paused = false;
        s.ended = false;
    }

    pub fn set_paused(&self, p: bool) {
        self.shared.lock().paused = p;
    }

    pub fn set_volume(&self, v: f32) {
        self.shared.lock().master = v.clamp(0.0, 1.0);
    }

    pub fn trigger_cart(&self, path: &Path) -> Result<()> {
        let (src, dur) = open(path)?;
        let src = Input::Ahead(Ahead::start(src));
        let mut s = self.shared.lock();
        let slot = s.carts.iter().position(|c| c.src.is_none()).unwrap_or(0);
        s.carts[slot] = Deck::new(src, dur, 1.0);
        s.carts[slot].path = Some(path.to_path_buf());
        Ok(())
    }

    /// Fade out jingles: all of them, or only the ones playing `path`. Returns how many were playing.
    pub fn stop_carts(&self, path: Option<&Path>, fade_secs: f32) -> usize {
        let mut s = self.shared.lock();
        let mut n = 0;
        for c in s.carts.iter_mut() {
            if c.src.is_some() && !c.fading_out && path.is_none_or(|p| c.path.as_deref() == Some(p)) {
                n += 1;
                if fade_secs > 0.0 {
                    c.fade_to(0.0, fade_secs);
                } else {
                    *c = Deck::default();
                }
            }
        }
        n
    }

    /// Files of the jingles playing right now.
    pub fn carts_playing(&self) -> Vec<PathBuf> {
        self.shared.lock().carts.iter().filter(|c| c.src.is_some() && !c.fading_out).filter_map(|c| c.path.clone()).collect()
    }

    pub fn set_dsp(&self, c: &DspConfig) {
        let mut s = self.shared.lock();
        s.dsp.set_config(c);
        s.trim = if c.silence_trim {
            Some((10f32.powf(c.silence_thresh_db / 20.0), (c.silence_max_secs.max(0.1) * SR as f32) as u32))
        } else {
            None
        };
    }

    /// Called every UI frame: the mixer lock is only taken when something actually changed.
    pub fn set_mic(&self, on: bool, gain: f32, duck: f32) {
        if std::mem::replace(&mut *self.mic_set.lock(), Some((on, gain, duck))) == Some((on, gain, duck)) {
            return;
        }
        self.mic_live.store(on, Ordering::Relaxed);
        let mut s = self.shared.lock();
        s.mic_on = on;
        s.mic_gain = gain;
        s.duck = duck;
    }

    pub fn mic_push(&self, samples: &[f32]) {
        let mut s = self.shared.lock();
        if !s.mic_on {
            return;
        }
        s.mic_buf.extend(samples.iter().copied());
        while s.mic_buf.len() > SR as usize * 2 {
            s.mic_buf.pop_front();
        }
    }

    pub fn seek(&self, secs: f32) -> Result<()> {
        let (path, total) = {
            let s = self.shared.lock();
            let d = &s.decks[s.active];
            (d.path.clone(), d.total_secs)
        };
        let Some(path) = path else { return Ok(()) };
        let secs = secs.clamp(0.0, (total - 0.5).max(0.0));
        let src = Input::Ahead(Ahead::start(open_at(&path, secs)?));
        let mut s = self.shared.lock();
        let a = s.active;
        if s.decks[a].path.as_ref() != Some(&path) {
            return Ok(());
        }
        let mut d = Deck::new(src, total, 1.0);
        d.path = Some(path);
        d.frames = (secs * SR as f32) as u64;
        d.heard = true;
        d.end_frame = s.decks[a].end_frame;
        d.auto_fade = s.decks[a].auto_fade;
        d.level = s.decks[a].level;
        s.decks[a] = d;
        Ok(())
    }

    /// Play audio arriving from RadioMaster Server (the connected app hears the station this way).
    /// None stops it.
    pub fn set_feed(&self, src: Option<NetSource>) {
        self.shared.lock().feed = src;
    }

    /// A connected app's mic, heard on air while the mic is on (see `set_mic`). Returns its id.
    #[allow(dead_code)] // RadioMaster-Server
    pub fn add_net_mic(&self, src: NetSource) -> u64 {
        let mut s = self.shared.lock();
        s.next_mic += 1;
        let id = s.next_mic;
        s.net_mics.push((id, src));
        id
    }

    /// Remove a connected app's mic; returns how many are left.
    #[allow(dead_code)] // RadioMaster-Server
    pub fn remove_net_mic(&self, id: u64) -> usize {
        let mut s = self.shared.lock();
        s.net_mics.retain(|(i, _)| *i != id);
        s.net_mics.len()
    }

    pub fn add_tap(&self, tx: Sender<Arc<Vec<i16>>>) {
        self.shared.lock().taps.push(tx);
    }

    pub fn take_ended(&self) -> bool {
        std::mem::take(&mut self.shared.lock().ended)
    }

    pub fn status(&self) -> Status {
        let s = self.shared.lock();
        let d = &s.decks[s.active];
        Status { has_track: d.src.is_some(), pos: d.frames as f32 / SR as f32, total: d.total_secs, peak_l: s.peak_l, peak_r: s.peak_r }
    }
}

/// Plays blocks of the master mix produced by the primary output. Output clocks differ slightly
/// between sound cards, so old blocks are dropped when this device falls behind and silence is
/// played if it runs ahead.
pub struct MonitorSource {
    rx: crossbeam_channel::Receiver<Arc<Vec<f32>>>,
    cur: Arc<Vec<f32>>,
    pos: usize,
}

impl Iterator for MonitorSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.cur.len() {
            while self.rx.len() > 10 {
                let _ = self.rx.try_recv();
            }
            match self.rx.try_recv() {
                Ok(b) => {
                    self.cur = b;
                    self.pos = 0;
                }
                Err(_) => return Some(0.0),
            }
        }
        let v = self.cur[self.pos];
        self.pos += 1;
        Some(v)
    }
}

impl Source for MonitorSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Audio arriving over the network (interleaved stereo blocks at `SR`). It starts once `prebuffer`
/// blocks have arrived and waits for that many again after running dry, so a late packet costs one
/// short gap instead of crackling. When more than three times that is queued (the two machines'
/// sound card clocks differ slightly) the oldest blocks are dropped, which keeps the delay short.
pub struct NetSource {
    rx: crossbeam_channel::Receiver<Vec<f32>>,
    cur: Vec<f32>,
    pos: usize,
    prebuffer: usize,
    buffering: bool,
}

/// A network feed: push blocks (with `try_send`, so a slow consumer never holds the network up)
/// into the sender; the `NetSource` plays them.
pub fn net_feed(prebuffer: usize) -> (Sender<Vec<f32>>, NetSource) {
    let prebuffer = prebuffer.max(1);
    let (tx, rx) = crossbeam_channel::bounded(prebuffer * 4);
    (tx, NetSource { rx, cur: Vec::new(), pos: 0, prebuffer, buffering: true })
}

impl NetSource {
    fn frame(&mut self) -> (f32, f32) {
        if self.pos + 1 >= self.cur.len() {
            if self.buffering {
                if self.rx.len() < self.prebuffer {
                    return (0.0, 0.0);
                }
                self.buffering = false;
            }
            while self.rx.len() > self.prebuffer * 3 {
                let _ = self.rx.try_recv();
            }
            match self.rx.try_recv() {
                Ok(b) => {
                    self.cur = b;
                    self.pos = 0;
                }
                Err(_) => {
                    self.buffering = true;
                    self.cur.clear();
                    self.pos = 0;
                    return (0.0, 0.0);
                }
            }
            if self.cur.len() < 2 {
                return (0.0, 0.0);
            }
        }
        let f = (self.cur[self.pos], self.cur[self.pos + 1]);
        self.pos += 2;
        f
    }
}

/// See `Engine::studio_gate`. Fades in about 50 ms (down) / 300 ms (back up) so there is no click.
pub struct StudioGate<S> {
    inner: S,
    mic: Arc<AtomicBool>,
    g: f32,
    n: u32,
}

impl<S: Source<Item = f32>> Iterator for StudioGate<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let x = self.inner.next()?;
        // the gain changes once per stereo frame so both channels get the same level
        if self.n % 2 == 0 {
            if self.mic.load(Ordering::Relaxed) {
                self.g = (self.g - 1.0 / (0.05 * SR as f32)).max(0.0);
            } else if self.g < 1.0 {
                self.g = (self.g + 1.0 / (0.3 * SR as f32)).min(1.0);
            }
        }
        self.n = self.n.wrapping_add(1);
        Some(x * self.g)
    }
}

impl<S: Source<Item = f32>> Source for StudioGate<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

/// Per-output volume (shared atomic so the UI can change it live).
pub struct GainSource<S> {
    inner: S,
    gain: Arc<AtomicU32>,
}

impl<S> GainSource<S> {
    pub fn new(inner: S, gain: Arc<AtomicU32>) -> Self {
        GainSource { inner, gain }
    }
}

impl<S: Source<Item = f32>> Iterator for GainSource<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let g = f32::from_bits(self.gain.load(Ordering::Relaxed));
        self.inner.next().map(|x| x * g)
    }
}

impl<S: Source<Item = f32>> Source for GainSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sine(path: &std::path::Path) {
        let mut data = Vec::new();
        for k in 0..44100u32 {
            let v = ((k as f32 * 440.0 * std::f32::consts::TAU / 44100.0).sin() * 12000.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&44100u32.to_le_bytes());
        h.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        h.extend_from_slice(&4u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&data);
        std::fs::write(path, h).unwrap();
    }

    #[test]
    fn speech_with_pauses_plays_to_the_end_even_with_silence_trimming() {
        if std::process::Command::new("espeak-ng").arg("--version").output().is_err() {
            return; // no speech engine here
        }
        let v = crate::live::Voice { line_pause_ms: 1500, ..Default::default() };
        let text = "First line here.\nSecond line here.\nThird line here.";
        let file = crate::live::speech_file(text, &v).unwrap();
        let want = open_at(&file, 0.0).unwrap().count() / 2;
        let engine = Engine::new_detached();
        engine.set_dsp(&crate::dsp::DspConfig { silence_trim: true, silence_max_secs: 0.4, ..Default::default() });
        let mut bus = engine.primary_source();
        let opts = crate::items::SpeechOpts { line_pause_ms: 1500, ..Default::default() };
        let src = crate::live::speech_source(text.into(), v, opts, Default::default());
        engine.play_source(Box::new(src), None, 0.0, CueSpec::default(), Some(0.0), true);
        let mut frames = 0usize;
        while !engine.take_ended() && frames < SR as usize * 60 {
            bus.next();
            bus.next();
            frames += 1;
            if frames % 4096 == 0 {
                std::thread::sleep(std::time::Duration::from_micros(200)); // let the reader keep up
            }
        }
        assert!(frames >= want, "played {frames} frames of {want}: the pauses between lines must not end it");
    }

    #[test]
    fn cued_playback_honours_in_out_and_fade() {
        // 1 s tone; play from 0.25 s, fade out from 0.5 s, end at 0.75 s
        let p = std::env::temp_dir().join("openboss_test_cue.wav");
        write_sine(&p);
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let mut bus = engine.primary_source();
        let spec = CueSpec { cue_in: 0.25, cue_out: Some(0.75), fade_in: 0.0, fade_out_at: Some(0.5), gain_db: 0.0 };
        engine.play_cued(&p, 1.0, spec, Some(0.0)).unwrap();
        let frames = (SR as f32 * 0.6) as usize;
        // pulled at a few times real time, like a sound card (files are decoded ahead on another thread)
        let mut out: Vec<f32> = Vec::new();
        while out.len() < frames * 2 {
            out.extend((0..2048.min(frames * 2 - out.len())).map(|_| bus.next().unwrap()));
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        let peak = |a: f32, b: f32| {
            let (i, j) = ((a * SR as f32) as usize * 2, (b * SR as f32) as usize * 2);
            out[i..j].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!(engine.status().pos >= 0.25, "position starts at cue in");
        assert!(peak(0.0, 0.2) > 0.3, "full level before the fade");
        assert!(peak(0.40, 0.45) < peak(0.0, 0.2) * 0.5, "fading after fade-out point (track time 0.65-0.70 s)");
        assert!(peak(0.52, 0.6) < 1e-4, "silent after cue out");
        assert!(engine.take_ended(), "cue out reports the end of the track");
    }

    #[test]
    fn a_slow_disk_never_holds_up_the_mix() {
        // a "file" whose reads stall for 200 ms per block after the first two
        struct Slow(usize);
        impl Iterator for Slow {
            type Item = f32;
            fn next(&mut self) -> Option<f32> {
                if self.0 >= AHEAD_BLOCK * 6 {
                    return None;
                }
                if self.0 >= AHEAD_BLOCK * 2 && self.0 % AHEAD_BLOCK == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                self.0 += 1;
                Some(0.5)
            }
        }
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let mut bus = engine.primary_source();
        engine.play_source(Box::new(Slow(0)), None, 0.0, CueSpec::default(), Some(0.0), false);
        let t = std::time::Instant::now();
        let first: Vec<f32> = (0..AHEAD_BLOCK * 2).map(|_| bus.next().unwrap()).collect();
        assert!(first.iter().all(|x| (x - 0.5).abs() < 1e-6), "the first blocks are ready as soon as the track starts");
        // the decoder is stuck on the disk now: the mix carries on (silence) instead of waiting for it
        let stalled: Vec<f32> = (0..BLOCK_FRAMES * 2).map(|_| bus.next().unwrap()).collect();
        assert!(t.elapsed() < std::time::Duration::from_millis(100), "the mix waited for the disk");
        assert!(stalled.iter().all(|&x| x == 0.0));
        assert!(!engine.take_ended(), "a stall is not the end of the track");
        // it catches up and the whole track is played, then reported as ended
        let mut heard = first.len();
        for _ in 0..2000 {
            heard += (0..BLOCK_FRAMES * 2).map(|_| bus.next().unwrap()).filter(|x| (x - 0.5).abs() < 1e-6).count();
            if engine.take_ended() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(heard, AHEAD_BLOCK * 6, "every sample is played once");
        assert!(!engine.status().has_track);
    }

    #[test]
    fn second_output_receives_identical_mix() {
        let p = std::env::temp_dir().join("openboss_test_sine.wav");
        write_sine(&p);
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let mut primary = engine.primary_source();
        let mut monitor = engine.monitor_source();
        engine.play_cued(&p, 1.0, CueSpec::default(), Some(0.0)).unwrap();
        let a: Vec<f32> = (0..4096).map(|_| primary.next().unwrap()).collect();
        let b: Vec<f32> = (0..4096).map(|_| monitor.next().unwrap()).collect();
        assert!(a.iter().any(|x| x.abs() > 0.1), "primary should carry audio");
        assert_eq!(a, b, "monitor output must be an exact copy of the primary mix");
    }

    #[test]
    fn studio_monitor_goes_quiet_with_the_mic_but_the_stream_does_not() {
        let p = std::env::temp_dir().join("openboss_test_studio.wav");
        write_sine(&p);
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let mut primary = engine.primary_source();
        let mut studio = engine.studio_gate(engine.monitor_source());
        let (tx, rx) = crossbeam_channel::unbounded();
        engine.add_tap(tx);
        engine.play_cued(&p, 1.0, CueSpec::default(), Some(0.0)).unwrap();
        engine.set_mic(true, 1.0, 1.0); // mic on, music not ducked
                                        // pull the primary and the studio monitor side by side, like two sound cards
        let mut run = |secs: f32| {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for _ in 0..(SR as f32 * secs) as usize / 256 {
                a.extend((0..512).map(|_| primary.next().unwrap()));
                b.extend((0..512).map(|_| studio.next().unwrap()));
                // at a few times real time, like a sound card (files are decoded ahead on another thread)
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            (a, b)
        };
        let peak = |v: &[f32]| v.iter().fold(0f32, |m, x| m.max(x.abs()));
        let (a, b) = run(0.2);
        let h = a.len() / 2;
        assert!(peak(&a[h..]) > 0.3, "the on-air mix keeps playing");
        assert!(peak(&b[h..]) < 1e-6, "the studio monitor is silent while the mic is on");
        let streamed: Vec<i16> = rx.try_iter().flat_map(|b| b.as_ref().clone()).collect();
        assert!(streamed.iter().any(|x| x.unsigned_abs() > 8000), "the stream still gets the music");
        engine.set_mic(false, 1.0, 1.0);
        let (_, c) = run(0.5);
        assert!(peak(&c[c.len() * 3 / 4..]) > 0.3, "the studio monitor comes back when the mic goes off");
    }
}
