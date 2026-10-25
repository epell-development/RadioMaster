//! Master-bus DSP: 12-band graphic EQ, compressor, slow auto volume leveling (AGC).

use crate::engine::SR;
use serde::{Deserialize, Serialize};

pub const EQ_FREQS: [f32; 12] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 3000.0, 4000.0, 8000.0, 12000.0, 16000.0];

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct DspConfig {
    pub eq_enabled: bool,
    pub eq_gains: [f32; 12],
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
            eq_gains: [0.0; 12],
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
        let q = 1.4;
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f / SR as f32;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        self.b0 = (1.0 + alpha * a) / a0;
        self.b1 = (-2.0 * c) / a0;
        self.b2 = (1.0 - alpha * a) / a0;
        self.a1 = (-2.0 * c) / a0;
        self.a2 = (1.0 - alpha / a) / a0;
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

pub struct Dsp {
    cfg: DspConfig,
    bands: [Bq; 12],
    rms: f32,
    agc_db: f32,
    red_db: f32,
}

impl Dsp {
    pub fn new() -> Self {
        let mut d = Dsp { cfg: DspConfig::default(), bands: [Bq::default(); 12], rms: 0.0, agc_db: 0.0, red_db: 0.0 };
        d.recalc();
        d
    }

    fn recalc(&mut self) {
        for (i, b) in self.bands.iter_mut().enumerate() {
            b.set_peaking(EQ_FREQS[i], self.cfg.eq_gains[i]);
        }
    }

    pub fn set_config(&mut self, c: &DspConfig) {
        let eq_changed = c.eq_gains != self.cfg.eq_gains;
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
        let rms_a = 1.0 / sr;
        let agc_a = 1.0 / (sr * 4.0);
        for f in buf.as_chunks_mut::<2>().0 {
            let (mut l, mut r) = (f[0], f[1]);
            if c.eq_enabled {
                for (i, b) in self.bands.iter_mut().enumerate() {
                    if c.eq_gains[i].abs() > 0.01 {
                        (l, r) = b.process(l, r);
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
