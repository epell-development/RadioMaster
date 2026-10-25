//! Playlist rows. Besides audio files a row can be an internet stream, live line input, a command,
//! a timed pause, a teaser, a time announcement, a comment, a container, a track list, an hour marker
//! or an event.

use crate::library::Track;
use crate::scheduler::ActionKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug, Default)]
pub enum ItemKind {
    /// Audio file.
    #[default]
    Track,
    /// Audio file recorded/inserted as a voice track.
    VoiceTrack,
    /// Internet stream. `secs` = how long to play it (0 = until it ends or Next is pressed).
    Url { url: String, secs: f32 },
    /// Live input device. `secs` = how long (0 = until Next is pressed).
    LineIn { secs: f32 },
    /// Station command (see `Playout::run_command`).
    Command(String),
    /// Silence for `secs` seconds.
    Pause(f32),
    /// Snippets of the next `count` tracks, `secs` seconds each.
    Teaser { count: u32, secs: f32 },
    /// Speaks / plays the current time.
    TimeAnnounce,
    /// A note in the playlist; never played.
    Comment(String),
    /// A group of files played as one item: all in order, or one picked at random.
    Container { items: Vec<PathBuf>, random: bool },
    /// Plays one track picked at random from a folder or .m3u file (with repeat protection).
    TrackList { source: String },
    /// Syncs the playlist to the clock. `hard` = cut in exactly on time; `wait` = wait if early.
    HourMarker { hour: u32, minute: u32, hard: bool, wait: bool },
    /// Shows where the next hour starts; does nothing on air. `auto` rows are placed (and moved)
    /// by RadioMaster itself.
    HourInfo { hour: u32, auto: bool },
    /// Reads `text` with the computer's speech voice. `{time}`, `{next}`, `{last}` and friends are
    /// filled in when it plays.
    Tts {
        text: String,
        voice: String,
        rate: i32,
        #[serde(default)]
        opts: SpeechOpts,
    },
    /// Runs a scheduler action when playback reaches this row (put another playlist on air, play
    /// the playlist again, start an encoder…). `path` / `arg` mean what they mean for `Event`.
    Event { action: ActionKind, path: String, arg: f32 },
}

/// Rough length of a speech row. `files` also measures the intro/outro sounds (reads the files).
pub fn speech_secs(text: &str, rate: i32, opts: &SpeechOpts, files: bool) -> f32 {
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count().max(1);
    let one = (text.split_whitespace().count() as f32 / (2.6 * (1.0 + rate as f32 * 0.08)).max(0.8)).max(1.5)
        + (lines - 1) as f32 * opts.line_pause_ms as f32 / 1000.0;
    let n = opts.repeat.max(1) as f32;
    let file_len = |f: &str| if !files || f.trim().is_empty() { 0.0 } else { Track::from_path(Path::new(f)).duration };
    one * n + (n - 1.0) * opts.repeat_pause_ms as f32 / 1000.0 + file_len(&opts.intro) + file_len(&opts.outro)
}

/// Extra speech settings for a text to speech row.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(default)]
pub struct SpeechOpts {
    /// -10 (lower) … 10 (higher)
    pub pitch: i32,
    /// 0 … 200 %
    pub volume: u32,
    /// silence between lines (ms)
    pub line_pause_ms: u32,
    /// how many times the text is read
    pub repeat: u32,
    /// silence between repeats (ms)
    pub repeat_pause_ms: u32,
    /// sound played before the speech (empty = none)
    pub intro: String,
    /// sound played after the speech (empty = none)
    pub outro: String,
    /// spell out numbers like "1999" as digits ("one nine nine nine")
    pub spell_numbers: bool,
}
impl Default for SpeechOpts {
    fn default() -> Self {
        Self {
            pitch: 0,
            volume: 100,
            line_pause_ms: 500,
            repeat: 1,
            repeat_pause_ms: 800,
            intro: String::new(),
            outro: String::new(),
            spell_numbers: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PlEntry {
    /// For files: the file and its tags. For other kinds `track.title` is the display name
    /// and `track.duration` the expected length.
    pub track: Track,
    pub played: bool,
    pub kind: ItemKind,
}

impl PlEntry {
    pub fn file(track: Track) -> Self {
        PlEntry { track, played: false, kind: ItemKind::Track }
    }
    pub fn special(kind: ItemKind, title: impl Into<String>, secs: f32) -> Self {
        let track = Track { title: title.into(), duration: secs, ..Default::default() };
        PlEntry { track, played: false, kind }
    }

    /// A row that plays an audio file directly.
    pub fn is_file(&self) -> bool {
        matches!(self.kind, ItemKind::Track | ItemKind::VoiceTrack)
    }

    /// A fresh copy (not yet played).
    pub fn fresh(&self) -> Self {
        PlEntry { played: false, ..self.clone() }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            ItemKind::Track | ItemKind::VoiceTrack => self.track.label(),
            ItemKind::Url { url, .. } => {
                if self.track.title.is_empty() {
                    url.clone()
                } else {
                    self.track.title.clone()
                }
            }
            ItemKind::LineIn { secs } => {
                if *secs > 0.0 {
                    format!("Line input ({})", mmss(*secs))
                } else {
                    "Line input (until Next)".into()
                }
            }
            ItemKind::Command(c) => format!("Command: {c}"),
            ItemKind::Pause(s) => format!("Timed pause {}", mmss(*s)),
            ItemKind::Teaser { count, secs } => format!("Teaser: next {count} tracks, {secs:.0}s each"),
            ItemKind::TimeAnnounce => "Time announcement".into(),
            ItemKind::Comment(t) => t.clone(),
            ItemKind::Container { items, random } => {
                let name = if self.track.title.is_empty() { "Container".to_string() } else { self.track.title.clone() };
                format!("{name} ({} {})", items.len(), if *random { "tracks, one at random" } else { "tracks" })
            }
            ItemKind::TrackList { source } => {
                let name = if self.track.title.is_empty() {
                    Path::new(source).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| source.clone())
                } else {
                    self.track.title.clone()
                };
                format!("Track list: {name}")
            }
            ItemKind::HourMarker { hour, minute, hard, .. } => {
                format!("Hour Marker {hour}:{minute:02}{}", if *hard { " (hard)" } else { "" })
            }
            ItemKind::HourInfo { hour, .. } => format!("{hour:02}:00 — new hour"),
            ItemKind::Tts { text, .. } => {
                if self.track.title.is_empty() {
                    format!("Speak: {text}")
                } else {
                    self.track.title.clone()
                }
            }
            ItemKind::Event { action, path, .. } => {
                if !self.track.title.is_empty() {
                    format!("Event: {}", self.track.title)
                } else if path.trim().is_empty() || *action == ActionKind::RunRule {
                    format!("Event: {}", action.label())
                } else {
                    let p = path.trim();
                    let name = Path::new(p).file_name().filter(|_| action.needs_path()).map(|n| n.to_string_lossy().to_string());
                    format!("Event: {} — {}", action.label(), name.unwrap_or_else(|| p.to_string()))
                }
            }
        }
    }

    /// Short type label for the playlist.
    pub fn type_label(&self) -> &'static str {
        match self.kind {
            ItemKind::Track => "Track",
            ItemKind::VoiceTrack => "Voice track",
            ItemKind::Url { .. } => "Stream",
            ItemKind::LineIn { .. } => "Line input",
            ItemKind::Command(_) => "Command",
            ItemKind::Pause(_) => "Pause",
            ItemKind::Teaser { .. } => "Teaser",
            ItemKind::TimeAnnounce => "Time",
            ItemKind::Comment(_) => "Comment",
            ItemKind::Container { .. } => "Container",
            ItemKind::TrackList { .. } => "Track list",
            ItemKind::HourMarker { .. } => "Hour marker",
            ItemKind::HourInfo { .. } => "Hour",
            ItemKind::Tts { .. } => "Speech",
            ItemKind::Event { .. } => "Event",
        }
    }

    /// Expected length in seconds for start-time estimates (0 = takes no time).
    pub fn est_secs(&self) -> f32 {
        match &self.kind {
            ItemKind::Track | ItemKind::VoiceTrack | ItemKind::Container { .. } | ItemKind::TrackList { .. } => self.track.duration,
            ItemKind::Url { secs, .. } | ItemKind::LineIn { secs } => *secs,
            ItemKind::Pause(s) => *s,
            ItemKind::Teaser { count, secs } => *count as f32 * secs,
            ItemKind::TimeAnnounce => 4.0,
            ItemKind::Tts { text, rate, opts, .. } => {
                if self.track.duration > 0.0 {
                    self.track.duration
                } else {
                    speech_secs(text, *rate, opts, false)
                }
            }
            ItemKind::Command(_)
            | ItemKind::Comment(_)
            | ItemKind::HourMarker { .. }
            | ItemKind::HourInfo { .. }
            | ItemKind::Event { .. } => 0.0,
        }
    }

    /// Length as shown in the playlist.
    pub fn length_text(&self) -> String {
        match &self.kind {
            ItemKind::Comment(_)
            | ItemKind::Command(_)
            | ItemKind::HourMarker { .. }
            | ItemKind::HourInfo { .. }
            | ItemKind::Event { .. } => String::new(),
            ItemKind::Url { secs, .. } | ItemKind::LineIn { secs } if *secs <= 0.0 => "live".into(),
            _ if self.est_secs() <= 0.0 => "—:—".into(),
            _ => mmss(self.est_secs()),
        }
    }

    /// One or more M3U lines. Non-file rows are written as `#RADIOMASTER:` comments so other players skip them.
    pub fn to_m3u(&self) -> String {
        match self.kind {
            ItemKind::Track => format!("#EXTINF:{},{}\n{}\n", self.track.duration as i64, self.track.label(), self.track.path.display()),
            ItemKind::VoiceTrack => format!(
                "#RADIOMASTER-VOICE\n#EXTINF:{},{}\n{}\n",
                self.track.duration as i64,
                self.track.label(),
                self.track.path.display()
            ),
            _ => {
                let rec = serde_json::json!({ "kind": self.kind, "title": self.track.title, "secs": self.track.duration });
                format!("#RADIOMASTER:{rec}\n")
            }
        }
    }
}

/// Parse a `#RADIOMASTER:` line back into a row.
pub fn from_m3u_comment(line: &str) -> Option<PlEntry> {
    let json = line.strip_prefix("#RADIOMASTER:")?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let kind: ItemKind = serde_json::from_value(v.get("kind")?.clone()).ok()?;
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let secs = v.get("secs").and_then(|t| t.as_f64()).unwrap_or(0.0) as f32;
    Some(PlEntry::special(kind, title, secs))
}

pub fn mmss(secs: f32) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_rows_round_trip_through_m3u() {
        let rows = vec![
            PlEntry::special(ItemKind::HourMarker { hour: 19, minute: 0, hard: true, wait: true }, "", 0.0),
            PlEntry::special(ItemKind::Url { url: "http://example.com/live".into(), secs: 600.0 }, "News feed", 600.0),
            PlEntry::special(
                ItemKind::Container { items: vec![PathBuf::from("/a.mp3"), PathBuf::from("/b.mp3")], random: true },
                "IDs",
                12.0,
            ),
            PlEntry::special(ItemKind::Comment("Talk break here".into()), "", 0.0),
            PlEntry::special(ItemKind::Event { action: ActionKind::SwitchPlaylist, path: "Main".into(), arg: 0.0 }, "", 0.0),
        ];
        for r in rows {
            let line = r.to_m3u();
            let back = from_m3u_comment(line.trim()).expect("parses");
            assert_eq!(back.kind, r.kind);
            assert_eq!(back.track.title, r.track.title);
        }
    }

    #[test]
    fn titles_and_lengths() {
        let m = PlEntry::special(ItemKind::HourMarker { hour: 1, minute: 0, hard: false, wait: true }, "", 0.0);
        assert_eq!(m.title(), "Hour Marker 1:00");
        assert_eq!(m.length_text(), "");
        let live = PlEntry::special(ItemKind::LineIn { secs: 0.0 }, "", 0.0);
        assert_eq!(live.length_text(), "live");
        let p = PlEntry::special(ItemKind::Pause(90.0), "", 90.0);
        assert_eq!(p.est_secs(), 90.0);
    }
}
