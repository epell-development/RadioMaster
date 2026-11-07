//! The Video window: the picture of the video file on air, kept in step with the deck.
//! ffmpeg decodes it on a background thread into small RGBA frames; the window only uploads the
//! newest one. Nothing runs while no video is on air or no Video window or panel is showing.
use crate::media::{ffmpeg, probe, quiet};
use eframe::egui;
use parking_lot::Mutex;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

/// Frames per second decoded (whatever the file's own rate).
const FPS: f32 = 30.0;
/// Largest picture decoded; the window scales it to fit.
const MAX_W: u32 = 960;
const MAX_H: u32 = 540;
/// How far the picture may drift from the sound before it is restarted at the right place (a seek).
const DRIFT: f32 = 1.5;

/// Why there is no picture.
#[derive(Clone, PartialEq)]
enum Note {
    Starting,
    Playing,
    NoFfmpeg,
    NoPicture,
    Missing,
}

/// What the decoder thread hands the window.
struct Out {
    seq: u64,
    size: [usize; 2],
    rgba: Vec<u8>,
    note: Note,
}

/// Where the deck is: the position at `at`, moving on with the clock unless paused.
struct Clock {
    pos: f32,
    at: Instant,
    paused: bool,
}

impl Clock {
    fn now(&self) -> f32 {
        if self.paused {
            self.pos
        } else {
            self.pos + self.at.elapsed().as_secs_f32()
        }
    }
}

struct Session {
    path: PathBuf,
    clock: Arc<Mutex<Clock>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct Viewer {
    out: Arc<Mutex<Out>>,
    session: Option<Session>,
    tex: Option<egui::TextureHandle>,
    shown: u64,
    /// a view was drawn this frame
    used: bool,
}

impl Default for Viewer {
    fn default() -> Self {
        Viewer {
            out: Arc::new(Mutex::new(Out { seq: 0, size: [0, 0], rgba: vec![], note: Note::Starting })),
            session: None,
            tex: None,
            shown: 0,
            used: false,
        }
    }
}

impl Viewer {
    /// Call once at the end of each frame: with no Video window or panel showing, decoding stops.
    pub fn end_frame(&mut self) {
        if !std::mem::take(&mut self.used) {
            self.session = None;
            self.tex = None;
        }
    }

    /// Follow the deck: `path` on air at `pos` seconds. Starts, restarts (after a seek) or stops
    /// the decoder. Called by every view each frame; only the first call in a frame does anything.
    fn follow(&mut self, ctx: &egui::Context, path: Option<&Path>, pos: f32, playing: bool) {
        if std::mem::replace(&mut self.used, true) {
            return;
        }
        let path = path.filter(|p| crate::library::is_video(p));
        let Some(path) = path else {
            if self.session.take().is_some() {
                self.tex = None;
            }
            return;
        };
        let restart = match &self.session {
            Some(s) if s.path == path => {
                let mut c = s.clock.lock();
                let off = (c.now() - pos).abs() > DRIFT;
                *c = Clock { pos, at: Instant::now(), paused: !playing };
                off
            }
            _ => {
                self.tex = None;
                true
            }
        };
        if restart {
            self.start(ctx, path, pos, playing);
        }
        if playing {
            // (the main window too: an Additional window is only drawn when it is)
            let d = Duration::from_secs_f32(1.0 / FPS);
            ctx.request_repaint_after(d);
            ctx.request_repaint_after_for(d, egui::ViewportId::ROOT);
        }
    }

    fn start(&mut self, ctx: &egui::Context, path: &Path, pos: f32, playing: bool) {
        let clock = Arc::new(Mutex::new(Clock { pos, at: Instant::now(), paused: !playing }));
        let stop = Arc::new(AtomicBool::new(false));
        self.session = Some(Session { path: path.to_path_buf(), clock: clock.clone(), stop: stop.clone() });
        let (out, ctx, path) = (self.out.clone(), ctx.clone(), path.to_path_buf());
        if self.tex.is_none() {
            out.lock().note = Note::Starting;
        }
        std::thread::spawn(move || {
            let note = decode(&path, pos, &clock, &stop, &out, &ctx);
            if !stop.load(Ordering::Relaxed) {
                if let Some(n) = note {
                    out.lock().note = n;
                }
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }

    /// Draw the picture of the video on air, scaled to fit the space (black bars around it).
    pub fn show(&mut self, ui: &mut egui::Ui, path: Option<&Path>, pos: f32, playing: bool) {
        self.follow(ui.ctx(), path, pos, playing);
        let video = path.is_some_and(crate::library::is_video);
        let note = {
            let o = self.out.lock();
            if video && o.seq != self.shown && o.size[0] > 0 && o.rgba.len() == o.size[0] * o.size[1] * 4 {
                let img = egui::ColorImage::from_rgba_unmultiplied(o.size, &o.rgba);
                match &mut self.tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.tex = Some(ui.ctx().load_texture("video_frame", img, egui::TextureOptions::LINEAR)),
                }
                self.shown = o.seq;
            }
            o.note.clone()
        };
        let avail = ui.available_size().max(egui::vec2(160.0, 90.0));
        let (rect, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        let text = match (&self.tex, video) {
            (Some(t), true) => {
                let s = t.size_vec2();
                let k = (rect.width() / s.x).min(rect.height() / s.y);
                let r = egui::Rect::from_center_size(rect.center(), s * k);
                ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                None
            }
            (_, false) => Some(crate::i18n::tr("No video on air")),
            (None, true) => Some(match note {
                Note::Starting | Note::Playing => crate::i18n::tr("Loading video…"),
                Note::NoFfmpeg => crate::i18n::tr("Showing video needs ffmpeg. Install it, or choose it under Tools → Streams / encoders."),
                Note::NoPicture => crate::i18n::tr("This file has no picture"),
                Note::Missing => crate::i18n::tr("The video file isn't on this computer"),
            }),
        };
        if let Some(t) = text {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                t,
                egui::FontId::proportional(15.0),
                egui::Color32::from_gray(170),
            );
        }
    }
}

/// The output size for a `w`×`h` picture: within MAX_W×MAX_H, even numbers (what ffmpeg's scaler likes).
fn fit(w: u32, h: u32) -> (u32, u32) {
    let k = (MAX_W as f32 / w as f32).min(MAX_H as f32 / h as f32).min(1.0);
    let even = |x: f32| ((x.round() as u32) / 2 * 2).max(2);
    (even(w as f32 * k), even(h as f32 * k))
}

/// Decode `path` from `start` seconds and hand frames over as the clock reaches them.
/// Returns what to say when it ends early (None: it played to the end or was stopped).
fn decode(path: &Path, start: f32, clock: &Mutex<Clock>, stop: &AtomicBool, out: &Mutex<Out>, ctx: &egui::Context) -> Option<Note> {
    if !path.is_file() {
        return Some(Note::Missing);
    }
    let Some(ff) = ffmpeg() else { return Some(Note::NoFfmpeg) };
    let Some((w, h)) = probe(path).and_then(|p| p.size) else { return Some(Note::NoPicture) };
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let (w, h) = fit(w, h);
    let mut c = quiet(&ff);
    c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    if start > 0.0 {
        c.args(["-ss", &format!("{start:.3}")]);
    }
    c.arg("-i").arg(path);
    c.args(["-an", "-sn", "-dn", "-vf", &format!("fps={FPS},scale={w}:{h}"), "-pix_fmt", "rgba", "-f", "rawvideo", "-"]);
    let Ok(mut child) = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
        return Some(Note::NoFfmpeg);
    };
    let mut pipe = child.stdout.take()?;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let mut k = 0u64;
    'frames: while !stop.load(Ordering::Relaxed) {
        if pipe.read_exact(&mut buf).is_err() {
            break; // the end of the video: the last picture stays
        }
        let at = start + k as f32 / FPS;
        k += 1;
        // wait for the deck to reach this frame (the pipe fills and ffmpeg waits too)
        let now = loop {
            if stop.load(Ordering::Relaxed) {
                break 'frames;
            }
            let now = clock.lock().now();
            if at <= now + 0.5 / FPS {
                break now;
            }
            std::thread::sleep(Duration::from_secs_f32((at - now).min(0.02)));
        };
        // frames the deck has already passed are skipped to catch up
        if at < now - 2.0 / FPS {
            continue;
        }
        {
            let mut o = out.lock();
            std::mem::swap(&mut o.rgba, &mut buf);
            o.size = [w as usize, h as usize];
            o.seq += 1;
            o.note = Note::Playing;
        }
        if buf.len() != (w * h * 4) as usize {
            buf = vec![0u8; (w * h * 4) as usize];
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_and_stay_even() {
        assert_eq!(fit(1920, 1080), (960, 540));
        assert_eq!(fit(640, 480), (640, 480));
        assert_eq!(fit(1080, 1920), (304, 540));
        assert_eq!(fit(721, 401), (720, 400));
    }
}
