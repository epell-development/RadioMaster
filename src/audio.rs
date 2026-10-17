//! Output/input device selection. Several output devices can play at once: the first one that
//! opens becomes the primary (it drives the mix), the others receive a copy of the master mix.
//! With no usable device a silent real-time clock drives the mix so streaming still works.

use crate::config::AudioDevices;
use crate::engine::{Engine, GainSource};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait},
};
use rodio::OutputStream;
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

pub struct Running {
    pub name: String,
    pub primary: bool,
    pub gain: Arc<AtomicU32>,
    _stream: OutputStream, // must stay alive; not Send, so the manager lives on the UI thread
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

    /// (Re)open every enabled output. Safe to call at any time.
    pub fn apply(&mut self, engine: &Engine, cfg: &AudioDevices) {
        self.running.clear(); // dropping the streams closes the devices
        self.report.clear();
        if let Some(s) = self.null_stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        let host = cpal::default_host();
        let mut wanted: Vec<(String, f32)> = cfg.outputs.iter().filter(|o| o.enabled).map(|o| (o.name.clone(), o.gain)).collect();
        let use_default = wanted.is_empty();
        if use_default {
            wanted.push((default_output_name().unwrap_or_else(|| "System default".into()), 1.0));
        }
        let mut have_primary = false;
        for (name, gain) in wanted {
            let device = if use_default {
                host.default_output_device()
            } else {
                host.output_devices().ok().and_then(|mut it| it.find(|d| d.name().map(|n| n == name).unwrap_or(false)))
            };
            let Some(device) = device else {
                self.report.push((name, "not found (unplugged?)".into()));
                continue;
            };
            match OutputStream::try_from_device(&device) {
                Ok((stream, handle)) => {
                    let g = Arc::new(AtomicU32::new(gain.max(0.0).to_bits()));
                    let ok = if !have_primary {
                        handle.play_raw(GainSource::new(engine.primary_source(), g.clone())).is_ok()
                    } else {
                        handle.play_raw(GainSource::new(engine.monitor_source(), g.clone())).is_ok()
                    };
                    if ok {
                        self.report
                            .push((name.clone(), if have_primary { "playing (monitor)".into() } else { "playing (primary)".into() }));
                        self.running.push(Running { name, primary: !have_primary, gain: g, _stream: stream });
                        have_primary = true;
                    } else {
                        self.report.push((name, "could not start playback".into()));
                    }
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
    _stream: OutputStream,
    stop: Arc<AtomicBool>,
    pub engine: Engine,
    pub label: String,
}

pub struct PreviewSide {
    pub path: std::path::PathBuf,
    pub duration: f32,
    pub cues: crate::fade::Cues,
}

impl PreviewPlayer {
    /// Play A from `from` seconds. If `b` is given, B comes in at A's mix point and plays for `tail`
    /// seconds; otherwise A plays to its cue out.
    pub fn start(device: &str, a: PreviewSide, b: Option<PreviewSide>, from: f32, tail: f32) -> anyhow::Result<PreviewPlayer> {
        use crate::engine::CueSpec;
        let host = cpal::default_host();
        let dev = if device.is_empty() {
            host.default_output_device()
        } else {
            host.output_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == device).unwrap_or(false)))
                .or_else(|| host.default_output_device())
        }
        .ok_or_else(|| anyhow::anyhow!("no output device for preview"))?;
        let label = dev.name().unwrap_or_else(|_| "default".into());
        let (stream, handle) = OutputStream::try_from_device(&dev)?;
        let engine = Engine::new_detached();
        engine.set_volume(1.0);
        handle.play_raw(engine.primary_source())?;
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
        Ok(PreviewPlayer { _stream: stream, stop, engine, label })
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
