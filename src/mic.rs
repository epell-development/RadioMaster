//! Microphone / line-in capture. The selected input device is converted to stereo 44.1 kHz and
//! handed to a sink: the master bus (Mic button), a line-input playlist item, or the voice-track recorder.

use crate::engine::{Engine, SR};
use anyhow::{anyhow, Result};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

/// Linear-interpolating stereo resampler (any rate → `SR`).
pub struct Resampler {
    prev: (f32, f32),
    t: f32,
    step: f32,
}

impl Resampler {
    pub fn new(from_rate: f32) -> Self {
        Resampler { prev: (0.0, 0.0), t: 0.0, step: (from_rate / SR as f32).max(1e-3) }
    }

    pub fn feed(&mut self, cur: (f32, f32), out: &mut Vec<f32>) {
        while self.t < 1.0 {
            let a = self.t;
            out.push(self.prev.0 + (cur.0 - self.prev.0) * a);
            out.push(self.prev.1 + (cur.1 - self.prev.1) * a);
            self.t += self.step;
        }
        self.t -= 1.0;
        self.prev = cur;
    }

    /// Feed interleaved samples with `ch` channels (mono is duplicated, extra channels ignored).
    pub fn feed_interleaved(&mut self, data: &[f32], ch: usize, out: &mut Vec<f32>) {
        let ch = ch.max(1);
        for fr in data.chunks_exact(ch) {
            let l = fr[0];
            let r = if ch > 1 { fr[1] } else { l };
            self.feed((l, r), out);
        }
    }
}

fn build<T, F>(dev: &cpal::Device, conf: &cpal::StreamConfig, rate: f32, mut sink: F) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
    F: FnMut(&[f32]) + Send + 'static,
{
    let ch = conf.channels as usize;
    let mut rs = Resampler::new(rate);
    let mut tmp: Vec<f32> = Vec::new();
    let stream = dev.build_input_stream(
        conf,
        move |data: &[T], _| {
            tmp.clear();
            for fr in data.chunks_exact(ch) {
                let l: f32 = <f32 as cpal::FromSample<T>>::from_sample_(fr[0]);
                let r: f32 = if ch > 1 { <f32 as cpal::FromSample<T>>::from_sample_(fr[1]) } else { l };
                rs.feed((l, r), &mut tmp);
            }
            sink(&tmp);
        },
        |e| eprintln!("input error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Open the input device (empty name = system default) and send stereo 44.1 kHz audio to `sink`.
pub fn capture<F: FnMut(&[f32]) + Send + 'static>(device_name: &str, sink: F) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let chosen = if device_name.is_empty() {
        None
    } else {
        host.input_devices().ok().and_then(|mut it| it.find(|d| d.name().map(|n| n == device_name).unwrap_or(false)))
    };
    let dev = chosen.or_else(|| host.default_input_device()).ok_or_else(|| anyhow!("no input device found"))?;
    let cfg = dev.default_input_config()?;
    let rate = cfg.sample_rate().0 as f32;
    let fmt = cfg.sample_format();
    let conf: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => build::<f32, F>(&dev, &conf, rate, sink)?,
        cpal::SampleFormat::I16 => build::<i16, F>(&dev, &conf, rate, sink)?,
        cpal::SampleFormat::U16 => build::<u16, F>(&dev, &conf, rate, sink)?,
        other => return Err(anyhow!("unsupported input sample format {other:?}")),
    };
    stream.play()?;
    Ok(stream)
}

/// Mic button: input goes straight into the master bus (music is ducked by the engine).
pub fn start(engine: Engine, device_name: &str) -> Result<cpal::Stream> {
    capture(device_name, move |s| engine.mic_push(s))
}
