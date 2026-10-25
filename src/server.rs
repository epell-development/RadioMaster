//! RadioMaster Server's HTTP API: what the desktop app's File → Connect to Server talks to.
//!
//!   GET  /api/v1/ping                      who is this (no sign-in needed)
//!   POST /api/v1/login   {user, password}  → {token, name, version, protocol, admin, perms}
//!   POST /api/v1/logout
//!   GET  /api/v1/state?rev=&cfg=&log=&hist=  → sync::Snapshot (lists only when they changed)
//!   GET  /api/v1/library                   → [Track]
//!   POST /api/v1/cmd     [sync::Cmd]
//!   POST /api/v1/tabs    {base, tabs}      → {rev}, or 409 when the server's playlists changed first
//!   POST /api/v1/config  {base, cfg}       → {rev}, or 409
//!   GET  /api/v1/server                    → sync::ServerInfo (name, sound cards, …)
//!   POST /api/v1/server  sync::ServerUpdate → sync::ServerInfo
//!   POST /api/v1/server/update             download the latest release, install it and restart
//!   POST /api/v1/server/restart | /stop
//!   GET  /api/v1/users                     → [sync::UserWire]
//!   POST /api/v1/users   sync::UserOp      → [sync::UserWire]
//!   POST /api/v1/fs/list {path, all}       → sync::FsList (a folder on the server)
//!   POST /api/v1/fs/expand [path]          → [path]: the audio files, folders listed
//!   POST /api/v1/fs/m3u  {path}            → [M3uRow]
//!   GET  /api/v1/fs/file?path=             → the file itself
//!   POST /api/v1/fs/upload?dir=&name=      file bytes → {path}
//!   GET  /api/v1/plays                     → {csv}: the play history, for Reports
//!   GET  /api/v1/listen                    → the on-air mix as an endless MP3 stream (what the app hears)
//!   POST /api/v1/mic     MP3 bytes         the app's mic, a slice at a time; heard on air until mic/off
//!   POST /api/v1/mic/off
//!
//! Every call but ping and login needs `Authorization: Bearer <token>`. The accounts are the server's
//! operators.json (auth::Accounts); each call is checked against the signed-in account's permissions,
//! and only administrators change the server itself, its accounts and its program.
//!
//! With HTTPS on, the same API is served over TLS (see tls.rs). The optional web portal is a page on
//! its own port that signs in and uses this same API from the browser.

use crate::auth::{Accounts, Operator, Perms};
use crate::config::data_dir;
use crate::playout::sync::{self, digest, Cmd, ServerInfo, ServerUpdate, Snapshot, TabsWire, UserOp, UserWire};
use crate::playout::Playout;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

/// server.json in the server's data folder.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ServerCfg {
    /// shown in the app when it connects
    pub name: String,
    /// (before v0.2.9 the server had one account: it moves to operators.json as an administrator)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// Argon2id; the password itself is never stored
    #[serde(skip_serializing_if = "String::is_empty")]
    pub password_hash: String,
    pub bind: String,
    pub port: u16,
    /// start playing the on-air playlist when the server starts
    pub autoplay: bool,
    /// play through the server's sound cards (off: a silent clock, streaming only)
    pub audio: bool,
    /// serve the API (and the portal) over HTTPS
    pub https: bool,
    /// PEM certificate (chain) and private key; both empty: a self-signed certificate in the data folder
    pub cert_file: String,
    pub key_file: String,
    /// the web portal: a browser page for running the station
    pub portal: bool,
    pub portal_port: u16,
    /// look for a newer RadioMaster Server release on GitHub (administrators can then update it from the app)
    pub check_updates: bool,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            name: "RadioMaster Server".into(),
            user: String::new(),
            password_hash: String::new(),
            bind: "0.0.0.0".into(),
            port: sync::DEFAULT_PORT,
            autoplay: true,
            audio: true,
            https: false,
            cert_file: String::new(),
            key_file: String::new(),
            portal: false,
            portal_port: sync::DEFAULT_PORTAL_PORT,
            check_updates: true,
        }
    }
}

impl ServerCfg {
    pub fn path() -> std::path::PathBuf {
        data_dir().join("server.json")
    }
    pub fn load() -> Option<Self> {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok())
    }
    pub fn save(&self) -> std::io::Result<()> {
        std::fs::write(Self::path(), serde_json::to_string_pretty(self).unwrap_or_default())
    }
    /// Someone can sign in as an administrator.
    pub fn ready(&self) -> bool {
        (!self.user.is_empty() && !self.password_hash.is_empty()) || Accounts::load().has_admin()
    }

    /// The one account of a server set up before v0.2.9 becomes an administrator in `accounts`.
    /// True when there was one to move.
    pub fn move_user(&mut self, accounts: &mut Accounts) -> bool {
        if self.user.is_empty() || self.password_hash.is_empty() {
            return false;
        }
        if !accounts.users.iter().any(|u| u.name == self.user) {
            accounts.users.push(Operator {
                name: std::mem::take(&mut self.user),
                password_hash: std::mem::take(&mut self.password_hash),
                admin: true,
                perms: Perms::default(),
                disabled: false,
            });
        }
        self.user.clear();
        self.password_hash.clear();
        true
    }
}

/// What the apps are told changed, worked out off the playout lock.
struct Revs {
    rev: u64,
    tabs_hash: u64,
    tabs: Arc<TabsWire>,
    cfg_rev: u64,
    cfg_hash: u64,
    cfg: Arc<crate::config::Config>,
    hist_rev: u64,
}

impl Revs {
    /// Look at the playout again and bump the revisions of whatever changed.
    fn refresh(&mut self, core: &Mutex<Playout>) {
        let (tabs, cfg, hist) = {
            let c = core.lock();
            let h = c.history.last().map(|h| (c.history.len(), h.time.clone(), h.text.clone()));
            (c.tabs_wire(), sync::station_cfg(&c.cfg), h)
        };
        let th = digest(&tabs);
        if th != self.tabs_hash {
            self.tabs_hash = th;
            self.tabs = Arc::new(tabs);
            self.rev += 1;
        }
        let ch = digest(&cfg);
        if ch != self.cfg_hash {
            self.cfg_hash = ch;
            self.cfg = Arc::new(cfg);
            self.cfg_rev += 1;
        }
        self.hist_rev = digest(&hist);
    }
}

/// Asked of the server program by an administrator.
pub enum Control {
    Stop,
    /// stop and start again (after an update: the new version)
    Restart,
}

/// What the server program does for the API: its sound cards live on its main thread.
pub struct Host {
    /// stop or restart the whole server (saving everything first)
    pub control: Box<dyn Fn(Control) + Send + Sync>,
    /// open the sound cards again from `audio_devices` and `ServerCfg::audio`
    pub reapply_audio: Box<dyn Fn() + Send + Sync>,
    /// (device, status) as last opened
    pub audio_report: Arc<Mutex<Vec<(String, String)>>>,
    /// started with --no-audio
    pub forced_silent: bool,
}

/// The server's own update: the release found, and what is happening.
#[derive(Default)]
struct UpdateState {
    latest: Option<crate::selfupdate::Release>,
    /// e.g. "Downloading… 40%" (empty: nothing going on)
    state: String,
    busy: bool,
}

struct Api {
    core: Arc<Mutex<Playout>>,
    cfg: Mutex<ServerCfg>,
    host: Host,
    revs: Mutex<Revs>,
    accounts: Mutex<Accounts>,
    /// token → (user, last used)
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    update: Mutex<UpdateState>,
    failures: Mutex<(u32, Instant)>,
    /// what is running now (server.json may have changed since): (https, portal, portal port)
    running: (bool, bool, u16),
    /// of the certificate in use (empty without HTTPS)
    fingerprint: String,
    /// HTTPS with a certificate browsers trust: the portal tells them to use only HTTPS. (Not with a
    /// self-signed one, where HSTS would stop browsers from letting the user past their warning.)
    hsts: bool,
    /// token → the mic audio of that app, while its mic is on
    mics: Mutex<HashMap<String, Sender<Vec<u8>>>>,
}

/// Where the server is listening.
pub struct Listening {
    pub addr: std::net::SocketAddr,
    pub portal: Option<std::net::SocketAddr>,
    /// the certificate's SHA-256 (None without HTTPS)
    pub fingerprint: Option<String>,
    /// the certificate is the server's own (apps ask once whether to trust it)
    pub self_signed: bool,
}

/// A plain HTTP server for the API; with `tls`, it listens on loopback behind the TLS layer, which
/// takes the public port.
fn listen(bind: &str, port: u16, tls: Option<&crate::tls::ServerTls>) -> Result<(tiny_http::Server, std::net::SocketAddr), String> {
    match tls {
        None => {
            let server = tiny_http::Server::http((bind, port)).map_err(|e| e.to_string())?;
            let addr = server.server_addr().to_ip().ok_or("not an IP address")?;
            Ok((server, addr))
        }
        Some(t) => {
            let public = std::net::TcpListener::bind((bind, port)).map_err(|e| e.to_string())?;
            let addr = public.local_addr().map_err(|e| e.to_string())?;
            let server = tiny_http::Server::http(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
            let inner = server.server_addr().to_ip().ok_or("not an IP address")?;
            crate::tls::serve(public, inner, t.config.clone());
            Ok((server, addr))
        }
    }
}

/// Sign-ins expire after this long without a call.
const TOKEN_IDLE: Duration = Duration::from_secs(24 * 3600);

/// MP3 bitrate of the mix sent to the apps.
const LISTEN_KBPS: u32 = 192;
/// An app's mic goes off when nothing arrives for this long (the app quit or lost the network).
const MIC_IDLE: Duration = Duration::from_secs(3);
/// Mic audio waits for this many decoded MP3 frames (about 26 ms each) before it plays.
const MIC_PREBUFFER: usize = 6;

/// How often the server looks for a newer release.
const UPDATE_EVERY: Duration = Duration::from_secs(6 * 3600);

/// Start listening. Returns the addresses actually bound (port 0 picks a free one).
pub fn spawn(core: Arc<Mutex<Playout>>, mut cfg: ServerCfg, host: Host) -> Result<Listening, String> {
    let mut accounts = Accounts::load();
    if cfg.move_user(&mut accounts) {
        accounts.save()?;
    }
    let tls = if cfg.https { Some(crate::tls::server_tls(&cfg.cert_file, &cfg.key_file, &data_dir())?) } else { None };
    let (server, addr) = listen(&cfg.bind, cfg.port, tls.as_ref())?;
    let portal = if cfg.portal {
        let (s, a) =
            listen(&cfg.bind, cfg.portal_port, tls.as_ref()).map_err(|e| format!("web portal on port {}: {e}", cfg.portal_port))?;
        Some((s, a))
    } else {
        None
    };
    // revisions start from the clock, so an app that was connected before a restart always gets fresh lists
    let start = chrono::Local::now().timestamp_millis() as u64;
    let mut revs =
        Revs { rev: start, tabs_hash: 0, tabs: Default::default(), cfg_rev: start, cfg_hash: 0, cfg: Default::default(), hist_rev: 0 };
    revs.refresh(&core);
    let running = (cfg.https, cfg.portal, cfg.portal_port);
    let check_updates = cfg.check_updates;
    let api = Arc::new(Api {
        core,
        cfg: Mutex::new(cfg),
        host,
        revs: Mutex::new(revs),
        accounts: Mutex::new(accounts),
        tokens: Mutex::new(HashMap::new()),
        update: Default::default(),
        failures: Mutex::new((0, Instant::now())),
        running,
        fingerprint: tls.as_ref().map(|t| t.fingerprint.clone()).unwrap_or_default(),
        hsts: tls.as_ref().is_some_and(|t| !t.self_signed),
        mics: Mutex::new(HashMap::new()),
    });
    {
        let api = api.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            api.revs.lock().refresh(&api.core);
        });
    }
    if check_updates && !cfg!(test) {
        let api = api.clone();
        thread::spawn(move || loop {
            match crate::selfupdate::latest() {
                Ok(r) => {
                    let mut u = api.update.lock();
                    if crate::selfupdate::is_newer(&r.tag_name, crate::selfupdate::CURRENT)
                        && u.latest.as_ref().is_none_or(|o| o.tag_name != r.tag_name)
                    {
                        api.core.lock().say(format!(
                            "Server: RadioMaster Server {} is available (this is {})",
                            r.version(),
                            crate::selfupdate::CURRENT
                        ));
                    }
                    u.latest = Some(r);
                }
                Err(e) => api.core.lock().say(format!("Server: couldn't check for updates: {e}")),
            }
            thread::sleep(UPDATE_EVERY);
        });
    }
    let portal_addr = portal.as_ref().map(|p| p.1);
    let mut servers = vec![(Arc::new(server), false, 4)];
    if let Some((s, _)) = portal {
        servers.push((Arc::new(s), true, 2));
    }
    for (server, portal, workers) in servers {
        for _ in 0..workers {
            let (server, api) = (server.clone(), api.clone());
            thread::spawn(move || {
                while let Ok(req) = server.recv() {
                    api.handle(req, portal);
                }
            });
        }
    }
    Ok(Listening {
        addr,
        portal: portal_addr,
        fingerprint: tls.as_ref().map(|t| t.fingerprint.clone()),
        self_signed: tls.as_ref().is_some_and(|t| t.self_signed),
    })
}

/// HTTP status and JSON body.
struct Reply(u16, String);

/// The web portal's files.
const PORTAL_HTML: &str = include_str!("../assets/portal/index.html");
const PORTAL_JS: &str = include_str!("../assets/portal/portal.js");
const PORTAL_CSS: &str = include_str!("../assets/portal/portal.css");

fn json(code: u16, v: impl Serialize) -> Reply {
    Reply(code, serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()))
}

fn error(code: u16, msg: &str) -> Reply {
    json(code, serde_json::json!({ "ok": false, "error": msg }))
}

#[derive(Deserialize)]
struct Login {
    user: String,
    password: String,
}

#[derive(Deserialize)]
struct TabsPush {
    base: u64,
    tabs: TabsWire,
}

#[derive(Deserialize)]
struct CfgPush {
    base: u64,
    cfg: crate::config::Config,
}

impl Api {
    /// `portal`: the request came to the web portal's port, which also serves the page itself.
    fn handle(self: &Arc<Self>, mut req: tiny_http::Request, portal: bool) {
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
        if portal && *req.method() == tiny_http::Method::Get {
            let file = match path {
                "/" | "/index.html" => Some((PORTAL_HTML, "text/html; charset=utf-8")),
                "/portal.js" => Some((PORTAL_JS, "text/javascript; charset=utf-8")),
                "/portal.css" => Some((PORTAL_CSS, "text/css; charset=utf-8")),
                _ => None,
            };
            if let Some((text, ct)) = file {
                return respond_file(req, text, ct, self.hsts);
            }
        }
        let param = |k: &str| -> u64 { query_str(query, k).and_then(|v| v.parse().ok()).unwrap_or(0) };
        let header = |name: &str| {
            req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string())
        };
        let gzip_ok = header("Accept-Encoding").is_some_and(|v| v.contains("gzip"));
        let token = header("Authorization").and_then(|v| v.strip_prefix("Bearer ").map(|t| t.trim().to_string()));
        let post = *req.method() == tiny_http::Method::Post;
        if path == "/api/v1/listen" && !post {
            match token.filter(|t| self.check(t).is_some()) {
                Some(t) => self.listen(req, t),
                None => respond(req, error(401, "sign in first"), false),
            }
            return;
        }
        if path == "/api/v1/fs/file" && !post {
            let me = token.as_deref().and_then(|t| self.check(t));
            return match me {
                Some(me) => self.send_file(req, &me, &query_str(query, "path").unwrap_or_default()),
                None => respond(req, error(401, "sign in first"), false),
            };
        }
        let mut body = Vec::new();
        if req.as_reader().take(256 * 1024 * 1024).read_to_end(&mut body).is_err() {
            return respond(req, error(400, "unreadable request"), false);
        }
        let reply = match (path, post) {
            ("/api/v1/ping", _) => json(
                200,
                serde_json::json!({ "app": "RadioMaster Server", "name": self.cfg.lock().name, "version": env!("CARGO_PKG_VERSION"), "protocol": sync::PROTOCOL }),
            ),
            ("/api/v1/login", true) => self.login(&body),
            _ => match token.as_deref().and_then(|t| self.check(t)) {
                Some(me) => self.call(path, post, &body, query, &param, token.as_deref().unwrap_or(""), &me),
                None => error(401, "sign in first"),
            },
        };
        respond(req, reply, gzip_ok);
    }

    fn login(&self, body: &[u8]) -> Reply {
        let Ok(l) = serde_json::from_slice::<Login>(body) else { return error(400, "bad request") };
        // slow down password guessing: one second per recent failure, up to ten
        let wait = {
            let f = self.failures.lock();
            if f.1.elapsed() > Duration::from_secs(600) {
                0
            } else {
                f.0.min(10)
            }
        };
        thread::sleep(Duration::from_secs(wait as u64));
        let found = self.accounts.lock().login(&l.user, &l.password);
        let Some(me) = found else {
            let mut f = self.failures.lock();
            f.0 = if f.1.elapsed() > Duration::from_secs(600) { 1 } else { f.0 + 1 };
            f.1 = Instant::now();
            self.core.lock().say(format!("Server: failed sign-in as '{}'", l.user));
            return error(401, "wrong username or password");
        };
        *self.failures.lock() = (0, Instant::now());
        let token: String = {
            use rand::Rng;
            let mut r = rand::thread_rng();
            (0..32).map(|_| format!("{:02x}", r.gen::<u8>())).collect()
        };
        let mut t = self.tokens.lock();
        t.retain(|_, (_, used)| used.elapsed() < TOKEN_IDLE);
        t.insert(token.clone(), (me.name.clone(), Instant::now()));
        drop(t);
        self.core.lock().say(format!("Server: {} connected", me.name));
        json(
            200,
            serde_json::json!({ "ok": true, "token": token, "name": self.cfg.lock().name.clone(), "version": env!("CARGO_PKG_VERSION"),
                "protocol": sync::PROTOCOL, "admin": me.admin, "perms": me.effective() }),
        )
    }

    /// The account signed in with `token`, as it is now (None: not signed in, or the account was
    /// deleted or disabled since).
    fn check(&self, token: &str) -> Option<Operator> {
        let mut t = self.tokens.lock();
        let user = match t.get_mut(token) {
            Some((user, used)) if used.elapsed() < TOKEN_IDLE => {
                *used = Instant::now();
                user.clone()
            }
            _ => return None,
        };
        let me = self.accounts.lock().active(&user).cloned();
        if me.is_none() {
            t.remove(token);
        }
        me
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        self: &Arc<Self>,
        path: &str,
        post: bool,
        body: &[u8],
        query: &str,
        param: &dyn Fn(&str) -> u64,
        token: &str,
        me: &Operator,
    ) -> Reply {
        let p = me.effective();
        let files_ok = p.edit_playlists || p.jingles || p.settings || p.tool(crate::auth::tool::FILE_EXPLORER);
        let denied = |what: &str| error(403, &format!("your account ({}) may not {what}", me.name));
        match (path, post) {
            ("/api/v1/logout", true) => {
                if let Some((user, _)) = self.tokens.lock().remove(token) {
                    self.core.lock().say(format!("Server: {user} disconnected"));
                }
                json(200, serde_json::json!({ "ok": true }))
            }
            ("/api/v1/state", false) => {
                let (rev, tabs, cfg_rev, cfg, hist_rev) = {
                    let r = self.revs.lock();
                    (r.rev, r.tabs.clone(), r.cfg_rev, r.cfg.clone(), r.hist_rev)
                };
                let c = self.core.lock();
                let snap = Snapshot {
                    live: c.live_wire(),
                    rev,
                    tabs: (param("rev") != rev).then(|| (*tabs).clone()),
                    cfg_rev,
                    cfg: (param("cfg") != cfg_rev).then(|| (*cfg).clone()),
                    log_rev: c.log_total,
                    log: (param("log") != c.log_total).then(|| c.log_wire()),
                    hist_rev,
                    history: (param("hist") != hist_rev).then(|| c.history_wire()),
                    lib_ver: c.library_ver,
                };
                drop(c);
                json(200, snap)
            }
            ("/api/v1/library", false) => {
                let lib = self.core.lock().library.clone();
                json(200, lib)
            }
            ("/api/v1/cmd", true) => {
                let Ok(cmds) = serde_json::from_slice::<Vec<Cmd>>(body) else { return error(400, "bad command") };
                let mut c = self.core.lock();
                let mut refused = 0;
                for cmd in cmds {
                    if cmd.allowed(&p) {
                        c.run_cmd(cmd);
                    } else {
                        refused += 1;
                        c.say(format!("Server: {} may not {cmd:?}", me.name));
                    }
                }
                json(200, serde_json::json!({ "ok": true, "refused": refused }))
            }
            ("/api/v1/tabs", true) => {
                if !p.edit_playlists {
                    return denied("change playlists");
                }
                let Ok(p) = serde_json::from_slice::<TabsPush>(body) else { return error(400, "bad playlists") };
                let mut r = self.revs.lock();
                if p.base != r.rev {
                    return error(409, "the playlists changed on the server");
                }
                let mut c = self.core.lock();
                if digest(&c.tabs_wire()) != r.tabs_hash {
                    return error(409, "the playlists changed on the server");
                }
                c.set_tabs(p.tabs);
                c.save_tabs();
                drop(c);
                r.refresh(&self.core);
                json(200, serde_json::json!({ "ok": true, "rev": r.rev }))
            }
            ("/api/v1/config", true) => {
                let Ok(push) = serde_json::from_slice::<CfgPush>(body) else { return error(400, "bad settings") };
                let mut r = self.revs.lock();
                if push.base != r.cfg_rev {
                    return error(409, "the settings changed on the server");
                }
                let mut c = self.core.lock();
                let now = sync::station_cfg(&c.cfg);
                if digest(&now) != r.cfg_hash {
                    return error(409, "the settings changed on the server");
                }
                if let Some(part) = sync::cfg_denied(&now, &sync::station_cfg(&push.cfg), &p) {
                    return denied(&format!("change these settings ({part})"));
                }
                c.set_station_cfg(push.cfg);
                c.save_all();
                drop(c);
                r.refresh(&self.core);
                json(200, serde_json::json!({ "ok": true, "rev": r.cfg_rev }))
            }
            ("/api/v1/mic", true) if !p.playback => denied("use the mic"),
            ("/api/v1/mic", true) => self.mic(token, body.to_vec()),
            ("/api/v1/mic/off", true) => {
                // the mic's thread ends once its audio runs out
                self.mics.lock().remove(token);
                json(200, serde_json::json!({ "ok": true }))
            }
            ("/api/v1/server", false) => json(200, self.info(me)),
            ("/api/v1/server" | "/api/v1/server/update" | "/api/v1/server/restart" | "/api/v1/server/stop" | "/api/v1/users", _)
                if !me.admin =>
            {
                denied("manage the server")
            }
            ("/api/v1/server", true) => {
                let Ok(u) = serde_json::from_slice::<ServerUpdate>(body) else { return error(400, "bad server settings") };
                self.update(u, me)
            }
            ("/api/v1/server/update", true) => self.self_update(me),
            ("/api/v1/server/restart", true) | ("/api/v1/server/stop", true) => {
                let restart = path.ends_with("restart");
                self.core.lock().say(format!("Server: {} asked it to {}", me.name, if restart { "restart" } else { "stop" }));
                self.update.lock().state = if restart { "Restarting…" } else { "Stopping…" }.into();
                let api = self.clone();
                // after this answer has gone out
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(300));
                    (api.host.control)(if restart { Control::Restart } else { Control::Stop });
                });
                json(200, self.info(me))
            }
            ("/api/v1/users", false) => json(200, self.users()),
            ("/api/v1/users", true) => {
                let Ok(op) = serde_json::from_slice::<UserOp>(body) else { return error(400, "bad request") };
                match self.user_op(op, me) {
                    Ok(()) => json(200, self.users()),
                    Err(e) => error(400, &e),
                }
            }
            ("/api/v1/fs/list" | "/api/v1/fs/expand" | "/api/v1/fs/m3u", true) if !files_ok => denied("browse the server's files"),
            ("/api/v1/fs/list", true) => {
                #[derive(Deserialize)]
                struct Ask {
                    #[serde(default)]
                    path: std::path::PathBuf,
                    #[serde(default)]
                    all: bool,
                }
                let Ok(a) = serde_json::from_slice::<Ask>(body) else { return error(400, "bad request") };
                let folders = self.core.lock().cfg.library_folders.clone();
                json(200, sync::list_folder(a.path, a.all, &folders))
            }
            ("/api/v1/fs/expand", true) => {
                let Ok(paths) = serde_json::from_slice::<Vec<std::path::PathBuf>>(body) else { return error(400, "bad request") };
                json(200, crate::library::expand_paths(paths))
            }
            ("/api/v1/fs/m3u", true) => {
                #[derive(Deserialize)]
                struct Ask {
                    path: std::path::PathBuf,
                }
                let Ok(a) = serde_json::from_slice::<Ask>(body) else { return error(400, "bad request") };
                json(200, crate::playout::parse_m3u(&a.path))
            }
            ("/api/v1/fs/upload", true) => {
                if !(p.edit_playlists || p.jingles) {
                    return denied("upload files");
                }
                self.upload(&query_str(query, "dir").unwrap_or_default(), &query_str(query, "name").unwrap_or_default(), body, me)
            }
            ("/api/v1/plays", false) => {
                use crate::auth::tool;
                if !(p.tool(tool::REPORTS) || p.tool(tool::HISTORY)) {
                    return denied("see the play history");
                }
                json(200, serde_json::json!({ "csv": std::fs::read_to_string(data_dir().join("history.csv")).unwrap_or_default() }))
            }
            _ => error(404, "unknown call"),
        }
    }

    fn users(&self) -> Vec<UserWire> {
        self.accounts
            .lock()
            .users
            .iter()
            .map(|u| UserWire { name: u.name.clone(), admin: u.admin, perms: u.perms, disabled: u.disabled })
            .collect()
    }

    fn user_op(&self, op: UserOp, me: &Operator) -> Result<(), String> {
        let mut a = self.accounts.lock();
        let what = match op {
            UserOp::Add { name, password, admin, perms } => {
                a.add_with(&name, &password, admin, perms)?;
                format!("added {}", name.trim())
            }
            UserOp::Update { name, admin, perms, disabled } => {
                if name == me.name && (disabled || !admin) && me.admin {
                    return Err("You can't take away your own administrator access or disable yourself".into());
                }
                a.update(&name, admin, perms, disabled)?;
                format!("changed {name}")
            }
            UserOp::Password { name, password } => {
                a.set_password(&name, &password)?;
                format!("set a new password for {name}")
            }
            UserOp::Delete { name } => {
                if name == me.name {
                    return Err("You can't delete your own account".into());
                }
                a.delete(&name)?;
                format!("deleted {name}")
            }
        };
        drop(a);
        self.core.lock().say(format!("Server: {} {what}", me.name));
        Ok(())
    }

    /// Download and install the newer release, then start again (in the background).
    fn self_update(self: &Arc<Self>, me: &Operator) -> Reply {
        let asset = {
            let mut u = self.update.lock();
            if u.busy {
                return error(409, "an update is already running");
            }
            let Some(r) = u.latest.clone().filter(|r| crate::selfupdate::is_newer(&r.tag_name, crate::selfupdate::CURRENT)) else {
                return error(400, "this server is up to date");
            };
            let Some(a) = r.server_asset().cloned() else {
                return error(400, &format!("release {} has no RadioMaster Server package for this system", r.version()));
            };
            u.busy = true;
            u.state = "Downloading…".into();
            a
        };
        self.core.lock().say(format!("Server: {} started the update to {}", me.name, asset.name));
        let api = self.clone();
        thread::spawn(move || {
            let r = crate::selfupdate::install(&asset, &|s| api.update.lock().state = s);
            match r {
                Ok(()) => {
                    api.core.lock().say("Server: update installed, restarting");
                    api.update.lock().state = "Installed: restarting…".into();
                    (api.host.control)(Control::Restart);
                }
                Err(e) => {
                    api.core.lock().say(format!("Server: the update failed: {e}"));
                    let mut u = api.update.lock();
                    u.busy = false;
                    u.state = format!("The update failed: {e}");
                }
            }
        });
        json(200, self.info(me))
    }

    /// A file from this computer, put in a folder on the server (never over a file that is there).
    fn upload(&self, dir: &str, name: &str, body: &[u8], me: &Operator) -> Reply {
        use std::path::Path;
        let name = Path::new(name).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if dir.is_empty() || name.is_empty() || name.starts_with('.') {
            return error(400, "a folder and a file name are needed");
        }
        let dir = Path::new(dir);
        if !dir.is_dir() {
            return error(400, &format!("{} is not a folder on the server", dir.display()));
        }
        let mut target = dir.join(&name);
        let (stem, ext) = (
            target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            target.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
        );
        let mut n = 2;
        while target.exists() {
            target = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).and_then(|mut f| f.write_all(body));
        match written {
            Ok(()) => {
                self.core.lock().say(format!("Server: {} uploaded {}", me.name, target.display()));
                json(200, serde_json::json!({ "ok": true, "path": target }))
            }
            Err(e) => error(500, &format!("could not write {}: {e}", target.display())),
        }
    }

    /// A file's bytes, for previews, waveforms and copies. Others than administrators only get files
    /// the station uses: in a library folder, the library, a playlist or the cart wall.
    fn send_file(&self, req: tiny_http::Request, me: &Operator, path: &str) {
        let path = std::path::PathBuf::from(path);
        let ok = me.admin || {
            let c = self.core.lock();
            c.cfg.library_folders.iter().any(|f| path.starts_with(f))
                || c.library.iter().any(|t| t.path == path)
                || c.playlist.iter().chain(c.tabs.iter().flat_map(|t| t.entries.iter())).any(|e| e.track.path == path)
                || c.cfg.carts.iter().any(|k| std::path::Path::new(&k.path) == path)
        };
        if !ok {
            return respond(req, error(403, "that file isn't one the station uses"), false);
        }
        match std::fs::File::open(&path) {
            Ok(f) if path.is_file() => {
                let ct = header("Content-Type", "application/octet-stream");
                let _ = req.respond(tiny_http::Response::from_file(f).with_header(ct));
            }
            _ => respond(req, error(404, "no such file on the server"), false),
        }
    }

    fn user_of(&self, token: &str) -> String {
        self.tokens.lock().get(token).map(|(u, _)| u.clone()).unwrap_or_default()
    }

    /// Send the on-air mix to an app for as long as it listens (on a thread of its own, so the
    /// API's workers stay free).
    fn listen(self: &Arc<Self>, req: tiny_http::Request, token: String) {
        let api = self.clone();
        thread::spawn(move || {
            let mut enc = match crate::stream::mp3_encoder(LISTEN_KBPS) {
                Ok(e) => e,
                Err(e) => return respond(req, error(500, &format!("no MP3 encoder: {e}")), false),
            };
            let engine = api.core.lock().engine.clone();
            let mut w = req.into_writer();
            let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nCache-Control: no-cache, no-store\r\nConnection: close\r\n\r\n";
            if w.write_all(head.as_bytes()).and_then(|_| w.flush()).is_err() {
                return;
            }
            let (tx, rx) = crossbeam_channel::bounded::<Arc<Vec<i16>>>(64);
            engine.add_tap(tx);
            let mut out = Vec::new();
            let mut checked = Instant::now();
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(pcm) => {
                        // keep the delay short: skip what piled up while the network was slow
                        while rx.len() > 12 {
                            let _ = rx.try_recv();
                        }
                        out.clear();
                        if crate::stream::mp3_encode(&mut enc, &pcm, &mut out).is_err() {
                            break;
                        }
                        if !out.is_empty() && w.write_all(&out).and_then(|_| w.flush()).is_err() {
                            break; // the app went away
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // signed out (or the sign-in expired): stop sending
                if checked.elapsed() > Duration::from_secs(5) {
                    checked = Instant::now();
                    if !api.tokens.lock().contains_key(&token) {
                        break;
                    }
                }
            }
        });
    }

    /// A slice of an app's mic audio (MP3). The first one turns the mic on.
    fn mic(self: &Arc<Self>, token: &str, data: Vec<u8>) -> Reply {
        let mut mics = self.mics.lock();
        let data = match mics.get(token) {
            Some(tx) => match tx.send(data) {
                Ok(()) => return json(200, serde_json::json!({ "ok": true })),
                // that mic went off meanwhile (nothing came for a while): start again
                Err(e) => e.0,
            },
            None => data,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let _ = tx.send(data);
        mics.insert(token.to_string(), tx);
        drop(mics);
        let (api, user) = (self.clone(), self.user_of(token));
        thread::spawn(move || api.run_mic(rx, user));
        json(200, serde_json::json!({ "ok": true }))
    }

    /// Decode one app's mic and mix it in, with the station's mic settings, until it goes off.
    fn run_mic(&self, rx: Receiver<Vec<u8>>, user: String) {
        let engine = self.core.lock().engine.clone();
        let (feed, src) = crate::engine::net_feed(MIC_PREBUFFER);
        let id = engine.add_net_mic(src);
        let level = || {
            let c = self.core.lock();
            engine.set_mic(true, c.cfg.mic_gain, c.cfg.mic_duck_level());
        };
        level();
        self.core.lock().say(format!("Server: {user}'s mic is on"));
        let mut last = Instant::now();
        let reader = ChanReader { rx, cur: Vec::new(), pos: 0 };
        let r = crate::live::decode_stream(Box::new(reader), "audio/mpeg", Some("mp3"), &mut |s| {
            let _ = feed.try_send(s.to_vec());
            // follow changes to the mic volume and ducking
            if last.elapsed() > Duration::from_millis(500) {
                last = Instant::now();
                level();
            }
            true
        });
        if engine.remove_net_mic(id) == 0 {
            engine.set_mic(false, 1.0, 1.0);
        }
        let c = self.core.lock();
        match r {
            Ok(()) => c.say(format!("Server: {user}'s mic is off")),
            Err(e) => c.say(format!("Server: {user}'s mic stopped: {e}")),
        }
    }

    fn info(&self, me: &Operator) -> ServerInfo {
        let c = self.cfg.lock().clone();
        let (latest, can_update, update_state) = {
            let u = self.update.lock();
            match u.latest.as_ref().filter(|r| crate::selfupdate::is_newer(&r.tag_name, crate::selfupdate::CURRENT)) {
                Some(r) => (r.version(), r.server_asset().is_some(), u.state.clone()),
                None => (String::new(), false, u.state.clone()),
            }
        };
        ServerInfo {
            name: c.name,
            user: me.name.clone(),
            admin: me.admin,
            version: env!("CARGO_PKG_VERSION").into(),
            latest,
            can_update,
            update_state,
            port: c.port,
            autoplay: c.autoplay,
            audio: c.audio,
            https: c.https,
            fingerprint: self.fingerprint.clone(),
            portal: c.portal,
            portal_port: c.portal_port,
            restart_needed: (c.https, c.portal, c.portal_port) != self.running,
            devices: crate::audio::list_outputs(),
            outputs: self.core.lock().cfg.audio_devices.outputs.clone(),
            report: self.host.audio_report.lock().clone(),
            forced_silent: self.host.forced_silent,
        }
    }

    fn update(&self, u: ServerUpdate, me: &Operator) -> Reply {
        let mut c = self.cfg.lock().clone();
        if let Some((current, new)) = &u.password {
            // your own password
            if !crate::auth::verify(&me.password_hash, current) {
                return error(403, "the current password is wrong");
            }
            if let Err(e) = self.accounts.lock().set_password(&me.name, new) {
                return error(400, &e);
            }
        }
        if !u.name.trim().is_empty() {
            c.name = u.name.trim().to_string();
        }
        c.autoplay = u.autoplay;
        // these take effect when the server starts again
        c.https = u.https;
        c.portal = u.portal;
        if u.portal_port != 0 {
            c.portal_port = u.portal_port;
        }
        let audio_changed = c.audio != u.audio || self.core.lock().cfg.audio_devices.outputs != u.outputs;
        c.audio = u.audio;
        if let Err(e) = c.save() {
            return error(500, &format!("could not save server.json: {e}"));
        }
        *self.cfg.lock() = c;
        if audio_changed {
            {
                let mut p = self.core.lock();
                p.cfg.audio_devices.outputs = u.outputs;
                p.save_all();
            }
            (self.host.reapply_audio)();
            // the main thread opens the cards; give it a moment so the answer shows the result
            thread::sleep(Duration::from_millis(700));
        }
        self.core.lock().say("Server: settings changed");
        json(200, self.info(me))
    }
}

/// A value from a query string, URL-decoded.
fn query_str(query: &str, k: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k))
        .map(|(_, v)| urlencoding::decode(&v.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default())
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

/// One of the portal's own files, with headers that keep it from being framed or mixed with other sites.
fn respond_file(req: tiny_http::Request, text: &'static str, content_type: &str, hsts: bool) {
    let mut r = tiny_http::Response::from_string(text)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-cache"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("X-Frame-Options", "DENY"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header(
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'",
        ));
    if hsts {
        r.add_header(header("Strict-Transport-Security", "max-age=31536000"));
    }
    let _ = req.respond(r);
}

/// Reads the mic slices as one stream; ends when the mic goes off or nothing arrives for `MIC_IDLE`.
struct ChanReader {
    rx: Receiver<Vec<u8>>,
    cur: Vec<u8>,
    pos: usize,
}

impl Read for ChanReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.cur.len() {
            match self.rx.recv_timeout(MIC_IDLE) {
                Ok(b) => {
                    self.cur = b;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.cur.len() - self.pos);
        buf[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn respond(req: tiny_http::Request, reply: Reply, gzip_ok: bool) {
    let Reply(code, text) = reply;
    let ct = header("Content-Type", "application/json");
    if gzip_ok && text.len() > 8 * 1024 {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        if gz.write_all(text.as_bytes()).is_ok() {
            if let Ok(data) = gz.finish() {
                let ce = tiny_http::Header::from_bytes(&b"Content-Encoding"[..], &b"gzip"[..]).unwrap();
                let _ = req.respond(tiny_http::Response::from_data(data).with_status_code(code).with_header(ct).with_header(ce));
                return;
            }
        }
    }
    let _ = req.respond(tiny_http::Response::from_string(text).with_status_code(code).with_header(ct));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::items::PlEntry;
    use crate::library::Track;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn row(name: &str) -> PlEntry {
        PlEntry::file(Track { path: format!("/srv/music/{name}.mp3").into(), title: name.into(), ..Default::default() })
    }

    /// How long until what reaches `rx` gets loud (None: not within `secs`).
    fn sound_after(rx: &crossbeam_channel::Receiver<Arc<Vec<i16>>>, secs: f32) -> Option<Duration> {
        let t = Instant::now();
        while t.elapsed().as_secs_f32() < secs {
            if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
                if b.iter().any(|x| x.saturating_abs() > 2000) {
                    return Some(t.elapsed());
                }
            }
        }
        None
    }

    fn drain(rx: &crossbeam_channel::Receiver<Arc<Vec<i16>>>) {
        thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
    }

    fn sine(hz: f32) -> crate::engine::Pcm {
        Box::new((0..).flat_map(move |k: u64| {
            let v = (k as f32 * hz * std::f32::consts::TAU / crate::engine::SR as f32).sin() * 0.5;
            [v, v]
        }))
    }

    /// The broadcaster hears the station in the app, and the app's mic goes on air on the server.
    #[test]
    fn app_hears_the_station_and_its_mic_goes_on_air() {
        crate::config::set_data_dir(std::env::temp_dir().join(format!("rm-server-test-{}", std::process::id())));
        let station_engine = Engine::new_detached();
        let _clock = station_engine.start_null_driver();
        let station = Arc::new(Mutex::new(Playout::new(station_engine.clone(), crate::config::Config::default())));
        let cfg = ServerCfg {
            user: "dj".into(),
            password_hash: crate::auth::hash("a long enough password").unwrap(),
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        };
        let host =
            Host { control: Box::new(|_| {}), reapply_audio: Box::new(|| {}), audio_report: Default::default(), forced_silent: true };
        let addr = spawn(station.clone(), cfg, host).unwrap().addr;
        let s = crate::link::login(&format!("127.0.0.1:{}", addr.port()), "dj", "a long enough password", "").unwrap();
        let token = Arc::new(Mutex::new(s.token.clone()));

        // the app's own engine (the one its sound cards play) plays what the station plays
        let here = Engine::new_detached();
        let _cards = here.start_null_driver();
        let (tx, heard) = crossbeam_channel::bounded(512);
        here.add_tap(tx);
        let listener = crate::link::Listener::start(here.clone(), s.base.clone(), s.pin.clone(), token.clone(), Box::new(|_| {}));
        thread::sleep(Duration::from_secs(2));
        drain(&heard);
        station_engine.set_volume(1.0);
        station_engine.play_source(sine(440.0), None, 0.0, crate::engine::CueSpec::default(), None, true);
        let delay = sound_after(&heard, 10.0).expect("the app should hear the station");
        assert!(delay < Duration::from_secs(2), "heard {delay:?} late");
        drop(listener);
        drain(&heard);
        assert!(sound_after(&heard, 1.5).is_none(), "silent again once it stops listening");

        // the music stops and the app's mic (sent as MP3 slices) is heard on the server
        station_engine.stop(0.0);
        let (tx, on_air) = crossbeam_channel::bounded(512);
        station_engine.add_tap(tx);
        drain(&on_air);
        let mut enc = crate::stream::mp3_encoder(128).unwrap();
        let mut mic = sine(220.0);
        let agent = ureq::agent();
        let post = |path: &str, body: &[u8]| {
            agent.post(&format!("{}/api/v1/{path}", s.base)).set("Authorization", &format!("Bearer {}", s.token)).send_bytes(body)
        };
        let sender = {
            let stop = Arc::new(AtomicBool::new(false));
            let st = stop.clone();
            let base = s.base.clone();
            let t = s.token.clone();
            let h = thread::spawn(move || {
                let agent = ureq::agent();
                while !st.load(Ordering::Relaxed) {
                    let pcm: Vec<i16> = (&mut mic).take(4410 * 2).map(|x| (x * 32767.0) as i16).collect();
                    let mut out = Vec::new();
                    crate::stream::mp3_encode(&mut enc, &pcm, &mut out).unwrap();
                    if !out.is_empty() {
                        agent.post(&format!("{base}/api/v1/mic")).set("Authorization", &format!("Bearer {t}")).send_bytes(&out).unwrap();
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            });
            (stop, h)
        };
        let delay = sound_after(&on_air, 10.0).expect("the mic should be on air");
        sender.0.store(true, Ordering::Relaxed);
        sender.1.join().unwrap();
        post("mic/off", b"{}").unwrap();
        wait_for("the mic to go off", || station.lock().notify.lock().contains("mic is off"));
        drain(&on_air);
        assert!(sound_after(&on_air, 1.5).is_none(), "silent once the mic is off");
    }

    /// A server and a desktop mirror talking over HTTP: sign-in, both directions of playlist sync,
    /// settings, and forwarded actions.
    #[test]
    fn app_and_server_stay_in_step() {
        crate::config::set_data_dir(std::env::temp_dir().join(format!("rm-server-test-{}", std::process::id())));
        let station = Arc::new(Mutex::new(Playout::new(Engine::new_detached(), crate::config::Config::default())));
        station.lock().append_to_tab(0, vec![row("one"), row("two")]);
        let cfg = ServerCfg {
            user: "studio".into(),
            password_hash: crate::auth::hash("correct horse battery").unwrap(),
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        };
        let host =
            Host { control: Box::new(|_| {}), reapply_audio: Box::new(|| {}), audio_report: Default::default(), forced_silent: true };
        let addr = spawn(station.clone(), cfg, host).unwrap().addr;
        let address = format!("127.0.0.1:{}", addr.port());

        assert!(crate::link::login(&address, "studio", "wrong password!!", "").is_err());
        let session = crate::link::login(&address, "studio", "correct horse battery", "").unwrap();
        assert_eq!(session.name, "RadioMaster Server");

        let local = Playout::new(Engine::new_detached(), crate::config::Config::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mirror = Arc::new(Mutex::new(Playout::mirror_of(&local, tx)));
        let link = crate::link::Link::start(mirror.clone(), session, rx, Box::new(|| {}));

        // the server's playlist reaches the app
        wait_for("the first copy", || mirror.lock().playlist.len() == 2);
        assert!(link.state.lock().online);
        // an edit in the app reaches the server
        mirror.lock().append_to_tab(0, vec![row("three")]);
        wait_for("the edit on the server", || station.lock().playlist.len() == 3);
        // a change on the server reaches the app
        station.lock().new_tab(Some("Evening".into()));
        wait_for("the new tab in the app", || mirror.lock().tabs.len() == 2);
        // settings, both ways
        mirror.lock().cfg.crossfade_secs = 6.5;
        wait_for("the setting on the server", || station.lock().cfg.crossfade_secs == 6.5);
        // forwarded actions
        mirror.lock().set_volume(0.25);
        wait_for("the volume on the server", || (station.lock().cfg.volume - 0.25).abs() < 1e-6);
        // the app's own interface language never goes to the server
        mirror.lock().cfg.language = "de".into();
        thread::sleep(Duration::from_millis(800));
        assert_eq!(station.lock().cfg.language, "");
        // the server's own settings window: rename, wrong then right password change
        let s = link.current();
        let mut info = crate::link::Link::server_info(&s).unwrap();
        assert_eq!(info.user, "studio");
        assert!(info.forced_silent);
        info.name = "Night Desk".into();
        let mut u = ServerUpdate { name: info.name, autoplay: false, audio: true, outputs: vec![], password: None, ..Default::default() };
        assert_eq!(crate::link::Link::server_update(&s, &u).unwrap().name, "Night Desk");
        u.password = Some(("not the password".into(), "a brand new password".into()));
        assert!(crate::link::Link::server_update(&s, &u).is_err());
        u.password = Some(("correct horse battery".into(), "a brand new password".into()));
        assert!(!crate::link::Link::server_update(&s, &u).unwrap().autoplay);
        assert!(crate::link::login(&address, "studio", "a brand new password", "").is_ok());
        link.stop();
    }

    fn start_plain(station: Arc<Mutex<Playout>>, user: &str, password: &str) -> (String, Arc<std::sync::atomic::AtomicU8>) {
        crate::config::set_data_dir(std::env::temp_dir().join(format!("rm-server-test-{}", std::process::id())));
        let cfg = ServerCfg {
            user: user.into(),
            password_hash: crate::auth::hash(password).unwrap(),
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        };
        // what the server program was asked to do: 1 stop, 2 restart
        let asked = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let a = asked.clone();
        let host = Host {
            control: Box::new(move |c| a.store(if matches!(c, Control::Stop) { 1 } else { 2 }, Ordering::Relaxed)),
            reapply_audio: Box::new(|| {}),
            audio_report: Default::default(),
            forced_silent: true,
        };
        (format!("127.0.0.1:{}", spawn(station, cfg, host).unwrap().addr.port()), asked)
    }

    /// An administrator adds a presenter on the server; the server holds the presenter to their
    /// permissions, and only administrators manage accounts and the server itself.
    #[test]
    fn server_accounts_and_permissions() {
        use crate::playout::sync::UserOp;
        let station = Arc::new(Mutex::new(Playout::new(Engine::new_detached(), crate::config::Config::default())));
        station.lock().append_to_tab(0, vec![row("one")]);
        let (address, asked) = start_plain(station.clone(), "chief", "the chief's password");
        let admin = crate::link::login(&address, "chief", "the chief's password", "").unwrap();
        assert!(admin.admin);
        let presenter = crate::auth::Perms::presenter();
        let users = crate::link::Link::user_op(
            &admin,
            &UserOp::Add { name: "night dj".into(), password: "a presenter password".into(), admin: false, perms: presenter },
        )
        .unwrap();
        assert!(users.iter().any(|u| u.name == "night dj" && !u.admin));
        let dj = crate::link::login(&address, "night dj", "a presenter password", "").unwrap();
        assert!(!dj.admin && !dj.perms.settings && dj.perms.edit_playlists);

        // the presenter can't manage the server or its accounts
        assert!(crate::link::Link::users(&dj).unwrap_err().contains("may not"));
        assert!(crate::link::Link::server_control(&dj, "stop").is_err());
        assert_eq!(asked.load(Ordering::Relaxed), 0);
        // …but can run the playlists, through a mirror like the app's
        let local = Playout::new(Engine::new_detached(), crate::config::Config::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mirror = Arc::new(Mutex::new(Playout::mirror_of(&local, tx)));
        let link = crate::link::Link::start(mirror.clone(), dj.clone(), rx, Box::new(|| {}));
        // (other tests may have left playlists in the shared data folder)
        let n = station.lock().playlist.len();
        wait_for("the first copy", || mirror.lock().playlist.len() == n);
        mirror.lock().append_to_tab(0, vec![row("two")]);
        wait_for("the presenter's edit on the server", || station.lock().playlist.len() == n + 1);
        // a setting the presenter may not change is refused, and the server's value comes back
        mirror.lock().cfg.crossfade_secs = 11.0;
        wait_for("the refusal", || mirror.lock().notify.lock().contains("may not change these settings"));
        wait_for("the server's value again", || mirror.lock().cfg.crossfade_secs != 11.0);
        assert_ne!(station.lock().cfg.crossfade_secs, 11.0);
        link.stop();
        // starting an encoder is refused; Next goes through
        let r: serde_json::Value = ureq::post(&format!("{}/api/v1/cmd", dj.base))
            .set("Authorization", &format!("Bearer {}", dj.token))
            .send_json(serde_json::json!([{ "cmd": "encoder_start", "i": 0 }, { "cmd": "next" }]))
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(r["refused"], 1);

        // disabled: signed out at once, and can't sign in again
        crate::link::Link::user_op(&admin, &UserOp::Update { name: "night dj".into(), admin: false, perms: presenter, disabled: true })
            .unwrap();
        assert!(crate::link::Link::server_info(&dj).is_err());
        assert!(crate::link::login(&address, "night dj", "a presenter password", "").is_err());
        // administrators can't lock themselves out
        assert!(crate::link::Link::user_op(&admin, &UserOp::Delete { name: "chief".into() }).is_err());
        // restart goes to the server program
        let info = crate::link::Link::server_control(&admin, "restart").unwrap();
        assert!(info.admin && info.update_state.contains("Restarting"));
        wait_for("the restart request", || asked.load(Ordering::Relaxed) == 2);
    }

    /// The app browses the server's folders, uploads into them and fetches a file back.
    #[test]
    fn server_files() {
        let station = Arc::new(Mutex::new(Playout::new(Engine::new_detached(), crate::config::Config::default())));
        let music = std::env::temp_dir().join(format!("rm-server-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&music);
        std::fs::create_dir_all(music.join("Jingles")).unwrap();
        std::fs::write(music.join("a song.mp3"), b"not really audio").unwrap();
        std::fs::write(music.join("notes.txt"), b"hello").unwrap();
        std::fs::write(music.join("Jingles").join("id.mp3"), b"jingle").unwrap();
        station.lock().cfg.library_folders = vec![music.to_string_lossy().to_string()];
        let (address, _) = start_plain(station.clone(), "files admin", "the files password");
        let s = crate::link::login(&address, "files admin", "the files password", "").unwrap();
        let fs = crate::link::Fs::Server(s.clone());

        // browsing starts in the first library folder
        let list = fs.list(std::path::Path::new(""), false, &[]);
        assert_eq!(list.path, music);
        let names: Vec<String> = list.entries.iter().map(|e| e.path.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["Jingles", "a song.mp3"], "folders first, audio only");
        assert!(list.places.contains(&music));
        assert_eq!(fs.list(&music, true, &[]).entries.len(), 3, "every file");
        assert_eq!(fs.audio_files_in(&music).len(), 2);

        // an upload lands in the folder, never over a file that's there
        let local = std::env::temp_dir().join(format!("rm-upload-{}.mp3", std::process::id()));
        std::fs::write(&local, b"from this computer").unwrap();
        let up = fs.upload(&local, &music.join("Jingles")).unwrap();
        assert_eq!(std::fs::read(&up).unwrap(), b"from this computer");
        let again = fs.upload(&local, &music.join("Jingles")).unwrap();
        assert_ne!(up, again);
        assert!(fs.upload(&local, &music.join("no such folder")).is_err());

        // a file comes back as it is
        let url = format!("{}/api/v1/fs/file?path={}", s.base, urlencoding::encode(&music.join("a song.mp3").to_string_lossy()));
        let r = ureq::get(&url).set("Authorization", &format!("Bearer {}", s.token)).call().unwrap();
        assert_eq!(r.into_string().unwrap(), "not really audio");
        assert!(ureq::get(&url).call().is_err(), "only when signed in");
        assert!(crate::link::Link::plays(&s).is_ok());
        let _ = std::fs::remove_dir_all(&music);
        let _ = std::fs::remove_file(&local);
    }

    /// HTTPS with the server's own certificate: the app has to trust it once, finds the https://
    /// address by itself, and refuses a different certificate later. The web portal is served too.
    #[test]
    fn https_and_portal() {
        let dir = std::env::temp_dir().join(format!("rm-server-tls-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::config::set_data_dir(dir.clone());
        let station = Arc::new(Mutex::new(Playout::new(Engine::new_detached(), crate::config::Config::default())));
        station.lock().append_to_tab(0, vec![row("one")]);
        let cfg = ServerCfg {
            user: "studio".into(),
            password_hash: crate::auth::hash("correct horse battery").unwrap(),
            bind: "127.0.0.1".into(),
            port: 0,
            https: true,
            portal: true,
            portal_port: 0,
            ..Default::default()
        };
        let host =
            || Host { control: Box::new(|_| {}), reapply_audio: Box::new(|| {}), audio_report: Default::default(), forced_silent: true };
        let l = spawn(station.clone(), cfg.clone(), host()).unwrap();
        assert!(l.self_signed);
        let fp = l.fingerprint.clone().unwrap();
        let address = format!("127.0.0.1:{}", l.addr.port());

        // typed without https://: plain HTTP is answered with "uses HTTPS", then the certificate is unknown
        let first = crate::link::login(&address, "studio", "correct horse battery", "").err().unwrap();
        let crate::link::LoginError::Untrusted { base, fingerprint, changed } = first else { panic!("{first:?}") };
        assert_eq!(base, format!("https://{address}"));
        assert_eq!(fingerprint, fp);
        assert!(!changed);
        // trusted: signed in over HTTPS, and the sync runs over it
        let session = crate::link::login(&address, "studio", "correct horse battery", &fp).unwrap();
        assert!(session.base.starts_with("https://"));
        assert_eq!(session.pin, fp);
        let info = crate::link::Link::server_info(&session).unwrap();
        assert!(info.https && info.portal && !info.restart_needed);
        assert_eq!(info.fingerprint, fp);
        let local = Playout::new(Engine::new_detached(), crate::config::Config::default());
        let (tx, rx) = crossbeam_channel::unbounded();
        let mirror = Arc::new(Mutex::new(Playout::mirror_of(&local, tx)));
        let link = crate::link::Link::start(mirror.clone(), session, rx, Box::new(|| {}));
        wait_for("the playlist over HTTPS", || mirror.lock().playlist.len() == 1);
        mirror.lock().append_to_tab(0, vec![row("two")]);
        wait_for("the edit on the server over HTTPS", || station.lock().playlist.len() == 2);
        link.stop();

        // a different certificate for the same address is refused
        let other = crate::link::login(&address, "studio", "correct horse battery", "00:11").err().unwrap();
        assert!(matches!(other, crate::link::LoginError::Untrusted { changed: true, .. }), "{other:?}");

        // the portal: its page, and the API beside it
        let portal = format!("https://127.0.0.1:{}", l.portal.unwrap().port());
        let agent = ureq::AgentBuilder::new().tls_config(crate::tls::client_config(&fp, Default::default())).build();
        let page = agent.get(&format!("{portal}/")).call().unwrap();
        assert!(page.header("Content-Security-Policy").is_some());
        // no HSTS with a self-signed certificate: browsers would no longer let the user past their warning
        assert!(page.header("Strict-Transport-Security").is_none());
        assert!(page.into_string().unwrap().contains("RadioMaster Web Portal"));
        assert!(agent.get(&format!("{portal}/portal.js")).call().unwrap().into_string().unwrap().contains("api/v1"));
        let ping: serde_json::Value = agent.get(&format!("{portal}/api/v1/ping")).call().unwrap().into_json().unwrap();
        assert_eq!(ping["app"], "RadioMaster Server");
        // the API port doesn't serve the page
        assert!(agent.get(&format!("https://{address}/")).call().is_err());
        // a browser typing http:// is sent to https://
        let plain = ureq::AgentBuilder::new().redirects(0).build();
        let r = plain.get(&format!("http://127.0.0.1:{}/", l.portal.unwrap().port())).call().unwrap();
        assert_eq!(r.status(), 301);
        assert!(r.header("Location").unwrap().starts_with("https://127.0.0.1:"));

        // the certificate is kept: the same one after a restart
        let again = spawn(station, cfg, host()).unwrap();
        assert_eq!(again.fingerprint.unwrap(), fp);
    }

    /// The station's sound reaches the app over HTTPS too, through the pinned certificate, and keeps
    /// flowing while the app sends nothing.
    #[test]
    fn listen_over_https() {
        let dir = std::env::temp_dir().join(format!("rm-server-tls-listen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::config::set_data_dir(dir);
        let station_engine = Engine::new_detached();
        let _clock = station_engine.start_null_driver();
        let station = Arc::new(Mutex::new(Playout::new(station_engine.clone(), crate::config::Config::default())));
        let cfg = ServerCfg {
            user: "dj".into(),
            password_hash: crate::auth::hash("a long enough password").unwrap(),
            bind: "127.0.0.1".into(),
            port: 0,
            https: true,
            ..Default::default()
        };
        let host =
            Host { control: Box::new(|_| {}), reapply_audio: Box::new(|| {}), audio_report: Default::default(), forced_silent: true };
        let l = spawn(station, cfg, host).unwrap();
        let s =
            crate::link::login(&format!("127.0.0.1:{}", l.addr.port()), "dj", "a long enough password", &l.fingerprint.unwrap()).unwrap();
        let here = Engine::new_detached();
        let _cards = here.start_null_driver();
        let (tx, heard) = crossbeam_channel::bounded(512);
        here.add_tap(tx);
        let listener = crate::link::Listener::start(here, s.base.clone(), s.pin.clone(), Arc::new(Mutex::new(s.token)), Box::new(|_| {}));
        thread::sleep(Duration::from_secs(2));
        drain(&heard);
        station_engine.set_volume(1.0);
        station_engine.play_source(sine(440.0), None, 0.0, crate::engine::CueSpec::default(), None, true);
        assert!(sound_after(&heard, 10.0).is_some(), "the app should hear the station over HTTPS");
        drop(listener);
    }
}
