//! Floating tool windows (instead of workzone tabs) and the waveform fade editor.

use super::*;
use crate::audio::{PreviewPlayer, PreviewSide};
use crate::fade::{detect_bpm, detect_level, smart_cues, Cues, FadeMode, Waveform, BIN};
use eframe::egui::{Align2, FontId, Pos2, Rect, Sense, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum ToolWin {
    FadeEditor,
    Generator,
    Ads,
    AudioFx,
    Streams,
    AzuraCast,
    Reports,
    History,
    Notepad,
    Plugins,
    Scheduler,
}

impl ToolWin {
    pub(super) const ALL: [ToolWin; 11] = [
        ToolWin::FadeEditor,
        ToolWin::Generator,
        ToolWin::Ads,
        ToolWin::AudioFx,
        ToolWin::Streams,
        ToolWin::AzuraCast,
        ToolWin::Reports,
        ToolWin::History,
        ToolWin::Notepad,
        ToolWin::Plugins,
        ToolWin::Scheduler,
    ];
    /// Whether an operator with these permissions may open this tool.
    pub(super) fn allowed(self, p: &crate::auth::Perms) -> bool {
        use crate::auth::tool;
        match self {
            ToolWin::FadeEditor => p.tool(tool::TRACK_EDITOR),
            ToolWin::Generator => p.tool(tool::GENERATOR),
            ToolWin::Ads => p.tool(tool::ADS),
            ToolWin::Streams => p.streaming,
            ToolWin::AzuraCast => p.tool(tool::AZURACAST),
            ToolWin::AudioFx => p.tool(tool::AUDIO_FX),
            ToolWin::Plugins => p.tool(tool::PLUGINS),
            ToolWin::Scheduler => p.tool(tool::SCHEDULER),
            ToolWin::Reports => p.tool(tool::REPORTS),
            ToolWin::History => p.tool(tool::HISTORY),
            ToolWin::Notepad => p.tool(tool::NOTEPAD),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ToolWin::FadeEditor => "Track editor",
            ToolWin::Generator => "Playlist generator",
            ToolWin::Ads => "Advertisement scheduler",
            ToolWin::AudioFx => "Audio processing",
            ToolWin::Streams => "Streams / encoders",
            ToolWin::AzuraCast => "AzuraCast station manager",
            ToolWin::Reports => "Reports",
            ToolWin::History => "Play history",
            ToolWin::Notepad => "Notepad",
            ToolWin::Plugins => "Plugins",
            ToolWin::Scheduler => "Event scheduler",
        }
    }
    pub(super) fn icon(self) -> &'static str {
        match self {
            ToolWin::FadeEditor => "✏",
            ToolWin::Generator => "🎲",
            ToolWin::Ads => "📢",
            ToolWin::AudioFx => "🔈",
            ToolWin::Streams => "📡",
            ToolWin::AzuraCast => "☁",
            ToolWin::Reports => "📊",
            ToolWin::History => "📜",
            ToolWin::Notepad => "📝",
            ToolWin::Plugins => "🔌",
            ToolWin::Scheduler => "⏰",
        }
    }
    fn size(self) -> [f32; 2] {
        match self {
            ToolWin::FadeEditor => [1060.0, 620.0],
            ToolWin::Generator => [820.0, 560.0],
            ToolWin::Ads => [1000.0, 600.0],
            ToolWin::AudioFx => [760.0, 560.0],
            ToolWin::Streams => [820.0, 520.0],
            ToolWin::AzuraCast => [900.0, 560.0],
            ToolWin::Reports => [760.0, 480.0],
            ToolWin::History => [560.0, 480.0],
            ToolWin::Notepad => [560.0, 460.0],
            ToolWin::Plugins => [720.0, 520.0],
            ToolWin::Scheduler => [900.0, 560.0],
        }
    }
}

/// Where the track being edited came from (decides what Prev / Next do).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum EditSrc {
    Playlist(usize),
    /// a library track (its index in the library), stepping through the library list on screen
    Library(Option<usize>),
}

/// Working state of the track editor (tags + cue points + fades).
pub(super) struct FadeEdit {
    src: EditSrc,
    path: PathBuf,
    duration: f32,
    tags: Track,
    tags_edited: bool,
    info: String,
    cues: Cues,
    have_cues: bool,
    edited: bool,
    next: Option<(PathBuf, String, f32)>,
    view: (f32, f32),
    view_set: bool,
    drag: Option<usize>,
    cursor: f32,
    msg: String,
}

// markers: 0 START, 1 FADE IN (end), 2 FADE OUT (start), 3 MIX (next starts), 4 END, 5 INTRO, 6 OUTRO
const MARK_NAMES: [&str; 7] = ["START", "FADE IN", "FADE OUT", "MIX", "END", "INTRO", "OUTRO"];

fn mark_color(i: usize) -> Color32 {
    match i {
        0 => Color32::from_rgb(60, 200, 90),
        1 => Color32::from_rgb(70, 200, 200),
        2 => Color32::from_rgb(245, 160, 40),
        3 => Color32::from_rgb(90, 150, 255),
        4 => Color32::from_rgb(235, 70, 70),
        5 => Color32::from_rgb(190, 120, 255),
        _ => Color32::from_rgb(255, 120, 200),
    }
}

fn mark_time(c: &Cues, i: usize) -> f32 {
    match i {
        0 => c.cue_in,
        1 => c.cue_in + c.fade_in,
        2 => c.fade_out,
        3 => c.mix,
        4 => c.cue_out,
        5 => c.intro,
        _ => c.outro,
    }
}

fn mark_visible(c: &Cues, i: usize) -> bool {
    match i {
        5 => c.intro > 0.0,
        6 => c.outro > 0.0,
        _ => true,
    }
}

fn set_mark(c: &mut Cues, i: usize, t: f32, dur: f32) {
    match i {
        0 => c.cue_in = t,
        1 => c.fade_in = (t - c.cue_in).max(0.0),
        2 => c.fade_out = t,
        3 => c.mix = t,
        4 => c.cue_out = t,
        5 => c.intro = t,
        _ => c.outro = t,
    }
    c.clamp(dur);
}

fn mmss(t: f32) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0) as u32, t % 60.0)
}

/// Highest peak in the bins covering [a, b) seconds.
fn peak_in(w: &Waveform, a: f32, b: f32) -> f32 {
    if w.peaks.is_empty() || a >= w.duration() || b <= 0.0 {
        return 0.0;
    }
    let i = ((a.max(0.0) / BIN) as usize).min(w.peaks.len() - 1);
    let j = ((b / BIN).ceil() as usize).clamp(i + 1, w.peaks.len());
    w.peaks[i..j].iter().fold(0f32, |m, &p| m.max(p))
}

impl App {
    /// Cover art of the track on air (embedded picture or cover.jpg next to the file), loaded in the background.
    pub(super) fn album_art(&mut self, ui: &mut egui::Ui, path: Option<&std::path::Path>, size: f32) {
        // receive a finished decode
        if let Some(rx) = &self.art_rx {
            if let Ok((p, img)) = rx.try_recv() {
                let tex = img.map(|im| ui.ctx().load_texture(format!("art:{}", p.display()), im, egui::TextureOptions::LINEAR));
                self.art = Some((p, tex));
                self.art_rx = None;
            }
        }
        // start a decode when the track changes
        let wanted = path.map(|p| p.to_path_buf());
        let have = self.art.as_ref().map(|(p, _)| p.clone());
        if wanted.is_some() && wanted != have && self.art_rx.is_none() {
            let p = wanted.clone().unwrap();
            let (tx, rx) = mpsc::channel();
            self.art_rx = Some(rx);
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let img = crate::library::cover_bytes(&p).and_then(|b| image::load_from_memory(&b).ok()).map(|im| {
                    let im = im.thumbnail(160, 160).to_rgba8();
                    egui::ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw())
                });
                let _ = tx.send((p, img));
                ctx.request_repaint();
            });
        }
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
        let tex = match (&self.art, &wanted) {
            (Some((p, Some(t))), Some(w)) if p == w => Some(t.clone()),
            _ => None,
        };
        match tex {
            Some(t) => {
                egui::Image::new(&t).corner_radius(4).paint_at(ui, rect);
            }
            None => {
                ui.painter().rect_filled(rect, 4.0, Color32::from_gray(70));
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, "♪", FontId::proportional(size * 0.45), Color32::from_gray(160));
            }
        }
    }

    /// Open a tool from a menu or button. The track editor starts on the selected (or on-air) track.
    pub(super) fn launch_tool(&mut self, core: &mut Playout, w: ToolWin) {
        if w == ToolWin::FadeEditor && w.allowed(&self.perm) {
            self.open_fade_editor(core, None);
        } else {
            self.open_tool(w);
        }
    }

    pub(super) fn open_tool(&mut self, w: ToolWin) {
        if w.allowed(&self.perm) {
            self.tool_open.insert(w);
        } else {
            *self.notify.lock() = format!("Your account isn't allowed to use {}", w.label());
        }
    }

    /// Open the track editor for a playlist row (or the selected / on-air track).
    pub(super) fn open_fade_editor(&mut self, core: &mut Playout, row: Option<usize>) {
        if !self.perm.tool(crate::auth::tool::TRACK_EDITOR) {
            core.say("Your account isn't allowed to use the track editor");
            return;
        }
        let idx = row.or_else(|| self.sel.iter().next().copied()).or(core.current).unwrap_or(0);
        let Some(entry) = core.playlist.get(idx) else {
            core.say("Add a track to the playlist first, then open the track editor on it");
            self.tool_open.insert(ToolWin::FadeEditor);
            return;
        };
        let t = entry.track.clone();
        let next = core.playlist.get(idx + 1).map(|e| (e.track.path.clone(), e.track.label(), e.track.duration));
        self.load_track_editor(core, EditSrc::Playlist(idx), t, next);
    }

    /// Open the track editor for a track that isn't (necessarily) in the playlist.
    pub(super) fn open_track_editor_for(&mut self, core: &mut Playout, t: Track, lib_index: Option<usize>) {
        // the next track in the library list (for the transition preview)
        let next = lib_index
            .and_then(|li| self.lib_filtered.iter().position(|&x| x == li))
            .and_then(|p| self.lib_filtered.get(p + 1))
            .and_then(|&j| core.library.get(j))
            .map(|n| (n.path.clone(), n.label(), n.duration));
        self.load_track_editor(core, EditSrc::Library(lib_index), t, next);
    }

    fn load_track_editor(&mut self, core: &mut Playout, src: EditSrc, t: Track, next: Option<(PathBuf, String, f32)>) {
        self.preview = None;
        self.te_from_lib = matches!(src, EditSrc::Library(_));
        self.te_scroll = true;
        core.request_analysis(&t.path);
        if let Some((p, _, _)) = &next {
            core.request_analysis(p);
        }
        let d = t.duration.max(0.1);
        self.fade_edit = Some(FadeEdit {
            src,
            path: t.path.clone(),
            duration: d,
            info: crate::library::format_info(&t.path),
            tags: t,
            tags_edited: false,
            cues: Cues::plain(d),
            have_cues: false,
            edited: false,
            next,
            view: (0.0, d),
            view_set: false,
            drag: None,
            cursor: 0.0,
            msg: String::new(),
        });
        self.tool_open.insert(ToolWin::FadeEditor);
    }

    /// Write edited tags and cues. Returns false if nothing needed saving.
    fn apply_track_edit(fe: &mut FadeEdit, core: &mut Playout) -> bool {
        let mut did = false;
        if fe.tags_edited {
            core.update_tags(fe.tags.clone());
            fe.tags_edited = false;
            did = true;
        }
        if fe.edited {
            core.set_manual_cues(&fe.path, fe.cues, fe.duration);
            fe.edited = false;
            fe.have_cues = true;
            did = true;
        }
        if did {
            core.save_all();
        }
        did
    }

    pub(super) fn tool_windows(&mut self, ctx: &egui::Context, core: &mut Playout) {
        // drop a preview that has finished
        if self.preview.as_ref().is_some_and(|p| p.finished()) {
            self.preview = None;
        }
        // a tool the operator may not use (e.g. after signing in as someone else) closes
        let perm = self.perm;
        self.tool_open.retain(|w| w.allowed(&perm));
        for w in ToolWin::ALL {
            if !self.tool_open.contains(&w) {
                continue;
            }
            let mut open = true;
            let title = match (&self.fade_edit, w) {
                (Some(fe), ToolWin::FadeEditor) => {
                    format!("Track editor — {}", fe.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                }
                _ => w.label().to_string(),
            };
            Self::window_for(ctx, &title, w.size()).id(egui::Id::new(("tool_window", w as u8))).open(&mut open).resizable(true).show(
                ctx,
                |ui| {
                    if w == ToolWin::FadeEditor {
                        self.track_editor(ui, core);
                        return;
                    }
                    if w == ToolWin::AzuraCast {
                        self.az_manager(ui, core);
                        return;
                    }
                    if w == ToolWin::Reports {
                        self.tab_reports(ui, core); // has its own scrolling list
                        return;
                    }
                    if w == ToolWin::Ads {
                        self.tab_ads(ui, core); // list + editor, each scrolling
                        return;
                    }
                    egui::ScrollArea::both().id_salt(("tool_scroll", w as u8)).auto_shrink([false, false]).show(ui, |ui| match w {
                        ToolWin::Generator => self.tab_generator(ui, core),
                        ToolWin::Ads => self.tab_ads(ui, core),
                        ToolWin::AudioFx => self.tab_dsp(ui, core),
                        ToolWin::Streams => self.tab_streaming(ui, core),
                        ToolWin::Notepad => self.tab_segment(ui, core),
                        ToolWin::Plugins => self.tab_plugins(ui, core),
                        ToolWin::Scheduler => self.tab_scheduler(ui, core),
                        ToolWin::AzuraCast => {}
                        ToolWin::Reports => self.tab_reports(ui, core),
                        ToolWin::History => self.tab_history(ui, core),
                        ToolWin::FadeEditor => {}
                    });
                },
            );
            if !open || (w == ToolWin::FadeEditor && self.fade_edit.as_ref().is_some_and(|f| f.msg == "\u{0}close")) {
                self.tool_open.remove(&w);
                if w == ToolWin::FadeEditor {
                    self.preview = None;
                    self.fade_edit = None;
                }
            }
        }
    }

    /// Smart-fading options (Settings → Playback).
    pub(super) fn smart_fade_settings(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let before = core.cfg.fade.clone();
        ui.horizontal(|ui| {
            ui.label(tr("Transitions"));
            ui.selectable_value(&mut core.cfg.fade.mode, FadeMode::Smart, tr("Smart fading"));
            ui.selectable_value(&mut core.cfg.fade.mode, FadeMode::Fixed, tr("Fixed crossfade"));
        });
        match core.cfg.fade.mode {
            FadeMode::Smart => {
                ui.small(tr("Each track is analysed: silence is skipped, cold endings get almost no overlap, and songs that fade out let the next one start as the outro dies away. Cues you set in the track editor always win."));
                let f = &mut core.cfg.fade;
                egui::Grid::new("smartfade").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Outro ends when it drops"));
                    ui.add(egui::Slider::new(&mut f.drop_db, 4.0..=30.0).suffix(" dB below the song"));
                    ui.end_row();
                    ui.label(tr("Longest overlap"));
                    ui.add(egui::Slider::new(&mut f.max_overlap, 0.5..=20.0).suffix(" s"));
                    ui.end_row();
                    ui.label(tr("Overlap on cold endings"));
                    ui.add(egui::Slider::new(&mut f.hard_overlap, 0.0..=2.0).suffix(" s"));
                    ui.end_row();
                    ui.label(tr("Fade on cold endings"));
                    ui.add(egui::Slider::new(&mut f.hard_fade, 0.05..=2.0).suffix(" s"));
                    ui.end_row();
                    ui.label(tr("Silence below"));
                    ui.add(egui::Slider::new(&mut f.silence_db, -80.0..=-20.0).suffix(" dBFS"));
                    ui.end_row();
                });
                if ui.button(tr("Reset smart settings")).clicked() {
                    core.cfg.fade = crate::fade::SmartFadeConfig::default();
                }
            }
            FadeMode::Fixed => {
                ui.add(egui::Slider::new(&mut core.cfg.crossfade_secs, 0.0..=15.0).text("Crossfade seconds"));
                ui.small(tr("Every track overlaps the next by this much. Tracks with cues set in the track editor still use their own."));
            }
        }
        if core.cfg.fade != before {
            core.reanalyse_cues();
        }
    }

    /// List beside the track editor: every track in the library (or the playlist), so you can
    /// jump to any of them. Returns the one clicked.
    fn track_browser(&mut self, ui: &mut egui::Ui, core: &mut Playout) -> Option<EditSrc> {
        let current = self.fade_edit.as_ref().map(|f| f.path.clone());
        let mut pick = None;
        egui::SidePanel::left("te_browser").resizable(true).default_width(250.0).width_range(160.0..=480.0).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.te_from_lib, true, tr("Library"));
                ui.selectable_value(&mut self.te_from_lib, false, tr("Playlist"));
            });
            if self.te_from_lib {
                ui.add(
                    egui::TextEdit::singleline(&mut self.lib_filter).desired_width(f32::INFINITY).hint_text(tr("🔍 search the library")),
                );
                self.refresh_lib_filter(core);
                let n = self.lib_filtered.len();
                ui.label(RichText::new(format!("{n} {} · {}", tr("tracks"), tr("✔ = cues set by hand"))).small().color(GRAY));
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                let mut scroll = egui::ScrollArea::vertical().id_salt("te_lib_list").auto_shrink([false, false]);
                if self.te_scroll {
                    if let Some(pos) = current.as_ref().and_then(|c| self.lib_filtered.iter().position(|&i| &core.library[i].path == c)) {
                        scroll = scroll.vertical_scroll_offset((pos as f32 * (row_h + ui.spacing().item_spacing.y) - 120.0).max(0.0));
                    }
                    self.te_scroll = false;
                }
                scroll.show_rows(ui, row_h, n, |ui, range| {
                    for pos in range {
                        let li = self.lib_filtered[pos];
                        let t = &core.library[li];
                        let on = current.as_ref() == Some(&t.path);
                        let mark = if core.manual_cues_for(&t.path) { "✔ " } else { "" };
                        let r = ui.add(
                            egui::Button::new(format!("{mark}{}", t.label()))
                                .selected(on)
                                .frame(on)
                                .truncate()
                                .min_size(egui::vec2(ui.available_width(), row_h)),
                        );
                        if r.clicked() && !on {
                            pick = Some(EditSrc::Library(Some(li)));
                        }
                    }
                });
            } else {
                let n = core.playlist.len();
                ui.label(RichText::new(format!("{n} {}", tr("rows"))).small().color(GRAY));
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                egui::ScrollArea::vertical().id_salt("te_pl_list").auto_shrink([false, false]).show_rows(ui, row_h, n, |ui, range| {
                    for i in range {
                        let e = &core.playlist[i];
                        let on = e.is_file() && current.as_ref() == Some(&e.track.path);
                        let r = ui.add_enabled(
                            e.is_file(),
                            egui::Button::new(format!("{}. {}", i + 1, e.title()))
                                .selected(on)
                                .frame(on)
                                .truncate()
                                .min_size(egui::vec2(ui.available_width(), row_h)),
                        );
                        if r.clicked() && !on {
                            pick = Some(EditSrc::Playlist(i));
                        }
                    }
                });
            }
        });
        pick
    }

    /// Open the track the browser list picked (after saving the one being edited).
    fn open_picked(&mut self, core: &mut Playout, pick: EditSrc) {
        if let Some(mut fe) = self.fade_edit.take() {
            Self::apply_track_edit(&mut fe, core);
        }
        match pick {
            EditSrc::Playlist(i) => {
                self.open_fade_editor(core, Some(i));
                self.sel.clear();
                self.sel.insert(i);
            }
            EditSrc::Library(Some(li)) => {
                if let Some(t) = core.library.get(li).cloned() {
                    self.lib_sel = [li].into();
                    self.lib_anchor = Some(li);
                    self.lib_scroll = Some(li);
                    self.open_track_editor_for(core, t, Some(li));
                }
            }
            EditSrc::Library(None) => {}
        }
        self.te_scroll = false; // it was clicked in the list: it is already in view
    }

    fn track_editor(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let header = ui.horizontal(|ui| {
            let t = if self.te_browser { tr("◀ Hide track list") } else { tr("▶ Track list") };
            ui.small_button(t).on_hover_text(tr("Show every track in the library or playlist next to the editor")).clicked()
        });
        if header.inner {
            self.te_browser = !self.te_browser;
            self.te_scroll = true;
        }
        if self.te_browser {
            if let Some(pick) = self.track_browser(ui, core) {
                self.open_picked(core, pick);
                return;
            }
        }
        let Some(mut fe) = self.fade_edit.take() else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("Select a track in the playlist, then:")).color(GRAY));
                if ui.button(tr("Edit selected track")).clicked() {
                    let row = self.sel.iter().next().copied();
                    self.open_fade_editor(core, row);
                }
                ui.label(RichText::new(tr("…or right-click a playlist row → Track editor.")).color(GRAY));
            });
            return;
        };

        // pick up analysed cues once they exist (unless the user already started editing)
        if !fe.have_cues && !fe.edited {
            if let Some(c) = core.cues.get(&fe.path).copied() {
                fe.cues = c;
                fe.have_cues = true;
            }
        }
        let wave = core.waves.get(&fe.path).cloned();
        if let Some(w) = &wave {
            if (w.duration() - fe.duration).abs() > 0.5 && w.duration() > 0.5 {
                fe.duration = w.duration(); // tag duration was wrong; trust the decoded audio
                fe.view_set = false;
            }
        }
        if !fe.view_set {
            fe.view = (0.0, fe.duration);
            fe.view_set = true;
        }
        let next = fe.next.clone();
        let next_wave = next.as_ref().and_then(|(p, _, _)| core.waves.get(p).cloned());
        let next_cues = next.as_ref().map(|(p, _, d)| core.cues.get(p).copied().unwrap_or_else(|| Cues::plain(*d)));
        let playhead = self.preview.as_ref().and_then(|pv| {
            let (pos, cur) = pv.position();
            (cur.as_deref() == Some(fe.path.as_path())).then_some(pos)
        });
        let mut nav: Option<i64> = None;
        let mut close = false;

        // keyboard: PgUp / PgDn = previous / next track, F12 = stop
        if !ui.ctx().wants_keyboard_input() {
            if ui.input(|i| i.key_pressed(egui::Key::PageUp)) {
                nav = Some(-1);
            }
            if ui.input(|i| i.key_pressed(egui::Key::PageDown)) {
                nav = Some(1);
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::F12)) {
            self.preview = None;
        }

        // ================= footer: prev / next / OK / cancel / apply
        egui::TopBottomPanel::bottom("te_footer").exact_height(40.0).show_inside(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let in_pl = matches!(fe.src, EditSrc::Playlist(_) | EditSrc::Library(Some(_)));
                if ui.add_enabled(in_pl, egui::Button::new(tr("◀ Prev (PgUp)"))).clicked() {
                    nav = Some(-1);
                }
                if ui.add_enabled(in_pl, egui::Button::new(tr("Next (PgDn) ▶"))).clicked() {
                    nav = Some(1);
                }
                if !fe.msg.is_empty() && fe.msg != "\u{0}close" {
                    ui.label(RichText::new(&fe.msg).small().color(GRAY));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("Cancel")).clicked() {
                        close = true;
                    }
                    if ui.button(tr("Apply")).clicked() {
                        fe.msg = if Self::apply_track_edit(&mut fe, core) { "Saved".into() } else { "Nothing to save".into() };
                    }
                    if ui.button(RichText::new(tr("  OK  ")).strong()).clicked() {
                        Self::apply_track_edit(&mut fe, core);
                        close = true;
                    }
                    let dirty = fe.edited || fe.tags_edited;
                    if dirty {
                        ui.label(RichText::new(tr("● unsaved")).small().color(Color32::from_rgb(210, 130, 0)));
                    }
                });
            });
        });

        // ================= left: tags
        egui::SidePanel::left("te_tags").resizable(false).exact_width(250.0).show_inside(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("te_tags_scroll").show(ui, |ui| {
                let mut ch = false;
                let mut field = |ui: &mut egui::Ui, label: &str, v: &mut String| {
                    ui.label(RichText::new(label).small().color(GRAY));
                    ch |= ui.add(egui::TextEdit::singleline(v).desired_width(f32::INFINITY)).changed();
                };
                field(ui, "Artist", &mut fe.tags.artist);
                field(ui, "Title", &mut fe.tags.title);
                field(ui, "Album", &mut fe.tags.album);
                field(ui, "Year", &mut fe.tags.year);
                field(ui, "Genre", &mut fe.tags.genre);
                ui.label(RichText::new(tr("Comment")).small().color(GRAY));
                ch |= ui.add(egui::TextEdit::multiline(&mut fe.tags.comment).desired_rows(3).desired_width(f32::INFINITY)).changed();
                if ch {
                    fe.tags_edited = true;
                }
                ui.add_space(8.0);
                ui.separator();
                let last = core.last_played_of(&fe.path);
                ui.label(RichText::new(format!("Length: {}", mmss(fe.duration))).small());
                ui.label(RichText::new(format!("Plays: {}   Last: {}", last.0, last.1.unwrap_or_else(|| "never".into()))).small());
            });
        });

        // ================= right: waveform + cues
        egui::CentralPanel::default().show_inside(ui, |ui| {
            // scrolls instead of growing the window past the screen
            egui::ScrollArea::vertical().id_salt("te_main").auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(fe.path.to_string_lossy()).small().color(GRAY)).truncate());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&fe.info).small().strong());
                    let stored = core.cues.get(&fe.path).copied();
                    let (badge, col) = if fe.edited {
                        ("cues: unsaved changes", Color32::from_rgb(200, 120, 0))
                    } else if stored.is_some_and(|c| c.manual) {
                        ("cues: set by hand", Color32::from_rgb(200, 120, 0))
                    } else if core.cfg.fade.mode == FadeMode::Smart {
                        ("cues: smart (automatic)", Color32::from_rgb(20, 140, 50))
                    } else {
                        ("cues: fixed crossfade — Apply to use these", GRAY)
                    };
                    ui.label(RichText::new(badge).small().color(col));
                    if wave.is_none() {
                        ui.spinner();
                        ui.label(RichText::new(tr("analysing…")).small().color(GRAY));
                    }
                    // zoom buttons follow the text and wrap onto a new line when narrow (never overlap it)
                    ui.separator();
                    ui.label(RichText::new(tr("Zoom")).small().color(GRAY));
                    if ui.small_button(tr("Whole")).clicked() {
                        fe.view = (0.0, fe.duration);
                    }
                    if ui.small_button(tr("Start")).clicked() {
                        fe.view = (0.0, 30.0f32.min(fe.duration));
                    }
                    if ui.small_button(tr("End")).clicked() {
                        fe.view = ((fe.duration - 30.0).max(0.0), fe.duration);
                    }
                    if ui.small_button(tr("Mix")).on_hover_text(tr("Zoom on the hand-over to the next track")).clicked() {
                        let m = fe.cues.mix;
                        fe.view = ((m - 12.0).max(0.0), (m + 10.0).min(fe.duration.max(m + 1.0)));
                    }
                });

                // ------------------------------------------------ canvas
                let avail = ui.available_size();
                let canvas_h = (avail.y - 190.0).clamp(150.0, 420.0);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(avail.x.max(200.0), canvas_h), Sense::click_and_drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 4.0, Color32::from_rgb(24, 28, 36));
                let ruler_h = 18.0;
                let lanes = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - ruler_h));
                let has_next = next.is_some();
                let split = if has_next { lanes.top() + lanes.height() * 0.58 } else { lanes.bottom() };
                let lane_a = Rect::from_min_max(lanes.min, Pos2::new(lanes.max.x, split - 2.0));
                let lane_b = Rect::from_min_max(Pos2::new(lanes.min.x, split + 2.0), lanes.max);
                let (v0, v1) = fe.view;
                let vspan = (v1 - v0).max(0.05);
                let to_x = |t: f32| rect.left() + (t - v0) / vspan * rect.width();
                let to_t = |x: f32| v0 + (x - rect.left()) / rect.width() * vspan;
                let c = fe.cues;

                if has_next {
                    p.text(
                        lane_a.left_top() + egui::vec2(6.0, 4.0),
                        Align2::LEFT_TOP,
                        "THIS TRACK",
                        FontId::proportional(11.0),
                        Color32::from_gray(150),
                    );
                    p.text(
                        lane_b.left_top() + egui::vec2(6.0, 4.0),
                        Align2::LEFT_TOP,
                        "NEXT TRACK",
                        FontId::proportional(11.0),
                        Color32::from_gray(150),
                    );
                    p.line_segment(
                        [Pos2::new(rect.left(), split), Pos2::new(rect.right(), split)],
                        Stroke::new(1.0_f32, Color32::from_gray(55)),
                    );
                }

                // waveforms: dim = raw audio, bright = what is heard after fades and level
                let cols = rect.width() as usize;
                let draw_lane = |lane: Rect, w: &Waveform, map: &dyn Fn(f32) -> f32, gain: &dyn Fn(f32) -> f32, color: Color32| {
                    let mid = lane.center().y;
                    let half = lane.height() * 0.46;
                    for x in 0..cols {
                        let ta = to_t(rect.left() + x as f32);
                        let tb = to_t(rect.left() + x as f32 + 1.0);
                        let (fa, fb) = (map(ta), map(tb));
                        if fb <= 0.0 || fa >= w.duration() {
                            continue;
                        }
                        let pk = peak_in(w, fa, fb).min(1.0);
                        if pk <= 0.0 {
                            continue;
                        }
                        let xx = rect.left() + x as f32 + 0.5;
                        let h = (pk.sqrt() * half).max(0.5);
                        p.line_segment([Pos2::new(xx, mid - h), Pos2::new(xx, mid + h)], Stroke::new(1.0_f32, color.gamma_multiply(0.28)));
                        let g = gain((fa + fb) * 0.5);
                        if g > 0.0 {
                            let hg = ((pk * g).min(1.0)).sqrt() * half;
                            p.line_segment([Pos2::new(xx, mid - hg), Pos2::new(xx, mid + hg)], Stroke::new(1.0_f32, color));
                        }
                    }
                };
                let lvl = 10f32.powf(c.gain_db / 20.0);
                if let Some(w) = &wave {
                    draw_lane(lane_a, w, &|t| t, &|t| c.gain_at(t) * lvl, Color32::from_rgb(70, 190, 190));
                }
                // next track on the same time axis: its start lines up with this track's mix point
                if let (Some(w), Some(nc)) = (&next_wave, next_cues) {
                    let off = c.mix - nc.cue_in;
                    let nl = 10f32.powf(nc.gain_db / 20.0);
                    draw_lane(lane_b, w, &|t| t - off, &|t| nc.gain_at(t) * nl, Color32::from_rgb(120, 220, 140));
                } else if has_next {
                    p.text(
                        lane_b.center(),
                        Align2::CENTER_CENTER,
                        "analysing next track…",
                        FontId::proportional(12.0),
                        Color32::from_gray(120),
                    );
                }

                // shade what is skipped (before START / after END) and the overlap region
                let shade = Color32::from_black_alpha(140);
                if c.cue_in > v0 {
                    p.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(rect.left(), lane_a.top()),
                            Pos2::new(to_x(c.cue_in).min(rect.right()), lane_a.bottom()),
                        ),
                        0.0,
                        shade,
                    );
                }
                if c.cue_out < v1 {
                    p.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(to_x(c.cue_out).max(rect.left()), lane_a.top()),
                            Pos2::new(rect.right(), lane_a.bottom()),
                        ),
                        0.0,
                        shade,
                    );
                }
                if has_next && c.cue_out > c.mix {
                    let ov = Rect::from_min_max(
                        Pos2::new(to_x(c.mix).max(rect.left()), lanes.top()),
                        Pos2::new(to_x(c.cue_out).min(rect.right()), lanes.bottom()),
                    );
                    p.rect_filled(ov, 0.0, Color32::from_rgba_unmultiplied(90, 150, 255, 22));
                }

                // volume envelope
                let env_y = |g: f32| lane_a.bottom() - 4.0 - g.clamp(0.0, 1.0) * (lane_a.height() - 24.0);
                let pts = vec![
                    Pos2::new(to_x(c.cue_in), env_y(0.0)),
                    Pos2::new(to_x(c.cue_in + c.fade_in), env_y(if c.fade_in > 0.0 { 1.0 } else { 1.0 })),
                    Pos2::new(to_x(c.fade_out), env_y(1.0)),
                    Pos2::new(to_x(c.cue_out), env_y(if c.cue_out > c.fade_out { 0.0 } else { 1.0 })),
                ];
                p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, Color32::from_rgb(255, 200, 80))));

                // markers
                let pointer = resp.hover_pos();
                let mut hover_mark = None;
                for i in 0..7 {
                    if !mark_visible(&c, i) {
                        continue;
                    }
                    let t = mark_time(&c, i);
                    let x = to_x(t);
                    if x < rect.left() - 1.0 || x > rect.right() + 1.0 {
                        continue;
                    }
                    let col = mark_color(i);
                    let bottom = if i == 3 { lanes.bottom() } else { lane_a.bottom() };
                    let w = if fe.drag == Some(i) { 2.5_f32 } else { 1.5_f32 };
                    p.line_segment([Pos2::new(x, lane_a.top()), Pos2::new(x, bottom)], Stroke::new(w, col));
                    let label_y = lane_a.top() + 18.0 + (i as f32) * 14.0;
                    let galley = p.layout_no_wrap(MARK_NAMES[i].to_string(), FontId::proportional(10.0), Color32::BLACK);
                    let lw = galley.size().x + 8.0;
                    let lx = if x + lw > rect.right() { x - lw } else { x };
                    let lr = Rect::from_min_size(Pos2::new(lx, label_y), egui::vec2(lw, 13.0));
                    p.rect_filled(lr, 3.0, col);
                    p.galley(lr.min + egui::vec2(4.0, 0.0), galley, Color32::BLACK);
                    if let Some(pp) = pointer {
                        if (pp.x - x).abs() < 7.0 && (i == 3 || pp.y < lane_a.bottom()) {
                            hover_mark = Some(i);
                        }
                    }
                }

                // edit cursor and preview playhead
                let cx = to_x(fe.cursor);
                if cx >= rect.left() && cx <= rect.right() {
                    p.line_segment(
                        [Pos2::new(cx, lanes.top()), Pos2::new(cx, lanes.bottom())],
                        Stroke::new(1.0_f32, Color32::from_white_alpha(120)),
                    );
                }
                if let Some(pv) = &self.preview {
                    let (pos, cur) = pv.position();
                    let x = if cur.as_deref() == Some(fe.path.as_path()) {
                        Some(to_x(pos))
                    } else if let (Some((np, _, _)), Some(nc)) = (&next, next_cues) {
                        (cur.as_deref() == Some(np.as_path())).then(|| to_x(c.mix + pos - nc.cue_in))
                    } else {
                        None
                    };
                    if let Some(x) = x {
                        p.line_segment([Pos2::new(x, lanes.top()), Pos2::new(x, lanes.bottom())], Stroke::new(2.0_f32, Color32::WHITE));
                    }
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
                }

                // ruler
                let steps = [0.5f32, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0];
                let step = steps.iter().copied().find(|s| vspan / s <= (rect.width() / 70.0).max(2.0)).unwrap_or(300.0);
                let mut tt = (v0 / step).ceil() * step;
                while tt <= v1 {
                    let x = to_x(tt);
                    p.line_segment(
                        [Pos2::new(x, rect.bottom() - ruler_h), Pos2::new(x, rect.bottom() - ruler_h + 5.0)],
                        Stroke::new(1.0_f32, Color32::from_gray(140)),
                    );
                    p.text(
                        Pos2::new(x + 2.0, rect.bottom() - 2.0),
                        Align2::LEFT_BOTTOM,
                        mmss(tt),
                        FontId::monospace(10.0),
                        Color32::from_gray(170),
                    );
                    tt += step;
                }

                // ------------------------------------------------ interaction
                if hover_mark.is_some() || fe.drag.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if resp.drag_started() {
                    fe.drag = hover_mark;
                }
                if let (Some(i), Some(pp)) = (fe.drag, resp.interact_pointer_pos()) {
                    if resp.dragged() {
                        set_mark(&mut fe.cues, i, to_t(pp.x).clamp(0.0, fe.duration), fe.duration);
                        fe.edited = true;
                    }
                }
                if resp.drag_stopped() {
                    fe.drag = None;
                }
                if resp.clicked() && hover_mark.is_none() {
                    if let Some(pp) = resp.interact_pointer_pos() {
                        fe.cursor = to_t(pp.x).clamp(0.0, fe.duration);
                    }
                }
                if resp.double_clicked() {
                    // double-click: play from here
                    self.preview = None;
                    let a = PreviewSide { path: fe.path.clone(), duration: fe.duration, cues: fe.cues };
                    if let Ok(pv) = PreviewPlayer::start(&core.cfg.audio_devices.preview, a, None, fe.cursor, 0.0) {
                        self.preview = Some(pv);
                    }
                }
                if resp.hovered() {
                    let (scroll, zoom, hx, ctrl) =
                        ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos().map(|p| p.x), i.modifiers.ctrl));
                    let anchor = hx.map(to_t).unwrap_or((v0 + v1) * 0.5);
                    let mut z = zoom;
                    if ctrl && scroll.y != 0.0 {
                        z = (1.0 + scroll.y * 0.003).clamp(0.5, 2.0);
                    }
                    if (z - 1.0).abs() > 1e-3 {
                        let ns = (vspan / z).clamp(1.0, fe.duration.max(1.0));
                        let f = (anchor - v0) / vspan;
                        let a = (anchor - f * ns).clamp(0.0, (fe.duration - ns).max(0.0));
                        fe.view = (a, a + ns);
                    } else if scroll.x != 0.0 || (scroll.y != 0.0 && !ctrl) {
                        let d = -(if scroll.x != 0.0 { scroll.x } else { scroll.y }) / rect.width() * vspan;
                        let a = (v0 + d).clamp(0.0, (fe.duration - vspan).max(0.0));
                        fe.view = (a, a + vspan);
                    }
                }

                // ------------------------------------------------ transport
                ui.horizontal(|ui| {
                    if self.preview.is_some() {
                        if ui.button(tr("⏹ Stop (F12)")).clicked() {
                            self.preview = None;
                        }
                    } else {
                        if ui
                            .button("▶ Play")
                            .on_hover_text(
                                "Play from the cursor on the preview device (not on air). Double-click the waveform to play from there.",
                            )
                            .clicked()
                        {
                            let a = PreviewSide { path: fe.path.clone(), duration: fe.duration, cues: fe.cues };
                            match PreviewPlayer::start(&core.cfg.audio_devices.preview, a, None, fe.cursor.max(fe.cues.cue_in), 0.0) {
                                Ok(pv) => self.preview = Some(pv),
                                Err(e) => fe.msg = format!("Preview failed: {e}"),
                            }
                        }
                        if ui
                            .add_enabled(has_next, egui::Button::new(tr("▶ Preview mix")))
                            .on_hover_text(tr("Last 6 s of this track into the next one, with the fades as set"))
                            .clicked()
                        {
                            let a = PreviewSide { path: fe.path.clone(), duration: fe.duration, cues: fe.cues };
                            let b = next.as_ref().map(|(p, _, d)| PreviewSide {
                                path: p.clone(),
                                duration: *d,
                                cues: next_cues.unwrap_or_else(|| Cues::plain(*d)),
                            });
                            match PreviewPlayer::start(&core.cfg.audio_devices.preview, a, b, fe.cues.mix - 6.0, 6.0) {
                                Ok(pv) => self.preview = Some(pv),
                                Err(e) => fe.msg = format!("Preview failed: {e}"),
                            }
                        }
                    }
                    let t = playhead.unwrap_or(fe.cursor);
                    ui.label(RichText::new(mmss(t)).monospace().strong());
                    if let Some((_, nt, _)) = &next {
                        ui.label(RichText::new(format!("next: {nt}")).small().color(GRAY));
                    }
                });

                // ------------------------------------------------ cue points (RadioBOSS-style grid)
                let set_from = playhead.unwrap_or(fe.cursor);
                let d = fe.duration;
                let mut cc = fe.cues;
                let mut changed = false;
                egui::Grid::new("te_points").num_columns(8).spacing([8.0, 4.0]).show(ui, |ui| {
                    let order = [[0usize, 1], [5, 3], [6, 2], [4, 99]]; // START·FADE IN / INTRO·MIX / OUTRO·FADE OUT / END
                    for row in order {
                        for i in row {
                            if i == 99 {
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                continue;
                            }
                            ui.label(RichText::new(MARK_NAMES[i]).small().strong().color(mark_color(i)));
                            let mut v = if i == 1 { cc.fade_in } else { mark_time(&cc, i) };
                            let r = ui.add(egui::DragValue::new(&mut v).speed(0.05).range(0.0..=d).fixed_decimals(2).suffix(" s"));
                            if r.changed() {
                                if i == 1 {
                                    cc.fade_in = v;
                                    cc.clamp(d);
                                } else {
                                    set_mark(&mut cc, i, v, d);
                                }
                                changed = true;
                            }
                            if ui.small_button(tr("Set")).on_hover_text(tr("Set to the playhead / cursor position")).clicked() {
                                set_mark(&mut cc, i, set_from, d);
                                changed = true;
                            }
                            if (i == 5 || i == 6) && ui.small_button("✖").on_hover_text(tr("Clear")).clicked() {
                                if i == 5 {
                                    cc.intro = 0.0
                                } else {
                                    cc.outro = 0.0
                                }
                                changed = true;
                            } else if !(i == 5 || i == 6) {
                                ui.label("");
                            }
                        }
                        ui.end_row();
                    }
                });
                if changed {
                    fe.cues = cc;
                    fe.edited = true;
                }

                ui.horizontal_wrapped(|ui| {
                    ui.label(tr("Playback level"));
                    if ui.add(egui::Slider::new(&mut fe.cues.gain_db, -12.0..=12.0).suffix(" dB").fixed_decimals(1)).changed() {
                        fe.edited = true;
                    }
                    if ui
                        .add_enabled(wave.is_some(), egui::Button::new(tr("Detect")))
                        .on_hover_text(tr("Level the track to a common loudness"))
                        .clicked()
                    {
                        if let Some(w) = &wave {
                            fe.cues.gain_db = detect_level(w, -16.0);
                            fe.edited = true;
                        }
                    }
                    ui.separator();
                    ui.label(tr("BPM"));
                    if ui.add(egui::DragValue::new(&mut fe.cues.bpm).range(0.0..=300.0).fixed_decimals(1)).changed() {
                        fe.edited = true;
                    }
                    if ui.add_enabled(wave.is_some(), egui::Button::new(tr("Detect"))).clicked() {
                        if let Some(b) = wave.as_deref().and_then(detect_bpm) {
                            fe.cues.bpm = b;
                            fe.edited = true;
                        } else {
                            fe.msg = "Couldn't find a steady beat".into();
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(wave.is_some(), egui::Button::new(tr("✨ Smart cues")))
                        .on_hover_text(tr("Work out START / END / MIX / FADE automatically"))
                        .clicked()
                    {
                        if let Some(w) = &wave {
                            let keep = fe.cues;
                            let mut sc = smart_cues(w, &core.cfg.fade);
                            sc.intro = keep.intro;
                            sc.outro = keep.outro;
                            sc.gain_db = keep.gain_db;
                            sc.bpm = keep.bpm;
                            fe.cues = sc;
                            fe.edited = true;
                        }
                    }
                    if ui.button(tr("↺ Automatic")).on_hover_text(tr("Forget the hand-set cues for this track")).clicked() {
                        core.clear_manual_cues(&fe.path);
                        core.save_all();
                        fe.edited = false;
                        fe.have_cues = false;
                        fe.cues = core.cues.get(&fe.path).copied().unwrap_or_else(|| Cues::plain(fe.duration));
                    }
                });
                ui.label(
                RichText::new(
                    "Drag the markers · click to place the cursor · double-click to play from there · scroll to pan · Ctrl+scroll to zoom",
                )
                .small()
                .color(GRAY),
            );
            });
        });

        // ================= navigation / close
        if let (Some(dir), EditSrc::Library(Some(li))) = (nav, fe.src) {
            // step through the library list as it is shown (search filter included)
            Self::apply_track_edit(&mut fe, core);
            if let Some(p) = self.lib_filtered.iter().position(|&x| x == li) {
                let j = p as i64 + dir;
                if j >= 0 && (j as usize) < self.lib_filtered.len() {
                    let lj = self.lib_filtered[j as usize];
                    if let Some(t) = core.library.get(lj).cloned() {
                        self.fade_edit = None;
                        self.lib_sel = [lj].into();
                        self.lib_anchor = Some(lj);
                        self.lib_scroll = Some(lj);
                        self.te_scroll = true;
                        self.open_track_editor_for(core, t, Some(lj));
                        return;
                    }
                }
            }
        }
        if let Some(dir) = nav {
            if let EditSrc::Playlist(i) = fe.src {
                Self::apply_track_edit(&mut fe, core);
                let j = i as i64 + dir;
                if j >= 0 && (j as usize) < core.playlist.len() {
                    self.fade_edit = None;
                    self.open_fade_editor(core, Some(j as usize));
                    self.sel.clear();
                    self.sel.insert(j as usize);
                    return;
                }
            }
        }
        if close {
            fe.msg = "\u{0}close".into();
        }
        self.fade_edit = Some(fe);
    }
}
