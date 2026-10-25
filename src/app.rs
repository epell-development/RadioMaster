use crate::azuracast::Client;
use crate::browser::Browsers;
use crate::config::{data_dir, CartPad, Category, Marker, PatternItem, ServerKind, Template};
use crate::dsp::EQ_FREQS;
use crate::i18n::tr;
use crate::items::PlEntry;
use crate::library::Track;
use crate::playout::{parse_m3u, M3uRow, PlayState, Playout};
use crate::scheduler::{ActionKind, Event};
use crate::stream::EncState;
use crate::workzones::{Tab, WorkzoneSet};
use chrono::{Local, Timelike};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

const GREEN: Color32 = Color32::from_rgb(70, 200, 90);
const AMBER: Color32 = Color32::from_rgb(235, 190, 60);
const RED: Color32 = Color32::from_rgb(230, 70, 70);
const GRAY: Color32 = Color32::from_rgb(98, 106, 120);
const ACCENT: Color32 = Color32::from_rgb(30, 110, 205);

/// Built-in EQ curves (31 Hz … 16 kHz, dB).
const EQ_PRESETS: &[(&str, [f32; 12])] = &[
    ("Flat", [0.0; 12]),
    ("Broadcast (FM warmth)", [2.0, 2.5, 1.5, 0.0, -1.0, -0.5, 0.5, 1.5, 2.0, 2.5, 2.0, 1.0]),
    ("Bass boost", [5.0, 4.5, 3.5, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Voice clarity", [-4.0, -3.0, -1.5, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 0.5, 0.0, -1.0]),
    ("Loudness", [4.0, 3.0, 1.5, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0, 3.5, 3.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.5, 4.5]),
    ("Dance", [4.5, 4.0, 2.0, 0.0, -1.0, -1.5, 0.0, 1.0, 2.0, 3.0, 2.5, 1.5]),
    ("Rock", [3.0, 2.5, 1.0, -0.5, -1.0, 0.0, 1.5, 2.5, 3.0, 3.0, 2.5, 2.0]),
    ("Talk / podcast", [-6.0, -4.0, -2.0, 0.0, 1.0, 2.0, 2.5, 2.0, 1.0, 0.0, -1.0, -2.0]),
    ("Small speakers", [-3.0, -1.0, 2.0, 3.0, 2.0, 1.0, 0.0, 0.5, 1.0, 1.5, 1.0, 0.0]),
];

#[derive(PartialEq, Eq, Clone, Copy)]
enum SettingsPage {
    General,
    Library,
    Playback,
    Devices,
    Streaming,
    Audio,
    Automation,
    Remote,
    Appearance,
    Accounts,
}
impl SettingsPage {
    const ALL: [SettingsPage; 10] = [
        Self::General,
        Self::Library,
        Self::Playback,
        Self::Devices,
        Self::Streaming,
        Self::Audio,
        Self::Automation,
        Self::Remote,
        Self::Appearance,
        Self::Accounts,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Library => "Library",
            Self::Playback => "Playback",
            Self::Devices => "Audio devices",
            Self::Streaming => "Broadcast / encoders",
            Self::Audio => "Audio processing",
            Self::Automation => "Automation / metadata",
            Self::Remote => "Remote API",
            Self::Appearance => "Appearance / workzones",
            Self::Accounts => "Users & access control",
        }
    }
}

enum Dlg {
    AddFiles(Vec<PathBuf>),
    /// the audio files in the chosen folder
    AddFolder(Vec<PathBuf>),
    LoadM3u(Vec<M3uRow>),
    SaveM3u(PathBuf),
    LibFolder(PathBuf),
    Cart(usize, PathBuf),
    SchedPath(usize, PathBuf),
    /// file for the event row being added / edited
    EventPath(PathBuf),
    RotFolder(usize, PathBuf),
    RenderWav(PathBuf),
    AdPath(usize, PathBuf),
    CatFolder(usize, PathBuf),
    SaveReport(PathBuf),
    AnnounceFolder(PathBuf),
    ContainerFiles(Vec<PathBuf>),
    FfmpegPath(PathBuf),
    /// (outro?, file) for a speech row
    SpeechSound(bool, PathBuf),
    TrackListSource(PathBuf),
    InsertVoice(Vec<PathBuf>),
    InsertPlaylist(Vec<M3uRow>),
    InsertFromPlaylist(PathBuf, Vec<M3uRow>),
    ExportCsv(PathBuf),
    AzUpload(Vec<PathBuf>),
    RotFiles(usize, Vec<PathBuf>),
    CleanupFolder(PathBuf),
    /// files (folders already listed) for playlist tab, at row (None: after the selection when
    /// that tab is viewed, otherwise the end)
    Files(usize, Option<usize>, Vec<PathBuf>),
    /// file explorer listing: (panel key, folder, entries)
    FolderList(String, PathBuf, Vec<(bool, PathBuf)>),
    /// sound card outputs, inputs, and the default output
    Devices(Vec<String>, Vec<String>, Option<String>),
    /// the play history for Reports
    History(Vec<crate::report::Play>),
}

/// Dialog results that read the disk do it here, on the dialog's thread, not on the UI thread.
impl Dlg {
    fn add_folder(p: PathBuf) -> Dlg {
        Dlg::AddFolder(crate::library::audio_files_in(&p))
    }
    fn load_m3u(p: PathBuf) -> Dlg {
        Dlg::LoadM3u(parse_m3u(&p))
    }
    fn insert_playlist(p: PathBuf) -> Dlg {
        Dlg::InsertPlaylist(parse_m3u(&p))
    }
    fn insert_from_playlist(p: PathBuf) -> Dlg {
        let rows = parse_m3u(&p);
        Dlg::InsertFromPlaylist(p, rows)
    }
}

enum PlAct {
    Play(usize),
    Remove,
    Up,
    Down,
    Upload,
    Fade(usize),
    Edit(usize),
    CopyTo(usize),
    MoveTo(usize),
}

#[path = "tools.rs"]
mod tools;
use tools::ToolWin;
#[path = "adsui.rs"]
mod adsui;
#[path = "aztool.rs"]
mod aztool;
#[path = "cleanupui.rs"]
mod cleanupui;
#[path = "panes.rs"]
mod panes;
#[path = "pltools.rs"]
mod pltools;
#[path = "plview.rs"]
mod plview;
#[path = "reportui.rs"]
mod reportui;
#[path = "serverui.rs"]
mod serverui;

/// Presenter notes for the Track Info tab (notes.json in the data folder).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct TrackNotes {
    /// by file path
    songs: BTreeMap<String, String>,
    /// by artist (lower case)
    artists: BTreeMap<String, String>,
}

impl TrackNotes {
    fn load() -> Self {
        std::fs::read_to_string(data_dir().join("notes.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(j) = serde_json::to_string_pretty(self) {
            crate::playout::write_in_background(data_dir().join("notes.json"), j);
        }
    }
}

/// Frame timing shown by Help → Performance (Ctrl+Shift+P), for diagnosing slowness.
#[derive(Default)]
struct Perf {
    frame_ms: f32,
    lock_ms: f32,
    worst_ms: f32,
    worst_lock_ms: f32,
    fps: f32,
    frames: u32,
    window: Option<std::time::Instant>,
}

pub struct App {
    core: Arc<Mutex<Playout>>,
    /// the status-bar message (shared with the playout), usable while the playout is locked
    notify: Arc<Mutex<String>>,
    pub audio: crate::audio::AudioManager,
    dev_outputs: Vec<String>,
    /// sound cards listed once / being listed in the background
    dev_listed: bool,
    dev_listing: bool,
    dev_inputs: Vec<String>,
    ctx: egui::Context,
    sel: BTreeSet<usize>,
    anchor: Option<usize>,
    lib_filter: String,
    lib_filtered: Vec<usize>,
    lib_key: (String, u64),
    /// selected library tracks (library indexes)
    lib_sel: BTreeSet<usize>,
    lib_anchor: Option<usize>,
    /// library track to scroll into view
    lib_scroll: Option<usize>,
    /// track editor: show the track list, list the library (else the playlist), scroll it to the track
    te_browser: bool,
    te_from_lib: bool,
    te_scroll: bool,
    dlg_tx: mpsc::Sender<Dlg>,
    dlg_rx: mpsc::Receiver<Dlg>,
    tag_edit: Option<Track>,
    show_about: bool,
    seek_drag: Option<f32>,
    mic_stream: Option<rodio::cpal::Stream>,
    mic_on: bool,
    /// playback was paused by the mic (Pause mode) and resumes when it goes off
    mic_paused: bool,
    tpl_sel: usize,
    rep: reportui::RepUi,
    /// where ffmpeg was found (None = look again)
    ffmpeg_found: Option<Option<String>>,
    ads: adsui::AdsUi,
    clean: cleanupui::CleanUi,
    workzones: WorkzoneSet,
    saved_layout: String,
    browsers: Browsers,
    web_state: HashMap<String, panes::WebState>,
    fx_dir: HashMap<String, PathBuf>,
    fx_list: HashMap<String, (PathBuf, std::time::Instant, Vec<(bool, PathBuf)>)>,
    /// explorer folders being listed in the background (panel key → folder)
    fx_loading: HashMap<String, PathBuf>,
    aux_spawn: HashMap<u64, ([f32; 2], Option<[f32; 2]>)>,
    layout_checked: std::time::Instant,
    perf: Option<Perf>,
    eq_name: String,
    notes: TrackNotes,
    notes_dirty: bool,
    info_pick: usize,
    updates: crate::updater::Shared,
    show_update: bool,
    update_next: Option<std::time::Instant>,
    aux_pl: HashMap<String, usize>,
    aux_sel: HashMap<String, BTreeSet<usize>>,
    icon: Option<Arc<egui::IconData>>,
    logo: Option<egui::TextureHandle>,
    icon_tex: Option<egui::TextureHandle>,
    view_pl: usize,
    renaming: Option<(usize, String)>,
    tab_ops: Vec<plview::TabOp>,
    scroll_to: Option<usize>,
    /// (tab, on-air row) the playlist last scrolled to follow
    followed: Option<(usize, Option<usize>)>,
    /// when the automatic hour lines were last checked
    hour_check: Option<std::time::Instant>,
    item_dlg: Option<pltools::ItemDlg>,
    ins_from_pl: Option<(PathBuf, Vec<PlEntry>, Vec<bool>)>,
    show_vt: bool,
    vt: Option<pltools::VoiceRec>,
    show_repeat: bool,
    show_find: bool,
    show_keys: bool,
    about_tab: usize,
    find_text: String,
    find_focus: bool,
    fullscreen: bool,
    az: aztool::AzTool,
    show_settings: bool,
    settings_page: SettingsPage,
    segment_text: String,
    segment_name: String,
    profile_name: String,
    profile_list: Vec<String>,
    show_profiles: bool,
    accounts: crate::auth::Accounts,
    logged_in: Option<crate::auth::Operator>,
    login_name: String,
    login_password: String,
    auth_error: String,
    show_users: bool,
    new_user_admin: bool,
    /// what the signed-in operator may do (all true for administrators), refreshed every frame
    perm: crate::auth::Perms,
    new_user_perms: crate::auth::Perms,
    user_pw: HashMap<String, String>,
    user_delete: Option<String>,
    tool_open: std::collections::BTreeSet<ToolWin>,
    fade_edit: Option<tools::FadeEdit>,
    preview: Option<crate::audio::PreviewPlayer>,
    art: Option<(PathBuf, Option<egui::TextureHandle>)>,
    left_w: f32,
    right_w: f32,
    art_rx: Option<mpsc::Receiver<(PathBuf, Option<egui::ColorImage>)>>,
    /// File → Connect to Server
    srv: serverui::ServerUi,
}

/// The one delete button used everywhere: 🗑, then "Delete!" / "Keep" so nothing goes by accident.
/// Returns true once the deletion is confirmed.
pub(super) fn delete_button(ui: &mut egui::Ui, id: impl std::hash::Hash, what: &str) -> bool {
    let ask = egui::Id::new(("delete_ask", id));
    let mut yes = false;
    if ui.data(|d| d.get_temp::<bool>(ask)).unwrap_or(false) {
        if ui.small_button(RichText::new(tr("Delete!")).color(RED)).clicked() {
            ui.data_mut(|d| d.remove::<bool>(ask));
            yes = true;
        }
        if ui.small_button(tr("Keep")).clicked() {
            ui.data_mut(|d| d.remove::<bool>(ask));
        }
    } else if ui.small_button("🗑").on_hover_text(format!("{} {what}", tr("Delete"))).clicked() {
        ui.data_mut(|d| d.insert_temp(ask, true));
    }
    yes
}

/// A date as "YYYY-MM-DD" text with a calendar button. `optional`: empty means "no limit" and a
/// ✖ clears it. Returns true when it changed.
pub(super) fn date_edit(ui: &mut egui::Ui, id: impl std::hash::Hash, value: &mut String, optional: bool) -> bool {
    let parsed = chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").ok();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        match parsed {
            Some(mut d) => {
                if ui
                    .add(egui_extras::DatePickerButton::new(&mut d).id_salt(&format!("{:?}", egui::Id::new(&id))).calendar_week(false))
                    .changed()
                {
                    *value = d.format("%Y-%m-%d").to_string();
                    changed = true;
                }
                if optional && ui.small_button("✖").on_hover_text(tr("No date limit")).clicked() {
                    value.clear();
                    changed = true;
                }
            }
            None if optional && value.trim().is_empty() => {
                if ui.button(format!("📅 {}", tr("any date"))).on_hover_text(tr("Click to pick a date")).clicked() {
                    *value = Local::now().format("%Y-%m-%d").to_string();
                    changed = true;
                }
            }
            None => {
                // typed something that isn't a date: let it be fixed by hand, or reset to today
                changed |= ui.add(egui::TextEdit::singleline(value).desired_width(90.0).hint_text("YYYY-MM-DD")).changed();
                if ui.small_button("📅").on_hover_text(tr("Today")).clicked() {
                    *value = Local::now().format("%Y-%m-%d").to_string();
                    changed = true;
                }
            }
        }
    });
    changed
}

fn fmt(secs: f32) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn name_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, cur: &mut String, opts: &[String]) {
    egui::ComboBox::from_id_salt(id).selected_text(cur.clone()).width(130.0).show_ui(ui, |ui| {
        for o in opts {
            ui.selectable_value(cur, o.clone(), o);
        }
    });
}

/// What an event action can point at: generator templates, advert / jingle rules, playlist tabs and encoders.
pub(super) struct ActionOpts {
    pub templates: Vec<String>,
    pub rules: Vec<(u64, String)>,
    pub tabs: Vec<String>,
    pub n_enc: usize,
}

impl ActionOpts {
    pub(super) fn of(core: &Playout) -> Self {
        ActionOpts {
            templates: core.cfg.templates.iter().map(|t| t.name.clone()).collect(),
            rules: core.cfg.ad_rules.iter().map(|r| (r.id, r.name.clone())).collect(),
            tabs: core.tabs.iter().map(|t| t.name.clone()).collect(),
            n_enc: core.cfg.encoders.len(),
        }
    }
}

/// The "Do …" part of a scheduler event or a playlist event row: the action and what it works on.
/// Returns true when "…" (browse) was clicked; the caller opens the file dialog.
pub(super) fn action_edit(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + Copy,
    action: &mut ActionKind,
    path: &mut String,
    arg: &mut f32,
    o: &ActionOpts,
) -> bool {
    let mut browse = false;
    egui::ComboBox::from_id_salt(("act", id)).selected_text(tr(action.label())).width(290.0).height(400.0).show_ui(ui, |ui| {
        for a in ActionKind::ALL {
            ui.selectable_value(action, a, tr(a.label()));
        }
    });
    if action.needs_template() {
        if o.templates.iter().all(|t| t != path) {
            *path = o.templates.first().cloned().unwrap_or_default();
        }
        name_combo(ui, ("ev_tpl", id), path, &o.templates);
    } else if *action == ActionKind::RunRule {
        let shown = o.rules.iter().find(|r| r.0 == *arg as u64).map(|r| r.1.clone()).unwrap_or_else(|| tr("pick a rule").to_string());
        egui::ComboBox::from_id_salt(("ev_rule", id)).selected_text(shown).width(180.0).show_ui(ui, |ui| {
            for (rid, name) in &o.rules {
                if ui.selectable_label(*arg as u64 == *rid, name).clicked() {
                    *arg = *rid as f32;
                }
            }
        });
    } else if *action == ActionKind::SwitchPlaylist {
        name_combo(ui, ("ev_tab", id), path, &o.tabs);
    } else if *action == ActionKind::Command {
        ui.add(egui::TextEdit::singleline(path).desired_width(260.0).hint_text("VOLUME 80 · ENCODER START 1 · RULE Jingles"));
    } else if action.needs_path() {
        ui.add(egui::TextEdit::singleline(path).desired_width(300.0));
        browse = ui.button("…").on_hover_text(tr("Browse")).clicked();
    }
    if action.needs_arg() {
        if *action == ActionKind::SetVolume {
            ui.add(egui::Slider::new(arg, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
        } else {
            let mut k = *arg as usize + 1;
            ui.label(tr("encoder"));
            ui.add(egui::DragValue::new(&mut k).range(1..=o.n_enc.max(1)));
            *arg = (k - 1) as f32;
        }
    }
    browse
}

/// File dialog for an action that takes a file: a playlist for "Load playlist", audio otherwise.
pub(super) fn action_file_dialog(action: ActionKind) -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    if action == ActionKind::LoadPlaylistAndPlay {
        d.add_filter("Playlist", &["m3u", "m3u8"])
    } else {
        d.add_filter("Audio", &crate::library::EXTS)
    }
}

/// Where generator tracks come from: a category, or the files of an advert / jingle rule
/// (stored as "rule:<id>"). `rules` = (id, name, adverts).
fn source_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, cur: &mut String, cats: &[String], rules: &[(u64, String, bool)]) {
    let shown = match cur.strip_prefix("rule:").and_then(|s| s.parse::<u64>().ok()) {
        Some(rid) => rules.iter().find(|r| r.0 == rid).map(|r| format!("⚡ {}", r.1)).unwrap_or_else(|| tr("(deleted rule)").to_string()),
        None => cur.clone(),
    };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(170.0).show_ui(ui, |ui| {
        ui.label(RichText::new(tr("Categories")).small().color(GRAY));
        for o in cats {
            ui.selectable_value(cur, o.clone(), o);
        }
        if !rules.is_empty() {
            ui.separator();
            ui.label(RichText::new(tr("Jingles & adverts (their rule's files)")).small().color(GRAY));
            for (rid, name, adverts) in rules {
                let tag = if *adverts { tr("advert") } else { tr("jingle") };
                ui.selectable_value(cur, format!("rule:{rid}"), format!("⚡ {name}  ({tag})"));
            }
        }
    });
}

fn host_of(url: &str) -> String {
    let u = url.trim().trim_start_matches("https://").trim_start_matches("http://");
    u.split('/').next().unwrap_or("").split(':').next().unwrap_or("").to_string()
}

fn vu(ui: &mut egui::Ui, level: f32, w: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 11.0), egui::Sense::hover());
    let db = 20.0 * level.max(1e-5).log10();
    let frac = ((db + 54.0) / 54.0).clamp(0.0, 1.0);
    let segs = 40;
    let sw = rect.width() / segs as f32;
    for k in 0..segs {
        let f = k as f32 / segs as f32;
        let base = if f > 0.88 {
            RED
        } else if f > 0.72 {
            AMBER
        } else {
            GREEN
        };
        let col = if f < frac { base } else { base.gamma_multiply(0.14) };
        let r = egui::Rect::from_min_size(egui::pos2(rect.left() + k as f32 * sw, rect.top()), egui::vec2(sw - 1.0, rect.height()));
        ui.painter().rect_filled(r, 0.0, col);
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, core: Arc<Mutex<Playout>>) -> Self {
        let workzones = WorkzoneSet::load();
        crate::theme::apply(&cc.egui_ctx, workzones.dark, workzones.scale);
        crate::theme::install_fallback_fonts(&cc.egui_ctx);
        let mut audio = crate::audio::AudioManager::default();
        {
            let c = core.lock();
            audio.apply(&c.engine, &c.cfg.audio_devices);
        }
        let (dlg_tx, dlg_rx) = mpsc::channel();
        let notify = core.lock().notify.clone();
        core.lock().plugin_event("app_start", serde_json::json!({ "version": crate::updater::CURRENT }));
        App {
            notify,
            core,
            audio,
            dev_outputs: vec![],
            dev_listed: false,
            dev_listing: false,
            dev_inputs: vec![],
            ctx: cc.egui_ctx.clone(),
            sel: BTreeSet::new(),
            anchor: None,
            lib_filter: String::new(),
            lib_filtered: vec![],
            lib_key: (String::new(), 0),
            lib_sel: BTreeSet::new(),
            lib_anchor: None,
            lib_scroll: None,
            te_browser: true,
            te_from_lib: true,
            te_scroll: true,
            dlg_tx,
            dlg_rx,
            tag_edit: None,
            show_about: false,
            tool_open: Default::default(),
            fade_edit: None,
            preview: None,
            art: None,
            left_w: 300.0,
            right_w: 400.0,
            art_rx: None,
            srv: Default::default(),
            seek_drag: None,
            mic_stream: None,
            mic_on: false,
            mic_paused: false,
            tpl_sel: 0,
            rep: Default::default(),
            ffmpeg_found: None,
            ads: Default::default(),
            clean: Default::default(),
            workzones,
            saved_layout: String::new(),
            browsers: Browsers::new(),
            web_state: HashMap::new(),
            fx_dir: HashMap::new(),
            fx_list: HashMap::new(),
            fx_loading: HashMap::new(),
            aux_spawn: HashMap::new(),
            layout_checked: std::time::Instant::now(),
            perf: None,
            eq_name: String::new(),
            notes: TrackNotes::load(),
            notes_dirty: false,
            info_pick: 1,
            updates: Default::default(),
            show_update: false,
            update_next: None,
            aux_pl: HashMap::new(),
            aux_sel: HashMap::new(),
            icon: crate::app_icon().map(Arc::new),
            logo: None,
            icon_tex: None,
            view_pl: usize::MAX,
            renaming: None,
            tab_ops: vec![],
            scroll_to: None,
            followed: None,
            hour_check: None,
            item_dlg: None,
            ins_from_pl: None,
            show_vt: false,
            vt: None,
            show_repeat: false,
            show_find: false,
            show_keys: false,
            about_tab: 0,
            find_text: String::new(),
            find_focus: false,
            fullscreen: false,
            az: Default::default(),
            show_settings: false,
            settings_page: SettingsPage::General,
            segment_text: std::fs::read_to_string(data_dir().join("segment.txt")).unwrap_or_default(),
            segment_name: "Live segment".into(),
            profile_name: "Default".into(),
            profile_list: std::fs::read_dir(data_dir().join("profiles"))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect(),
            show_profiles: false,
            accounts: crate::auth::Accounts::load(),
            logged_in: None,
            login_name: String::new(),
            login_password: String::new(),
            auth_error: String::new(),
            show_users: false,
            new_user_admin: false,
            perm: crate::auth::Perms::default(),
            new_user_perms: crate::auth::Perms::presenter(),
            user_pw: HashMap::new(),
            user_delete: None,
        }
    }

    /// A window that always fits on screen, centred, with a sensible size.
    fn window_for(ctx: &egui::Context, title: &str, want: [f32; 2]) -> egui::Window<'static> {
        let screen = ctx.screen_rect().size();
        let w = want[0].min(screen.x * 0.9).min(screen.x - 24.0).max(280.0);
        let h = want[1].min(screen.y * 0.85).min(screen.y - 24.0).max(200.0);
        let pos = egui::pos2(((screen.x - w) / 2.0).max(8.0), ((screen.y - h) / 2.0).max(8.0));
        egui::Window::new(title.to_string())
            .default_size([w, h])
            .default_pos(pos)
            .constrain(true)
            .max_size([screen.x - 16.0, screen.y - 110.0])
    }

    fn dlg<F: FnOnce() -> Option<Dlg> + Send + 'static>(&self, f: F) {
        let (tx, ctx) = (self.dlg_tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            if let Some(d) = f() {
                let _ = tx.send(d);
                ctx.request_repaint();
            }
        });
    }

    /// Add files and folders to playlist `tab` (see `Dlg::Files`) once the folders have been listed
    /// off the UI thread.
    pub(super) fn add_files_later(&self, tab: usize, at: Option<usize>, paths: Vec<PathBuf>) {
        self.dlg(move || Some(Dlg::Files(tab, at, crate::library::expand_paths(paths))));
    }

    fn handle_dialogs(&mut self, core: &mut Playout) {
        while let Ok(d) = self.dlg_rx.try_recv() {
            match d {
                Dlg::AddFiles(v) => {
                    let rows: Vec<PlEntry> = v.iter().map(|p| core.entry_quick(p)).collect();
                    self.insert_rows_here(core, rows);
                }
                Dlg::AddFolder(files) => {
                    let rows: Vec<PlEntry> = files.iter().map(|f| core.entry_quick(f)).collect();
                    if rows.is_empty() {
                        core.say("No audio files in that folder");
                    }
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertVoice(v) => {
                    let rows: Vec<PlEntry> = v
                        .iter()
                        .map(|p| {
                            let mut e = core.entry_quick(p);
                            e.kind = crate::items::ItemKind::VoiceTrack;
                            e
                        })
                        .collect();
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertPlaylist(m3u) => {
                    let rows = core.m3u_rows(m3u);
                    if rows.is_empty() {
                        core.say("That playlist is empty or could not be read");
                    }
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertFromPlaylist(p, m3u) => {
                    let rows = core.m3u_rows(m3u);
                    let n = rows.len();
                    self.ins_from_pl = Some((p, rows, vec![true; n]));
                }
                Dlg::AnnounceFolder(p) => core.cfg.announce.folder = p.to_string_lossy().to_string(),
                Dlg::ContainerFiles(v) => {
                    if let Some(d) = self.item_dlg.as_mut() {
                        d.items.extend(v);
                    }
                }
                Dlg::SpeechSound(outro, p) => {
                    if let Some(d) = self.item_dlg.as_mut() {
                        let s = p.to_string_lossy().to_string();
                        if outro {
                            d.speech.outro = s;
                        } else {
                            d.speech.intro = s;
                        }
                    }
                }
                Dlg::TrackListSource(p) => {
                    if let Some(d) = self.item_dlg.as_mut() {
                        d.text = p.to_string_lossy().to_string();
                    }
                }
                Dlg::AzUpload(v) => self.az_upload_here(core, v),
                Dlg::RotFiles(i, v) => {
                    if let Some(r) = core.cfg.ad_rules.get_mut(i) {
                        r.files.extend(v.iter().map(|p| p.to_string_lossy().to_string()));
                    }
                }
                Dlg::LoadM3u(m3u) => {
                    let rows = core.m3u_rows(m3u);
                    if rows.is_empty() {
                        core.say("That playlist is empty or could not be read");
                    } else {
                        core.with_tab(self.view_pl, |c| c.replace_rows(rows));
                    }
                }
                Dlg::Files(tab, at, files) => {
                    let rows: Vec<PlEntry> = files.iter().map(|p| core.entry_quick(p)).collect();
                    if rows.is_empty() {
                        core.say("No audio files in that folder");
                    }
                    self.insert_rows_at(core, tab, at, rows);
                }
                Dlg::FolderList(key, dir, list) => {
                    self.fx_loading.remove(&key);
                    self.fx_list.insert(key, (dir, std::time::Instant::now(), list));
                }
                Dlg::Devices(outs, ins, default) => {
                    self.dev_outputs = outs;
                    self.dev_inputs = ins;
                    self.dev_listing = false;
                    let sel = &mut core.cfg.audio_devices.outputs;
                    for n in &self.dev_outputs {
                        if !sel.iter().any(|o| &o.name == n) {
                            sel.push(crate::config::OutputSel { name: n.clone(), enabled: false, gain: 1.0, studio: false });
                        }
                    }
                    if !sel.iter().any(|o| o.enabled) {
                        if let Some(o) = default.and_then(|d| sel.iter_mut().find(|o| o.name == d)) {
                            o.enabled = true;
                        }
                    }
                }
                Dlg::History(all) => self.rep.set_history(all),
                Dlg::SaveM3u(p) => core.with_tab(self.view_pl, |c| c.save_m3u(&p)),
                Dlg::RenderWav(p) => core.with_tab(self.view_pl, |c| c.render_playlist(p)),
                Dlg::ExportCsv(p) => match core.with_tab(self.view_pl, |c| c.export_csv(&p)) {
                    Ok(()) => core.say(format!("Playlist exported to {}", p.display())),
                    Err(e) => core.say(format!("Export failed: {e}")),
                },
                Dlg::AdPath(i, p) => {
                    if let Some(x) = core.cfg.ads.get_mut(i) {
                        x.path = p.to_string_lossy().to_string();
                    }
                }
                Dlg::CatFolder(i, p) => {
                    if let Some(x) = core.cfg.categories.get_mut(i) {
                        x.folder = p.to_string_lossy().to_string();
                    }
                }
                Dlg::SaveReport(p) => {
                    let msg = self.rep.write_export(&p, Self::station_name(core)).unwrap_or_else(|e| format!("Couldn't save: {e}"));
                    core.say(msg.clone());
                    self.rep.msg = msg;
                }
                Dlg::FfmpegPath(p) => {
                    core.cfg.ffmpeg_path = p.to_string_lossy().to_string();
                    self.ffmpeg_found = None;
                }
                Dlg::CleanupFolder(p) => self.clean.extra = Some(p),
                Dlg::LibFolder(p) => {
                    core.cfg.library_folders.push(p.to_string_lossy().to_string());
                    core.start_scan();
                }
                Dlg::Cart(i, p) => {
                    let c = &mut core.cfg.carts[i];
                    c.path = p.to_string_lossy().to_string();
                    if c.label.is_empty() {
                        c.label = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    }
                }
                Dlg::SchedPath(i, p) => {
                    if let Some(e) = core.cfg.events.get_mut(i) {
                        e.path = p.to_string_lossy().to_string();
                    }
                }
                Dlg::EventPath(p) => {
                    if let Some(d) = self.item_dlg.as_mut().filter(|d| d.kind == pltools::DlgKind::Event) {
                        d.text = p.to_string_lossy().to_string();
                    }
                }
                Dlg::RotFolder(i, p) => {
                    if let Some(r) = core.cfg.ad_rules.get_mut(i) {
                        r.folder = p.to_string_lossy().to_string();
                    }
                }
            }
        }
    }

    fn upload(core: &Playout, paths: Vec<PathBuf>) {
        let (c, notify) = (core.cfg.azuracast.clone(), core.notify.clone());
        thread::spawn(move || {
            let cl = Client::new(&c);
            let n = paths.len();
            for (k, p) in paths.iter().enumerate() {
                *notify.lock() = format!("AzuraCast: uploading {}/{} …", k + 1, n);
                if let Err(e) = cl.upload(p, &c.upload_dir) {
                    *notify.lock() = format!("AzuraCast upload failed: {e}");
                    return;
                }
            }
            *notify.lock() = format!("AzuraCast: uploaded {n} file(s)");
        });
    }

    // ------------------------------------------------------------- menu
    fn logo_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let dark = ctx.style().visuals.dark_mode;
        let want = if dark { "logo_dark" } else { "logo_light" };
        if self.logo.as_ref().is_none_or(|t| t.name() != want) {
            let bytes: &[u8] = if dark { include_bytes!("../assets/logo_dark.png") } else { include_bytes!("../assets/logo_light.png") };
            let img = image::load_from_memory(bytes).ok()?.to_rgba8();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            self.logo = Some(ctx.load_texture(want, ci, egui::TextureOptions::LINEAR));
        }
        self.logo.clone()
    }

    fn icon_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        if self.icon_tex.is_none() {
            let i = self.icon.as_ref()?;
            let ci = egui::ColorImage::from_rgba_unmultiplied([i.width as usize, i.height as usize], &i.rgba);
            self.icon_tex = Some(ctx.load_texture("rm_icon", ci, egui::TextureOptions::LINEAR));
        }
        self.icon_tex.clone()
    }

    fn menu(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let logo = self.logo_texture(ctx);
        let icon = self.icon_texture(ctx);
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button(tr("File"), |ui| {
                    if ui.button(tr("New playlist")).clicked() {
                        self.tab_ops.push(plview::TabOp::New);
                        ui.close_menu();
                    }
                    if ui.button(tr("Open playlist…")).clicked() {
                        self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::load_m3u));
                        ui.close_menu();
                    }
                    if ui.button(tr("Save playlist as…")).clicked() {
                        let name =
                            format!("{}.m3u", core.tabs.get(self.view_pl).map(|t| t.name.clone()).unwrap_or_else(|| "playlist".into()));
                        self.dlg(move || {
                            rfd::FileDialog::new().add_filter("Playlist", &["m3u"]).set_file_name(&name).save_file().map(Dlg::SaveM3u)
                        });
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(tr("Add library folder…")).clicked() {
                        self.pick_library_folder();
                        ui.close_menu();
                    }
                    if ui.button(tr("Rescan library")).clicked() {
                        core.start_scan();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(tr("Station profiles…")).clicked() {
                        self.show_profiles = true;
                        ui.close_menu();
                    }
                    self.server_menu(ui, core);
                    if self.logged_in.as_ref().is_some_and(|a| a.admin) && ui.button(tr("Users…")).clicked() {
                        self.show_users = true;
                        ui.close_menu();
                    }
                    if ui.button(tr("Log out")).clicked() {
                        if self.srv.signed_in_remotely() {
                            self.disconnect(core);
                        }
                        self.logged_in = None;
                        self.login_password.clear();
                        self.auth_error.clear();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(tr("Exit")).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button(tr("View"), |ui| {
                    ui.menu_button(tr("Work zones"), |ui| {
                        for (n, text) in [(1u8, "1   (playlist only)"), (2, "2   (playlist + panel)"), (3, "3   (playlist + two panels)")] {
                            let b = egui::Button::new(text).shortcut_text(format!("Ctrl+{n}")).selected(self.workzones.panels == n);
                            if ui.add(b).clicked() {
                                self.workzones.panels = n;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.add(egui::Button::new(tr("Additional (new window)")).shortcut_text("Ctrl+4")).clicked() {
                            self.workzones.new_aux(Tab::AuxPlaylist);
                            ui.close_menu();
                        }
                        if !self.workzones.aux.is_empty() {
                            ui.label(RichText::new(format!("{} Additional window(s) open", self.workzones.aux.len())).small().color(GRAY));
                            if ui.button(tr("Close all Additional windows")).clicked() {
                                for a in self.workzones.aux.clone() {
                                    self.browsers.close_window(&crate::workzones::aux_title(a.id));
                                    self.workzones.close_aux(a.id);
                                }
                                ui.close_menu();
                            }
                        }
                    });
                    ui.add(egui::Checkbox::new(&mut self.workzones.show_left, "Left panel")).on_hover_text(tr("Ctrl+L"));
                    ui.separator();
                    for (tab, key) in [(Tab::FileExplorer, "F1"), (Tab::Search, "F2")] {
                        if ui.add(egui::Button::new(self.tab_name(tab)).shortcut_text(key)).clicked() {
                            self.workzones.show_left = true;
                            self.workzones.left.add(tab);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let mut dark = self.workzones.dark;
                    if ui.checkbox(&mut dark, tr("Dark theme")).changed() {
                        self.workzones.dark = dark;
                        crate::theme::apply(ctx, dark, self.workzones.scale);
                    }
                    if ui.add(egui::Button::new(tr("Full screen")).shortcut_text("F11").selected(self.fullscreen)).clicked() {
                        self.fullscreen = !self.fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                        ui.close_menu();
                    }
                });
                ui.menu_button(tr("Playlist"), |ui| self.playlist_menu(ui, core));
                ui.menu_button(tr("Tools"), |ui| {
                    for w in [
                        ToolWin::FadeEditor,
                        ToolWin::Generator,
                        ToolWin::Ads,
                        ToolWin::Scheduler,
                        ToolWin::AudioFx,
                        ToolWin::Streams,
                        ToolWin::AzuraCast,
                        ToolWin::Reports,
                        ToolWin::History,
                        ToolWin::Cleanup,
                        ToolWin::Notepad,
                        ToolWin::Plugins,
                    ] {
                        let sc = match w {
                            ToolWin::Generator => "Ctrl+G",
                            ToolWin::Scheduler => "F3",
                            ToolWin::AudioFx => "F4",
                            ToolWin::Notepad => "T",
                            _ => "",
                        };
                        if ui
                            .add_enabled(
                                w.allowed(&self.perm),
                                egui::Button::new(format!("{}  {}…", w.icon(), tr(w.label()))).shortcut_text(sc),
                            )
                            .on_disabled_hover_text(tr("Your account isn't allowed to use this tool"))
                            .clicked()
                        {
                            self.launch_tool(core, w);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let vt = self.perm.edit_playlists && self.perm.tool(crate::auth::tool::VOICE_TRACK);
                    if ui
                        .add_enabled(vt, egui::Button::new(tr("🔴 Record voice track…")))
                        .on_disabled_hover_text(tr("Your account isn't allowed to do this"))
                        .clicked()
                    {
                        self.show_vt = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button(tr("Jingles"), |ui| {
                    ui.label(RichText::new(tr("Cart wall")).small().color(GRAY));
                    if !self.perm.jingles {
                        ui.label(RichText::new(tr("Your account isn't allowed to play jingles")).small().color(AMBER));
                        ui.disable();
                    }
                    let mut fire = None;
                    for (i, c) in core.cfg.carts.iter().enumerate().take(12) {
                        let label = if c.label.is_empty() { format!("{}. (empty)", i + 1) } else { format!("{}. {}", i + 1, c.label) };
                        if ui.add_enabled(!c.path.is_empty(), egui::Button::new(label)).clicked() {
                            fire = Some(i);
                            ui.close_menu();
                        }
                    }
                    if let Some(i) = fire {
                        core.trigger_cart(i);
                    }
                    ui.separator();
                    if ui.add(egui::Button::new(tr("■ Stop all jingles")).shortcut_text("Ctrl+J")).clicked() {
                        core.stop_carts();
                        ui.close_menu();
                    }
                    if ui.button(tr("Show cart wall")).clicked() {
                        self.workzones.show_left = true;
                        self.workzones.left.add(Tab::Carts);
                        ui.close_menu();
                    }
                });
                ui.menu_button(tr("Settings"), |ui| {
                    if !self.perm.settings {
                        ui.disable();
                    }
                    if ui.add(egui::Button::new(tr("⚙ Settings…")).shortcut_text("Ctrl+,")).clicked() {
                        self.show_settings = true;
                        ui.close_menu();
                    }
                    if ui.button(tr("Station profiles…")).clicked() {
                        self.show_profiles = true;
                        ui.close_menu();
                    }
                    if ui.button(tr("Track List Repeat Protection…")).clicked() {
                        self.show_repeat = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button(tr("Help"), |ui| {
                    if ui.add(egui::Button::new(tr("Performance")).shortcut_text("Ctrl+Shift+P").selected(self.perf.is_some())).clicked() {
                        self.perf = if self.perf.is_some() { None } else { Some(Perf::default()) };
                        ui.close_menu();
                    }
                    if ui.add(egui::Button::new(tr("Keyboard shortcuts")).shortcut_text("Ctrl+/")).clicked() {
                        self.show_keys = true;
                        ui.close_menu();
                    }
                    if ui.button(tr("Check for updates…")).clicked() {
                        crate::updater::check(self.updates.clone(), ctx.clone(), true);
                        self.show_update = true;
                        ui.close_menu();
                    }
                    if ui.button(tr("About RadioMaster")).clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
                ui.separator();
                // quick buttons
                if ui.add_enabled(self.perm.settings, egui::Button::new("⚙")).on_hover_text(tr("Settings")).clicked() {
                    self.show_settings = true;
                }
                for (w, short) in [
                    (ToolWin::FadeEditor, "Track editor"),
                    (ToolWin::Generator, "Generator"),
                    (ToolWin::Ads, "Ads"),
                    (ToolWin::Scheduler, "Scheduler"),
                    (ToolWin::AudioFx, "Audio FX"),
                    (ToolWin::Streams, "Streams"),
                    (ToolWin::AzuraCast, "AzuraCast"),
                ] {
                    if ui.add_enabled(w.allowed(&self.perm), egui::Button::new(tr(short))).on_hover_text(tr(w.label())).clicked() {
                        self.launch_tool(core, w);
                    }
                }
                ui.separator();
                for n in 1..=3u8 {
                    if ui
                        .selectable_label(self.workzones.panels == n, n.to_string())
                        .on_hover_text(format!("Work zone {n}: {n} panel(s) (Ctrl+{n})"))
                        .clicked()
                    {
                        self.workzones.panels = n;
                    }
                }
                if ui.button("➕🗖").on_hover_text(tr("Additional window (Ctrl+4) — open as many as you like")).clicked() {
                    self.workzones.new_aux(Tab::AuxPlaylist);
                }
                let new_version = self.updates.lock().available.as_ref().map(|a| a.version.clone());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(tr("Master")).strong().size(15.0).color(Color32::from_rgb(20, 150, 235)));
                    ui.add_space(-ui.spacing().item_spacing.x);
                    ui.label(RichText::new(tr("Radio")).strong().size(15.0));
                    if let Some(t) = &icon {
                        ui.add(egui::Image::new((t.id(), egui::vec2(18.0, 18.0))));
                    }
                    self.server_chip(ui);
                    if let Some(v) = &new_version {
                        let b = egui::Button::new(RichText::new(format!("⬆ Update available: v{v}")).strong().color(Color32::WHITE))
                            .fill(Color32::from_rgb(20, 150, 235));
                        if ui
                            .add(b)
                            .on_hover_text(tr("A newer RadioMaster is on GitHub — click to see what's new and install it"))
                            .clicked()
                        {
                            self.show_update = true;
                        }
                    }
                });
            });
        });
        self.about_window(ctx, logo.as_ref());
    }

    fn about_window(&mut self, ctx: &egui::Context, logo: Option<&egui::TextureHandle>) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        let mut ok = false;
        Self::window_for(ctx, "About RadioMaster", [560.0, 420.0])
            .id(egui::Id::new("about_window"))
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (k, name) in ["About", "Contributors", "Credits"].into_iter().enumerate() {
                        ui.selectable_value(&mut self.about_tab, k, name);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(290.0).show(ui, |ui| match self.about_tab {
                    0 => {
                        if let Some(t) = logo {
                            let size = t.size_vec2();
                            ui.add(egui::Image::new((t.id(), egui::vec2(280.0, 280.0 * size.y / size.x))));
                        }
                        ui.label(tr("RadioMaster — radio automation for Icecast, Shoutcast and AzuraCast, written in Rust."));
                        ui.label(
                            RichText::new(format!(
                                "Version {} ({}, {})",
                                crate::updater::CURRENT,
                                env!("RM_GIT_HASH"),
                                env!("RM_BUILD_DATE")
                            ))
                            .monospace(),
                        );
                        ui.add_space(10.0);
                        ui.vertical_centered(|ui| {
                            let b = egui::Button::new(RichText::new(tr("   Join us on Discord   ")).strong().color(Color32::WHITE))
                                .fill(Color32::from_rgb(30, 140, 230))
                                .corner_radius(14);
                            if ui.add(b).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab("https://discord.gg/zVxeUXCjhc"));
                            }
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.hyperlink_to("RadioMaster website", "https://radiomaster.dovafm.com/");
                                ui.label("·");
                                ui.hyperlink_to("GitHub", "https://github.com/epell-development/RadioMaster");
                                ui.label("·");
                                ui.hyperlink_to("DovaFM DEG", "https://deg.dovafm.com");
                            });
                        });
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(tr("A DovaFM Digital Entertainment Group brand. Free and open source under the MIT licence."))
                                .small()
                                .color(GRAY),
                        );
                        ui.label(RichText::new(tr("egui · rodio · symphonia · LAME")).small().color(GRAY));
                    }
                    1 => {
                        let mut section = String::new();
                        for line in include_str!("../CONTRIBUTORS.md").lines() {
                            if let Some(h) = line.strip_prefix("## ") {
                                section = h.to_string();
                            } else if let Some(entry) = line.strip_prefix("- ") {
                                if !section.is_empty() {
                                    ui.add_space(6.0);
                                    ui.strong(std::mem::take(&mut section));
                                }
                                ui.label(entry);
                            }
                        }
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(tr(
                                "Want to be listed? Contribute code, a translation or testing — see CONTRIBUTORS.md on GitHub.",
                            ))
                            .small()
                            .color(GRAY),
                        );
                    }
                    _ => {
                        ui.label(tr("RadioMaster is built on these open-source projects:"));
                        ui.add_space(4.0);
                        egui::Grid::new("credits").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                            for (name, what) in [
                                ("egui / eframe", "user interface"),
                                ("rodio · cpal", "audio output and input"),
                                ("symphonia", "MP3, AAC, FLAC, OGG and WAV decoding"),
                                ("LAME (mp3lame-encoder)", "MP3 encoding for streaming"),
                                ("lofty", "tags and album art"),
                                ("wry · Microsoft Edge WebView2", "embedded web panels"),
                                ("ureq · rustls", "AzuraCast, updates and HTTP"),
                                ("tiny_http", "remote control API"),
                                ("argon2", "password hashing"),
                                ("serde · chrono · image · rfd", "settings, time, images, file dialogs"),
                            ] {
                                ui.strong(name);
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(tr("Each keeps its own licence (MIT, Apache-2.0, MPL-2.0 or LGPL for LAME)."))
                                .small()
                                .color(GRAY),
                        );
                    }
                });
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ok = ui.button(RichText::new(tr("   OK   ")).strong()).clicked();
                });
            });
        self.show_about = open && !ok;
    }

    // ----------------------------------------------------------- status bar
    fn status_bar(&self, ctx: &egui::Context, core: &Playout) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(core.notify.lock().clone());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(Local::now().format("%a %d %b  %H:%M:%S").to_string());
                    ui.separator();
                    let live = core.handles.iter().flatten().filter(|h| h.status.lock().state == EncState::Live).count();
                    ui.label(RichText::new(format!("● {live} stream(s) live")).color(if live > 0 { GREEN } else { GRAY }));
                    if let Some(np) = &core.azura.lock().np {
                        ui.separator();
                        ui.label(format!("👥 {} listeners ({} unique)", np.listeners.current, np.listeners.unique));
                    }
                });
            });
        });
    }

    // --------------------------------------------------------------- player
    fn player(&mut self, ctx: &egui::Context, core: &mut Playout) {
        egui::TopBottomPanel::top("player").show(ctx, |ui| {
            ui.add_space(4.0);
            let now = Local::now();
            let st = core.status.clone();
            let title = Some(core.on_air_text()).filter(|t| !t.is_empty()).unwrap_or_else(|| "No track playing".into());
            let next_i = core.current.map(|i| i + 1).unwrap_or(0);
            let coming = core
                .playlist
                .iter()
                .skip(next_i)
                .find(|e| !matches!(e.kind, crate::items::ItemKind::Comment(_) | crate::items::ItemKind::HourMarker { .. }))
                .map(|p| p.title())
                .unwrap_or_else(|| "Queue empty".into());
            let previous =
                core.history.iter().rev().nth(if st.has_track { 1 } else { 0 }).map(|h| h.text.clone()).unwrap_or_else(|| "—".into());
            let marker = core.secs_to_next_marker();

            // ---- row 1: clock / last / on air / next (widths derived from the real space available)
            let gap = ui.spacing().item_spacing.x;
            let margin = 8.0;
            let usable = (ui.available_width() - 3.0 * gap - 4.0 * 2.0 * margin - 6.0).max(520.0);
            let clock_w = 228.0;
            let rest = (usable - clock_w).max(300.0);
            let tiles = [clock_w, rest * 0.26, rest * 0.46, rest * 0.28];
            let tile = |ui: &mut egui::Ui, fill: Color32, w: f32, f: &mut dyn FnMut(&mut egui::Ui)| {
                egui::Frame::default().fill(fill).corner_radius(4).inner_margin(margin as i8).show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        ui.set_min_height(46.0);
                        f(ui);
                    });
                });
            };
            ui.horizontal(|ui| {
                tile(ui, Color32::from_rgb(232, 236, 243), tiles[0], &mut |ui| {
                    ui.label(RichText::new(now.format("%a %d/%m/%Y").to_string()).color(Color32::from_rgb(70, 78, 92)).small());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(now.format("%H:%M:%S").to_string())
                                .monospace()
                                .size(22.0)
                                .strong()
                                .color(Color32::from_rgb(20, 24, 32)),
                        );
                        // countdown to the top of the next hour
                        let into = now.minute() * 60 + now.second();
                        let left = 3600 - into;
                        let next_h = (now.hour() + 1) % 24;
                        ui.label(
                            RichText::new(format!("{:02}:{:02}", left / 60, left % 60))
                                .monospace()
                                .size(17.0)
                                .strong()
                                .color(Color32::from_rgb(0, 128, 128)),
                        )
                        .on_hover_text(format!("Time left until {next_h:02}:00"));
                        if let Some(m) = marker.filter(|m| *m < 6 * 3600) {
                            ui.label(
                                RichText::new(format!("⏰{:02}:{:02}", m / 60, m % 60))
                                    .monospace()
                                    .size(13.0)
                                    .color(Color32::from_rgb(170, 40, 120)),
                            )
                            .on_hover_text(tr("Time until the next hour marker in the playlist"));
                        }
                    });
                });
                tile(ui, Color32::from_rgb(255, 214, 170), tiles[1], &mut |ui| {
                    ui.label(RichText::new(tr("LAST TRACK")).color(Color32::from_rgb(90, 60, 20)).small());
                    ui.add(egui::Label::new(RichText::new(previous.clone()).color(Color32::BLACK)).truncate());
                });
                tile(ui, Color32::from_rgb(176, 240, 186), tiles[2], &mut |ui| {
                    ui.label(RichText::new(tr("●  ON AIR")).strong().color(Color32::from_rgb(18, 90, 34)).small());
                    ui.add(egui::Label::new(RichText::new(title.clone()).color(Color32::BLACK).size(17.0).strong()).truncate());
                });
                tile(ui, Color32::from_rgb(245, 246, 190), tiles[3], &mut |ui| {
                    ui.label(RichText::new(tr("COMING UP NEXT")).color(Color32::from_rgb(80, 80, 30)).small());
                    ui.add(egui::Label::new(RichText::new(coming.clone()).color(Color32::BLACK)).truncate());
                });
            });
            ui.add_space(4.0);
            ui.separator();

            // ---- row 2: transport | elapsed | track + seek | meters + mic (also width-aware)
            let total = ui.available_width();
            let bw = 44.0;
            let transport_w = 5.0 * bw + 4.0 * gap;
            let elapsed_w = 118.0;
            let meters_w = (total * 0.20).clamp(170.0, 260.0);
            let sep = 2.0 * gap + 8.0;
            let mid_w = (total - transport_w - elapsed_w - meters_w - 3.0 * sep).max(200.0);
            ui.horizontal(|ui| {
                ui.allocate_ui(egui::vec2(transport_w, 78.0), |ui| {
                    ui.vertical(|ui| {
                        let bs = egui::vec2(bw, 34.0);
                        let may_play = self.perm.playback;
                        ui.horizontal(|ui| {
                            if !may_play {
                                ui.disable();
                            }
                            if ui.add_sized(bs, egui::Button::new("⏮")).on_hover_text(tr("Previous")).clicked() {
                                core.prev();
                            }
                            if ui
                                .add_sized(bs, egui::Button::new(RichText::new("▶").color(Color32::from_rgb(20, 140, 50))))
                                .on_hover_text(tr("Play"))
                                .clicked()
                            {
                                let s = self.sel.iter().next().cloned();
                                core.play_pressed(s);
                            }
                            if ui.add_sized(bs, egui::Button::new("⏸")).on_hover_text(tr("Pause")).clicked() {
                                core.pause_toggle();
                            }
                            if ui
                                .add_sized(bs, egui::Button::new(RichText::new("⏹").color(Color32::from_rgb(200, 40, 40))))
                                .on_hover_text(tr("Stop"))
                                .clicked()
                            {
                                core.stop();
                            }
                            if ui.add_sized(bs, egui::Button::new("⏭")).on_hover_text(tr("Next")).clicked() {
                                core.next();
                            }
                        });
                        ui.spacing_mut().slider_width = (transport_w - 95.0).max(70.0);
                        ui.horizontal(|ui| {
                            ui.label(tr("Vol "));
                            let mut v = core.cfg.volume;
                            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                                core.set_volume(v);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(tr("Fade"));
                            let smart = core.cfg.fade.mode == crate::fade::FadeMode::Smart;
                            if ui
                                .selectable_label(smart, "Smart")
                                .on_hover_text(tr("Smart fading: analyse each track and pick the mix point automatically"))
                                .clicked()
                            {
                                core.cfg.fade.mode = if smart { crate::fade::FadeMode::Fixed } else { crate::fade::FadeMode::Smart };
                            }
                            if !smart {
                                ui.add(egui::Slider::new(&mut core.cfg.crossfade_secs, 0.0..=15.0).suffix(" s"));
                            }
                        });
                    });
                });
                ui.separator();
                ui.allocate_ui(egui::vec2(elapsed_w, 78.0), |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(tr("ELAPSED")).small().color(GRAY));
                        ui.label(RichText::new(fmt(st.pos)).monospace().size(32.0).strong().color(ACCENT));
                        ui.label(RichText::new(format!("-{}", fmt(st.total - st.pos))).monospace().size(15.0).color(GRAY));
                    });
                });
                ui.separator();
                ui.allocate_ui(egui::vec2(mid_w, 78.0), |ui| {
                    let on_air_path = core.on_air.as_ref().filter(|_| st.has_track).map(|t| t.path.clone());
                    self.album_art(ui, on_air_path.as_deref(), 70.0);
                    let mid_w = (mid_w - 80.0).max(120.0);
                    ui.vertical(|ui| {
                        let (t, artist) = match &core.on_air {
                            Some(tr) if st.has_track => {
                                (if tr.title.is_empty() { core.on_air_label.clone() } else { tr.title.clone() }, tr.artist.clone())
                            }
                            _ if core.wait_until.is_some() => (core.on_air_text(), String::new()),
                            _ => ("— idle —".to_string(), String::new()),
                        };
                        ui.add(egui::Label::new(RichText::new(t).size(19.0).strong()).truncate());
                        ui.add(egui::Label::new(RichText::new(artist).color(GRAY)).truncate());
                        let mut p = self.seek_drag.unwrap_or(st.pos);
                        ui.spacing_mut().slider_width = (mid_w - 16.0).max(100.0);
                        let r = ui.add(egui::Slider::new(&mut p, 0.0..=st.total.max(1.0)).show_value(false));
                        if r.dragged() {
                            self.seek_drag = Some(p);
                        }
                        if r.drag_stopped() {
                            core.seek(p);
                            self.seek_drag = None;
                        } else if r.changed() && !r.dragged() {
                            core.seek(p);
                        }
                    });
                });
                ui.separator();
                ui.allocate_ui(egui::vec2(meters_w, 78.0), |ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(tr("L"));
                            vu(ui, st.peak_l, meters_w - 62.0);
                            let (txt, col) = match core.state {
                                PlayState::Playing => ("PLAY", Color32::from_rgb(20, 140, 50)),
                                PlayState::Paused => ("PAUSE", Color32::from_rgb(190, 120, 0)),
                                PlayState::Stopped => ("STOP", GRAY),
                            };
                            ui.label(RichText::new(txt).small().strong().color(col));
                        });
                        ui.horizontal(|ui| {
                            ui.label(tr("R"));
                            vu(ui, st.peak_r, meters_w - 62.0);
                        });
                        ui.horizontal(|ui| {
                            let r = ui
                                .add_enabled(self.perm.playback, egui::SelectableLabel::new(self.mic_on, "🎤 Mic"))
                                .on_hover_text(tr("Microphone on/off (Ctrl+M) · right-click: what the music does"));
                            if r.clicked() {
                                self.toggle_mic(core);
                            }
                            r.context_menu(|ui| {
                                use crate::config::MicMode;
                                ui.label(RichText::new(tr("While the mic is on")).small().color(GRAY));
                                ui.radio_value(&mut core.cfg.mic_mode, MicMode::Duck, tr("Lower the music (slider)"));
                                ui.radio_value(&mut core.cfg.mic_mode, MicMode::Mute, tr("Mute the music"));
                                ui.radio_value(&mut core.cfg.mic_mode, MicMode::Pause, tr("Pause the music, carry on after"));
                                ui.radio_value(&mut core.cfg.mic_mode, MicMode::Keep, tr("Keep the music at full level"));
                            });
                            ui.spacing_mut().slider_width = ((meters_w - 90.0) / 2.0).max(36.0);
                            ui.add(egui::Slider::new(&mut core.cfg.mic_gain, 0.0..=2.0).show_value(false))
                                .on_hover_text(tr("Microphone volume"));
                            use crate::config::MicMode;
                            ui.add_enabled(
                                core.cfg.mic_mode == MicMode::Duck,
                                egui::Slider::new(&mut core.cfg.mic_duck, 0.0..=1.0).show_value(false),
                            )
                            .on_hover_text(tr("Music level while the mic is on (left = silent, right = no ducking)"))
                            .on_disabled_hover_text(tr(
                                "The music is muted, paused or kept at full level while the mic is on (right-click the Mic button to change)",
                            ));
                        });
                    });
                });
            });
            ui.add_space(4.0);
        });
    }

    fn tab_queue(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong(tr("PLAYBACK QUEUE"));
            if ui.button(tr("Add files…")).clicked() {
                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AddFiles));
            }
        });
        let start = core.current.map(|i| i + 1).unwrap_or(0);
        let mut play = None;
        for (index, entry) in core.playlist.iter().enumerate().skip(start).take(50) {
            ui.horizontal(|ui| {
                if ui.small_button("▶").on_hover_text(tr("Play this item next")).clicked() {
                    play = Some(index);
                }
                ui.label(RichText::new(format!("{:03}", index + 1)).monospace().color(GRAY));
                ui.add(egui::Label::new(entry.title()).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(fmt(entry.track.duration));
                });
            });
        }
        if let Some(i) = play {
            let fade = if core.state == PlayState::Playing { core.cfg.crossfade_secs } else { 0.0 };
            core.play_index(i, fade);
        }
        if start >= core.playlist.len() {
            ui.label(tr("Queue is empty. Add music to the playlist."));
        }
    }

    fn tab_search(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong(tr("SEARCH LIBRARY"));
            if ui.button(tr("Rescan")).clicked() {
                core.start_scan();
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.lib_filter)
                .hint_text(tr("Search artist, title, album, genre…"))
                .desired_width(f32::INFINITY),
        );
        if self.lib_filter.trim().is_empty() {
            ui.label(tr("Enter a search to find music in your library."));
            return;
        }
        let needle = self.lib_filter.to_lowercase();
        let hits: Vec<usize> = core
            .library
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.artist.to_lowercase().contains(&needle)
                    || t.title.to_lowercase().contains(&needle)
                    || t.album.to_lowercase().contains(&needle)
                    || t.genre.to_lowercase().contains(&needle)
            })
            .take(150)
            .map(|(i, _)| i)
            .collect();
        ui.label(format!("{} results (up to 150 shown)", hits.len()));
        let mut append = None;
        for i in hits {
            let track = &core.library[i];
            ui.horizontal(|ui| {
                if ui.add_enabled(self.perm.edit_playlists, egui::Button::new("➕").small()).on_hover_text(tr("Add to playlist")).clicked()
                {
                    append = Some(i);
                }
                ui.add(egui::Label::new(track.label()).truncate());
                ui.monospace(fmt(track.duration));
            });
        }
        if let Some(i) = append {
            let p = core.library[i].path.clone();
            let v = self.view_pl;
            core.with_tab(v, |c| c.add_paths(vec![p]));
        }
    }

    fn tab_statistics(&mut self, ui: &mut egui::Ui, core: &Playout) {
        ui.strong(tr("STATION STATISTICS"));
        ui.separator();
        ui.label(format!("Playlist tracks: {}", core.playlist.len()));
        ui.label(format!("Total playlist length: {}", fmt(core.total_secs())));
        ui.label(format!("History entries this session: {}", core.history.len()));
        let live = core.handles.iter().flatten().filter(|h| h.status.lock().state == EncState::Live).count();
        ui.colored_label(if live > 0 { GREEN } else { GRAY }, format!("Connected encoders: {live}"));
        if let Some(np) = &core.azura.lock().np {
            ui.label(format!("Listeners: {} ({} unique)", np.listeners.current, np.listeners.unique));
        } else {
            ui.label(tr("Listeners: not available from AzuraCast"));
        }
        ui.separator();
        ui.small(tr("Live station status; historical audience graphs require a stats source."));
    }

    fn tab_clock(&mut self, ui: &mut egui::Ui, core: &Playout) {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new(tr("STUDIO CLOCK")).small().color(GRAY));
            ui.label(RichText::new(Local::now().format("%H:%M:%S").to_string()).monospace().size(51.0).strong().color(ACCENT));
            ui.label(Local::now().format("%A, %d %B %Y").to_string());
            if let Some(m) = core.secs_to_next_marker() {
                ui.label(
                    RichText::new(format!("Next hour marker in {:02}:{:02}:{:02}", m / 3600, (m / 60) % 60, m % 60))
                        .color(Color32::from_rgb(170, 40, 120)),
                );
            }
        });
        ui.separator();
        ui.colored_label(GREEN, "ON AIR");
        let on = core.on_air_text();
        ui.label(if on.is_empty() { "Silence / idle".to_string() } else { on });
        ui.separator();
        ui.strong(tr("Projected start times"));
        let starts = core.projected_starts(true);
        let next = core.current.map(|n| n + 1).unwrap_or(0);
        for (i, e) in core.playlist.iter().enumerate().skip(next).take(14) {
            ui.horizontal(|ui| {
                let t = starts.get(i).copied().flatten();
                ui.monospace(t.map(|s| s.at.format("%H:%M:%S").to_string()).unwrap_or_else(|| "--:--:--".into()));
                let mut txt = RichText::new(e.title());
                if t.is_some_and(|s| s.skipped) {
                    txt = txt.strikethrough().color(GRAY);
                }
                ui.label(txt);
            });
        }
    }

    fn tab_segment(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong(tr("SEGMENT / SCRIPT"));
            if ui.button(tr("Save")).clicked() {
                let _ = std::fs::write(data_dir().join("segment.txt"), &self.segment_text);
                core.say("Segment saved");
            }
            if ui.button(tr("New")).clicked() {
                self.segment_text.clear();
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.segment_name).desired_width(220.0));
        ui.add(egui::TextEdit::multiline(&mut self.segment_text).desired_width(f32::INFINITY).desired_rows(20));
        ui.small(tr("Presenter notes are local text; they are not automatically broadcast."));
    }

    fn settings_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_settings {
            return;
        }
        if !self.perm.settings {
            self.show_settings = false;
            core.say("Your account isn't allowed to change settings");
            return;
        }
        let mut open = self.show_settings;
        let mut close = false;
        Self::window_for(ctx,"RadioMaster Settings",[900.0,620.0]).id(egui::Id::new("settings_window"))
            .open(&mut open).collapsible(false).resizable(true).show(ctx,|ui| {
                // footer first so Save / Close can never be pushed off-screen
                egui::TopBottomPanel::bottom("settings_footer").exact_height(40.0).show_inside(ui,|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                        if ui.button(tr("Close")).clicked() {close=true;}
                        if ui.button(tr("Save configuration")).clicked() {core.save_all(); let _=self.workzones.save();}
                    });
                });
                egui::SidePanel::left("settings_pages").resizable(false).exact_width(178.0).show_inside(ui,|ui| {
                    ui.heading(tr("Settings"));
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt("settings_pages_scroll").show(ui,|ui| {
                        for page in SettingsPage::ALL {
                            if ui.selectable_label(self.settings_page==page,tr(page.label())).clicked() {
                                self.settings_page=page;
                            }
                        }
                    });
                });
                egui::CentralPanel::default().show_inside(ui,|ui| {
                    ui.heading(tr(self.settings_page.label()));
                    ui.separator();
                    egui::ScrollArea::both().id_salt("settings_content").auto_shrink([false,false]).show(ui,|ui| {
                        match self.settings_page {
                            SettingsPage::General=>{
                                ui.label(tr("Radio automation, playback and studio settings."));
                                ui.label(format!("Data folder: {}",data_dir().display()));
                                ui.label(format!("Active profile: {}",self.profile_name));
                                if ui.button(tr("Station profiles…")).clicked(){self.show_profiles=true;}
                                ui.separator();
                                ui.checkbox(&mut core.cfg.check_updates, tr("Check for updates when RadioMaster starts"));
                                ui.separator();
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(tr("Language"));
                                    let before = core.cfg.language.clone();
                                    egui::ComboBox::from_id_salt("language")
                                        .selected_text(if core.cfg.language.is_empty() { "English".to_string() } else { core.cfg.language.clone() })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut core.cfg.language, String::new(), "English");
                                            for l in crate::i18n::languages() {
                                                ui.selectable_value(&mut core.cfg.language, l.clone(), l);
                                            }
                                        });
                                    if core.cfg.language != before {
                                        match crate::i18n::load(&core.cfg.language) {
                                            Ok(n) => core.say(format!("{}: {n}", tr("Language loaded, translated texts"))),
                                            Err(e) => core.say(e),
                                        }
                                    }
                                    if ui.button(tr("Export translation template")).on_hover_text(tr("Writes a file with every text to translate into the lang folder of your data folder")).clicked() {
                                        match crate::i18n::export_template(&core.cfg.language) {
                                            Ok(p) => {
                                                core.say(format!("{} {}", tr("Saved"), p.display()));
                                                if let Some(d) = p.parent() {
                                                    crate::plugins::open_folder(d);
                                                }
                                            }
                                            Err(e) => core.say(e),
                                        }
                                    }
                                });
                                ui.label(RichText::new(tr("Translate RadioMaster: export the template, fill in the right-hand side of each line, save it as e.g. Deutsch.json in the lang folder, then pick it here. Share it on GitHub so everyone gets it.")).small().color(GRAY));
                                ui.label(RichText::new(format!("This is version {}. Help → Check for updates looks right now.", crate::updater::CURRENT)).small().color(GRAY));
                            },
                            SettingsPage::Library=>{
                                ui.label(tr("Music folders scanned by the library"));
                                let mut remove=None;
                                for (i,path) in core.cfg.library_folders.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(path);
                                        if ui.small_button(tr("Remove")).clicked(){remove=Some(i);}
                                    });
                                }
                                if let Some(i)=remove {core.cfg.library_folders.remove(i);}
                                if ui.button(tr("Add folder…")).clicked() {
                                    self.pick_library_folder();
                                }
                                if ui.button(tr("Rescan")).clicked(){core.start_scan();}
                            },
                            SettingsPage::Playback=>{
                                ui.checkbox(&mut core.cfg.loop_playlist,"Loop the playlist");
                                ui.checkbox(&mut core.cfg.auto_enabled,"Enable automated playlist top-up");
                                ui.checkbox(&mut core.cfg.random_fill,"Fill from music library");
                                ui.separator();
                                ui.heading(tr("Transitions"));
                                self.smart_fade_settings(ui, core);
                            },
                            SettingsPage::Streaming=>self.tab_streaming(ui,core),
                            SettingsPage::Audio=>self.tab_dsp(ui,core),
                            SettingsPage::Devices=>self.tab_devices(ui,core),
                            SettingsPage::Automation=>self.tab_settings(ui,core),
                            SettingsPage::Remote=>{
                                ui.label(tr("Remote control API (restart required)"));
                                ui.checkbox(&mut core.cfg.remote.enabled,"Enable remote API");
                                ui.add(egui::DragValue::new(&mut core.cfg.remote.port).range(1..=65535).prefix("Port "));
                                ui.add(egui::TextEdit::singleline(&mut core.cfg.remote.password).password(true).hint_text(tr("API password")));
                                ui.colored_label(AMBER,"Set a strong password before enabling network access.");
                            },
                            SettingsPage::Appearance=>{
                                if ui.checkbox(&mut self.workzones.dark,"Dark theme").changed() {
                                    crate::theme::apply(ctx, self.workzones.dark, self.workzones.scale);
                                }
                                ui.horizontal(|ui| {
                                    ui.label(tr("Interface size"));
                                    let r = ui.add(egui::Slider::new(&mut self.workzones.scale, 0.8..=1.6).show_value(false));
                                    ui.label(format!("{}%", (self.workzones.scale * 100.0).round() as i32));
                                    if (r.drag_stopped() || (r.changed() && !r.dragged())) && !ui.input(|i| i.pointer.any_down()) {
                                        crate::theme::apply(ctx, self.workzones.dark, self.workzones.scale);
                                    }
                                    if ui.button(tr("Reset")).clicked() {
                                        self.workzones.scale = 1.0;
                                        crate::theme::apply(ctx, self.workzones.dark, 1.0);
                                    }
                                });
                                ui.small(tr("Make everything larger or smaller if the text is hard to read."));
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(tr("Work zone"));
                                    for n in 1..=3u8 {
                                        ui.selectable_value(&mut self.workzones.panels, n, format!("{n} panel(s)"));
                                    }
                                });
                                ui.small(tr("View → Work zones (Ctrl+1/2/3) sets how many panels the main window has; Ctrl+4 opens another Additional window."));
                                if ui.button(tr("Reset panel layout")).clicked() {
                                    let d = WorkzoneSet::default();
                                    self.workzones.left = d.left;
                                    self.workzones.top = d.top;
                                    self.workzones.bottom = d.bottom;
                                    self.workzones.panels = d.panels;
                                    self.workzones.show_left = d.show_left;
                                }
                            },
                            SettingsPage::Accounts=>{
                                let admin=self.logged_in.as_ref().is_some_and(|u|u.admin);
                                if admin && ui.button(tr("Manage operator accounts…")).clicked(){self.show_users=true;}
                                if !admin {ui.label(tr("Only administrators can manage user accounts."));}
                                ui.small(tr("Accounts gate the local UI; secure your Windows account and streaming API separately."));
                            },
                        }

                    });
                });
            });
        self.show_settings = open && !close;
    }

    fn authentication(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                if let Some(t) = self.logo_texture(ui.ctx()) {
                    let size = t.size_vec2();
                    ui.add(egui::Image::new((t.id(), egui::vec2(340.0, 340.0 * size.y / size.x))));
                    ui.add_space(10.0);
                }
                ui.heading(tr("Studio Access"));
                ui.add_space(18.0);
                if self.accounts.users.is_empty() {
                    ui.label(tr("First run: create your administrator account."));
                } else {
                    ui.label(tr("Sign in to access station controls."));
                }
                ui.add_sized([300.0, 26.0], egui::TextEdit::singleline(&mut self.login_name).hint_text(tr("Username")));
                ui.add_sized(
                    [300.0, 26.0],
                    egui::TextEdit::singleline(&mut self.login_password)
                        .password(true)
                        .hint_text(tr("Password (12+ characters for setup)")),
                );
                let enter =
                    ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.login_name.trim().is_empty() && !self.login_password.is_empty();
                if ui.button(if self.accounts.users.is_empty() { tr("Create administrator") } else { tr("Sign in") }).clicked() || enter {
                    if self.accounts.users.is_empty() {
                        match self.accounts.add(&self.login_name, &self.login_password, true) {
                            Ok(()) => {
                                self.logged_in = self.accounts.login(&self.login_name, &self.login_password);
                                self.auth_error.clear();
                            }
                            Err(e) => self.auth_error = e,
                        }
                    } else {
                        self.logged_in = self.accounts.login(&self.login_name, &self.login_password);
                        self.auth_error = if self.logged_in.is_some() { String::new() } else { "Invalid username or password".into() };
                    }
                    self.login_password.clear();
                    if self.logged_in.is_some() {
                        self.login_name.clear(); // the "add a user" form reuses this box
                    }
                }
                ui.colored_label(RED, &self.auth_error);
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);
                if ui
                    .add_sized([300.0, 26.0], egui::Button::new(tr("Connect to Remote Server…")))
                    .on_hover_text(tr("Run a station on a RadioMaster Server, signing in with the server's username and password instead of a local account"))
                    .clicked()
                {
                    self.open_connect(true);
                }
                ui.add_space(20.0);
            });
            });
        });
    }

    /// Microphone on / off.
    fn toggle_mic(&mut self, core: &mut Playout) {
        use crate::config::MicMode;
        if self.mic_on {
            self.mic_stream = None;
            self.mic_on = false;
            if std::mem::take(&mut self.mic_paused) && core.state == crate::playout::PlayState::Paused {
                core.pause_toggle(); // carry on where the music was
            }
        } else {
            // while connected the mic goes to the server, which puts it on air
            let device = &core.cfg.audio_devices.input;
            let started = self.server_mic(device).unwrap_or_else(|| crate::mic::start(core.engine.clone(), device));
            match started {
                Ok(s) => {
                    self.mic_stream = Some(s);
                    self.mic_on = true;
                    if core.cfg.mic_mode == MicMode::Pause && core.state == crate::playout::PlayState::Playing {
                        core.pause_toggle();
                        self.mic_paused = true;
                    }
                }
                Err(e) => core.say(format!("Mic error: {e}")),
            }
        }
    }

    /// "9 of 15 tools ▾" button: tick which tools a user may open.
    fn tools_picker(ui: &mut egui::Ui, id: &str, perms: &mut crate::auth::Perms) {
        use crate::auth::TOOLS;
        let n = perms.tools_allowed();
        let text = if n == TOOLS.len() { tr("All tools").to_string() } else { format!("{n} / {} {}", TOOLS.len(), tr("tools")) };
        egui::ComboBox::from_id_salt(("tools_picker", id)).width(120.0).height(460.0).selected_text(text).show_ui(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button(tr("All")).clicked() {
                    perms.denied = 0;
                }
                if ui.small_button(tr("None")).clicked() {
                    (0..TOOLS.len()).for_each(|t| perms.set_tool(t, false));
                }
            });
            ui.separator();
            for (t, (name, help)) in TOOLS.iter().enumerate() {
                let mut on = perms.tool(t);
                if ui.checkbox(&mut on, tr(name)).on_hover_text(tr(help)).changed() {
                    perms.set_tool(t, on);
                }
            }
        });
    }

    fn users_window(&mut self, ctx: &egui::Context) {
        if !self.show_users || !self.logged_in.as_ref().map(|u| u.admin).unwrap_or(false) {
            return;
        }
        use crate::auth::Perms;
        let mut open = self.show_users;
        let me = self.logged_in.as_ref().map(|u| u.name.clone()).unwrap_or_default();
        let mut changes: Vec<(String, bool, Perms, bool)> = vec![];
        let mut reset: Option<(String, String)> = None;
        let mut delete: Option<String> = None;
        Self::window_for(ctx, "Users and permissions", [1000.0, 520.0]).id(egui::Id::new("users_window")).open(&mut open).show(ctx, |ui| {
            ui.label(
                RichText::new(tr(
                    "Administrators can do everything, including managing users. Other operators only get the ticked permissions.",
                ))
                .small()
                .color(GRAY),
            );
            ui.add_space(4.0);
            egui::ScrollArea::both().max_height(300.0).show(ui, |ui| {
                egui::Grid::new("users_grid").striped(true).num_columns(11).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.strong(tr("User"));
                    ui.strong(tr("Admin"));
                    for (name, help) in Perms::LIST {
                        ui.strong(tr(name)).on_hover_text(tr(help));
                    }
                    ui.strong(tr("Tools")).on_hover_text(tr("Which tools this user can open"));
                    ui.strong(tr("Disabled"));
                    ui.strong(tr("New password"));
                    ui.label("");
                    ui.end_row();
                    for u in &self.accounts.users {
                        let (mut admin, mut perms, mut disabled) = (u.admin, u.perms, u.disabled);
                        ui.label(if u.name == me { RichText::new(format!("{} (you)", u.name)).strong() } else { RichText::new(&u.name) });
                        ui.checkbox(&mut admin, "");
                        for k in 0..Perms::LIST.len() {
                            ui.add_enabled(!admin, egui::Checkbox::without_text(perms.get_mut(k)));
                        }
                        ui.add_enabled_ui(!admin, |ui| Self::tools_picker(ui, &u.name, &mut perms));
                        ui.add_enabled(u.name != me, egui::Checkbox::without_text(&mut disabled))
                            .on_disabled_hover_text(tr("You can't disable your own account"));
                        let pw = self.user_pw.entry(u.name.clone()).or_default();
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(pw).password(true).hint_text(tr("12+ characters")).desired_width(120.0));
                            if ui.add_enabled(pw.len() >= 12, egui::Button::new(tr("Set"))).clicked() {
                                reset = Some((u.name.clone(), pw.clone()));
                            }
                        });
                        if self.user_delete.as_deref() == Some(&u.name) {
                            ui.horizontal(|ui| {
                                if ui.button(RichText::new(tr("Delete!")).color(RED)).clicked() {
                                    delete = Some(u.name.clone());
                                }
                                if ui.button(tr("Keep")).clicked() {
                                    self.user_delete = None;
                                }
                            });
                        } else if ui.add_enabled(u.name != me, egui::Button::new("🗑")).on_hover_text(tr("Delete this user")).clicked() {
                            self.user_delete = Some(u.name.clone());
                        }
                        ui.end_row();
                        if (admin, perms, disabled) != (u.admin, u.perms, u.disabled) {
                            changes.push((u.name.clone(), admin, perms, disabled));
                        }
                    }
                });
            });
            ui.separator();
            ui.strong(tr("Add a user"));
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.login_name).hint_text(tr("username")).desired_width(150.0));
                ui.add(
                    egui::TextEdit::singleline(&mut self.login_password)
                        .password(true)
                        .hint_text(tr("password (12+ characters)"))
                        .desired_width(190.0),
                );
                ui.checkbox(&mut self.new_user_admin, tr("Administrator"));
            });
            if !self.new_user_admin {
                ui.horizontal_wrapped(|ui| {
                    for (k, (name, help)) in Perms::LIST.iter().enumerate() {
                        ui.checkbox(self.new_user_perms.get_mut(k), tr(name)).on_hover_text(tr(help));
                    }
                    ui.label(tr("Tools:"));
                    Self::tools_picker(ui, "\u{0}new", &mut self.new_user_perms);
                });
            }
            if ui.button(RichText::new(tr("Create user")).strong()).clicked() {
                let perms = if self.new_user_admin { Perms::default() } else { self.new_user_perms };
                match self.accounts.add_with(&self.login_name, &self.login_password, self.new_user_admin, perms) {
                    Ok(()) => {
                        self.auth_error = format!("Created {}", self.login_name.trim());
                        self.login_name.clear();
                    }
                    Err(e) => self.auth_error = e,
                }
                self.login_password.clear();
            }
            if !self.auth_error.is_empty() {
                ui.colored_label(
                    if self.auth_error.starts_with("Created") || self.auth_error.starts_with("Saved") { GREEN } else { RED },
                    &self.auth_error,
                );
            }
        });
        for (name, admin, perms, disabled) in changes {
            self.auth_error = match self.accounts.update(&name, admin, perms, disabled) {
                Ok(()) => format!("Saved {name}"),
                Err(e) => e,
            };
        }
        if let Some((name, pw)) = reset {
            self.auth_error = match self.accounts.set_password(&name, &pw) {
                Ok(()) => format!("Saved a new password for {name}"),
                Err(e) => e,
            };
            self.user_pw.remove(&name);
        }
        if let Some(name) = delete {
            self.auth_error = match self.accounts.delete(&name) {
                Ok(()) => format!("Saved: {name} deleted"),
                Err(e) => e,
            };
            self.user_delete = None;
        }
        // keep the signed-in snapshot in step with edits to your own account
        if let Some(u) = self.logged_in.as_ref().and_then(|me| self.accounts.users.iter().find(|x| x.name == me.name)).cloned() {
            self.logged_in = Some(u);
        }
        self.show_users = open;
    }

    fn profiles_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_profiles {
            return;
        }
        let mut open = self.show_profiles;
        Self::window_for(ctx, "Station profiles", [460.0, 380.0]).open(&mut open).show(ctx, |ui| {
            ui.label(tr("Save or load individual station settings (including encoder passwords). Keep the profile directory private."));
            ui.horizontal(|ui| {
                ui.label(tr("Profile:"));
                ui.add(egui::TextEdit::singleline(&mut self.profile_name).desired_width(220.0));
                if ui.button(tr("Save snapshot")).clicked() {
                    let safe: String = self.profile_name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
                    if !safe.is_empty() {
                        let path = data_dir().join("profiles");
                        let _ = std::fs::create_dir_all(&path);
                        let _ = std::fs::write(path.join(format!("{safe}.json")), serde_json::to_vec_pretty(&core.cfg).unwrap_or_default());
                        if !self.profile_list.contains(&safe) {
                            self.profile_list.push(safe);
                        }
                    }
                }
            });
            for name in self.profile_list.clone() {
                ui.horizontal(|ui| {
                    ui.label(&name);
                    if ui.button(format!("Load##{name}")).clicked() {
                        let path = data_dir().join("profiles").join(format!("{name}.json"));
                        if let Ok(data) = std::fs::read_to_string(path) {
                            if let Ok(cfg) = serde_json::from_str::<crate::config::Config>(&data) {
                                core.cfg = cfg;
                                core.save_all();
                                core.say(format!("Profile {name} loaded. Restart to apply all engine/network changes."));
                            }
                        }
                    }
                });
            }
        });
        self.show_profiles = open;
    }

    /// Keep the filtered library list (library tab, track editor list) in step with the search text.
    pub(super) fn refresh_lib_filter(&mut self, core: &Playout) {
        let key = (self.lib_filter.to_lowercase(), core.library_ver);
        if key != self.lib_key {
            let f = key.0.clone();
            self.lib_filtered = core
                .library
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    f.is_empty()
                        || t.artist.to_lowercase().contains(&f)
                        || t.title.to_lowercase().contains(&f)
                        || t.album.to_lowercase().contains(&f)
                })
                .map(|(i, _)| i)
                .collect();
            self.lib_key = key;
        }
    }

    fn tab_library(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("➕ Folder…")).clicked() {
                self.pick_library_folder();
            }
            if ui.add_enabled(!core.scanning, egui::Button::new("⟳")).on_hover_text(tr("Rescan")).clicked() {
                core.start_scan();
            }
            if ui
                .add_enabled(self.perm.edit_playlists, egui::Button::new(tr("Add all")))
                .on_hover_text(tr("Add all shown tracks to the playlist"))
                .clicked()
            {
                let paths = self.lib_filtered.iter().map(|&i| core.library[i].path.clone()).collect();
                let v = self.view_pl;
                core.with_tab(v, |c| c.add_paths(paths));
            }
            if core.scanning {
                ui.spinner();
                ui.label(format!("{}", core.scan_progress));
            } else {
                ui.label(RichText::new(format!("{} tracks", core.library.len())).color(GRAY));
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.lib_filter).desired_width(f32::INFINITY).hint_text(tr("🔍 search artist, title, album")),
        );
        self.refresh_lib_filter(core);

        let mut act: Option<(usize, &'static str)> = None;
        let lib = &core.library;
        let filt = &self.lib_filtered;
        let lsel = &mut self.lib_sel;
        let lanchor = &mut self.lib_anchor;
        let may_edit = self.perm.edit_playlists;
        let may_play = self.perm.playback;
        let may_tag = self.perm.tool(crate::auth::tool::TRACK_EDITOR);
        let mods = ui.input(|i| i.modifiers);
        let lib_w = ui.available_width();
        let mut scroll_to = self.lib_scroll.take();
        if let Some(li) = scroll_to.take() {
            scroll_to = filt.iter().position(|&x| x == li);
        }
        let mut tb = TableBuilder::new(ui)
            .id_salt(("library_table", (lib_w / 24.0) as i32))
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click_and_drag())
            .column(Column::initial((lib_w * 0.38).max(70.0)).at_least(40.0).clip(true))
            .column(Column::remainder().at_least(60.0).clip(true))
            .column(Column::exact(58.0));
        if let Some(r) = scroll_to {
            tb = tb.scroll_to_row(r, Some(egui::Align::Center));
        }
        tb.header(20.0, |mut h| {
            for t in ["Artist", "Title", "Length"] {
                h.col(|ui| {
                    ui.strong(tr(t));
                });
            }
        })
        .body(|body| {
            body.rows(20.0, filt.len(), |mut row| {
                let pos = row.index();
                let li = filt[pos];
                let t = &lib[li];
                row.set_selected(lsel.contains(&li));
                row.col(|ui| {
                    ui.add(egui::Label::new(&t.artist).truncate());
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(&t.title).truncate());
                });
                row.col(|ui| {
                    ui.label(fmt(t.duration));
                });
                let r = row.response();
                if r.clicked() {
                    // click = one, Ctrl+click = add/remove, Shift+click = range (in the list as shown)
                    if mods.shift && lanchor.is_some() {
                        let a = filt.iter().position(|&x| Some(x) == *lanchor).unwrap_or(pos);
                        lsel.clear();
                        lsel.extend(filt[a.min(pos)..=a.max(pos)].iter().copied());
                    } else if mods.command {
                        if !lsel.remove(&li) {
                            lsel.insert(li);
                        }
                        *lanchor = Some(li);
                    } else {
                        lsel.clear();
                        lsel.insert(li);
                        *lanchor = Some(li);
                    }
                }
                if r.secondary_clicked() && !lsel.contains(&li) {
                    lsel.clear();
                    lsel.insert(li);
                    *lanchor = Some(li);
                }
                if r.drag_started() && may_edit {
                    if !lsel.contains(&li) {
                        lsel.clear();
                        lsel.insert(li);
                        *lanchor = Some(li);
                    }
                    // drag the whole selection, in the order it is shown
                    let paths: Vec<PathBuf> = filt.iter().filter(|x| lsel.contains(x)).map(|&x| lib[x].path.clone()).collect();
                    r.dnd_set_drag_payload(plview::DragFiles(paths));
                }
                if r.double_clicked() && may_edit {
                    act = Some((li, "add"));
                }
                r.context_menu(|ui| {
                    let n = lsel.len().max(1);
                    let many = if n > 1 { format!(" ({n})") } else { String::new() };
                    let items: [(String, &str, bool); 5] = [
                        (format!("{}{many}", tr("Add to playlist")), "add", may_edit),
                        (format!("{}{many}", tr("Insert as next")), "next", may_edit),
                        (tr("Play now").to_string(), "now", may_edit && may_play),
                        (tr("✏ Track editor…").to_string(), "tags", may_tag),
                        (format!("{}{many}", tr("☁ Upload to AzuraCast")), "up", true),
                    ];
                    for (l, k, ok) in items {
                        if ui
                            .add_enabled(ok, egui::Button::new(l))
                            .on_disabled_hover_text(tr("Your account isn't allowed to do this"))
                            .clicked()
                        {
                            act = Some((li, k));
                            ui.close_menu();
                        }
                    }
                });
            });
        });

        if let Some((li, a)) = act {
            // the selection (in the order shown), or the clicked track
            let mut paths: Vec<PathBuf> =
                self.lib_filtered.iter().filter(|x| self.lib_sel.contains(x)).map(|&x| core.library[x].path.clone()).collect();
            if paths.is_empty() || !self.lib_sel.contains(&li) {
                paths = vec![core.library[li].path.clone()];
            }
            let edit = self.perm.edit_playlists;
            match a {
                "add" if edit => {
                    let v = self.view_pl;
                    core.with_tab(v, |c| c.add_paths(paths));
                }
                "next" if edit => {
                    let v = self.view_pl;
                    core.with_tab(v, |c| c.insert_next(paths));
                }
                "now" if edit && self.perm.playback => {
                    core.insert_next(vec![core.library[li].path.clone()]);
                    core.next();
                }
                "tags" if self.perm.tool(crate::auth::tool::TRACK_EDITOR) => {
                    let t = core.library[li].clone();
                    self.open_track_editor_for(core, t, Some(li));
                }
                "up" => Self::upload(core, paths),
                _ => *self.notify.lock() = tr("Your account isn't allowed to do this").into(),
            }
        }
    }

    fn tab_carts(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let cols = ((ui.available_width() / 116.0) as usize).clamp(1, 6);
            let pad_width = ((ui.available_width() - (cols.saturating_sub(1)) as f32 * 6.0) / cols as f32 - 8.0).max(86.0);
            let mut trigger = None;
            let may_play = self.perm.jingles;
            let playing = core.carts_playing();
            if !playing.is_empty() {
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(may_play && !playing.is_empty(), egui::Button::new(RichText::new(tr("■ Stop jingles")).strong()))
                    .on_hover_text(tr("Fade out every jingle that is playing (Ctrl+J)"))
                    .clicked()
                {
                    core.stop_carts();
                }
                if !playing.is_empty() {
                    ui.label(RichText::new(format!("{} playing", playing.len())).color(GREEN));
                }
            });
            egui::Grid::new("carts").spacing([6.0, 6.0]).show(ui, |ui| {
                for i in 0..core.cfg.carts.len() {
                    let pad: &mut CartPad = &mut core.cfg.carts[i];
                    let loaded = !pad.path.is_empty();
                    let on = loaded && playing.iter().any(|p| p.as_path() == std::path::Path::new(&pad.path));
                    let label = if loaded { pad.label.clone() } else { format!("Pad {}", i + 1) };
                    let fill = if on {
                        Color32::from_rgb(200, 60, 60)
                    } else if loaded {
                        Color32::from_rgb(40, 80, 130)
                    } else {
                        Color32::from_gray(48)
                    };
                    let b = ui.add_sized([pad_width, 54.0], egui::Button::new(RichText::new(label).strong()).fill(fill).wrap());
                    if b.clicked() && loaded && may_play {
                        trigger = Some(i);
                    }
                    b.context_menu(|ui| {
                        if ui.button(tr("Assign audio file…")).clicked() {
                            self.dlg(move || {
                                rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_file().map(|p| Dlg::Cart(i, p))
                            });
                            ui.close_menu();
                        }
                        ui.label(tr("Label:"));
                        ui.add(egui::TextEdit::singleline(&mut pad.label).desired_width(220.0));
                        if ui.button(tr("Clear pad")).clicked() {
                            *pad = CartPad::default();
                            ui.close_menu();
                        }
                    });
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
            if let Some(i) = trigger {
                core.trigger_cart(i);
            }
            ui.label(
                RichText::new(tr("Click to fire (plays over the main program); click a red, playing pad to stop it. Right-click a pad to assign or relabel."))
                    .small()
                    .color(GRAY),
            );
        });
    }

    fn tab_scheduler(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            if ui.button(tr("➕ Add event")).clicked() {
                core.cfg.events.push(Event::default());
            }
            ui.label(
                RichText::new(tr(
                    "Things that happen at a set time: a new playlist from a template, adverts, a playlist going on air, encoders, volume…",
                ))
                .small()
                .color(GRAY),
            );
        });
        ui.add_space(4.0);
        let mut del = None;
        let mut run_now = None;
        let opts = ActionOpts::of(core);
        let now = Local::now();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if core.cfg.events.is_empty() {
                ui.label(RichText::new(tr("No events yet.")).color(GRAY));
            }
            for i in 0..core.cfg.events.len() {
                let e = &mut core.cfg.events[i];
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut e.enabled, "");
                        ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(170.0));
                        ui.label(tr("at"));
                        ui.add(egui::DragValue::new(&mut e.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                        ui.label(":");
                        ui.add(egui::DragValue::new(&mut e.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                        if ui.button(tr("▶ Run now")).on_hover_text(tr("Do it now, as a test")).clicked() {
                            run_now = Some(i);
                        }
                        if delete_button(ui, ("event", i), tr("this event")) {
                            del = Some(i);
                        }
                    });
                    adsui::days_edit(ui, &mut e.days);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Dates"));
                        date_edit(ui, ("ev_from", i), &mut e.from_date, true);
                        ui.label(tr("to"));
                        date_edit(ui, ("ev_to", i), &mut e.to_date, true);
                        let mut probe = e.clone();
                        probe.hour = now.hour();
                        probe.minute = now.minute();
                        if e.enabled && !probe.due(&now) {
                            ui.label(RichText::new(tr("○ not today")).small().color(AMBER));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(tr("Do")).strong());
                        if action_edit(ui, ("sched", i), &mut e.action, &mut e.path, &mut e.arg, &opts) {
                            let a = e.action;
                            self.dlg(move || action_file_dialog(a).pick_file().map(|p| Dlg::SchedPath(i, p)));
                        }
                    });
                });
            }
        });
        if let Some(i) = run_now {
            let e = core.cfg.events[i].clone();
            core.run_event(&e);
        }
        if let Some(i) = del {
            core.cfg.events.remove(i);
        }
    }

    /// List the sound cards in the background (asking the drivers can take a while).
    fn refresh_devices(&mut self) {
        if self.dev_listing {
            return;
        }
        self.dev_listing = true;
        self.dev_listed = true;
        self.dlg(|| Some(Dlg::Devices(crate::audio::list_outputs(), crate::audio::list_inputs(), crate::audio::default_output_name())));
    }

    fn tab_devices(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if !self.dev_listed {
            self.refresh_devices();
        }
        ui.horizontal(|ui| {
            if ui.button(tr("⟳ Refresh devices")).clicked() {
                self.refresh_devices();
            }
            if self.dev_listing {
                ui.spinner();
            }
            if ui.button(tr("Apply outputs")).clicked() {
                let engine = self.output_engine(core);
                self.audio.apply(&engine, &core.cfg.audio_devices);
                core.say("Audio outputs applied");
            }
        });
        ui.add_space(4.0);
        ui.heading(tr("Outputs"));
        ui.label(tr("Tick every device that should play the on-air mix. The first working device is the primary; the others get a copy (monitor / headphones / second sound card). Each has its own volume."));
        ui.label(tr("Mark the speakers in the studio as Studio monitor: they go quiet while the mic is on, so the mic doesn't pick them up. The stream and the other outputs keep playing."));
        let mut changed = false;
        let mut to_top = None;
        let n = core.cfg.audio_devices.outputs.len();
        for i in 0..n {
            let name = core.cfg.audio_devices.outputs[i].name.clone();
            let present = self.dev_listing || self.dev_outputs.contains(&name);
            let role = self.audio.role_of(&name);
            ui.horizontal(|ui| {
                let o = &mut core.cfg.audio_devices.outputs[i];
                if ui.checkbox(&mut o.enabled, "").changed() {
                    changed = true;
                }
                let mut t = RichText::new(&name);
                if !present {
                    t = t.color(GRAY).italics();
                }
                ui.allocate_ui_with_layout(egui::vec2(260.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(t).truncate());
                });
                if !present {
                    ui.label(RichText::new(tr("(not connected)")).small().color(GRAY));
                }
                if let Some(r) = role {
                    ui.label(RichText::new(r.to_uppercase()).small().strong().color(if r == "primary" {
                        Color32::from_rgb(20, 140, 50)
                    } else {
                        Color32::from_rgb(30, 110, 205)
                    }));
                }
                ui.spacing_mut().slider_width = 110.0;
                if ui.add(egui::Slider::new(&mut o.gain, 0.0..=1.5).show_value(false)).changed() {
                    self.audio.set_gain(&name, o.gain);
                }
                ui.label(format!("{}%", (o.gain * 100.0).round() as i32));
                if ui
                    .checkbox(&mut o.studio, tr("Studio monitor"))
                    .on_hover_text(tr("Muted while the mic is on (the stream keeps playing)"))
                    .changed()
                {
                    changed = true;
                }
                if i > 0 && ui.small_button(tr("⬆ primary")).on_hover_text(tr("Make this the primary output")).clicked() {
                    to_top = Some(i);
                }
            });
        }
        if let Some(i) = to_top {
            let o = core.cfg.audio_devices.outputs.remove(i);
            core.cfg.audio_devices.outputs.insert(0, o);
            changed = true;
        }
        if changed {
            let engine = self.output_engine(core);
            self.audio.apply(&engine, &core.cfg.audio_devices);
        }
        if !self.audio.report.is_empty() {
            ui.add_space(4.0);
            for (d, s) in &self.audio.report {
                ui.label(RichText::new(format!("{d}: {s}")).small().color(GRAY));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.heading(tr("Input (microphone / line in)"));
        let cur = if core.cfg.audio_devices.input.is_empty() { "System default".to_string() } else { core.cfg.audio_devices.input.clone() };
        egui::ComboBox::from_id_salt("input_device").width(340.0).selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut core.cfg.audio_devices.input, String::new(), tr("System default"));
            for n in &self.dev_inputs {
                ui.selectable_value(&mut core.cfg.audio_devices.input, n.clone(), n);
            }
        });
        ui.small(tr("Switch the Mic button off and on again after changing the input."));
        ui.add_space(6.0);
        {
            use crate::config::MicMode;
            ui.label(RichText::new(tr("While the mic is on, the music…")).strong());
            ui.radio_value(&mut core.cfg.mic_mode, MicMode::Duck, tr("is lowered (set how far with the slider next to the Mic volume)"));
            if core.cfg.mic_mode == MicMode::Duck {
                ui.add(
                    egui::Slider::new(&mut core.cfg.mic_duck, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
                        .text(tr("music level")),
                );
            }
            ui.radio_value(&mut core.cfg.mic_mode, MicMode::Mute, tr("is muted (it keeps playing silently)"));
            ui.radio_value(&mut core.cfg.mic_mode, MicMode::Pause, tr("is paused, and carries on when the mic goes off"));
            ui.radio_value(&mut core.cfg.mic_mode, MicMode::Keep, tr("keeps playing at full level"));
            ui.small(tr("This is what the stream hears. Outputs marked Studio monitor are always muted while the mic is on."));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.heading(tr("Preview (track editor)"));
        ui.label(tr("Where ▶ Play / Preview mix in the track editor is heard. It is never sent to the stream, but if it is the same device as your on-air output, it will be heard in the room."));
        let cur =
            if core.cfg.audio_devices.preview.is_empty() { "System default".to_string() } else { core.cfg.audio_devices.preview.clone() };
        egui::ComboBox::from_id_salt("preview_device").width(340.0).selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut core.cfg.audio_devices.preview, String::new(), tr("System default"));
            for n in &self.dev_outputs {
                ui.selectable_value(&mut core.cfg.audio_devices.preview, n.clone(), n);
            }
        });
    }

    fn tab_streaming(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let mut act: Option<(usize, &'static str)> = None;
        let needs_ffmpeg =
            core.cfg.encoders.iter().any(|e| !matches!(e.format, crate::config::StreamFormat::Mp3 | crate::config::StreamFormat::Custom));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("➕ Add encoder")).clicked() {
                core.add_encoder();
            }
            ui.separator();
            // ffmpeg makes AAC / Opus / Vorbis / FLAC streams
            ui.label(RichText::new("ffmpeg").strong());
            let found = self
                .ffmpeg_found
                .get_or_insert_with(|| crate::stream::find_ffmpeg(&core.cfg.ffmpeg_path).map(|p| p.display().to_string()))
                .clone();
            match found {
                Some(p) => {
                    ui.label(RichText::new(format!("✔ {p}")).small().color(GREEN));
                }
                None => {
                    ui.label(
                        RichText::new(if needs_ffmpeg {
                            tr("✖ not found — needed for AAC, Opus, Vorbis and FLAC streams")
                        } else {
                            tr("not found (only needed for formats other than MP3)")
                        })
                        .small()
                        .color(if needs_ffmpeg { RED } else { GRAY }),
                    );
                    ui.hyperlink_to(tr("get ffmpeg"), "https://ffmpeg.org/download.html");
                }
            }
            if ui.small_button(tr("Choose…")).on_hover_text(tr("Pick the ffmpeg program")).clicked() {
                self.dlg(|| rfd::FileDialog::new().pick_file().map(Dlg::FfmpegPath));
            }
            if ui.small_button("⟳").on_hover_text(tr("Look again")).clicked() {
                self.ffmpeg_found = None;
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            for i in 0..core.cfg.encoders.len() {
                let (txt, col, running, bytes) = match &core.handles[i] {
                    Some(h) => {
                        let st = h.status.lock();
                        let (t, c) = match &st.state {
                            EncState::Idle => ("stopped".to_string(), GRAY),
                            EncState::Connecting => ("connecting…".to_string(), AMBER),
                            EncState::Live => ("LIVE".to_string(), GREEN),
                            EncState::Error(e) => (format!("error: {e}"), RED),
                        };
                        (t, c, h.is_running(), st.bytes)
                    }
                    None => ("stopped".to_string(), GRAY, false, 0),
                };
                let enc = &mut core.cfg.encoders[i];
                egui::CollapsingHeader::new(RichText::new(format!("● #{i}  {} — {txt}", enc.name)).color(col).strong())
                    .id_salt(("enc", i))
                    .default_open(i == 0)
                    .show(ui, |ui| {
                        egui::Grid::new(("encgrid", i)).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                            ui.label(tr("Name"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.name));
                            ui.label(tr("Server type"));
                            egui::ComboBox::from_id_salt(("kind", i)).selected_text(format!("{:?}", enc.kind)).show_ui(ui, |ui| {
                                ui.selectable_value(&mut enc.kind, ServerKind::Icecast, tr("Icecast"));
                                ui.selectable_value(&mut enc.kind, ServerKind::Shoutcast, tr("Shoutcast (v2 DJ)"));
                            });
                            ui.end_row();
                            ui.label(tr("Host"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.host));
                            ui.label(tr("Port"));
                            ui.add(egui::DragValue::new(&mut enc.port).range(1..=65535));
                            ui.end_row();
                            ui.label(tr("Format"));
                            {
                                use crate::config::StreamFormat;
                                let before = enc.format;
                                egui::ComboBox::from_id_salt(("fmt", i)).width(230.0).selected_text(tr(enc.format.label())).show_ui(ui, |ui| {
                                    for f in StreamFormat::ALL {
                                        ui.selectable_value(&mut enc.format, f, tr(f.label()));
                                    }
                                });
                                if enc.format != before {
                                    // follow the format in the mount point's ending (/radio.mp3 → /radio.opus)
                                    let (old, new) = (before.extension(), enc.format.extension());
                                    if !old.is_empty() && !new.is_empty() && enc.mount.ends_with(old) {
                                        enc.mount = format!("{}{new}", &enc.mount[..enc.mount.len() - old.len()]);
                                    }
                                }
                            }
                            ui.label(RichText::new(match (enc.kind, enc.format) {
                                (ServerKind::Shoutcast, f) if !matches!(f, crate::config::StreamFormat::Mp3 | crate::config::StreamFormat::Aac | crate::config::StreamFormat::Custom) => tr("⚠ Shoutcast only takes MP3 or AAC"),
                                (_, crate::config::StreamFormat::Opus | crate::config::StreamFormat::Vorbis | crate::config::StreamFormat::Flac) => tr("Ogg: some servers show titles only for MP3/AAC"),
                                _ => "",
                            }).small().color(AMBER));
                            ui.label("");
                            ui.end_row();
                            if enc.format == crate::config::StreamFormat::Custom {
                                ui.label(tr("Command"));
                                ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.command).hint_text("ffmpeg -f s16le -ar 44100 -ac 2 -i - … -"));
                                ui.label(tr("Content type"));
                                ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.content_type).hint_text("audio/aac"));
                                ui.end_row();
                                ui.label("");
                                ui.label(
                                    RichText::new(tr("The program gets 16-bit 44.1 kHz stereo PCM on its input and must write the stream to its output. {bitrate} = the bitrate in kbps."))
                                        .small()
                                        .color(GRAY),
                                );
                                ui.end_row();
                            }
                            ui.label(tr("Mount"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.mount));
                            ui.label(tr("Bitrate (kbps)"));
                            egui::ComboBox::from_id_salt(("br", i)).selected_text(enc.bitrate_kbps.to_string()).show_ui(ui, |ui| {
                                let low: &[u32] = if enc.format == crate::config::StreamFormat::Mp3 { &[] } else { &[24, 32, 48] };
                                for &b in low.iter().chain(&[64, 96, 128, 160, 192, 256, 320]) {
                                    ui.selectable_value(&mut enc.bitrate_kbps, b, b.to_string());
                                }
                            });
                            ui.end_row();
                            ui.label(tr("Username"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.username));
                            ui.label(tr("Password"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.password).password(true));
                            ui.end_row();
                            ui.label(tr("Stream name"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.stream_name));
                            ui.label(tr("Genre"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.genre));
                            ui.end_row();
                            ui.label(tr("Description"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.description));
                            ui.label(tr("Website"));
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.url));
                            ui.end_row();
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut enc.public, tr("List publicly"));
                            ui.checkbox(&mut enc.send_metadata, tr("Send track titles"));
                            ui.checkbox(&mut enc.start_with_app, tr("Connect on startup"));
                        });
                        ui.horizontal(|ui| {
                            if running {
                                if ui.button(RichText::new(tr("⏹ Disconnect")).color(RED)).clicked() {
                                    act = Some((i, "stop"));
                                }
                            } else if ui.button(RichText::new(tr("▶ Connect")).color(GREEN)).clicked() {
                                act = Some((i, "start"));
                            }
                            if ui
                                .button("AzuraCast DJ preset")
                                .on_hover_text(tr("Icecast, mount “/”, user/pass = AzuraCast Streamer account, port = station DJ port"))
                                .clicked()
                            {
                                act = Some((i, "preset"));
                            }
                            if ui.button(tr("🗑 Remove")).clicked() {
                                act = Some((i, "remove"));
                            }
                            if running {
                                ui.label(RichText::new(format!("{:.1} MB sent", bytes as f64 / 1_048_576.0)).color(GRAY));
                            }
                        });
                    });
            }
        });
        if let Some((i, a)) = act {
            match a {
                "start" => core.start_encoder(i),
                "stop" => core.stop_encoder(i),
                "remove" => core.remove_encoder(i),
                _ => {
                    let host = host_of(&core.cfg.azuracast.base_url);
                    let e = &mut core.cfg.encoders[i];
                    e.kind = ServerKind::Icecast;
                    e.mount = "/".into();
                    if !host.is_empty() {
                        e.host = host;
                    }
                    if e.port == 8000 {
                        e.port = 8005;
                    }
                    if e.username == "source" {
                        e.username = "dj".into();
                    }
                }
            }
        }
    }

    fn tab_azuracast(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let np = core.azura.lock().np.clone();
        let err = core.azura.lock().error.clone();
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.strong(tr("Connection"));
            let a = &mut core.cfg.azuracast;
            egui::Grid::new("az").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label(tr("Enable polling"));
                ui.checkbox(&mut a.enabled, "");
                ui.end_row();
                ui.label(tr("Base URL"));
                ui.add(egui::TextEdit::singleline(&mut a.base_url).hint_text(tr("https://radio.example.com")).desired_width(260.0));
                ui.end_row();
                ui.label(tr("Station"));
                ui.add(egui::TextEdit::singleline(&mut a.station).hint_text(tr("short name or ID")).desired_width(260.0));
                ui.end_row();
                ui.label(tr("API key"));
                ui.add(
                    egui::TextEdit::singleline(&mut a.api_key)
                        .password(true)
                        .hint_text(tr("id:secret (for skip/upload)"))
                        .desired_width(260.0),
                );
                ui.end_row();
                ui.label(tr("Upload folder"));
                ui.add(egui::TextEdit::singleline(&mut a.upload_dir).hint_text(tr("optional")).desired_width(260.0));
                ui.end_row();
                ui.label(tr("Poll every (s)"));
                ui.add(egui::DragValue::new(&mut a.poll_secs).range(2..=60));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button(tr("⏭ Skip AutoDJ song")).clicked() {
                    let (c, n) = (core.cfg.azuracast.clone(), core.notify.clone());
                    thread::spawn(move || {
                        *n.lock() = match Client::new(&c).skip() {
                            Ok(_) => "AzuraCast: skipped current AutoDJ song".into(),
                            Err(e) => format!("AzuraCast skip failed: {e}"),
                        };
                    });
                }
                if ui.button(tr("Use for encoder #0")).on_hover_text(tr("Copies host + server type into the first encoder")).clicked() {
                    let host = host_of(&core.cfg.azuracast.base_url);
                    let kind = np.as_ref().map(|n| n.station.frontend.clone()).unwrap_or_default();
                    if let Some(e) = core.cfg.encoders.first_mut() {
                        e.host = host;
                        e.mount = "/".into();
                        e.kind = if kind.starts_with("shoutcast") { ServerKind::Shoutcast } else { ServerKind::Icecast };
                    }
                }
            });
            if let Some(e) = &err {
                ui.colored_label(RED, e);
            }
            ui.label(
                RichText::new(tr(
                    "Tip: create a Streamer/DJ account in AzuraCast, then use the “AzuraCast DJ preset” on the Streaming tab.",
                ))
                .small()
                .color(GRAY),
            );

            let ui = &mut cols[1];
            ui.strong(tr("On air"));
            match &np {
                Some(n) => {
                    ui.label(RichText::new(&n.station.name).heading());
                    ui.label(if n.is_online {
                        RichText::new(tr("● online")).color(GREEN)
                    } else {
                        RichText::new(tr("● offline")).color(RED)
                    });
                    if n.live.is_live {
                        ui.label(RichText::new(format!("🎤 Live DJ: {}", n.live.streamer_name)).color(AMBER));
                    }
                    ui.label(format!("{} — {}", n.now_playing.song.artist, n.now_playing.song.title));
                    ui.label(format!("Listeners: {} (unique {}, total {})", n.listeners.current, n.listeners.unique, n.listeners.total));
                    if let Some(p) = &n.playing_next {
                        ui.label(RichText::new(format!("Next: {} — {}", p.song.artist, p.song.title)).color(GRAY));
                    }
                    ui.separator();
                    ui.strong(tr("Recent"));
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for h in n.song_history.iter().take(15) {
                            ui.label(format!("{} — {}", h.song.artist, h.song.title));
                        }
                    });
                }
                None => {
                    ui.label(RichText::new(tr("No data yet — enable polling and fill in URL + station.")).color(GRAY));
                }
            }
        });
    }

    fn tab_history(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.label(RichText::new(tr("Also appended to history.csv in the RadioMaster data folder.")).small().color(GRAY));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for h in core.history.iter().rev() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&h.time).monospace().color(GRAY));
                    if h.kind != "song" {
                        ui.label(RichText::new(tr(crate::report::kind_name(h.kind))).small().color(AMBER));
                    }
                    ui.label(&h.text);
                });
            }
        });
    }

    fn tab_settings(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong(tr("Library folders"));
            let mut del = None;
            for (i, f) in core.cfg.library_folders.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✖").clicked() {
                        del = Some(i);
                    }
                    ui.label(f);
                });
            }
            if let Some(i) = del {
                core.cfg.library_folders.remove(i);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tr("Adverts and jingles every N songs are set up in"));
                if ui.link("Tools → Advertisement scheduler").clicked() {
                    self.open_tool(ToolWin::Ads);
                }
            });
            ui.separator();
            ui.strong(tr("Export now-playing metadata"));
            egui::Grid::new("exp").num_columns(2).show(ui, |ui| {
                ui.label(tr("Text file"));
                ui.add(egui::TextEdit::singleline(&mut core.cfg.export.text_file).desired_width(380.0));
                ui.end_row();
                ui.label(tr("XML file"));
                ui.add(egui::TextEdit::singleline(&mut core.cfg.export.xml_file).desired_width(380.0));
                ui.end_row();
                ui.label(tr("HTTP GET URL"));
                ui.add(
                    egui::TextEdit::singleline(&mut core.cfg.export.http_url)
                        .desired_width(380.0)
                        .hint_text("https://site/np?artist={artist}&title={title}"),
                );
                ui.end_row();
            });
            ui.separator();
            ui.strong(tr("Remote control API  (restart required)"));
            ui.horizontal(|ui| {
                ui.checkbox(&mut core.cfg.remote.enabled, tr("Enable"));
                ui.label(tr("port"));
                ui.add(egui::DragValue::new(&mut core.cfg.remote.port).range(1..=65535));
                ui.label(tr("password"));
                ui.add(egui::TextEdit::singleline(&mut core.cfg.remote.password).password(true).desired_width(120.0));
            });
            ui.label(
                RichText::new(format!(
                    "e.g. http://localhost:{}/api/status  ·  /api/next  ·  /api/volume?v=0.8  (add &pw=… if set)",
                    core.cfg.remote.port
                ))
                .small()
                .color(GRAY),
            );
        });
    }

    fn tab_generator(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let cat_names: Vec<String> = core.cfg.categories.iter().map(|c| c.name.clone()).collect();
        let tpl_names: Vec<String> = core.cfg.templates.iter().map(|t| t.name.clone()).collect();
        let rules: Vec<(u64, String, bool)> = core.cfg.ad_rules.iter().map(|r| (r.id, r.name.clone(), r.adverts)).collect();
        let first_jingle = rules.iter().find(|r| !r.2).map(|r| format!("rule:{}", r.0));
        let mut gen: Option<(String, bool)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong(tr("Auto-playlist"));
            ui.horizontal(|ui| {
                ui.checkbox(&mut core.cfg.auto_enabled, tr("Keep the playlist filled automatically"));
                ui.label(tr("using"));
                name_combo(ui, "autotpl", &mut core.cfg.auto_template, &tpl_names);
                ui.label(tr("when under"));
                ui.add(egui::DragValue::new(&mut core.cfg.auto_min_mins).range(1..=600).suffix(" min left"));
            });
            ui.separator();
            ui.strong(tr("Categories  (what each pool of tracks is made from)"));
            let mut del = None;
            for i in 0..core.cfg.categories.len() {
                let c = &mut core.cfg.categories[i];
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(100.0));
                    ui.label(tr("folder"));
                    ui.add(egui::TextEdit::singleline(&mut c.folder).desired_width(260.0).hint_text(tr("any")));
                    if ui.button("…").clicked() {
                        self.dlg(move || rfd::FileDialog::new().pick_folder().map(|p| Dlg::CatFolder(i, p)));
                    }
                    ui.label(tr("genre"));
                    ui.add(egui::TextEdit::singleline(&mut c.genre).desired_width(90.0).hint_text(tr("any")));
                    ui.label(tr("length"));
                    ui.add(egui::DragValue::new(&mut c.min_secs).range(0.0..=3600.0).suffix("s"));
                    ui.label("–");
                    ui.add(egui::DragValue::new(&mut c.max_secs).range(0.0..=7200.0).suffix("s"));
                    if delete_button(ui, ("category", i), tr("this category")) {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                core.cfg.categories.remove(i);
            }
            if ui.button(tr("➕ Add category")).clicked() {
                core.cfg.categories.push(Category { name: format!("Category {}", core.cfg.categories.len() + 1), ..Default::default() });
            }
            ui.separator();
            ui.strong(tr("Templates"));
            ui.horizontal(|ui| {
                for (i, n) in tpl_names.iter().enumerate() {
                    ui.selectable_value(&mut self.tpl_sel, i, n);
                }
                if ui.button(tr("➕ New")).clicked() {
                    core.cfg.templates.push(Template { name: format!("Template {}", core.cfg.templates.len() + 1), ..Default::default() });
                    self.tpl_sel = core.cfg.templates.len() - 1;
                }
            });
            if self.tpl_sel >= core.cfg.templates.len() {
                self.tpl_sel = core.cfg.templates.len().saturating_sub(1);
            }
            let mut del_tpl = false;
            if let Some(t) = core.cfg.templates.get_mut(self.tpl_sel) {
                ui.horizontal(|ui| {
                    ui.label(tr("Name"));
                    ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(140.0));
                    ui.label(tr("Length"));
                    ui.add(egui::DragValue::new(&mut t.minutes).range(5..=1440).suffix(" min"));
                    ui.label(tr("Artist separation"));
                    ui.add(egui::DragValue::new(&mut t.artist_gap).range(0..=50).suffix(" tracks"));
                });
                ui.label(RichText::new(tr("Pattern (repeats until the length is reached):")).color(GRAY));
                let mut pdel = None;
                for (k, p) in t.pattern.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}.", k + 1));
                        ui.add(egui::DragValue::new(&mut p.count).range(1..=20).suffix(" ×"));
                        source_combo(ui, ("pat", k), &mut p.category, &cat_names, &rules);
                        if ui.small_button("✖").clicked() {
                            pdel = Some(k);
                        }
                    });
                }
                if let Some(k) = pdel {
                    t.pattern.remove(k);
                }
                if ui.button(tr("➕ Add pattern step")).clicked() {
                    t.pattern.push(PatternItem::default());
                }
                ui.label(
                    RichText::new(tr("Jingles, adverts and other inserts by time (counted along the generated running order):"))
                        .color(GRAY),
                );
                let mut mdel = None;
                for (k, m) in t.markers.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(tr("every"));
                        ui.add(egui::DragValue::new(&mut m.every_mins).range(1..=600).suffix(" min"));
                        ui.label(tr("insert"));
                        ui.add(egui::DragValue::new(&mut m.count).range(1..=10).suffix(" ×"));
                        ui.label(tr("from"));
                        source_combo(ui, ("mk", k), &mut m.category, &cat_names, &rules);
                        if ui.small_button("✖").clicked() {
                            mdel = Some(k);
                        }
                    });
                }
                if let Some(k) = mdel {
                    t.markers.remove(k);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(first_jingle.is_some(), egui::Button::new(tr("➕ Add jingles")))
                        .on_hover_text(tr("Uses the files of a jingle rule (Advertisement scheduler) every 15 minutes"))
                        .on_disabled_hover_text(tr("Make a jingle rule in the Advertisement scheduler first, or add a Jingles category"))
                        .clicked()
                    {
                        t.markers.push(Marker { category: first_jingle.clone().unwrap_or_default(), every_mins: 15, count: 1 });
                    }
                    if ui.button(tr("➕ Add insert")).clicked() {
                        t.markers.push(Marker::default());
                    }
                    ui.checkbox(&mut t.hour_markers, tr("Hour marker at the top of every hour"));
                });
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(tr("▶ Generate (replace playlist)")).color(GREEN)).clicked() {
                        gen = Some((t.name.clone(), false));
                    }
                    if ui.button(tr("Append to playlist")).clicked() {
                        gen = Some((t.name.clone(), true));
                    }
                    if delete_button(ui, ("template", self.tpl_sel), tr("this template")) {
                        del_tpl = true;
                    }
                });
            }
            if del_tpl {
                core.cfg.templates.remove(self.tpl_sel);
            }
        });
        if let Some((n, append)) = gen {
            let c = core.generate_playlist(&n, append);
            if c > 0 {
                core.say(format!("Generated {c} tracks from '{n}'"));
            }
        }
    }

    fn tab_plugins(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.label(tr("Plugins are small programs, in any language, that RadioMaster runs when something happens on air — a song starts, the hour changes, an encoder drops. They can also send station commands back."));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("📂 Open plugins folder")).clicked() {
                match crate::plugins::write_example() {
                    Ok(dir) => crate::plugins::open_folder(&dir),
                    Err(e) => core.say(format!("Plugins folder: {e}")),
                }
            }
            if ui.button(tr("⟳ Reload plugins")).clicked() {
                let off = core.cfg.plugins_off.clone();
                core.plugins.reload(&off);
                core.say(format!("{} plugin(s) loaded", core.plugins.defs.len()));
            }
            if ui.button(tr("How to write one")).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab("https://github.com/epell-development/RadioMaster/blob/main/PLUGINS.md"));
            }
        });
        ui.label(RichText::new(format!("Folder: {}", crate::plugins::folder().display())).small().color(GRAY));
        ui.separator();
        if core.plugins.defs.is_empty() {
            ui.label(RichText::new(tr("No plugins yet. \"Open plugins folder\" adds a working example to start from.")).color(GRAY));
        }
        let mut toggle: Option<(String, bool)> = None;
        let mut test: Option<usize> = None;
        for (i, d) in core.plugins.defs.iter().enumerate() {
            let id = crate::plugins::Plugins::id_of(d);
            let mut on = !core.cfg.plugins_off.contains(&id);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, RichText::new(&d.name).strong()).changed() {
                        toggle = Some((id.clone(), on));
                    }
                    let (txt, col) = match (&d.error, d.is_service(), core.plugins.running(d)) {
                        (Some(_), _, _) => ("problem", RED),
                        (None, true, true) => ("running", GREEN),
                        (None, true, false) => ("service · not running", GRAY),
                        (None, false, _) => ("runs on events", GRAY),
                    };
                    ui.label(RichText::new(txt).small().color(col));
                    if ui.small_button(tr("Send a test event")).clicked() {
                        test = Some(i);
                    }
                });
                if !d.description.is_empty() {
                    ui.label(&d.description);
                }
                ui.label(
                    RichText::new(format!("{} {}  ·  events: {}", d.command, d.args.join(" "), d.events.join(", ")))
                        .small()
                        .monospace()
                        .color(GRAY),
                );
                if let Some(e) = &d.error {
                    ui.colored_label(RED, e);
                }
            });
        }
        if let Some(i) = test {
            let ev = core.plugins.defs[i].events.first().cloned().unwrap_or_else(|| "track_started".into());
            let ev = if ev == "*" { "track_started".to_string() } else { ev };
            core.plugin_event(
                &ev,
                serde_json::json!({ "title": "Test song", "artist": "RadioMaster", "text": "RadioMaster - Test song", "test": true }),
            );
            core.say(format!("Sent a test \"{ev}\" event — see the Log tab for what the plugin printed"));
        }
        if let Some((id, on)) = toggle {
            core.cfg.plugins_off.retain(|x| *x != id);
            if !on {
                core.cfg.plugins_off.push(id);
            }
            let off = core.cfg.plugins_off.clone();
            core.plugins.reload(&off);
        }
        ui.separator();
        ui.strong(tr("Events"));
        egui::Grid::new("plugin_events").num_columns(2).spacing([14.0, 2.0]).show(ui, |ui| {
            for (e, what) in crate::plugins::EVENTS {
                ui.label(RichText::new(e).monospace());
                ui.label(what);
                ui.end_row();
            }
        });
        ui.label(
            RichText::new(tr("A plugin controls RadioMaster by printing lines such as  RM: VOLUME 60  or  RM: PLAYLIST Evening."))
                .small()
                .color(GRAY),
        );
        ui.label(RichText::new(tr("Plugins run with your account's rights — only add ones you trust.")).small().color(AMBER));
    }

    fn tab_dsp(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ---- equalizer: sliders, exact values under them, presets
            let mut save_preset = false;
            let mut delete_preset: Option<usize> = None;
            {
                let custom = core.cfg.eq_presets.clone();
                let d = &mut core.cfg.dsp;
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut d.eq_enabled, tr("12-band equalizer"));
                    egui::ComboBox::from_id_salt("eq_preset").selected_text("Presets…").width(150.0).show_ui(ui, |ui| {
                        for (name, g) in EQ_PRESETS {
                            if ui.selectable_label(false, *name).clicked() {
                                d.eq_gains = *g;
                                d.eq_enabled = true;
                            }
                        }
                        if !custom.is_empty() {
                            ui.separator();
                            for (k, (name, g)) in custom.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui.selectable_label(false, name).clicked() {
                                        d.eq_gains = *g;
                                        d.eq_enabled = true;
                                    }
                                    if ui.small_button("🗑").on_hover_text(tr("Delete this preset")).clicked() {
                                        delete_preset = Some(k);
                                    }
                                });
                            }
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.eq_name).hint_text(tr("preset name")).desired_width(110.0));
                    save_preset = ui.add_enabled(!self.eq_name.trim().is_empty(), egui::Button::new(tr("Save preset"))).clicked();
                    if ui.button(tr("Flat")).clicked() {
                        d.eq_gains = [0.0; 12];
                    }
                });
                ui.horizontal(|ui| {
                    for i in 0..12 {
                        ui.vertical(|ui| {
                            ui.set_width(46.0);
                            ui.add(egui::Slider::new(&mut d.eq_gains[i], -12.0..=12.0).vertical().show_value(false));
                            // exact value in dB: drag it, or double-click to type
                            ui.add_sized(
                                [46.0, 20.0],
                                egui::DragValue::new(&mut d.eq_gains[i]).range(-12.0..=12.0).speed(0.05).fixed_decimals(1),
                            )
                            .on_hover_text(tr("Drag, or double-click to type an exact value"));
                            let f = EQ_FREQS[i];
                            ui.label(RichText::new(if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") }).small());
                        });
                    }
                });
            }
            if save_preset {
                let name = self.eq_name.trim().to_string();
                let g = core.cfg.dsp.eq_gains;
                core.cfg.eq_presets.retain(|(n, _)| *n != name);
                core.cfg.eq_presets.push((name.clone(), g));
                core.say(format!("EQ preset \"{name}\" saved"));
                self.eq_name.clear();
            }
            if let Some(k) = delete_preset {
                core.cfg.eq_presets.remove(k);
            }
            let d = &mut core.cfg.dsp;
            ui.separator();
            ui.checkbox(&mut d.comp_enabled, tr("Compressor"));
            egui::Grid::new("comp").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label(tr("Threshold"));
                ui.add(egui::Slider::new(&mut d.comp_threshold_db, -60.0..=0.0).suffix(" dB"));
                ui.label(tr("Ratio"));
                ui.add(egui::Slider::new(&mut d.comp_ratio, 1.0..=20.0).suffix(" :1"));
                ui.end_row();
                ui.label(tr("Attack"));
                ui.add(egui::Slider::new(&mut d.comp_attack_ms, 1.0..=200.0).suffix(" ms"));
                ui.label(tr("Release"));
                ui.add(egui::Slider::new(&mut d.comp_release_ms, 20.0..=2000.0).suffix(" ms"));
                ui.end_row();
                ui.label(tr("Make-up gain"));
                ui.add(egui::Slider::new(&mut d.comp_makeup_db, 0.0..=18.0).suffix(" dB"));
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.agc_enabled, tr("Automatic volume leveling"));
                ui.label(tr("target"));
                ui.add(egui::Slider::new(&mut d.agc_target_db, -30.0..=-6.0).suffix(" dB"));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.silence_trim, tr("Trim silence"));
                ui.label(tr("below"));
                ui.add(egui::Slider::new(&mut d.silence_thresh_db, -80.0..=-20.0).suffix(" dB"));
                ui.label(tr("cut tail after"));
                ui.add(egui::Slider::new(&mut d.silence_max_secs, 0.2..=5.0).suffix(" s"));
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tr("Teaser mode — play only the first"));
                ui.add(egui::DragValue::new(&mut core.cfg.teaser_secs).range(0.0..=120.0).suffix(" s"));
                ui.label(RichText::new(tr("of each track (0 = off)")).color(GRAY));
            });
        });
    }

    fn update_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        use crate::updater::Phase;
        if !self.show_update {
            return;
        }
        let (avail, phase) = {
            let u = self.updates.lock();
            (u.available.clone(), u.phase.clone())
        };
        let installed = crate::updater::installed_copy();
        let mut open = true;
        let mut start_download = false;
        let mut install: Option<PathBuf> = None;
        Self::window_for(ctx, "Update RadioMaster", [520.0, 420.0]).id(egui::Id::new("update_window")).collapsible(false).open(&mut open).show(ctx, |ui| {
            ui.label(format!("You have version {}.", crate::updater::CURRENT));
            match (&avail, &phase) {
                (_, Phase::Checking) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Checking GitHub for a newer version…"));
                    });
                }
                (None, Phase::UpToDate) => {
                    ui.label(RichText::new(tr("✔ RadioMaster is up to date.")).color(GREEN).strong());
                }
                (None, Phase::Failed(e)) => {
                    ui.colored_label(RED, format!("Couldn't check for updates: {e}"));
                }
                (None, _) => {
                    ui.label(tr("No update information yet."));
                }
                (Some(a), phase) => {
                    ui.label(RichText::new(format!("Version {} is available — {}", a.version, a.title)).strong().size(16.0));
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        ui.label(if a.notes.trim().is_empty() { "No release notes." } else { a.notes.trim() });
                    });
                    ui.separator();
                    match phase {
                        Phase::Downloading { done, total } => {
                            let f = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                            ui.add(egui::ProgressBar::new(f).text(format!("Downloading… {:.1} / {:.1} MB", *done as f32 / 1e6, *total as f32 / 1e6)));
                        }
                        Phase::Ready(p) => {
                            ui.label(tr("Downloaded. RadioMaster will close, update and start again."));
                            if ui.button(RichText::new(tr("Install now")).strong()).clicked() {
                                install = Some(p.clone());
                            }
                        }
                        other => {
                            if let Phase::Failed(e) = other {
                                ui.colored_label(RED, e);
                            }
                            ui.horizontal(|ui| {
                                if installed && a.installer.is_some() {
                                    if ui.button(RichText::new(tr("⬇ Download and install")).strong()).clicked() {
                                        start_download = true;
                                    }
                                } else if ui.button(RichText::new(tr("⬇ Open the download page")).strong()).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(a.page.clone()));
                                }
                                if ui.button(tr("Release page")).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(a.page.clone()));
                                }
                            });
                            if !installed {
                                ui.label(RichText::new(tr("This copy wasn't installed with the setup program, so download the new zip (or the installer) from the page.")).small().color(GRAY));
                            }
                        }
                    }
                    if core.state == PlayState::Playing {
                        ui.label(RichText::new(tr("Playback stops while the update installs — best done between shows.")).small().color(AMBER));
                    }
                }
            }
        });
        if start_download {
            crate::updater::download(self.updates.clone(), ctx.clone());
        }
        if let Some(p) = install {
            match crate::updater::run_installer(&p) {
                Ok(()) => {
                    core.say("Installing the update…");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(e) => self.updates.lock().phase = crate::updater::Phase::Failed(format!("Couldn't start the installer: {e}")),
            }
        }
        self.show_update = open;
    }

    fn tag_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let (mut save, mut close) = (false, false);
        if let Some(t) = &mut self.tag_edit {
            Self::window_for(ctx, "Edit tags", [460.0, 260.0]).collapsible(false).show(ctx, |ui| {
                ui.label(RichText::new(t.path.display().to_string()).small().color(GRAY));
                egui::Grid::new("tags").num_columns(2).show(ui, |ui| {
                    ui.label(tr("Artist"));
                    ui.add(egui::TextEdit::singleline(&mut t.artist).desired_width(220.0));
                    ui.end_row();
                    ui.label(tr("Title"));
                    ui.add(egui::TextEdit::singleline(&mut t.title).desired_width(220.0));
                    ui.end_row();
                    ui.label(tr("Album"));
                    ui.add(egui::TextEdit::singleline(&mut t.album).desired_width(220.0));
                    ui.end_row();
                    ui.label(tr("Genre"));
                    ui.add(egui::TextEdit::singleline(&mut t.genre).desired_width(220.0));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    save = ui.button(tr("Save")).clicked();
                    close = ui.button(tr("Cancel")).clicked();
                });
            });
        }
        if save {
            if let Some(t) = self.tag_edit.take() {
                core.update_tags(t);
            }
        } else if close {
            self.tag_edit = None;
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // redraw ~10×/s while audio is moving (VU meters, elapsed time), ~3×/s when idle (the clock)
        let busy = self.mic_on || self.core.try_lock().is_none_or(|c| c.state == PlayState::Playing);
        ctx.request_repaint_after(Duration::from_millis(if busy { 100 } else { 330 }));
        self.browsers.set_main(frame);
        self.browsers.start_frame();
        // If an embedded web view kept keyboard focus, take it back as soon as the user clicks our UI.
        if ctx.input(|i| i.pointer.any_pressed()) && !ctx.input(|i| i.focused) {
            self.browsers.focus_parent("");
        }
        if self.logged_in.is_none() {
            self.browsers.block("");
            self.browsers.finish();
            self.authentication(ctx);
            self.connect_window(ctx);
            if self.srv.ask.is_some() {
                let arc = self.core.clone();
                let mut core = arc.lock();
                if self.srv.from_login {
                    self.remote_sign_in(&mut core);
                } else {
                    self.save_session_window(ctx, &mut core);
                }
            }
            return;
        }
        let t0 = std::time::Instant::now();
        self.perm = match &self.logged_in {
            Some(u) if u.admin => crate::auth::Perms::default(),
            Some(u) => u.perms,
            None => crate::auth::Perms {
                playback: false,
                jingles: false,
                edit_playlists: false,
                tools: false,
                streaming: false,
                azuracast: false,
                settings: false,
                denied: u32::MAX,
                v: 1,
            },
        };
        let arc = self.core.clone();
        let mut core = arc.lock();
        let lock_ms = t0.elapsed().as_secs_f32() * 1000.0;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::P)) {
            self.perf = if self.perf.is_some() { None } else { Some(Perf::default()) };
        }
        self.handle_dialogs(&mut core);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() && self.perm.edit_playlists {
            self.add_files_later(self.view_pl, None, dropped);
        }
        if !self.show_settings {
            self.shortcuts(ctx, &mut core);
        }
        self.menu(ctx, &mut core);
        self.status_bar(ctx, &core);
        self.player(ctx, &mut core);
        // automatic hour lines: kept where each new hour starts (not while something is being dragged)
        if self.hour_check.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) && !ctx.input(|i| i.pointer.any_down()) {
            self.hour_check = Some(std::time::Instant::now());
            let mut tabs = vec![core.playing_tab];
            if self.view_pl < core.tabs.len() && self.view_pl != core.playing_tab {
                tabs.push(self.view_pl);
            }
            for t in tabs {
                let on_air = t == core.playing_tab;
                let lines = core.cfg.hour_lines;
                let map = core.with_tab(t, |c| if lines { c.refresh_hour_info(on_air) } else { c.clear_hour_info() });
                if let (Some(m), true) = (map, t == self.view_pl) {
                    self.sel = self.sel.iter().filter_map(|&i| m.get(i).copied().flatten()).collect();
                    self.anchor = self.anchor.and_then(|a| m.get(a).copied().flatten());
                }
            }
        }
        self.right_workspace(ctx, &mut core);
        self.left_workspace(ctx, &mut core);
        self.playlist(ctx, &mut core);
        self.aux_windows(ctx, &mut core);
        let duck = core.cfg.mic_duck_level();
        core.engine.set_mic(self.mic_on, core.cfg.mic_gain, duck);
        if self.srv.connected() {
            // this machine's sound cards play the local engine: its studio monitors go quiet with the mic too
            self.output_engine(&core).set_mic(self.mic_on, core.cfg.mic_gain, duck);
        }
        self.tag_window(ctx, &mut core);
        self.profiles_window(ctx, &mut core);
        self.server_windows(ctx, &mut core);
        self.users_window(ctx);
        self.settings_window(ctx, &mut core);
        let v = self.view_pl;
        core.with_tab(v, |c| {
            self.tool_windows(ctx, c);
            self.item_dialog_window(ctx, c);
            self.insert_from_playlist_window(ctx, c);
            self.voice_track_window(ctx, c);
            self.find_window(ctx, c);
        });
        self.repeat_window(ctx, &mut core);
        self.keys_window(ctx);
        self.az_take_downloads(&mut core);
        if core.cfg.check_updates && self.update_next.is_none_or(|t| std::time::Instant::now() >= t) {
            // at start-up, then every 6 hours for stations that run for weeks
            self.update_next = Some(std::time::Instant::now() + Duration::from_secs(6 * 3600));
            crate::updater::check(self.updates.clone(), ctx.clone(), false);
        }
        self.update_window(ctx, &mut core);
        self.apply_tab_ops(&mut core);
        // A native web view paints above everything egui draws, so hide the main window's views
        // whenever an egui window, menu, combo list or popup overlaps them.
        let force = self.show_settings
            || self.show_users
            || self.show_profiles
            || self.show_about
            || self.show_update
            || self.tag_edit.is_some()
            || self.srv.window_open();
        self.guard_web_views(ctx, "", force);
        self.browsers.finish();
        // save the layout when it changed, at most every 2 seconds (never while a window is being dragged)
        if self.layout_checked.elapsed() >= Duration::from_secs(2) && !ctx.input(|i| i.pointer.any_down()) {
            self.layout_checked = std::time::Instant::now();
            if self.notes_dirty {
                self.notes.save();
                self.notes_dirty = false;
            }
            let serialized = serde_json::to_string(&self.workzones).unwrap_or_default();
            if serialized != self.saved_layout && self.workzones.save().is_ok() {
                self.saved_layout = serialized;
            }
        }
        drop(core);
        let frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
        if let Some(p) = self.perf.as_mut() {
            p.frame_ms = p.frame_ms * 0.9 + frame_ms * 0.1;
            p.lock_ms = p.lock_ms * 0.9 + lock_ms * 0.1;
            p.frames += 1;
            let w = p.window.get_or_insert_with(std::time::Instant::now);
            if w.elapsed() >= Duration::from_secs(2) {
                p.fps = p.frames as f32 / w.elapsed().as_secs_f32();
                p.frames = 0;
                p.window = Some(std::time::Instant::now());
                p.worst_ms = 0.0;
                p.worst_lock_ms = 0.0;
            }
            p.worst_ms = p.worst_ms.max(frame_ms);
            p.worst_lock_ms = p.worst_lock_ms.max(lock_ms);
            let txt = format!(
                "UI work {:.1} ms (worst {:.0})  ·  waiting for playout {:.1} ms (worst {:.0})  ·  {:.0} frames/s  ·  {} window(s)",
                p.frame_ms,
                p.worst_ms,
                p.lock_ms,
                p.worst_lock_ms,
                p.fps,
                1 + self.workzones.aux.len()
            );
            egui::Area::new(egui::Id::new("perf_overlay")).anchor(egui::Align2::RIGHT_BOTTOM, [-8.0, -30.0]).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(txt).monospace().small());
                });
            });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let arc = self.core.clone();
        let mut c = arc.lock();
        // connected to a server: the local session comes back first, so it is what gets saved
        self.disconnect(&mut c);
        c.plugin_event("app_exit", serde_json::json!({}));
        for i in 0..c.cfg.encoders.len() {
            c.stop_encoder(i);
        }
        c.save_all();
        c.plugins.stop_services();
        drop(c);
        if self.notes_dirty {
            self.notes.save();
        }
        crate::playout::flush_writes();
    }
}
