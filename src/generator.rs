//! Playlist generator: builds multi-hour programming from templates
//! (category pattern + time-of-day patterns + inserts by songs / minutes / set times + artist and
//! song separation), laid out along the clock from the time the new rows will start.

use crate::config::{hours_ok, Category, GenLength, Marker, Template, Trigger};
use crate::library::Track;
use chrono::{Duration, NaiveDateTime, NaiveTime, Timelike};
use rand::seq::SliceRandom;
use std::collections::{HashMap, VecDeque};

/// Marker source that inserts a time announcement instead of tracks.
pub const TIME_SOURCE: &str = "@time";
/// Rough air time of a time announcement.
pub const TIME_SECS: f32 = 4.0;

/// One generated row.
#[derive(Clone, Debug)]
pub enum Gen {
    /// a track from a pattern step (`insert` = false) or an insert
    Track {
        track: Track,
        insert: bool,
    },
    TimeAnnounce,
}

struct Bag {
    idx: Vec<usize>,
    pos: usize,
}

impl Bag {
    /// The next track: not by an artist in `avoid`, nor one of `recent` (falls back to the
    /// first candidate when everything is blocked).
    fn next(&mut self, lib: &[Track], avoid: &VecDeque<String>, recent: &VecDeque<(usize, f64)>) -> Option<usize> {
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
            if (a.is_empty() || !avoid.contains(&a)) && !recent.iter().any(|r| r.0 == i) {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        fallback
    }
}

fn words(s: &str) -> Vec<String> {
    s.split(',').map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()).collect()
}

fn pool(cat: &Category, lib: &[Track]) -> Vec<usize> {
    let folder = cat.folder.trim();
    let genres = words(&cat.genre);
    let exclude = words(&cat.exclude);
    let mut v: Vec<usize> = lib
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            let year: u32 = t.year.trim().get(..4).and_then(|y| y.parse().ok()).unwrap_or(0);
            let genre = t.genre.to_lowercase();
            (folder.is_empty() || t.path.starts_with(folder))
                && (genres.is_empty() || genres.iter().any(|g| genre.contains(g.as_str())))
                && t.duration >= cat.min_secs
                && (cat.max_secs <= 0.0 || t.duration <= cat.max_secs)
                && (cat.min_year == 0 || year >= cat.min_year)
                && (cat.max_year == 0 || (year > 0 && year <= cat.max_year))
                && (exclude.is_empty() || {
                    let hay = format!("{} {} {} {}", t.artist, t.title, t.genre, t.path.display()).to_lowercase();
                    !exclude.iter().any(|w| hay.contains(w.as_str()))
                })
        })
        .map(|(i, _)| i)
        .collect();
    v.shuffle(&mut rand::thread_rng());
    v
}

/// The time of day `(h, m)` next after `t` (today or tomorrow).
fn next_clock(t: NaiveDateTime, (h, m): (u32, u32)) -> NaiveDateTime {
    let at = t.date().and_time(NaiveTime::from_hms_opt(h.min(23), m.min(59), 0).unwrap_or_default());
    if at > t {
        at
    } else {
        at + Duration::days(1)
    }
}

/// When an insert by time goes off next after `t` (None: never, or it counts songs).
fn next_due(m: &Marker, t: NaiveDateTime) -> Option<NaiveDateTime> {
    match m.trigger {
        Trigger::Songs => None,
        Trigger::Times => m.times.iter().map(|&hm| next_clock(t, hm)).min(),
        Trigger::Minutes => {
            let n = m.every_mins.max(1) as i64;
            if !m.on_clock {
                return Some(t + Duration::minutes(n));
            }
            let midnight = t.date().and_time(NaiveTime::MIN);
            let mins = (t - midnight).num_seconds() / 60;
            Some(midnight + Duration::minutes((mins / n + 1) * n))
        }
    }
}

/// When a generated playlist starting at `start` should end.
pub fn end_of(t: &Template, start: NaiveDateTime) -> NaiveDateTime {
    match t.length {
        GenLength::Minutes => start + Duration::minutes(t.minutes.max(1) as i64),
        GenLength::Until => next_clock(start, t.until),
    }
}

/// Build the rows for template `t`, starting on air at `start`. `preset` gives ready-made pools
/// (indexes into `lib`) for sources that aren't categories, e.g. "rule:3" = the files of advert /
/// jingle rule 3. `span` = how long a track takes on air.
pub fn generate(
    t: &Template,
    cats: &[Category],
    lib: &[Track],
    preset: &HashMap<String, Vec<usize>>,
    start: NaiveDateTime,
    span: &dyn Fn(&Track) -> f32,
) -> Vec<Gen> {
    let mut out: Vec<Gen> = vec![];
    if t.pattern.is_empty() && t.dayparts.iter().all(|d| d.pattern.is_empty()) {
        return out;
    }
    let end = end_of(t, start);
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
    let secs = |s: f32| Duration::milliseconds((s.max(1.0) * 1000.0) as i64);
    let mut clock = start;
    let mut recent: VecDeque<String> = VecDeque::new();
    // (track, seconds after start) of songs inside the song gap
    let mut played: VecDeque<(usize, f64)> = VecDeque::new();
    let gap = t.song_gap_mins as f64 * 60.0;
    let mut due: Vec<Option<NaiveDateTime>> = t.markers.iter().map(|m| next_due(m, start)).collect();
    let mut songs: Vec<u32> = vec![0; t.markers.len()];

    // which pattern is in use (None = the main one) and the step it's on
    let mut part: Option<Option<usize>> = None;
    let mut step = 0usize;
    let mut misses = 0;
    for _ in 0..20000 {
        if clock >= end {
            break;
        }
        let now_part = t.dayparts.iter().position(|d| !d.pattern.is_empty() && d.covers(&clock));
        if part != Some(now_part) {
            part = Some(now_part);
            step = 0;
        }
        let pat = match now_part {
            Some(k) => &t.dayparts[k].pattern,
            None => &t.pattern,
        };
        if pat.is_empty() {
            // nothing for this time of day: move on by a minute and look again
            clock += Duration::minutes(1);
            continue;
        }
        let p = &pat[step % pat.len()];
        step += 1;
        let mut picked = false;
        for _ in 0..p.count.max(1) {
            get_bag(&p.category, &mut bags);
            let since = (clock - start).num_milliseconds() as f64 / 1000.0;
            while played.front().is_some_and(|f| since - f.1 >= gap) {
                played.pop_front();
            }
            let Some(i) = bags.get_mut(&p.category).and_then(|b| b.next(lib, &recent, &played)) else { break };
            picked = true;
            let tr = lib[i].clone();
            clock += secs(span(&tr));
            recent.push_back(tr.artist.to_lowercase());
            while recent.len() > t.artist_gap as usize {
                recent.pop_front();
            }
            if gap > 0.0 {
                played.push_back((i, since));
            }
            out.push(Gen::Track { track: tr, insert: false });

            // inserts that go off now
            for (k, m) in t.markers.iter().enumerate() {
                let fire = match m.trigger {
                    Trigger::Songs => {
                        songs[k] += 1;
                        if songs[k] >= m.every_songs.max(1) {
                            songs[k] = 0;
                            Some(clock)
                        } else {
                            None
                        }
                    }
                    _ => match due[k] {
                        Some(at) if clock >= at => {
                            // the next time after now (missed times aren't played twice)
                            let mut next = next_due(m, at);
                            while let Some(n) = next.filter(|n| *n <= clock) {
                                next = next_due(m, n);
                            }
                            due[k] = next;
                            Some(at)
                        }
                        _ => None,
                    },
                };
                let Some(at) = fire else { continue };
                if !hours_ok(m.from_hour, m.to_hour, at.hour()) {
                    continue;
                }
                if m.category == TIME_SOURCE {
                    clock += secs(TIME_SECS);
                    out.push(Gen::TimeAnnounce);
                    continue;
                }
                get_bag(&m.category, &mut bags);
                for _ in 0..m.count.max(1) {
                    if let Some(j) = bags.get_mut(&m.category).and_then(|b| b.next(lib, &VecDeque::new(), &VecDeque::new())) {
                        clock += secs(span(&lib[j]));
                        out.push(Gen::Track { track: lib[j].clone(), insert: true });
                    }
                }
            }
            if clock >= end {
                return out;
            }
        }
        if picked {
            misses = 0;
        } else {
            misses += 1;
            if misses > 64 {
                break; // every source is empty
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Daypart, Marker, PatternItem};
    use chrono::NaiveDate;
    use std::path::PathBuf;

    fn lib() -> Vec<Track> {
        let mut v = vec![];
        for i in 0..40 {
            v.push(Track {
                path: PathBuf::from(format!("/music/t{i}.mp3")),
                artist: format!("A{}", i % 10),
                title: format!("T{i}"),
                duration: 180.0,
                genre: if i < 20 { "Rock".into() } else { "Pop".into() },
                year: format!("{}", 1980 + i),
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
        for i in 0..6 {
            v.push(Track {
                path: PathBuf::from(format!("/talk/k{i}.mp3")),
                artist: format!("Host{i}"),
                title: format!("K{i}"),
                duration: 300.0,
                ..Default::default()
            });
        }
        v
    }
    fn cats() -> Vec<Category> {
        vec![
            Category { name: "Music".into(), folder: "/music".into(), ..Default::default() },
            Category { name: "Jingles".into(), folder: "/jingles".into(), ..Default::default() },
            Category { name: "Talk".into(), folder: "/talk".into(), ..Default::default() },
        ]
    }
    fn at(h: u32, m: u32) -> NaiveDateTime {
        // a Monday
        NaiveDate::from_ymd_opt(2026, 10, 26).unwrap().and_hms_opt(h, m, 0).unwrap()
    }
    fn run(t: &Template, start: NaiveDateTime) -> Vec<Gen> {
        generate(t, &cats(), &lib(), &HashMap::new(), start, &|t| t.duration)
    }
    fn tracks(out: &[Gen]) -> Vec<&Track> {
        out.iter()
            .filter_map(|g| match g {
                Gen::Track { track, .. } => Some(track),
                Gen::TimeAnnounce => None,
            })
            .collect()
    }
    /// (clock time, row) for every generated row
    fn timeline(out: &[Gen], start: NaiveDateTime) -> Vec<(NaiveDateTime, &Gen)> {
        let mut t = start;
        out.iter()
            .map(|g| {
                let here = t;
                let s = match g {
                    Gen::Track { track, .. } => track.duration,
                    Gen::TimeAnnounce => TIME_SECS,
                };
                t += Duration::milliseconds((s * 1000.0) as i64);
                (here, g)
            })
            .collect()
    }

    #[test]
    fn reaches_target_length_with_markers_and_artist_gap() {
        let t = Template {
            name: "t".into(),
            minutes: 60,
            pattern: vec![PatternItem { category: "Music".into(), count: 3 }],
            markers: vec![Marker { category: "Jingles".into(), every_mins: 15, count: 1, ..Default::default() }],
            artist_gap: 4,
            ..Default::default()
        };
        let out = run(&t, at(12, 0));
        let all = tracks(&out);
        let total: f32 = all.iter().map(|t| t.duration).sum();
        assert!((3600.0..3600.0 + 400.0).contains(&total), "total {total}");
        assert!(all.iter().any(|t| t.path.starts_with("/jingles")));
        // no artist repeats within the gap among music tracks
        let music: Vec<&&Track> = all.iter().filter(|t| t.path.starts_with("/music")).collect();
        for w in music.windows(5) {
            let mut s = std::collections::HashSet::new();
            assert!(w.iter().all(|t| s.insert(t.artist.clone())), "artist repeated inside gap");
        }
    }

    #[test]
    fn empty_pool_terminates() {
        let t = Template { pattern: vec![PatternItem { category: "Nope".into(), count: 2 }], ..Default::default() };
        assert!(run(&t, at(12, 0)).is_empty());
    }

    #[test]
    fn jingle_rules_can_be_markers_and_steps() {
        let lib = lib();
        // "rule:1" = the five jingles (indexes 40..45)
        let preset: HashMap<String, Vec<usize>> = [("rule:1".to_string(), (40..45).collect())].into();
        let t = Template {
            minutes: 60,
            pattern: vec![PatternItem { category: "Music".into(), count: 2 }, PatternItem { category: "rule:1".into(), count: 1 }],
            markers: vec![Marker { category: "rule:1".into(), every_mins: 30, count: 2, ..Default::default() }],
            ..Default::default()
        };
        let out = generate(&t, &cats(), &lib, &preset, at(12, 0), &|t| t.duration);
        let all = tracks(&out);
        let jingles = all.iter().filter(|t| t.path.starts_with("/jingles")).count();
        let songs = all.len() - jingles;
        assert!(jingles >= songs / 2 + 2, "a jingle after every 2 songs plus 2 every half hour ({jingles} for {songs} songs)");
    }

    #[test]
    fn time_announcement_at_the_top_of_every_hour() {
        let t = Template {
            minutes: 180,
            pattern: vec![PatternItem { category: "Music".into(), count: 1 }],
            markers: vec![Marker { category: TIME_SOURCE.into(), every_mins: 60, on_clock: true, ..Default::default() }],
            ..Default::default()
        };
        let start = at(12, 10);
        let out = run(&t, start);
        let times: Vec<NaiveDateTime> =
            timeline(&out, start).into_iter().filter(|(_, g)| matches!(g, Gen::TimeAnnounce)).map(|(t, _)| t).collect();
        assert_eq!(times.len(), 3, "13:00, 14:00 and 15:00");
        for (k, t) in times.iter().enumerate() {
            let top = at(13 + k as u32, 0);
            // after the song that runs over the hour, so within one song length
            assert!(*t >= top && *t < top + Duration::minutes(3) + Duration::seconds(1), "announcement at {t}");
        }
    }

    #[test]
    fn inserts_at_set_times_and_after_songs_with_hours() {
        let t = Template {
            minutes: 240,
            pattern: vec![PatternItem { category: "Music".into(), count: 1 }],
            markers: vec![
                Marker { category: "Talk".into(), trigger: Trigger::Times, times: vec![(13, 30), (15, 0)], ..Default::default() },
                // a jingle after every 3 songs, but only from 14:00 to 15:00
                Marker {
                    category: "Jingles".into(),
                    trigger: Trigger::Songs,
                    every_songs: 3,
                    from_hour: 14,
                    to_hour: 15,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let start = at(12, 0);
        let out = run(&t, start);
        let line = timeline(&out, start);
        let talk: Vec<NaiveDateTime> = line
            .iter()
            .filter(|(_, g)| matches!(g, Gen::Track { track, .. } if track.path.starts_with("/talk")))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(talk.len(), 2);
        assert!(talk[0] >= at(13, 30) && talk[0] < at(13, 34));
        assert!(talk[1] >= at(15, 0) && talk[1] < at(15, 4));
        let jingles: Vec<NaiveDateTime> = line
            .iter()
            .filter(|(_, g)| matches!(g, Gen::Track { track, .. } if track.path.starts_with("/jingles")))
            .map(|(t, _)| *t)
            .collect();
        assert!(jingles.len() >= 5, "about 6 in the hour ({})", jingles.len());
        // fired from 14:00 (the song before it counted) and before 15:00 (plus one song)
        assert!(jingles.iter().all(|t| *t >= at(14, 0) && *t < at(15, 4)), "{jingles:?}");
    }

    #[test]
    fn dayparts_switch_patterns_and_until_ends_on_time() {
        let t = Template {
            length: GenLength::Until,
            until: (8, 0),
            pattern: vec![PatternItem { category: "Music".into(), count: 1 }],
            dayparts: vec![Daypart {
                from_hour: 7,
                to_hour: 9,
                pattern: vec![PatternItem { category: "Talk".into(), count: 1 }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let start = at(6, 0);
        let out = run(&t, start);
        let line = timeline(&out, start);
        let (last_t, last) = line.last().unwrap();
        let end = *last_t
            + Duration::seconds(match last {
                Gen::Track { track, .. } => track.duration as i64,
                _ => 4,
            });
        assert!(end >= at(8, 0) && end < at(8, 6), "ends at {end}");
        for (t, g) in &line {
            let Gen::Track { track, .. } = g else { continue };
            if *t < at(7, 0) {
                assert!(track.path.starts_with("/music"), "{t}: music before 7");
            } else {
                assert!(track.path.starts_with("/talk"), "{t}: talk from 7");
            }
        }
    }

    #[test]
    fn song_gap_keeps_songs_apart() {
        let t = Template {
            minutes: 120,
            artist_gap: 0,
            song_gap_mins: 30,
            pattern: vec![PatternItem { category: "Talk".into(), count: 1 }, PatternItem { category: "Music".into(), count: 1 }],
            ..Default::default()
        };
        let start = at(12, 0);
        let out = run(&t, start);
        let line = timeline(&out, start);
        let talk: Vec<(NaiveDateTime, &str)> = line
            .iter()
            .filter_map(|(t, g)| match g {
                Gen::Track { track, .. } if track.path.starts_with("/talk") => Some((*t, track.title.as_str())),
                _ => None,
            })
            .collect();
        for (k, (t, title)) in talk.iter().enumerate() {
            for (t2, title2) in &talk[k + 1..] {
                if title == title2 {
                    assert!(*t2 - *t >= Duration::minutes(30), "{title} again after {}", *t2 - *t);
                }
            }
        }
    }

    #[test]
    fn category_filters_genres_years_and_words() {
        let lib = lib();
        let c = Category { genre: "rock, jazz".into(), min_year: 1990, exclude: "t15".into(), ..Default::default() };
        let got: Vec<&Track> = pool(&c, &lib).into_iter().map(|i| &lib[i]).collect();
        assert!(!got.is_empty());
        assert!(got.iter().all(|t| t.genre == "Rock" && t.year.as_str() >= "1990" && t.title != "T15"));
        assert_eq!(got.len(), 9, "1990…1999 without T15");
    }
}
