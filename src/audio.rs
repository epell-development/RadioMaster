//! Output/input device selection. Several output devices can play at once: the first one that
//! opens becomes the primary (it drives the mix), the others receive a copy of the master mix.
//! With no usable device a silent real-time clock drives the mix so streaming still works.

use crate::config::AudioDevices;
use crate::engine::{Engine, GainSource};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, SampleFormat, SizedSample,
};
use rodio::source::UniformSourceIterator;
use rodio::Source;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

pub fn list_outputs() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices().map(|d| d.filter_map(|x| x.name().ok()).collect()).unwrap_or_default()
}

pub fn list_inputs() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices().map(|d| d.filter_map(|x| x.name().ok()).collect()).unwrap_or_default()
}

pub fn default_output_name() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

/// Sound card buffer when the setting is left on its default.
pub const DEFAULT_BUFFER_MS: u32 = 80;

/// Plays `make()` on `device` through cpal, with a buffer of about `buffer_ms` milliseconds.
/// A bigger buffer than the driver's default rides out a busy CPU (slow PCs) instead of crackling.
/// `make` is called again if the driver refuses that buffer size and its own default is used.
fn open_output<S, F>(device: &cpal::Device, buffer_ms: u32, mut make: F) -> anyhow::Result<cpal::Stream>
where
    S: Source<Item = f32> + Send + 'static,
    F: FnMut() -> S,
{
    let sup = device.default_output_config()?;
    let format = sup.sample_format();
    let mut cfg = sup.config();
    let ms = if buffer_ms == 0 { DEFAULT_BUFFER_MS } else { buffer_ms };
    let mut frames = (cfg.sample_rate.0 as u64 * ms as u64 / 1000) as u32;
    if let cpal::SupportedBufferSize::Range { min, max } = sup.buffer_size() {
        frames = frames.clamp(*min, *max);
    }
    cfg.buffer_size = cpal::BufferSize::Fixed(frames);
    let stream = match build_output(device, &cfg, format, make()) {
        Ok(s) => s,
        Err(_) => {
            cfg.buffer_size = cpal::BufferSize::Default;
            build_output(device, &cfg, format, make())?
        }
    };
    stream.play()?;
    Ok(stream)
}

fn build_output<S>(device: &cpal::Device, cfg: &cpal::StreamConfig, format: SampleFormat, src: S) -> anyhow::Result<cpal::Stream>
where
    S: Source<Item = f32> + Send + 'static,
{
    match format {
        SampleFormat::F32 => build_typed::<f32, S>(device, cfg, src),
        SampleFormat::I16 => build_typed::<i16, S>(device, cfg, src),
        SampleFormat::U16 => build_typed::<u16, S>(device, cfg, src),
        SampleFormat::I32 => build_typed::<i32, S>(device, cfg, src),
        SampleFormat::F64 => build_typed::<f64, S>(device, cfg, src),
        other => Err(anyhow::anyhow!("unsupported sample format {other}")),
    }
}

fn build_typed<T, S>(device: &cpal::Device, cfg: &cpal::StreamConfig, src: S) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
    S: Source<Item = f32> + Send + 'static,
{
    // converted to the card's own rate and channel count here, on the audio thread, sample by sample
    let mut src = UniformSourceIterator::<S, f32>::new(src, cfg.channels, cfg.sample_rate.0);
    let stream = device.build_output_stream::<T, _, _>(
        cfg,
        move |out: &mut [T], _| {
            for x in out.iter_mut() {
                *x = T::from_sample(src.next().unwrap_or(0.0));
            }
        },
        |e| eprintln!("audio output: {e}"),
        None,
    )?;
    Ok(stream)
}

fn find_output(name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if name.is_empty() {
        return host.default_output_device();
    }
    host.output_devices()
        .ok()
        .and_then(|mut it| it.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
        .or_else(|| host.default_output_device())
}

pub struct Running {
    pub name: String,
    pub primary: bool,
    pub gain: Arc<AtomicU32>,
    _stream: cpal::Stream, // must stay alive; not Send, so the manager lives on the UI thread
}

#[derive(Default)]
pub struct AudioManager {
    pub running: Vec<Running>,
    pub report: Vec<(String, String)>, // (device, status)
    null_stop: Option<Arc<AtomicBool>>,
}

impl AudioManager {
    pub fn set_gain(&self, name: &str, g: f32) {
        for r in &self.running {
            if r.name == name {
                r.gain.store(g.max(0.0).to_bits(), Ordering::Relaxed);
            }
        }
    }

    pub fn role_of(&self, name: &str) -> Option<&'static str> {
        self.running.iter().find(|r| r.name == name).map(|r| if r.primary { "primary" } else { "monitor" })
    }

    /// Close every output and keep the mix running on a silent clock (streaming only).
    #[allow(dead_code)] // RadioMaster-Server
    pub fn silence(&mut self, engine: &Engine) {
        self.running.clear();
        self.report.clear();
        if let Some(s) = self.null_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        self.report.push(("(sound cards off)".into(), "running silently — streaming and automation still work".into()));
        self.null_stop = Some(engine.start_null_driver());
    }

    /// (Re)open every enabled output. Safe to call at any time.
    pub fn apply(&mut self, engine: &Engine, cfg: &AudioDevices) {
        self.running.clear(); // dropping the streams closes the devices
        self.report.clear();
        if let Some(s) = self.null_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        let host = cpal::default_host();
        let mut wanted: Vec<(String, f32, bool)> =
            cfg.outputs.iter().filter(|o| o.enabled).map(|o| (o.name.clone(), o.gain, o.studio)).collect();
        let use_default = wanted.is_empty();
        if use_default {
            wanted.push((default_output_name().unwrap_or_else(|| "System default".into()), 1.0, false));
        }
        let mut have_primary = false;
        for (name, gain, studio) in wanted {
            let device = if use_default {
                host.default_output_device()
            } else {
                host.output_devices().ok().and_then(|mut it| it.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
            };
            let Some(device) = device else {
                self.report.push((name, "not found (unplugged?)".into()));
                continue;
            };
            let g = Arc::new(AtomicU32::new(gain.max(0.0).to_bits()));
            let ms = cfg.buffer_ms;
            // a studio monitor is gated after the mix, so muting it never touches the stream
            let opened = match (have_primary, studio) {
                (false, false) => open_output(&device, ms, || GainSource::new(engine.primary_source(), g.clone())),
                (false, true) => open_output(&device, ms, || GainSource::new(engine.studio_gate(engine.primary_source()), g.clone())),
                (true, false) => open_output(&device, ms, || GainSource::new(engine.monitor_source(), g.clone())),
                (true, true) => open_output(&device, ms, || GainSource::new(engine.studio_gate(engine.monitor_source()), g.clone())),
            };
            match opened {
                Ok(stream) => {
                    let role = if have_primary { "monitor" } else { "primary" };
                    let extra = if studio { ", muted while the mic is on" } else { "" };
                    self.report.push((name.clone(), format!("playing ({role}{extra})")));
                    self.running.push(Running { name, primary: !have_primary, gain: g, _stream: stream });
                    have_primary = true;
                }
                Err(e) => self.report.push((name, format!("could not open: {e}"))),
            }
        }
        if !have_primary {
            self.report.push(("(no sound device)".into(), "running silently — streaming and automation still work".into()));
            self.null_stop = Some(engine.start_null_driver());
        }
    }
}

/// Plays a transition (end of one track into the start of the next) on a separate device,
/// through its own private engine, so it never reaches the on-air mix or the stream encoders.
pub struct PreviewPlayer {
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    pub engine: Engine,
}

pub struct PreviewSide {
    pub path: std::path::PathBuf,
    pub duration: f32,
    pub cues: crate::fade::Cues,
}

impl PreviewPlayer {
    /// Play any audio source (e.g. a speech preview) on the preview device.
    pub fn play_pcm(device: &str, src: crate::engine::Pcm) -> anyhow::Result<PreviewPlayer> {
        let dev = find_output(device).ok_or_else(|| anyhow::anyhow!("no output device for preview"))?;
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let stream = open_output(&dev, 0, || engine.primary_source())?;
        engine.play_source(src, None, 0.0, crate::engine::CueSpec::default(), Some(0.0), false);
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, e2) = (stop.clone(), engine.clone());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            while !s2.load(Ordering::Relaxed) && e2.status().has_track {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            s2.store(true, Ordering::Relaxed);
        });
        Ok(PreviewPlayer { _stream: stream, stop, engine })
    }

    /// Play A from `from` seconds. If `b` is given, B comes in at A's mix point and plays for `tail`
    /// seconds; otherwise A plays to its cue out.
    pub fn start(device: &str, a: PreviewSide, b: Option<PreviewSide>, from: f32, tail: f32) -> anyhow::Result<PreviewPlayer> {
        use crate::engine::CueSpec;
        let dev = find_output(device).ok_or_else(|| anyhow::anyhow!("no output device for preview"))?;
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        let stream = open_output(&dev, 0, || engine.primary_source())?;
        let ca = a.cues;
        let from = from.clamp(0.0, (ca.cue_out - 0.05).max(0.0));
        let fade_in = if from <= ca.cue_in + 1e-3 { ca.fade_in } else { 0.0 };
        engine.play_cued(
            &a.path,
            a.duration,
            CueSpec { cue_in: from, cue_out: Some(ca.cue_out), fade_in, fade_out_at: Some(ca.fade_out), gain_db: ca.gain_db },
            Some(0.0),
        )?;
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, e2) = (stop.clone(), engine.clone());
        std::thread::spawn(move || {
            let sleep = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
            match b {
                Some(b) => {
                    // wait for A to reach its mix point (or finish)
                    loop {
                        if s2.load(Ordering::Relaxed) {
                            return;
                        }
                        let st = e2.status();
                        if st.pos >= ca.mix - 0.005 || !st.has_track {
                            break;
                        }
                        sleep(5);
                    }
                    let cb = b.cues;
                    let _ = e2.play_cued(
                        &b.path,
                        b.duration,
                        CueSpec {
                            cue_in: cb.cue_in,
                            cue_out: Some(cb.cue_out),
                            fade_in: cb.fade_in,
                            fade_out_at: Some(cb.fade_out),
                            gain_db: cb.gain_db,
                        },
                        None,
                    );
                    let t1 = std::time::Instant::now();
                    while !s2.load(Ordering::Relaxed) && t1.elapsed().as_secs_f32() < tail {
                        sleep(20);
                    }
                }
                None => {
                    sleep(100);
                    while !s2.load(Ordering::Relaxed) && e2.status().has_track {
                        sleep(20);
                    }
                }
            }
            e2.stop(0.3);
            sleep(350);
            s2.store(true, Ordering::Relaxed);
        });
        Ok(PreviewPlayer { _stream: stream, stop, engine })
    }

    pub fn finished(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> (f32, Option<std::path::PathBuf>) {
        (self.engine.status().pos, self.engine.current_path())
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
