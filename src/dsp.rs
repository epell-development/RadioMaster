//! Master-bus DSP: equalizer (12-band graphic, or parametric with up to 16 free bands),
//! compressor, slow auto volume leveling (AGC).

use crate::engine::SR;
use serde::{Deserialize, Serialize};

pub const EQ_FREQS: [f32; 12] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 3000.0, 4000.0, 8000.0, 12000.0, 16000.0];

/// The most bands the parametric equalizer takes.
pub const PEQ_MAX_BANDS: usize = 16;

/// Which equalizer the master bus uses. Both keep their settings while the other is in use.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum EqMode {
    /// the classic 12 sliders at fixed frequencies
    #[default]
    Graphic,
    /// free bands: each picks its filter, frequency, gain and width
    Parametric,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum FilterKind {
    Peak,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
}

impl FilterKind {
    pub const ALL: [FilterKind; 6] = [Self::Peak, Self::LowShelf, Self::HighShelf, Self::LowCut, Self::HighCut, Self::Notch];
    pub fn label(self) -> &'static str {
        match self {
            Self::Peak => "Bell",
            Self::LowShelf => "Low shelf",
            Self::HighShelf => "High shelf",
            Self::LowCut => "Low cut",
            Self::HighCut => "High cut",
            Self::Notch => "Notch",
        }
    }
    /// Whether the band's gain does anything (cuts and notches only use frequency and width).
    pub fn has_gain(self) -> bool {
        matches!(self, Self::Peak | Self::LowShelf | Self::HighShelf)
    }
}

/// One band of the parametric equalizer.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct PeqBand {
    pub on: bool,
    pub kind: FilterKind,
    pub freq: f32,
    pub gain_db: f32,
    /// width: higher is narrower (0.71 is a gentle, standard slope for cuts and shelves)
    pub q: f32,
}

impl Default for PeqBand {
    fn default() -> Self {
        Self { on: true, kind: FilterKind::Peak, freq: 1000.0, gain_db: 0.0, q: 1.0 }
    }
}

impl PeqBand {
    pub const fn new(kind: FilterKind, freq: f32, gain_db: f32, q: f32) -> Self {
        Self { on: true, kind, freq, gain_db, q }
    }
    /// Whether processing this band changes the sound at all.
    fn active(&self) -> bool {
        self.on && (!self.kind.has_gain() || self.gain_db.abs() > 0.01)
    }
}

/// The bands a fresh parametric equalizer starts with: flat, spread over the range.
pub fn default_peq_bands() -> Vec<PeqBand> {
    use FilterKind::*;
    vec![
        PeqBand::new(LowShelf, 100.0, 0.0, 0.71),
        PeqBand::new(Peak, 300.0, 0.0, 1.0),
        PeqBand::new(Peak, 1000.0, 0.0, 1.0),
        PeqBand::new(Peak, 3000.0, 0.0, 1.0),
        PeqBand::new(HighShelf, 8000.0, 0.0, 0.71),
    ]
}

/// Built-in parametric curves.
pub fn peq_presets() -> Vec<(&'static str, Vec<PeqBand>)> {
    use FilterKind::*;
    let b = PeqBand::new;
    vec![
        ("Flat", default_peq_bands()),
        ("Rumble filter", vec![b(LowCut, 40.0, 0.0, 0.71)]),
        (
            "Broadcast punch",
            vec![
                b(LowCut, 30.0, 0.0, 0.71),
                b(LowShelf, 90.0, 3.0, 0.71),
                b(Peak, 350.0, -2.0, 1.2),
                b(Peak, 3500.0, 2.0, 1.0),
                b(HighShelf, 10000.0, 2.5, 0.71),
            ],
        ),
        (
            "Warm FM",
            vec![
                b(LowShelf, 120.0, 2.5, 0.71),
                b(Peak, 2500.0, -1.0, 0.8),
                b(HighShelf, 12000.0, 1.5, 0.71),
                b(HighCut, 15000.0, 0.0, 0.71),
            ],
        ),
        (
            "Voice presence",
            vec![b(LowCut, 80.0, 0.0, 0.71), b(Peak, 250.0, -3.0, 1.4), b(Peak, 3000.0, 3.0, 1.2), b(Peak, 7000.0, -2.0, 3.0)],
        ),
        ("De-mud", vec![b(Peak, 250.0, -4.0, 1.0), b(Peak, 500.0, -2.0, 1.4)]),
        ("Soften sibilance", vec![b(Peak, 6500.0, -4.0, 3.0)]),
        ("Air", vec![b(HighShelf, 12000.0, 4.0, 0.71)]),
        ("Sub bass", vec![b(LowCut, 25.0, 0.0, 0.71), b(Peak, 55.0, 5.0, 1.2), b(Peak, 200.0, -1.5, 1.0)]),
        ("AM radio", vec![b(LowCut, 150.0, 0.0, 0.71), b(Peak, 1500.0, 3.0, 0.7), b(HighCut, 5000.0, 0.0, 0.71)]),
        ("Telephone", vec![b(LowCut, 400.0, 0.0, 1.0), b(Peak, 1800.0, 4.0, 1.0), b(HighCut, 3400.0, 0.0, 1.0)]),
        ("Mains hum (50 Hz)", vec![b(Notch, 50.0, 0.0, 8.0), b(Notch, 100.0, 0.0, 8.0), b(Notch, 150.0, 0.0, 8.0)]),
        ("Mains hum (60 Hz)", vec![b(Notch, 60.0, 0.0, 8.0), b(Notch, 120.0, 0.0, 8.0), b(Notch, 180.0, 0.0, 8.0)]),
    ]
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct DspConfig {
    pub eq_enabled: bool,
    pub eq_mode: EqMode,
    pub eq_gains: [f32; 12],
    pub peq_bands: Vec<PeqBand>,
    /// level change before the equalizer, so boosts have room (dB)
    pub eq_preamp_db: f32,
    pub comp_enabled: bool,
    pub comp_threshold_db: f32,
    pub comp_ratio: f32,
    pub comp_attack_ms: f32,
    pub comp_release_ms: f32,
    pub comp_makeup_db: f32,
    pub agc_enabled: bool,
    pub agc_target_db: f32,
    pub silence_trim: bool,
    pub silence_thresh_db: f32,
    pub silence_max_secs: f32,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            eq_enabled: false,
            eq_mode: EqMode::Graphic,
            eq_gains: [0.0; 12],
            peq_bands: default_peq_bands(),
            eq_preamp_db: 0.0,
            comp_enabled: false,
            comp_threshold_db: -18.0,
            comp_ratio: 3.0,
            comp_attack_ms: 10.0,
            comp_release_ms: 250.0,
            comp_makeup_db: 3.0,
            agc_enabled: false,
            agc_target_db: -16.0,
            silence_trim: false,
            silence_thresh_db: -50.0,
            silence_max_secs: 0.8,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Bq {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z: [f32; 4],
}

impl Bq {
    fn set_peaking(&mut self, f: f32, gain_db: f32) {
        self.set(FilterKind::Peak, f, gain_db, 1.4);
    }

    /// The filter's coefficients (RBJ audio EQ cookbook). The delay line is kept, so a change
    /// while playing doesn't click.
    fn set(&mut self, kind: FilterKind, f: f32, gain_db: f32, q: f32) {
        let [b0, b1, b2, a1, a2] = coefs(kind, f, gain_db, q, SR as f32);
        (self.b0, self.b1, self.b2, self.a1, self.a2) = (b0, b1, b2, a1, a2);
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let yl = self.b0 * l + self.z[0];
        self.z[0] = self.b1 * l - self.a1 * yl + self.z[1];
        self.z[1] = self.b2 * l - self.a2 * yl;
        let yr = self.b0 * r + self.z[2];
        self.z[2] = self.b1 * r - self.a1 * yr + self.z[3];
        self.z[3] = self.b2 * r - self.a2 * yr;
        (yl, yr)
    }
}

/// Normalised biquad coefficients `[b0, b1, b2, a1, a2]` for one filter.
fn coefs(kind: FilterKind, f: f32, gain_db: f32, q: f32, sr: f32) -> [f32; 5] {
    let f = f.clamp(10.0, sr * 0.45);
    let q = q.clamp(0.1, 30.0);
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f / sr;
    let (s, c) = w0.sin_cos();
    let alpha = s / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        FilterKind::Peak => (1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a),
        FilterKind::LowShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * c + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - k),
                (a + 1.0) + (a - 1.0) * c + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - k,
            )
        }
        FilterKind::HighShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * c + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - k),
                (a + 1.0) - (a - 1.0) * c + k,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - k,
            )
        }
        FilterKind::LowCut => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
        FilterKind::HighCut => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
        FilterKind::Notch => (1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// How much one filter lifts or cuts frequency `at` (dB), for drawing the curve.
fn coef_db(k: &[f32; 5], at: f32, sr: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * at / sr;
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    // H(e^jw) = (b0 + b1 e^-jw + b2 e^-2jw) / (1 + a1 e^-jw + a2 e^-2jw)
    let (nr, ni) = (k[0] + k[1] * c1 + k[2] * c2, -(k[1] * s1 + k[2] * s2));
    let (dr, di) = (1.0 + k[3] * c1 + k[4] * c2, -(k[3] * s1 + k[4] * s2));
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).max(1e-12).log10()
}

/// The equalizer's overall response at each frequency in `freqs` (dB, preamp included), as set in `c`.
pub fn eq_response(c: &DspConfig, freqs: &[f32]) -> Vec<f32> {
    let sr = SR as f32;
    let filters: Vec<[f32; 5]> = match c.eq_mode {
        EqMode::Graphic => {
            (0..12).filter(|&i| c.eq_gains[i].abs() > 0.01).map(|i| coefs(FilterKind::Peak, EQ_FREQS[i], c.eq_gains[i], 1.4, sr)).collect()
        }
        EqMode::Parametric => c.peq_bands.iter().filter(|b| b.active()).map(|b| coefs(b.kind, b.freq, b.gain_db, b.q, sr)).collect(),
    };
    freqs.iter().map(|&f| c.eq_preamp_db + filters.iter().map(|k| coef_db(k, f, sr)).sum::<f32>()).collect()
}

pub struct Dsp {
    cfg: DspConfig,
    bands: [Bq; 12],
    peq: Vec<Bq>,
    rms: f32,
    agc_db: f32,
    red_db: f32,
}

impl Dsp {
    pub fn new() -> Self {
        let mut d = Dsp { cfg: DspConfig::default(), bands: [Bq::default(); 12], peq: vec![], rms: 0.0, agc_db: 0.0, red_db: 0.0 };
        d.recalc();
        d
    }

    fn recalc(&mut self) {
        for (i, b) in self.bands.iter_mut().enumerate() {
            b.set_peaking(EQ_FREQS[i], self.cfg.eq_gains[i]);
        }
        let bands = self.cfg.peq_bands.iter().take(PEQ_MAX_BANDS);
        self.peq.resize(bands.len(), Bq::default());
        for (q, b) in self.peq.iter_mut().zip(bands) {
            q.set(b.kind, b.freq, b.gain_db, b.q);
        }
    }

    pub fn set_config(&mut self, c: &DspConfig) {
        let eq_changed = c.eq_gains != self.cfg.eq_gains || c.peq_bands != self.cfg.peq_bands;
        self.cfg = c.clone();
        if eq_changed {
            self.recalc();
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        let c = &self.cfg;
        if !c.eq_enabled && !c.comp_enabled && !c.agc_enabled {
            return;
        }
        let sr = SR as f32;
        let ratio = c.comp_ratio.max(1.0);
        let att = (-1.0 / (c.comp_attack_ms.max(0.1) * 0.001 * sr)).exp();
        let rel = (-1.0 / (c.comp_release_ms.max(1.0) * 0.001 * sr)).exp();
        let makeup = 10f32.powf(c.comp_makeup_db / 20.0);
        let preamp = 10f32.powf(c.eq_preamp_db / 20.0);
        let rms_a = 1.0 / sr;
        let agc_a = 1.0 / (sr * 4.0);
        for f in buf.chunks_exact_mut(2) {
            let (mut l, mut r) = (f[0], f[1]);
            if c.eq_enabled {
                l *= preamp;
                r *= preamp;
                match c.eq_mode {
                    EqMode::Graphic => {
                        for (i, b) in self.bands.iter_mut().enumerate() {
                            if c.eq_gains[i].abs() > 0.01 {
                                (l, r) = b.process(l, r);
                            }
                        }
                    }
                    EqMode::Parametric => {
                        for (q, b) in self.peq.iter_mut().zip(&c.peq_bands) {
                            if b.active() {
                                (l, r) = q.process(l, r);
                            }
                        }
                    }
                }
            }
            if c.agc_enabled {
                self.rms += ((l * l + r * r) * 0.5 - self.rms) * rms_a;
                let level_db = 10.0 * (self.rms + 1e-12).log10();
                if level_db > -55.0 {
                    let want = (c.agc_target_db - level_db).clamp(-10.0, 10.0);
                    self.agc_db += (want - self.agc_db) * agc_a;
                }
                let g = 10f32.powf(self.agc_db / 20.0);
                l *= g;
                r *= g;
            }
            if c.comp_enabled {
                let peak = l.abs().max(r.abs());
                let lvl = 20.0 * (peak + 1e-9).log10();
                let over = (lvl - c.comp_threshold_db).max(0.0);
                let target = over * (1.0 - 1.0 / ratio);
                let coef = if target > self.red_db { att } else { rel };
                self.red_db = target + coef * (self.red_db - target);
                let g = 10f32.powf(-self.red_db / 20.0) * makeup;
                l *= g;
                r *= g;
            }
            f[0] = l;
            f[1] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let v = amp * (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / SR as f32).sin();
                [v, v]
            })
            .collect()
    }
    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
    }

    #[test]
    fn eq_boosts_1k() {
        let mut d = Dsp::new();
        let mut c = DspConfig::default();
        c.eq_enabled = true;
        c.eq_gains[5] = 12.0;
        d.set_config(&c);
        let mut b = sine(0.1, 44100);
        d.process(&mut b);
        let r = rms(&b[b.len() / 2..]) / (0.1 / 2f32.sqrt());
        assert!(r > 3.0 && r < 5.0, "gain ratio {r}"); // ~ +12 dB = x3.98
    }

    #[test]
    fn parametric_bell_boosts_1k_and_low_cut_removes_lows() {
        let mut d = Dsp::new();
        let mut c = DspConfig::default();
        c.eq_enabled = true;
        c.eq_mode = EqMode::Parametric;
        c.peq_bands = vec![PeqBand::new(FilterKind::Peak, 1000.0, 6.0, 1.0)];
        d.set_config(&c);
        let mut b = sine(0.1, 44100);
        d.process(&mut b);
        let r = rms(&b[b.len() / 2..]) / (0.1 / 2f32.sqrt());
        assert!(r > 1.8 && r < 2.2, "gain ratio {r}"); // +6 dB = x2.0

        c.peq_bands = vec![PeqBand::new(FilterKind::LowCut, 8000.0, 0.0, 0.71)];
        d.set_config(&c);
        let mut b = sine(0.1, 44100);
        d.process(&mut b);
        assert!(rms(&b[b.len() / 2..]) < 0.1 / 2f32.sqrt() * 0.1);
    }

    #[test]
    fn response_curve_matches_the_filters() {
        let mut c = DspConfig::default();
        c.eq_gains[5] = 12.0;
        let r = eq_response(&c, &[1000.0, 20.0]);
        assert!((r[0] - 12.0).abs() < 0.3, "{r:?}");
        assert!(r[1].abs() < 0.5, "{r:?}");
        c.eq_mode = EqMode::Parametric;
        c.eq_preamp_db = -3.0;
        c.peq_bands = vec![PeqBand::new(FilterKind::HighShelf, 4000.0, 6.0, 0.71)];
        let r = eq_response(&c, &[100.0, 16000.0]);
        assert!((r[0] + 3.0).abs() < 0.3 && (r[1] - 3.0).abs() < 0.5, "{r:?}");
    }

    #[test]
    fn old_settings_keep_the_graphic_eq() {
        let c: DspConfig = serde_json::from_str(r#"{"eq_enabled":true,"eq_gains":[1,2,3,4,5,6,7,8,9,10,11,12]}"#).unwrap();
        assert_eq!(c.eq_mode, EqMode::Graphic);
        assert_eq!(c.peq_bands, default_peq_bands());
    }

    #[test]
    fn compressor_reduces_loud_signal() {
        let mut d = Dsp::new();
        let mut c = DspConfig::default();
        c.comp_enabled = true;
        c.comp_makeup_db = 0.0;
        c.comp_threshold_db = -20.0;
        c.comp_ratio = 4.0;
        d.set_config(&c);
        let mut b = sine(0.8, 44100);
        let before = rms(&b);
        d.process(&mut b);
        assert!(rms(&b[b.len() / 2..]) < before * 0.7);
    }
}
