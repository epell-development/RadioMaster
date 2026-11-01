//! Show recording (a logger): the on-air mix is saved to a folder, one file per hour (or another
//! length), lined up with the clock, as proof of broadcast. Files go into a folder per day:
//! `<folder>/2026-11-01/2026-11-01 14-00.mp3`. Recordings older than `keep_days` are deleted.
//! Everything here runs on its own thread; the UI only reads `status`.

use crate::config::{data_dir, RecFormat, RecordCfg};
use crate::engine::{Engine, SR};
use crate::stream::{find_ffmpeg, mp3_encode, mp3_encoder, PipeEncoder};
use anyhow::{anyhow, Context, Result};
use chrono::{Duration as CDuration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use crossbeam_channel::{bounded, RecvTimeoutError};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// What the recorder is doing (also sent to apps connected to a RadioMaster Server).
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(default)]
pub struct RecStatus {
    /// "starting" (no sound from the mixer yet), "recording", "waiting" (outside the days / hours
    /// set), "error" or "stopped"
    pub state: String,
    /// the file being written
    pub file: String,
    /// when this file started and when the next one starts (unix seconds)
    pub seg_start: i64,
    pub seg_end: i64,
    /// seconds of sound in this file
    pub secs: f64,
    pub bytes: u64,
    /// the last problem (empty when there is none)
    pub error: String,
    /// files finished since recording started
    pub files_done: u32,
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    pub status: Arc<Mutex<RecStatus>>,
    /// the settings it was started with
    pub cfg: RecordCfg,
    pub ffmpeg: String,
}

impl Recorder {
    /// `ffmpeg`: the ffmpeg program for FLAC ("" = look for it).
    pub fn start(cfg: RecordCfg, engine: &Engine, ffmpeg: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(RecStatus { state: "starting".into(), ..Default::default() }));
        // about 45 s of sound: enough to ride out a slow network folder
        let (tx, rx) = bounded::<Arc<Vec<i16>>>(2048);
        engine.add_tap(tx);
        let (s2, st2, c2, f2) = (stop.clone(), status.clone(), cfg.clone(), ffmpeg.clone());
        thread::spawn(move || run(&c2, &f2, rx, &s2, &st2));
        Recorder { stop, status, cfg, ffmpeg }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(cfg: &RecordCfg, ffmpeg: &str, rx: crossbeam_channel::Receiver<Arc<Vec<i16>>>, stop: &AtomicBool, status: &Mutex<RecStatus>) {
    let mut seg: Option<Segment> = None;
    // after a file couldn't be opened, wait a little before trying again
    let mut retry_at: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let pcm = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(p) => p,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let now = Local::now().naive_local();
        if !cfg.active_at(now) {
            if let Some(s) = seg.take() {
                finish(s, status);
            }
            let mut st = status.lock();
            if st.state != "waiting" {
                st.state = "waiting".into();
                st.file.clear();
                st.secs = 0.0;
                st.bytes = 0;
            }
            continue;
        }
        let (start, end) = segment_bounds(now, cfg.segment());
        if seg.as_ref().is_some_and(|s| s.start != start) {
            if let Some(s) = seg.take() {
                finish(s, status);
            }
        }
        if seg.is_none() && retry_at.is_none_or(|t| Instant::now() >= t) {
            match Segment::open(cfg, ffmpeg, start) {
                Ok((s, note)) => {
                    retry_at = None;
                    let mut st = status.lock();
                    st.state = "recording".into();
                    st.file = s.path.display().to_string();
                    st.seg_start = unix(start);
                    st.seg_end = unix(end);
                    st.secs = 0.0;
                    st.bytes = 0;
                    st.error = note;
                    drop(st);
                    seg = Some(s);
                    if cfg.keep_days > 0 {
                        let (dir, days) = (cfg.folder_path(), cfg.keep_days);
                        thread::spawn(move || delete_old(&dir, now.date() - CDuration::days(days as i64)));
                    }
                }
                Err(e) => {
                    retry_at = Some(Instant::now() + Duration::from_secs(10));
                    let mut st = status.lock();
                    st.state = "error".into();
                    st.error = format!("{e:#}");
                }
            }
        }
        if let Some(s) = seg.as_mut() {
            match s.write(&pcm) {
                Ok(()) => {
                    let mut st = status.lock();
                    st.secs = s.frames as f64 / SR as f64;
                    st.bytes = s.bytes;
                }
                Err(e) => {
                    seg = None;
                    retry_at = Some(Instant::now() + Duration::from_secs(10));
                    let mut st = status.lock();
                    st.state = "error".into();
                    st.error = format!("{e:#}");
                }
            }
        }
    }
    if let Some(s) = seg.take() {
        finish(s, status);
    }
    status.lock().state = "stopped".into();
}

fn finish(s: Segment, status: &Mutex<RecStatus>) {
    match s.close() {
        Ok(()) => status.lock().files_done += 1,
        Err(e) => status.lock().error = format!("{e:#}"),
    }
}

fn unix(t: NaiveDateTime) -> i64 {
    Local.from_local_datetime(&t).earliest().map(|d| d.timestamp()).unwrap_or_else(|| t.and_utc().timestamp())
}

/// The file that `now` falls in: `mins` long, lined up with midnight, never running into the next day.
pub fn segment_bounds(now: NaiveDateTime, mins: u32) -> (NaiveDateTime, NaiveDateTime) {
    use chrono::Timelike;
    let mins = mins.clamp(1, 1440);
    let day = now.date();
    let m = now.hour() * 60 + now.minute();
    let start_m = m / mins * mins;
    let start = day.and_time(NaiveTime::from_hms_opt(start_m / 60, start_m % 60, 0).unwrap_or_default());
    let end = (start + CDuration::minutes(mins as i64)).min(day.and_time(NaiveTime::MIN) + CDuration::days(1));
    (start, end)
}

/// Where the file for a segment starting at `start` goes: a folder per day, and a number added when
/// that name is taken (RadioMaster was restarted during the hour).
pub fn file_for(dir: &Path, start: NaiveDateTime, ext: &str) -> PathBuf {
    let day = dir.join(start.format("%Y-%m-%d").to_string());
    let base = start.format("%Y-%m-%d %H-%M").to_string();
    let mut p = day.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = day.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

/// The date of a recording made by RadioMaster, from its file name ("2026-11-01 14-00.mp3",
/// "2026-11-01 14-00 (2).wav"); None for any other file.
pub fn recording_date(name: &str) -> Option<NaiveDate> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !RecFormat::ALL.iter().any(|f| f.extension().eq_ignore_ascii_case(ext)) {
        return None;
    }
    let date = NaiveDate::parse_from_str(stem.get(..10)?, "%Y-%m-%d").ok()?;
    NaiveTime::parse_from_str(stem.get(11..16)?, "%H-%M").ok()?;
    if stem.as_bytes().get(10) != Some(&b' ') {
        return None;
    }
    Some(date)
}

/// Delete RadioMaster's recordings from before `before` (only files it named itself, in the day
/// folders it made), and day folders left empty.
fn delete_old(dir: &Path, before: NaiveDate) {
    let Ok(days) = std::fs::read_dir(dir) else { return };
    for d in days.flatten() {
        let name = d.file_name().to_string_lossy().to_string();
        let Ok(day) = NaiveDate::parse_from_str(&name, "%Y-%m-%d") else { continue };
        if day >= before || !d.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            if recording_date(&f.file_name().to_string_lossy()).is_some_and(|fd| fd < before) {
                let _ = std::fs::remove_file(f.path());
            }
        }
        let _ = std::fs::remove_dir(d.path()); // only goes when empty
    }
}

enum Out {
    Mp3(Box<mp3lame_encoder::Encoder>),
    Flac(PipeEncoder),
    Wav { last_header: Instant },
}

struct Segment {
    start: NaiveDateTime,
    path: PathBuf,
    file: BufWriter<File>,
    out: Out,
    frames: u64,
    bytes: u64,
    buf: Vec<u8>,
}

impl Segment {
    /// Open the file in the chosen folder, or in the data folder's "recordings" when the chosen one
    /// can't be written (a network folder that is offline). The note says when that happened.
    fn open(cfg: &RecordCfg, ffmpeg: &str, start: NaiveDateTime) -> Result<(Segment, String)> {
        let ext = cfg.format.extension();
        let chosen = cfg.folder_path();
        let (file, path, note) = match create(&file_for(&chosen, start, ext)) {
            Ok((f, p)) => (f, p, String::new()),
            Err(e) if !cfg.folder.trim().is_empty() => {
                let fallback = data_dir().join("recordings");
                let (f, p) = create(&file_for(&fallback, start, ext))?;
                (f, p, format!("Can't write to {} ({e:#}), so this hour is recorded in {}", chosen.display(), fallback.display()))
            }
            Err(e) => return Err(e),
        };
        let mut file = BufWriter::with_capacity(256 * 1024, file);
        let out = match cfg.format {
            RecFormat::Mp3 => Out::Mp3(Box::new(mp3_encoder(cfg.bitrate_kbps)?)),
            RecFormat::Flac => {
                let prog = find_ffmpeg(ffmpeg).ok_or_else(|| {
                    anyhow!("FLAC recordings need ffmpeg: put ffmpeg next to RadioMaster or choose it in Streams → ffmpeg")
                })?;
                let sr = SR.to_string();
                let args: Vec<String> = [
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "s16le",
                    "-ar",
                    &sr,
                    "-ac",
                    "2",
                    "-i",
                    "-",
                    "-c:a",
                    "flac",
                    "-f",
                    "flac",
                    "-",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
                Out::Flac(PipeEncoder::start(&prog.to_string_lossy(), &args)?)
            }
            RecFormat::Wav => {
                file.write_all(&wav_header(0))?;
                Out::Wav { last_header: Instant::now() }
            }
        };
        Ok((Segment { start, path, file, out, frames: 0, bytes: 0, buf: Vec::new() }, note))
    }

    fn write(&mut self, pcm: &[i16]) -> Result<()> {
        self.buf.clear();
        match &mut self.out {
            Out::Mp3(enc) => mp3_encode(enc, pcm, &mut self.buf)?,
            Out::Flac(p) => p.encode(pcm, &mut self.buf)?,
            Out::Wav { .. } => self.buf.extend(pcm.iter().flat_map(|s| s.to_le_bytes())),
        }
        self.file.write_all(&self.buf).with_context(|| format!("can't write {}", self.path.display()))?;
        self.bytes += self.buf.len() as u64;
        self.frames += pcm.len() as u64 / 2;
        // keep a WAV file's length right every few seconds, so it plays even if RadioMaster stops suddenly
        if let Out::Wav { last_header } = &mut self.out {
            if last_header.elapsed() > Duration::from_secs(10) {
                *last_header = Instant::now();
                fix_wav_header(&mut self.file, self.bytes)?;
            }
        }
        Ok(())
    }

    fn close(mut self) -> Result<()> {
        self.buf.clear();
        match &mut self.out {
            Out::Mp3(enc) => {
                self.buf.reserve(7200);
                let _ = enc.flush_to_vec::<mp3lame_encoder::FlushNoGap>(&mut self.buf);
            }
            Out::Flac(p) => p.finish(&mut self.buf, Duration::from_secs(10)),
            Out::Wav { .. } => {}
        }
        self.file.write_all(&self.buf)?;
        self.bytes += self.buf.len() as u64;
        if let Out::Wav { .. } = self.out {
            fix_wav_header(&mut self.file, self.bytes)?;
        }
        self.file.flush()?;
        if self.bytes == 0 {
            let _ = std::fs::remove_file(&self.path);
        }
        Ok(())
    }
}

fn create(p: &Path) -> Result<(File, PathBuf)> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("can't make the folder {}", d.display()))?;
    }
    let f = File::create(p).with_context(|| format!("can't make {}", p.display()))?;
    Ok((f, p.to_path_buf()))
}

/// A 44-byte header for 16-bit stereo PCM at `SR` with `data` bytes of sound.
fn wav_header(data: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&SR.to_le_bytes());
    h.extend_from_slice(&(SR * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data.to_le_bytes());
    h
}

fn fix_wav_header(file: &mut BufWriter<File>, data: u64) -> Result<()> {
    file.flush()?;
    let f = file.get_mut();
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&wav_header(data.min(u32::MAX as u64 - 36) as u32))?;
    f.seek(SeekFrom::End(0))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn files_line_up_with_the_clock() {
        assert_eq!(segment_bounds(at("2026-11-01 14:37:12"), 60), (at("2026-11-01 14:00:00"), at("2026-11-01 15:00:00")));
        assert_eq!(segment_bounds(at("2026-11-01 14:37:12"), 15), (at("2026-11-01 14:30:00"), at("2026-11-01 14:45:00")));
        assert_eq!(segment_bounds(at("2026-11-01 23:59:59"), 120), (at("2026-11-01 22:00:00"), at("2026-11-02 00:00:00")));
        assert_eq!(segment_bounds(at("2026-11-01 05:00:00"), 1440), (at("2026-11-01 00:00:00"), at("2026-11-02 00:00:00")));
    }

    #[test]
    fn segment_lengths_divide_the_day() {
        let mut c = RecordCfg { segment_mins: 50, ..Default::default() };
        assert_eq!(c.segment(), 30);
        c.segment_mins = 1440;
        assert_eq!(c.segment(), 1440);
        c.format = RecFormat::Wav;
        assert_eq!(c.segment(), 360, "a WAV file stays under 4 GB");
    }

    #[test]
    fn only_recordings_are_recognised() {
        assert_eq!(recording_date("2026-11-01 14-00.mp3"), NaiveDate::from_ymd_opt(2026, 11, 1));
        assert_eq!(recording_date("2026-11-01 14-00 (2).WAV"), NaiveDate::from_ymd_opt(2026, 11, 1));
        assert_eq!(recording_date("2026-11-01 14-00.txt"), None);
        assert_eq!(recording_date("my show.mp3"), None);
        assert_eq!(recording_date("2026-11-01_14-00.mp3"), None);
    }

    #[test]
    fn records_hourly_files_and_deletes_old_ones() {
        let dir = std::env::temp_dir().join(format!("rm-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = RecordCfg { enabled: true, folder: dir.display().to_string(), format: RecFormat::Wav, ..Default::default() };
        let start = at("2026-11-01 14:00:00");
        let (mut s, note) = Segment::open(&cfg, "", start).unwrap();
        assert!(note.is_empty());
        let pcm = vec![1000i16; 2048];
        for _ in 0..10 {
            s.write(&pcm).unwrap();
        }
        let path = s.path.clone();
        s.close().unwrap();
        assert_eq!(path, dir.join("2026-11-01").join("2026-11-01 14-00.wav"));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 10 * 2048 * 2);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 10 * 2048 * 2);
        // a restart during the hour makes a second file instead of overwriting the first
        assert_eq!(file_for(&dir, start, "wav"), dir.join("2026-11-01").join("2026-11-01 14-00 (2).wav"));

        // MP3
        let mp3 = RecordCfg { format: RecFormat::Mp3, bitrate_kbps: 64, ..cfg.clone() };
        let (mut s, _) = Segment::open(&mp3, "", at("2026-11-02 09:00:00")).unwrap();
        for _ in 0..50 {
            s.write(&pcm).unwrap();
        }
        let mp3_path = s.path.clone();
        s.close().unwrap();
        assert!(std::fs::metadata(&mp3_path).unwrap().len() > 1000);

        let other = dir.join("2026-11-01").join("notes.txt");
        std::fs::write(&other, "keep me").unwrap();
        delete_old(&dir, NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        assert!(!path.exists(), "the old recording goes");
        assert!(other.exists(), "other files stay");
        assert!(mp3_path.exists(), "newer recordings stay");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_flac_through_ffmpeg() {
        if find_ffmpeg("").is_none() {
            return; // no ffmpeg on this machine
        }
        let dir = std::env::temp_dir().join(format!("rm-rec-flac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = RecordCfg { enabled: true, folder: dir.display().to_string(), format: RecFormat::Flac, ..Default::default() };
        let (mut s, _) = Segment::open(&cfg, "", at("2026-11-01 08:00:00")).unwrap();
        let pcm: Vec<i16> = (0..2048).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
        for _ in 0..100 {
            s.write(&pcm).unwrap();
        }
        let path = s.path.clone();
        s.close().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
