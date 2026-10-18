//! AzuraCast station manager (Tools → AzuraCast). Optional: everything except the public
//! now-playing view needs an API key with permissions for the station.
//!
//! All requests run on background threads; replies are parsed loosely (serde_json::Value) so
//! the tool keeps working across AzuraCast versions.

use super::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum AzPage {
    Overview,
    Queue,
    Requests,
    Playlists,
    Media,
    Streamers,
    Listeners,
    History,
    Schedule,
    Connection,
}

impl AzPage {
    const ALL: [AzPage; 10] = [
        AzPage::Overview,
        AzPage::Queue,
        AzPage::Requests,
        AzPage::Playlists,
        AzPage::Media,
        AzPage::Streamers,
        AzPage::Listeners,
        AzPage::History,
        AzPage::Schedule,
        AzPage::Connection,
    ];
    fn label(self) -> &'static str {
        match self {
            AzPage::Overview => "📻 Overview",
            AzPage::Queue => "⏭ Queue",
            AzPage::Requests => "🙋 Requests",
            AzPage::Playlists => "📃 Playlists",
            AzPage::Media => "🗁 Media",
            AzPage::Streamers => "🎤 Streamers/DJs",
            AzPage::Listeners => "👥 Listeners",
            AzPage::History => "📜 History",
            AzPage::Schedule => "📅 Schedule",
            AzPage::Connection => "🔑 Connection",
        }
    }
}

#[derive(Default, Clone)]
pub(super) struct DjForm {
    id: Option<i64>,
    username: String,
    password: String,
    display_name: String,
    comments: String,
    is_active: bool,
    enforce_schedule: bool,
}

pub(super) struct AzTool {
    pub page: AzPage,
    data: HashMap<String, Result<Value, String>>,
    fetched: HashMap<String, Instant>,
    busy: HashMap<String, Instant>,
    tx: mpsc::Sender<(String, Result<Value, String>)>,
    rx: mpsc::Receiver<(String, Result<Value, String>)>,
    pub msg: String,
    media_dir: String,
    media_filter: String,
    req_filter: String,
    history_date: String,
    dj: Option<DjForm>,
    confirm: Option<(String, String, String)>, // (question, method, path)
    new_folder: String,
    /// finished media downloads: (local file, insert after the selection?) or an error
    dl_tx: mpsc::Sender<Result<(PathBuf, bool), String>>,
    dl_rx: mpsc::Receiver<Result<(PathBuf, bool), String>>,
    pub downloading: usize,
}

impl Default for AzTool {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let (dl_tx, dl_rx) = mpsc::channel();
        AzTool {
            dl_tx,
            dl_rx,
            downloading: 0,
            page: AzPage::Overview,
            data: HashMap::new(),
            fetched: HashMap::new(),
            busy: HashMap::new(),
            tx,
            rx,
            msg: String::new(),
            media_dir: String::new(),
            media_filter: String::new(),
            req_filter: String::new(),
            history_date: Local::now().format("%Y-%m-%d").to_string(),
            dj: None,
            confirm: None,
            new_folder: String::new(),
        }
    }
}

// --------------------------------------------------------------- JSON helpers
/// String at a dotted path ("song.text"), numbers and booleans included.
pub(super) fn vs(v: &Value, path: &str) -> String {
    let mut cur = v;
    for k in path.split('.') {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return String::new(),
        }
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}
fn vf(v: &Value, path: &str) -> f64 {
    vs(v, path).parse().unwrap_or(0.0)
}
fn vb(v: &Value, path: &str) -> bool {
    matches!(vs(v, path).as_str(), "true" | "1")
}
/// First non-empty of several paths.
fn first(v: &Value, paths: &[&str]) -> String {
    paths.iter().map(|p| vs(v, p)).find(|s| !s.is_empty()).unwrap_or_default()
}
/// The list in a reply: a plain array, or a paginated object's rows.
pub(super) fn rows(v: &Value) -> Vec<Value> {
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    for k in ["rows", "items", "data", "results"] {
        if let Some(a) = v.get(k).and_then(|x| x.as_array()) {
            return a.clone();
        }
    }
    vec![]
}
fn song_text(v: &Value) -> String {
    let t = first(v, &["song.text", "track.text", "media.text", "text"]);
    if !t.is_empty() {
        return t;
    }
    let (a, ti) = (
        first(v, &["song.artist", "track.artist", "media.artist", "artist"]),
        first(v, &["song.title", "track.title", "media.title", "title"]),
    );
    match (a.is_empty(), ti.is_empty()) {
        (false, false) => format!("{a} - {ti}"),
        (true, false) => ti,
        _ => a,
    }
}
fn when(ts: f64) -> String {
    if ts <= 0.0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(ts as i64, 0).map(|d| d.with_timezone(&Local).format("%d/%m %H:%M:%S").to_string()).unwrap_or_default()
}
/// A time that may be a unix timestamp or an ISO string.
fn when_any(v: &Value, paths: &[&str]) -> String {
    for p in paths {
        let s = vs(v, p);
        if s.is_empty() {
            continue;
        }
        if let Ok(n) = s.parse::<f64>() {
            return when(n);
        }
        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(&s) {
            return d.with_timezone(&Local).format("%d/%m %H:%M").to_string();
        }
        return s;
    }
    String::new()
}
fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => o.push(b as char),
            _ => o += &format!("%{b:02X}"),
        }
    }
    o
}

impl AzTool {
    /// Run a request in the background. `key` names the reply; keys starting with '!' are
    /// actions whose reply message is shown and which refresh `then` afterwards.
    fn run(&mut self, cfg: &crate::config::AzuraConfig, ctx: &egui::Context, key: &str, method: &str, path: String, body: Option<Value>) {
        if self.busy.get(key).is_some_and(|t| t.elapsed() < Duration::from_secs(25)) {
            return;
        }
        self.busy.insert(key.to_string(), Instant::now());
        let (tx, c, k, m, ctx) = (self.tx.clone(), cfg.clone(), key.to_string(), method.to_string(), ctx.clone());
        thread::spawn(move || {
            let r = Client::new(&c).call(&m, &path, body).map_err(|e| e.to_string());
            let _ = tx.send((k, r));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) -> Vec<String> {
        let mut done = vec![];
        while let Ok((k, r)) = self.rx.try_recv() {
            self.busy.remove(&k);
            if let Some(action) = k.strip_prefix('!') {
                self.msg = match &r {
                    Ok(v) => {
                        let m = first(v, &["formatted_message", "message"]);
                        if m.is_empty() {
                            format!("{action}: done")
                        } else {
                            m
                        }
                    }
                    Err(e) => format!("{action} failed: {e}"),
                };
                done.push(action.split('|').nth(1).unwrap_or("").to_string());
            } else {
                self.fetched.insert(k.clone(), Instant::now());
                self.data.insert(k, r);
            }
        }
        done
    }

    fn get(&self, key: &str) -> Option<&Result<Value, String>> {
        self.data.get(key)
    }

    fn stale(&self, key: &str, max: u64) -> bool {
        self.fetched.get(key).is_none_or(|t| t.elapsed() > Duration::from_secs(max))
    }

    fn loading(&self, key: &str) -> bool {
        self.busy.contains_key(key)
    }
}

impl App {
    fn az_fetch(&mut self, core: &Playout, key: &str) {
        let path = match key {
            "status" => "/station/{st}/status".to_string(),
            "queue" => "/station/{st}/queue".into(),
            "requests" => "/station/{st}/reports/requests".into(),
            "requestable" => "/station/{st}/requests".into(),
            "playlists" => "/station/{st}/playlists".into(),
            "files" => format!("/station/{{st}}/files/list?currentDirectory={}", enc(&self.az.media_dir)),
            "streamers" => "/station/{st}/streamers".into(),
            "listeners" => "/station/{st}/listeners".into(),
            "history" => format!("/station/{{st}}/history?start={0}&end={0}", self.az.history_date),
            "schedule" => "/station/{st}/schedule".into(),
            "stations" => "/stations".into(),
            _ => return,
        };
        let ctx = self.ctx.clone();
        self.az.run(&core.cfg.azuracast, &ctx, key, "GET", path, None);
    }

    /// `label|refresh-key`
    fn az_action(&mut self, core: &Playout, label: &str, refresh: &str, method: &str, path: String, body: Option<Value>) {
        let ctx = self.ctx.clone();
        self.az.run(&core.cfg.azuracast, &ctx, &format!("!{label}|{refresh}"), method, path, body);
    }

    fn az_auto(&mut self, core: &Playout, key: &str, secs: u64) {
        if self.az.stale(key, secs) && !self.az.loading(key) {
            self.az_fetch(core, key);
        }
    }

    fn az_poll(&mut self, core: &Playout) {
        for refresh in self.az.poll() {
            if !refresh.is_empty() {
                self.az_fetch(core, &refresh);
            }
        }
    }

    /// Header line for a page: refresh button + state.
    fn az_head(&mut self, ui: &mut egui::Ui, core: &Playout, key: &str, title: &str) -> bool {
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.strong(title);
            if ui.small_button(tr("⟳ Refresh")).clicked() {
                refresh = true;
            }
            if self.az.loading(key) {
                ui.spinner();
            } else if let Some(t) = self.az.fetched.get(key) {
                ui.label(RichText::new(format!("updated {}s ago", t.elapsed().as_secs())).small().color(GRAY));
            }
        });
        if refresh {
            self.az_fetch(core, key);
        }
        if let Some(Err(e)) = self.az.get(key) {
            ui.colored_label(RED, e);
        }
        refresh
    }

    fn az_needs_key(ui: &mut egui::Ui, core: &Playout) -> bool {
        if core.cfg.azuracast.api_key.trim().is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("This page needs an AzuraCast API key.")).strong());
                ui.label(
                    RichText::new(tr(
                        "In AzuraCast: your account (top right) → API Keys → Add API key. Paste it on the 🔑 Connection page.",
                    ))
                    .small()
                    .color(GRAY),
                );
            });
            return true;
        }
        false
    }

    /// The station manager window body.
    pub(super) fn az_manager(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        self.az_poll(core);
        let configured = !core.cfg.azuracast.base_url.trim().is_empty() && !core.cfg.azuracast.station.trim().is_empty();
        if !configured && self.az.page != AzPage::Connection {
            self.az.page = AzPage::Connection;
        }
        egui::SidePanel::left("az_nav").resizable(false).exact_width(175.0).show_inside(ui, |ui| {
            let np = core.azura.lock().np.clone();
            if let Some(n) = &np {
                ui.label(RichText::new(&n.station.name).strong());
                ui.label(if n.is_online {
                    RichText::new(tr("● online")).color(GREEN).small()
                } else {
                    RichText::new(tr("● offline")).color(RED).small()
                });
                ui.label(RichText::new(format!("👥 {}", n.listeners.current)).small());
            } else {
                ui.label(RichText::new(tr("AzuraCast")).strong());
                ui.label(RichText::new(if core.cfg.azuracast.enabled { "connecting…" } else { "not connected" }).small().color(GRAY));
            }
            ui.separator();
            for p in AzPage::ALL {
                let enabled = configured || p == AzPage::Connection;
                if ui.add_enabled(enabled, egui::SelectableLabel::new(self.az.page == p, tr(p.label()))).clicked() {
                    self.az.page = p;
                }
            }
        });
        egui::TopBottomPanel::bottom("az_msg").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(if self.az.msg.is_empty() { " " } else { &self.az.msg }).small());
            });
        });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("az_page").auto_shrink([false, false]).show(ui, |ui| match self.az.page {
                AzPage::Overview => self.az_overview(ui, core),
                AzPage::Queue => self.az_queue(ui, core),
                AzPage::Requests => self.az_requests_panel(ui, core),
                AzPage::Playlists => self.az_playlists(ui, core),
                AzPage::Media => self.az_media(ui, core),
                AzPage::Streamers => self.az_streamers(ui, core),
                AzPage::Listeners => self.az_listeners(ui, core),
                AzPage::History => self.az_history(ui, core),
                AzPage::Schedule => self.az_schedule(ui, core),
                AzPage::Connection => self.az_connection(ui, core),
            });
        });
        self.az_confirm(ui.ctx(), core);
        self.az_dj_form(ui.ctx(), core);
    }

    fn az_confirm(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some((q, method, path)) = self.az.confirm.clone() else { return };
        let mut answer = None;
        egui::Window::new("Confirm")
            .id(egui::Id::new("az_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(&q);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(tr("Yes")).strong()).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(tr("No")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        match answer {
            Some(true) => {
                self.az.confirm = None;
                let refresh = if path.contains("/streamer") {
                    "streamers"
                } else if path.contains("/files") {
                    "files"
                } else if path.contains("/reports/requests") {
                    "requests"
                } else if path.contains("/queue") {
                    "queue"
                } else {
                    "status"
                };
                let body = if path.contains("/files/batch") {
                    let (kind, p) = method.split_at(2);
                    Some(if kind == "D:" {
                        json!({"do": "delete", "files": [], "dirs": [p]})
                    } else {
                        json!({"do": "delete", "files": [p], "dirs": []})
                    })
                } else {
                    None
                };
                let m = if body.is_some() { "PUT" } else { "DELETE" };
                self.az_action(core, "Delete", refresh, m, path, body);
            }
            Some(false) => self.az.confirm = None,
            None => {}
        }
    }

    // ------------------------------------------------------------- overview
    fn az_overview(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let np = core.azura.lock().np.clone();
        let err = core.azura.lock().error.clone();
        if !core.cfg.azuracast.enabled {
            ui.horizontal(|ui| {
                ui.label(tr("Now-playing polling is off."));
                if ui.button(tr("Turn on")).clicked() {
                    core.cfg.azuracast.enabled = true;
                }
            });
        }
        if let Some(e) = err {
            ui.colored_label(RED, format!("Now playing: {e}"));
        }
        match &np {
            Some(n) => {
                ui.heading(&n.station.name);
                ui.horizontal_wrapped(|ui| {
                    ui.label(if n.is_online {
                        RichText::new(tr("● On air")).color(GREEN).strong()
                    } else {
                        RichText::new(tr("● Offline")).color(RED).strong()
                    });
                    ui.separator();
                    ui.label(format!("👥 {} listening ({} unique)", n.listeners.current, n.listeners.unique));
                    if n.live.is_live {
                        ui.separator();
                        ui.label(RichText::new(format!("🎤 LIVE: {}", n.live.streamer_name)).color(AMBER).strong());
                    }
                });
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(tr("NOW PLAYING")).small().color(GRAY));
                    ui.label(
                        RichText::new(if n.now_playing.song.text.is_empty() {
                            format!("{} - {}", n.now_playing.song.artist, n.now_playing.song.title)
                        } else {
                            n.now_playing.song.text.clone()
                        })
                        .size(17.0)
                        .strong(),
                    );
                    let (el, du) = (n.now_playing.elapsed as f32, n.now_playing.duration as f32);
                    if du > 0.0 {
                        ui.add(egui::ProgressBar::new((el / du).clamp(0.0, 1.0)).text(format!("{} / {}", fmt(el), fmt(du))));
                    }
                    if let Some(p) = &n.playing_next {
                        ui.label(
                            RichText::new(format!(
                                "Next: {}",
                                if p.song.text.is_empty() { format!("{} - {}", p.song.artist, p.song.title) } else { p.song.text.clone() }
                            ))
                            .color(GRAY),
                        );
                    }
                });
                if !n.station.listen_url.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr("Listen:")).small().color(GRAY));
                        ui.hyperlink(&n.station.listen_url);
                    });
                }
            }
            None => {
                ui.label(RichText::new(tr("No now-playing data yet.")).color(GRAY));
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.strong(tr("Station control"));
        if core.cfg.azuracast.api_key.trim().is_empty() {
            ui.label(RichText::new(tr("Add an API key on the Connection page to skip songs, restart services, manage queue, requests, playlists, media, DJs and listeners.")).small().color(GRAY));
            return;
        }
        self.az_auto(core, "status", 20);
        if let Some(Err(e)) = self.az.get("status") {
            ui.colored_label(RED, e);
        }
        let status = match self.az.get("status") {
            Some(Ok(v)) => {
                Some((vb(v, "backendRunning") || vb(v, "backend_running"), vb(v, "frontendRunning") || vb(v, "frontend_running")))
            }
            _ => None,
        };
        egui::Grid::new("az_services").num_columns(3).spacing([10.0, 8.0]).show(ui, |ui| {
            for (name, part, running) in
                [("AutoDJ (Liquidsoap)", "backend", status.map(|s| s.0)), ("Broadcast server", "frontend", status.map(|s| s.1))]
            {
                ui.label(name);
                ui.label(match running {
                    Some(true) => RichText::new(tr("● running")).color(GREEN),
                    Some(false) => RichText::new(tr("● stopped")).color(RED),
                    None => RichText::new("…").color(GRAY),
                });
                ui.horizontal(|ui| {
                    for (label, act) in [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")] {
                        if ui.small_button(label).clicked() {
                            self.az_action(
                                core,
                                &format!("{label} {part}"),
                                "status",
                                "POST",
                                format!("/station/{{st}}/{part}/{act}"),
                                None,
                            );
                        }
                    }
                });
                ui.end_row();
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("⏭ Skip song")).clicked() {
                self.az_action(core, "Skip", "queue", "POST", "/station/{st}/backend/skip".into(), None);
            }
            if ui.button(tr("🔌 Disconnect live DJ")).clicked() {
                self.az_action(core, "Disconnect streamer", "status", "POST", "/station/{st}/backend/disconnect".into(), None);
            }
            if ui
                .button("♻ Restart broadcasting")
                .on_hover_text(tr("Applies configuration changes; listeners are briefly disconnected"))
                .clicked()
            {
                self.az_action(core, "Restart", "status", "POST", "/station/{st}/restart".into(), None);
            }
        });
    }

    // ---------------------------------------------------------------- queue
    fn az_queue(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "queue", 20);
        self.az_head(ui, core, "queue", "Upcoming song queue (AutoDJ)");
        let items = match self.az.get("queue") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        if items.is_empty() && !self.az.loading("queue") {
            ui.label(RichText::new(tr("The queue is empty.")).color(GRAY));
        }
        let mut del = None;
        egui::Grid::new("az_queue").striped(true).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.strong(tr("Cued"));
            ui.strong(tr("Song"));
            ui.strong(tr("From"));
            ui.label("");
            ui.end_row();
            for it in &items {
                ui.label(when_any(it, &["cued_at", "timestamp_cued"]));
                ui.label(song_text(it));
                let from = if vb(it, "is_request") { "Request".to_string() } else { first(it, &["playlist", "playlist.name"]) };
                ui.label(RichText::new(from).small().color(GRAY));
                if ui.small_button("🗑").on_hover_text(tr("Remove from queue")).clicked() {
                    del = Some(vs(it, "id"));
                }
                ui.end_row();
            }
        });
        if let Some(id) = del {
            self.az_action(core, "Remove from queue", "queue", "DELETE", format!("/station/{{st}}/queue/{id}"), None);
        }
    }

    // ------------------------------------------------------------- requests
    /// Pending listener requests (also the Song Requests panel tab), plus "request a song".
    pub(super) fn az_requests_panel(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if core.cfg.azuracast.base_url.trim().is_empty() || core.cfg.azuracast.station.trim().is_empty() {
            ui.label(RichText::new(tr("Song requests come from AzuraCast. Set it up in Tools → AzuraCast → Connection.")).color(GRAY));
            if ui.button(tr("Open AzuraCast tool")).clicked() {
                self.az.page = AzPage::Connection;
                self.open_tool(ToolWin::AzuraCast);
            }
            return;
        }
        self.az_poll(core);
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "requests", 30);
        self.az_head(ui, core, "requests", "Listener requests");
        let items = match self.az.get("requests") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        let pending: Vec<&Value> = items.iter().filter(|r| vs(r, "played_at").is_empty() || vf(r, "played_at") == 0.0).collect();
        ui.horizontal(|ui| {
            ui.label(format!("{} pending", pending.len()));
            if !pending.is_empty() && ui.small_button(tr("Clear all pending")).clicked() {
                self.az_action(core, "Clear requests", "requests", "POST", "/station/{st}/reports/requests/clear".into(), None);
            }
        });
        let mut del = None;
        let mut add_local = None;
        egui::Grid::new("az_req").striped(true).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
            for r in &pending {
                ui.label(when_any(r, &["timestamp", "submitted_at", "requested_at"]));
                let text = song_text(r);
                ui.label(&text);
                if ui.small_button("➕").on_hover_text(tr("Find this song in the local library and add it to the playlist")).clicked() {
                    add_local = Some(text.clone());
                }
                if ui.small_button("🗑").on_hover_text(tr("Delete request")).clicked() {
                    del = Some(vs(r, "id"));
                }
                ui.end_row();
            }
        });
        if let Some(id) = del {
            self.az_action(core, "Delete request", "requests", "DELETE", format!("/station/{{st}}/reports/requests/{id}"), None);
        }
        if let Some(text) = add_local {
            let needle = text.to_lowercase();
            let hit = core.library.iter().find(|t| {
                let l = t.label().to_lowercase();
                l == needle
                    || (!t.title.is_empty()
                        && needle.contains(&t.title.to_lowercase())
                        && (t.artist.is_empty() || needle.contains(&t.artist.to_lowercase())))
            });
            match hit.map(|t| t.path.clone()) {
                Some(p) => {
                    let e = core.entry_for(&p);
                    self.insert_rows_here(core, vec![e]);
                    core.say(format!("Request added: {text}"));
                }
                None => core.say(format!("\"{text}\" is not in the local library")),
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.collapsing("Request a song on the station", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.az.req_filter).hint_text(tr("search requestable songs")).desired_width(220.0));
                if ui.button(tr("Load list")).clicked() {
                    self.az_fetch(core, "requestable");
                }
                if self.az.loading("requestable") {
                    ui.spinner();
                }
            });
            if let Some(Err(e)) = self.az.get("requestable") {
                ui.colored_label(RED, e);
            }
            let list = match self.az.get("requestable") {
                Some(Ok(v)) => rows(v),
                _ => vec![],
            };
            let f = self.az.req_filter.to_lowercase();
            let mut req = None;
            for s in list.iter().filter(|s| f.is_empty() || song_text(s).to_lowercase().contains(&f)).take(100) {
                ui.horizontal(|ui| {
                    if ui.small_button(tr("Request")).clicked() {
                        req = Some((vs(s, "request_id"), song_text(s)));
                    }
                    ui.label(song_text(s));
                });
            }
            if let Some((id, t)) = req {
                self.az_action(core, &format!("Request {t}"), "requests", "POST", format!("/station/{{st}}/request/{id}"), None);
            }
        });
    }

    // ------------------------------------------------------------ playlists
    fn az_playlists(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "playlists", 60);
        self.az_head(ui, core, "playlists", "AutoDJ playlists");
        let items = match self.az.get("playlists") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        let mut act: Option<(String, String, String)> = None;
        egui::Grid::new("az_pl").striped(true).num_columns(6).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["On", "Name", "Type", "Songs", "Length", ""] {
                ui.strong(tr(h));
            }
            ui.end_row();
            for p in &items {
                let id = vs(p, "id");
                let mut on = vb(p, "is_enabled");
                if ui.checkbox(&mut on, "").on_hover_text(tr("Enable / disable")).changed() {
                    act = Some(("Toggle playlist".into(), "PUT".into(), format!("/station/{{st}}/playlist/{id}/toggle")));
                }
                ui.label(RichText::new(vs(p, "name")).strong());
                ui.label(RichText::new(format!("{} · {}", vs(p, "type"), vs(p, "source"))).small().color(GRAY));
                ui.label(vs(p, "num_songs"));
                ui.label(fmt(vf(p, "total_length") as f32));
                ui.horizontal(|ui| {
                    if ui.small_button(tr("🔀 Reshuffle")).clicked() {
                        act = Some(("Reshuffle".into(), "PUT".into(), format!("/station/{{st}}/playlist/{id}/reshuffle")));
                    }
                    if ui.small_button(tr("Queue all")).on_hover_text(tr("Queue this playlist's songs to play next")).clicked() {
                        act = Some(("Queue playlist".into(), "PUT".into(), format!("/station/{{st}}/playlist/{id}/queue")));
                    }
                });
                ui.end_row();
            }
        });
        if let Some((l, m, p)) = act {
            let refresh = if l == "Queue playlist" { "queue" } else { "playlists" };
            self.az_action(core, &l, refresh, &m, p, None);
            if refresh == "queue" {
                self.az_fetch(core, "playlists");
            }
        }
    }

    // ---------------------------------------------------------------- media
    fn az_media(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "files", 120);
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.az.media_dir.is_empty(), egui::Button::new(tr("⬆ Up"))).clicked() {
                let d = self.az.media_dir.trim_end_matches('/');
                self.az.media_dir = d.rsplit_once('/').map(|(a, _)| a.to_string()).unwrap_or_default();
                self.az_fetch(core, "files");
            }
            ui.label(RichText::new(format!("/{}", self.az.media_dir)).monospace());
        });
        self.az_head(ui, core, "files", "Station media");
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("☁ Upload files here…")).clicked() {
                self.dlg(|| rfd::FileDialog::new().add_filter("Audio", &crate::library::EXTS).pick_files().map(Dlg::AzUpload));
            }
            ui.add(egui::TextEdit::singleline(&mut self.az.new_folder).hint_text(tr("new folder name")).desired_width(130.0));
            if ui.add_enabled(!self.az.new_folder.trim().is_empty(), egui::Button::new(tr("📁 Create folder"))).clicked() {
                let body = json!({"currentDirectory": self.az.media_dir, "name": self.az.new_folder.trim()});
                self.az.new_folder.clear();
                self.az_action(core, "Create folder", "files", "POST", "/station/{st}/files/mkdir".into(), Some(body));
            }
            ui.add(egui::TextEdit::singleline(&mut self.az.media_filter).hint_text(tr("filter")).desired_width(130.0));
        });
        let items = match self.az.get("files") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        let f = self.az.media_filter.to_lowercase();
        let mut open_dir = None;
        let mut get: Option<(String, bool)> = None;
        let mut act: Option<(String, Value)> = None;
        let mut del = None;
        egui::Grid::new("az_files").striped(true).num_columns(4).spacing([10.0, 3.0]).show(ui, |ui| {
            for it in items.iter().filter(|it| f.is_empty() || first(it, &["text", "path_short", "path"]).to_lowercase().contains(&f)) {
                let path = vs(it, "path");
                let is_dir = vs(it, "type") == "directory"
                    || it.get("dir").is_some_and(|d| !d.is_null()) && it.get("media").is_none_or(|m| m.is_null());
                let name = first(it, &["path_short", "text", "path"]);
                if is_dir {
                    if ui.link(format!("🗁 {name}")).clicked() {
                        open_dir = Some(path.clone());
                    }
                    ui.label("");
                } else {
                    let t = song_text(it);
                    ui.label(format!("🎵 {}", if t.is_empty() { name.clone() } else { t }));
                    ui.label(RichText::new(first(it, &["media.length_text", "length_text"])).small().color(GRAY));
                }
                ui.label(
                    RichText::new(if is_dir { String::new() } else { format!("{:.1} MB", vf(it, "size") / 1_048_576.0) })
                        .small()
                        .color(GRAY),
                );
                let may_edit = self.perm.edit_playlists;
                ui.horizontal(|ui| {
                    if !is_dir
                        && may_edit
                        && ui
                            .small_button(tr("➕ Playlist"))
                            .on_hover_text(tr("Download it and add it to the playlist you're viewing"))
                            .clicked()
                    {
                        get = Some((path.clone(), false));
                    }
                    if !is_dir
                        && may_edit
                        && ui.small_button(tr("Insert")).on_hover_text(tr("Download it and put it after the selected row")).clicked()
                    {
                        get = Some((path.clone(), true));
                    }
                    if !is_dir
                        && ui.small_button(tr("Queue next")).on_hover_text(tr("AzuraCast plays it next on the station's AutoDJ")).clicked()
                    {
                        act = Some(("Queue".into(), json!({"do": "queue", "files": [path.clone()], "dirs": []})));
                    }
                    if ui.small_button("🗑").on_hover_text(tr("Delete from the station")).clicked() {
                        del = Some((name.clone(), format!("{}{path}", if is_dir { "D:" } else { "F:" })));
                    }
                });
                ui.end_row();
            }
        });
        if let Some((remote, after_sel)) = get {
            self.az_download(core, remote, after_sel);
        }
        if self.az.downloading > 0 {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Downloading {} file(s) from the station…", self.az.downloading));
            });
        }
        if let Some(d) = open_dir {
            self.az.media_dir = d;
            self.az.media_filter.clear();
            self.az_fetch(core, "files");
        }
        if let Some((l, body)) = act {
            self.az_action(core, &l, "queue", "PUT", "/station/{st}/files/batch".into(), Some(body));
        }
        if let Some((name, path)) = del {
            // confirm: (question, file path, endpoint)
            self.az.confirm = Some((format!("Delete \"{name}\" from the station?"), path, "/station/{st}/files/batch".into()));
        }
    }

    /// Fetch a station media file into the local cache, then add it to the playlist.
    fn az_download(&mut self, core: &Playout, remote: String, after_sel: bool) {
        let c = core.cfg.azuracast.clone();
        let safe_station: String =
            c.station.chars().map(|ch| if ch.is_alphanumeric() || ch == '_' || ch == '-' { ch } else { '_' }).collect();
        let rel: PathBuf = remote.split('/').filter(|p| !p.is_empty() && *p != "..").collect();
        let dest = data_dir().join("azuracast").join(safe_station).join(rel);
        let (tx, ctx) = (self.az.dl_tx.clone(), self.ctx.clone());
        self.az.downloading += 1;
        thread::spawn(move || {
            // already downloaded earlier: use the cached copy
            let r = if dest.exists() { Ok(()) } else { Client::new(&c).download(&remote, &dest).map_err(|e| format!("{remote}: {e}")) };
            let _ = tx.send(r.map(|_| (dest, after_sel)));
            ctx.request_repaint();
        });
    }

    /// Add finished downloads to the playlist being viewed (called every frame).
    pub(super) fn az_take_downloads(&mut self, core: &mut Playout) {
        while let Ok(r) = self.az.dl_rx.try_recv() {
            self.az.downloading = self.az.downloading.saturating_sub(1);
            match r {
                Ok((_, _)) if !self.perm.edit_playlists => self.az.msg = tr("Your account isn't allowed to edit playlists").into(),
                Ok((p, after_sel)) => {
                    let e = core.entry_for(&p);
                    let title = e.title();
                    if after_sel {
                        self.insert_rows_here(core, vec![e]);
                    } else {
                        let v = self.view_pl;
                        core.with_tab(v, |c| c.insert_rows(usize::MAX, vec![e]));
                    }
                    self.az.msg = format!("Added {title}");
                }
                Err(e) => self.az.msg = format!("Download failed: {e}"),
            }
        }
    }

    /// Upload local files into the media folder being viewed.
    pub(super) fn az_upload_here(&mut self, core: &Playout, paths: Vec<PathBuf>) {
        let (mut c, tx, ctx, dir) = (core.cfg.azuracast.clone(), self.az.tx.clone(), self.ctx.clone(), self.az.media_dir.clone());
        c.upload_dir = dir.clone();
        self.az.msg = format!("Uploading {} file(s)…", paths.len());
        thread::spawn(move || {
            let cl = Client::new(&c);
            let n = paths.len();
            let mut res = Ok(Value::String(format!("Uploaded {n} file(s) to /{dir}")));
            for p in &paths {
                if let Err(e) = cl.upload(p, &dir) {
                    res = Err(format!("{}: {e}", p.display()));
                    break;
                }
            }
            let _ = tx.send(("!Upload|files".into(), res.map(|v| json!({"message": v}))));
            ctx.request_repaint();
        });
    }

    // ------------------------------------------------------------ streamers
    fn az_streamers(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "streamers", 120);
        self.az_head(ui, core, "streamers", "Streamer / DJ accounts");
        if ui.button(tr("➕ Add streamer")).clicked() {
            self.az.dj = Some(DjForm { is_active: true, ..Default::default() });
        }
        let items = match self.az.get("streamers") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        let mut edit = None;
        let mut del = None;
        let mut use_for = None;
        egui::Grid::new("az_dj").striped(true).num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Username", "Display name", "Active", ""] {
                ui.strong(tr(h));
            }
            ui.end_row();
            for d in &items {
                ui.label(vs(d, "streamer_username"));
                ui.label(vs(d, "display_name"));
                ui.label(if vb(d, "is_active") { RichText::new(tr("yes")).color(GREEN) } else { RichText::new(tr("no")).color(GRAY) });
                ui.horizontal(|ui| {
                    if ui.small_button(tr("✏ Edit")).clicked() {
                        edit = Some(d.clone());
                    }
                    if ui
                        .small_button("Use in encoder")
                        .on_hover_text(tr("Fill encoder 1 with this DJ's login (type the password there)"))
                        .clicked()
                    {
                        use_for = Some(vs(d, "streamer_username"));
                    }
                    if ui.small_button("🗑").clicked() {
                        del = Some((vs(d, "streamer_username"), vs(d, "id")));
                    }
                });
                ui.end_row();
            }
        });
        if let Some(d) = edit {
            self.az.dj = Some(DjForm {
                id: vs(&d, "id").parse().ok(),
                username: vs(&d, "streamer_username"),
                password: String::new(),
                display_name: vs(&d, "display_name"),
                comments: vs(&d, "comments"),
                is_active: vb(&d, "is_active"),
                enforce_schedule: vb(&d, "enforce_schedule"),
            });
        }
        if let Some((name, id)) = del {
            self.az.confirm = Some((format!("Delete streamer \"{name}\"?"), String::new(), format!("/station/{{st}}/streamer/{id}")));
        }
        if let Some(user) = use_for {
            let host = host_of(&core.cfg.azuracast.base_url);
            if let Some(e) = core.cfg.encoders.first_mut() {
                e.host = host;
                e.username = user;
                e.mount = "/".into();
                e.kind = ServerKind::Icecast;
                core.say("Encoder 1 set up for the DJ — enter the password and port under Tools → Streams");
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(tr("DJs connect to the station's DJ port (usually 8005) with these logins — RadioMaster's encoders can too."))
                .small()
                .color(GRAY),
        );
    }

    fn az_dj_form(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some(mut f) = self.az.dj.take() else { return };
        let mut keep = true;
        let mut save = false;
        egui::Window::new(if f.id.is_some() { "Edit streamer" } else { "Add streamer" })
            .id(egui::Id::new("az_dj_form"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("dj_form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Username"));
                    ui.add(egui::TextEdit::singleline(&mut f.username).desired_width(220.0));
                    ui.end_row();
                    ui.label(tr("Password"));
                    ui.add(
                        egui::TextEdit::singleline(&mut f.password)
                            .password(true)
                            .hint_text(if f.id.is_some() { "leave empty to keep" } else { "" })
                            .desired_width(220.0),
                    );
                    ui.end_row();
                    ui.label(tr("Display name"));
                    ui.add(egui::TextEdit::singleline(&mut f.display_name).desired_width(220.0));
                    ui.end_row();
                    ui.label(tr("Comments"));
                    ui.add(egui::TextEdit::singleline(&mut f.comments).desired_width(220.0));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut f.is_active, tr("Account is active"));
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut f.enforce_schedule, tr("Only allow during scheduled times"));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(
                            !f.username.trim().is_empty() && (f.id.is_some() || !f.password.is_empty()),
                            egui::Button::new(RichText::new(tr("Save")).strong()),
                        )
                        .clicked();
                    if ui.button(tr("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
        if save {
            let mut body = json!({
                "streamer_username": f.username.trim(),
                "display_name": f.display_name,
                "comments": f.comments,
                "is_active": f.is_active,
                "enforce_schedule": f.enforce_schedule,
            });
            if !f.password.is_empty() {
                body["streamer_password"] = json!(f.password);
            }
            match f.id {
                Some(id) => self.az_action(core, "Save streamer", "streamers", "PUT", format!("/station/{{st}}/streamer/{id}"), Some(body)),
                None => self.az_action(core, "Add streamer", "streamers", "POST", "/station/{st}/streamers".into(), Some(body)),
            }
            return;
        }
        if keep {
            self.az.dj = Some(f);
        }
    }

    // ------------------------------------------------------------ listeners
    fn az_listeners(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "listeners", 30);
        self.az_head(ui, core, "listeners", "Listeners right now");
        let items = match self.az.get("listeners") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        ui.label(format!("{} connected", items.len()));
        egui::Grid::new("az_lis").striped(true).num_columns(5).spacing([10.0, 3.0]).show(ui, |ui| {
            for h in ["IP", "Location", "Time", "Mount", "Player"] {
                ui.strong(tr(h));
            }
            ui.end_row();
            for l in &items {
                ui.label(vs(l, "ip"));
                ui.label(first(l, &["location.description", "location.country", "location.city"]));
                ui.label(fmt(vf(l, "connected_time") as f32));
                ui.label(first(l, &["mount_name", "mount"]));
                let ua = vs(l, "user_agent");
                ui.add(egui::Label::new(RichText::new(&ua).small().color(GRAY)).truncate()).on_hover_text(ua);
                ui.end_row();
            }
        });
    }

    // -------------------------------------------------------------- history
    fn az_history(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(tr("Day"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.az.history_date).desired_width(100.0));
            if r.lost_focus() || ui.button(tr("Show")).clicked() {
                self.az_fetch(core, "history");
            }
        });
        self.az_auto(core, "history", 300);
        self.az_head(ui, core, "history", "Song history");
        let items = match self.az.get("history") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        egui::Grid::new("az_hist").striped(true).num_columns(4).spacing([10.0, 3.0]).show(ui, |ui| {
            for h in ["Played", "Song", "Source", "Listeners"] {
                ui.strong(tr(h));
            }
            ui.end_row();
            for h in &items {
                ui.label(when_any(h, &["played_at"]));
                ui.label(song_text(h));
                let src = if vb(h, "is_request") {
                    "Request".to_string()
                } else {
                    let s = first(h, &["streamer", "playlist"]);
                    s
                };
                ui.label(RichText::new(src).small().color(GRAY));
                ui.label(first(h, &["listeners_start", "listeners"]));
                ui.end_row();
            }
        });
    }

    // ------------------------------------------------------------- schedule
    fn az_schedule(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if Self::az_needs_key(ui, core) {
            return;
        }
        self.az_auto(core, "schedule", 300);
        self.az_head(ui, core, "schedule", "Upcoming schedule");
        let items = match self.az.get("schedule") {
            Some(Ok(v)) => rows(v),
            _ => vec![],
        };
        if items.is_empty() && !self.az.loading("schedule") {
            ui.label(RichText::new(tr("Nothing scheduled.")).color(GRAY));
        }
        for s in &items {
            ui.horizontal(|ui| {
                let now = vb(s, "is_now");
                ui.label(
                    RichText::new(format!("{} → {}", when_any(s, &["start_timestamp", "start"]), when_any(s, &["end_timestamp", "end"])))
                        .monospace(),
                );
                ui.label(RichText::new(first(s, &["title", "name"])).strong().color(if now { GREEN } else { ui.visuals().text_color() }));
                ui.label(RichText::new(vs(s, "type")).small().color(GRAY));
                if now {
                    ui.label(RichText::new(tr("NOW")).small().strong().color(GREEN));
                }
            });
        }
    }

    // ----------------------------------------------------------- connection
    fn az_connection(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.label(RichText::new(tr("The AzuraCast tool is optional. Now-playing info only needs the address and station; managing the station needs an API key.")).small().color(GRAY));
        ui.add_space(4.0);
        self.tab_azuracast(ui, core);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(tr("Find stations on this server")).clicked() {
                self.az_fetch(core, "stations");
            }
            if ui.button(tr("Test API key")).clicked() {
                self.az_action(core, "API key test", "status", "GET", "/station/{st}/status".into(), None);
            }
        });
        if let Some(r) = self.az.get("stations").cloned() {
            match r {
                Ok(v) => {
                    let mut pick = None;
                    for s in rows(&v) {
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("Use")).clicked() {
                                pick = Some(vs(&s, "shortcode"));
                            }
                            ui.label(format!("{}  ({}, id {})", vs(&s, "name"), vs(&s, "shortcode"), vs(&s, "id")));
                        });
                    }
                    if let Some(sc) = pick {
                        core.cfg.azuracast.station = sc;
                        core.cfg.azuracast.enabled = true;
                    }
                }
                Err(e) => {
                    ui.colored_label(RED, e);
                }
            }
        }
    }
}
