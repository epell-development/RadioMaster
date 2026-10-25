//! RadioMaster Server's HTTP API: what the desktop app's File → Connect to Server talks to.
//!
//!   GET  /api/v1/ping                      who is this (no sign-in needed)
//!   POST /api/v1/login   {user, password}  → {token, name, version, protocol}
//!   POST /api/v1/logout
//!   GET  /api/v1/state?rev=&cfg=&log=&hist=  → sync::Snapshot (lists only when they changed)
//!   GET  /api/v1/library                   → [Track]
//!   POST /api/v1/cmd     [sync::Cmd]
//!   POST /api/v1/tabs    {base, tabs}      → {rev}, or 409 when the server's playlists changed first
//!   POST /api/v1/config  {base, cfg}       → {rev}, or 409
//!
//! Every call but ping and login needs `Authorization: Bearer <token>`.

use crate::config::data_dir;
use crate::playout::sync::{self, digest, Cmd, Snapshot, TabsWire};
use crate::playout::Playout;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::Read,
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
    pub user: String,
    /// Argon2id; the password itself is never stored
    pub password_hash: String,
    pub bind: String,
    pub port: u16,
    /// start playing the on-air playlist when the server starts
    pub autoplay: bool,
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
    pub fn ready(&self) -> bool {
        !self.user.is_empty() && !self.password_hash.is_empty()
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

struct Api {
    core: Arc<Mutex<Playout>>,
    cfg: ServerCfg,
    revs: Mutex<Revs>,
    /// token → (user, last used)
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    failures: Mutex<(u32, Instant)>,
}

/// Sign-ins expire after this long without a call.
const TOKEN_IDLE: Duration = Duration::from_secs(24 * 3600);

/// Start listening. Returns the address actually bound (port 0 picks a free one).
pub fn spawn(core: Arc<Mutex<Playout>>, cfg: ServerCfg) -> Result<std::net::SocketAddr, String> {
    let server = tiny_http::Server::http((cfg.bind.as_str(), cfg.port)).map_err(|e| e.to_string())?;
    let addr = server.server_addr().to_ip().ok_or("not an IP address")?;
    // revisions start from the clock, so an app that was connected before a restart always gets fresh lists
    let start = chrono::Local::now().timestamp_millis() as u64;
    let mut revs =
        Revs { rev: start, tabs_hash: 0, tabs: Default::default(), cfg_rev: start, cfg_hash: 0, cfg: Default::default(), hist_rev: 0 };
    revs.refresh(&core);
    let api =
        Arc::new(Api { core, cfg, revs: Mutex::new(revs), tokens: Mutex::new(HashMap::new()), failures: Mutex::new((0, Instant::now())) });
    {
        let api = api.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            api.revs.lock().refresh(&api.core);
        });
    }
    let server = Arc::new(server);
    for _ in 0..4 {
        let (server, api) = (server.clone(), api.clone());
        thread::spawn(move || {
            while let Ok(req) = server.recv() {
                api.handle(req);
            }
        });
    }
    Ok(addr)
}

/// HTTP status and JSON body.
struct Reply(u16, String);

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
    fn handle(&self, mut req: tiny_http::Request) {
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
        let param = |k: &str| -> u64 {
            query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).and_then(|(_, b)| b.parse().ok())).unwrap_or(0)
        };
        let header = |name: &str| {
            req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string())
        };
        let gzip_ok = header("Accept-Encoding").is_some_and(|v| v.contains("gzip"));
        let token = header("Authorization").and_then(|v| v.strip_prefix("Bearer ").map(|t| t.trim().to_string()));
        let mut body = String::new();
        if req.as_reader().take(256 * 1024 * 1024).read_to_string(&mut body).is_err() {
            return respond(req, error(400, "unreadable request"), false);
        }
        let post = *req.method() == tiny_http::Method::Post;
        let reply = match (path, post) {
            ("/api/v1/ping", _) => json(
                200,
                serde_json::json!({ "app": "RadioMaster Server", "name": self.cfg.name, "version": env!("CARGO_PKG_VERSION"), "protocol": sync::PROTOCOL }),
            ),
            ("/api/v1/login", true) => self.login(&body),
            _ => match token.as_deref().map(|t| self.check(t)) {
                Some(true) => self.call(path, post, &body, &param, token.as_deref().unwrap_or("")),
                _ => error(401, "sign in first"),
            },
        };
        respond(req, reply, gzip_ok);
    }

    fn login(&self, body: &str) -> Reply {
        let Ok(l) = serde_json::from_str::<Login>(body) else { return error(400, "bad request") };
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
        if l.user != self.cfg.user || !crate::auth::verify(&self.cfg.password_hash, &l.password) {
            let mut f = self.failures.lock();
            f.0 = if f.1.elapsed() > Duration::from_secs(600) { 1 } else { f.0 + 1 };
            f.1 = Instant::now();
            self.core.lock().say(format!("Server: failed sign-in as '{}'", l.user));
            return error(401, "wrong username or password");
        }
        *self.failures.lock() = (0, Instant::now());
        let token: String = {
            use rand::Rng;
            let mut r = rand::thread_rng();
            (0..32).map(|_| format!("{:02x}", r.gen::<u8>())).collect()
        };
        let mut t = self.tokens.lock();
        t.retain(|_, (_, used)| used.elapsed() < TOKEN_IDLE);
        t.insert(token.clone(), (l.user.clone(), Instant::now()));
        drop(t);
        self.core.lock().say(format!("Server: {} connected", l.user));
        json(
            200,
            serde_json::json!({ "ok": true, "token": token, "name": self.cfg.name, "version": env!("CARGO_PKG_VERSION"), "protocol": sync::PROTOCOL }),
        )
    }

    fn check(&self, token: &str) -> bool {
        let mut t = self.tokens.lock();
        match t.get_mut(token) {
            Some((_, used)) if used.elapsed() < TOKEN_IDLE => {
                *used = Instant::now();
                true
            }
            _ => false,
        }
    }

    fn call(&self, path: &str, post: bool, body: &str, param: &dyn Fn(&str) -> u64, token: &str) -> Reply {
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
                let Ok(cmds) = serde_json::from_str::<Vec<Cmd>>(body) else { return error(400, "bad command") };
                let mut c = self.core.lock();
                for cmd in cmds {
                    c.run_cmd(cmd);
                }
                json(200, serde_json::json!({ "ok": true }))
            }
            ("/api/v1/tabs", true) => {
                let Ok(p) = serde_json::from_str::<TabsPush>(body) else { return error(400, "bad playlists") };
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
                let Ok(p) = serde_json::from_str::<CfgPush>(body) else { return error(400, "bad settings") };
                let mut r = self.revs.lock();
                if p.base != r.cfg_rev {
                    return error(409, "the settings changed on the server");
                }
                let mut c = self.core.lock();
                if digest(&sync::station_cfg(&c.cfg)) != r.cfg_hash {
                    return error(409, "the settings changed on the server");
                }
                c.set_station_cfg(p.cfg);
                c.save_all();
                drop(c);
                r.refresh(&self.core);
                json(200, serde_json::json!({ "ok": true, "rev": r.cfg_rev }))
            }
            _ => error(404, "unknown call"),
        }
    }
}

fn respond(req: tiny_http::Request, reply: Reply, gzip_ok: bool) {
    let Reply(code, text) = reply;
    let ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
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
        let addr = spawn(station.clone(), cfg).unwrap();
        let address = format!("127.0.0.1:{}", addr.port());

        assert!(crate::link::login(&address, "studio", "wrong password!!").is_err());
        let session = crate::link::login(&address, "studio", "correct horse battery").unwrap();
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
        link.stop();
    }
}
