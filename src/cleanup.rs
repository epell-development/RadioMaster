//! Library cleanup: duplicate files, missing ("lost") and unreadable tracks, and tidying genre and
//! artist tags. The scans and the file / tag changes run on a background thread and report back
//! through a channel; nothing here touches the interface. Every change is listed in
//! `cleanup-log.csv` in the data folder so it can be undone by hand.

use crate::config::data_dir;
use crate::i18n::tr;
use crate::library::{self, Track};
use lofty::prelude::*;
use lofty::probe::Probe;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hasher;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

// ------------------------------------------------------------------ text keys

/// Trimmed, with runs of spaces collapsed to one.
pub fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lowercase letters and digits only, with "&" / "+" read as "and", so "Hip-Hop", "hip hop" and
/// "HipHop" share a key. A few common spellings of the same genre are folded together.
pub fn loose_key(s: &str) -> String {
    let s = s.to_lowercase().replace(['&', '+'], " and ");
    let k: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    match k.as_str() {
        "rnb" | "rhythmandblues" => "randb".into(),
        "dnb" | "drumnbass" => "drumandbass".into(),
        "rocknroll" => "rockandroll".into(),
        _ => k,
    }
}

/// An artist written the usual way: tidied, "Beatles, The" → "The Beatles", and "ft." /
/// "featuring" / "Feat" all written "feat.".
pub fn normal_artist(s: &str) -> String {
    let mut s = tidy(s);
    for (suffix, prefix) in [(", the", "The "), (", a", "A ")] {
        let cut = s.len().saturating_sub(suffix.len());
        if cut > 0 && s.is_char_boundary(cut) && s[cut..].eq_ignore_ascii_case(suffix) {
            s = format!("{prefix}{}", &s[..cut]);
        }
    }
    let words: Vec<String> = s
        .split(' ')
        .enumerate()
        .map(|(i, w)| match w.to_lowercase().as_str() {
            "ft." | "ft" | "feat" | "feat." | "featuring" if i > 0 => "feat.".to_string(),
            "(ft." | "(ft" | "(feat" | "(feat." | "(featuring" if i > 0 => "(feat.".to_string(),
            _ => w.to_string(),
        })
        .collect();
    words.join(" ")
}

/// Key for grouping spellings of one artist ("the" in front doesn't count).
pub fn artist_key(s: &str) -> String {
    let n = normal_artist(s).to_lowercase();
    let n = n.strip_prefix("the ").unwrap_or(&n);
    loose_key(n)
}

/// One way to spell a value, how many tracks use it.
pub type Count = (String, usize);

/// Every distinct value of a tag in the library, with how many tracks have it (empty ones left out).
pub fn counts(lib: &[Track], field: fn(&Track) -> &str) -> Vec<Count> {
    let mut m: HashMap<&str, usize> = HashMap::new();
    for t in lib {
        let v = field(t);
        if !v.trim().is_empty() {
            *m.entry(v).or_default() += 1;
        }
    }
    let mut v: Vec<Count> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.0.cmp(&b.0)));
    v
}

/// Spellings that should become one value.
#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    /// the spellings to change, with their track counts (may include `to` itself)
    pub from: Vec<Count>,
    pub to: String,
}

impl Merge {
    /// Tracks whose tag would actually change.
    pub fn changes(&self) -> usize {
        self.from.iter().filter(|(s, _)| *s != self.to).map(|(_, n)| n).sum()
    }
}

/// How nicely a spelling is written: mixed case beats ALL CAPS or all lower case.
fn nice(s: &str) -> u8 {
    let up = s.chars().any(|c| c.is_uppercase());
    let low = s.chars().any(|c| c.is_lowercase());
    u8::from(up && low) * 2 + u8::from(up)
}

/// Group spellings by `key`; suggest the most used one (tidied with `fix`) for every group that
/// has more than one spelling, or one that `fix` would change.
pub fn suggest(values: &[Count], key: fn(&str) -> String, fix: fn(&str) -> String) -> Vec<Merge> {
    let mut groups: BTreeMap<String, Vec<Count>> = BTreeMap::new();
    for (s, n) in values {
        let k = key(s);
        if !k.is_empty() {
            groups.entry(k).or_default().push((s.clone(), *n));
        }
    }
    let mut out = vec![];
    for (_, mut g) in groups {
        g.sort_by(|a, b| b.1.cmp(&a.1).then(nice(&b.0).cmp(&nice(&a.0))).then(a.0.cmp(&b.0)));
        let to = fix(&g[0].0);
        if g.len() > 1 || g[0].0 != to {
            out.push(Merge { from: g, to });
        }
    }
    out
}

pub fn genre_suggestions(values: &[Count]) -> Vec<Merge> {
    suggest(values, loose_key, tidy)
}

pub fn artist_suggestions(values: &[Count]) -> Vec<Merge> {
    suggest(values, artist_key, normal_artist)
}

/// "Artist - Title" read from a title when the artist tag is empty (untagged files are listed by
/// their file name).
pub fn split_artist_title(t: &Track) -> Option<(String, String)> {
    if !t.artist.trim().is_empty() {
        return None;
    }
    let (a, b) = t.title.split_once(" - ")?;
    let (a, b) = (tidy(a), tidy(b));
    (!a.is_empty() && !b.is_empty() && !a.chars().all(|c| c.is_ascii_digit())).then_some((a, b))
}

// ------------------------------------------------------------------ duplicates

/// Library indexes of tracks that look like the same song: same artist and title (loosely), and
/// lengths within `tolerance` seconds of the first one (an unknown length matches anything).
pub fn song_groups(lib: &[Track], tolerance: f32) -> Vec<Vec<usize>> {
    let mut by: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, t) in lib.iter().enumerate() {
        let title = loose_key(&t.title);
        if title.is_empty() {
            continue;
        }
        by.entry((artist_key(&t.artist), title)).or_default().push(i);
    }
    let mut out = vec![];
    for (_, mut v) in by {
        if v.len() < 2 {
            continue;
        }
        v.sort_by(|&a, &b| lib[a].duration.total_cmp(&lib[b].duration));
        // split where the length jumps by more than the tolerance (radio edit vs. album version)
        let mut cur: Vec<usize> = vec![];
        for i in v {
            let d = lib[i].duration;
            let close = cur.last().is_none_or(|&j: &usize| d <= 0.0 || lib[j].duration <= 0.0 || d - lib[j].duration <= tolerance);
            if !close {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            cur.push(i);
        }
        if cur.len() > 1 {
            out.push(cur);
        }
    }
    out.sort();
    out
}

/// One copy in a group of duplicates.
#[derive(Clone, Debug)]
pub struct Dup {
    pub path: PathBuf,
    pub label: String,
    /// "MP3 · 320 kbps · 3:41 · 8.4 MB"
    pub info: String,
    /// higher = better copy to keep
    pub score: u64,
    /// byte-for-byte the same as another copy in the group
    pub identical: bool,
}

#[derive(Clone, Debug)]
pub struct DupGroup {
    pub copies: Vec<Dup>,
    /// the copy to keep (index into `copies`)
    pub keep: usize,
}

const LOSSLESS: [&str; 2] = ["flac", "wav"];

fn mmss(t: f32) -> String {
    let t = t.max(0.0) as u32;
    format!("{}:{:02}", t / 60, t % 60)
}

/// Content hash of a file (only compared within one scan).
fn file_hash(p: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.write(&buf[..n]);
    }
    Some(h.finish())
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        r = parent[r];
    }
    parent[i] = r;
    r
}

pub enum Msg {
    Progress(String, usize, usize),
    Dups(Vec<DupGroup>),
    Health(Health),
    Removed(Vec<(PathBuf, Result<Option<PathBuf>, String>)>),
    Written(Vec<(PathBuf, Result<Track, String>)>),
    /// (old path, the file it was found as)
    Relinked(Vec<(PathBuf, Track)>),
}

/// Find duplicate files: byte-identical copies always, and (with `by_song`) different files of
/// the same song.
pub fn scan_duplicates(lib: Vec<Track>, by_song: bool, tolerance: f32, tx: Sender<Msg>, wake: impl Fn()) {
    let n = lib.len();
    let say = |what: &str, i: usize, of: usize| {
        let _ = tx.send(Msg::Progress(what.to_string(), i, of));
        wake();
    };
    let mut sizes: Vec<Option<u64>> = Vec::with_capacity(n);
    for (i, t) in lib.iter().enumerate() {
        sizes.push(std::fs::metadata(&t.path).ok().filter(|m| m.is_file()).map(|m| m.len()));
        if i % 200 == 0 {
            say("Checking files", i, n);
        }
    }
    let mut parent: Vec<usize> = (0..n).collect();
    let mut same_as: HashSet<usize> = HashSet::new();
    // byte-identical: only files of exactly the same size are read
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, s) in sizes.iter().enumerate() {
        if let Some(s) = s.filter(|&s| s > 0) {
            by_size.entry(s).or_default().push(i);
        }
    }
    let candidates: Vec<Vec<usize>> = by_size.into_values().filter(|v| v.len() > 1).collect();
    let total: usize = candidates.iter().map(Vec::len).sum();
    let mut done = 0;
    for v in candidates {
        let mut by_hash: HashMap<u64, usize> = HashMap::new();
        for i in v {
            done += 1;
            say("Comparing files", done, total);
            let Some(h) = file_hash(&lib[i].path) else { continue };
            match by_hash.get(&h) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a] = b;
                    same_as.insert(i);
                    same_as.insert(j);
                }
                None => {
                    by_hash.insert(h, i);
                }
            }
        }
    }
    if by_song {
        for g in song_groups(&lib, tolerance) {
            for w in g.windows(2) {
                if sizes[w[0]].is_some() && sizes[w[1]].is_some() {
                    let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                    parent[a] = b;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, size) in sizes.iter().enumerate() {
        let r = find(&mut parent, i);
        if size.is_some() {
            groups.entry(r).or_default().push(i);
        }
    }
    let groups: Vec<Vec<usize>> = groups.into_values().filter(|v| v.len() > 1).collect();
    let total = groups.len();
    let mut out = vec![];
    for (gi, g) in groups.into_iter().enumerate() {
        say("Comparing quality", gi, total);
        let mut copies: Vec<Dup> = g
            .iter()
            .map(|&i| {
                let t = &lib[i];
                let size = sizes[i].unwrap_or(0);
                let ext = t.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let kbps = Probe::open(&t.path).and_then(|p| p.read()).ok().and_then(|f| f.properties().audio_bitrate()).unwrap_or(0);
                let tags = [&t.artist, &t.title, &t.album, &t.genre, &t.year].iter().filter(|x| !x.trim().is_empty()).count() as u64;
                let lossless = LOSSLESS.contains(&ext.as_str());
                // lossless, then bitrate, then full length, then more tags, then a shorter path
                let score = (u64::from(lossless) << 60)
                    | ((kbps.min(9999) as u64) << 44)
                    | (((t.duration.max(0.0) as u64).min(65535)) << 28)
                    | (tags << 24)
                    | (0xFF_FFFF - (t.path.as_os_str().len() as u64).min(0xFF_FFFF));
                let mut info = vec![ext.to_uppercase()];
                if kbps > 0 {
                    info.push(format!("{kbps} kbps"));
                }
                info.push(mmss(t.duration));
                info.push(format!("{:.1} MB", size as f64 / 1_048_576.0));
                Dup { path: t.path.clone(), label: t.label(), info: info.join(" · "), score, identical: same_as.contains(&i) }
            })
            .collect();
        copies.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
        out.push(DupGroup { copies, keep: 0 });
    }
    out.sort_by(|a, b| a.copies[0].label.to_lowercase().cmp(&b.copies[0].label.to_lowercase()));
    let _ = tx.send(Msg::Dups(out));
    wake();
}

// ------------------------------------------------------------------ missing and unreadable files

/// A track whose file isn't where the library or a playlist expects it.
#[derive(Clone, Debug)]
pub struct Lost {
    pub path: PathBuf,
    pub label: String,
    pub in_library: bool,
    /// playlist rows (all tabs) that use it
    pub rows: usize,
    /// files that look like it, best first
    pub found: Vec<PathBuf>,
}

/// A file that is there but can't be played.
#[derive(Clone, Debug)]
pub struct Broken {
    pub path: PathBuf,
    pub label: String,
    pub why: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Health {
    pub lost: Vec<Lost>,
    pub broken: Vec<Broken>,
    /// files checked
    pub checked: usize,
}

fn name_key(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn stem_key(p: &Path) -> String {
    p.file_stem().map(|n| loose_key(&n.to_string_lossy())).unwrap_or_default()
}

/// Rank found files for a lost one: same folder name first, then the shortest path.
fn rank(lost: &Path, found: &mut [PathBuf]) {
    let dir = |p: &Path| p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_lowercase());
    let want = dir(lost);
    found.sort_by_key(|p| (dir(p) != want, p.as_os_str().len(), p.clone()));
}

/// Check every library track and playlist row (`rows`: path + label per row). Lost files are
/// looked for in `folders` by file name, then by a loose file name, then by artist and title.
pub fn scan_health(lib: Vec<Track>, rows: Vec<(PathBuf, String)>, folders: Vec<PathBuf>, tx: Sender<Msg>, wake: impl Fn()) {
    let say = |what: &str, i: usize, of: usize| {
        let _ = tx.send(Msg::Progress(what.to_string(), i, of));
        wake();
    };
    // every file to check, once
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    let mut order: Vec<(PathBuf, String, bool, usize, f32)> = vec![];
    for t in &lib {
        if !seen.contains_key(&t.path) {
            seen.insert(t.path.clone(), order.len());
            order.push((t.path.clone(), t.label(), true, 0, t.duration));
        }
    }
    for (p, label) in rows {
        if p.as_os_str().is_empty() {
            continue;
        }
        match seen.get(&p) {
            Some(&i) => order[i].3 += 1,
            None => {
                seen.insert(p.clone(), order.len());
                order.push((p, label, false, 1, -1.0));
            }
        }
    }
    let n = order.len();
    let mut lost_idx = vec![];
    let mut broken = vec![];
    for (i, (p, label, in_lib, _, dur)) in order.iter().enumerate() {
        if i % 100 == 0 {
            say("Checking files", i, n);
        }
        match std::fs::metadata(p) {
            Err(_) => lost_idx.push(i),
            Ok(m) if !m.is_file() => lost_idx.push(i),
            Ok(m) if m.len() == 0 => broken.push(Broken { path: p.clone(), label: label.clone(), why: tr("Empty file (0 bytes)") }),
            Ok(_) if !*in_lib || *dur <= 0.0 => {
                // the library scan already read every other file
                match Probe::open(p).ok().and_then(|x| x.guess_file_type().ok()).and_then(|x| x.read().ok()) {
                    None => broken.push(Broken { path: p.clone(), label: label.clone(), why: tr("Can't be read (damaged or not audio)") }),
                    Some(f) if f.properties().duration().as_secs_f32() <= 0.0 => {
                        broken.push(Broken { path: p.clone(), label: label.clone(), why: tr("No audio length") })
                    }
                    Some(_) => {}
                }
            }
            Ok(_) => {}
        }
    }
    let mut lost = vec![];
    if !lost_idx.is_empty() {
        // everything in the search folders, by name, loose name, and (for library files) song
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut files = 0;
        for f in &folders {
            for p in library::audio_files_in(f) {
                files += 1;
                if files % 500 == 0 {
                    say("Looking for lost files", files, 0);
                }
                by_name.entry(name_key(&p)).or_default().push(p.clone());
                by_stem.entry(stem_key(&p)).or_default().push(p);
            }
        }
        let mut by_song: HashMap<(String, String), Vec<PathBuf>> = HashMap::new();
        for t in &lib {
            if !t.title.trim().is_empty() {
                by_song.entry((artist_key(&t.artist), loose_key(&t.title))).or_default().push(t.path.clone());
            }
        }
        let missing: HashSet<&PathBuf> = lost_idx.iter().map(|&i| &order[i].0).collect();
        for i in lost_idx {
            let (p, label, in_lib, rows, _) = &order[i];
            let mut found = by_name.get(&name_key(p)).cloned().unwrap_or_default();
            if found.is_empty() {
                found = by_stem.get(&stem_key(p)).cloned().unwrap_or_default();
            }
            if found.is_empty() {
                if let Some(t) = lib.iter().find(|t| &t.path == p) {
                    found = by_song.get(&(artist_key(&t.artist), loose_key(&t.title))).cloned().unwrap_or_default();
                }
            }
            found.retain(|f| !missing.contains(f) && f != p);
            found.dedup();
            rank(p, &mut found);
            lost.push(Lost { path: p.clone(), label: label.clone(), in_library: *in_lib, rows: *rows, found });
        }
    }
    lost.sort_by_key(|a| a.label.to_lowercase());
    broken.sort_by_key(|a| a.label.to_lowercase());
    let _ = tx.send(Msg::Health(Health { lost, broken, checked: n }));
    wake();
}

// ------------------------------------------------------------------ changes

/// The folder removed files are moved to (outside the library folders, so a rescan won't bring
/// them back).
pub fn removed_folder() -> PathBuf {
    data_dir().join("Removed tracks")
}

fn log(lines: &[[String; 4]]) {
    if lines.is_empty() {
        return;
    }
    let path = data_dir().join("cleanup-log.csv");
    let new = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    if new {
        let _ = writeln!(f, "time,action,file,detail");
    }
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for [a, b, c, d] in lines {
        let _ = writeln!(f, "{},{},{},{}", if a.is_empty() { &now } else { a }, q(b), q(c), q(d));
    }
}

/// A free name for `name` inside `dir` ("song (2).mp3" if "song.mp3" is taken).
fn free_name(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let n = Path::new(name);
    let stem = n.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = n.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).unwrap()
}

/// Move a file, copying when it is on another drive.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Move files to `removed_folder()` (or delete them for good with `delete`). Each result is the
/// file's new place (None when deleted) or why it failed.
pub fn remove_files(paths: Vec<PathBuf>, delete: bool, tx: Sender<Msg>, wake: impl Fn()) {
    let dir = removed_folder();
    let mut out = vec![];
    let mut lines = vec![];
    let n = paths.len();
    for (i, p) in paths.into_iter().enumerate() {
        let _ = tx.send(Msg::Progress("Removing files".into(), i, n));
        wake();
        let r = if delete {
            std::fs::remove_file(&p).map(|_| None).map_err(|e| e.to_string())
        } else {
            std::fs::create_dir_all(&dir)
                .and_then(|_| {
                    let to = free_name(&dir, p.file_name().unwrap_or_default());
                    move_file(&p, &to).map(|_| to)
                })
                .map(Some)
                .map_err(|e| e.to_string())
        };
        match &r {
            Ok(Some(to)) => lines.push([String::new(), "moved".into(), p.display().to_string(), to.display().to_string()]),
            Ok(None) => lines.push([String::new(), "deleted".into(), p.display().to_string(), String::new()]),
            Err(_) => {}
        }
        out.push((p, r));
    }
    log(&lines);
    let _ = tx.send(Msg::Removed(out));
    wake();
}

/// Read the tags of found files so lost tracks can be pointed at them.
pub fn relink(pairs: Vec<(PathBuf, PathBuf)>, tx: Sender<Msg>, wake: impl Fn()) {
    let lines: Vec<[String; 4]> =
        pairs.iter().map(|(a, b)| [String::new(), "relinked".into(), a.display().to_string(), b.display().to_string()]).collect();
    let out = pairs.into_iter().filter(|(_, to)| to.is_file()).map(|(from, to)| (from, Track::from_path(&to))).collect();
    log(&lines);
    let _ = tx.send(Msg::Relinked(out));
    wake();
}

/// A tag change on one file.
#[derive(Clone, Debug, PartialEq)]
pub enum TagFix {
    Genre(String),
    Artist(String),
    ArtistTitle(String, String),
}

/// Write tag changes. Each file's tags are read fresh so only the one field changes; the result
/// is the file's new tags.
pub fn write_tags(fixes: Vec<(PathBuf, TagFix)>, tx: Sender<Msg>, wake: impl Fn()) {
    let n = fixes.len();
    let mut out = vec![];
    let mut lines = vec![];
    for (i, (p, fix)) in fixes.into_iter().enumerate() {
        if i % 10 == 0 {
            let _ = tx.send(Msg::Progress("Writing tags".into(), i, n));
            wake();
        }
        if !p.exists() {
            out.push((p, Err("file not found".into())));
            continue;
        }
        let mut t = Track::from_path(&p);
        let (field, old) = match &fix {
            TagFix::Genre(g) => ("genre", std::mem::replace(&mut t.genre, g.clone())),
            TagFix::Artist(a) => ("artist", std::mem::replace(&mut t.artist, a.clone())),
            TagFix::ArtistTitle(a, b) => {
                let old = format!("{} | {}", t.artist, t.title);
                t.artist = a.clone();
                t.title = b.clone();
                ("artist+title", old)
            }
        };
        let r = library::write_tags(&t).map(|_| t).map_err(|e| e.to_string());
        if let Ok(t) = &r {
            let new = match fix {
                TagFix::Genre(_) => t.genre.clone(),
                TagFix::Artist(_) => t.artist.clone(),
                TagFix::ArtistTitle(..) => format!("{} | {}", t.artist, t.title),
            };
            lines.push([String::new(), format!("tag {field}"), p.display().to_string(), format!("{old} → {new}")]);
        }
        out.push((p, r));
    }
    log(&lines);
    let _ = tx.send(Msg::Written(out));
    wake();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trk(artist: &str, title: &str, dur: f32) -> Track {
        Track {
            path: PathBuf::from(format!("/m/{artist}-{title}-{dur}.mp3")),
            artist: artist.into(),
            title: title.into(),
            duration: dur,
            ..Default::default()
        }
    }

    #[test]
    fn genre_keys() {
        assert_eq!(loose_key("Hip-Hop"), loose_key("hip hop"));
        assert_eq!(loose_key("R&B"), loose_key("RnB"));
        assert_eq!(loose_key("Drum & Bass"), loose_key("drum n bass"));
        assert_ne!(loose_key("Pop"), loose_key("Pop Rock"));
    }

    #[test]
    fn genre_merge_picks_most_used() {
        let v = vec![("Hip-Hop".to_string(), 10), ("hip hop".to_string(), 2), ("Rock".to_string(), 5), (" Jazz ".to_string(), 1)];
        let s = genre_suggestions(&v);
        assert_eq!(s.len(), 2);
        let hh = s.iter().find(|m| m.to == "Hip-Hop").unwrap();
        assert_eq!(hh.changes(), 2);
        assert!(s.iter().any(|m| m.to == "Jazz" && m.changes() == 1));
    }

    #[test]
    fn artists() {
        assert_eq!(normal_artist("Beatles, The"), "The Beatles");
        assert_eq!(normal_artist("  Drake  ft.  Rihanna "), "Drake feat. Rihanna");
        assert_eq!(normal_artist("Calvin Harris Featuring Ellie"), "Calvin Harris feat. Ellie");
        assert_eq!(artist_key("The Beatles"), artist_key("beatles"));
        assert_eq!(artist_key("Beatles, The"), artist_key("THE BEATLES"));
        let v = vec![("The Beatles".to_string(), 8), ("BEATLES, THE".to_string(), 1), ("Adele".to_string(), 3)];
        let s = artist_suggestions(&v);
        assert_eq!(s, vec![Merge { from: v[..2].to_vec(), to: "The Beatles".into() }]);
    }

    #[test]
    fn split_titles() {
        assert_eq!(split_artist_title(&trk("", "Adele - Hello", 0.0)), Some(("Adele".into(), "Hello".into())));
        assert_eq!(split_artist_title(&trk("Adele", "Adele - Hello", 0.0)), None);
        assert_eq!(split_artist_title(&trk("", "01 - Hello", 0.0)), None);
        assert_eq!(split_artist_title(&trk("", "Hello", 0.0)), None);
    }

    #[test]
    fn songs() {
        let lib = vec![
            trk("Adele", "Hello", 295.0),
            trk("ADELE", "hello", 296.5),
            trk("Adele", "Hello", 240.0), // radio edit: a different version
            trk("Adele", "Skyfall", 286.0),
            trk("The Beatles", "Help!", 138.0),
            trk("Beatles", "Help", 0.0),
        ];
        assert_eq!(song_groups(&lib, 3.0), vec![vec![0, 1], vec![5, 4]]);
    }

    #[test]
    fn identical_files_and_moves() {
        let dir = std::env::temp_dir().join(format!("rm-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.mp3");
        let b = dir.join("b.mp3");
        std::fs::write(&a, b"same bytes").unwrap();
        std::fs::write(&b, b"same bytes").unwrap();
        assert_eq!(file_hash(&a), file_hash(&b));
        let (tx, rx) = std::sync::mpsc::channel();
        let lib = vec![
            Track { path: a.clone(), title: "x".into(), ..Default::default() },
            Track { path: b.clone(), title: "y".into(), ..Default::default() },
        ];
        scan_duplicates(lib, false, 3.0, tx, || {});
        let groups = rx.try_iter().find_map(|m| if let Msg::Dups(g) = m { Some(g) } else { None }).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].copies.iter().all(|c| c.identical));
        assert_eq!(free_name(&dir, std::ffi::OsStr::new("a.mp3")), dir.join("a (2).mp3"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
