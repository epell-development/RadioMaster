//! Playlist generator: builds multi-hour programming from templates
//! (category pattern + rotations + markers + artist separation).

use crate::config::{Category, Template};
use crate::library::Track;
use rand::seq::SliceRandom;
use std::collections::{HashMap, VecDeque};

struct Bag {
    idx: Vec<usize>,
    pos: usize,
}

impl Bag {
    fn next(&mut self, lib: &[Track], avoid: &VecDeque<String>) -> Option<usize> {
        if self.idx.is_empty() {
            return None;
        }
        let mut fallback = None;
        for _ in 0..self.idx.len().min(40) {
            if self.pos >= self.idx.len() {
                self.idx.shuffle(&mut rand::thread_rng());
                self.pos = 0;
            }
            let i = self.idx[self.pos];
            self.pos += 1;
            let a = lib[i].artist.to_lowercase();
            if a.is_empty() || !avoid.contains(&a) {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        fallback
    }
}

fn pool(cat: &Category, lib: &[Track]) -> Vec<usize> {
    let folder = cat.folder.trim();
    let genre = cat.genre.trim().to_lowercase();
    let mut v: Vec<usize> = lib
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            (folder.is_empty() || t.path.starts_with(folder))
                && (genre.is_empty() || t.genre.to_lowercase().contains(&genre))
                && t.duration >= cat.min_secs
                && (cat.max_secs <= 0.0 || t.duration <= cat.max_secs)
        })
        .map(|(i, _)| i)
        .collect();
    v.shuffle(&mut rand::thread_rng());
    v
}

/// Build the tracks for template `t`. `preset` gives ready-made pools (indexes into `lib`) for
/// sources that aren't categories, e.g. "rule:3" = the files of advert / jingle rule 3.
pub fn generate(t: &Template, cats: &[Category], lib: &[Track], preset: &HashMap<String, Vec<usize>>) -> Vec<Track> {
    let mut out: Vec<Track> = vec![];
    if t.pattern.is_empty() {
        return out;
    }
    let target = t.minutes as f32 * 60.0;
    let mut bags: HashMap<String, Bag> = HashMap::new();
    let get_bag = |name: &str, bags: &mut HashMap<String, Bag>| {
        if !bags.contains_key(name) {
            let idx = match preset.get(name) {
                Some(v) => {
                    let mut v = v.clone();
                    v.shuffle(&mut rand::thread_rng());
                    v
                }
                None => cats.iter().find(|c| c.name == name).map(|c| pool(c, lib)).unwrap_or_default(),
            };
            bags.insert(name.to_string(), Bag { idx, pos: 0 });
        }
    };
    let mut recent: VecDeque<String> = VecDeque::new();
    let mut marker_next: Vec<f32> = t.markers.iter().map(|m| m.every_mins.max(1) as f32 * 60.0).collect();
    let mut total = 0.0f32;
    for _cycle in 0..5000 {
        let mut picked_any = false;
        for p in &t.pattern {
            for _ in 0..p.count.max(1) {
                get_bag(&p.category, &mut bags);
                let Some(i) = bags.get_mut(&p.category).and_then(|b| b.next(lib, &recent)) else { continue };
                let tr = lib[i].clone();
                total += tr.duration.max(1.0);
                recent.push_back(tr.artist.to_lowercase());
                while recent.len() > t.artist_gap as usize {
                    recent.pop_front();
                }
                out.push(tr);
                picked_any = true;
                for (k, m) in t.markers.iter().enumerate() {
                    if total >= marker_next[k] {
                        get_bag(&m.category, &mut bags);
                        for _ in 0..m.count.max(1) {
                            if let Some(j) = bags.get_mut(&m.category).and_then(|b| b.next(lib, &VecDeque::new())) {
                                total += lib[j].duration.max(1.0);
                                out.push(lib[j].clone());
                            }
                        }
                        marker_next[k] += m.every_mins.max(1) as f32 * 60.0;
                    }
                }
                if total >= target {
                    return out;
                }
            }
        }
        if !picked_any {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Marker, PatternItem};
    use std::path::PathBuf;

    fn lib() -> Vec<Track> {
        let mut v = vec![];
        for i in 0..40 {
            v.push(Track {
                path: PathBuf::from(format!("/music/t{i}.mp3")),
                artist: format!("A{}", i % 10),
                title: format!("T{i}"),
                duration: 180.0,
                genre: "Rock".into(),
                ..Default::default()
            });
        }
        for i in 0..5 {
            v.push(Track {
                path: PathBuf::from(format!("/jingles/j{i}.mp3")),
                title: format!("J{i}"),
                duration: 10.0,
                ..Default::default()
            });
        }
        v
    }
    fn cats() -> Vec<Category> {
        vec![
            Category { name: "Music".into(), folder: "/music".into(), ..Default::default() },
            Category { name: "Jingles".into(), folder: "/jingles".into(), ..Default::default() },
        ]
    }

    #[test]
    fn reaches_target_length_with_markers_and_artist_gap() {
        let t = Template {
            name: "t".into(),
            minutes: 60,
            pattern: vec![PatternItem { category: "Music".into(), count: 3 }],
            markers: vec![Marker { category: "Jingles".into(), every_mins: 15, count: 1 }],
            artist_gap: 4,
            hour_markers: false,
        };
        let out = generate(&t, &cats(), &lib(), &HashMap::new());
        let total: f32 = out.iter().map(|t| t.duration).sum();
        assert!(total >= 3600.0 && total < 3600.0 + 400.0, "total {total}");
        assert!(out.iter().any(|t| t.path.starts_with("/jingles")));
        // no artist repeats within the gap among music tracks
        let music: Vec<&Track> = out.iter().filter(|t| t.path.starts_with("/music")).collect();
        for w in music.windows(5) {
            let mut s = std::collections::HashSet::new();
            assert!(w.iter().all(|t| s.insert(t.artist.clone())), "artist repeated inside gap");
        }
    }

    #[test]
    fn empty_pool_terminates() {
        let t = Template { pattern: vec![PatternItem { category: "Nope".into(), count: 2 }], ..Default::default() };
        assert!(generate(&t, &cats(), &lib(), &HashMap::new()).is_empty());
    }

    #[test]
    fn jingle_rules_can_be_markers_and_steps() {
        let lib = lib();
        // "rule:1" = the five jingles (indexes 40..45)
        let preset: HashMap<String, Vec<usize>> = [("rule:1".to_string(), (40..45).collect())].into();
        let t = Template {
            minutes: 60,
            pattern: vec![PatternItem { category: "Music".into(), count: 2 }, PatternItem { category: "rule:1".into(), count: 1 }],
            markers: vec![Marker { category: "rule:1".into(), every_mins: 30, count: 2 }],
            ..Default::default()
        };
        let out = generate(&t, &cats(), &lib, &preset);
        let jingles = out.iter().filter(|t| t.path.starts_with("/jingles")).count();
        let songs = out.len() - jingles;
        assert!(jingles >= songs / 2 + 2, "a jingle after every 2 songs plus 2 every half hour ({jingles} for {songs} songs)");
    }
}
