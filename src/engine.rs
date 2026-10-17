//! Audio engine: a single master bus (what listeners hear) fed by two crossfading
//! decks plus cart-wall slots. The same mixed PCM is tapped for the stream encoders,
//! so what you hear is exactly what goes on air.

use crate::dsp::{Dsp, DspConfig};
use anyhow::{anyhow, Result};
use crossbeam_channel::{Sender, TrySendError};
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

#[derive(Default)]
struct Deck {
    src: Option<Pcm>,
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
    fn new(src: Pcm, total_secs: f32, gain: f32) -> Self {
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
            let (l, r) = loop {
                let (Some(l), Some(r)) = (src.next(), src.next()) else {
                    ended = true;
                    break (0.0, 0.0);
                };
                if let Some((thr, _)) = trim {
                    if !self.heard && l.abs().max(r.abs()) < thr && self.skipped < SKIP_MAX {
                        self.skipped += 1; // skip leading silence
                        continue;
                    }
                }
                break (l, r);
            };
            if ended {
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
    mic_buf: VecDeque<f32>,
    monitors: Vec<Sender<Arc<Vec<f32>>>>,
}

#[derive(Clone, Default, Debug)]
pub struct Status {
    pub has_track: bool,
    pub paused: bool,
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
        if s.mic_on {
            let d = s.duck;
            self.buf.iter_mut().for_each(|x| *x *= d);
        } else if !s.mic_buf.is_empty() {
            s.mic_buf.clear();
        }
        for c in s.carts.iter_mut() {
            c.mix(&mut self.buf, None);
        }
        if s.mic_on {
            let g = s.mic_gain;
            for x in self.buf.iter_mut() {
                *x += s.mic_buf.pop_front().unwrap_or(0.0) * g;
            }
        }
        s.dsp.process(&mut self.buf);
        let vol = s.master;
        let (mut pl, mut pr) = (0f32, 0f32);
        for f in self.buf.chunks_exact_mut(2) {
            f[0] = (f[0] * vol).clamp(-1.0, 1.0);
            f[1] = (f[1] * vol).clamp(-1.0, 1.0);
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
}

fn open(path: &Path) -> Result<(Pcm, f32)> {
    let dec = Decoder::new(BufReader::new(File::open(path)?))?;
    let dur = dec.total_duration().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let src = UniformSourceIterator::<_, f32>::new(dec.convert_samples::<f32>(), 2, SR);
    Ok((Box::new(src), dur))
}

pub fn open_at(path: &Path, secs: f32) -> Result<Pcm> {
    let mut dec = Decoder::new(BufReader::new(File::open(path)?))?;
    if secs > 0.0 {
        let _ = dec.try_seek(Duration::from_secs_f32(secs));
    }
    Ok(Box::new(UniformSourceIterator::<_, f32>::new(dec.convert_samples::<f32>(), 2, SR)))
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
            mic_buf: VecDeque::new(),
            monitors: vec![],
        }));
        Engine { shared }
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

    /// Start a track on the idle deck. If `xfade` > 0 the previous track fades out while
    /// the new one fades in.
    pub fn play(&self, path: &Path, duration_hint: f32, xfade: f32) -> Result<()> {
        self.play_cued(path, duration_hint, CueSpec { fade_in: xfade, ..Default::default() }, Some(xfade))
    }

    /// Start a track on the idle deck honouring its cue points.
    /// `old_fade`: `Some(secs)` fades the previous track out over `secs` (0 = cut it),
    /// `None` lets it finish on its own cue fade / cue out (smart fading).
    pub fn play_cued(&self, path: &Path, duration_hint: f32, spec: CueSpec, old_fade: Option<f32>) -> Result<()> {
        let src = open_at(path, spec.cue_in)?;
        let total = if duration_hint > 0.0 {
            duration_hint
        } else {
            Decoder::new(BufReader::new(File::open(path)?))?.total_duration().map(|d| d.as_secs_f32()).unwrap_or(0.0)
        };
        self.start_deck(src, Some(path.to_path_buf()), total, spec, old_fade, false);
        Ok(())
    }

    /// Start any source (internet stream, line input, teaser, time announcement…) on the idle deck.
    /// `total` = expected length in seconds (0 = unknown). With `spec.cue_out` set, it ends there.
    pub fn play_source(&self, src: Pcm, label: Option<PathBuf>, total: f32, spec: CueSpec, old_fade: Option<f32>, live: bool) {
        self.start_deck(src, label, total, spec, old_fade, live);
    }

    fn start_deck(&self, src: Pcm, path: Option<PathBuf>, total: f32, spec: CueSpec, old_fade: Option<f32>, live: bool) {
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
        let mut s = self.shared.lock();
        let slot = s.carts.iter().position(|c| c.src.is_none()).unwrap_or(0);
        s.carts[slot] = Deck::new(src, dur, 1.0);
        Ok(())
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

    pub fn set_mic(&self, on: bool, gain: f32, duck: f32) {
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
        let src = open_at(&path, secs)?;
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

    pub fn add_tap(&self, tx: Sender<Arc<Vec<i16>>>) {
        self.shared.lock().taps.push(tx);
    }

    pub fn take_ended(&self) -> bool {
        std::mem::take(&mut self.shared.lock().ended)
    }

    pub fn status(&self) -> Status {
        let s = self.shared.lock();
        let d = &s.decks[s.active];
        Status {
            has_track: d.src.is_some(),
            paused: s.paused,
            pos: d.frames as f32 / SR as f32,
            total: d.total_secs,
            peak_l: s.peak_l,
            peak_r: s.peak_r,
        }
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
        let out: Vec<f32> = (0..frames * 2).map(|_| bus.next().unwrap()).collect();
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
    fn second_output_receives_identical_mix() {
        let p = std::env::temp_dir().join("openboss_test_sine.wav");
        write_sine(&p);
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let mut primary = engine.primary_source();
        let mut monitor = engine.monitor_source();
        engine.play(&p, 1.0, 0.0).unwrap();
        let a: Vec<f32> = (0..4096).map(|_| primary.next().unwrap()).collect();
        let b: Vec<f32> = (0..4096).map(|_| monitor.next().unwrap()).collect();
        assert!(a.iter().any(|x| x.abs() > 0.1), "primary should carry audio");
        assert_eq!(a, b, "monitor output must be an exact copy of the primary mix");
    }
}
