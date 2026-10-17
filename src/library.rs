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
