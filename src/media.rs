//! Video files as tracks. Their sound plays like any other file: symphonia reads the AAC in
//! MP4/MOV/M4V and the AAC/Vorbis/FLAC in MKV/WebM itself, and anything else (Opus in WebM, AVI,
//! WMV, MPEG…) is decoded by ffmpeg when it is installed. The Video window (video.rs) shows the
//! picture, also through ffmpeg.
use crate::engine::{Pcm, SR};
use parking_lot::Mutex;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

/// The ffmpeg chosen in Settings, and the program found for it (looked for once per choice).
static FFMPEG: Mutex<Option<(String, Option<PathBuf>)>> = Mutex::new(None);
static CHOICE: Mutex<String> = Mutex::new(String::new());

/// Remember the ffmpeg program set in Settings (empty = look for it). Cheap: call it as often as you like.
pub fn set_ffmpeg_choice(chosen: &str) {
    let mut c = CHOICE.lock();
    if *c != chosen {
        *c = chosen.to_string();
    }
}

/// The ffmpeg program to use, if there is one. The first call for a choice may start ffmpeg to
/// see if it is on the PATH, so call it off the UI thread.
pub fn ffmpeg() -> Option<PathBuf> {
    let chosen = CHOICE.lock().clone();
    if let Some((c, found)) = FFMPEG.lock().as_ref() {
        if *c == chosen {
            return found.clone();
        }
    }
    let found = crate::stream::find_ffmpeg(&chosen);
    *FFMPEG.lock() = Some((chosen, found.clone()));
    found
}

/// A command that opens no console window on Windows.
pub fn quiet(prog: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(prog);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// What ffmpeg says about a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    /// seconds; 0 = unknown
    pub duration: f32,
    /// width and height of the first real video stream (cover art doesn't count)
    pub size: Option<(u32, u32)>,
    pub has_audio: bool,
}

/// Ask ffmpeg about a file. None when there is no ffmpeg or it can't read the file.
pub fn probe(path: &Path) -> Option<Probe> {
    let ff = ffmpeg()?;
    let out = quiet(&ff).args(["-nostdin", "-hide_banner", "-i"]).arg(path).stdin(Stdio::null()).stdout(Stdio::null()).output().ok()?;
    let p = parse_probe(&String::from_utf8_lossy(&out.stderr));
    (p.duration > 0.0 || p.size.is_some() || p.has_audio).then_some(p)
}

/// Read `ffmpeg -i`'s report: "Duration: 00:03:21.45, …" and "Stream #0:0: Video: h264 …, 1920x1080 …".
pub fn parse_probe(text: &str) -> Probe {
    let mut p = Probe::default();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Duration:") {
            let hms = rest.trim().split(',').next().unwrap_or("");
            let parts: Vec<f32> = hms.split(':').filter_map(|x| x.trim().parse().ok()).collect();
            if parts.len() == 3 {
                p.duration = parts[0] * 3600.0 + parts[1] * 60.0 + parts[2];
            }
        } else if line.starts_with("Stream #") {
            if line.contains("Audio:") {
                p.has_audio = true;
            } else if let Some(v) = line.split("Video:").nth(1) {
                if p.size.is_some() || line.contains("attached pic") {
                    continue;
                }
                p.size = v.split(',').find_map(|part| {
                    let word = part.split_whitespace().next()?;
                    let (w, h) = word.split_once('x')?;
                    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
                    (w > 0 && h > 0).then_some((w, h))
                });
            }
        }
    }
    p
}

/// A file's sound decoded by ffmpeg from `secs` on: interleaved stereo f32 at `SR`.
struct FfAudio {
    child: Child,
    out: BufReader<ChildStdout>,
}

impl Iterator for FfAudio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let mut b = [0u8; 4];
        self.out.read_exact(&mut b).ok()?;
        Some(f32::from_le_bytes(b))
    }
}

impl Drop for FfAudio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Open a file's sound through ffmpeg at `secs` seconds, with its length (0 = unknown).
/// Used for the files symphonia can't read.
pub fn open_audio(path: &Path, secs: f32) -> anyhow::Result<(Pcm, f32)> {
    let ff = ffmpeg().ok_or_else(|| anyhow::anyhow!("this file needs ffmpeg, which isn't installed"))?;
    let info = probe(path).ok_or_else(|| anyhow::anyhow!("ffmpeg can't read this file"))?;
    if !info.has_audio {
        anyhow::bail!("the file has no sound");
    }
    let mut c = quiet(&ff);
    c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    if secs > 0.0 {
        c.args(["-ss", &format!("{secs:.3}")]);
    }
    c.arg("-i").arg(path);
    c.args(["-vn", "-sn", "-dn", "-ac", "2", "-ar", &SR.to_string(), "-f", "f32le", "-"]);
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let out = child.stdout.take().ok_or_else(|| anyhow::anyhow!("ffmpeg gave no output"))?;
    Ok((Box::new(FfAudio { child, out: BufReader::with_capacity(256 * 1024, out) }), info.duration))
}

/// A video file's sound read by symphonia: the first track it can decode (the picture is skipped),
/// interleaved f32 at the file's own rate. rodio's decoder isn't used for video files because it
/// panics on them.
pub struct SymAudio {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track: u32,
    channels: u16,
    rate: u32,
    duration: f32,
    buf: Vec<f32>,
    pos: usize,
}

impl SymAudio {
    /// Open `path` at `secs` seconds.
    pub fn open(path: &Path, secs: f32) -> anyhow::Result<SymAudio> {
        use symphonia::core::{
            codecs::{DecoderOptions, CODEC_TYPE_NULL},
            formats::{FormatOptions, SeekMode, SeekTo},
            io::MediaSourceStream,
            meta::MetadataOptions,
            probe::Hint,
            units::Time,
        };
        let file = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(&ext.to_ascii_lowercase());
        }
        let opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let mut format = symphonia::default::get_probe().format(&hint, mss, &opts, &MetadataOptions::default())?.format;
        let codecs = symphonia::default::get_codecs();
        let track = format
            .tracks()
            .iter()
            .find(|t| {
                t.codec_params.codec != CODEC_TYPE_NULL
                    && t.codec_params.sample_rate.is_some()
                    && codecs.get_codec(t.codec_params.codec).is_some()
            })
            .ok_or_else(|| anyhow::anyhow!("no sound symphonia can read"))?;
        let (id, params) = (track.id, track.codec_params.clone());
        let decoder = codecs.make(&params, &DecoderOptions::default())?;
        let rate = params.sample_rate.unwrap_or(SR).max(1);
        let channels = params.channels.map(|c| c.count() as u16).unwrap_or(2).max(1);
        let duration = params.time_base.zip(params.n_frames).map(|(b, n)| {
            let t = b.calc_time(n);
            t.seconds as f32 + t.frac as f32
        });
        let duration = duration.or(params.n_frames.map(|n| n as f32 / rate as f32)).unwrap_or(0.0);
        if secs > 0.0 {
            let time = Time { seconds: secs as u64, frac: secs.fract() as f64 };
            format.seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(id) })?;
        }
        let mut a = SymAudio { format, decoder, track: id, channels, rate, duration, buf: vec![], pos: 0 };
        // the first packet tells the real layout (some files don't say in the header)
        a.refill();
        Ok(a)
    }

    pub fn duration(&self) -> f32 {
        self.duration
    }

    /// Decode the next packet of our track. False at the end.
    fn refill(&mut self) -> bool {
        use symphonia::core::{audio::SampleBuffer, errors::Error};
        self.buf.clear();
        self.pos = 0;
        let mut errors = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return false, // the end, or something unreadable
            };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(d) => {
                    let spec = *d.spec();
                    if d.frames() == 0 {
                        continue;
                    }
                    self.channels = spec.channels.count().max(1) as u16;
                    self.rate = spec.rate.max(1);
                    let mut sb = SampleBuffer::<f32>::new(d.capacity() as u64, spec);
                    sb.copy_interleaved_ref(d);
                    self.buf.extend_from_slice(sb.samples());
                    return true;
                }
                Err(Error::DecodeError(_)) if errors < 10 => errors += 1, // a damaged frame: skip it
                Err(_) => return false,
            }
        }
    }
}

impl Iterator for SymAudio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() && !self.refill() {
            return None;
        }
        let v = self.buf[self.pos];
        self.pos += 1;
        Some(v)
    }
}

impl rodio::Source for SymAudio {
    fn current_frame_len(&self) -> Option<usize> {
        // the layout is taken from the first packet and stays
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

/// A video file's sound from `secs` on as stereo at `SR`, with its length (0 = unknown): read by
/// symphonia when it can (MP4/MOV/M4V, MKV/WebM with AAC, Vorbis or FLAC), otherwise by ffmpeg.
pub fn open_video(path: &Path, secs: f32) -> anyhow::Result<(Pcm, f32)> {
    match SymAudio::open(path, secs) {
        Ok(a) => {
            let d = a.duration();
            Ok((Box::new(rodio::source::UniformSourceIterator::<_, f32>::new(a, 2, SR)), d))
        }
        Err(e) => open_audio(path, secs).map_err(|f| anyhow::anyhow!("{e}; {f}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ffmpeg_report() {
        let text = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mp4':\n  Duration: 00:01:06.02, start: 0.000000, bitrate: 3089 kb/s\n  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1280x720 [SAR 1:1 DAR 16:9], 2952 kb/s, 30 fps, 30 tbr, 15360 tbn (default)\n  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, mono, fltp, 69 kb/s (default)\nAt least one output file must be specified\n";
        let p = parse_probe(text);
        assert!((p.duration - 66.02).abs() < 0.01);
        assert_eq!(p.size, Some((1280, 720)));
        assert!(p.has_audio);
    }

    #[test]
    fn cover_art_is_not_video() {
        let text = "  Duration: 00:03:00.00, start: 0.025057, bitrate: 320 kb/s\n  Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 320 kb/s\n  Stream #0:1: Video: mjpeg (Baseline), yuvj420p(pc, bt470bg/unknown/unknown), 500x500 [SAR 1:1 DAR 1:1], 90k tbr, 90k tbn (attached pic)\n";
        let p = parse_probe(text);
        assert_eq!(p.size, None);
        assert!((p.duration - 180.0).abs() < 0.01);
    }
}
