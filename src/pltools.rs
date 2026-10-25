//! Playlist menu tools (RadioBOSS "Playlist" menu): add URL / line input / command / timed pause /
//! teaser / time announcement / comment / container / track list / hour marker / event, voice tracks
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
    Tts,
    Event,
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
            DlgKind::Tts => "Add text to speech",
            DlgKind::Event => "Add event",
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
    /// hour marker that only shows where the hour starts
    pub info: bool,
    pub voice: String,
    pub rate: i32,
    pub speech: crate::items::SpeechOpts,
    /// event rows: what to do (`text` = its file / template / playlist / command, `arg` = rule id,
    /// encoder or volume)
    pub action: crate::scheduler::ActionKind,
    pub arg: f32,
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
            info: false,
            voice: String::new(),
            rate: 0,
            speech: Default::default(),
            action: crate::scheduler::ActionKind::SwitchPlaylist,
            arg: 0.0,
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
            ItemKind::HourInfo { hour, .. } => {
                let mut d = self.new_item_dlg(DlgKind::HourMarker);
                (d.hour, d.minute, d.info) = (*hour, 0, true);
                d
            }
            ItemKind::Tts { text, voice, rate, opts } => {
                let mut d = self.new_item_dlg(DlgKind::Tts);
                (d.text, d.voice, d.rate, d.speech) = (text.clone(), voice.clone(), *rate, opts.clone());
                d
            }
            ItemKind::Event { action, path, arg } => {
                let mut d = self.new_item_dlg(DlgKind::Event);
                (d.action, d.text, d.arg) = (*action, path.clone(), *arg);
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
            DlgKind::Event => {
                use crate::scheduler::ActionKind as A;
                let path = d.text.trim();
                let wants_text = d.action.needs_template() || d.action.needs_path() || matches!(d.action, A::SwitchPlaylist | A::Command);
                if wants_text && path.is_empty() {
                    return Err(match d.action {
                        A::SwitchPlaylist => "Pick the playlist to put on air",
                        A::Command => "Type a command",
                        _ if d.action.needs_template() => "Pick a generator template",
                        _ => "Choose a file",
                    }
                    .into());
                }
                let path = if wants_text { path.to_string() } else { String::new() };
                PlEntry::special(ItemKind::Event { action: d.action, path, arg: d.arg }, title, 0.0)
            }
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
            DlgKind::HourMarker if d.info => PlEntry::special(ItemKind::HourInfo { hour: d.hour.min(23), auto: false }, title, 0.0),
            DlgKind::HourMarker => PlEntry::special(
                ItemKind::HourMarker { hour: d.hour.min(23), minute: d.minute.min(59), hard: d.hard, wait: d.wait },
                title,
                0.0,
            ),
            DlgKind::Tts => {
                if d.text.trim().is_empty() {
                    return Err("Type what should be said".into());
                }
                let e = PlEntry::special(
                    ItemKind::Tts { text: d.text.trim().to_string(), voice: d.voice.clone(), rate: d.rate, opts: d.speech.clone() },
                    title,
                    0.0,
                );
                let secs = crate::items::speech_secs(&d.text, d.rate, &d.speech, true);
                PlEntry { track: crate::library::Track { duration: secs, ..e.track }, ..e }
            }
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
        if !self.perm.edit_playlists {
            return; // the dialog closes: this account can't change playlists
        }
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        let title = if d.edit.is_some() { d.kind.title().replace("Add", "Edit") } else { d.kind.title().to_string() };
        Self::window_for(ctx, &title, [460.0, 300.0]).id(egui::Id::new("item_dlg")).collapsible(false).open(&mut open).show(ctx, |ui| {
            // the fields scroll; OK / Cancel always stay on screen
            let max_h = (ui.ctx().screen_rect().height() - 220.0).max(160.0);
            egui::ScrollArea::vertical().id_salt("item_dlg_scroll").max_height(max_h).show(ui, |ui| {
            egui::Grid::new("item_dlg_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                let name_row = |ui: &mut egui::Ui, d: &mut ItemDlg, hint: &str| {
                    ui.label(tr("Name"));
                    ui.add(egui::TextEdit::singleline(&mut d.title).hint_text(hint).desired_width(300.0));
                    ui.end_row();
                };
                match d.kind {
                    DlgKind::Url => {
                        ui.label(tr("Stream address"));
                        ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(tr("https://radio.example.com/live.mp3")).desired_width(300.0));
                        ui.end_row();
                        name_row(ui, &mut d, "shown in the playlist (optional)");
                        ui.label(tr("Play for"));
                        Self::mins_secs(ui, &mut d, "until the stream ends / Next");
                        ui.end_row();
                    }
                    DlgKind::LineIn => {
                        ui.label(tr("Input"));
                        let dev = core.cfg.audio_devices.input.clone();
                        ui.label(if dev.is_empty() { "System default input (Settings → Audio devices)".to_string() } else { dev });
                        ui.end_row();
                        ui.label(tr("Play for"));
                        Self::mins_secs(ui, &mut d, "until Next is pressed");
                        ui.end_row();
                        name_row(ui, &mut d, "e.g. Studio 2 live");
                    }
                    DlgKind::Command => {
                        ui.label(tr("Command"));
                        ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(tr("STOP")).desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(COMMAND_HELP).small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::Event => {
                        if d.action == crate::scheduler::ActionKind::SwitchPlaylist && d.text.trim().is_empty() {
                            // most often: back to the main playlist when this one ends
                            let other = (0..core.tabs.len()).find(|&t| t != d.tab).unwrap_or(0);
                            d.text = core.tabs.get(other).map(|t| t.name.clone()).unwrap_or_default();
                        }
                        let opts = ActionOpts::of(core);
                        ui.label(tr("Do"));
                        ui.horizontal_wrapped(|ui| {
                            if action_edit(ui, "item_event", &mut d.action, &mut d.text, &mut d.arg, &opts) {
                                let a = d.action;
                                self.dlg(move || action_file_dialog(a).pick_file().map(Dlg::EventPath));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.vertical(|ui| {
                            ui.set_max_width(300.0); // wraps instead of widening the dialog
                            ui.label(
                                RichText::new(tr(
                                    "Runs when playback reaches this row, like a scheduler event. Example: put this row at the end of an evening playlist and pick \"Put a playlist tab on air → Main\", so Main comes back on air instead of dead air. At the end of Main, \"Play the on-air playlist again from the top\" keeps it looping.",
                                ))
                                .small()
                                .color(GRAY),
                            );
                        });
                        ui.end_row();
                        name_row(ui, &mut d, "shown in the playlist (optional)");
                    }
                    DlgKind::Pause => {
                        ui.label(tr("Silence for"));
                        Self::mins_secs(ui, &mut d, "");
                        ui.end_row();
                    }
                    DlgKind::Teaser => {
                        ui.label(tr("Upcoming tracks"));
                        ui.add(egui::DragValue::new(&mut d.count).range(1..=10));
                        ui.end_row();
                        ui.label(tr("Seconds of each"));
                        ui.add(egui::DragValue::new(&mut d.secs).range(2..=60).suffix(" s"));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(tr("Plays a short faded snippet from each of the next tracks after this row.")).small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::TimeAnnounce => {
                        let a = &mut core.cfg.announce;
                        ui.label(tr("Spoken text"));
                        ui.add(egui::TextEdit::singleline(&mut a.template).desired_width(300.0));
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(format!("Now: \"{}\"  ·  {{h12}} {{h24}} {{mm}} {{ampm}} {{time}}", crate::live::announce_text(&a.template, &Local::now()))).small().color(GRAY));
                        ui.end_row();
                        ui.label(tr("Voice"));
                        voice_picker(ui, "tts_voice", &mut a.voice, tr("Default"));
                        ui.end_row();
                        ui.label(tr("Recordings"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut a.folder).hint_text(tr("optional folder")).desired_width(220.0));
                            if ui.button("…").clicked() {
                                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::AnnounceFolder));
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        ui.label(RichText::new(tr("With a folder of recordings named H00…H23 and M00…M59 those are played; otherwise the computer's speech voice reads the text.")).small().color(GRAY));
                        ui.end_row();
                    }
                    DlgKind::Comment => {
                        ui.label(tr("Comment"));
                        ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(3).desired_width(300.0));
                        ui.end_row();
                    }
                    DlgKind::Container => {
                        name_row(ui, &mut d, "e.g. Station IDs");
                        ui.label(tr("Plays"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.random, false, tr("all files in order"));
                            ui.radio_value(&mut d.random, true, tr("one file at random"));
                        });
                        ui.end_row();
                        ui.label(tr("Files"));
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
                            if ui.button(tr("Add files…")).clicked() {
                                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::ContainerFiles));
                            }
                        });
                        ui.end_row();
                    }
                    DlgKind::TrackList => {
                        name_row(ui, &mut d, "e.g. Recurrent");
                        ui.label(tr("Source"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(tr("folder or .m3u")).desired_width(200.0));
                            if ui.button(tr("Folder…")).clicked() {
                                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::TrackListSource));
                            }
                            if ui.button(tr("M3U…")).clicked() {
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
                        ui.label(tr("Time"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut d.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                            ui.label(":");
                            if d.info {
                                ui.label("00");
                            } else {
                                ui.add(egui::DragValue::new(&mut d.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                            }
                        });
                        ui.end_row();
                        ui.label(tr("Type"));
                        ui.vertical(|ui| {
                            ui.radio_value(&mut d.info, false, tr("keep the playlist on time (marker)"));
                            ui.radio_value(&mut d.info, true, tr("just show where the hour starts — no action"));
                        });
                        ui.end_row();
                        if !d.info {
                            ui.label(tr("If early"));
                            ui.checkbox(&mut d.wait, tr("wait (silence) until the time"));
                            ui.end_row();
                            ui.label(tr("If late"));
                            ui.vertical(|ui| {
                                ui.radio_value(&mut d.hard, false, tr("soft: finish the current track, then jump here"));
                                ui.radio_value(&mut d.hard, true, tr("hard: cut in exactly on time"));
                            });
                            ui.end_row();
                        }
                        if d.edit.is_none() {
                            ui.label("");
                            ui.label(RichText::new(tr("It goes into the playlist where that time falls, going by the start times.")).small().color(GRAY));
                            ui.end_row();
                        }
                    }
                    DlgKind::Tts => {
                        ui.label(tr("Say"));
                        // a fixed-height box that scrolls: long text never pushes the buttons off screen
                        egui::ScrollArea::vertical().id_salt("tts_text").max_height(110.0).min_scrolled_height(110.0).show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut d.text)
                                    .desired_rows(5)
                                    .desired_width(330.0)
                                    .hint_text("That was {last}. Coming up: {next}."),
                            );
                        });
                        ui.end_row();
                        ui.label("");
                        ui.label(
                            RichText::new("{time} {h12}:{mm} {ampm} · {next} {next_artist} {next_title} · {last} {last_artist} {last_title} · {station}")
                                .small()
                                .color(GRAY),
                        );
                        ui.end_row();
                        ui.label(tr("Voice"));
                        voice_picker(ui, "tts_item_voice", &mut d.voice, tr("Same as time announcements"));
                        ui.end_row();
                        ui.label(tr("Speed"));
                        ui.add(egui::Slider::new(&mut d.rate, -10..=10).text(tr("slower ↔ faster")));
                        ui.end_row();
                        ui.label(tr("Pitch"));
                        ui.add(egui::Slider::new(&mut d.speech.pitch, -10..=10).text(tr("lower ↔ higher")));
                        ui.end_row();
                        ui.label(tr("Volume"));
                        ui.add(egui::Slider::new(&mut d.speech.volume, 0..=200).suffix(" %"));
                        ui.end_row();
                        ui.label(tr("Between lines"));
                        ui.add(egui::Slider::new(&mut d.speech.line_pause_ms, 0..=3000).suffix(" ms").text(tr("pause")));
                        ui.end_row();
                        ui.label(tr("Read it"));
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut d.speech.repeat).range(1..=20).suffix(" ×"));
                            if d.speech.repeat > 1 {
                                ui.label(tr("pause"));
                                ui.add(egui::DragValue::new(&mut d.speech.repeat_pause_ms).range(0..=10000).suffix(" ms"));
                            }
                        });
                        ui.end_row();
                        for outro in [false, true] {
                            ui.label(if outro { tr("Outro sound") } else { tr("Intro sound") });
                            let path = if outro { &mut d.speech.outro } else { &mut d.speech.intro };
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(path).hint_text(tr("none")).desired_width(230.0));
                                if ui.small_button("…").on_hover_text(tr("Pick a sound file")).clicked() {
                                    self.dlg(move || {
                                        rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_file().map(|p| Dlg::SpeechSound(outro, p))
                                    });
                                }
                                if !path.is_empty() && ui.small_button("✖").on_hover_text(tr("No sound")).clicked() {
                                    path.clear();
                                }
                            });
                            ui.end_row();
                        }
                        ui.label("");
                        ui.checkbox(&mut d.speech.spell_numbers, tr("Read numbers digit by digit"));
                        ui.end_row();
                        ui.label("");
                        ui.horizontal(|ui| {
                            if ui.button(tr("▶ Preview")).on_hover_text(tr("Plays through the preview output, not on air")).clicked() {
                                let (text, v) = core.speech_for(&d.text, &d.voice, d.rate, &d.speech, core.current.unwrap_or(0));
                                self.preview_speech(core, text, v, d.speech.clone());
                            }
                            if self.preview.is_some() && ui.button(tr("■ Stop")).clicked() {
                                self.preview = None;
                            }
                        });
                        ui.end_row();
                        name_row(ui, &mut d, "shown in the playlist (optional)");
                    }
                }
            });
            });
            if !d.err.is_empty() {
                ui.colored_label(RED, &d.err);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.button(RichText::new(tr("  OK  ")).strong()).clicked();
                cancel = ui.button(tr("Cancel")).clicked();
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
                        None if d.kind == DlgKind::HourMarker => {
                            // goes where that time falls in the running order
                            self.view_pl = d.tab;
                            let on_air = d.tab == core.playing_tab;
                            let (h, m) = (d.hour.min(23), if d.info { 0 } else { d.minute.min(59) });
                            let at = core.with_tab(d.tab, |c| c.row_for_time(h, m, on_air));
                            core.with_tab(d.tab, |c| c.insert_rows(at, vec![e]));
                            self.sel = std::iter::once(at).collect();
                            self.anchor = Some(at);
                            self.scroll_to = Some(at);
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

    /// Speak `text` on the preview output (never on air).
    pub(super) fn preview_speech(&mut self, core: &mut Playout, text: String, v: crate::live::Voice, opts: crate::items::SpeechOpts) {
        let src = crate::live::speech_source(text, v, opts, core.log_sink.clone());
        match crate::audio::PreviewPlayer::play_pcm(&core.cfg.audio_devices.preview, Box::new(src)) {
            Ok(p) => self.preview = Some(p),
            Err(e) => core.say(format!("Preview: {e}")),
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
                    if ui.small_button(tr("Select all")).clicked() {
                        pick.iter_mut().for_each(|p| *p = true);
                    }
                    if ui.small_button(tr("Select none")).clicked() {
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
                    ok = ui.button(RichText::new(tr("Insert")).strong()).clicked();
                    cancel = ui.button(tr("Cancel")).clicked();
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
        if !(self.perm.edit_playlists && self.perm.tool(crate::auth::tool::VOICE_TRACK)) {
            self.show_vt = false;
        }
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
                        if ui.button(RichText::new(tr("■ Stop")).strong()).clicked() {
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
                    } else if ui.button(RichText::new(tr("● Record")).strong().color(RED)).clicked() {
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
                        if ui.button(tr("Insert after selected row")).clicked() {
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
                ui.small(tr("Recordings are saved as WAV in the voicetracks folder of your RadioMaster data folder."));
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
                ui.label(tr("When a Track List row picks a track, skip:"));
                egui::Grid::new("repeat_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label(tr("the same track played in the last"));
                    ui.add(egui::DragValue::new(&mut core.cfg.repeat.track_mins).range(0..=10080).suffix(" min"));
                    ui.end_row();
                    ui.label(tr("the same artist played in the last"));
                    ui.add(egui::DragValue::new(&mut core.cfg.repeat.artist_mins).range(0..=1440).suffix(" min"));
                    ui.end_row();
                });
                ui.small(tr("If every track has played recently, the least recently played ones are used."));
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
            let r = ui.add(egui::TextEdit::singleline(&mut self.find_text).hint_text(tr("text in the title")).desired_width(f32::INFINITY));
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
        let edit = self.perm.edit_playlists;
        let tools = self.perm.tool(crate::auth::tool::GENERATOR);
        let vt = edit && self.perm.tool(crate::auth::tool::VOICE_TRACK);
        fn item(ui: &mut egui::Ui, enabled: bool, text: &str, sc: &str) -> bool {
            let mut b = egui::Button::new(text);
            if !sc.is_empty() {
                b = b.shortcut_text(sc);
            }
            if ui.add_enabled(enabled, b).on_disabled_hover_text(tr("Your account isn't allowed to do this")).clicked() {
                ui.close_menu();
                true
            } else {
                false
            }
        }
        if item(ui, tools, tr("🎲 Playlist Generator Pro…"), "Ctrl+G") {
            self.open_tool(ToolWin::Generator);
        }
        ui.separator();
        if item(ui, edit, tr("Add file…"), "Alt+F") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AddFiles));
        }
        if item(ui, edit, tr("Add folder…"), "Alt+D") {
            self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::AddFolder));
        }
        if item(ui, edit, tr("Add URL…"), "Alt+U") {
            self.open_item_dlg(DlgKind::Url);
        }
        if item(ui, edit, tr("Add Line Input…"), "") {
            self.open_item_dlg(DlgKind::LineIn);
        }
        if item(ui, edit, tr("Add command…"), "Alt+C") {
            self.open_item_dlg(DlgKind::Command);
        }
        if item(ui, edit, tr("Add event…"), "") {
            self.open_item_dlg(DlgKind::Event);
        }
        if item(ui, edit, tr("Add timed pause…"), "Ctrl+P") {
            self.open_item_dlg(DlgKind::Pause);
        }
        if item(ui, edit, tr("Add teaser…"), "") {
            self.open_item_dlg(DlgKind::Teaser);
        }
        if item(ui, edit, tr("Add time announcement…"), "") {
            self.open_item_dlg(DlgKind::TimeAnnounce);
        }
        if item(ui, edit, tr("Add text to speech…"), "Alt+T") {
            self.open_item_dlg(DlgKind::Tts);
        }
        if item(ui, edit, tr("Add comment…"), "Alt+W") {
            self.open_item_dlg(DlgKind::Comment);
        }
        if item(ui, edit, tr("Add container…"), "") {
            self.open_item_dlg(DlgKind::Container);
        }
        ui.separator();
        if item(ui, edit, tr("Insert voice track…"), "Ctrl+W") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::InsertVoice));
        }
        if item(ui, vt, tr("🔴 Record voice track…"), "Ctrl+R") {
            self.show_vt = true;
        }
        ui.separator();
        if item(ui, edit, tr("Add Track List…"), "") {
            self.open_item_dlg(DlgKind::TrackList);
        }
        if item(ui, edit, tr("Track List Repeat Protection…"), "") {
            self.show_repeat = true;
        }
        ui.separator();
        if item(ui, edit, tr("Insert playlist"), "") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::InsertPlaylist));
        }
        if item(ui, edit, tr("Insert tracks from playlist…"), "") {
            self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::InsertFromPlaylist));
        }
        ui.separator();
        ui.menu_button(tr("Extra"), |ui| {
            let v = self.view_pl;
            if item(ui, edit, tr("Add hour marker…"), "") {
                self.open_item_dlg(DlgKind::HourMarker);
            }
            ui.separator();
            if item(ui, edit, tr("Shuffle upcoming"), "") {
                core.with_tab(v, |c| c.shuffle());
            }
            ui.menu_button(tr("Sort by"), |ui| {
                for (l, k) in [("Artist", "artist"), ("Title", "title"), ("Duration", "dur")] {
                    if item(ui, edit, l, "") {
                        core.with_tab(v, |c| c.sort_by(k));
                        self.sel.clear();
                    }
                }
            });
            if item(ui, edit, tr("Reverse order"), "") {
                core.with_tab(v, |c| c.reverse());
                self.sel.clear();
            }
            ui.separator();
            if item(ui, edit, tr("Remove duplicates"), "") {
                let n = core.with_tab(v, |c| c.remove_duplicates());
                core.say(format!("Removed {n} duplicate(s)"));
                self.sel.clear();
            }
            if item(ui, edit, tr("Remove played"), "") {
                let n = core.with_tab(v, |c| c.remove_played());
                core.say(format!("Removed {n} played item(s)"));
                self.sel.clear();
            }
            if item(ui, edit, tr("Remove missing files"), "") {
                let n = core.with_tab(v, |c| c.remove_missing());
                core.say(format!("Removed {n} missing file(s)"));
                self.sel.clear();
            }
            ui.separator();
            if item(ui, true, tr("Select all"), "Ctrl+A") {
                self.sel = (0..core.tab_len(v)).collect();
            }
            if item(ui, true, tr("Render playlist to WAV…"), "") {
                self.dlg(|| rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name("mix.wav").save_file().map(Dlg::RenderWav));
            }
            if item(ui, true, tr("Export as CSV…"), "") {
                self.dlg(|| {
                    rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("playlist.csv").save_file().map(Dlg::ExportCsv)
                });
            }
            if item(ui, edit, tr("Clear playlist"), "") {
                core.with_tab(v, |c| c.clear_playlist());
                self.sel.clear();
            }
        });
        if item(ui, true, tr("📝 Notepad…"), "T") {
            self.open_tool(ToolWin::Notepad);
        }
        if item(ui, true, tr("🔍 Find…"), "Ctrl+F") {
            self.show_find = true;
            self.find_focus = true;
        }
    }

    /// Keyboard shortcuts. Ignored while typing in a text box; each needs the matching permission.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context, core: &mut Playout) {
        use egui::{Key, Modifiers};
        if ctx.wants_keyboard_input() {
            return;
        }
        let p = self.perm;
        let press = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        // (egui ignores extra Shift/Alt on a shortcut, so the more specific ones are checked first)
        // ---- playback
        if p.playback {
            // Shift+F1 … Shift+F12: cart pads 1–12
            let fkeys = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12];
            for (i, k) in fkeys.into_iter().enumerate() {
                if press(Modifiers::SHIFT, k) && p.jingles {
                    core.trigger_cart(i);
                }
            }
            if press(Modifiers::NONE, Key::F5) || press(Modifiers::NONE, Key::Enter) {
                let s = if self.view_pl == core.playing_tab || self.sel.is_empty() { self.sel.iter().next().copied() } else { None };
                match s {
                    Some(i) if self.view_pl != core.playing_tab => {
                        let xf = if core.state == PlayState::Playing { core.cfg.crossfade_secs } else { 0.0 };
                        core.with_tab(self.view_pl, |c| c.play_index(i, xf));
                    }
                    Some(i) if core.state == PlayState::Playing => {
                        let xf = core.cfg.crossfade_secs;
                        core.play_index(i, xf);
                    }
                    _ => core.play_pressed(s),
                }
            }
            if press(Modifiers::NONE, Key::F6) {
                core.pause_toggle();
            }
            if press(Modifiers::NONE, Key::F7) {
                core.stop();
            }
            if press(Modifiers::NONE, Key::F8) {
                core.next();
            }
            if press(Modifiers::NONE, Key::F9) {
                core.prev();
            }
            if press(Modifiers::COMMAND, Key::J) && p.jingles {
                core.stop_carts();
            }
            if press(Modifiers::COMMAND, Key::M) {
                self.toggle_mic(core);
            }
            for (k, d) in [(Key::ArrowUp, 0.05f32), (Key::ArrowDown, -0.05)] {
                if press(Modifiers::ALT, k) {
                    let v = (core.cfg.volume + d).clamp(0.0, 1.0);
                    core.set_volume(v);
                }
            }
        }
        // ---- playlist editing
        if p.edit_playlists {
            if press(Modifiers::ALT, Key::F) || press(Modifiers::NONE, Key::Insert) {
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
            if press(Modifiers::ALT, Key::T) {
                self.open_item_dlg(DlgKind::Tts);
            }
            if press(Modifiers::COMMAND, Key::P) {
                self.open_item_dlg(DlgKind::Pause);
            }
            if press(Modifiers::ALT, Key::W) {
                self.open_item_dlg(DlgKind::Comment);
            }
            if press(Modifiers::COMMAND, Key::H) {
                self.open_item_dlg(DlgKind::HourMarker);
            }
            if press(Modifiers::COMMAND, Key::W) {
                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::InsertVoice));
            }
            if press(Modifiers::COMMAND, Key::R) && p.tool(crate::auth::tool::VOICE_TRACK) {
                self.show_vt = true;
            }
            if press(Modifiers::COMMAND, Key::N) {
                self.tab_ops.push(super::plview::TabOp::New);
            }
            if press(Modifiers::COMMAND, Key::O) {
                self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::LoadM3u));
            }
            if press(Modifiers::COMMAND, Key::S) {
                let name = format!("{}.m3u", core.tabs.get(self.view_pl).map(|t| t.name.clone()).unwrap_or_else(|| "playlist".into()));
                self.dlg(move || {
                    rfd::FileDialog::new().add_filter("Playlist", &["m3u"]).set_file_name(&name).save_file().map(Dlg::SaveM3u)
                });
            }
            // Ctrl+↑ / Ctrl+↓ move the selected rows
            for (k, up) in [(Key::ArrowUp, true), (Key::ArrowDown, false)] {
                if press(Modifiers::COMMAND, k) && !self.sel.is_empty() {
                    let sel = self.sel.clone();
                    self.sel = core.with_tab(self.view_pl, |c| c.move_sel(&sel, up));
                    self.scroll_to = self.sel.iter().next().copied();
                }
            }
        }
        if p.tools {
            if press(Modifiers::COMMAND, Key::G) {
                self.open_tool(ToolWin::Generator);
            }
            if press(Modifiers::COMMAND, Key::E) {
                let row = self.sel.iter().next().copied();
                core.with_tab(self.view_pl, |c| self.open_fade_editor(c, row));
            }
        }
        // ---- moving around
        let len = core.tab_len(self.view_pl);
        for (k, up) in [(Key::ArrowUp, true), (Key::ArrowDown, false)] {
            let shift = ctx.input(|i| i.modifiers.shift);
            if (press(Modifiers::NONE, k) || press(Modifiers::SHIFT, k)) && len > 0 {
                let from = if up { self.sel.iter().next().copied() } else { self.sel.iter().next_back().copied() };
                let to = match from {
                    Some(i) if up => i.saturating_sub(1),
                    Some(i) => (i + 1).min(len - 1),
                    None => core.current.unwrap_or(0).min(len - 1),
                };
                if shift {
                    self.sel.insert(to);
                } else {
                    self.sel.clear();
                    self.sel.insert(to);
                    self.anchor = Some(to);
                }
                self.scroll_to = Some(to);
            }
        }
        if press(shift_cmd, Key::Tab) && !core.tabs.is_empty() {
            self.tab_ops.push(super::plview::TabOp::View((self.view_pl + core.tabs.len() - 1) % core.tabs.len()));
        }
        if press(Modifiers::COMMAND, Key::Tab) && !core.tabs.is_empty() {
            self.tab_ops.push(super::plview::TabOp::View((self.view_pl + 1) % core.tabs.len()));
        }
        if press(Modifiers::COMMAND, Key::F) {
            self.show_find = true;
            self.find_focus = true;
        }
        if press(Modifiers::NONE, Key::T) {
            self.open_tool(ToolWin::Notepad);
        }
        if press(Modifiers::COMMAND, Key::Comma) && p.settings {
            self.show_settings = true;
        }
        if press(Modifiers::COMMAND, Key::Slash) {
            self.show_keys = !self.show_keys;
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
        for (k, tab) in [(Key::F1, Tab::FileExplorer), (Key::F2, Tab::Search)] {
            if press(Modifiers::NONE, k) {
                self.workzones.show_left = true;
                self.workzones.left.add(tab);
            }
        }
        if press(Modifiers::NONE, Key::F3) {
            self.open_tool(ToolWin::Scheduler);
        }
        if press(Modifiers::NONE, Key::F4) {
            self.open_tool(ToolWin::AudioFx);
        }
        if press(Modifiers::NONE, Key::F11) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
    }

    /// Help → Keyboard shortcuts.
    pub(super) fn keys_window(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            return;
        }
        let mut open = true;
        Self::window_for(ctx, "Keyboard shortcuts", [560.0, 560.0]).id(egui::Id::new("keys_window")).open(&mut open).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (group, keys) in SHORTCUTS {
                    ui.add_space(6.0);
                    ui.strong(*group);
                    egui::Grid::new(("keys", *group)).num_columns(2).spacing([18.0, 4.0]).striped(true).show(ui, |ui| {
                        for (k, what) in *keys {
                            ui.label(RichText::new(*k).monospace().color(ACCENT));
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr("Shortcuts don't fire while you're typing in a text box. Some need permission for your account."))
                        .small()
                        .color(GRAY),
                );
            });
        });
        self.show_keys = open;
    }
}

pub(super) const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Playback",
        &[
            ("F5 / Enter", "Play (the selected row, or carry on)"),
            ("F6", "Pause / resume"),
            ("F7", "Stop"),
            ("F8", "Next"),
            ("F9", "Previous"),
            ("Alt+↑ / Alt+↓", "Volume up / down"),
            ("Shift+F1 … F12", "Fire cart pad 1 … 12 (again to stop it)"),
            ("Ctrl+J", "Stop all jingles"),
            ("Ctrl+M", "Microphone on / off"),
        ],
    ),
    (
        "Playlist",
        &[
            ("↑ / ↓", "Select the row above / below (Shift adds to the selection)"),
            ("Ctrl+↑ / Ctrl+↓", "Move the selected rows"),
            ("Delete", "Remove the selected rows"),
            ("Ctrl+A", "Select all"),
            ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous playlist tab"),
            ("Ctrl+N", "New playlist"),
            ("Ctrl+O / Ctrl+S", "Open / save playlist (M3U)"),
            ("Insert / Alt+F", "Add files"),
            ("Alt+D", "Add folder"),
            ("Alt+U", "Add URL"),
            ("Alt+C", "Add command"),
            ("Alt+T", "Add text to speech"),
            ("Alt+W", "Add comment"),
            ("Ctrl+P", "Add timed pause"),
            ("Ctrl+H", "Add hour marker"),
            ("Ctrl+W", "Insert voice track"),
            ("Ctrl+R", "Record voice track"),
            ("Ctrl+F", "Find"),
            ("Drag rows", "Reorder, or drop on another playlist's tab to move them there"),
        ],
    ),
    (
        "Tools and windows",
        &[
            ("Ctrl+G", "Playlist generator"),
            ("Ctrl+E", "Track editor for the selected row"),
            ("F4", "Audio processing"),
            ("T", "Notepad"),
            ("Ctrl+,", "Settings"),
            ("Ctrl+1 / 2 / 3", "Work zone with 1, 2 or 3 panels"),
            ("Ctrl+4", "New Additional window"),
            ("Ctrl+L", "Left panel on / off"),
            ("F1 / F2 / F3", "File explorer / Search / Scheduler in the left panel"),
            ("F11", "Full screen"),
            ("Ctrl+Shift+P", "Performance numbers"),
            ("Ctrl+/", "This list"),
        ],
    ),
];

/// Voice choice with a search box (there can be hundreds of voices). `none` = the empty choice.
pub(super) fn voice_picker(ui: &mut egui::Ui, id: &str, voice: &mut String, none: &str) {
    let voices = crate::live::voices();
    if voices.is_empty() {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(voice).hint_text(none).desired_width(260.0));
            ui.spinner().on_hover_text(tr("Looking for the computer's voices…"));
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        return;
    }
    let fid = egui::Id::new(("voice_filter", id));
    let mut filter: String = ui.data(|d| d.get_temp(fid)).unwrap_or_default();
    let shown = if voice.is_empty() { none.to_string() } else { voice.clone() };
    egui::ComboBox::from_id_salt(id).width(330.0).height(360.0).selected_text(shown).show_ui(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut filter).hint_text(tr("🔍 search voices")).desired_width(300.0));
        ui.label(RichText::new(format!("{} {}", voices.len(), tr("voices on this computer"))).small().color(GRAY));
        ui.selectable_value(voice, String::new(), none);
        let f = filter.to_lowercase();
        for v in voices.iter().filter(|v| f.is_empty() || v.to_lowercase().contains(&f)) {
            ui.selectable_value(voice, v.clone(), v);
        }
    });
    ui.data_mut(|d| d.insert_temp(fid, filter));
}
