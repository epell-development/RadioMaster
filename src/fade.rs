//! Smart fading: waveform analysis and cue points.
//!
//! Every track can have five cue points (seconds from the start of the file):
//!   cue_in   – playback starts here (skips silence / count-ins)
//!   fade_in  – length of the fade-in that starts at cue_in (0 = start at full volume)
//!   fade_out – the volume starts falling here...
//!   cue_out  – ...and reaches silence / playback ends here
//!   mix      – the NEXT track starts here (may be before fade_out, inside the fade, or at cue_out)
//!
//! Cues come either from the fade editor (manual) or from `smart_cues`, which reads the
//! loudness envelope: it finds where the music really starts, where the song's body ends and
//! the outro begins to die away, and starts the next song there. Hard (cold) endings get only a
//! tiny overlap so they are not chopped; long fading outros get a longer overlap.

use anyhow::Result;
use rodio::{source::UniformSourceIterator, Decoder, Source};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::BufReader,
    path::Path,
    time::{Duration, Instant},
};

/// Passes samples through, pausing after every `every` samples for as long as producing them took.
struct Gentle<I> {
    inner: I,
    n: usize,
    every: usize,
    t: Instant,
}

impl<I: Iterator<Item = f32>> Iterator for Gentle<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.n += 1;
        if self.n >= self.every {
            self.n = 0;
            std::thread::sleep(self.t.elapsed().min(Duration::from_millis(500)));
            self.t = Instant::now();
        }
        self.inner.next()
    }
}

/// Seconds per analysis bin.
pub const BIN: f32 = 0.02;

/// Peak and RMS level per 20 ms bin (mono, linear 0..1).
#[derive(Clone, Debug, Default)]
pub struct Waveform {
    pub peaks: Vec<f32>,
    pub rms: Vec<f32>,
}

impl Waveform {
    pub fn duration(&self) -> f32 {
        self.peaks.len() as f32 * BIN
    }

    /// Decode a file and build its waveform. Runs on a background thread, at a gentle pace: it
    /// rests as long as it works, so on a slow PC playback and the window keep a core to themselves.
    pub fn analyze(path: &Path) -> Result<Waveform> {
        let dec = Decoder::new(BufReader::with_capacity(256 * 1024, File::open(path)?))?;
        let rate = 22_050u32; // half rate is plenty for envelopes and twice as fast
        let src = UniformSourceIterator::<_, f32>::new(dec.convert_samples::<f32>(), 2, rate);
        let gentle = Gentle { inner: src, n: 0, every: rate as usize * 2 * 5, t: Instant::now() };
        Ok(Self::from_interleaved(gentle, rate))
    }

    pub fn from_interleaved<I: Iterator<Item = f32>>(mut src: I, rate: u32) -> Waveform {
        let per_bin = ((rate as f32 * BIN) as usize).max(1);
        let mut w = Waveform::default();
        let (mut pk, mut sq, mut n) = (0f32, 0f64, 0usize);
        loop {
            let (Some(l), Some(r)) = (src.next(), src.next()) else { break };
            let m = (l + r) * 0.5;
            pk = pk.max(l.abs().max(r.abs()));
            sq += (m as f64) * (m as f64);
            n += 1;
            if n == per_bin {
                w.peaks.push(pk);
                w.rms.push((sq / n as f64).sqrt() as f32);
                pk = 0.0;
                sq = 0.0;
                n = 0;
            }
        }
        if n > per_bin / 4 {
            w.peaks.push(pk);
            w.rms.push((sq / n as f64).sqrt() as f32);
        }
        w
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Cues {
    pub cue_in: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub mix: f32,
    pub cue_out: f32,
    /// true = set by hand in the fade editor (always honoured, never recomputed)
    pub manual: bool,
    /// end of the intro (talk-over time); 0 = not set
    pub intro: f32,
    /// start of the outro; 0 = not set
    pub outro: f32,
    /// playback level adjustment in dB
    pub gain_db: f32,
    /// beats per minute; 0 = unknown
    pub bpm: f32,
}

impl Cues {
    pub fn plain(duration: f32) -> Cues {
        Cues { cue_in: 0.0, fade_in: 0.0, fade_out: duration, mix: duration, cue_out: duration, ..Default::default() }
    }

    /// Keep the points in a sensible order after an edit.
    pub fn clamp(&mut self, duration: f32) {
        let d = duration.max(0.1);
        self.cue_out = self.cue_out.clamp(0.1, d);
        self.cue_in = self.cue_in.clamp(0.0, (self.cue_out - 0.1).max(0.0));
        self.fade_in = self.fade_in.clamp(0.0, self.cue_out - self.cue_in);
        self.fade_out = self.fade_out.clamp(self.cue_in + self.fade_in, self.cue_out);
        self.mix = self.mix.clamp(self.cue_in, self.cue_out);
        self.intro = self.intro.clamp(0.0, d);
        self.outro = self.outro.clamp(0.0, d);
        self.gain_db = self.gain_db.clamp(-24.0, 24.0);
    }

    /// Seconds from this track's start until the next one starts.
    pub fn span(&self) -> f32 {
        (self.mix - self.cue_in).max(0.0)
    }

    /// Volume (0..1) of this track at time `t` according to its fades.
    pub fn gain_at(&self, t: f32) -> f32 {
        if t < self.cue_in || t > self.cue_out {
            return 0.0;
        }
        let mut g = 1.0;
        if self.fade_in > 0.0 && t < self.cue_in + self.fade_in {
            g *= (t - self.cue_in) / self.fade_in;
        }
        if t > self.fade_out && self.cue_out > self.fade_out {
            g *= 1.0 - (t - self.fade_out) / (self.cue_out - self.fade_out);
        }
        g.clamp(0.0, 1.0)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FadeMode {
    /// Fixed-length crossfade (the "Fade" slider) for every track.
    Fixed,
    /// Analyse every track and pick the mix point automatically.
    #[default]
    Smart,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct SmartFadeConfig {
    pub mode: FadeMode,
    /// Anything quieter than this is treated as silence when finding cue in/out (dBFS).
    pub silence_db: f32,
    /// The outro "has ended" when it drops this many dB below the song's normal level.
    pub drop_db: f32,
    /// Never overlap two songs by more than this.
    pub max_overlap: f32,
    /// Overlap used for cold (hard) endings.
    pub hard_overlap: f32,
    /// Fade a hard ending out over this long so it does not click.
    pub hard_fade: f32,
}

impl Default for SmartFadeConfig {
    fn default() -> Self {
        Self { mode: FadeMode::Smart, silence_db: -45.0, drop_db: 12.0, max_overlap: 8.0, hard_overlap: 0.15, hard_fade: 0.3 }
    }
}

fn db(x: f32) -> f32 {
    20.0 * (x.max(1e-6)).log10()
}

/// Work out cue points from a waveform.
pub fn smart_cues(w: &Waveform, c: &SmartFadeConfig) -> Cues {
    let n = w.peaks.len();
    let dur = w.duration();
    if n < 10 {
        return Cues::plain(dur);
    }
    let t = |i: usize| i as f32 * BIN;

    // 1. cue in / cue out: first and last bins above the silence floor
    let first = w.peaks.iter().position(|&p| db(p) > c.silence_db).unwrap_or(0);
    let last = w.peaks.iter().rposition(|&p| db(p) > c.silence_db).unwrap_or(n - 1);
    if last <= first + 10 {
        return Cues::plain(dur);
    }
    let cue_in = (t(first) - 0.02).max(0.0);
    let cue_out = (t(last + 1) + 0.05).min(dur);

    // 2. smoothed loudness (400 ms window) and the song's typical level (median of the body)
    let win = 20usize;
    let mut sm = vec![0f32; n];
    let mut acc = 0f64;
    for i in 0..n {
        acc += (w.rms[i] as f64).powi(2);
        if i >= win {
            acc -= (w.rms[i - win] as f64).powi(2);
        }
        sm[i] = db((acc.max(0.0) / win.min(i + 1) as f64).sqrt() as f32);
    }
    let lo = first + (last - first) / 10;
    let hi = last - (last - first) / 10;
    let mut body: Vec<f32> = sm[lo..hi.max(lo + 1)].iter().copied().filter(|&v| v > c.silence_db).collect();
    if body.is_empty() {
        return Cues { cue_in, fade_in: 0.0, fade_out: cue_out, mix: cue_out, cue_out, ..Default::default() };
    }
    body.sort_by(|a, b| a.total_cmp(b));
    let typical = body[body.len() / 2];

    // 3. walk back from the end to the last moment the song was still at full strength
    let thresh = typical - c.drop_db;
    let body_end = (first..=last).rev().find(|&i| sm[i] >= thresh).unwrap_or(last);
    // `sm` lags by the window, so step forward by half of it
    let loud_until = (t(body_end) + win as f32 * BIN * 0.5).min(cue_out);
    let tail = cue_out - loud_until;

    if tail < 0.6 {
        // cold ending: let it ring out, then bring the next song in almost immediately
        let mix = (cue_out - c.hard_overlap).max(cue_in);
        let fade_out = (cue_out - c.hard_fade).max(cue_in);
        Cues { cue_in, fade_in: 0.0, fade_out, mix, cue_out, ..Default::default() }
    } else {
        // fading / decaying outro: start the next song once the outro has died away,
        // and fade out whatever is left under it
        let overlap = tail.min(c.max_overlap);
        let mix = (cue_out - overlap).max(loud_until).min(cue_out);
        Cues { cue_in, fade_in: 0.0, fade_out: mix, mix, cue_out, ..Default::default() }
    }
}

/// Rough tempo estimate from the loudness envelope (autocorrelation of onsets, 60–200 BPM).
pub fn detect_bpm(w: &Waveform) -> Option<f32> {
    let n = w.rms.len();
    if n < 500 {
        return None;
    }
    // onset strength: positive change in log energy
    let e: Vec<f32> = w.rms.iter().map(|&r| (r * r + 1e-9).ln()).collect();
    let mut on: Vec<f32> = (1..n).map(|i| (e[i] - e[i - 1]).max(0.0)).collect();
    let mean = on.iter().sum::<f32>() / on.len() as f32;
    on.iter_mut().for_each(|x| *x -= mean);
    let fps = 1.0 / BIN; // 50 frames/s
    let (lo, hi) = ((fps * 60.0 / 200.0) as usize, (fps * 60.0 / 60.0) as usize);
    let ac = |lag: usize| -> f32 { on.iter().zip(on.iter().skip(lag)).map(|(a, b)| a * b).sum() };
    let scores: Vec<f32> = (lo..=hi + 1).map(ac).collect();
    let (bi, _) = scores[..scores.len() - 1].iter().enumerate().skip(1).max_by(|a, b| a.1.total_cmp(b.1))?;
    // parabolic interpolation for sub-frame accuracy
    let (y0, y1, y2) = (scores[bi - 1], scores[bi], scores[bi + 1]);
    let den = y0 - 2.0 * y1 + y2;
    let off = if den.abs() > 1e-9 { 0.5 * (y0 - y2) / den } else { 0.0 };
    let lag = (lo + bi) as f32 + off.clamp(-0.5, 0.5);
    let mut bpm = 60.0 * fps / lag;
    while bpm < 70.0 {
        bpm *= 2.0;
    }
    while bpm > 180.0 {
        bpm /= 2.0;
    }
    Some((bpm * 10.0).round() / 10.0)
}

/// Gain (dB) that brings the loud part of the track to `target_db` RMS.
pub fn detect_level(w: &Waveform, target_db: f32) -> f32 {
    let mut v: Vec<f32> = w.rms.iter().copied().filter(|&r| db(r) > -50.0).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let loud = v[v.len() * 3 / 4]; // 75th percentile: the body of the song, not the quiet bits
    (target_db - db(loud)).clamp(-12.0, 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic track: `lead` s silence, `body` s of loud tone, then an outro shaped by `outro(t)`.
    fn synth(lead: f32, body: f32, outro_secs: f32, outro: impl Fn(f32) -> f32, trail: f32) -> Waveform {
        let rate = 8000u32;
        let total = lead + body + outro_secs + trail;
        let n = (total * rate as f32) as usize;
        let samples = (0..n).flat_map(move |k| {
            let s = k as f32 / rate as f32;
            let amp = if s < lead {
                0.0
            } else if s < lead + body {
                0.5
            } else if s < lead + body + outro_secs {
                0.5 * outro(s - lead - body)
            } else {
                0.0
            };
            let v = amp * (s * 440.0 * std::f32::consts::TAU).sin();
            [v, v]
        });
        Waveform::from_interleaved(samples, rate)
    }

    #[test]
    fn finds_cue_in_after_leading_silence() {
        let w = synth(1.5, 20.0, 0.0, |_| 0.0, 1.0);
        let c = smart_cues(&w, &SmartFadeConfig::default());
        assert!((c.cue_in - 1.5).abs() < 0.1, "cue_in {}", c.cue_in);
        assert!((c.cue_out - 21.5).abs() < 0.2, "cue_out {}", c.cue_out);
    }

    #[test]
    fn cold_ending_gets_tiny_overlap() {
        let w = synth(0.0, 30.0, 0.0, |_| 0.0, 2.0);
        let c = smart_cues(&w, &SmartFadeConfig::default());
        assert!(c.cue_out - c.mix < 0.5, "overlap {} on a hard ending", c.cue_out - c.mix);
    }

    #[test]
    fn fading_outro_starts_next_track_early() {
        // 6 s linear fade-out after the body
        let w = synth(0.0, 30.0, 6.0, |t| 1.0 - t / 6.0, 1.0);
        let c = smart_cues(&w, &SmartFadeConfig::default());
        let overlap = c.cue_out - c.mix;
        assert!(overlap > 1.0 && overlap <= 8.0, "overlap {overlap}");
        assert!(c.mix > 30.0, "should not cut into the body, mix at {}", c.mix);
    }

    #[test]
    fn overlap_is_capped() {
        let w = synth(0.0, 20.0, 20.0, |t| (1.0 - t / 20.0).powi(2), 0.0);
        let cfg = SmartFadeConfig { max_overlap: 5.0, ..Default::default() };
        let c = smart_cues(&w, &cfg);
        assert!(c.cue_out - c.mix <= 5.01);
    }

    #[test]
    fn bpm_of_a_click_track() {
        // 120 BPM: a short loud click every 0.5 s
        let rate = 8000u32;
        let n = 30 * rate as usize;
        let s = (0..n).flat_map(|k| {
            let ph = k % (rate as usize / 2);
            let v = if ph < 400 { 0.8 * ((k as f32) * 0.9).sin() } else { 0.0 };
            [v, v]
        });
        let w = Waveform::from_interleaved(s, rate);
        let b = detect_bpm(&w).unwrap();
        assert!((b - 120.0).abs() < 2.5, "bpm {b}");
    }

    #[test]
    fn level_detection_matches_target() {
        let w = synth(0.0, 20.0, 0.0, |_| 0.0, 0.0); // tone at 0.5 amplitude ≈ -9 dB RMS
        let g = detect_level(&w, -16.0);
        assert!(g < -5.0 && g > -9.0, "gain {g}");
    }

    #[test]
    fn gain_envelope() {
        let c = Cues { cue_in: 1.0, fade_in: 2.0, fade_out: 8.0, mix: 8.0, cue_out: 10.0, manual: true, ..Default::default() };
        assert_eq!(c.gain_at(0.5), 0.0);
        assert!((c.gain_at(2.0) - 0.5).abs() < 1e-5);
        assert_eq!(c.gain_at(5.0), 1.0);
        assert!((c.gain_at(9.0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn clamp_orders_points() {
        let mut c = Cues { cue_in: 9.0, fade_in: 5.0, fade_out: 2.0, mix: 50.0, cue_out: 40.0, manual: true, ..Default::default() };
        c.clamp(30.0);
        assert!(c.cue_in <= c.cue_in + c.fade_in && c.cue_in + c.fade_in <= c.fade_out && c.fade_out <= c.cue_out);
        assert!(c.mix <= c.cue_out && c.cue_out <= 30.0);
    }
}
