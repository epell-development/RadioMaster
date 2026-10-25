//! File → Connect to Server: sign in to a RadioMaster Server and run that station from this app.
//! The local session (playlists, settings, encoders) is put aside while connected, optionally saved
//! as a station profile first, and comes back on disconnect.
use super::*;
use crate::link::{Link, Session};
use crate::playout::sync;

/// The last server used (server_link.json; the password is never kept).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Remembered {
    address: String,
    user: String,
}

impl Remembered {
    fn path() -> PathBuf {
        data_dir().join("server_link.json")
    }
}

struct Connected {
    link: Link,
    /// this app's own session, put aside while connected
    local: Playout,
    /// the profile it was saved as (None: the server's playlists and settings stay after disconnecting)
    profile: Option<String>,
    /// local encoders that were on air, started again on disconnect
    encoders: Vec<usize>,
}

pub(super) struct ServerUi {
    show_connect: bool,
    address: String,
    user: String,
    password: String,
    busy: Option<mpsc::Receiver<Result<Session, String>>>,
    error: String,
    /// signed in, waiting for the answer to "save your local session?"
    ask: Option<Session>,
    profile: String,
    show_status: bool,
    /// "Add library folder" while connected: a folder on the server, typed in
    lib_folder: Option<String>,
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
            ask: None,
            profile: String::new(),
            show_status: false,
            lib_folder: None,
            conn: None,
        }
    }
}

impl ServerUi {
    pub(super) fn connected(&self) -> bool {
        self.conn.is_some()
    }

    /// A window of this tool is open (web views must hide under it).
    pub(super) fn window_open(&self) -> bool {
        self.show_connect || self.ask.is_some() || self.show_status || self.lib_folder.is_some()
    }
}

impl App {
    /// File menu entries.
    pub(super) fn server_menu(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if self.srv.connected() {
            if ui.button(tr("Server connection…")).clicked() {
                self.srv.show_status = true;
                ui.close_menu();
            }
            if ui.button(tr("Disconnect from Server")).clicked() {
                self.disconnect(core);
                ui.close_menu();
            }
        } else if ui.button(tr("Connect to Server…")).clicked() {
            self.srv.show_connect = true;
            self.srv.error.clear();
            ui.close_menu();
        }
    }

    /// "Add library folder…": a folder picker here, or while connected a folder on the server, typed in.
    pub(super) fn pick_library_folder(&mut self) {
        if self.srv.connected() {
            self.srv.lib_folder = Some(String::new());
        } else {
            self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::LibFolder));
        }
    }

    /// The engine the sound cards belong to: the local one, also while a server is mirrored.
    pub(super) fn output_engine(&self, core: &Playout) -> crate::engine::Engine {
        self.srv.conn.as_ref().map(|c| c.local.engine.clone()).unwrap_or_else(|| core.engine.clone())
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
        self.connect_window(ctx);
        self.save_session_window(ctx, core);
        self.server_status_window(ctx, core);
        self.server_folder_window(ctx, core);
    }

    fn connect_window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.srv.busy {
            match rx.try_recv() {
                Ok(Ok(s)) => {
                    self.srv.busy = None;
                    self.srv.show_connect = false;
                    self.srv.password.clear();
                    self.srv.profile = format!("Local session {}", Local::now().format("%Y-%m-%d %H%M"));
                    self.srv.ask = Some(s);
                }
                Ok(Err(e)) => {
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
                    RichText::new(tr("Without a port, 9259 is used. For a server on the internet, put it behind HTTPS and type its https:// address."))
                        .small()
                        .color(GRAY),
                );
                if !self.srv.error.is_empty() {
                    ui.colored_label(RED, &self.srv.error);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let ready = self.srv.busy.is_none() && !self.srv.address.trim().is_empty() && !self.srv.user.trim().is_empty() && !self.srv.password.is_empty();
                if ui.add_enabled(ready, egui::Button::new(tr("Connect"))).clicked() {
                    go = true;
                }
                if self.srv.busy.is_some() {
                    ui.spinner();
                    ui.label(tr("Signing in…"));
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.srv.show_connect = false;
                }
            });
        });
        if !open {
            self.srv.show_connect = false;
        }
        if go && self.srv.busy.is_none() && !self.srv.password.is_empty() {
            let (tx, rx) = mpsc::channel();
            let (a, u, p, ctx) =
                (self.srv.address.trim().to_string(), self.srv.user.trim().to_string(), self.srv.password.clone(), ctx.clone());
            thread::spawn(move || {
                let _ = tx.send(crate::link::login(&a, &u, &p));
                ctx.request_repaint();
            });
            self.srv.busy = Some(rx);
            self.srv.error.clear();
        }
    }

    fn save_session_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
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
        std::mem::swap(core, &mut local);
        let ctx = self.ctx.clone();
        let link = Link::start(self.core.clone(), s.clone(), rx, Box::new(move || ctx.request_repaint()));
        let r = Remembered { address: self.srv.address.trim().to_string(), user: s.user.clone() };
        let _ = std::fs::write(Remembered::path(), serde_json::to_string_pretty(&r).unwrap_or_default());
        core.say(format!("Connected to {} ({}) as {}", s.name, s.base, s.user));
        self.srv.conn = Some(Connected { link, local, profile, encoders });
        self.reset_playlist_view();
    }

    /// Back to the local session.
    pub(super) fn disconnect(&mut self, core: &mut Playout) {
        let Some(mut c) = self.srv.conn.take() else { return };
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
    }

    fn reset_playlist_view(&mut self) {
        self.view_pl = usize::MAX;
        self.sel.clear();
        self.anchor = None;
        self.followed = None;
        self.lib_sel.clear();
        self.lib_key = (String::new(), 0);
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
                    ui.label(tr("Signed in as"));
                    ui.label(&s.user);
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
                    ui.label(tr("Local session"));
                    ui.label(match &profile {
                        Some(p) => format!("saved as profile \"{p}\", restored on disconnect"),
                        None => "not saved: the server's playlists and settings stay after disconnecting".into(),
                    });
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr("Files, folders and the library are the server's. Previews, waveforms and the microphone need the files on this computer, so they don't work while connected."))
                        .small()
                        .color(GRAY),
                );
            });
            ui.separator();
            ui.horizontal(|ui| {
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

    fn server_folder_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some(mut folder) = self.srv.lib_folder.take() else { return };
        let mut open = true;
        let mut done = false;
        Self::window_for(ctx, "Add a library folder on the server", [440.0, 200.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            ui.label(tr("Type the folder as the server sees it, e.g. /srv/music or D:\\Music."));
            ui.add(egui::TextEdit::singleline(&mut folder).desired_width(380.0));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(!folder.trim().is_empty(), egui::Button::new(tr("Add and rescan"))).clicked() {
                    let f = folder.trim().to_string();
                    if !core.cfg.library_folders.contains(&f) {
                        core.cfg.library_folders.push(f);
                    }
                    core.start_scan();
                    done = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    done = true;
                }
            });
        });
        if open && !done {
            self.srv.lib_folder = Some(folder);
        }
    }
}

/// A profile name as a file name.
fn profile_file(name: &str) -> String {
    name.chars().map(|c| if c == ' ' { '-' } else { c }).filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}
