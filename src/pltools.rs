//! Playlist menu tools (RadioBOSS "Playlist" menu): add URL / line input / command / timed pause /
//! teaser / time announcement / comment / container / track list / hour marker, voice tracks
//! (insert or record), insert playlist, insert tracks from playlist, Extra, Notepad and Find.

use super::*;
use crate::items::{ItemKind, PlEntry};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DlgKind {
    Url,
    LineIn,
    Command,
    Pause,
    Teaser,
    TimeAnnounce,
    Comment,
    Container,
    TrackList,
    HourMarker,
}

impl DlgKind {
    fn title(self) -> &'static str {
        match self {
            DlgKind::Url => "Add URL",
            DlgKind::LineIn => "Add Line Input",
            DlgKind::Command => "Add command",
            DlgKind::Pause => "Add timed pause",
            DlgKind::Teaser => "Add teaser",
            DlgKind::TimeAnnounce => "Add time announcement",
            DlgKind::Comment => "Add comment",
            DlgKind::Container => "Add container",
            DlgKind::TrackList => "Add Track List",
            DlgKind::HourMarker => "Add hour marker",
        }
    }
}

/// An "Add …" / "Edit …" dialog for a special playlist row.
pub(super) struct ItemDlg {
    pub kind: DlgKind,
    /// row being edited (in `tab`), or None to insert a new one
    pub edit: Option<usize>,
    pub tab: usize,
    pub title: String,
    pub text: String,
    pub mins: u32,
    pub secs: u32,
    pub count: u32,
    pub hour: u32,
    pub minute: u32,
    pub hard: bool,
    pub wait: bool,
    pub items: Vec<PathBuf>,
    pub random: bool,
    pub err: String,
}

/// Recording a voice track from the input device.
pub(super) struct VoiceRec {
    stream: Option<rodio::cpal::Stream>,
    buf: Arc<Mutex<Vec<f32>>>,
    peak: Arc<AtomicU32>,
    started: std::time::Instant,
    pub file: Option<PathBuf>,
    pub err: String,
}

impl VoiceRec {
    pub(super) fn new() -> Self {
        VoiceRec {
            stream: None,
            buf: Arc::new(Mutex::new(vec![])),
            peak: Arc::new(AtomicU32::new(0)),
            started: std::time::Instant::now(),
            file: None,
            err: String::new(),
        }
    }
}

const COMMAND_HELP: &str =
    "STOP · PAUSE · VOLUME 80 · ENCODER START 1 · ENCODER STOP 1 · LOAD C:\\Shows\\news.m3u · PLAYLIST Evening · RUN program arguments";

impl App {
    fn new_item_dlg(&self, kind: DlgKind) -> ItemDlg {
        let next_hour = (Local::now().hour() + 1) % 24;
        ItemDlg {
            kind,
            edit: None,
            tab: self.view_pl,
            title: String::new(),
            text: String::new(),
            mins: if kind == DlgKind::Pause { 0 } else { 5 },
            secs: if kind == DlgKind::Pause { 30 } else { 0 },
            count: 3,
            hour: next_hour,
            minute: 0,
            hard: false,
            wait: true,
            items: vec![],
            random: false,
            err: String::new(),
        }
    }

    pub(super) fn open_item_dlg(&mut self, kind: DlgKind) {
        let mut d = self.new_item_dlg(kind);
        if kind == DlgKind::Teaser {
            d.secs = 8;
        }
        if kind == DlgKind::LineIn {
            d.mins = 0;
        }
        self.item_dlg = Some(d);
    }

    /// Open the dialog for an existing special row so it can be changed.
    pub(super) fn edit_item(&mut self, row: usize, e: &PlEntry) {
        let split = |s: f32| ((s.max(0.0) as u32) / 60, (s.max(0.0) as u32) % 60);
        let mut d = match &e.kind {
            ItemKind::Url { url, secs } => {
                let mut d = self.new_item_dlg(DlgKind::Url);
                d.text = url.clone();
                (d.mins, d.secs) = split(*secs);
                d
            }
            ItemKind::LineIn { secs } => {
                let mut d = self.new_item_dlg(DlgKind::LineIn);
                (d.mins, d.secs) = split(*secs);
                d
            }
            ItemKind::Command(c) => {
                let mut d = self.new_item_dlg(DlgKind::Command);
                d.text = c.clone();
                d
            }
            ItemKind::Pause(s) => {
                let mut d = self.new_item_dlg(DlgKind::Pause);
                (d.mins, d.secs) = split(*s);
                d
            }
            ItemKind::Teaser { count, secs } => {
                let mut d = self.new_item_dlg(DlgKind::Teaser);
                d.count = *count;
                d.secs = *secs as u32;
                d
            }
            ItemKind::Comment(t) => {
                let mut d = self.new_item_dlg(DlgKind::Comment);
                d.text = t.clone();
                d
            }
            ItemKind::Container { items, random } => {
                let mut d = self.new_item_dlg(DlgKind::Container);
                d.items = items.clone();
                d.random = *random;
                d
            }
            ItemKind::TrackList { source } => {
                let mut d = self.new_item_dlg(DlgKind::TrackList);
                d.text = source.clone();
                d
            }
            ItemKind::HourMarker { hour, minute, hard, wait } => {
                let mut d = self.new_item_dlg(DlgKind::HourMarker);
                (d.hour, d.minute, d.hard, d.wait) = (*hour, *minute, *hard, *wait);
                d
            }
            ItemKind::TimeAnnounce => self.new_item_dlg(DlgKind::TimeAnnounce),
            ItemKind::Track | ItemKind::VoiceTrack => return,
        };
        d.title = e.track.title.clone();
        d.edit = Some(row);
        self.item_dlg = Some(d);
    }

    /// Build the playlist row a dialog describes.
    fn item_from_dlg(d: &ItemDlg) -> Result<PlEntry, String> {
        let dur = (d.mins * 60 + d.secs) as f32;
        let title = d.title.trim().to_string();
        Ok(match d.kind {
            DlgKind::Url => {
                let url = d.text.trim();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err("Enter a stream address starting with http:// or https://".into());
                }
                PlEntry::special(ItemKind::Url { url: url.to_string(), secs: dur }, title, dur)
            }
            DlgKind::LineIn => PlEntry::special(ItemKind::LineIn { secs: dur }, title, dur),
            DlgKind::Command => {
                if d.text.trim().is_empty() {
                    return Err("Type a command".into());
                }
                PlEntry::special(ItemKind::Command(d.text.trim().to_string()), title, 0.0)
            }
            DlgKind::Pause => {
                if dur <= 0.0 {
                    return Err("A pause needs a length".into());
                }
                PlEntry::special(ItemKind::Pause(dur), title, dur)
            }
            DlgKind::Teaser => {
                let secs = d.secs.clamp(2, 60) as f32;
                PlEntry::special(ItemKind::Teaser { count: d.count.clamp(1, 10), secs }, title, d.count as f32 * secs)
            }
            DlgKind::TimeAnnounce => PlEntry::special(ItemKind::TimeAnnounce, title, 4.0),
            DlgKind::Comment => {
                if d.text.trim().is_empty() {
                    return Err("Type the comment".into());
                }
                PlEntry::special(ItemKind::Comment(d.text.trim().to_string()), title, 0.0)
            }
            DlgKind::Container => {
                if d.items.is_empty() {
                    return Err("Add at least one file".into());
                }
                let durs: Vec<f32> = d.items.iter().map(|p| crate::library::Track::from_path(p).duration).collect();
                let total: f32 = durs.iter().sum();
                let secs = if d.random { total / durs.len() as f32 } else { total };
                PlEntry::special(
                    ItemKind::Container { items: d.items.clone(), random: d.random },
                    if title.is_empty() { "Container".into() } else { title },
                    secs,
                )
            }
            DlgKind::TrackList => {
                let src = d.text.trim();
                let p = std::path::Path::new(src);
                let files = if p.is_dir() { crate::library::audio_files_in(p) } else { crate::playout::m3u_paths(p) };
                if files.is_empty() {
                    return Err("Choose a folder (or .m3u) that contains audio files".into());
                }
                let sample: Vec<f32> = files
                    .iter()
                    .step_by((files.len() / 25).max(1))
                    .map(|f| crate::library::Track::from_path(f).duration)
                    .filter(|d| *d > 0.0)
                    .collect();
                let avg = if sample.is_empty() { 210.0 } else { sample.iter().sum::<f32>() / sample.len() as f32 };
                PlEntry::special(ItemKind::TrackList { source: src.to_string() }, title, avg)
            }
            DlgKind::HourMarker => PlEntry::special(
                ItemKind::HourMarker { hour: d.hour.min(23), minute: d.minute.min(59), hard: d.hard, wait: d.wait },
                title,
                0.0,
            ),
        })
    }

    fn mins_secs(ui: &mut egui::Ui, d: &mut ItemDlg, zero_label: &str) {
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut d.mins).range(0..=600).suffix(" min"));
            ui.add(egui::DragValue::new(&mut d.secs).range(0..=59).suffix(" s"));
            if d.mins == 0 && d.secs == 0 && !zero_label.is_empty() {
                ui.label(RichText::new(zero_label).small().color(GRAY));
            }
        });
    }

    pub(super) fn item_dialog_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some(mut d) = self.item_dlg.take() else { return };
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        let title = if d.edit.is_some() { d.kind.title().replace("Add", "Edit") } else { d.kind.title().to_string() };
        Self::window_for(ctx, &title, [460.0, 300.0]).id(egui::Id::new("item_dlg")).collapsible(false).open(&mut open).show(ctx, |ui| {
            egui::Grid::new("item_dlg_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                let name_row = |ui: &mut egui::Ui, d: &mut ItemDlg, hint: &str| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut d.title).hint_text(hint).desired_width(300.0));
                    ui.end_row();
                };
                match d.kind {
                    DlgKind::Url => {
                        ui.label("Stream address");
                        ui.add(egui::TextEdit::singleline(&mut d.text).hint_text("https://radio.example.com/live.mp3").desired_width(300.0));
                        ui.end_row();
                        name_row(ui, &mut d, "shown in the playlist (optional)");
                        ui.label("Play for");
                        Self::mins_secs(ui, &mut d, "until the stream ends / Next");
                        ui.end_row();
                    }
                    DlgKind::LineIn => {
                        ui.label("Input");
                        let dev = core.cfg.audio_devices.input.clone();
                        ui.label(if dev.is_empty() { "System default input (Settings → Audio devices)".to_string() } else { dev });
                        ui.end_row();
                        ui.label("Play for");
                        Self::mins_secs(ui, &mut d, "until Next is pressed");
                        ui.end_row();
                        name_row(ui, &mut d, "e.g. Studio 2 live");
                    }
                    DlgKind::Command => {
                        ui.label("Command");
                        ui.add(egui::TextEdit::singleline(&mut d.text).hint_text("STOP").desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(COMMAND_HELP).small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::Pause => {
                        ui.label("Silence for");
                        Self::mins_secs(ui, &mut d, "");
                        ui.end_row();
                    }
                    DlgKind::Teaser => {
                        ui.label("Upcoming tracks");
                        ui.add(egui::DragValue::new(&mut d.count).range(1..=10));
                        ui.end_row();
                        ui.label("Seconds of each");
                        ui.add(egui::DragValue::new(&mut d.secs).range(2..=60).suffix(" s"));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new("Plays a short faded snippet from each of the next tracks after this row.").small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::TimeAnnounce => {
                        let a = &mut core.cfg.announce;
                        ui.label("Spoken text");
                        ui.add(egui::TextEdit::singleline(&mut a.template).desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(format!("Now: \"{}\"  ·  {{h12}} {{h24}} {{mm}} {{ampm}} {{time}}", crate::live::announce_text(&a.template, &Local::now()))).small().color(GRAY));
                        ui.end_row();
                        ui.label("Voice");
                        if self.voices.is_empty() {
                            ui.add(egui::TextEdit::singleline(&mut a.voice).hint_text("default voice").desired_width(300.0));
                        } else {
                            egui::ComboBox::from_id_salt("tts_voice").width(300.0).selected_text(if a.voice.is_empty() { "Default".to_string() } else { a.voice.clone() }).show_ui(ui, |ui| {
                                ui.selectable_value(&mut a.voice, String::new(), "Default");
                                for v in &self.voices {
                                    ui.selectable_value(&mut a.voice, v.clone(), v);
                                }
                            });
                        }
                        ui.end_row();
                        ui.label("Recordings");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut a.folder).hint_text("optional folder").desired_width(220.0));
                            if ui.button("…").clicked() {
                                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::AnnounceFolder));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new("With a folder of recordings named H00…H23 and M00…M59 those are played; otherwise the computer's speech voice reads the text.").small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::Comment => {
                        ui.label("Comment");
                        ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(3).desired_width(300.0));
                        ui.end_row();
                    }
                    DlgKind::Container => {
                        name_row(ui, &mut d, "e.g. Station IDs");
                        ui.label("Plays");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.random, false, "all files in order");
                            ui.radio_value(&mut d.random, true, "one file at random");
                        });
                        ui.end_row();
                        ui.label("Files");
                        ui.vertical(|ui| {
                            let mut del = None;
                            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                                for (k, p) in d.items.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("✖").clicked() {
                                            del = Some(k);
                                        }
                                        ui.label(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                                    });
                                }
                            });
                            if let Some(k) = del {
                                d.items.remove(k);
                            }
                            if ui.button("Add files…").clicked() {
                                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::ContainerFiles));
                            }
                        });
                        ui.end_row();
                    }
                    DlgKind::TrackList => {
                        name_row(ui, &mut d, "e.g. Recurrent");
                        ui.label("Source");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.text).hint_text("folder or .m3u").desired_width(200.0));
                            if ui.button("Folder…").clicked() {
                                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::TrackListSource));
                            }
                            if ui.button("M3U…").clicked() {
                                self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::TrackListSource));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        let r = core.cfg.repeat;
                        ui.label(
                            RichText::new(format!(
                                "Each time this row is reached one track is picked at random — not one played in the last {} min, nor an artist from the last {} min (Track List Repeat Protection).",
                                r.track_mins, r.artist_mins
                            ))
                            .small()
                            .color(GRAY),
                        );
                        ui.end_row();
                    }
                    DlgKind::HourMarker => {
                        ui.label("Time");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut d.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                            ui.label(":");
                            ui.add(egui::DragValue::new(&mut d.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                        });
                        ui.end_row();
                        ui.label("If early");
                        ui.checkbox(&mut d.wait, "wait (silence) until the time");
                        ui.end_row();
                        ui.label("If late");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut d.hard, false, "soft: finish the current track, then jump here");
                            ui.radio_value(&mut d.hard, true, "hard: cut in exactly on time");
                        });
                        ui.end_row();
                    }
                }
            });
            if !d.err.is_empty() {
                ui.colored_label(RED, &d.err);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.button(RichText::new("  OK  ").strong()).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if ok {
            match Self::item_from_dlg(&d) {
                Ok(e) => {
                    match d.edit {
                        Some(row) => {
                            core.with_tab(d.tab, |c| {
                                if let Some(r) = c.playlist.get_mut(row) {
                                    let played = r.played;
                                    *r = e;
                                    r.played = played;
                                }
                            });
                        }
                        None => {
                            self.view_pl = d.tab;
                            self.insert_rows_here(core, vec![e]);
                        }
                    }
                    return;
                }
                Err(msg) => d.err = msg,
            }
        }
        if open && !cancel {
            self.item_dlg = Some(d);
        }
    }

    // ------------------------------------------------- insert from playlist
    pub(super) fn insert_from_playlist_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some((path, rows, mut pick)) = self.ins_from_pl.take() else { return };
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        Self::window_for(ctx, "Insert tracks from playlist", [520.0, 480.0]).id(egui::Id::new("ins_from_pl")).open(&mut open).show(
            ctx,
            |ui| {
                ui.label(RichText::new(path.display().to_string()).small().color(GRAY));
                ui.horizontal(|ui| {
                    if ui.small_button("Select all").clicked() {
                        pick.iter_mut().for_each(|p| *p = true);
                    }
                    if ui.small_button("Select none").clicked() {
                        pick.iter_mut().for_each(|p| *p = false);
                    }
                    ui.label(format!("{} of {} selected", pick.iter().filter(|p| **p).count(), rows.len()));
                });
                egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                    for (k, e) in rows.iter().enumerate() {
                        ui.checkbox(&mut pick[k], format!("{}  [{}]", e.title(), e.length_text()));
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ok = ui.button(RichText::new("Insert").strong()).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            },
        );
        if ok {
            let chosen: Vec<PlEntry> = rows.iter().zip(pick.iter()).filter(|(_, p)| **p).map(|(e, _)| e.fresh()).collect();
            self.insert_rows_here(core, chosen);
            return;
        }
        if open && !cancel {
            self.ins_from_pl = Some((path, rows, pick));
        }
    }

    // ------------------------------------------------------ voice tracks
    pub(super) fn voice_track_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_vt {
            if let Some(r) = self.vt.as_mut() {
                r.stream = None;
            }
            return;
        }
        let vt = self.vt.get_or_insert_with(VoiceRec::new);
        let recording = vt.stream.is_some();
        if recording {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let mut open = true;
        let mut insert = None;
        let device = core.cfg.audio_devices.input.clone();
        Self::window_for(ctx, "Record voice track", [430.0, 260.0]).id(egui::Id::new("vt_window")).collapsible(false).open(&mut open).show(
            ctx,
            |ui| {
                ui.label(format!("Input: {}", if device.is_empty() { "system default" } else { &device }));
                let secs = if recording {
                    vt.started.elapsed().as_secs_f32()
                } else {
                    vt.buf.lock().len() as f32 / (2.0 * crate::engine::SR as f32)
                };
                ui.label(RichText::new(fmt(secs)).monospace().size(28.0).strong().color(if recording {
                    RED
                } else {
                    ui.visuals().text_color()
                }));
                let peak = f32::from_bits(vt.peak.swap(0, Ordering::Relaxed));
                vu(ui, if recording { peak } else { 0.0 }, ui.available_width().min(380.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if recording {
                        if ui.button(RichText::new("■ Stop").strong()).clicked() {
                            vt.stream = None;
                            let dir = data_dir().join("voicetracks");
                            let _ = std::fs::create_dir_all(&dir);
                            let f = dir.join(format!("VT_{}.wav", Local::now().format("%Y%m%d_%H%M%S")));
                            let samples = vt.buf.lock().clone();
                            match crate::engine::write_wav(&f, &samples, 2, crate::engine::SR) {
                                Ok(()) => vt.file = Some(f),
                                Err(e) => vt.err = format!("Could not save: {e}"),
                            }
                        }
                    } else if ui.button(RichText::new("● Record").strong().color(RED)).clicked() {
                        vt.buf.lock().clear();
                        vt.file = None;
                        vt.err.clear();
                        let (buf, peak) = (vt.buf.clone(), vt.peak.clone());
                        match crate::mic::capture(&device, move |s| {
                            let p = s.iter().fold(0f32, |m, x| m.max(x.abs()));
                            let old = f32::from_bits(peak.load(Ordering::Relaxed));
                            peak.store(old.max(p).to_bits(), Ordering::Relaxed);
                            buf.lock().extend_from_slice(s);
                        }) {
                            Ok(s) => {
                                vt.stream = Some(s);
                                vt.started = std::time::Instant::now();
                            }
                            Err(e) => vt.err = format!("Input device: {e}"),
                        }
                    }
                    if let Some(f) = vt.file.clone() {
                        if ui.button("Insert after selected row").clicked() {
                            insert = Some(f.clone());
                        }
                        ui.label(
                            RichText::new(f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).small().color(GRAY),
                        );
                    }
                });
                if !vt.err.is_empty() {
                    ui.colored_label(RED, &vt.err);
                }
                ui.small("Recordings are saved as WAV in the voicetracks folder of your RadioMaster data folder.");
            },
        );
        if let Some(f) = insert {
            let mut e = core.entry_for(&f);
            e.kind = ItemKind::VoiceTrack;
            if e.track.title.starts_with("VT_") {
                e.track.title = format!("Voice track {}", Local::now().format("%H:%M"));
            }
            self.insert_rows_here(core, vec![e]);
            if let Some(v) = self.vt.as_mut() {
                v.file = None;
            }
        }
        if !open {
            self.show_vt = false;
            if let Some(r) = self.vt.as_mut() {
                r.stream = None;
            }
        }
    }

    // --------------------------------------------- track list repeat protection
    pub(super) fn repeat_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_repeat {
            return;
        }
        let mut open = true;
        Self::window_for(ctx, "Track List Repeat Protection", [420.0, 200.0])
            .id(egui::Id::new("repeat_window"))
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("When a Track List row picks a track, skip:");
                egui::Grid::new("repeat_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("the same track played in the last");
                    ui.add(egui::DragValue::new(&mut core.cfg.repeat.track_mins).range(0..=10080).suffix(" min"));
                    ui.end_row();
                    ui.label("the same artist played in the last");
                    ui.add(egui::DragValue::new(&mut core.cfg.repeat.artist_mins).range(0..=1440).suffix(" min"));
                    ui.end_row();
                });
                ui.small("If every track has played recently, the least recently played ones are used.");
            });
        self.show_repeat = open;
    }

    // ---------------------------------------------------------------- find
    pub(super) fn find_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_find {
            return;
        }
        let mut open = true;
        let mut jump = None;
        let view = self.view_pl;
        let rows: Vec<(usize, String)> = core.tab_rows(view).iter().enumerate().map(|(i, e)| (i, e.title())).collect();
        Self::window_for(ctx, "Find", [440.0, 380.0]).id(egui::Id::new("find_window")).open(&mut open).show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.find_text).hint_text("text in the title").desired_width(f32::INFINITY));
            if self.find_focus {
                r.request_focus();
                self.find_focus = false;
            }
            let f = self.find_text.to_lowercase();
            let hits: Vec<&(usize, String)> =
                if f.is_empty() { vec![] } else { rows.iter().filter(|(_, t)| t.to_lowercase().contains(&f)).collect() };
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !hits.is_empty() {
                // Enter: next match after the selection
                let after = self.sel.iter().next().copied();
                let next = hits.iter().find(|(i, _)| after.is_none_or(|a| *i > a)).or(hits.first()).map(|(i, _)| *i);
                jump = next;
                self.find_focus = true;
            }
            ui.label(
                RichText::new(format!("{} match(es) in {}", hits.len(), core.tabs.get(view).map(|t| t.name.as_str()).unwrap_or("")))
                    .small()
                    .color(GRAY),
            );
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, t) in hits {
                    if ui.selectable_label(self.sel.contains(i), format!("{}. {}", i + 1, t)).clicked() {
                        jump = Some(*i);
                    }
                }
            });
        });
        if let Some(i) = jump {
            self.sel.clear();
            self.sel.insert(i);
            self.anchor = Some(i);
            self.scroll_to = Some(i);
        }
        self.show_find = open;
    }

    // ------------------------------------------------------- the menu itself
    pub(super) fn playlist_menu(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        fn item(ui: &mut egui::Ui, text: &str, sc: &str) -> bool {
            let mut b = egui::Button::new(text);
            if !sc.is_empty() {
                b = b.shortcut_text(sc);
            }
            if ui.add(b).clicked() {
                ui.close_menu();
                true
            } else {
                false
            }
        }
        if item(ui, "🎲 Playlist Generator Pro…", "Ctrl+G") {
            self.open_tool(ToolWin::Generator);
        }
        ui.separator();
        if item(ui, "Add file…", "Alt+F") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AddFiles));
        }
        if item(ui, "Add folder…", "Alt+D") {
            self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::AddFolder));
        }
        if item(ui, "Add URL…", "Alt+U") {
            self.open_item_dlg(DlgKind::Url);
        }
        if item(ui, "Add Line Input…", "") {
            self.open_item_dlg(DlgKind::LineIn);
        }
        if item(ui, "Add command…", "Alt+C") {
            self.open_item_dlg(DlgKind::Command);
        }
        if item(ui, "Add timed pause…", "Ctrl+P") {
            self.open_item_dlg(DlgKind::Pause);
        }
        if item(ui, "Add teaser…", "") {
            self.open_item_dlg(DlgKind::Teaser);
        }
        if item(ui, "Add time announcement…", "") {
            if self.voices.is_empty() {
                self.voices = crate::live::list_voices();
            }
            self.open_item_dlg(DlgKind::TimeAnnounce);
        }
        if item(ui, "Add comment…", "Alt+W") {
            self.open_item_dlg(DlgKind::Comment);
        }
        if item(ui, "Add container…", "") {
            self.open_item_dlg(DlgKind::Container);
        }
        ui.separator();
        if item(ui, "Insert voice track…", "Ctrl+W") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::InsertVoice));
        }
        if item(ui, "🔴 Record voice track…", "Ctrl+R") {
            self.show_vt = true;
        }
        ui.separator();
        if item(ui, "Add Track List…", "") {
            self.open_item_dlg(DlgKind::TrackList);
        }
        if item(ui, "Track List Repeat Protection…", "") {
            self.show_repeat = true;
        }
        ui.separator();
        if item(ui, "Insert playlist", "") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::InsertPlaylist));
        }
        if item(ui, "Insert tracks from playlist…", "") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::InsertFromPlaylist));
        }
        ui.separator();
        ui.menu_button("Extra", |ui| {
            let v = self.view_pl;
            if item(ui, "Add hour marker…", "") {
                self.open_item_dlg(DlgKind::HourMarker);
            }
            ui.separator();
            if item(ui, "Shuffle upcoming", "") {
                core.with_tab(v, |c| c.shuffle());
            }
            ui.menu_button("Sort by", |ui| {
                for (l, k) in [("Artist", "artist"), ("Title", "title"), ("Duration", "dur")] {
                    if item(ui, l, "") {
                        core.with_tab(v, |c| c.sort_by(k));
                        self.sel.clear();
                    }
                }
            });
            if item(ui, "Reverse order", "") {
                core.with_tab(v, |c| c.reverse());
                self.sel.clear();
            }
            ui.separator();
            if item(ui, "Remove duplicates", "") {
                let n = core.with_tab(v, |c| c.remove_duplicates());
                core.say(format!("Removed {n} duplicate(s)"));
                self.sel.clear();
            }
            if item(ui, "Remove played", "") {
                let n = core.with_tab(v, |c| c.remove_played());
                core.say(format!("Removed {n} played item(s)"));
                self.sel.clear();
            }
            if item(ui, "Remove missing files", "") {
                let n = core.with_tab(v, |c| c.remove_missing());
                core.say(format!("Removed {n} missing file(s)"));
                self.sel.clear();
            }
            ui.separator();
            if item(ui, "Select all", "Ctrl+A") {
                self.sel = (0..core.tab_len(v)).collect();
            }
            if item(ui, "Render playlist to WAV…", "") {
                self.dlg(|| rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name("mix.wav").save_file().map(Dlg::RenderWav));
            }
            if item(ui, "Export as CSV…", "") {
                self.dlg(|| {
                    rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("playlist.csv").save_file().map(Dlg::ExportCsv)
                });
            }
            if item(ui, "Clear playlist", "") {
                core.with_tab(v, |c| c.clear_playlist());
                self.sel.clear();
            }
        });
        if item(ui, "📝 Notepad…", "T") {
            self.open_tool(ToolWin::Notepad);
        }
        if item(ui, "🔍 Find…", "Ctrl+F") {
            self.show_find = true;
            self.find_focus = true;
        }
    }

    /// Keyboard shortcuts of the Playlist and View menus.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context, core: &mut Playout) {
        use egui::{Key, Modifiers};
        if ctx.wants_keyboard_input() {
            return;
        }
        let press = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if press(Modifiers::COMMAND, Key::G) {
            self.open_tool(ToolWin::Generator);
        }
        if press(Modifiers::ALT, Key::F) {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AddFiles));
        }
        if press(Modifiers::ALT, Key::D) {
            self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::AddFolder));
        }
        if press(Modifiers::ALT, Key::U) {
            self.open_item_dlg(DlgKind::Url);
        }
        if press(Modifiers::ALT, Key::C) {
            self.open_item_dlg(DlgKind::Command);
        }
        if press(Modifiers::COMMAND, Key::P) {
            self.open_item_dlg(DlgKind::Pause);
        }
        if press(Modifiers::ALT, Key::W) {
            self.open_item_dlg(DlgKind::Comment);
        }
        if press(Modifiers::COMMAND, Key::W) {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::InsertVoice));
        }
        if press(Modifiers::COMMAND, Key::R) {
            self.show_vt = true;
        }
        if press(Modifiers::COMMAND, Key::F) {
            self.show_find = true;
            self.find_focus = true;
        }
        if press(Modifiers::NONE, Key::T) {
            self.open_tool(ToolWin::Notepad);
        }
        // work zones: Ctrl+1/2/3 = number of panels, Ctrl+4 = new Additional window
        for (k, n) in [(Key::Num1, 1u8), (Key::Num2, 2), (Key::Num3, 3)] {
            if press(Modifiers::COMMAND, k) {
                self.workzones.panels = n;
            }
        }
        if press(Modifiers::COMMAND, Key::Num4) {
            self.workzones.new_aux(Tab::AuxPlaylist);
        }
        if press(Modifiers::COMMAND, Key::L) {
            self.workzones.show_left = !self.workzones.show_left;
        }
        for (k, tab) in [(Key::F1, Tab::FileExplorer), (Key::F2, Tab::Search), (Key::F3, Tab::Scheduler)] {
            if press(Modifiers::NONE, k) {
                self.workzones.show_left = true;
                self.workzones.left.add(tab);
            }
        }
        if press(Modifiers::NONE, Key::F4) {
            self.open_tool(ToolWin::AudioFx);
        }
        if press(Modifiers::NONE, Key::F11) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        let _ = core;
    }
}
