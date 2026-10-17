use crate::azuracast::Client;
use crate::browser::Browsers;
use crate::config::{data_dir, Ad, AdBreak, CartPad, Category, Marker, PatternItem, Rotation, ServerKind, Template};
use crate::dsp::EQ_FREQS;
use crate::items::PlEntry;
use crate::library::Track;
use crate::playout::{PlayState, Playout};
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
    AddFolder(PathBuf),
    LoadM3u(PathBuf),
    SaveM3u(PathBuf),
    LibFolder(PathBuf),
    Cart(usize, PathBuf),
    SchedPath(usize, PathBuf),
    RotFolder(usize, PathBuf),
    RenderWav(PathBuf),
    AdPath(usize, PathBuf),
    CatFolder(usize, PathBuf),
    SaveReport(PathBuf),
    AnnounceFolder(PathBuf),
    ContainerFiles(Vec<PathBuf>),
    TrackListSource(PathBuf),
    InsertVoice(Vec<PathBuf>),
    InsertPlaylist(PathBuf),
    InsertFromPlaylist(PathBuf),
    ExportCsv(PathBuf),
    AzUpload(Vec<PathBuf>),
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
#[path = "aztool.rs"]
mod aztool;
#[path = "panes.rs"]
mod panes;
#[path = "pltools.rs"]
mod pltools;
#[path = "plview.rs"]
mod plview;

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
    pub audio: crate::audio::AudioManager,
    dev_outputs: Vec<String>,
    dev_inputs: Vec<String>,
    ctx: egui::Context,
    sel: BTreeSet<usize>,
    anchor: Option<usize>,
    lib_filter: String,
    lib_filtered: Vec<usize>,
    lib_key: (String, u64),
    lib_sel: Option<usize>,
    dlg_tx: mpsc::Sender<Dlg>,
    dlg_rx: mpsc::Receiver<Dlg>,
    tag_edit: Option<Track>,
    show_about: bool,
    seek_drag: Option<f32>,
    mic_stream: Option<rodio::cpal::Stream>,
    mic_on: bool,
    tpl_sel: usize,
    report_from: String,
    report_to: String,
    report_excl: String,
    report_rows: Vec<(String, u32, String)>,
    workzones: WorkzoneSet,
    saved_layout: String,
    browsers: Browsers,
    web_state: HashMap<String, panes::WebState>,
    fx_dir: HashMap<String, PathBuf>,
    fx_list: HashMap<String, (PathBuf, std::time::Instant, Vec<(bool, PathBuf)>)>,
    aux_spawn: HashMap<u64, ([f32; 2], Option<[f32; 2]>)>,
    layout_checked: std::time::Instant,
    perf: Option<Perf>,
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
    item_dlg: Option<pltools::ItemDlg>,
    ins_from_pl: Option<(PathBuf, Vec<PlEntry>, Vec<bool>)>,
    show_vt: bool,
    vt: Option<pltools::VoiceRec>,
    show_repeat: bool,
    show_find: bool,
    find_text: String,
    find_focus: bool,
    voices: Vec<String>,
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
    tool_open: std::collections::BTreeSet<ToolWin>,
    fade_edit: Option<tools::FadeEdit>,
    preview: Option<crate::audio::PreviewPlayer>,
    art: Option<(PathBuf, Option<egui::TextureHandle>)>,
    left_w: f32,
    right_w: f32,
    art_rx: Option<mpsc::Receiver<(PathBuf, Option<egui::ColorImage>)>>,
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
        let mut audio = crate::audio::AudioManager::default();
        {
            let c = core.lock();
            audio.apply(&c.engine, &c.cfg.audio_devices);
        }
        let (dlg_tx, dlg_rx) = mpsc::channel();
        App {
            core,
            audio,
            dev_outputs: vec![],
            dev_inputs: vec![],
            ctx: cc.egui_ctx.clone(),
            sel: BTreeSet::new(),
            anchor: None,
            lib_filter: String::new(),
            lib_filtered: vec![],
            lib_key: (String::new(), 0),
            lib_sel: None,
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
            seek_drag: None,
            mic_stream: None,
            mic_on: false,
            tpl_sel: 0,
            report_from: (Local::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string(),
            report_to: Local::now().format("%Y-%m-%d").to_string(),
            report_excl: String::new(),
            report_rows: vec![],
            workzones,
            saved_layout: String::new(),
            browsers: Browsers::new(),
            web_state: HashMap::new(),
            fx_dir: HashMap::new(),
            fx_list: HashMap::new(),
            aux_spawn: HashMap::new(),
            layout_checked: std::time::Instant::now(),
            perf: None,
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
            item_dlg: None,
            ins_from_pl: None,
            show_vt: false,
            vt: None,
            show_repeat: false,
            show_find: false,
            find_text: String::new(),
            find_focus: false,
            voices: vec![],
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
        }
    }

    /// A window that always fits on screen, centred, with a sensible size.
    fn window_for(ctx: &egui::Context, title: &str, want: [f32; 2]) -> egui::Window<'static> {
        let screen = ctx.screen_rect().size();
        let w = want[0].min(screen.x * 0.9).min(screen.x - 24.0).max(280.0);
        let h = want[1].min(screen.y * 0.85).min(screen.y - 24.0).max(200.0);
        let pos = egui::pos2(((screen.x - w) / 2.0).max(8.0), ((screen.y - h) / 2.0).max(8.0));
        egui::Window::new(title.to_string()).default_size([w, h]).default_pos(pos).constrain(true)
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

    fn handle_dialogs(&mut self, core: &mut Playout) {
        while let Ok(d) = self.dlg_rx.try_recv() {
            match d {
                Dlg::AddFiles(v) => {
                    let rows: Vec<PlEntry> = v.iter().map(|p| core.entry_for(p)).collect();
                    self.insert_rows_here(core, rows);
                }
                Dlg::AddFolder(p) => {
                    let rows: Vec<PlEntry> = crate::library::audio_files_in(&p).iter().map(|f| core.entry_for(f)).collect();
                    if rows.is_empty() {
                        core.say("No audio files in that folder");
                    }
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertVoice(v) => {
                    let rows: Vec<PlEntry> = v
                        .iter()
                        .map(|p| {
                            let mut e = core.entry_for(p);
                            e.kind = crate::items::ItemKind::VoiceTrack;
                            e
                        })
                        .collect();
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertPlaylist(p) => {
                    let rows = core.read_m3u(&p);
                    if rows.is_empty() {
                        core.say("That playlist is empty or could not be read");
                    }
                    self.insert_rows_here(core, rows);
                }
                Dlg::InsertFromPlaylist(p) => {
                    let rows = core.read_m3u(&p);
                    let n = rows.len();
                    self.ins_from_pl = Some((p, rows, vec![true; n]));
                }
                Dlg::AnnounceFolder(p) => core.cfg.announce.folder = p.to_string_lossy().to_string(),
                Dlg::ContainerFiles(v) => {
                    if let Some(d) = self.item_dlg.as_mut() {
                        d.items.extend(v);
                    }
                }
                Dlg::TrackListSource(p) => {
                    if let Some(d) = self.item_dlg.as_mut() {
                        d.text = p.to_string_lossy().to_string();
                    }
                }
                Dlg::AzUpload(v) => self.az_upload_here(core, v),
                Dlg::LoadM3u(p) => core.with_tab(self.view_pl, |c| c.load_m3u(&p)),
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
                    let mut s = String::from("Title,Plays,Last played\n");
                    for (t, n, l) in &self.report_rows {
                        s += &format!("\"{}\",{},{}\n", t.replace('"', "'"), n, l);
                    }
                    let _ = std::fs::write(&p, s);
                    core.say(format!("Report saved to {}", p.display()));
                }
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
                Dlg::RotFolder(i, p) => {
                    if let Some(r) = core.cfg.rotations.get_mut(i) {
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
                ui.menu_button("File", |ui| {
                    if ui.button("New playlist").clicked() {
                        self.tab_ops.push(plview::TabOp::New);
                        ui.close_menu();
                    }
                    if ui.button("Open playlist…").clicked() {
                        self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::LoadM3u));
                        ui.close_menu();
                    }
                    if ui.button("Save playlist as…").clicked() {
                        let name =
                            format!("{}.m3u", core.tabs.get(self.view_pl).map(|t| t.name.clone()).unwrap_or_else(|| "playlist".into()));
                        self.dlg(move || {
                            rfd::FileDialog::new().add_filter("Playlist", &["m3u"]).set_file_name(&name).save_file().map(Dlg::SaveM3u)
                        });
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Add library folder…").clicked() {
                        self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::LibFolder));
                        ui.close_menu();
                    }
                    if ui.button("Rescan library").clicked() {
                        core.start_scan();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Station profiles…").clicked() {
                        self.show_profiles = true;
                        ui.close_menu();
                    }
                    if self.logged_in.as_ref().is_some_and(|a| a.admin) && ui.button("Users…").clicked() {
                        self.show_users = true;
                        ui.close_menu();
                    }
                    if ui.button("Log out").clicked() {
                        self.logged_in = None;
                        self.login_password.clear();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.menu_button("Work zones", |ui| {
                        for (n, text) in [(1u8, "1   (playlist only)"), (2, "2   (playlist + panel)"), (3, "3   (playlist + two panels)")] {
                            let b = egui::Button::new(text).shortcut_text(format!("Ctrl+{n}")).selected(self.workzones.panels == n);
                            if ui.add(b).clicked() {
                                self.workzones.panels = n;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.add(egui::Button::new("Additional (new window)").shortcut_text("Ctrl+4")).clicked() {
                            self.workzones.new_aux(Tab::AuxPlaylist);
                            ui.close_menu();
                        }
                        if !self.workzones.aux.is_empty() {
                            ui.label(RichText::new(format!("{} Additional window(s) open", self.workzones.aux.len())).small().color(GRAY));
                            if ui.button("Close all Additional windows").clicked() {
                                for a in self.workzones.aux.clone() {
                                    self.browsers.close_window(&crate::workzones::aux_title(a.id));
                                    self.workzones.close_aux(a.id);
                                }
                                ui.close_menu();
                            }
                        }
                    });
                    ui.add(egui::Checkbox::new(&mut self.workzones.show_left, "Left panel")).on_hover_text("Ctrl+L");
                    ui.separator();
                    for (tab, key) in [(Tab::FileExplorer, "F1"), (Tab::Search, "F2"), (Tab::Scheduler, "F3")] {
                        if ui.add(egui::Button::new(self.tab_name(tab)).shortcut_text(key)).clicked() {
                            self.workzones.show_left = true;
                            self.workzones.left.add(tab);
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    let mut dark = self.workzones.dark;
                    if ui.checkbox(&mut dark, "Dark theme").changed() {
                        self.workzones.dark = dark;
                        crate::theme::apply(ctx, dark, self.workzones.scale);
                    }
                    if ui.add(egui::Button::new("Full screen").shortcut_text("F11").selected(self.fullscreen)).clicked() {
                        self.fullscreen = !self.fullscreen;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
                        ui.close_menu();
                    }
                });
                ui.menu_button("Playlist", |ui| self.playlist_menu(ui, core));
                ui.menu_button("Tools", |ui| {
                    for w in [
                        ToolWin::FadeEditor,
                        ToolWin::Generator,
                        ToolWin::Ads,
                        ToolWin::AudioFx,
                        ToolWin::Streams,
                        ToolWin::AzuraCast,
                        ToolWin::Reports,
                        ToolWin::History,
                        ToolWin::Notepad,
                    ] {
                        let sc = match w {
                            ToolWin::Generator => "Ctrl+G",
                            ToolWin::AudioFx => "F4",
                            ToolWin::Notepad => "T",
                            _ => "",
                        };
                        if ui.add(egui::Button::new(format!("{}  {}…", w.icon(), w.label())).shortcut_text(sc)).clicked() {
                            if w == ToolWin::FadeEditor {
                                self.open_fade_editor(core, None);
                            } else {
                                self.open_tool(w);
                            }
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("🔴 Record voice track…").clicked() {
                        self.show_vt = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Jingles", |ui| {
                    ui.label(RichText::new("Cart wall").small().color(GRAY));
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
                    if ui.button("Show cart wall").clicked() {
                        self.workzones.show_left = true;
                        self.workzones.left.add(Tab::Carts);
                        ui.close_menu();
                    }
                });
                ui.menu_button("Settings", |ui| {
                    if ui.button("⚙ Settings…").clicked() {
                        self.show_settings = true;
                        ui.close_menu();
                    }
                    if ui.button("Station profiles…").clicked() {
                        self.show_profiles = true;
                        ui.close_menu();
                    }
                    if ui.button("Track List Repeat Protection…").clicked() {
                        self.show_repeat = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.add(egui::Button::new("Performance").shortcut_text("Ctrl+Shift+P").selected(self.perf.is_some())).clicked() {
                        self.perf = if self.perf.is_some() { None } else { Some(Perf::default()) };
                        ui.close_menu();
                    }
                    if ui.button("Check for updates…").clicked() {
                        crate::updater::check(self.updates.clone(), ctx.clone(), true);
                        self.show_update = true;
                        ui.close_menu();
                    }
                    if ui.button("About RadioMaster").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
                ui.separator();
                // quick buttons
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    self.show_settings = true;
                }
                for (w, short) in [
                    (ToolWin::FadeEditor, "Track editor"),
                    (ToolWin::Generator, "Generator"),
                    (ToolWin::Ads, "Ads"),
                    (ToolWin::AudioFx, "Audio FX"),
                    (ToolWin::Streams, "Streams"),
                    (ToolWin::AzuraCast, "AzuraCast"),
                ] {
                    if ui.button(short).on_hover_text(w.label()).clicked() {
                        if w == ToolWin::FadeEditor {
                            self.open_fade_editor(core, None);
                        } else {
                            self.open_tool(w);
                        }
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
                if ui.button("➕🗗").on_hover_text("Additional window (Ctrl+4) — open as many as you like").clicked() {
                    self.workzones.new_aux(Tab::AuxPlaylist);
                }
                let new_version = self.updates.lock().available.as_ref().map(|a| a.version.clone());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Master").strong().size(15.0).color(Color32::from_rgb(20, 150, 235)));
                    ui.add_space(-ui.spacing().item_spacing.x);
                    ui.label(RichText::new("Radio").strong().size(15.0));
                    if let Some(t) = &icon {
                        ui.add(egui::Image::new((t.id(), egui::vec2(18.0, 18.0))));
                    }
                    if let Some(v) = &new_version {
                        let b = egui::Button::new(RichText::new(format!("⬆ Update available: v{v}")).strong().color(Color32::WHITE))
                            .fill(Color32::from_rgb(20, 150, 235));
                        if ui.add(b).on_hover_text("A newer RadioMaster is on GitHub — click to see what's new and install it").clicked()
                        {
                            self.show_update = true;
                        }
                    }
                });
            });
        });
        if self.show_about {
            Self::window_for(ctx, "About RadioMaster", [380.0, 240.0]).open(&mut self.show_about).collapsible(false).show(ctx, |ui| {
                if let Some(t) = &logo {
                    let size = t.size_vec2();
                    ui.add(egui::Image::new((t.id(), egui::vec2(300.0, 300.0 * size.y / size.x))));
                }
                ui.heading(format!("RadioMaster {}", env!("CARGO_PKG_VERSION")));
                ui.label("Radio automation in Rust.");
                ui.label("Icecast · Shoutcast · AzuraCast");
            });
        }
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
                            .on_hover_text("Time until the next hour marker in the playlist");
                        }
                    });
                });
                tile(ui, Color32::from_rgb(255, 214, 170), tiles[1], &mut |ui| {
                    ui.label(RichText::new("LAST TRACK").color(Color32::from_rgb(90, 60, 20)).small());
                    ui.add(egui::Label::new(RichText::new(previous.clone()).color(Color32::BLACK)).truncate());
                });
                tile(ui, Color32::from_rgb(176, 240, 186), tiles[2], &mut |ui| {
                    ui.label(RichText::new("●  ON AIR").strong().color(Color32::from_rgb(18, 90, 34)).small());
                    ui.add(egui::Label::new(RichText::new(title.clone()).color(Color32::BLACK).size(17.0).strong()).truncate());
                });
                tile(ui, Color32::from_rgb(245, 246, 190), tiles[3], &mut |ui| {
                    ui.label(RichText::new("COMING UP NEXT").color(Color32::from_rgb(80, 80, 30)).small());
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
                        ui.horizontal(|ui| {
                            if ui.add_sized(bs, egui::Button::new("⏮")).on_hover_text("Previous").clicked() {
                                core.prev();
                            }
                            if ui
                                .add_sized(bs, egui::Button::new(RichText::new("▶").color(Color32::from_rgb(20, 140, 50))))
                                .on_hover_text("Play")
                                .clicked()
                            {
                                let s = self.sel.iter().next().cloned();
                                core.play_pressed(s);
                            }
                            if ui.add_sized(bs, egui::Button::new("⏸")).on_hover_text("Pause").clicked() {
                                core.pause_toggle();
                            }
                            if ui
                                .add_sized(bs, egui::Button::new(RichText::new("⏹").color(Color32::from_rgb(200, 40, 40))))
                                .on_hover_text("Stop")
                                .clicked()
                            {
                                core.stop();
                            }
                            if ui.add_sized(bs, egui::Button::new("⏭")).on_hover_text("Next").clicked() {
                                core.next();
                            }
                        });
                        ui.spacing_mut().slider_width = (transport_w - 95.0).max(70.0);
                        ui.horizontal(|ui| {
                            ui.label("Vol ");
                            let mut v = core.cfg.volume;
                            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                                core.set_volume(v);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Fade");
                            let smart = core.cfg.fade.mode == crate::fade::FadeMode::Smart;
                            if ui
                                .selectable_label(smart, "Smart")
                                .on_hover_text("Smart fading: analyse each track and pick the mix point automatically")
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
                        ui.label(RichText::new("ELAPSED").small().color(GRAY));
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
                            ui.label("L");
                            vu(ui, st.peak_l, meters_w - 62.0);
                            let (txt, col) = match core.state {
                                PlayState::Playing => ("PLAY", Color32::from_rgb(20, 140, 50)),
                                PlayState::Paused => ("PAUSE", Color32::from_rgb(190, 120, 0)),
                                PlayState::Stopped => ("STOP", GRAY),
                            };
                            ui.label(RichText::new(txt).small().strong().color(col));
                        });
                        ui.horizontal(|ui| {
                            ui.label("R");
                            vu(ui, st.peak_r, meters_w - 62.0);
                        });
                        ui.horizontal(|ui| {
                            let mut on = self.mic_on;
                            if ui.toggle_value(&mut on, "🎤 Mic").changed() {
                                if on {
                                    match crate::mic::start(core.engine.clone(), &core.cfg.audio_devices.input) {
                                        Ok(s) => {
                                            self.mic_stream = Some(s);
                                            self.mic_on = true;
                                        }
                                        Err(e) => core.say(format!("Mic error: {e}")),
                                    }
                                } else {
                                    self.mic_stream = None;
                                    self.mic_on = false;
                                }
                            }
                            ui.spacing_mut().slider_width = ((meters_w - 90.0) / 2.0).max(36.0);
                            ui.add(egui::Slider::new(&mut core.cfg.mic_gain, 0.0..=2.0).show_value(false)).on_hover_text("Mic gain");
                            ui.add(egui::Slider::new(&mut core.cfg.mic_duck, 0.0..=1.0).show_value(false))
                                .on_hover_text("Music level while mic is on");
                        });
                    });
                });
            });
            ui.add_space(4.0);
        });
    }

    fn tab_queue(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong("PLAYBACK QUEUE");
            if ui.button("Add files…").clicked() {
                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AddFiles));
            }
        });
        let start = core.current.map(|i| i + 1).unwrap_or(0);
        let mut play = None;
        for (index, entry) in core.playlist.iter().enumerate().skip(start).take(50) {
            ui.horizontal(|ui| {
                if ui.small_button("▶").on_hover_text("Play this item next").clicked() {
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
            ui.label("Queue is empty. Add music to the playlist.");
        }
    }

    fn tab_search(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong("SEARCH LIBRARY");
            if ui.button("Rescan").clicked() {
                core.start_scan();
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.lib_filter).hint_text("Search artist, title, album, genre…").desired_width(f32::INFINITY),
        );
        if self.lib_filter.trim().is_empty() {
            ui.label("Enter a search to find music in your library.");
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
                if ui.small_button("➕").on_hover_text("Add to playlist").clicked() {
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
        ui.strong("STATION STATISTICS");
        ui.separator();
        ui.label(format!("Playlist tracks: {}", core.playlist.len()));
        ui.label(format!("Total playlist length: {}", fmt(core.total_secs())));
        ui.label(format!("History entries this session: {}", core.history.len()));
        let live = core.handles.iter().flatten().filter(|h| h.status.lock().state == EncState::Live).count();
        ui.colored_label(if live > 0 { GREEN } else { GRAY }, format!("Connected encoders: {live}"));
        if let Some(np) = &core.azura.lock().np {
            ui.label(format!("Listeners: {} ({} unique)", np.listeners.current, np.listeners.unique));
        } else {
            ui.label("Listeners: not available from AzuraCast");
        }
        ui.separator();
        ui.small("Live station status; historical audience graphs require a stats source.");
    }

    fn tab_clock(&mut self, ui: &mut egui::Ui, core: &Playout) {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("STUDIO CLOCK").small().color(GRAY));
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
        ui.strong("Projected start times");
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
            ui.strong("SEGMENT / SCRIPT");
            if ui.button("Save").clicked() {
                let _ = std::fs::write(data_dir().join("segment.txt"), &self.segment_text);
                core.say("Segment saved");
            }
            if ui.button("New").clicked() {
                self.segment_text.clear();
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.segment_name).desired_width(220.0));
        ui.add(egui::TextEdit::multiline(&mut self.segment_text).desired_width(f32::INFINITY).desired_rows(20));
        ui.small("Presenter notes are local text; they are not automatically broadcast.");
    }

    fn settings_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        let mut close = false;
        Self::window_for(ctx,"RadioMaster Settings",[900.0,620.0]).id(egui::Id::new("settings_window"))
            .open(&mut open).collapsible(false).resizable(true).show(ctx,|ui| {
                // footer first so Save / Close can never be pushed off-screen
                egui::TopBottomPanel::bottom("settings_footer").exact_height(40.0).show_inside(ui,|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                        if ui.button("Close").clicked() {close=true;}
                        if ui.button("Save configuration").clicked() {core.save_all(); let _=self.workzones.save();}
                    });
                });
                egui::SidePanel::left("settings_pages").resizable(false).exact_width(178.0).show_inside(ui,|ui| {
                    ui.heading("Settings");
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt("settings_pages_scroll").show(ui,|ui| {
                        for page in SettingsPage::ALL {
                            if ui.selectable_label(self.settings_page==page,page.label()).clicked() {
                                self.settings_page=page;
                            }
                        }
                    });
                });
                egui::CentralPanel::default().show_inside(ui,|ui| {
                    ui.heading(self.settings_page.label());
                    ui.separator();
                    egui::ScrollArea::both().id_salt("settings_content").auto_shrink([false,false]).show(ui,|ui| {
                        match self.settings_page {
                            SettingsPage::General=>{
                                ui.label("Radio automation, playback and studio settings.");
                                ui.label(format!("Data folder: {}",data_dir().display()));
                                ui.label(format!("Active profile: {}",self.profile_name));
                                if ui.button("Station profiles…").clicked(){self.show_profiles=true;}
                                ui.separator();
                                ui.checkbox(&mut core.cfg.check_updates, "Check for updates when RadioMaster starts");
                                ui.label(RichText::new(format!("This is version {}. Help → Check for updates looks right now.", crate::updater::CURRENT)).small().color(GRAY));
                            },
                            SettingsPage::Library=>{
                                ui.label("Music folders scanned by the library");
                                let mut remove=None;
                                for (i,path) in core.cfg.library_folders.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(path);
                                        if ui.small_button("Remove").clicked(){remove=Some(i);}
                                    });
                                }
                                if let Some(i)=remove {core.cfg.library_folders.remove(i);}
                                if ui.button("Add folder…").clicked() {
                                    self.dlg(||rfd::FileDialog::new().pick_folder().map(Dlg::LibFolder));
                                }
                                if ui.button("Rescan").clicked(){core.start_scan();}
                            },
                            SettingsPage::Playback=>{
                                ui.checkbox(&mut core.cfg.loop_playlist,"Loop the playlist");
                                ui.checkbox(&mut core.cfg.auto_enabled,"Enable automated playlist top-up");
                                ui.checkbox(&mut core.cfg.random_fill,"Fill from music library");
                                ui.separator();
                                ui.heading("Transitions");
                                self.smart_fade_settings(ui, core);
                            },
                            SettingsPage::Streaming=>self.tab_streaming(ui,core),
                            SettingsPage::Audio=>self.tab_dsp(ui,core),
                            SettingsPage::Devices=>self.tab_devices(ui,core),
                            SettingsPage::Automation=>self.tab_settings(ui,core),
                            SettingsPage::Remote=>{
                                ui.label("Remote control API (restart required)");
                                ui.checkbox(&mut core.cfg.remote.enabled,"Enable remote API");
                                ui.add(egui::DragValue::new(&mut core.cfg.remote.port).range(1..=65535).prefix("Port "));
                                ui.add(egui::TextEdit::singleline(&mut core.cfg.remote.password).password(true).hint_text("API password"));
                                ui.colored_label(AMBER,"Set a strong password before enabling network access.");
                            },
                            SettingsPage::Appearance=>{
                                if ui.checkbox(&mut self.workzones.dark,"Dark theme").changed() {
                                    crate::theme::apply(ctx, self.workzones.dark, self.workzones.scale);
                                }
                                ui.horizontal(|ui| {
                                    ui.label("Interface size");
                                    let r = ui.add(egui::Slider::new(&mut self.workzones.scale, 0.8..=1.6).show_value(false));
                                    ui.label(format!("{}%", (self.workzones.scale * 100.0).round() as i32));
                                    if (r.drag_stopped() || (r.changed() && !r.dragged())) && !ui.input(|i| i.pointer.any_down()) {
                                        crate::theme::apply(ctx, self.workzones.dark, self.workzones.scale);
                                    }
                                    if ui.button("Reset").clicked() {
                                        self.workzones.scale = 1.0;
                                        crate::theme::apply(ctx, self.workzones.dark, 1.0);
                                    }
                                });
                                ui.small("Make everything larger or smaller if the text is hard to read.");
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label("Work zone");
                                    for n in 1..=3u8 {
                                        ui.selectable_value(&mut self.workzones.panels, n, format!("{n} panel(s)"));
                                    }
                                });
                                ui.small("View → Work zones (Ctrl+1/2/3) sets how many panels the main window has; Ctrl+4 opens another Additional window.");
                                if ui.button("Reset panel layout").clicked() {
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
                                if admin && ui.button("Manage operator accounts…").clicked(){self.show_users=true;}
                                if !admin {ui.label("Only administrators can manage user accounts.");}
                                ui.small("Accounts gate the local UI; secure your Windows account and streaming API separately.");
                            },
                        }

                    });
                });
            });
        self.show_settings = open && !close;
    }

    fn authentication(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                if let Some(t) = self.logo_texture(ui.ctx()) {
                    let size = t.size_vec2();
                    ui.add(egui::Image::new((t.id(), egui::vec2(340.0, 340.0 * size.y / size.x))));
                    ui.add_space(10.0);
                }
                ui.heading("Studio Access");
                ui.add_space(18.0);
                if self.accounts.users.is_empty() {
                    ui.label("First run: create your administrator account.");
                } else {
                    ui.label("Sign in to access station controls.");
                }
                ui.add_sized([300.0, 26.0], egui::TextEdit::singleline(&mut self.login_name).hint_text("Username"));
                ui.add_sized(
                    [300.0, 26.0],
                    egui::TextEdit::singleline(&mut self.login_password).password(true).hint_text("Password (12+ characters for setup)"),
                );
                let enter =
                    ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.login_name.trim().is_empty() && !self.login_password.is_empty();
                if ui.button(if self.accounts.users.is_empty() { "Create administrator" } else { "Sign in" }).clicked() || enter {
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
                }
                ui.colored_label(RED, &self.auth_error);
            });
        });
    }

    fn users_window(&mut self, ctx: &egui::Context) {
        if !self.show_users || !self.logged_in.as_ref().map(|u| u.admin).unwrap_or(false) {
            return;
        }
        let mut open = self.show_users;
        Self::window_for(ctx, "Access control — operators", [520.0, 380.0]).open(&mut open).show(ctx, |ui| {
            ui.label("Existing operators");
            for u in &self.accounts.users {
                ui.label(format!("{} — {}", u.name, if u.admin { "Administrator" } else { "Operator" }));
            }
            ui.separator();
            ui.label("Create operator");
            ui.add(egui::TextEdit::singleline(&mut self.login_name).desired_width(220.0));
            ui.add(egui::TextEdit::singleline(&mut self.login_password).password(true).hint_text("New password"));
            ui.checkbox(&mut self.new_user_admin, "Administrator");
            if ui.button("Create user").clicked() {
                if let Err(e) = self.accounts.add(&self.login_name, &self.login_password, self.new_user_admin) {
                    self.auth_error = e;
                } else {
                    self.auth_error.clear();
                }
                self.login_password.clear();
            }
            ui.colored_label(RED, &self.auth_error);
        });
        self.show_users = open;
    }

    fn profiles_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.show_profiles {
            return;
        }
        let mut open = self.show_profiles;
        Self::window_for(ctx, "Station profiles", [460.0, 380.0]).open(&mut open).show(ctx, |ui| {
            ui.label("Save or load individual station settings (including encoder passwords). Keep the profile directory private.");
            ui.horizontal(|ui| {
                ui.label("Profile:");
                ui.add(egui::TextEdit::singleline(&mut self.profile_name).desired_width(220.0));
                if ui.button("Save snapshot").clicked() {
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

    fn tab_library(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("➕ Folder…").clicked() {
                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::LibFolder));
            }
            if ui.add_enabled(!core.scanning, egui::Button::new("⟳")).on_hover_text("Rescan").clicked() {
                core.start_scan();
            }
            if ui.button("Add all").on_hover_text("Add all shown tracks to the playlist").clicked() {
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
        ui.add(egui::TextEdit::singleline(&mut self.lib_filter).desired_width(f32::INFINITY).hint_text("🔍 search artist, title, album"));
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

        let mut act: Option<(usize, &'static str)> = None;
        let lib = &core.library;
        let filt = &self.lib_filtered;
        let lsel = &mut self.lib_sel;
        let lib_w = ui.available_width();
        TableBuilder::new(ui)
            .id_salt(("library_table", (lib_w / 24.0) as i32))
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .column(Column::initial((lib_w * 0.38).max(70.0)).at_least(40.0).clip(true))
            .column(Column::remainder().at_least(60.0).clip(true))
            .column(Column::exact(58.0))
            .header(20.0, |mut h| {
                for t in ["Artist", "Title", "Length"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, filt.len(), |mut row| {
                    let li = filt[row.index()];
                    let t = &lib[li];
                    row.set_selected(*lsel == Some(li));
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
                    if r.clicked() || r.secondary_clicked() {
                        *lsel = Some(li);
                    }
                    if r.double_clicked() {
                        act = Some((li, "add"));
                    }
                    r.context_menu(|ui| {
                        for (l, k) in [
                            ("Add to playlist", "add"),
                            ("Insert as next", "next"),
                            ("Play now", "now"),
                            ("✏ Track editor…", "tags"),
                            ("☁ Upload to AzuraCast", "up"),
                        ] {
                            if ui.button(l).clicked() {
                                act = Some((li, k));
                                ui.close_menu();
                            }
                        }
                    });
                });
            });

        if let Some((li, a)) = act {
            let t = core.library[li].clone();
            match a {
                "add" => {
                    let v = self.view_pl;
                    core.with_tab(v, |c| c.add_paths(vec![t.path]));
                }
                "next" => core.insert_next(vec![t.path]),
                "now" => {
                    core.insert_next(vec![t.path]);
                    core.next();
                }
                "tags" => self.open_track_editor_for(core, t),
                _ => Self::upload(core, vec![t.path]),
            }
        }
    }

    fn tab_carts(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let cols = ((ui.available_width() / 116.0) as usize).clamp(1, 6);
            let pad_width = ((ui.available_width() - (cols.saturating_sub(1)) as f32 * 6.0) / cols as f32 - 8.0).max(86.0);
            let mut trigger = None;
            egui::Grid::new("carts").spacing([6.0, 6.0]).show(ui, |ui| {
                for i in 0..core.cfg.carts.len() {
                    let pad: &mut CartPad = &mut core.cfg.carts[i];
                    let loaded = !pad.path.is_empty();
                    let label = if loaded { pad.label.clone() } else { format!("Pad {}", i + 1) };
                    let fill = if loaded { Color32::from_rgb(40, 80, 130) } else { Color32::from_gray(48) };
                    let b = ui.add_sized([pad_width, 54.0], egui::Button::new(RichText::new(label).strong()).fill(fill).wrap());
                    if b.clicked() && loaded {
                        trigger = Some(i);
                    }
                    b.context_menu(|ui| {
                        if ui.button("Assign audio file…").clicked() {
                            self.dlg(move || {
                                rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_file().map(|p| Dlg::Cart(i, p))
                            });
                            ui.close_menu();
                        }
                        ui.label("Label:");
                        ui.add(egui::TextEdit::singleline(&mut pad.label).desired_width(220.0));
                        if ui.button("Clear pad").clicked() {
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
                RichText::new("Click to fire (plays over the main program). Right-click a pad to assign or relabel.").small().color(GRAY),
            );
        });
    }

    fn tab_scheduler(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if ui.button("➕ Add event").clicked() {
            core.cfg.events.push(Event::default());
        }
        let mut del = None;
        let n_enc = core.cfg.encoders.len();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for i in 0..core.cfg.events.len() {
                let e = &mut core.cfg.events[i];
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut e.enabled, "");
                        ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(160.0));
                        ui.add(egui::DragValue::new(&mut e.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                        ui.label(":");
                        ui.add(egui::DragValue::new(&mut e.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                        for (d, l) in e.days.iter_mut().zip(["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]) {
                            ui.checkbox(d, l);
                        }
                        if ui.button("🗑").clicked() {
                            del = Some(i);
                        }
                    });
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("act", i)).selected_text(e.action.label()).width(230.0).show_ui(ui, |ui| {
                            for a in ActionKind::ALL {
                                ui.selectable_value(&mut e.action, a, a.label());
                            }
                        });
                        if e.action.needs_path() {
                            ui.add(egui::TextEdit::singleline(&mut e.path).desired_width(360.0));
                            if ui.button("Browse…").clicked() {
                                let pl = e.action == ActionKind::LoadPlaylistAndPlay;
                                self.dlg(move || {
                                    let d = rfd::FileDialog::new();
                                    let d = if pl {
                                        d.add_filter("Playlist", &["m3u", "m3u8"])
                                    } else {
                                        d.add_filter("Audio", &crate::library::EXTS)
                                    };
                                    d.pick_file().map(|p| Dlg::SchedPath(i, p))
                                });
                            }
                        }
                        if e.action.needs_arg() {
                            if e.action == ActionKind::SetVolume {
                                ui.add(egui::Slider::new(&mut e.arg, 0.0..=1.0));
                            } else {
                                let mut k = e.arg as usize;
                                ui.label("encoder #");
                                ui.add(egui::DragValue::new(&mut k).range(0..=n_enc.saturating_sub(1)));
                                e.arg = k as f32;
                                ui.label(RichText::new("(0 = first)").small().color(GRAY));
                            }
                        }
                    });
                });
            }
        });
        if let Some(i) = del {
            core.cfg.events.remove(i);
        }
    }

    fn refresh_devices(&mut self, core: &mut Playout) {
        self.dev_outputs = crate::audio::list_outputs();
        self.dev_inputs = crate::audio::list_inputs();
        let outs = &mut core.cfg.audio_devices.outputs;
        for n in &self.dev_outputs {
            if !outs.iter().any(|o| &o.name == n) {
                outs.push(crate::config::OutputSel { name: n.clone(), enabled: false, gain: 1.0 });
            }
        }
        if !outs.iter().any(|o| o.enabled) {
            if let Some(d) = crate::audio::default_output_name() {
                if let Some(o) = outs.iter_mut().find(|o| o.name == d) {
                    o.enabled = true;
                }
            }
        }
    }

    fn tab_devices(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if self.dev_outputs.is_empty() && self.dev_inputs.is_empty() {
            self.refresh_devices(core);
        }
        ui.horizontal(|ui| {
            if ui.button("⟳ Refresh devices").clicked() {
                self.refresh_devices(core);
            }
            if ui.button("Apply outputs").clicked() {
                self.audio.apply(&core.engine, &core.cfg.audio_devices);
                core.say("Audio outputs applied");
            }
        });
        ui.add_space(4.0);
        ui.heading("Outputs");
        ui.label("Tick every device that should play the on-air mix. The first working device is the primary; the others get a copy (monitor / headphones / second sound card). Each has its own volume.");
        let mut changed = false;
        let mut to_top = None;
        let n = core.cfg.audio_devices.outputs.len();
        for i in 0..n {
            let name = core.cfg.audio_devices.outputs[i].name.clone();
            let present = self.dev_outputs.contains(&name);
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
                    ui.label(RichText::new("(not connected)").small().color(GRAY));
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
                if i > 0 && ui.small_button("⬆ primary").on_hover_text("Make this the primary output").clicked() {
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
            self.audio.apply(&core.engine, &core.cfg.audio_devices);
        }
        if !self.audio.report.is_empty() {
            ui.add_space(4.0);
            for (d, s) in &self.audio.report {
                ui.label(RichText::new(format!("{d}: {s}")).small().color(GRAY));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.heading("Input (microphone / line in)");
        let cur = if core.cfg.audio_devices.input.is_empty() { "System default".to_string() } else { core.cfg.audio_devices.input.clone() };
        egui::ComboBox::from_id_salt("input_device").width(340.0).selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut core.cfg.audio_devices.input, String::new(), "System default");
            for n in &self.dev_inputs {
                ui.selectable_value(&mut core.cfg.audio_devices.input, n.clone(), n);
            }
        });
        ui.small("Switch the Mic button off and on again after changing the input.");
        ui.add_space(8.0);
        ui.separator();
        ui.heading("Preview (track editor)");
        ui.label("Where ▶ Play / Preview mix in the track editor is heard. It is never sent to the stream, but if it is the same device as your on-air output, it will be heard in the room.");
        let cur =
            if core.cfg.audio_devices.preview.is_empty() { "System default".to_string() } else { core.cfg.audio_devices.preview.clone() };
        egui::ComboBox::from_id_salt("preview_device").width(340.0).selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut core.cfg.audio_devices.preview, String::new(), "System default");
            for n in &self.dev_outputs {
                ui.selectable_value(&mut core.cfg.audio_devices.preview, n.clone(), n);
            }
        });
    }

    fn tab_streaming(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let mut act: Option<(usize, &'static str)> = None;
        if ui.button("➕ Add encoder").clicked() {
            core.add_encoder();
        }
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
                            ui.label("Name");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.name));
                            ui.label("Server type");
                            egui::ComboBox::from_id_salt(("kind", i)).selected_text(format!("{:?}", enc.kind)).show_ui(ui, |ui| {
                                ui.selectable_value(&mut enc.kind, ServerKind::Icecast, "Icecast");
                                ui.selectable_value(&mut enc.kind, ServerKind::Shoutcast, "Shoutcast (v2 DJ)");
                            });
                            ui.end_row();
                            ui.label("Host");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.host));
                            ui.label("Port");
                            ui.add(egui::DragValue::new(&mut enc.port).range(1..=65535));
                            ui.end_row();
                            ui.label("Mount");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.mount));
                            ui.label("Bitrate (kbps)");
                            egui::ComboBox::from_id_salt(("br", i)).selected_text(enc.bitrate_kbps.to_string()).show_ui(ui, |ui| {
                                for b in [64, 96, 128, 160, 192, 256, 320] {
                                    ui.selectable_value(&mut enc.bitrate_kbps, b, b.to_string());
                                }
                            });
                            ui.end_row();
                            ui.label("Username");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.username));
                            ui.label("Password");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.password).password(true));
                            ui.end_row();
                            ui.label("Stream name");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.stream_name));
                            ui.label("Genre");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.genre));
                            ui.end_row();
                            ui.label("Description");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.description));
                            ui.label("Website");
                            ui.add_sized([230.0, 26.0], egui::TextEdit::singleline(&mut enc.url));
                            ui.end_row();
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut enc.public, "List publicly");
                            ui.checkbox(&mut enc.send_metadata, "Send track titles");
                            ui.checkbox(&mut enc.start_with_app, "Connect on startup");
                        });
                        ui.horizontal(|ui| {
                            if running {
                                if ui.button(RichText::new("⏹ Disconnect").color(RED)).clicked() {
                                    act = Some((i, "stop"));
                                }
                            } else if ui.button(RichText::new("▶ Connect").color(GREEN)).clicked() {
                                act = Some((i, "start"));
                            }
                            if ui
                                .button("AzuraCast DJ preset")
                                .on_hover_text("Icecast, mount “/”, user/pass = AzuraCast Streamer account, port = station DJ port")
                                .clicked()
                            {
                                act = Some((i, "preset"));
                            }
                            if ui.button("🗑 Remove").clicked() {
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
            ui.strong("Connection");
            let a = &mut core.cfg.azuracast;
            egui::Grid::new("az").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label("Enable polling");
                ui.checkbox(&mut a.enabled, "");
                ui.end_row();
                ui.label("Base URL");
                ui.add(egui::TextEdit::singleline(&mut a.base_url).hint_text("https://radio.example.com").desired_width(260.0));
                ui.end_row();
                ui.label("Station");
                ui.add(egui::TextEdit::singleline(&mut a.station).hint_text("short name or ID").desired_width(260.0));
                ui.end_row();
                ui.label("API key");
                ui.add(
                    egui::TextEdit::singleline(&mut a.api_key).password(true).hint_text("id:secret (for skip/upload)").desired_width(260.0),
                );
                ui.end_row();
                ui.label("Upload folder");
                ui.add(egui::TextEdit::singleline(&mut a.upload_dir).hint_text("optional").desired_width(260.0));
                ui.end_row();
                ui.label("Poll every (s)");
                ui.add(egui::DragValue::new(&mut a.poll_secs).range(2..=60));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("⏭ Skip AutoDJ song").clicked() {
                    let (c, n) = (core.cfg.azuracast.clone(), core.notify.clone());
                    thread::spawn(move || {
                        *n.lock() = match Client::new(&c).skip() {
                            Ok(_) => "AzuraCast: skipped current AutoDJ song".into(),
                            Err(e) => format!("AzuraCast skip failed: {e}"),
                        };
                    });
                }
                if ui.button("Use for encoder #0").on_hover_text("Copies host + server type into the first encoder").clicked() {
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
                RichText::new("Tip: create a Streamer/DJ account in AzuraCast, then use the “AzuraCast DJ preset” on the Streaming tab.")
                    .small()
                    .color(GRAY),
            );

            let ui = &mut cols[1];
            ui.strong("On air");
            match &np {
                Some(n) => {
                    ui.label(RichText::new(&n.station.name).heading());
                    ui.label(if n.is_online { RichText::new("● online").color(GREEN) } else { RichText::new("● offline").color(RED) });
                    if n.live.is_live {
                        ui.label(RichText::new(format!("🎙 Live DJ: {}", n.live.streamer_name)).color(AMBER));
                    }
                    ui.label(format!("{} — {}", n.now_playing.song.artist, n.now_playing.song.title));
                    ui.label(format!("Listeners: {} (unique {}, total {})", n.listeners.current, n.listeners.unique, n.listeners.total));
                    if let Some(p) = &n.playing_next {
                        ui.label(RichText::new(format!("Next: {} — {}", p.song.artist, p.song.title)).color(GRAY));
                    }
                    ui.separator();
                    ui.strong("Recent");
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for h in n.song_history.iter().take(15) {
                            ui.label(format!("{} — {}", h.song.artist, h.song.title));
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("No data yet — enable polling and fill in URL + station.").color(GRAY));
                }
            }
        });
    }

    fn tab_history(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.label(RichText::new("Also appended to history.csv in the RadioMaster data folder.").small().color(GRAY));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for h in core.history.iter().rev() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&h.time).monospace().color(GRAY));
                    ui.label(&h.text);
                });
            }
        });
    }

    fn tab_settings(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong("Library folders");
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
            ui.strong("Rotations (insert jingles / ads / sweepers every N songs)");
            let mut rdel = None;
            for i in 0..core.cfg.rotations.len() {
                let r = &mut core.cfg.rotations[i];
                ui.horizontal(|ui| {
                    ui.checkbox(&mut r.enabled, "");
                    ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(100.0));
                    ui.label("every");
                    ui.add(egui::DragValue::new(&mut r.every_n).range(1..=50));
                    ui.label("songs from");
                    ui.add(egui::TextEdit::singleline(&mut r.folder).desired_width(220.0));
                    if ui.button("Browse…").clicked() {
                        self.dlg(move || rfd::FileDialog::new().pick_folder().map(|p| Dlg::RotFolder(i, p)));
                    }
                    if ui.small_button("🗑").clicked() {
                        rdel = Some(i);
                    }
                });
            }
            if let Some(i) = rdel {
                core.cfg.rotations.remove(i);
            }
            if ui.button("➕ Add rotation").clicked() {
                core.cfg.rotations.push(Rotation::default());
            }
            ui.label(RichText::new("The rotation folder must be inside a scanned library folder.").small().color(GRAY));
            ui.separator();
            ui.strong("Export now-playing metadata");
            egui::Grid::new("exp").num_columns(2).show(ui, |ui| {
                ui.label("Text file");
                ui.add(egui::TextEdit::singleline(&mut core.cfg.export.text_file).desired_width(380.0));
                ui.end_row();
                ui.label("XML file");
                ui.add(egui::TextEdit::singleline(&mut core.cfg.export.xml_file).desired_width(380.0));
                ui.end_row();
                ui.label("HTTP GET URL");
                ui.add(
                    egui::TextEdit::singleline(&mut core.cfg.export.http_url)
                        .desired_width(380.0)
                        .hint_text("https://site/np?artist={artist}&title={title}"),
                );
                ui.end_row();
            });
            ui.separator();
            ui.strong("Remote control API  (restart required)");
            ui.horizontal(|ui| {
                ui.checkbox(&mut core.cfg.remote.enabled, "Enable");
                ui.label("port");
                ui.add(egui::DragValue::new(&mut core.cfg.remote.port).range(1..=65535));
                ui.label("password");
                ui.add(egui::TextEdit::singleline(&mut core.cfg.remote.password).password(true).desired_width(120.0));
            });
            ui.label(
                RichText::new("e.g. http://localhost:9000/api/status  ·  /api/next  ·  /api/volume?v=0.8  (add &pw=… if set)")
                    .small()
                    .color(GRAY),
            );
        });
    }

    fn tab_generator(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let cat_names: Vec<String> = core.cfg.categories.iter().map(|c| c.name.clone()).collect();
        let tpl_names: Vec<String> = core.cfg.templates.iter().map(|t| t.name.clone()).collect();
        let mut gen: Option<(String, bool)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong("Auto-playlist");
            ui.horizontal(|ui| {
                ui.checkbox(&mut core.cfg.auto_enabled, "Keep the playlist filled automatically");
                ui.label("using");
                name_combo(ui, "autotpl", &mut core.cfg.auto_template, &tpl_names);
                ui.label("when under");
                ui.add(egui::DragValue::new(&mut core.cfg.auto_min_mins).range(1..=600).suffix(" min left"));
            });
            ui.separator();
            ui.strong("Categories  (what each pool of tracks is made from)");
            let mut del = None;
            for i in 0..core.cfg.categories.len() {
                let c = &mut core.cfg.categories[i];
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(100.0));
                    ui.label("folder");
                    ui.add(egui::TextEdit::singleline(&mut c.folder).desired_width(260.0).hint_text("any"));
                    if ui.button("…").clicked() {
                        self.dlg(move || rfd::FileDialog::new().pick_folder().map(|p| Dlg::CatFolder(i, p)));
                    }
                    ui.label("genre");
                    ui.add(egui::TextEdit::singleline(&mut c.genre).desired_width(90.0).hint_text("any"));
                    ui.label("length");
                    ui.add(egui::DragValue::new(&mut c.min_secs).range(0.0..=3600.0).suffix("s"));
                    ui.label("–");
                    ui.add(egui::DragValue::new(&mut c.max_secs).range(0.0..=7200.0).suffix("s"));
                    if ui.small_button("🗑").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                core.cfg.categories.remove(i);
            }
            if ui.button("➕ Add category").clicked() {
                core.cfg.categories.push(Category { name: format!("Category {}", core.cfg.categories.len() + 1), ..Default::default() });
            }
            ui.separator();
            ui.strong("Templates");
            ui.horizontal(|ui| {
                for (i, n) in tpl_names.iter().enumerate() {
                    ui.selectable_value(&mut self.tpl_sel, i, n);
                }
                if ui.button("➕ New").clicked() {
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
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(140.0));
                    ui.label("Length");
                    ui.add(egui::DragValue::new(&mut t.minutes).range(5..=1440).suffix(" min"));
                    ui.label("Artist separation");
                    ui.add(egui::DragValue::new(&mut t.artist_gap).range(0..=50).suffix(" tracks"));
                });
                ui.label(RichText::new("Pattern (repeats until the length is reached):").color(GRAY));
                let mut pdel = None;
                for (k, p) in t.pattern.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}.", k + 1));
                        ui.add(egui::DragValue::new(&mut p.count).range(1..=20).suffix(" ×"));
                        name_combo(ui, ("pat", k), &mut p.category, &cat_names);
                        if ui.small_button("✖").clicked() {
                            pdel = Some(k);
                        }
                    });
                }
                if let Some(k) = pdel {
                    t.pattern.remove(k);
                }
                if ui.button("➕ Add pattern step").clicked() {
                    t.pattern.push(PatternItem::default());
                }
                ui.label(RichText::new("Markers (jingles / ads / hour markers inserted at intervals):").color(GRAY));
                let mut mdel = None;
                for (k, m) in t.markers.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label("every");
                        ui.add(egui::DragValue::new(&mut m.every_mins).range(1..=600).suffix(" min"));
                        ui.label("insert from");
                        name_combo(ui, ("mk", k), &mut m.category, &cat_names);
                        if ui.small_button("✖").clicked() {
                            mdel = Some(k);
                        }
                    });
                }
                if let Some(k) = mdel {
                    t.markers.remove(k);
                }
                if ui.button("➕ Add marker").clicked() {
                    t.markers.push(Marker::default());
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("▶ Generate (replace playlist)").color(GREEN)).clicked() {
                        gen = Some((t.name.clone(), false));
                    }
                    if ui.button("Append to playlist").clicked() {
                        gen = Some((t.name.clone(), true));
                    }
                    if ui.button("🗑 Delete template").clicked() {
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

    fn tab_ads(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong("Commercials");
            let mut del = None;
            for i in 0..core.cfg.ads.len() {
                let a: &mut Ad = &mut core.cfg.ads[i];
                ui.horizontal(|ui| {
                    ui.checkbox(&mut a.enabled, "");
                    ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(110.0));
                    ui.add(egui::TextEdit::singleline(&mut a.path).desired_width(280.0).hint_text("audio file"));
                    if ui.button("…").clicked() {
                        self.dlg(move || {
                            rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_file().map(|p| Dlg::AdPath(i, p))
                        });
                    }
                    ui.label("from");
                    ui.add(egui::TextEdit::singleline(&mut a.start_date).desired_width(80.0).hint_text("YYYY-MM-DD"));
                    ui.label("to");
                    ui.add(egui::TextEdit::singleline(&mut a.end_date).desired_width(80.0).hint_text("YYYY-MM-DD"));
                    for (d, l) in a.days.iter_mut().zip(["M", "T", "W", "T", "F", "S", "S"]) {
                        ui.checkbox(d, l);
                    }
                    if ui.small_button("🗑").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                core.cfg.ads.remove(i);
            }
            if ui.button("➕ Add commercial").clicked() {
                core.cfg.ads.push(Ad::default());
            }
            ui.separator();
            ui.strong("Commercial breaks  (queues the active ads right after the current track)");
            let mut del = None;
            for i in 0..core.cfg.ad_breaks.len() {
                let b: &mut AdBreak = &mut core.cfg.ad_breaks[i];
                ui.horizontal(|ui| {
                    ui.checkbox(&mut b.enabled, "");
                    ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(110.0));
                    ui.add(egui::DragValue::new(&mut b.hour).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                    ui.label(":");
                    ui.add(egui::DragValue::new(&mut b.minute).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                    for (d, l) in b.days.iter_mut().zip(["M", "T", "W", "T", "F", "S", "S"]) {
                        ui.checkbox(d, l);
                    }
                    ui.label("max spots");
                    ui.add(egui::DragValue::new(&mut b.max_ads).range(0..=50));
                    ui.label(RichText::new("(0 = all)").small().color(GRAY));
                    if ui.small_button("🗑").clicked() {
                        del = Some(i);
                    }
                });
            }
            if let Some(i) = del {
                core.cfg.ad_breaks.remove(i);
            }
            if ui.button("➕ Add break").clicked() {
                core.cfg.ad_breaks.push(AdBreak::default());
            }
        });
    }

    fn tab_dsp(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let d = &mut core.cfg.dsp;
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.eq_enabled, "12-band equalizer");
                if ui.button("Reset").clicked() {
                    d.eq_gains = [0.0; 12];
                }
            });
            ui.horizontal(|ui| {
                for i in 0..12 {
                    ui.vertical(|ui| {
                        ui.add(egui::Slider::new(&mut d.eq_gains[i], -12.0..=12.0).vertical().show_value(false));
                        let f = EQ_FREQS[i];
                        ui.label(RichText::new(if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") }).small());
                    });
                }
            });
            ui.separator();
            ui.checkbox(&mut d.comp_enabled, "Compressor");
            egui::Grid::new("comp").num_columns(4).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("Threshold");
                ui.add(egui::Slider::new(&mut d.comp_threshold_db, -60.0..=0.0).suffix(" dB"));
                ui.label("Ratio");
                ui.add(egui::Slider::new(&mut d.comp_ratio, 1.0..=20.0).suffix(" :1"));
                ui.end_row();
                ui.label("Attack");
                ui.add(egui::Slider::new(&mut d.comp_attack_ms, 1.0..=200.0).suffix(" ms"));
                ui.label("Release");
                ui.add(egui::Slider::new(&mut d.comp_release_ms, 20.0..=2000.0).suffix(" ms"));
                ui.end_row();
                ui.label("Make-up gain");
                ui.add(egui::Slider::new(&mut d.comp_makeup_db, 0.0..=18.0).suffix(" dB"));
                ui.end_row();
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.agc_enabled, "Automatic volume leveling");
                ui.label("target");
                ui.add(egui::Slider::new(&mut d.agc_target_db, -30.0..=-6.0).suffix(" dB"));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.silence_trim, "Trim silence");
                ui.label("below");
                ui.add(egui::Slider::new(&mut d.silence_thresh_db, -80.0..=-20.0).suffix(" dB"));
                ui.label("cut tail after");
                ui.add(egui::Slider::new(&mut d.silence_max_secs, 0.2..=5.0).suffix(" s"));
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Teaser mode — play only the first");
                ui.add(egui::DragValue::new(&mut core.cfg.teaser_secs).range(0.0..=120.0).suffix(" s"));
                ui.label(RichText::new("of each track (0 = off)").color(GRAY));
            });
        });
    }

    fn tab_reports(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.label("From");
            ui.add(egui::TextEdit::singleline(&mut self.report_from).desired_width(90.0));
            ui.label("To");
            ui.add(egui::TextEdit::singleline(&mut self.report_to).desired_width(90.0));
            ui.label("Exclude containing");
            ui.add(egui::TextEdit::singleline(&mut self.report_excl).desired_width(180.0).hint_text("jingle, ad, …"));
            if ui.button("Generate").clicked() {
                let text = std::fs::read_to_string(data_dir().join("history.csv")).unwrap_or_default();
                let excl: Vec<String> = self.report_excl.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
                let mut map: BTreeMap<String, (u32, String)> = BTreeMap::new();
                for line in text.lines() {
                    let line = line.trim().trim_start_matches('"').trim_end_matches('"');
                    let Some((t, x)) = line.split_once("\",\"") else { continue };
                    let d = &t[..t.len().min(10)];
                    if d < self.report_from.as_str() || d > self.report_to.as_str() {
                        continue;
                    }
                    let lx = x.to_lowercase();
                    if excl.iter().any(|e| lx.contains(e)) {
                        continue;
                    }
                    let e = map.entry(x.to_string()).or_insert((0, String::new()));
                    e.0 += 1;
                    e.1 = t.to_string();
                }
                let mut rows: Vec<(String, u32, String)> = map.into_iter().map(|(k, (n, l))| (k, n, l)).collect();
                rows.sort_by(|a, b| b.1.cmp(&a.1));
                self.report_rows = rows;
            }
            if ui.button("Export CSV…").clicked() {
                self.dlg(|| {
                    rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("report.csv").save_file().map(Dlg::SaveReport)
                });
            }
            let total: u32 = self.report_rows.iter().map(|r| r.1).sum();
            ui.label(RichText::new(format!("{} tracks · {} plays", self.report_rows.len(), total)).color(GRAY));
        });
        let _ = core;
        let rows = &self.report_rows;
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::remainder().at_least(200.0))
            .column(Column::exact(60.0))
            .column(Column::exact(150.0))
            .header(20.0, |mut h| {
                for t in ["Track", "Plays", "Last played"] {
                    h.col(|ui| {
                        ui.strong(t);
                    });
                }
            })
            .body(|body| {
                body.rows(18.0, rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.col(|ui| {
                        ui.add(egui::Label::new(&r.0).truncate());
                    });
                    row.col(|ui| {
                        ui.label(r.1.to_string());
                    });
                    row.col(|ui| {
                        ui.label(&r.2);
                    });
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
                        ui.label("Checking GitHub for a newer version…");
                    });
                }
                (None, Phase::UpToDate) => {
                    ui.label(RichText::new("✔ RadioMaster is up to date.").color(GREEN).strong());
                }
                (None, Phase::Failed(e)) => {
                    ui.colored_label(RED, format!("Couldn't check for updates: {e}"));
                }
                (None, _) => {
                    ui.label("No update information yet.");
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
                            ui.label("Downloaded. RadioMaster will close, update and start again.");
                            if ui.button(RichText::new("Install now").strong()).clicked() {
                                install = Some(p.clone());
                            }
                        }
                        other => {
                            if let Phase::Failed(e) = other {
                                ui.colored_label(RED, e);
                            }
                            ui.horizontal(|ui| {
                                if installed && a.installer.is_some() {
                                    if ui.button(RichText::new("⬇ Download and install").strong()).clicked() {
                                        start_download = true;
                                    }
                                } else if ui.button(RichText::new("⬇ Open the download page").strong()).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(a.page.clone()));
                                }
                                if ui.button("Release page").clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(a.page.clone()));
                                }
                            });
                            if !installed {
                                ui.label(RichText::new("This copy wasn't installed with the setup program, so download the new zip (or the installer) from the page.").small().color(GRAY));
                            }
                        }
                    }
                    if core.state == PlayState::Playing {
                        ui.label(RichText::new("Playback stops while the update installs — best done between shows.").small().color(AMBER));
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
                    ui.label("Artist");
                    ui.add(egui::TextEdit::singleline(&mut t.artist).desired_width(220.0));
                    ui.end_row();
                    ui.label("Title");
                    ui.add(egui::TextEdit::singleline(&mut t.title).desired_width(220.0));
                    ui.end_row();
                    ui.label("Album");
                    ui.add(egui::TextEdit::singleline(&mut t.album).desired_width(220.0));
                    ui.end_row();
                    ui.label("Genre");
                    ui.add(egui::TextEdit::singleline(&mut t.genre).desired_width(220.0));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    save = ui.button("Save").clicked();
                    close = ui.button("Cancel").clicked();
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
        ctx.request_repaint_after(Duration::from_millis(100));
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
            return;
        }
        let t0 = std::time::Instant::now();
        let arc = self.core.clone();
        let mut core = arc.lock();
        let lock_ms = t0.elapsed().as_secs_f32() * 1000.0;
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::P)) {
            self.perf = if self.perf.is_some() { None } else { Some(Perf::default()) };
        }
        self.handle_dialogs(&mut core);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            let rows: Vec<PlEntry> = dropped
                .iter()
                .flat_map(|p| if p.is_dir() { crate::library::audio_files_in(p) } else { vec![p.clone()] })
                .map(|p| core.entry_for(&p))
                .collect();
            self.insert_rows_here(&mut core, rows);
        }
        if !self.show_settings {
            self.shortcuts(ctx, &mut core);
        }
        self.menu(ctx, &mut core);
        self.status_bar(ctx, &core);
        self.player(ctx, &mut core);
        self.right_workspace(ctx, &mut core);
        self.left_workspace(ctx, &mut core);
        self.playlist(ctx, &mut core);
        self.aux_windows(ctx, &mut core);
        core.engine.set_mic(self.mic_on, core.cfg.mic_gain, core.cfg.mic_duck);
        self.tag_window(ctx, &mut core);
        self.profiles_window(ctx, &mut core);
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
        if core.cfg.check_updates && self.update_next.is_none_or(|t| std::time::Instant::now() >= t) {
            // at start-up, then every 6 hours for stations that run for weeks
            self.update_next = Some(std::time::Instant::now() + Duration::from_secs(6 * 3600));
            crate::updater::check(self.updates.clone(), ctx.clone(), false);
        }
        self.update_window(ctx, &mut core);
        self.apply_tab_ops(&mut core);
        // A native web view paints above everything egui draws, so hide the main window's views
        // whenever an egui window, menu, combo list or popup overlaps them.
        let force =
            self.show_settings || self.show_users || self.show_profiles || self.show_about || self.show_update || self.tag_edit.is_some();
        self.guard_web_views(ctx, "", force);
        self.browsers.finish();
        // save the layout when it changed, at most every 2 seconds (never while a window is being dragged)
        if self.layout_checked.elapsed() >= Duration::from_secs(2) && !ctx.input(|i| i.pointer.any_down()) {
            self.layout_checked = std::time::Instant::now();
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
        let mut c = self.core.lock();
        for i in 0..c.cfg.encoders.len() {
            c.stop_encoder(i);
        }
        c.save_all();
        drop(c);
        crate::playout::flush_writes();
    }
}
