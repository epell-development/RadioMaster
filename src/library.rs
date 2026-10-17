use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const EXTS: [&str; 9] = ["mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "mp4", "opus"];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Track {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub genre: String,
    pub year: String,
    pub comment: String,
    pub duration: f32,
}

impl Track {
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }

    /// Read tags + duration. Never fails: falls back to the file name.
    pub fn from_path(p: &Path) -> Track {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut t = Track { path: p.to_path_buf(), title: stem, ..Default::default() };
        if let Ok(probe) = Probe::open(p) {
            if let Ok(tf) = probe.read() {
                t.duration = tf.properties().duration().as_secs_f32();
                if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
                    if let Some(a) = tag.artist() {
                        t.artist = a.to_string();
                    }
                    if let Some(x) = tag.title() {
                        if !x.trim().is_empty() {
                            t.title = x.to_string();
                        }
                    }
                    if let Some(a) = tag.album() {
                        t.album = a.to_string();
                    }
                    if let Some(g) = tag.genre() {
                        t.genre = g.to_string();
                    }
                    if let Some(y) = tag.year() {
                        t.year = y.to_string();
                    }
                    if let Some(c) = tag.comment() {
                        t.comment = c.to_string();
                    }
                }
            }
        }
        t
    }
}

pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

pub fn audio_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_audio(e.path()))
        .map(|e| e.into_path())
        .collect();
    v.sort();
    v
}

pub fn write_tags(t: &Track) -> anyhow::Result<()> {
    let mut tf = Probe::open(&t.path)?.read()?;
    if tf.primary_tag().is_none() {
        let ty = tf.primary_tag_type();
        tf.insert_tag(Tag::new(ty));
    }
    let tag = tf.primary_tag_mut().ok_or_else(|| anyhow::anyhow!("format has no writable tag"))?;
    tag.set_artist(t.artist.clone());
    tag.set_title(t.title.clone());
    tag.set_album(t.album.clone());
    tag.set_genre(t.genre.clone());
    match t.year.trim().parse::<u32>() {
        Ok(y) => tag.set_year(y),
        Err(_) if t.year.trim().is_empty() => tag.remove_year(),
        Err(_) => {}
    }
    if t.comment.is_empty() {
        tag.remove_comment();
    } else {
        tag.set_comment(t.comment.clone());
    }
    tf.save_to_path(&t.path, WriteOptions::default())?;
    Ok(())
}

/// "MP3 · Stereo · 192 kbps · 44.1 kHz" style description of a file.
pub fn format_info(p: &Path) -> String {
    let Ok(tf) = Probe::open(p).and_then(|x| x.read()) else { return String::new() };
    let pr = tf.properties();
    let kind = p.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    let ch = match pr.channels() {
        Some(1) => "Mono".to_string(),
        Some(2) => "Stereo".to_string(),
        Some(n) => format!("{n} ch"),
        None => String::new(),
    };
    let br = pr.audio_bitrate().map(|b| format!("{b} kbps")).unwrap_or_default();
    let sr = pr.sample_rate().map(|r| format!("{:.1} kHz", r as f32 / 1000.0)).unwrap_or_default();
    [kind, ch, br, sr].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Embedded cover art, or cover/folder/front.jpg|png next to the file.
pub fn cover_bytes(p: &Path) -> Option<Vec<u8>> {
    if let Ok(tf) = Probe::open(p).and_then(|x| x.read()) {
        for tag in tf.tags() {
            if let Some(pic) =
                tag.pictures().iter().find(|x| x.pic_type() == lofty::picture::PictureType::CoverFront).or_else(|| tag.pictures().first())
            {
                return Some(pic.data().to_vec());
            }
        }
    }
    let dir = p.parent()?;
    for name in ["cover", "folder", "front", "Cover", "Folder", "Front", "album", "Album"] {
        for ext in ["jpg", "jpeg", "png"] {
            if let Ok(b) = std::fs::read(dir.join(format!("{name}.{ext}"))) {
                return Some(b);
            }
        }
    }
    None
}

// ------------------------------------------------------------------ fast checks for the UI
use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct MissingCache {
    seen: Mutex<HashMap<PathBuf, (bool, Instant)>>,
    tx: Mutex<mpsc::Sender<PathBuf>>,
}

fn missing_cache() -> &'static MissingCache {
    static CACHE: OnceLock<MissingCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        std::thread::spawn(move || {
            while let Ok(p) = rx.recv() {
                let missing = !p.exists();
                if let Some(c) = CACHE.get() {
                    c.seen.lock().unwrap().insert(p, (missing, Instant::now()));
                }
            }
        });
        MissingCache { seen: Mutex::new(HashMap::new()), tx: Mutex::new(tx) }
    })
}

/// Whether a playlist file is missing, without touching the disk on the UI thread: files are checked
/// in the background (slow on network / cloud drives) and re-checked every 30 s.
pub fn file_missing(p: &Path) -> bool {
    if p.as_os_str().is_empty() {
        return false;
    }
    let c = missing_cache();
    let mut seen = c.seen.lock().unwrap();
    match seen.get_mut(p) {
        Some((missing, at)) => {
            if at.elapsed() > Duration::from_secs(30) {
                *at = Instant::now(); // keep the old answer until the new check lands
                let _ = c.tx.lock().unwrap().send(p.to_path_buf());
            }
            *missing
        }
        None => {
            seen.insert(p.to_path_buf(), (false, Instant::now()));
            let _ = c.tx.lock().unwrap().send(p.to_path_buf());
            false
        }
    }
}

/// Drive letters for the file explorer (Windows), read once without probing each drive.
pub fn drive_roots() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            extern "system" {
                fn GetLogicalDrives() -> u32;
            }
            // SAFETY: no arguments; returns a bitmask of drive letters.
            let mask = unsafe { GetLogicalDrives() };
            (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    })
}
