//! Running a RadioMaster Server from the app, for its administrators: its accounts (File → Server
//! users), and its program (an update notice like the app's own, Update, Restart and Stop).
use super::*;
use crate::link::Link;
use crate::playout::sync::{ServerInfo, UserOp, UserWire};

type Answer<T> = Option<mpsc::Receiver<Result<T, String>>>;

/// How often the app asks the server about itself (its version, a newer release).
const INFO_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub(super) struct ServerAdmin {
    /// what the server said about itself last
    info: Option<ServerInfo>,
    info_rx: Answer<ServerInfo>,
    info_next: Option<std::time::Instant>,
    pub(super) show_update: bool,
    /// the update notice opened once for this connection
    told: bool,
    control_rx: Answer<ServerInfo>,
    /// Stop or Restart pressed: waiting for "Stop!/Keep"
    confirm: Option<&'static str>,
    control_msg: String,
    show_users: bool,
    users: Vec<UserWire>,
    users_rx: Answer<Vec<UserWire>>,
    /// (ok, message)
    users_msg: (bool, String),
}

impl ServerAdmin {
    pub(super) fn window_open(&self) -> bool {
        self.show_update || self.show_users
    }
}

/// Run `f` off the UI thread and wake the UI with its answer.
fn ask<T: Send + 'static>(ctx: &egui::Context, f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Answer<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
    Some(rx)
}

/// The newest version the server could run: a release it found, or this app's own version when the
/// server is older than the app. None: it is up to date.
fn behind(info: &ServerInfo) -> Option<String> {
    let app = crate::updater::CURRENT;
    if !info.latest.is_empty() && crate::updater::is_newer(&info.latest, app) {
        Some(info.latest.clone())
    } else if crate::updater::is_newer(app, &info.version) {
        Some(app.to_string())
    } else if !info.latest.is_empty() {
        // (the server only sends `latest` when it is newer than the server itself)
        Some(info.latest.clone())
    } else {
        None
    }
}

impl App {
    /// The account signed in on the server is an administrator.
    pub(super) fn server_admin(&self) -> bool {
        self.srv.session().is_some_and(|s| s.admin)
    }

    pub(super) fn open_server_users(&mut self) {
        let Some(s) = self.srv.session() else { return };
        let a = &mut self.srv.admin;
        a.show_users = true;
        a.users_msg = (true, String::new());
        a.users_rx = ask(&self.ctx, move || Link::users(&s));
    }

    pub(super) fn server_admin_windows(&mut self, ctx: &egui::Context) {
        let Some(s) = self.srv.session() else { return };
        let a = &mut self.srv.admin;
        // what the server says about itself: now and then, and every couple of seconds while it updates
        if let Some(Ok(r)) = a.info_rx.as_ref().map(|rx| rx.try_recv()) {
            a.info_rx = None;
            if let Ok(info) = r {
                if !a.told && s.admin && behind(&info).is_some() && info.update_state.is_empty() {
                    a.told = true;
                    a.show_update = true;
                }
                a.info = Some(info);
            }
        }
        let busy = a.info.as_ref().is_some_and(|i| !i.update_state.is_empty() && !i.update_state.starts_with("The update failed"));
        if a.info_rx.is_none() && a.info_next.is_none_or(|t| std::time::Instant::now() >= t) {
            a.info_next = Some(std::time::Instant::now() + if busy { Duration::from_secs(2) } else { INFO_EVERY });
            let s = s.clone();
            a.info_rx = ask(ctx, move || Link::server_info(&s));
        }
        if let Some(Ok(r)) = a.control_rx.as_ref().map(|rx| rx.try_recv()) {
            a.control_rx = None;
            match r {
                Ok(info) => {
                    a.control_msg = info.update_state.clone();
                    a.info = Some(info);
                    a.info_next = Some(std::time::Instant::now() + Duration::from_secs(2));
                }
                Err(e) => a.control_msg = e,
            }
        }
        self.server_update_window(ctx, &s);
        self.server_users_window(ctx, &s);
    }

    /// "⬆ Server update" beside the connection chip, like the app's own update button.
    pub(super) fn server_update_button(&mut self, ui: &mut egui::Ui) {
        let Some(v) = self.srv.admin.info.as_ref().and_then(behind) else { return };
        let b = egui::Button::new(RichText::new(format!("⬆ Server: v{v}")).strong().color(Color32::WHITE))
            .fill(Color32::from_rgb(20, 150, 235));
        let hover = if self.server_admin() {
            tr("RadioMaster Server is behind — click to update it")
        } else {
            tr("RadioMaster Server is behind: ask one of its administrators to update it")
        };
        if ui.add(b).on_hover_text(hover).clicked() {
            self.srv.admin.show_update = true;
        }
    }

    /// Update, Restart and Stop, in the update window and Server settings (administrators only).
    pub(super) fn server_program_section(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.srv.session() else { return };
        let a = &mut self.srv.admin;
        let info = a.info.clone().unwrap_or_default();
        ui.label(format!(
            "RadioMaster Server {} · this app {}",
            if info.version.is_empty() { "?" } else { &info.version },
            crate::updater::CURRENT
        ));
        match behind(&info) {
            Some(v) => ui.label(RichText::new(format!("Version {v} is available.")).strong()),
            None if a.info.is_some() => ui.colored_label(GREEN, tr("✔ The server is up to date.")),
            None => ui.label(RichText::new(tr("Asking the server…")).color(GRAY)),
        };
        if !s.admin {
            ui.label(RichText::new(tr("Only the server's administrators can update, restart or stop it.")).small().color(GRAY));
            return;
        }
        let working = a.control_rx.is_some() || (!info.update_state.is_empty() && !info.update_state.starts_with("The update failed"));
        let mut go: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            let can = behind(&info).is_some() && info.can_update && !working;
            let r = ui.add_enabled(can, egui::Button::new(RichText::new(tr("⬇ Update the server")).strong()));
            if r.clicked() {
                go = Some("update");
            }
            if behind(&info).is_some() && !info.can_update && !info.latest.is_empty() {
                r.on_disabled_hover_text(tr("There's no RadioMaster Server download for the server's system: update it by hand"));
            }
            for (what, label) in [("restart", tr("Restart")), ("stop", tr("Stop"))] {
                if a.confirm == Some(what) {
                    if ui.button(RichText::new(format!("{label}!")).color(RED)).clicked() {
                        go = Some(what);
                        a.confirm = None;
                    }
                    if ui.button(tr("Keep")).clicked() {
                        a.confirm = None;
                    }
                } else if ui.add_enabled(!working, egui::Button::new(label)).clicked() {
                    a.confirm = Some(what);
                }
            }
            if working {
                ui.spinner();
            }
        });
        if a.confirm == Some("stop") {
            ui.colored_label(AMBER, tr("A stopped server is off air until someone starts it again on that computer."));
        }
        let state = if a.control_msg.is_empty() { info.update_state.clone() } else { a.control_msg.clone() };
        if !state.is_empty() {
            ui.label(RichText::new(state).small());
        }
        ui.label(
            RichText::new(tr("The station goes quiet while the server restarts (a few seconds) — best done between shows. This app reconnects by itself."))
                .small()
                .color(GRAY),
        );
        if let Some(what) = go {
            a.control_msg.clear();
            a.control_rx = ask(&self.ctx, move || Link::server_control(&s, what));
        }
    }

    fn server_update_window(&mut self, ctx: &egui::Context, s: &crate::link::Session) {
        if !self.srv.admin.show_update {
            return;
        }
        let mut open = true;
        let latest = self.srv.admin.info.as_ref().and_then(behind);
        Self::window_for(ctx, "Update RadioMaster Server", [480.0, 300.0])
            .id(egui::Id::new("server_update"))
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(RichText::new(&s.name).strong());
                self.server_program_section(ui);
                if let Some(v) = &latest {
                    if ui.link(tr("What's new (release page)")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://github.com/{}/releases/tag/v{v}", crate::updater::REPO)));
                    }
                }
            });
        self.srv.admin.show_update = open;
    }

    fn server_users_window(&mut self, ctx: &egui::Context, s: &crate::link::Session) {
        if !self.srv.admin.show_users {
            return;
        }
        if let Some(Ok(r)) = self.srv.admin.users_rx.as_ref().map(|rx| rx.try_recv()) {
            self.srv.admin.users_rx = None;
            match r {
                Ok(u) => {
                    self.srv.admin.users = u;
                    if self.srv.admin.users_msg.1.is_empty() {
                        self.srv.admin.users_msg = (true, String::new());
                    }
                }
                Err(e) => self.srv.admin.users_msg = (false, e),
            }
        }
        let rows: Vec<UserRow> = self
            .srv
            .admin
            .users
            .iter()
            .map(|u| UserRow { name: u.name.clone(), admin: u.admin, perms: u.perms, disabled: u.disabled })
            .collect();
        let busy = self.srv.admin.users_rx.is_some();
        let mut open = true;
        let mut acts = vec![];
        let title = format!("{} — {}", tr("Server users"), s.name);
        Self::window_for(ctx, &title, [1000.0, 520.0]).id(egui::Id::new("server_users")).open(&mut open).show(ctx, |ui| {
            ui.label(
                RichText::new(tr(
                    "These accounts sign in to the server (from the app and the web portal). The server checks every action against them.",
                ))
                .small()
                .color(GRAY),
            );
            ui.add_enabled_ui(!busy, |ui| acts = self.users_editor(ui, "server_users_grid", &rows, &s.user));
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                }
                let (ok, msg) = &self.srv.admin.users_msg;
                if !msg.is_empty() {
                    ui.colored_label(if *ok { GREEN } else { RED }, msg);
                }
            });
        });
        self.srv.admin.show_users = open;
        // one change at a time: the answer is the list as the server has it then
        if let Some(act) = acts.into_iter().next() {
            let (op, done) = match act {
                UserAct::Add { name, password, admin, perms } => {
                    let msg = format!("Created {}", name.trim());
                    (UserOp::Add { name, password, admin, perms }, msg)
                }
                UserAct::Change { name, admin, perms, disabled } => {
                    let msg = format!("Saved {name}");
                    (UserOp::Update { name, admin, perms, disabled }, msg)
                }
                UserAct::Password { name, password } => {
                    let msg = format!("Saved a new password for {name}");
                    (UserOp::Password { name, password }, msg)
                }
                UserAct::Delete { name } => {
                    let msg = format!("Deleted {name}");
                    (UserOp::Delete { name }, msg)
                }
            };
            self.srv.admin.users_msg = (true, done);
            let s = s.clone();
            self.srv.admin.users_rx = ask(ctx, move || Link::user_op(&s, &op));
        }
    }
}
