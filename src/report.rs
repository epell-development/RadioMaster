//! Play reports from the play history (history.csv): songs, adverts, jingles…, summed up per
//! item, exported as CSV or as PDF (a summary report, or proof of play for each item).

use crate::pdf::{Font, Pdf, Rgb};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use std::collections::BTreeMap;

/// Item kinds in the history, with their names.
pub const KINDS: [(&str, &str); 10] = [
    ("song", "Song"),
    ("advert", "Advert"),
    ("jingle", "Jingle"),
    ("voice", "Voice track"),
    ("speech", "Speech"),
    ("time", "Time announcement"),
    ("stream", "Stream"),
    ("live", "Live input"),
    ("teaser", "Teaser"),
    ("other", "Other"),
];

pub fn kind_name(k: &str) -> &'static str {
    KINDS.iter().find(|(id, _)| *id == k).map(|(_, n)| *n).unwrap_or("Other")
}

/// One line of the play history.
#[derive(Clone, Debug)]
pub struct Play {
    pub at: NaiveDateTime,
    pub text: String,
    pub kind: String,
    pub path: String,
    pub secs: f32,
}

/// Read history.csv. Lines from before v0.2.4 only have time and text: their kind is guessed.
pub fn parse(csv: &str) -> Vec<Play> {
    let mut out = vec![];
    for line in csv.lines() {
        let line = line.trim();
        let Some(inner) = line.strip_prefix('"').and_then(|l| l.strip_suffix('"')) else { continue };
        let f: Vec<&str> = inner.split("\",\"").collect();
        let Some(at) = f.first().and_then(|t| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").ok()) else { continue };
        let text = f.get(1).copied().unwrap_or("").to_string();
        let kind = match f.get(2) {
            Some(k) if !k.is_empty() => k.to_string(),
            _ if text.starts_with("Speech —") => "speech".into(),
            _ if text.starts_with("Time announcement") => "time".into(),
            _ => "song".into(),
        };
        out.push(Play {
            at,
            text,
            kind,
            path: f.get(3).copied().unwrap_or("").to_string(),
            secs: f.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        });
    }
    out
}

pub fn load() -> Vec<Play> {
    parse(&std::fs::read_to_string(crate::config::data_dir().join("history.csv")).unwrap_or_default())
}

/// Which plays a report covers.
#[derive(Clone, Debug)]
pub struct Filter {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// kinds to include (empty = all)
    pub kinds: Vec<String>,
    /// only items whose name contains this (case-insensitive)
    pub search: String,
    /// leave out items containing any of these
    pub exclude: Vec<String>,
}

impl Filter {
    pub fn matches(&self, p: &Play) -> bool {
        let d = p.at.date();
        if d < self.from || d > self.to {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&p.kind) {
            return false;
        }
        let t = p.text.to_lowercase();
        if !self.search.trim().is_empty() && !t.contains(&self.search.trim().to_lowercase()) {
            return false;
        }
        !self.exclude.iter().any(|e| !e.is_empty() && t.contains(e))
    }
}

/// One item in a report.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub kind: String,
    pub plays: u32,
    pub first: NaiveDateTime,
    pub last: NaiveDateTime,
    /// total airtime (seconds)
    pub secs: f32,
}

/// Plays summed up per item, most played first.
pub fn summarize(plays: &[&Play]) -> Vec<Row> {
    let mut map: BTreeMap<(String, String), Row> = BTreeMap::new();
    for p in plays {
        let r = map.entry((p.kind.clone(), p.text.clone())).or_insert_with(|| Row {
            name: p.text.clone(),
            kind: p.kind.clone(),
            plays: 0,
            first: p.at,
            last: p.at,
            secs: 0.0,
        });
        r.plays += 1;
        r.first = r.first.min(p.at);
        r.last = r.last.max(p.at);
        r.secs += p.secs;
    }
    let mut rows: Vec<Row> = map.into_values().collect();
    rows.sort_by(|a, b| b.plays.cmp(&a.plays).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    rows
}

fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// One line per item.
pub fn csv_summary(rows: &[Row]) -> String {
    let mut s = String::from("Item,Type,Plays,Airtime (min),First played,Last played\n");
    for r in rows {
        s += &format!(
            "{},{},{},{:.1},{},{}\n",
            csv_field(&r.name),
            kind_name(&r.kind),
            r.plays,
            r.secs / 60.0,
            r.first.format("%Y-%m-%d %H:%M:%S"),
            r.last.format("%Y-%m-%d %H:%M:%S")
        );
    }
    s
}

/// One line per play (proof of play).
pub fn csv_plays(plays: &[&Play]) -> String {
    let mut s = String::from("Date,Time,Item,Type,Length (s),File\n");
    for p in plays {
        s += &format!(
            "{},{},{},{},{:.0},{}\n",
            p.at.format("%Y-%m-%d"),
            p.at.format("%H:%M:%S"),
            csv_field(&p.text),
            kind_name(&p.kind),
            p.secs,
            csv_field(&p.path)
        );
    }
    s
}

/// What goes in the page header.
pub struct Meta {
    /// station name (stream name) or "RadioMaster"
    pub station: String,
    /// e.g. "Advert report"
    pub title: String,
    pub filter: Filter,
    /// the station's own logo and signature
    pub brand: Brand,
}

// ------------------------------------------------------------------ station logo and signature

/// The station's logo and signature, chosen once and put on every PDF report. Kept as PNG files
/// in the data folder (not in the settings: a connected app makes its PDFs itself).
#[derive(Clone, Default)]
pub struct Brand {
    pub logo: Option<image::RgbaImage>,
    pub signature: Option<image::RgbaImage>,
    /// printed under the signature
    pub signed_by: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrandImage {
    Logo,
    Signature,
}

impl BrandImage {
    fn file(self) -> &'static str {
        match self {
            BrandImage::Logo => "logo.png",
            BrandImage::Signature => "signature.png",
        }
    }
    /// Where the copy is kept.
    pub fn path(self) -> std::path::PathBuf {
        brand_dir().join(self.file())
    }
}

fn brand_dir() -> std::path::PathBuf {
    crate::config::data_dir().join("report-branding")
}

/// Largest side of a stored logo or signature (plenty for print, keeps PDFs small).
const BRAND_MAX: u32 = 900;

impl Brand {
    /// Read the stored logo, signature and name (call off the UI thread). None chosen is normal;
    /// a stored picture that can't be read comes back as a message (and the report goes without it).
    pub fn load() -> (Brand, Vec<String>) {
        let mut problems = vec![];
        let mut img = |b: BrandImage, what: &str| {
            let path = b.path();
            if !path.exists() {
                return None;
            }
            match image::open(&path) {
                Ok(i) => Some(i.to_rgba8()),
                Err(e) => {
                    problems.push(format!("Couldn't read the {what} for reports ({}): {e}", path.display()));
                    None
                }
            }
        };
        let logo = img(BrandImage::Logo, "logo");
        let signature = img(BrandImage::Signature, "signature");
        let signed_by = std::fs::read_to_string(brand_dir().join("signed-by.txt")).unwrap_or_default().trim().to_string();
        (Brand { logo, signature, signed_by }, problems)
    }

    /// Keep a copy of a picture (PNG or JPEG) as the logo or signature, scaled down if it is big.
    /// A signature photographed on white paper gets its white made see-through.
    pub fn store(which: BrandImage, src: &std::path::Path) -> Result<image::RgbaImage, String> {
        let img = image::open(src).map_err(|e| e.to_string())?;
        let img = if img.width() > BRAND_MAX || img.height() > BRAND_MAX { img.thumbnail(BRAND_MAX, BRAND_MAX) } else { img };
        let mut img = img.to_rgba8();
        if which == BrandImage::Signature {
            clear_paper(&mut img);
        }
        std::fs::create_dir_all(brand_dir()).map_err(|e| e.to_string())?;
        img.save_with_format(which.path(), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        Ok(img)
    }

    pub fn remove(which: BrandImage) {
        let _ = std::fs::remove_file(which.path());
    }

    pub fn save_name(name: &str) {
        let _ = std::fs::create_dir_all(brand_dir());
        let _ = std::fs::write(brand_dir().join("signed-by.txt"), name.trim());
    }
}

/// Make near-white pixels see-through (a scanned or photographed signature on paper), when the
/// picture has no transparency of its own.
fn clear_paper(img: &mut image::RgbaImage) {
    if img.pixels().any(|p| p[3] < 255) {
        return;
    }
    for p in img.pixels_mut() {
        let light = p[0].min(p[1]).min(p[2]);
        if light > 200 {
            // fade out from light grey to white, so the pen strokes keep soft edges
            p[3] = (((255 - light) as u32 * 255) / 55) as u8;
        }
    }
}

impl Meta {
    fn period(&self) -> String {
        if self.filter.from == self.filter.to {
            self.filter.from.format("%A %-d %B %Y").to_string()
        } else {
            format!("{} – {}", self.filter.from.format("%-d %b %Y"), self.filter.to.format("%-d %b %Y"))
        }
    }
    fn kinds(&self) -> String {
        if self.filter.kinds.is_empty() {
            "Everything".into()
        } else {
            self.filter.kinds.iter().map(|k| kind_name(k)).collect::<Vec<_>>().join(", ")
        }
    }
}

pub fn airtime(secs: f32) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s / 60) % 60)
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

// ------------------------------------------------------------------ PDF

const INK: Rgb = (28, 33, 43);
const MUTED: Rgb = (110, 118, 132);
const ACCENT: Rgb = (31, 111, 214);
const BAND: Rgb = (22, 28, 40);
const CARD: Rgb = (241, 244, 249);
const ZEBRA: Rgb = (247, 249, 252);
const RULE: Rgb = (222, 227, 235);
const WHITE: Rgb = (255, 255, 255);
const M: f32 = 40.0; // page margin
const RIGHT: f32 = Pdf::W - M;

fn kind_color(k: &str) -> Rgb {
    match k {
        "advert" => (214, 120, 31),
        "jingle" => (126, 87, 194),
        "song" => (31, 111, 214),
        "voice" | "speech" | "time" => (30, 150, 110),
        _ => (110, 118, 132),
    }
}

struct Doc {
    pdf: Pdf,
    /// RadioMaster's logo (when the station has none of its own)
    logo: Option<(usize, f32)>,
    /// the station's logo, drawn on a white card
    station_logo: Option<(usize, f32)>,
    signature: Option<(usize, f32)>,
    signed_by: String,
    y: f32,
    label: String,
}

/// Put a picture in the PDF: (image number, width / height).
fn add_img(pdf: &mut Pdf, i: &image::RgbaImage) -> (usize, f32) {
    let aspect = i.width() as f32 / i.height().max(1) as f32;
    (pdf.add_image(i.width(), i.height(), i.as_raw()), aspect)
}

/// Width and height of a picture fitted into `max_w` × `max_h`.
fn fit_box(aspect: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    let h = max_h.min(max_w / aspect.max(0.01));
    (h * aspect, h)
}

impl Doc {
    fn new(label: &str, brand: &Brand) -> Self {
        let mut pdf = Pdf::new();
        let station_logo = brand.logo.as_ref().map(|i| add_img(&mut pdf, i));
        let logo = if station_logo.is_some() {
            None
        } else {
            image::load_from_memory(include_bytes!("../assets/logo_dark.png")).ok().map(|i| add_img(&mut pdf, &i.to_rgba8()))
        };
        let signature = brand.signature.as_ref().map(|i| add_img(&mut pdf, i));
        Doc { pdf, logo, station_logo, signature, signed_by: brand.signed_by.clone(), y: 0.0, label: label.to_string() }
    }

    /// Station / Signed / Date lines at the end of a report, with the stored signature and name.
    fn sign_off(&mut self, station: &str) {
        let sig_h = if self.signature.is_some() { 46.0 } else { 0.0 };
        self.need(70.0 + sig_h);
        self.y += 18.0;
        let line_y = self.y + 20.0 + sig_h;
        let w = (RIGHT - M - 40.0) / 3.0;
        for (i, label) in ["Station", "Signed", "Date"].iter().enumerate() {
            let x = M + i as f32 * (w + 20.0);
            self.pdf.line(x, line_y, x + w, line_y, 0.7, MUTED);
            self.pdf.text(x, line_y + 12.0, 7.5, Font::Regular, MUTED, label);
        }
        let x_sig = M + w + 20.0;
        if let Some((img, aspect)) = self.signature {
            // filled in: the station's name, the signature and today's date
            let (iw, ih) = fit_box(aspect, w, sig_h + 10.0);
            self.pdf.image(img, x_sig, line_y - ih - 2.0, iw, ih);
            self.pdf.text(M, line_y - 5.0, 9.5, Font::Regular, INK, &Pdf::fit(station, 9.5, Font::Regular, w));
            let today = chrono::Local::now().format("%-d %B %Y").to_string();
            self.pdf.text(M + 2.0 * (w + 20.0), line_y - 5.0, 9.5, Font::Regular, INK, &today);
        }
        if !self.signed_by.is_empty() {
            let name = Pdf::fit(&self.signed_by, 8.5, Font::Bold, w);
            self.pdf.text(x_sig, line_y + 24.0, 8.5, Font::Bold, INK, &name);
        }
        self.y = line_y + 36.0;
    }

    /// Dark band at the top of a page with the logo, the report name and the station.
    fn header(&mut self, meta: &Meta, heading: &str) {
        let p = &mut self.pdf;
        p.rect(0.0, 0.0, Pdf::W, 96.0, BAND);
        p.rect(0.0, 96.0, Pdf::W, 3.0, ACCENT);
        if let Some((img, aspect)) = self.station_logo {
            // on a white card, so a dark logo still shows on the dark band
            let (w, h) = fit_box(aspect, 150.0, 52.0);
            p.round_rect(M, 48.0 - h / 2.0 - 8.0, w + 16.0, h + 16.0, 6.0, WHITE);
            p.image(img, M + 8.0, 48.0 - h / 2.0, w, h);
        } else if let Some((img, aspect)) = self.logo {
            let h = 34.0;
            p.image(img, M, 31.0, h * aspect, h);
        }
        p.text_right(RIGHT, 40.0, 9.0, Font::Bold, (150, 175, 215), &self.label.to_uppercase());
        p.text_right(RIGHT, 64.0, 20.0, Font::Bold, WHITE, &Pdf::fit(heading, 20.0, Font::Bold, 330.0));
        p.text_right(RIGHT, 82.0, 9.5, Font::Regular, (190, 200, 215), &Pdf::fit(&meta.station, 9.5, Font::Regular, 330.0));
        self.y = 124.0;
        // period / contents / generated
        let gen = chrono::Local::now().format("%-d %b %Y %H:%M").to_string();
        let cols = [("PERIOD", meta.period()), ("INCLUDES", meta.kinds()), ("GENERATED", gen)];
        let w = (RIGHT - M) / 3.0;
        for (i, (k, v)) in cols.iter().enumerate() {
            let x = M + i as f32 * w;
            p.text(x, self.y, 7.5, Font::Bold, MUTED, k);
            p.text(x, self.y + 14.0, 10.0, Font::Regular, INK, &Pdf::fit(v, 10.0, Font::Regular, w - 10.0));
        }
        self.y += 36.0;
    }

    /// A row of number cards.
    fn cards(&mut self, cards: &[(String, &str)]) {
        let gap = 10.0;
        let n = cards.len().max(1) as f32;
        let w = (RIGHT - M - gap * (n - 1.0)) / n;
        for (i, (value, label)) in cards.iter().enumerate() {
            let x = M + i as f32 * (w + gap);
            self.pdf.round_rect(x, self.y, w, 58.0, 6.0, CARD);
            self.pdf.rect(x, self.y + 10.0, 3.0, 38.0, ACCENT);
            self.pdf.text(x + 14.0, self.y + 30.0, 18.0, Font::Bold, INK, &Pdf::fit(value, 18.0, Font::Bold, w - 20.0));
            self.pdf.text(x + 14.0, self.y + 46.0, 8.0, Font::Regular, MUTED, label);
        }
        self.y += 74.0;
    }

    fn section(&mut self, title: &str) {
        self.need(40.0);
        self.pdf.text(M, self.y + 10.0, 11.0, Font::Bold, INK, title);
        self.pdf.line(M, self.y + 16.0, RIGHT, self.y + 16.0, 0.6, RULE);
        self.y += 26.0;
    }

    /// Start a new page if less than `h` points are left.
    fn need(&mut self, h: f32) -> bool {
        if self.y + h > Pdf::H - 56.0 {
            self.pdf.new_page();
            self.y = 48.0;
            true
        } else {
            false
        }
    }

    /// Bars of plays per day (or per week for long periods).
    fn chart(&mut self, plays: &[&Play], from: NaiveDate, to: NaiveDate) {
        let days = (to - from).num_days().max(0) + 1;
        let weekly = days > 62;
        let mut buckets: Vec<(String, u32)> = vec![];
        if weekly {
            let mut d = from;
            while d <= to {
                buckets.push((d.format("%-d/%-m").to_string(), 0));
                d += chrono::Duration::days(7);
            }
        } else {
            for i in 0..days {
                let d = from + chrono::Duration::days(i);
                buckets.push((if days <= 14 { d.format("%a %-d").to_string() } else { d.format("%-d").to_string() }, 0));
            }
        }
        for p in plays {
            let i = (p.at.date() - from).num_days();
            let i = if weekly { i / 7 } else { i };
            if let Some(b) = buckets.get_mut(i.max(0) as usize) {
                b.1 += 1;
            }
        }
        if buckets.len() < 2 {
            return;
        }
        let h = 92.0;
        self.section(if weekly { "Plays per week" } else { "Plays per day" });
        self.need(h + 24.0);
        let top = self.y;
        let max = buckets.iter().map(|b| b.1).max().unwrap_or(0).max(1);
        let n = buckets.len() as f32;
        let slot = (RIGHT - M) / n;
        let bw = (slot * 0.7).max(1.5);
        for k in 0..=2 {
            let gy = top + h - h * k as f32 / 2.0;
            self.pdf.line(M, gy, RIGHT, gy, 0.4, RULE);
        }
        self.pdf.text(M, top - 3.0, 7.0, Font::Regular, MUTED, &max.to_string());
        let every = ((n / 16.0).ceil() as usize).max(1);
        for (i, (label, v)) in buckets.iter().enumerate() {
            let bh = h * *v as f32 / max as f32;
            let x = M + i as f32 * slot + (slot - bw) / 2.0;
            if *v > 0 {
                self.pdf.rect(x, top + h - bh, bw, bh, ACCENT);
            }
            if i % every == 0 {
                self.pdf.text_center(x + bw / 2.0, top + h + 11.0, 6.5, Font::Regular, MUTED, label);
            }
        }
        self.y = top + h + 24.0;
    }

    /// A table with a header row (repeated on new pages) and striped rows.
    /// `group`: rows with the same group number share a stripe (a long row wrapped onto several lines).
    fn table(&mut self, cols: &[(&str, f32, bool)], rows: &[Vec<String>], group: Option<&[usize]>) {
        let total: f32 = cols.iter().map(|c| c.1).sum();
        let scale = (RIGHT - M) / total;
        let head = |d: &mut Doc| {
            d.pdf.rect(M, d.y, RIGHT - M, 20.0, INK);
            let mut x = M;
            for (name, w, right) in cols {
                let w = w * scale;
                if *right {
                    d.pdf.text_right(x + w - 6.0, d.y + 13.5, 8.0, Font::Bold, WHITE, name);
                } else {
                    d.pdf.text(x + 6.0, d.y + 13.5, 8.0, Font::Bold, WHITE, name);
                }
                x += w;
            }
            d.y += 20.0;
        };
        self.need(44.0);
        head(self);
        for (ri, row) in rows.iter().enumerate() {
            if self.need(18.0) {
                head(self);
            }
            let g = group.and_then(|g| g.get(ri).copied()).unwrap_or(ri);
            if g % 2 == 1 {
                self.pdf.rect(M, self.y, RIGHT - M, 17.0, ZEBRA);
            }
            let mut x = M;
            for (ci, ((_, w, right), v)) in cols.iter().zip(row).enumerate() {
                let w = w * scale;
                let font = if ci == 1 { Font::Bold } else { Font::Regular };
                let color = INK;
                let v = Pdf::fit(v, 8.5, font, w - 12.0);
                if *right {
                    self.pdf.text_right(x + w - 6.0, self.y + 12.0, 8.5, font, color, &v);
                } else {
                    self.pdf.text(x + 6.0, self.y + 12.0, 8.5, font, color, &v);
                }
                x += w;
            }
            self.y += 17.0;
        }
        self.pdf.line(M, self.y, RIGHT, self.y, 0.6, RULE);
        self.y += 14.0;
    }

    fn finish(mut self, meta: &Meta) -> Vec<u8> {
        let n = self.pdf.page_count();
        for i in 0..n {
            self.pdf.goto_page(i);
            let y = Pdf::H - 26.0;
            self.pdf.line(M, y - 12.0, RIGHT, y - 12.0, 0.5, RULE);
            self.pdf.text(M, y, 7.5, Font::Regular, MUTED, &format!("{} · {} · {}", meta.station, self.label, meta.period()));
            self.pdf.text_right(RIGHT, y, 7.5, Font::Regular, MUTED, &format!("Page {} of {n} · RadioMaster", i + 1));
        }
        let title = format!("{} — {}", self.label, meta.period());
        self.pdf.finish(&title)
    }
}

/// Busiest hour of the day, as "14:00–15:00".
fn busiest_hour(plays: &[&Play]) -> String {
    let mut h = [0u32; 24];
    for p in plays {
        h[p.at.hour() as usize] += 1;
    }
    match h.iter().enumerate().max_by_key(|(_, n)| **n) {
        Some((i, n)) if *n > 0 => format!("{i:02}:00"),
        _ => "—".into(),
    }
}

/// Summary report: totals, plays per day and one row per item.
pub fn pdf_summary(meta: &Meta, rows: &[Row], plays: &[&Play]) -> Vec<u8> {
    let mut d = Doc::new(&meta.title, &meta.brand);
    d.header(meta, &meta.title);
    let airtime_total: f32 = rows.iter().map(|r| r.secs).sum();
    d.cards(&[
        (plays.len().to_string(), "PLAYS"),
        (rows.len().to_string(), "DIFFERENT ITEMS"),
        (airtime(airtime_total), "AIRTIME"),
        (busiest_hour(plays), "BUSIEST HOUR (STARTING)"),
    ]);
    d.chart(plays, meta.filter.from, meta.filter.to);
    // per kind, when the report mixes kinds
    let mut kinds: BTreeMap<&str, (u32, f32)> = BTreeMap::new();
    for r in rows {
        let e = kinds.entry(kind_name(&r.kind)).or_default();
        e.0 += r.plays;
        e.1 += r.secs;
    }
    if kinds.len() > 1 {
        d.section("By type");
        let krows: Vec<Vec<String>> = kinds.iter().map(|(k, (n, s))| vec![k.to_string(), n.to_string(), airtime(*s)]).collect();
        d.table(&[("Type", 3.0, false), ("Plays", 1.0, true), ("Airtime", 1.2, true)], &krows, None);
    }
    d.section(&format!("Items ({})", rows.len()));
    let trows: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            vec![
                format!("{}", i + 1),
                r.name.clone(),
                kind_name(&r.kind).to_string(),
                r.plays.to_string(),
                airtime(r.secs),
                r.first.format("%d/%m %H:%M").to_string(),
                r.last.format("%d/%m %H:%M").to_string(),
            ]
        })
        .collect();
    d.table(
        &[
            ("#", 0.45, true),
            ("Item", 4.6, false),
            ("Type", 1.15, false),
            ("Plays", 0.75, true),
            ("Airtime", 1.05, true),
            ("First", 1.35, false),
            ("Last", 1.35, false),
        ],
        &trows,
        None,
    );
    if rows.is_empty() {
        d.pdf.text(M, d.y + 10.0, 10.0, Font::Regular, MUTED, "Nothing was played in this period.");
        d.y += 20.0;
    }
    if d.signature.is_some() || !d.signed_by.is_empty() {
        d.sign_off(&meta.station);
    }
    d.finish(meta)
}

/// Proof of play: for each item, every time it went on air, grouped by day, on its own page.
pub fn pdf_proof(meta: &Meta, rows: &[Row], plays: &[&Play]) -> Vec<u8> {
    let mut d = Doc::new("Proof of play", &meta.brand);
    for (k, r) in rows.iter().enumerate() {
        if k > 0 {
            d.pdf.new_page();
        }
        d.header(meta, &r.name);
        let mine: Vec<&&Play> = plays.iter().filter(|p| p.text == r.name && p.kind == r.kind).collect();
        let mut by_day: BTreeMap<NaiveDate, Vec<NaiveDateTime>> = BTreeMap::new();
        for p in &mine {
            by_day.entry(p.at.date()).or_default().push(p.at);
        }
        let file = mine.iter().map(|p| p.path.as_str()).find(|p| !p.is_empty()).unwrap_or("");
        d.pdf.round_rect(M, d.y, RIGHT - M, 30.0, 5.0, CARD);
        d.pdf.round_rect(M + 10.0, d.y + 8.0, Pdf::width(kind_name(&r.kind), 8.0, Font::Bold) + 14.0, 15.0, 7.0, kind_color(&r.kind));
        d.pdf.text(M + 17.0, d.y + 18.5, 8.0, Font::Bold, WHITE, kind_name(&r.kind));
        let fx = M + 34.0 + Pdf::width(kind_name(&r.kind), 8.0, Font::Bold);
        d.pdf.text(fx, d.y + 19.0, 8.0, Font::Regular, MUTED, &Pdf::fit(file, 8.0, Font::Regular, RIGHT - fx - 10.0));
        d.y += 44.0;
        d.cards(&[
            (r.plays.to_string(), "TIMES AIRED"),
            (by_day.len().to_string(), "DAYS ON AIR"),
            (airtime(r.secs), "AIRTIME"),
            (r.first.format("%d/%m %H:%M").to_string(), "FIRST AIRED"),
        ]);
        d.section("Every time it was played");
        let trows: Vec<Vec<String>> = by_day
            .iter()
            .map(|(day, times)| {
                let list = times.iter().map(|t| t.format("%H:%M").to_string()).collect::<Vec<_>>().join("  ");
                vec![day.format("%a %d/%m/%Y").to_string(), list, times.len().to_string()]
            })
            .collect();
        // long days wrap onto several rows so every time is shown
        let mut wrapped: Vec<Vec<String>> = vec![];
        let mut groups: Vec<usize> = vec![];
        for (gi, row) in trows.into_iter().enumerate() {
            let times: Vec<&str> = row[1].split("  ").collect();
            for (i, chunk) in times.chunks(12).enumerate() {
                groups.push(gi);
                wrapped.push(vec![
                    if i == 0 { row[0].clone() } else { String::new() },
                    chunk.join("   "),
                    if i == 0 { row[2].clone() } else { String::new() },
                ]);
            }
        }
        d.table(&[("Date", 1.4, false), ("Times on air", 5.2, false), ("Count", 0.7, true)], &wrapped, Some(&groups));
        d.sign_off(&meta.station);
    }
    if rows.is_empty() {
        d.header(meta, "Proof of play");
        d.pdf.text(M, d.y + 10.0, 10.0, Font::Regular, MUTED, "Nothing was played in this period.");
    }
    d.finish(meta)
}

/// Monday of the week `d` is in.
pub fn week_start(d: NaiveDate) -> NaiveDate {
    d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = r#""2026-10-01 08:00:00","Old Song"
"2026-10-01 08:04:00","Ad One","advert","C:\ads\one.mp3","30.0"
"2026-10-01 09:04:00","Ad One","advert","C:\ads\one.mp3","30.0"
"2026-10-02 10:00:00","Jingle A","jingle","/j/a.wav","5.5"
"2026-10-02 10:01:00","Speech — hello","speech","","3.0"
"2026-10-03 10:00:00","Song B","song","/m/b.mp3","200.0"
"#;

    fn filter(kinds: &[&str]) -> Filter {
        Filter {
            from: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(),
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            search: String::new(),
            exclude: vec![],
        }
    }

    #[test]
    fn reads_old_and_new_history_and_sums_per_item() {
        let all = parse(CSV);
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].kind, "song", "old two-column lines still count");
        assert_eq!(all[4].kind, "speech");
        let f = filter(&["advert"]);
        let ads: Vec<&Play> = all.iter().filter(|p| f.matches(p)).collect();
        let rows = summarize(&ads);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].plays, rows[0].secs), (2, 60.0));
        assert!(csv_summary(&rows).contains("\"Ad One\",Advert,2,1.0"));
        assert!(csv_plays(&ads).lines().count() == 3);
        let f = filter(&[]);
        let n = all.iter().filter(|p| f.matches(p)).count();
        assert_eq!(n, 5, "the 3rd of October is outside the period");
    }

    #[test]
    fn pdfs_are_made() {
        let all = parse(CSV);
        let f = filter(&[]);
        let sel: Vec<&Play> = all.iter().filter(|p| f.matches(p)).collect();
        let rows = summarize(&sel);
        let meta = Meta { station: "Test FM".into(), title: "Play report".into(), filter: f.clone(), brand: Brand::default() };
        let a = pdf_summary(&meta, &rows, &sel);
        let b = pdf_proof(&meta, &rows, &sel);
        // with the station's own logo and signature
        let mut sig = image::RgbaImage::from_pixel(300, 100, image::Rgba([255, 255, 255, 255]));
        for x in 20..280 {
            sig.put_pixel(x, 50 + (x % 20) / 4, image::Rgba([10, 20, 90, 255]));
        }
        clear_paper(&mut sig);
        assert_eq!(sig.get_pixel(0, 0)[3], 0, "the paper is see-through");
        assert_eq!(sig.get_pixel(20, 50)[3], 255, "the ink is not");
        let brand = Brand {
            logo: Some(image::RgbaImage::from_pixel(200, 80, image::Rgba([200, 30, 30, 255]))),
            signature: Some(sig),
            signed_by: "Jane Doe, Station Manager".into(),
        };
        let branded = Meta { brand, ..meta };
        let c = pdf_proof(&branded, &rows, &sel);
        let e = pdf_summary(&branded, &rows, &sel);
        let images = |pdf: &[u8]| String::from_utf8_lossy(pdf).matches("/Subtype /Image").count();
        let masks = |pdf: &[u8]| String::from_utf8_lossy(pdf).matches("/SMask").count();
        let plain = images(&b) - masks(&b);
        assert_eq!(images(&c) - masks(&c), plain + 1, "the station's logo instead of RadioMaster's, and the signature");
        assert!(masks(&c) >= 1, "the signature keeps its see-through paper");
        let meta = branded;
        for pdf in [&a, &b, &c, &e] {
            assert!(pdf.starts_with(b"%PDF-1.4"));
            assert!(String::from_utf8_lossy(pdf).contains("%%EOF"));
        }
        assert!(String::from_utf8_lossy(&b).contains(&format!("/Count {}", rows.len())), "one page per item");
        assert!(String::from_utf8_lossy(&c).contains(&format!("/Count {}", rows.len())), "signing adds no pages here");
        // RM_PDF_OUT=folder [RM_PDF_IN=history.csv]: write sample PDFs to look at
        if let Ok(dir) = std::env::var("RM_PDF_OUT") {
            let src = std::env::var("RM_PDF_IN").ok().and_then(|p| std::fs::read_to_string(p).ok());
            let all = src.map(|s| parse(&s)).unwrap_or(all);
            let mut f = filter(&[]);
            f.from = all.iter().map(|p| p.at.date()).min().unwrap();
            f.to = all.iter().map(|p| p.at.date()).max().unwrap();
            let sel: Vec<&Play> = all.iter().filter(|p| f.matches(p)).collect();
            let rows = summarize(&sel);
            let brand = if std::env::var("RM_PDF_BRAND").is_ok() { Brand::load().0 } else { meta.brand.clone() };
            let meta = Meta { station: "DovaFM Test Station".into(), title: "Play report".into(), filter: f.clone(), brand };
            std::fs::write(format!("{dir}/summary.pdf"), pdf_summary(&meta, &rows, &sel)).unwrap();
            let ads: Vec<&Play> = sel.iter().copied().filter(|p| p.kind == "advert").collect();
            let ad_rows = summarize(&ads);
            let meta = Meta { title: "Advert report".into(), filter: Filter { kinds: vec!["advert".into()], ..f }, ..meta };
            std::fs::write(format!("{dir}/proof.pdf"), pdf_proof(&meta, &ad_rows, &ads)).unwrap();
            std::fs::write(format!("{dir}/ads.pdf"), pdf_summary(&meta, &ad_rows, &ads)).unwrap();
        }
    }
}
