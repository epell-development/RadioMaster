//! The desktop app's side of RadioMaster Server: signs in, keeps the mirror playout in step with the
//! server (about four times a second) and sends it the actions taken in the app.

use crate::playout::sync::{self, digest, Cmd, Snapshot, TabsWire};
use crate::playout::Playout;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// A signed-in connection.
#[derive(Clone)]
pub struct Session {
    /// e.g. http://192.168.1.20:9259
    pub base: String,
    pub user: String,
    password: String,
    pub token: String,
    /// the station name the server was set up with
    pub name: String,
    pub version: String,
    /// SHA-256 of the self-signed certificate the user trusted for this server (empty: none needed)
    pub pin: String,
    /// the account is one of the server's administrators
    pub admin: bool,
    /// what the account may do on the server (it checks every call too)
    pub perms: crate::auth::Perms,
}

/// The address was typed without http:// or https://.
fn no_scheme(addr: &str) -> bool {
    let a = addr.trim();
    !a.starts_with("http://") && !a.starts_with("https://")
}

/// "radio.local" → "http://radio.local:9259"; an address with http:// or https:// is kept as typed.
pub fn normalize(addr: &str) -> String {
    let a = addr.trim().trim_end_matches('/');
    if a.starts_with("http://") || a.starts_with("https://") {
        return a.to_string();
    }
    let host_has_port =
        a.rsplit_once(':').is_some_and(|(h, p)| !h.is_empty() && !h.ends_with(':') && p.parse::<u16>().is_ok()) && !a.ends_with(']');
    if host_has_port {
        format!("http://{a}")
    } else {
        format!("http://{a}:{}", sync::DEFAULT_PORT)
    }
}

enum Fail {
    /// not signed in (any more)
    Auth(String),
    /// the account may not do that
    Denied(String),
    /// the server's copy changed first
    Conflict,
    Other(String),
}

impl Fail {
    fn text(&self) -> String {
        match self {
            Fail::Other(e) if e == crate::tls::UNTRUSTED || e == crate::tls::CHANGED => {
                "the server's HTTPS certificate isn't the one trusted: disconnect and connect again to check it".into()
            }
            Fail::Auth(e) | Fail::Denied(e) | Fail::Other(e) => e.clone(),
            Fail::Conflict => "the server's copy changed first".into(),
        }
    }
}

/// An HTTP(S) client that trusts public certificates and the one pinned for this server.
fn agent(pin: &str) -> ureq::Agent {
    agent_seen(pin, Default::default())
}

/// …also noting in `seen` the certificate the server showed.
fn agent_seen(pin: &str, seen: crate::tls::Seen) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .tls_config(crate::tls::client_config(pin, seen))
        .build()
}

fn call<T: DeserializeOwned>(agent: &ureq::Agent, base: &str, token: &str, path: &str, body: Option<serde_json::Value>) -> Result<T, Fail> {
    let url = format!("{base}/api/v1/{path}");
    let req = if body.is_some() { agent.post(&url) } else { agent.get(&url) };
    let req = req.set("Authorization", &format!("Bearer {token}"));
    let res = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match res {
        Ok(r) => r.into_json::<T>().map_err(|e| Fail::Other(format!("unexpected answer from the server: {e}"))),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {code}"));
            Err(match code {
                401 => Fail::Auth(msg),
                403 => Fail::Denied(msg),
                409 => Fail::Conflict,
                _ => Fail::Other(msg),
            })
        }
        // (without the URL, which the app shows already)
        Err(ureq::Error::Transport(t)) => Err(Fail::Other(match trust_mark(&t) {
            Some(mark) => mark.into(),
            None => match t.message() {
                Some(m) => format!("{} ({m})", t.kind()),
                None => t.kind().to_string(),
            },
        })),
    }
}

/// The certificate check's verdict (`tls::UNTRUSTED` or `tls::CHANGED`), found in the error's causes.
fn trust_mark(t: &ureq::Transport) -> Option<&'static str> {
    let mark = |m: &str| [crate::tls::UNTRUSTED, crate::tls::CHANGED].into_iter().find(|k| m == *k);
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(t);
    while let Some(e) = cause {
        // rustls' error travels inside an io::Error
        let inner = e.downcast_ref::<std::io::Error>().and_then(|io| io.get_ref()).map(|i| i as &(dyn std::error::Error + 'static));
        for e in [Some(e), inner].into_iter().flatten() {
            if let Some(rustls::Error::General(m)) = e.downcast_ref::<rustls::Error>() {
                if let Some(k) = mark(m) {
                    return Some(k);
                }
            }
        }
        cause = e.source();
    }
    None
}

#[derive(Deserialize)]
struct LoginReply {
    token: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    protocol: u32,
    #[serde(default)]
    admin: bool,
    /// (servers before v0.2.9 had one account, which could do everything)
    #[serde(default)]
    perms: crate::auth::Perms,
}

/// Why signing in didn't work.
#[derive(Debug, Clone)]
pub enum LoginError {
    Failed(String),
    /// The server's HTTPS certificate isn't signed by a public authority (a self-signed one), or isn't
    /// the one trusted before (`changed`). Signing in again with `fingerprint` as the pin trusts it.
    Untrusted {
        base: String,
        fingerprint: String,
        changed: bool,
    },
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Failed(m) => f.write_str(m),
            LoginError::Untrusted { base, changed: false, .. } => write!(f, "{base} uses a certificate this app doesn't know yet"),
            LoginError::Untrusted { base, changed: true, .. } => write!(f, "The certificate of {base} changed since you trusted it"),
        }
    }
}

/// Sign in (blocks; run it off the UI thread). `pin` is the certificate fingerprint trusted for this
/// server, if any. Without http:// or https:// in the address, the server is asked (without the
/// password) whether it uses HTTPS.
pub fn login(addr: &str, user: &str, password: &str, pin: &str) -> Result<Session, LoginError> {
    let mut base = normalize(addr);
    // ask first, without the password, so it never goes out unencrypted to a server that uses HTTPS
    if no_scheme(addr) {
        if let Err(Fail::Other(m)) = call::<serde_json::Value>(&agent(pin), &base, "", "ping", None) {
            if m.starts_with(crate::tls::HTTPS_ONLY) {
                base = base.replacen("http://", "https://", 1);
            }
        }
    }
    login_at(&base, user, password, pin)
}

fn login_at(base: &str, user: &str, password: &str, pin: &str) -> Result<Session, LoginError> {
    let seen = crate::tls::Seen::default();
    let body = serde_json::json!({ "user": user, "password": password });
    let r: LoginReply = call(&agent_seen(pin, seen.clone()), base, "", "login", Some(body)).map_err(|e| match e {
        Fail::Other(m) if m == crate::tls::UNTRUSTED || m == crate::tls::CHANGED => {
            LoginError::Untrusted { base: base.to_string(), fingerprint: seen.lock().clone(), changed: m == crate::tls::CHANGED }
        }
        Fail::Other(m) if ["Connection Failed", "Dns Failed", "timed out", "Network Error"].iter().any(|k| m.contains(k)) => {
            LoginError::Failed(format!("Can't reach {base} — check the address and that RadioMaster Server is running ({m})"))
        }
        e => LoginError::Failed(e.text()),
    })?;
    if r.protocol != sync::PROTOCOL {
        return Err(LoginError::Failed(format!(
            "The server runs RadioMaster {} and this app is {}: update both to the same version",
            r.version,
            env!("CARGO_PKG_VERSION")
        )));
    }
    // a pin is kept only for a certificate that needed one
    let pin = if base.starts_with("https://") { pin.to_string() } else { String::new() };
    Ok(Session {
        base: base.to_string(),
        user: user.to_string(),
        password: password.to_string(),
        token: r.token,
        name: r.name,
        version: r.version,
        pin,
        admin: r.admin,
        perms: r.perms,
    })
}

/// How the connection is doing (shown next to the logo).
#[derive(Clone, Default)]
pub struct LinkState {
    pub online: bool,
    /// why it isn't (empty when online)
    pub error: String,
    /// time of one poll, in ms
    pub rtt_ms: u32,
    pub since: Option<Instant>,
    /// edits that clashed with a change on the server and were undone
    pub conflicts: u32,
}

pub struct Link {
    stop: Arc<AtomicBool>,
    pub state: Arc<Mutex<LinkState>>,
    pub session: Session,
    /// the current sign-in (renewed when the server restarts)
    token: Arc<Mutex<String>>,
    /// set when the password is changed from this app, for signing in again later
    new_password: Arc<Mutex<Option<String>>>,
}

impl Link {
    /// Keep `core` (a mirror made with `Playout::mirror_of`) in step with the server.
    /// `cmds` is the receiving end of the mirror's link; `repaint` wakes the UI after each update.
    pub fn start(core: Arc<Mutex<Playout>>, session: Session, cmds: Receiver<Cmd>, repaint: Box<dyn Fn() + Send>) -> Link {
        let stop = Arc::new(AtomicBool::new(false));
        let token = Arc::new(Mutex::new(session.token.clone()));
        let new_password = Arc::new(Mutex::new(None));
        let state = Arc::new(Mutex::new(LinkState { since: Some(Instant::now()), ..Default::default() }));
        let mut s = Syncer {
            core,
            session: session.clone(),
            token: token.clone(),
            new_password: new_password.clone(),
            agent: agent(&session.pin),
            cmds,
            state: state.clone(),
            rev: 0,
            cfg_rev: 0,
            log_rev: 0,
            hist_rev: 0,
            lib_ver: 0,
            tabs_hash: None,
            cfg_hash: None,
            notify: String::new(),
            queued: vec![],
        };
        let st = stop.clone();
        thread::spawn(move || s.run(&st, repaint));
        Link { stop, state, session, token, new_password }
    }

    /// Stop syncing and sign out (in the background).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        Self::sign_out(self.current());
    }

    /// The sign-in as it stands (the token changes when the server restarts).
    pub fn current(&self) -> Session {
        let mut s = self.session.clone();
        s.token = self.token.lock().clone();
        s
    }

    /// The sign-in as it changes (for the audio connections, which outlive a server restart).
    pub fn token(&self) -> Arc<Mutex<String>> {
        self.token.clone()
    }

    /// The password was changed: sign in with the new one from now on.
    pub fn password_changed(&mut self, password: String) {
        self.session.password = password.clone();
        *self.new_password.lock() = Some(password);
    }

    /// The server's own settings (blocks; run it off the UI thread).
    pub fn server_info(s: &Session) -> Result<sync::ServerInfo, String> {
        call(&agent(&s.pin), &s.base, &s.token, "server", None).map_err(|e| e.text())
    }

    /// Change the server's own settings (blocks; run it off the UI thread).
    pub fn server_update(s: &Session, u: &sync::ServerUpdate) -> Result<sync::ServerInfo, String> {
        call(&agent(&s.pin), &s.base, &s.token, "server", Some(serde_json::to_value(u).unwrap_or_default())).map_err(|e| e.text())
    }

    /// The server's accounts (administrators only; blocks).
    pub fn users(s: &Session) -> Result<Vec<sync::UserWire>, String> {
        call(&agent(&s.pin), &s.base, &s.token, "users", None).map_err(|e| e.text())
    }

    /// Change the server's accounts (administrators only; blocks). Returns them as they are now.
    pub fn user_op(s: &Session, op: &sync::UserOp) -> Result<Vec<sync::UserWire>, String> {
        call(&agent(&s.pin), &s.base, &s.token, "users", Some(serde_json::to_value(op).unwrap_or_default())).map_err(|e| e.text())
    }

    /// Update, restart or stop the server: `what` is "update", "restart" or "stop" (administrators only; blocks).
    pub fn server_control(s: &Session, what: &str) -> Result<sync::ServerInfo, String> {
        call(&agent(&s.pin), &s.base, &s.token, &format!("server/{what}"), Some(serde_json::json!({}))).map_err(|e| e.text())
    }

    /// The server's play history (history.csv) for Reports (blocks).
    pub fn plays(s: &Session) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Plays {
            csv: String,
        }
        call::<Plays>(&agent(&s.pin), &s.base, &s.token, "plays", None).map(|p| p.csv).map_err(|e| e.text())
    }

    /// End a sign-in on the server (in the background).
    pub fn sign_out(s: Session) {
        thread::spawn(move || {
            let _ = call::<serde_json::Value>(&agent(&s.pin), &s.base, &s.token, "logout", Some(serde_json::json!({})));
        });
    }
}

// ------------------------------------------------------------ files

/// Where the files a dialog picks live: this computer, or the server the app is connected to
/// (its paths are the server's own). Every call reads the disk or the network: use it off the UI thread.
#[derive(Clone)]
pub enum Fs {
    Local,
    Server(Session),
}

impl Fs {
    pub fn is_server(&self) -> bool {
        matches!(self, Fs::Server(_))
    }

    /// A folder's folders and audio files (`all`: every file). An empty path: where browsing starts.
    pub fn list(&self, path: &std::path::Path, all: bool, library_folders: &[String]) -> sync::FsList {
        match self {
            Fs::Local => sync::list_folder(path.to_path_buf(), all, library_folders),
            Fs::Server(s) => {
                let body = serde_json::json!({ "path": path, "all": all });
                call(&agent(&s.pin), &s.base, &s.token, "fs/list", Some(body)).unwrap_or_else(|e| sync::FsList {
                    path: path.to_path_buf(),
                    error: e.text(),
                    ..Default::default()
                })
            }
        }
    }

    /// Audio files as they are, and the audio files in folders (sorted, sub-folders included).
    pub fn expand(&self, paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
        match self {
            Fs::Local => crate::library::expand_paths(paths),
            Fs::Server(s) => call(&agent(&s.pin), &s.base, &s.token, "fs/expand", Some(serde_json::to_value(paths).unwrap_or_default()))
                .unwrap_or_default(),
        }
    }

    pub fn audio_files_in(&self, dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        self.expand(vec![dir.to_path_buf()])
    }

    /// An M3U's rows (see `playout::parse_m3u`).
    pub fn m3u(&self, path: &std::path::Path) -> Vec<crate::playout::M3uRow> {
        match self {
            Fs::Local => crate::playout::parse_m3u(path),
            Fs::Server(s) => {
                call(&agent(&s.pin), &s.base, &s.token, "fs/m3u", Some(serde_json::json!({ "path": path }))).unwrap_or_default()
            }
        }
    }

    /// Copy a file from this computer into `dir` on the server; returns where it went there.
    pub fn upload(&self, file: &std::path::Path, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        let Fs::Server(s) = self else { return Err("not connected to a server".into()) };
        let data = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let len = data.metadata().map(|m| m.len()).unwrap_or(0);
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let url =
            format!("{}/api/v1/fs/upload?dir={}&name={}", s.base, urlencoding::encode(&dir.to_string_lossy()), urlencoding::encode(&name));
        // big files take a while on a slow line (and go as they are read, not held in memory)
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(600))
            .tls_config(crate::tls::client_config(&s.pin, Default::default()))
            .build();
        #[derive(Deserialize)]
        struct Put {
            path: std::path::PathBuf,
        }
        match agent.post(&url).set("Authorization", &format!("Bearer {}", s.token)).set("Content-Length", &len.to_string()).send(data) {
            Ok(r) => r.into_json::<Put>().map(|p| p.path).map_err(|e| e.to_string()),
            Err(ureq::Error::Status(code, r)) => Err(r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {code}"))),
            Err(e) => Err(e.to_string()),
        }
    }
}

// ------------------------------------------------------------ audio

/// How much of the server's mix is buffered before it plays (decoded MP3 frames of about 26 ms).
const LISTEN_PREBUFFER: usize = 12;
/// MP3 bitrate of the mic sent to the server.
const MIC_KBPS: u32 = 128;
/// The mic goes up in slices this long.
const MIC_SLICE: Duration = Duration::from_millis(100);

/// Plays the station (the server's on-air mix) on this machine's sound cards while connected,
/// through `engine`, the engine those sound cards play. Stops when dropped.
pub struct Listener {
    stop: Arc<AtomicBool>,
    engine: crate::engine::Engine,
}

impl Listener {
    /// `say` reports trouble in the station log.
    /// `pin`: the session's trusted certificate (see `Session::pin`).
    pub fn start(
        engine: crate::engine::Engine,
        base: String,
        pin: String,
        token: Arc<Mutex<String>>,
        say: Box<dyn Fn(String) + Send>,
    ) -> Listener {
        let (feed, src) = crate::engine::net_feed(LISTEN_PREBUFFER);
        engine.set_feed(Some(src));
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        thread::spawn(move || {
            // (no overall time limit: the answer never ends)
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(5))
                .timeout_read(Duration::from_secs(10))
                .tls_config(crate::tls::client_config(&pin, Default::default()))
                .build();
            let mut told = false;
            while !st.load(Ordering::Relaxed) {
                let t = token.lock().clone();
                match agent.get(&format!("{base}/api/v1/listen")).set("Authorization", &format!("Bearer {t}")).call() {
                    Ok(r) => {
                        told = false;
                        let mime = r.content_type().to_string();
                        let _ = crate::live::decode_stream(r.into_reader(), &mime, Some("mp3"), &mut |s| {
                            let _ = feed.try_send(s.to_vec());
                            !st.load(Ordering::Relaxed)
                        });
                    }
                    Err(ureq::Error::Status(404, _)) if !told => {
                        told = true;
                        say("This server doesn't send its audio to the app: update RadioMaster Server to this app's version".into());
                    }
                    // offline or signed out: the syncer signs in again, then this tries again
                    Err(_) => {}
                }
                for _ in 0..20 {
                    if st.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Listener { stop, engine }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.engine.set_feed(None);
    }
}

/// The mic while connected: captured here and sent to the server, which mixes it on air (with the
/// station's mic settings). Dropping the returned stream turns it off.
pub fn mic_uplink(base: String, pin: String, token: Arc<Mutex<String>>, device: &str) -> anyhow::Result<rodio::cpal::Stream> {
    let mut enc = crate::stream::mp3_encoder(MIC_KBPS)?;
    let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(256);
    let stream = crate::mic::capture(device, move |s| {
        let _ = tx.try_send(s.to_vec());
    })?;
    thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .tls_config(crate::tls::client_config(&pin, Default::default()))
            .build();
        let post = |path: &str, body: &[u8]| {
            agent
                .post(&format!("{base}/api/v1/{path}"))
                .set("Authorization", &format!("Bearer {}", token.lock()))
                .set("Content-Type", "audio/mpeg")
                .send_bytes(body)
        };
        let (mut pcm, mut out) = (Vec::<i16>::new(), Vec::new());
        let mut sent = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(b) => pcm.extend(b.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // a slow answer makes the next slice longer, so nothing piles up
            if sent.elapsed() >= MIC_SLICE && !pcm.is_empty() {
                sent = Instant::now();
                out.clear();
                if crate::stream::mp3_encode(&mut enc, &pcm, &mut out).is_ok() && !out.is_empty() {
                    let _ = post("mic", &out);
                }
                pcm.clear();
            }
        }
        let _ = post("mic/off", b"{}");
    });
    Ok(stream)
}

#[derive(Serialize)]
struct TabsPush<'a> {
    base: u64,
    tabs: &'a TabsWire,
}

#[derive(Deserialize)]
struct RevReply {
    rev: u64,
}

struct Syncer {
    core: Arc<Mutex<Playout>>,
    session: Session,
    token: Arc<Mutex<String>>,
    new_password: Arc<Mutex<Option<String>>>,
    agent: ureq::Agent,
    cmds: Receiver<Cmd>,
    state: Arc<Mutex<LinkState>>,
    rev: u64,
    cfg_rev: u64,
    log_rev: u64,
    hist_rev: u64,
    lib_ver: u64,
    /// fingerprints of the mirror's playlists / station settings as last agreed with the server
    /// (None until the first copy arrives, and after a clash: the server's copy then wins)
    tabs_hash: Option<u64>,
    cfg_hash: Option<u64>,
    /// the server's last status-bar message
    notify: String,
    queued: Vec<Cmd>,
}

impl Syncer {
    fn run(&mut self, stop: &AtomicBool, repaint: Box<dyn Fn() + Send>) {
        let mut backoff = Duration::from_millis(250);
        while !stop.load(Ordering::Relaxed) {
            let t0 = Instant::now();
            match self.round(stop) {
                Ok(()) => {
                    let mut s = self.state.lock();
                    if !s.online {
                        s.since = Some(Instant::now());
                    }
                    s.online = true;
                    s.error.clear();
                    s.rtt_ms = t0.elapsed().as_millis() as u32;
                    backoff = Duration::from_millis(250);
                }
                Err(Fail::Auth(_)) if !stop.load(Ordering::Relaxed) => {
                    // the server restarted or the sign-in expired: sign in again with the same details
                    if let Some(p) = self.new_password.lock().take() {
                        self.session.password = p;
                    }
                    let s = &self.session;
                    match login(&s.base, &s.user, &s.password, &s.pin) {
                        Ok(n) => {
                            *self.token.lock() = n.token.clone();
                            self.session = n;
                        }
                        Err(e) => self.offline(e.to_string(), &mut backoff),
                    }
                }
                Err(e) => self.offline(e.text(), &mut backoff),
            }
            repaint();
            if stop.load(Ordering::Relaxed) {
                break;
            }
            // wait for the next poll, or less when the app sends an action
            let wait = if self.state.lock().online { Duration::from_millis(250).saturating_sub(t0.elapsed()) } else { backoff };
            match self.cmds.recv_timeout(wait) {
                Ok(c) => self.queued.push(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn offline(&mut self, e: String, backoff: &mut Duration) {
        let was_online = {
            let mut s = self.state.lock();
            let was = s.online;
            if was {
                s.since = Some(Instant::now());
            }
            s.online = false;
            s.error = e.clone();
            was
        };
        // (the UI locks the playout before the state: never hold both here)
        if was_online {
            let c = self.core.lock();
            if c.link.is_some() {
                c.say(format!("Lost the connection to {}, retrying: {e}", self.session.name));
            }
        }
        *backoff = (*backoff * 2).clamp(Duration::from_secs(1), Duration::from_secs(10));
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Fail> {
        call(&self.agent, &self.session.base, &self.session.token, path, None)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: serde_json::Value) -> Result<T, Fail> {
        call(&self.agent, &self.session.base, &self.session.token, path, Some(body))
    }

    fn clash(&mut self, what: &str) {
        self.state.lock().conflicts += 1;
        self.core.lock().say(format!("{what} changed on the server at the same time: the server's version is shown"));
    }

    fn round(&mut self, stop: &AtomicBool) -> Result<(), Fail> {
        // 1. playlist and settings edits made in the app (the whole list goes up; the server refuses it
        //    when its own copy changed in the meantime)
        if self.tabs_hash.is_some() || self.cfg_hash.is_some() {
            let (tabs, cfg) = {
                let c = self.core.lock();
                (self.tabs_hash.map(|_| c.tabs_wire()), self.cfg_hash.map(|_| sync::station_cfg(&c.cfg)))
            };
            if let (Some(t), Some(h)) = (tabs, self.tabs_hash) {
                let now = digest(&t);
                if now != h {
                    match self.post::<RevReply>("tabs", serde_json::to_value(TabsPush { base: self.rev, tabs: &t }).unwrap_or_default()) {
                        Ok(r) => {
                            self.rev = r.rev;
                            self.tabs_hash = Some(now);
                        }
                        Err(Fail::Conflict) => {
                            self.tabs_hash = None;
                            self.rev = 0;
                            self.clash("A playlist");
                        }
                        Err(Fail::Denied(m)) => {
                            // not allowed: the server's copy comes back
                            self.tabs_hash = None;
                            self.rev = 0;
                            self.core.lock().say(format!("Not changed on the server: {m}"));
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            if let (Some(c), Some(h)) = (cfg, self.cfg_hash) {
                let now = digest(&c);
                if now != h {
                    match self.post::<RevReply>("config", serde_json::json!({ "base": self.cfg_rev, "cfg": c })) {
                        Ok(r) => {
                            self.cfg_rev = r.rev;
                            self.cfg_hash = Some(now);
                        }
                        Err(Fail::Conflict) => {
                            self.cfg_hash = None;
                            self.cfg_rev = 0;
                            self.clash("The settings");
                        }
                        Err(Fail::Denied(m)) => {
                            self.cfg_hash = None;
                            self.cfg_rev = 0;
                            self.core.lock().say(format!("Not changed on the server: {m}"));
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        // 2. actions taken in the app (after the edits, so "play the row I just added" finds it)
        self.queued.extend(self.cmds.try_iter());
        if !self.queued.is_empty() {
            let cmds = std::mem::take(&mut self.queued);
            match self.post::<serde_json::Value>("cmd", serde_json::to_value(&cmds).unwrap_or_default()) {
                Ok(r) => {
                    if r["refused"].as_u64().is_some_and(|n| n > 0) {
                        self.core.lock().say("Your server account may not do that");
                    }
                }
                Err(e) => {
                    match e {
                        // signed out: they go once the app has signed in again
                        Fail::Auth(_) => self.queued = cmds,
                        // an action that couldn't be sent now is dropped (pressing Next a minute late would surprise)
                        _ => self.core.lock().say(format!("Not sent to the server: {} action(s)", cmds.len())),
                    }
                    return Err(e);
                }
            }
        }
        // 3. what the server has now
        let snap: Snapshot =
            self.get(&format!("state?rev={}&cfg={}&log={}&hist={}", self.rev, self.cfg_rev, self.log_rev, self.hist_rev))?;
        let library = if snap.lib_ver != self.lib_ver { Some(self.get::<Vec<crate::library::Track>>("library")?) } else { None };
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut c = self.core.lock();
        if c.link.is_none() {
            return Ok(()); // disconnected meanwhile: this is the local playout again
        }
        let notify = snap.live.notify.clone();
        c.apply_live(snap.live);
        if notify != self.notify {
            *c.notify.lock() = notify.clone();
            self.notify = notify;
        }
        if let Some(t) = snap.tabs {
            // an edit made since the last round goes up first (next round); the server's copy waits
            if self.tabs_hash.is_none_or(|h| h == digest(&c.tabs_wire())) {
                c.set_tabs(t);
                self.tabs_hash = Some(digest(&c.tabs_wire()));
                self.rev = snap.rev;
            }
        }
        if let Some(cfg) = snap.cfg {
            if self.cfg_hash.is_none_or(|h| h == digest(&sync::station_cfg(&c.cfg))) {
                c.set_station_cfg(cfg);
                self.cfg_hash = Some(digest(&sync::station_cfg(&c.cfg)));
                self.cfg_rev = snap.cfg_rev;
            }
        }
        if let Some(l) = snap.log {
            c.apply_log(l);
            self.log_rev = snap.log_rev;
        }
        if let Some(h) = snap.history {
            c.apply_history(h);
            self.hist_rev = snap.hist_rev;
        }
        if let Some(lib) = library {
            c.set_library(lib);
            self.lib_ver = snap.lib_ver;
        }
        Ok(())
    }
}
