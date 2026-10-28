//! File → Connect to Server: sign in to a RadioMaster Server and run that station from this app.
//! The local session (playlists, settings, encoders) is put aside while connected, optionally saved
//! as a station profile first, and comes back on disconnect.
use super::*;
use crate::link::{Hearing, Link, Listener, LoginError, Session};
use crate::playout::sync;

/// The last server used (server_link.json; the password is never kept).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Remembered {
    address: String,
    user: String,
    /// host:port → SHA-256 of the self-signed HTTPS certificate trusted for it
    pins: std::collections::BTreeMap<String, String>,
}

impl Remembered {
    fn path() -> PathBuf {
        data_dir().join("server_link.json")
    }
}

/// The key a trusted certificate is kept under: the address without http:// or https://.
fn pin_key(base: &str) -> String {
    base.trim_start_matches("https://").trim_start_matches("http://").to_lowercase()
}

/// Where the local session is kept while someone signed in from the login screen runs a server.
const REMOTE_PROFILE: &str = "Local session before remote sign-in";

struct Connected {
    link: Link,
    /// this app's own session, put aside while connected
    local: Playout,
    /// the profile it was saved as (None: the server's playlists and settings stay after disconnecting)
    profile: Option<String>,
    /// local encoders that were on air, started again on disconnect
    encoders: Vec<usize>,
    /// the station's sound, playing here (None when "Hear the station here" is off)
    listen: Option<Listener>,
}

pub(super) struct ServerUi {
    show_connect: bool,
    address: String,
    user: String,
    password: String,
    busy: Option<mpsc::Receiver<Result<Session, LoginError>>>,
    error: String,
    /// "Trust this certificate?": (address, fingerprint, it changed since it was trusted)
    trust: Option<(String, String, bool)>,
    pins: std::collections::BTreeMap<String, String>,
    /// Connect to Remote Server was pressed on the login screen: no local sign-in, and the local
    /// session is saved and comes back on disconnect without asking
    pub(super) from_login: bool,
    /// signed in that way: disconnecting goes back to the login screen
    remote_login: bool,
    /// signed in, waiting for the answer to "save your local session?"
    pub(super) ask: Option<Session>,
    profile: String,
    show_status: bool,
    /// Server settings window: the server's settings being edited (None while they load)
    show_settings: bool,
    settings: Option<sync::ServerInfo>,
    settings_busy: Option<mpsc::Receiver<Result<sync::ServerInfo, String>>>,
    /// (saved, message)
    settings_msg: (bool, String),
    pw_current: String,
    pw_new: String,
    pw_again: String,
    /// a new Cloudflare Tunnel token typed in Server settings (the saved one is never shown)
    tunnel_token: String,
    /// accounts, updates, restart and stop (administrators)
    pub(super) admin: super::serveradmin::ServerAdmin,
    conn: Option<Connected>,
}

impl Default for ServerUi {
    fn default() -> Self {
        let r: Remembered =
            std::fs::read_to_string(Remembered::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        ServerUi {
            show_connect: false,
            address: r.address,
            user: r.user,
            password: String::new(),
            busy: None,
            error: String::new(),
            trust: None,
            pins: r.pins,
            from_login: false,
            remote_login: false,
            ask: None,
            profile: String::new(),
            show_status: false,
            show_settings: false,
            settings: None,
            settings_busy: None,
            settings_msg: (false, String::new()),
            pw_current: String::new(),
            pw_new: String::new(),
            pw_again: String::new(),
            tunnel_token: String::new(),
            admin: Default::default(),
            conn: None,
        }
    }
}

impl ServerUi {
    pub(super) fn connected(&self) -> bool {
        self.conn.is_some()
    }

    /// The sign-in as it stands (None when not connected).
    pub(super) fn session(&self) -> Option<Session> {
        self.conn.as_ref().map(|c| c.link.current())
    }

    /// Connected with Connect to Remote Server on the login screen (no local account).
    pub(super) fn signed_in_remotely(&self) -> bool {
        self.remote_login
    }

    /// A window of this tool is open (web views must hide under it).
    pub(super) fn window_open(&self) -> bool {
        self.show_connect || self.ask.is_some() || self.show_status || self.show_settings || self.admin.window_open()
    }
}

impl App {
    /// File menu entries.
    pub(super) fn server_menu(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if self.srv.connected() {
            if self.server_admin() {
                if ui.button(tr("Server settings…")).clicked() {
                    self.open_server_settings();
                    ui.close_menu();
                }
                if ui.button(tr("Server users…")).clicked() {
                    self.open_server_users();
                    ui.close_menu();
                }
            }
            if ui.button(tr("Server info…")).clicked() {
                self.srv.show_status = true;
                ui.close_menu();
            }
            if ui.button(tr("Disconnect")).clicked() {
                self.disconnect(core);
                ui.close_menu();
            }
        } else if ui.button(tr("Connect to Server…")).clicked() {
            self.open_connect(false);
            ui.close_menu();
        }
    }

    /// The Connect to Server window; `from_login`: opened from the login screen.
    pub(super) fn open_connect(&mut self, from_login: bool) {
        self.srv.show_connect = true;
        self.srv.from_login = from_login;
        self.srv.error.clear();
        self.srv.trust = None;
    }

    /// Signed in from the login screen: keep the local session as a profile and run the server's
    /// station, with the permissions the server account gives (everything but local user accounts).
    pub(super) fn remote_sign_in(&mut self, core: &mut Playout) {
        let Some(s) = self.srv.ask.take() else { return };
        self.srv.profile = REMOTE_PROFILE.into();
        let Some(profile) = self.save_session_profile(core) else {
            self.srv.error = "The local session could not be saved, so it can't be put aside".into();
            self.srv.show_connect = true;
            Link::sign_out(s);
            return;
        };
        let name = format!("{} @ {}", s.user, s.name);
        self.connect(core, s, Some(profile));
        self.srv.remote_login = true;
        self.srv.from_login = false;
        self.logged_in = Some(crate::auth::Operator {
            name,
            password_hash: String::new(),
            admin: false,
            perms: crate::auth::Perms::default(),
            disabled: false,
        });
        self.auth_error.clear();
    }

    /// "Add library folder…": a folder here, or while connected a folder on the server.
    pub(super) fn pick_library_folder(&mut self) {
        self.pick(Pick::Folder, tr("Add library folder"), |_, v| Some(Dlg::LibFolder(v.into_iter().next()?)));
    }

    /// The engine the sound cards belong to: the local one, also while a server is mirrored.
    pub(super) fn output_engine(&self, core: &Playout) -> crate::engine::Engine {
        self.srv.conn.as_ref().map(|c| c.local.engine.clone()).unwrap_or_else(|| core.engine.clone())
    }

    /// While connected: the mic, sent to the server (None when not connected).
    pub(super) fn server_mic(&self, device: &str) -> Option<anyhow::Result<rodio::cpal::Stream>> {
        let c = self.srv.conn.as_ref()?;
        Some(crate::link::mic_uplink(c.link.session.base.clone(), c.link.session.pin.clone(), c.link.token(), device))
    }

    /// Start or stop playing the station here when "Hear the station here" changes.
    fn server_audio(&mut self, core: &Playout) {
        let Some(c) = &mut self.srv.conn else { return };
        if core.cfg.hear_server == c.listen.is_some() {
            return;
        }
        c.listen = core.cfg.hear_server.then(|| Self::listener(c, core));
        // (kept with this machine's own settings)
        c.local.cfg.hear_server = core.cfg.hear_server;
        c.local.save_all();
    }

    fn listener(c: &Connected, core: &Playout) -> Listener {
        let notify = core.notify.clone();
        let say = Box::new(move |m: String| *notify.lock() = m);
        Listener::start(c.local.engine.clone(), c.link.session.base.clone(), c.link.session.pin.clone(), c.link.token(), say)
    }

    /// The connection indicator left of the RadioMaster logo.
    pub(super) fn server_chip(&mut self, ui: &mut egui::Ui) {
        let Some(c) = &self.srv.conn else { return };
        let st = c.link.state.lock().clone();
        let (fill, hover) = if st.online {
            (
                Color32::from_rgb(40, 150, 70),
                format!("Connected to {} ({}) as {} — click for details", c.link.session.name, c.link.session.base, c.link.session.user),
            )
        } else {
            (Color32::from_rgb(200, 140, 20), format!("Connection to {} lost, retrying: {}", c.link.session.name, st.error))
        };
        let b = egui::Button::new(RichText::new(format!("🖧 {}", c.link.session.name)).strong().color(Color32::WHITE)).fill(fill);
        if ui.add(b).on_hover_text(hover).clicked() {
            self.srv.show_status = true;
        }
    }

    pub(super) fn server_windows(&mut self, ctx: &egui::Context, core: &mut Playout) {
        self.server_audio(core);
        self.connect_window(ctx);
        self.save_session_window(ctx, core);
        self.server_status_window(ctx, core);
        self.server_settings_window(ctx);
        self.server_picker_window(ctx, core);
        self.server_admin_windows(ctx);
    }

    pub(super) fn connect_window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.srv.busy {
            match rx.try_recv() {
                Ok(Ok(s)) => {
                    self.srv.busy = None;
                    self.srv.show_connect = false;
                    self.srv.password.clear();
                    self.srv.profile = format!("Local session {}", Local::now().format("%Y-%m-%d %H%M"));
                    self.srv.ask = Some(s);
                }
                Ok(Err(LoginError::Untrusted { base, fingerprint, changed })) => {
                    self.srv.busy = None;
                    self.srv.trust = Some((base, fingerprint, changed));
                }
                Ok(Err(LoginError::Failed(e))) => {
                    self.srv.busy = None;
                    self.srv.error = e;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.srv.busy = None,
            }
        }
        if !self.srv.show_connect {
            return;
        }
        let mut open = true;
        let mut go = false;
        Self::window_for(ctx, "Connect to Server", [440.0, 300.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                ui.label(tr("Run a station that lives on a RadioMaster Server from this app. Sign in with the username and password set up on the server."));
                ui.add_space(6.0);
                egui::Grid::new("srv_connect").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Server"));
                    ui.add(egui::TextEdit::singleline(&mut self.srv.address).hint_text("192.168.1.20:9259").desired_width(260.0));
                    ui.end_row();
                    ui.label(tr("Username"));
                    ui.add(egui::TextEdit::singleline(&mut self.srv.user).desired_width(260.0));
                    ui.end_row();
                    ui.label(tr("Password"));
                    let r = ui.add(egui::TextEdit::singleline(&mut self.srv.password).password(true).desired_width(260.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        go = true;
                    }
                    ui.end_row();
                });
                ui.label(
                    RichText::new(tr("Without a port, 9259 is used. A server with HTTPS on is found either way; type https:// to never try without it."))
                        .small()
                        .color(GRAY),
                );
                if self.srv.from_login {
                    ui.label(
                        RichText::new(tr("Signing in here skips the local login. Your local session is put aside and comes back when you disconnect."))
                            .small()
                            .color(GRAY),
                    );
                }
                if let Some((base, fp, changed)) = &self.srv.trust {
                    ui.add_space(6.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        if *changed {
                            ui.colored_label(RED, format!("The HTTPS certificate of {base} is not the one you trusted before."));
                            ui.label(tr("If nobody changed the server's certificate, someone may be listening in. Don't trust it unless you're sure."));
                        } else {
                            ui.label(RichText::new(format!("{base} uses its own (self-signed) HTTPS certificate.")).strong());
                            ui.label(tr("Check that this fingerprint is the one RadioMaster Server printed when it started (also in its Server settings). It is remembered for this server."));
                        }
                        ui.add(egui::Label::new(RichText::new(fp).monospace().small()).wrap());
                    });
                }
                if !self.srv.error.is_empty() {
                    ui.colored_label(RED, &self.srv.error);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let ready = self.srv.busy.is_none() && !self.srv.address.trim().is_empty() && !self.srv.user.trim().is_empty() && !self.srv.password.is_empty();
                if self.srv.trust.is_some() {
                    if ui.add_enabled(ready, egui::Button::new(RichText::new(tr("Trust and connect")).strong())).clicked() {
                        if let Some((base, fp, _)) = self.srv.trust.take() {
                            self.srv.pins.insert(pin_key(&base), fp);
                            // https:// from now on, so the pinned certificate is always asked for
                            self.srv.address = base;
                        }
                        go = true;
                    }
                } else if ui.add_enabled(ready, egui::Button::new(tr("Connect"))).clicked() {
                    go = true;
                }
                if self.srv.busy.is_some() {
                    ui.spinner();
                    ui.label(tr("Signing in…"));
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.srv.show_connect = false;
                    self.srv.trust = None;
                }
            });
        });
        if !open {
            self.srv.show_connect = false;
            self.srv.trust = None;
        }
        if go && self.srv.busy.is_none() && !self.srv.password.is_empty() {
            let (tx, rx) = mpsc::channel();
            let (a, u, p, ctx) =
                (self.srv.address.trim().to_string(), self.srv.user.trim().to_string(), self.srv.password.clone(), ctx.clone());
            let pin = self.srv.pins.get(&pin_key(&crate::link::normalize(&a))).cloned().unwrap_or_default();
            self.srv.trust = None;
            thread::spawn(move || {
                let _ = tx.send(crate::link::login(&a, &u, &p, &pin));
                ctx.request_repaint();
            });
            self.srv.busy = Some(rx);
            self.srv.error.clear();
        }
    }

    pub(super) fn save_session_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some(s) = &self.srv.ask else { return };
        let name = s.name.clone();
        let mut choice: Option<bool> = None;
        let mut cancel = false;
        let mut open = true;
        Self::window_for(ctx, "Save your local session?", [480.0, 300.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                ui.label(format!("Signed in to {name}. While you're connected, this app shows and runs the server's station; your own playback and encoders stop."));
                ui.add_space(4.0);
                ui.label(tr("Save your playlists and settings as a station profile? They come back exactly as they are when you disconnect."));
                ui.horizontal(|ui| {
                    ui.label(tr("Profile:"));
                    ui.add(egui::TextEdit::singleline(&mut self.srv.profile).desired_width(240.0));
                });
                ui.label(
                    RichText::new(tr("If you don't save, the server's playlists and settings stay in this app after you disconnect."))
                        .small()
                        .color(GRAY),
                );
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!profile_file(&self.srv.profile).is_empty(), egui::Button::new(RichText::new(tr("Save and connect")).strong())).clicked() {
                    choice = Some(true);
                }
                if ui.button(tr("Connect without saving")).clicked() {
                    choice = Some(false);
                }
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel || !open {
            if let Some(s) = self.srv.ask.take() {
                // signed in for nothing: sign out again
                Link::sign_out(s);
            }
            return;
        }
        if let Some(save) = choice {
            let s = self.srv.ask.take().unwrap();
            let profile = if save { self.save_session_profile(core) } else { None };
            if save && profile.is_none() {
                self.srv.error = "The profile could not be saved".into();
                self.srv.ask = Some(s);
                return;
            }
            self.connect(core, s, profile);
        }
    }

    /// Write the station settings and every playlist to profiles/<name>.json.
    fn save_session_profile(&mut self, core: &Playout) -> Option<String> {
        let safe = profile_file(&self.srv.profile);
        if safe.is_empty() {
            return None;
        }
        let mut v = serde_json::to_value(&core.cfg).ok()?;
        // Station profiles → Load reads the settings and ignores the playlists
        v.as_object_mut()?.insert("_session".into(), serde_json::to_value(core.tabs_wire()).ok()?);
        let dir = data_dir().join("profiles");
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(dir.join(format!("{safe}.json")), serde_json::to_vec_pretty(&v).ok()?).ok()?;
        if !self.profile_list.contains(&safe) {
            self.profile_list.push(safe);
        }
        Some(self.srv.profile.trim().to_string())
    }

    fn connect(&mut self, core: &mut Playout, s: Session, profile: Option<String>) {
        // the local station goes quiet: one station on air at a time
        let encoders: Vec<usize> = (0..core.handles.len()).filter(|&i| core.handles[i].as_ref().is_some_and(|h| h.is_running())).collect();
        for &i in &encoders {
            core.stop_encoder(i);
        }
        core.stop();
        core.stop_carts();
        self.mic_stream = None;
        self.mic_on = false;
        core.save_all();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut local = Playout::mirror_of(core, tx);
        let waves = local.wave_requests();
        std::mem::swap(core, &mut local);
        let ctx = self.ctx.clone();
        // (`core` is the locked playout: Link::start must not lock it again, or the app hangs)
        let link = Link::start(self.core.clone(), s.clone(), rx, waves, Box::new(move || ctx.request_repaint()));
        if !s.pin.is_empty() {
            self.srv.pins.insert(pin_key(&s.base), s.pin.clone());
        }
        let r = Remembered { address: self.srv.address.trim().to_string(), user: s.user.clone(), pins: self.srv.pins.clone() };
        let _ = std::fs::write(Remembered::path(), serde_json::to_string_pretty(&r).unwrap_or_default());
        core.say(format!("Connected to {} ({}) as {}", s.name, s.base, s.user));
        let mut c = Connected { link, local, profile, encoders, listen: None };
        if core.cfg.hear_server {
            c.listen = Some(Self::listener(&c, core));
        }
        self.srv.conn = Some(c);
        self.reset_playlist_view();
    }

    /// Back to the local session.
    pub(super) fn disconnect(&mut self, core: &mut Playout) {
        let Some(mut c) = self.srv.conn.take() else { return };
        // the mic and the station's sound were the server's
        self.mic_stream = None;
        self.mic_on = false;
        self.mic_paused = false;
        c.listen = None;
        c.link.stop();
        std::mem::swap(core, &mut c.local);
        let mirror = c.local;
        match &c.profile {
            Some(p) => {
                for i in c.encoders {
                    core.start_encoder(i);
                }
                core.say(format!("Disconnected from {}: your local session ({p}) is back", c.link.session.name));
            }
            None => {
                // not saved: carry on with a copy of the server's playlists and settings
                core.set_tabs(mirror.tabs_wire());
                core.set_station_cfg(sync::station_cfg(&mirror.cfg));
                core.set_library(mirror.library.clone());
                core.save_library();
                core.save_all();
                core.say(format!("Disconnected from {}: its playlists and settings stay here", c.link.session.name));
            }
        }
        drop(mirror);
        self.srv.show_status = false;
        self.reset_playlist_view();
        if std::mem::take(&mut self.srv.remote_login) {
            // signed in from the login screen: back to it
            self.logged_in = None;
            self.login_password.clear();
            self.srv.show_settings = false;
        }
    }

    fn reset_playlist_view(&mut self) {
        // file browsers start again on this computer's or the server's folders
        self.files.reset();
        self.srv.admin = Default::default();
        self.rep.forget();
        self.fx_dir.clear();
        self.fx_list.clear();
        self.fx_loading.clear();
        self.view_pl = usize::MAX;
        self.sel.clear();
        self.anchor = None;
        self.followed = None;
        self.lib_sel.clear();
        // the other session's library can have the same version number
        self.lib_key = (String::new(), 0);
        self.lib_lower = (0, vec![]);
        self.search_hits = ((String::new(), 0), vec![]);
    }

    fn server_status_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.srv.show_status {
            return;
        }
        let Some(c) = &self.srv.conn else {
            self.srv.show_status = false;
            return;
        };
        let st = c.link.state.lock().clone();
        let s = c.link.session.clone();
        let profile = c.profile.clone();
        let hearing = c.listen.as_ref().map(|l| l.state.lock().clone());
        let mut open = true;
        let mut off = false;
        Self::window_for(ctx, "Server connection", [460.0, 320.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                egui::Grid::new("srv_status").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    ui.label(tr("Server"));
                    ui.label(RichText::new(&s.name).strong());
                    ui.end_row();
                    ui.label(tr("Address"));
                    ui.label(&s.base);
                    ui.end_row();
                    ui.label(tr("Security"));
                    if !s.base.starts_with("https://") {
                        ui.colored_label(AMBER, tr("Not encrypted (http). Turn on HTTPS in Server settings for a server on the internet."));
                    } else if s.pin.is_empty() {
                        ui.colored_label(GREEN, tr("HTTPS, certificate from a public authority"));
                    } else {
                        ui.colored_label(GREEN, tr("HTTPS, the server's own certificate that you trusted"))
                            .on_hover_text(format!("SHA-256 {}", s.pin));
                    }
                    ui.end_row();
                    ui.label(tr("Signed in as"));
                    if s.admin {
                        ui.label(format!("{} ({})", s.user, tr("administrator")));
                    } else {
                        ui.label(&s.user).on_hover_text(tr("What this account may do is set by the server's administrators"));
                    }
                    ui.end_row();
                    ui.label(tr("Version"));
                    ui.label(&s.version);
                    ui.end_row();
                    ui.label(tr("Status"));
                    if st.online {
                        ui.colored_label(GREEN, format!("Connected · {} ms", st.rtt_ms));
                    } else {
                        ui.colored_label(AMBER, format!("Reconnecting… {}", st.error));
                    }
                    ui.end_row();
                    ui.label(tr("Station audio"));
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut core.cfg.hear_server, tr("Hear the station here"))
                            .on_hover_text(tr("Play the server's on-air mix on this computer's outputs (Settings → Audio). Turn it off when the server's speakers are in the same room."));
                        match &hearing {
                            Some(Hearing::Playing) => {
                                ui.colored_label(GREEN, tr("Playing"));
                            }
                            Some(Hearing::Connecting) => {
                                ui.colored_label(GRAY, tr("Connecting…"));
                            }
                            Some(Hearing::Reconnecting) => {
                                ui.colored_label(GRAY, tr("Reconnecting…"));
                            }
                            Some(Hearing::SigningIn) => {
                                ui.colored_label(GRAY, tr("Signing in again…"));
                            }
                            Some(Hearing::Trouble(_)) => {
                                ui.colored_label(AMBER, tr("Not playing"));
                            }
                            None => {}
                        }
                    });
                    ui.end_row();
                    if let Some(Hearing::Trouble(why)) = &hearing {
                        ui.label("");
                        ui.add(egui::Label::new(RichText::new(why).small().color(AMBER)).wrap());
                        ui.end_row();
                    }
                    ui.label(tr("Local session"));
                    ui.label(match &profile {
                        Some(p) => format!("saved as profile \"{p}\", restored on disconnect"),
                        None => "not saved: the server's playlists and settings stay after disconnecting".into(),
                    });
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr("Files, folders and the library are the server's: file pickers and the file explorer show the server's folders, and Upload copies files from this computer to it. The Mic button sends this computer's microphone to the server, which puts it on air."))
                        .small()
                        .color(GRAY),
                );
                ui.separator();
                ui.label(RichText::new(tr("Server program")).strong());
                self.server_program_section(ui);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if self.server_admin() && ui.button(tr("Server settings…")).clicked() {
                    self.open_server_settings();
                }
                if ui.button(RichText::new(tr("Disconnect")).strong()).clicked() {
                    off = true;
                }
                if ui.button(tr("Close")).clicked() {
                    self.srv.show_status = false;
                }
            });
        });
        if !open {
            self.srv.show_status = false;
        }
        if off {
            self.disconnect(core);
        }
    }
}

impl App {
    fn open_server_settings(&mut self) {
        self.srv.show_settings = true;
        self.srv.settings = None;
        self.srv.settings_msg = (false, String::new());
        self.server_request(None);
    }

    /// Load (`None`) or save the server's settings in the background.
    fn server_request(&mut self, update: Option<sync::ServerUpdate>) {
        let Some(c) = &self.srv.conn else { return };
        let (s, ctx) = (c.link.current(), self.ctx.clone());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let r = match &update {
                Some(u) => Link::server_update(&s, u),
                None => Link::server_info(&s),
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.srv.settings_busy = Some(rx);
    }

    fn server_settings_window(&mut self, ctx: &egui::Context) {
        if !self.srv.show_settings {
            return;
        }
        if self.srv.conn.is_none() {
            self.srv.show_settings = false;
            return;
        }
        if let Some(rx) = &self.srv.settings_busy {
            match rx.try_recv() {
                Ok(Ok(info)) => {
                    let saving = self.srv.settings.is_some();
                    if let Some(c) = self.srv.conn.as_mut() {
                        c.link.session.name = info.name.clone();
                        if saving && !self.srv.pw_new.is_empty() {
                            c.link.password_changed(self.srv.pw_new.clone());
                        }
                    }
                    self.srv.settings = Some(info);
                    self.srv.settings_busy = None;
                    if saving {
                        self.srv.settings_msg = (true, "Saved on the server.".into());
                        self.srv.pw_current.clear();
                        self.srv.pw_new.clear();
                        self.srv.pw_again.clear();
                    }
                }
                Ok(Err(e)) => {
                    self.srv.settings_busy = None;
                    self.srv.settings_msg = (false, e);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.srv.settings_busy = None,
            }
        }
        let busy = self.srv.settings_busy.is_some();
        let mut open = true;
        let mut save = false;
        let mut reload = false;
        let mut users = false;
        Self::window_for(ctx, "Server settings", [500.0, 520.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            let srv = &mut self.srv;
            let Some(info) = srv.settings.as_mut() else {
                ui.horizontal(|ui| {
                    if busy {
                        ui.spinner();
                        ui.label(tr("Loading the server's settings…"));
                    }
                });
                if !srv.settings_msg.1.is_empty() {
                    ui.colored_label(RED, &srv.settings_msg.1);
                }
                return;
            };
            egui::ScrollArea::vertical().max_height(ui.available_height() - 40.0).show(ui, |ui| {
                ui.label(
                    RichText::new(tr("These belong to the server itself. Station settings (playlists, streams, adverts…) are in the usual places while you're connected."))
                        .small()
                        .color(GRAY),
                );
                ui.add_space(4.0);
                egui::Grid::new("srv_settings").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Station name"));
                    ui.add(egui::TextEdit::singleline(&mut info.name).desired_width(260.0));
                    ui.end_row();
                    ui.label(tr("Server"));
                    ui.label(format!("RadioMaster Server {} · port {} · user {}", info.version, info.port, info.user));
                    ui.end_row();
                });
                ui.checkbox(&mut info.autoplay, tr("Start playing when the server starts"));
                ui.separator();
                ui.label(RichText::new(tr("Sound output")).strong());
                if info.forced_silent {
                    ui.colored_label(AMBER, tr("The server was started with --no-audio, so it streams only until it is started without it."));
                }
                ui.checkbox(&mut info.audio, tr("Play through the server's sound cards"))
                    .on_hover_text(tr("Off: no sound card is used and the station only streams"));
                ui.add_enabled_ui(info.audio, |ui| {
                    if info.devices.is_empty() {
                        ui.label(RichText::new(tr("The server has no sound cards.")).color(GRAY));
                    }
                    for d in info.devices.clone() {
                        let k = info.outputs.iter().position(|o| o.name == d);
                        let mut on = k.is_some_and(|k| info.outputs[k].enabled);
                        if ui.checkbox(&mut on, &d).changed() {
                            match k {
                                Some(k) => info.outputs[k].enabled = on,
                                None => info.outputs.push(crate::config::OutputSel { name: d.clone(), enabled: on, ..Default::default() }),
                            }
                        }
                    }
                    ui.label(
                        RichText::new(tr("With none ticked, the server's default sound card plays. The first one that opens drives the mix."))
                            .small()
                            .color(GRAY),
                    );
                });
                for (dev, st) in &info.report {
                    ui.label(RichText::new(format!("{dev}: {st}")).small());
                }
                ui.separator();
                ui.label(RichText::new(tr("Security and web portal")).strong());
                ui.checkbox(&mut info.https, tr("Use HTTPS"))
                    .on_hover_text(tr("Encrypts everything between the apps and the server. Without a certificate given to the server (--cert/--key) it makes its own, which each app trusts once."));
                if !info.fingerprint.is_empty() {
                    ui.label(RichText::new(format!("{} {}", tr("Certificate SHA-256:"), info.fingerprint)).small().monospace());
                }
                if info.cert_expires > 0 {
                    if let Some(d) = chrono::DateTime::from_timestamp(info.cert_expires, 0) {
                        ui.label(RichText::new(format!("{} {}", tr("Certificate valid until"), d.format("%Y-%m-%d"))).small().color(GRAY));
                    }
                }
                ui.add_enabled_ui(info.https, |ui| {
                    ui.checkbox(&mut info.acme, tr("Get a free certificate from Let's Encrypt"))
                        .on_hover_text(tr("Browsers and apps trust it like any website. Port 80 of each domain name has to reach the server; it is renewed on its own."));
                    if info.acme {
                        egui::Grid::new("srv_acme").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                            ui.label(tr("Domain names"));
                            ui.add(egui::TextEdit::singleline(&mut info.acme_domains).hint_text("radio.example.com").desired_width(260.0));
                            ui.end_row();
                            ui.label(tr("Email (optional)"));
                            ui.add(egui::TextEdit::singleline(&mut info.acme_email).desired_width(260.0));
                            ui.end_row();
                        });
                        ui.checkbox(&mut info.acme_staging, tr("Use Let's Encrypt's staging server (for trying it out)"));
                        let color = match info.acme_state.as_str() {
                            "ok" => GREEN,
                            "error" => RED,
                            _ => GRAY,
                        };
                        if !info.acme_message.is_empty() {
                            ui.colored_label(color, format!("Let's Encrypt: {}", info.acme_message));
                        }
                    }
                });
                ui.checkbox(&mut info.portal, tr("Web portal: run the station from a browser"));
                ui.add_enabled_ui(info.portal, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("Portal port"));
                        ui.add(egui::DragValue::new(&mut info.portal_port).range(1..=65535));
                    });
                    if info.portal && info.portal_port == info.port {
                        ui.colored_label(RED, tr("The portal needs a port of its own."));
                    }
                });
                ui.label(
                    RichText::new(tr("The portal signs in with the server's username and password. These change when the server starts again."))
                        .small()
                        .color(GRAY),
                );
                ui.checkbox(&mut info.tunnel, tr("Put the web portal on the internet with a Cloudflare Tunnel"))
                    .on_hover_text(tr("No port forwarding needed. The server runs Cloudflare's cloudflared program, which has to be installed on it."));
                if info.tunnel {
                    ui.horizontal(|ui| {
                        ui.label(tr("Tunnel token"));
                        ui.add(
                            egui::TextEdit::singleline(&mut srv.tunnel_token)
                                .password(true)
                                .hint_text(if info.tunnel_token_set { tr("saved (type to replace)") } else { tr("none: a quick tunnel") })
                                .desired_width(240.0),
                        );
                    });
                    ui.label(
                        RichText::new(tr("Make a tunnel in the Cloudflare dashboard (Zero Trust → Networks → Tunnels) for your own domain, and point it at http://localhost and the portal port. Without a token you get a random trycloudflare.com address."))
                            .small()
                            .color(GRAY),
                    );
                    let color = match info.tunnel_state.as_str() {
                        "running" => GREEN,
                        "error" => RED,
                        _ => GRAY,
                    };
                    if !info.tunnel_state.is_empty() {
                        let mut line = format!("Cloudflare Tunnel: {}", info.tunnel_state);
                        if !info.tunnel_url.is_empty() {
                            line += &format!(" · {}", info.tunnel_url);
                        }
                        if !info.tunnel_message.is_empty() {
                            line += &format!(" · {}", info.tunnel_message);
                        }
                        ui.colored_label(color, line);
                    }
                }
                if info.restart_needed {
                    ui.colored_label(AMBER, tr("Saved: restart RadioMaster Server to use them."));
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Server users…")).clicked() {
                        users = true;
                    }
                    if ui.button(tr("Update, restart or stop…")).clicked() {
                        srv.admin.show_update = true;
                    }
                });
                ui.separator();
                ui.label(RichText::new(tr("Change your password")).strong());
                egui::Grid::new("srv_pw").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label(tr("Current password"));
                    ui.add(egui::TextEdit::singleline(&mut srv.pw_current).password(true).desired_width(240.0));
                    ui.end_row();
                    ui.label(tr("New password"));
                    ui.add(egui::TextEdit::singleline(&mut srv.pw_new).password(true).desired_width(240.0));
                    ui.end_row();
                    ui.label(tr("New password again"));
                    ui.add(egui::TextEdit::singleline(&mut srv.pw_again).password(true).desired_width(240.0));
                    ui.end_row();
                });
                ui.label(RichText::new(tr("Leave these empty to keep the password. At least 12 characters.")).small().color(GRAY));
                if !srv.settings_msg.1.is_empty() {
                    ui.colored_label(if srv.settings_msg.0 { GREEN } else { RED }, &srv.settings_msg.1);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(tr("Save")).strong())).clicked() {
                    save = true;
                }
                if ui.add_enabled(!busy, egui::Button::new(tr("Reload"))).clicked() {
                    reload = true;
                }
                if busy {
                    ui.spinner();
                }
            });
        });
        if !open {
            self.srv.show_settings = false;
            self.srv.pw_current.clear();
            self.srv.pw_new.clear();
            self.srv.pw_again.clear();
            return;
        }
        if reload {
            self.open_server_settings();
        }
        if users {
            self.open_server_users();
        }
        if save {
            let Some(info) = &self.srv.settings else { return };
            let password = if self.srv.pw_new.is_empty() && self.srv.pw_current.is_empty() {
                None
            } else if self.srv.pw_new != self.srv.pw_again {
                self.srv.settings_msg = (false, "The new passwords don't match".into());
                return;
            } else {
                Some((self.srv.pw_current.clone(), self.srv.pw_new.clone()))
            };
            let u = sync::ServerUpdate {
                name: info.name.clone(),
                autoplay: info.autoplay,
                audio: info.audio,
                outputs: info.outputs.clone(),
                password,
                https: info.https,
                portal: info.portal,
                portal_port: if info.portal_port == info.port { 0 } else { info.portal_port },
                acme: Some(info.acme),
                acme_domains: Some(info.acme_domains.clone()),
                acme_email: Some(info.acme_email.clone()),
                acme_staging: Some(info.acme_staging),
                tunnel: Some(info.tunnel),
                tunnel_token: (!self.srv.tunnel_token.trim().is_empty()).then(|| self.srv.tunnel_token.trim().to_string()),
            };
            self.srv.tunnel_token.clear();
            self.srv.settings_msg = (false, String::new());
            self.server_request(Some(u));
        }
    }
}

/// A profile name as a file name.
fn profile_file(name: &str) -> String {
    name.chars().map(|c| if c == ' ' { '-' } else { c }).filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}
