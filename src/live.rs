//! Audio sources for playlist items that are not plain files: internet streams, line input,
//! teasers and time announcements. Each produces interleaved stereo f32 at `SR` Hz.

use crate::engine::{open_at, Pcm, SR};
use crate::mic::{capture, Resampler};
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

/// Messages for the station log, written from background threads.
pub type LogSink = Arc<Mutex<Vec<String>>>;

fn log(sink: &LogSink, msg: impl Into<String>) {
    sink.lock().push(msg.into());
}

// ------------------------------------------------------------------ feed

/// Plays blocks pushed by a producer thread. Gives silence while it is buffering (or the network
/// stalls) and ends once the producer has finished and everything has been played.
pub struct FeedSource {
    rx: Receiver<Vec<f32>>,
    cur: Vec<f32>,
    pos: usize,
    started: bool,
    prebuffer: usize,
    done: Arc<AtomicBool>,
    /// a silent frame was started: its right sample must be silent too (keeps L/R aligned)
    half_silent: bool,
}

/// Producer side of a [`FeedSource`]. Dropping it marks the feed as finished.
pub struct Feeder {
    tx: Sender<Vec<f32>>,
    done: Arc<AtomicBool>,
}

impl Feeder {
    /// Blocking send. Returns false once nobody is listening any more.
    pub fn send(&self, block: Vec<f32>) -> bool {
        self.tx.send(block).is_ok()
    }
    /// Non-blocking send (drops the block if the buffer is full). False once nobody is listening.
    pub fn try_send(&self, block: Vec<f32>) -> bool {
        !matches!(self.tx.try_send(block), Err(TrySendError::Disconnected(_)))
    }
    pub fn sender(&self) -> Sender<Vec<f32>> {
        self.tx.clone()
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

/// A bounded feed of `capacity` blocks that starts playing once `prebuffer` blocks are queued.
pub fn feed(capacity: usize, prebuffer: usize) -> (Feeder, FeedSource) {
    let (tx, rx) = bounded(capacity.max(1));
    let done = Arc::new(AtomicBool::new(false));
    let src = FeedSource { rx, cur: Vec::new(), pos: 0, started: false, prebuffer, done: done.clone(), half_silent: false };
    (Feeder { tx, done }, src)
}

impl Iterator for FeedSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.half_silent {
            self.half_silent = false;
            return Some(0.0);
        }
        loop {
            if self.pos < self.cur.len() {
                let v = self.cur[self.pos];
                self.pos += 1;
                return Some(v);
            }
            if !self.started {
                if self.rx.len() < self.prebuffer && !self.done.load(Ordering::Relaxed) {
                    self.half_silent = true;
                    return Some(0.0);
                }
                self.started = true;
            }
            match self.rx.try_recv() {
                Ok(b) => {
                    self.cur = b;
                    self.pos = 0;
                }
                Err(TryRecvError::Empty) if !self.done.load(Ordering::Relaxed) => {
                    self.half_silent = true; // network stall: play silence, keep going
                    return Some(0.0);
                }
                Err(_) => return None,
            }
        }
    }
}

const BLOCK: usize = 4096; // samples (2048 stereo frames ≈ 46 ms)

/// Collects stereo samples and hands them over in `BLOCK`-sized pieces.
struct Chunker {
    buf: Vec<f32>,
}

impl Chunker {
    fn push(&mut self, s: &[f32], f: &Feeder) -> bool {
        self.buf.extend_from_slice(s);
        while self.buf.len() >= BLOCK {
            let rest = self.buf.split_off(BLOCK);
            let block = std::mem::replace(&mut self.buf, rest);
            if !f.send(block) {
                return false;
            }
        }
        true
    }
    fn flush(&mut self, f: &Feeder) {
        if !self.buf.is_empty() {
            let mut b = std::mem::take(&mut self.buf);
            if b.len() % 2 == 1 {
                b.push(0.0);
            }
            f.send(b);
        }
    }
}

// --------------------------------------------------------------- streams

/// Internet radio stream (Icecast / Shoutcast / plain HTTP file): MP3, AAC (ADTS), Ogg Vorbis, FLAC, WAV.
pub fn url_source(url: &str, sink: LogSink) -> FeedSource {
    let (feeder, src) = feed(96, 40); // ~2 s of pre-buffer, ~4 s of headroom
    let url = url.trim().to_string();
    thread::spawn(move || {
        if let Err(e) = run_stream(&url, &feeder) {
            log(&sink, format!("Stream {url}: {e}"));
        }
    });
    src
}

fn run_stream(url: &str, feeder: &Feeder) -> Result<()> {
    use symphonia::core::{
        audio::SampleBuffer,
        codecs::{DecoderOptions, CODEC_TYPE_NULL},
        errors::Error as SymErr,
        formats::FormatOptions,
        io::{MediaSourceStream, ReadOnlySource},
        meta::MetadataOptions,
        probe::Hint,
    };
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(8)).timeout_read(Duration::from_secs(20)).build();
    let resp = agent.get(url).set("User-Agent", "RadioMaster/1.0").call().map_err(|e| anyhow!("could not connect ({e})"))?;
    let mime = resp.content_type().to_string();
    let reader = resp.into_reader();
    let mss = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
    let mut hint = Hint::new();
    hint.mime_type(&mime);
    if mime.contains("aac") {
        hint.with_extension("aac");
    } else if mime.contains("mpeg") || mime.contains("mp3") {
        hint.with_extension("mp3");
    } else if mime.contains("ogg") {
        hint.with_extension("ogg");
    }
    if let Some(ext) = url.split('?').next().and_then(|u| u.rsplit('.').next()).filter(|e| e.len() <= 4) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow!("unsupported stream format ({mime}): {e}"))?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or_else(|| anyhow!("no audio in stream"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut rs: Option<(u32, Resampler)> = None;
    let mut out = Vec::with_capacity(BLOCK * 2);
    let mut chunk = Chunker { buf: Vec::new() };
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(_)) => break, // end of stream / connection closed
            Err(e) => return Err(anyhow!("{e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymErr::DecodeError(_)) => continue, // a damaged frame: skip it
            Err(e) => return Err(anyhow!("{e}")),
        };
        let spec = *decoded.spec();
        let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sb.copy_interleaved_ref(decoded);
        if rs.as_ref().map(|(r, _)| *r) != Some(spec.rate) {
            rs = Some((spec.rate, Resampler::new(spec.rate as f32)));
        }
        out.clear();
        rs.as_mut().unwrap().1.feed_interleaved(sb.samples(), spec.channels.count(), &mut out);
        if !chunk.push(&out, feeder) {
            return Ok(()); // the item was stopped
        }
    }
    chunk.flush(feeder);
    Ok(())
}

// ------------------------------------------------------------ line input

/// Live input device as a playlist item. Runs until the item is stopped or its time is up.
pub fn line_source(device: &str, sink: LogSink) -> FeedSource {
    let (feeder, src) = feed(48, 3);
    let device = device.to_string();
    thread::spawn(move || {
        let tx = feeder.sender();
        let gone = Arc::new(AtomicBool::new(false));
        let g2 = gone.clone();
        let stream = capture(&device, move |s| {
            if !s.is_empty() && matches!(tx.try_send(s.to_vec()), Err(TrySendError::Disconnected(_))) {
                g2.store(true, Ordering::Relaxed);
            }
        });
        match stream {
            Ok(stream) => {
                while !gone.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(e) => log(&sink, format!("Line input: {e}")),
        }
        drop(feeder);
    });
    src
}

// ---------------------------------------------------------------- teaser

/// Short snippets of several tracks back to back, each faded in and out.
struct Teaser {
    parts: VecDeque<Pcm>,
    cur: Option<Pcm>,
    left: usize,
    len: usize,
    fade: usize,
}

impl Iterator for Teaser {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        loop {
            if self.cur.is_none() {
                self.cur = Some(self.parts.pop_front()?);
                self.left = self.len;
            }
            let i = self.len - self.left;
            match self.cur.as_mut().unwrap().next() {
                Some(s) => {
                    let g = (i as f32 / self.fade as f32).min(self.left as f32 / self.fade as f32).min(1.0);
                    self.left -= 1;
                    if self.left == 0 {
                        self.cur = None;
                    }
                    return Some(s * g);
                }
                None => self.cur = None,
            }
        }
    }
}

/// `parts`: (file, where to start in seconds). Each plays for `secs` seconds.
pub fn teaser_source(parts: &[(PathBuf, f32)], secs: f32) -> Result<Pcm> {
    let mut opened = VecDeque::new();
    for (p, start) in parts {
        if let Ok(src) = open_at(p, *start) {
            opened.push_back(src);
        }
    }
    if opened.is_empty() {
        return Err(anyhow!("no upcoming tracks to tease"));
    }
    let len = ((secs.max(1.0) * SR as f32) as usize) * 2;
    let fade = ((0.6 * SR as f32) as usize * 2).min(len / 3).max(2);
    Ok(Box::new(Teaser { parts: opened, cur: None, left: 0, len, fade }))
}

// ----------------------------------------------------- time announcement

/// Text for a time announcement, e.g. "It's {h12}:{mm} {ampm}".
pub fn announce_text(template: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    use chrono::Timelike;
    let (h, m) = (now.hour(), now.minute());
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let ampm = if h < 12 { "AM" } else { "PM" };
    let t = if template.trim().is_empty() { "It's {h12}:{mm} {ampm}" } else { template };
    t.replace("{h24}", &format!("{h:02}"))
        .replace("{h12}", &h12.to_string())
        .replace("{mm}", &format!("{m:02}"))
        .replace("{ampm}", ampm)
        .replace("{time}", &format!("{h12}:{m:02} {ampm}"))
}

/// Recorded announcement files in `folder`: H00…H23 for hours and M00…M59 for minutes (any audio type).
pub fn announce_files(folder: &Path, now: &chrono::DateTime<chrono::Local>) -> Vec<PathBuf> {
    use chrono::Timelike;
    let find = |stem: String| -> Option<PathBuf> {
        let rd = std::fs::read_dir(folder).ok()?;
        rd.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
            crate::library::is_audio(p) && p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem)).unwrap_or(false)
        })
    };
    let mut v = vec![];
    if let Some(h) = find(format!("H{:02}", now.hour())) {
        v.push(h);
        if let Some(m) = find(format!("M{:02}", now.minute())) {
            v.push(m);
        }
    }
    v
}

/// Speak `text` into a WAV file with the system's speech engine.
pub fn tts_to_wav(text: &str, out: &Path, voice: &str) -> Result<()> {
    use std::process::Command;
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::CommandExt;
        let q = |s: &str| s.replace('\'', "''");
        let select = if voice.trim().is_empty() { String::new() } else { format!("$s.SelectVoice('{}'); ", q(voice)) };
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; {select}$s.SetOutputToWaveFile('{}'); $s.Speak('{}'); $s.Dispose()",
            q(&out.to_string_lossy()),
            q(text)
        );
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()?
    };
    #[cfg(target_os = "macos")]
    let status = {
        let mut c = Command::new("say");
        if !voice.trim().is_empty() {
            c.args(["-v", voice]);
        }
        c.args(["--file-format=WAVE", "--data-format=LEI16@22050", "-o"]).arg(out).arg(text).status()?
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = {
        let run = |bin: &str| {
            let mut c = Command::new(bin);
            if !voice.trim().is_empty() {
                c.args(["-v", voice]);
            }
            c.arg("-w").arg(out).arg(text).status()
        };
        run("espeak-ng").or_else(|_| run("espeak")).map_err(|_| anyhow!("install espeak-ng for spoken time announcements"))?
    };
    if !status.success() || !out.exists() {
        return Err(anyhow!("speech engine failed"));
    }
    Ok(())
}

/// Installed speech voices (best effort; empty if unknown).
pub fn list_voices() -> Vec<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name }",
            ])
            .creation_flags(0x0800_0000)
            .output();
        if let Ok(o) = out {
            return String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        }
    }
    Vec::new()
}

/// Time announcement as a playable source. Uses recorded files if `folder` has them, otherwise speech.
pub fn announce_source(text: String, folder: String, voice: String, tmp: PathBuf, sink: LogSink) -> FeedSource {
    let (feeder, src) = feed(64, 1);
    thread::spawn(move || {
        let now = chrono::Local::now();
        let mut files = if folder.trim().is_empty() { vec![] } else { announce_files(Path::new(&folder), &now) };
        if files.is_empty() {
            let _ = std::fs::create_dir_all(tmp.parent().unwrap_or(Path::new(".")));
            match tts_to_wav(&text, &tmp, &voice) {
                Ok(()) => files.push(tmp.clone()),
                Err(e) => {
                    log(&sink, format!("Time announcement: {e}"));
                    return;
                }
            }
        }
        let mut chunk = Chunker { buf: Vec::new() };
        for f in files {
            match open_at(&f, 0.0) {
                Ok(src) => {
                    let mut tmpb = Vec::with_capacity(BLOCK);
                    for s in src {
                        tmpb.push(s);
                        if tmpb.len() == BLOCK {
                            if !chunk.push(&tmpb, &feeder) {
                                return;
                            }
                            tmpb.clear();
                        }
                    }
                    chunk.push(&tmpb, &feeder);
                }
                Err(e) => log(&sink, format!("Time announcement {}: {e}", f.display())),
            }
        }
        chunk.flush(&feeder);
    });
    src
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_keeps_channels_aligned_and_ends() {
        let (f, mut src) = feed(8, 2);
        // nothing queued yet: silence comes in whole stereo frames
        assert_eq!(src.next(), Some(0.0));
        assert_eq!(src.next(), Some(0.0));
        assert!(f.send(vec![0.1, 0.2, 0.3, 0.4]));
        assert!(f.send(vec![0.5, 0.6]));
        let got: Vec<f32> = (0..6).map(|_| src.next().unwrap()).collect();
        assert_eq!(got, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        drop(f);
        assert_eq!(src.next(), None, "ends once the producer is done and drained");
    }

    #[test]
    fn teaser_fades_each_part() {
        let p = std::env::temp_dir().join("rm_teaser_tone.wav");
        let tone: Vec<f32> = (0..SR * 3)
            .flat_map(|k| {
                let v = 0.5 * (k as f32 * 440.0 * std::f32::consts::TAU / SR as f32).sin();
                [v, v]
            })
            .collect();
        crate::engine::write_wav(&p, &tone, 2, SR).unwrap();
        let src = teaser_source(&[(p.clone(), 0.5), (p.clone(), 1.0)], 1.0).unwrap();
        let out: Vec<f32> = src.collect();
        assert_eq!(out.len(), 2 * 2 * SR as usize, "two 1-second snippets");
        let peak = |a: usize, b: usize| out[a..b].iter().fold(0f32, |m, x| m.max(x.abs()));
        let n = SR as usize * 2;
        assert!(peak(0, 400) < 0.05, "starts faded in");
        assert!(peak(n / 2 - 2000, n / 2 + 2000) > 0.4, "full level in the middle");
        assert!(peak(n - 400, n) < 0.05, "faded out before the next part");
    }

    #[test]
    fn announce_text_template() {
        use chrono::TimeZone;
        let t = chrono::Local.with_ymd_and_hms(2026, 10, 17, 14, 5, 0).unwrap();
        assert_eq!(announce_text("It's {h12}:{mm} {ampm}", &t), "It's 2:05 PM");
        assert_eq!(announce_text("{h24}{mm}", &t), "1405");
    }
}
