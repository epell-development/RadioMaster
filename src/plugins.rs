//! Plugins: small programs (any language) that RadioMaster runs when things happen on air.
//!
//! Each plugin is a folder in `<data folder>/plugins/` with a `plugin.json`:
//!
//! ```json
//! { "name": "Now playing to Discord", "command": "python", "args": ["np.py"],
//!   "events": ["track_started"], "mode": "event" }
//! ```
//!
//! * `mode: "event"` (default) — the command runs once per event, with the event as JSON on stdin
//!   and in `RM_*` environment variables.
//! * `mode: "service"` — the command starts with RadioMaster and keeps running; every event is
//!   written to its stdin as one JSON line.
//!
//! Either kind can control RadioMaster by printing lines that start with `RM:` followed by a
//! station command, e.g. `RM: VOLUME 60` or `RM: PLAYLIST Evening` (the same commands as
//! "Add command…" rows). Everything else a plugin prints goes to the Log tab.

use crate::live::LogSink;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

/// Every event a plugin can subscribe to ("*" = all of them).
pub const EVENTS: [(&str, &str); 7] = [
    ("app_start", "RadioMaster started"),
    ("app_exit", "RadioMaster is closing"),
    ("track_started", "a song, voice track or jingle from the playlist started"),
    ("item_started", "another playlist row started (stream, line input, speech…)"),
    ("playback_stopped", "playback stopped"),
    ("hour", "the top of every hour"),
    ("encoder", "a stream encoder connected, failed or stopped"),
];

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PluginDef {
    pub name: String,
    pub description: String,
    pub command: String,
    pub args: Vec<String>,
    pub events: Vec<String>,
    /// "event" or "service"
    pub mode: String,
    #[serde(skip)]
    pub dir: PathBuf,
    #[serde(skip)]
    pub error: Option<String>,
}

impl PluginDef {
    pub fn is_service(&self) -> bool {
        self.mode.eq_ignore_ascii_case("service")
    }
    pub fn wants(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == "*" || e.eq_ignore_ascii_case(event))
    }
    fn id(&self) -> String {
        self.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| self.name.clone())
    }
}

struct Service {
    child: Child,
    stdin: Option<ChildStdin>,
}

pub struct Plugins {
    pub defs: Vec<PluginDef>,
    services: HashMap<String, Service>,
    cmd_tx: mpsc::Sender<(String, String)>,
    cmd_rx: mpsc::Receiver<(String, String)>,
    log: LogSink,
}

pub fn folder() -> PathBuf {
    crate::config::data_dir().join("plugins")
}

fn command(def: &PluginDef) -> Command {
    let mut c = Command::new(&def.command);
    c.args(&def.args).current_dir(&def.dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Read a plugin's output: `RM:` lines are commands, the rest is logged.
fn pump(name: String, out: impl std::io::Read + Send + 'static, tx: mpsc::Sender<(String, String)>, log: LogSink) {
    thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let l = line.trim();
            if let Some(cmd) = l.strip_prefix("RM:") {
                let _ = tx.send((name.clone(), cmd.trim().to_string()));
            } else if !l.is_empty() {
                log.lock().push(format!("[{name}] {l}"));
            }
        }
    });
}

impl Plugins {
    pub fn new(log: LogSink) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let mut p = Plugins { defs: vec![], services: HashMap::new(), cmd_tx, cmd_rx, log };
        p.reload(&[]);
        p
    }

    /// No plugins at all (a playout that mirrors a server: the server runs the plugins).
    pub fn none(log: LogSink) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        Plugins { defs: vec![], services: HashMap::new(), cmd_tx, cmd_rx, log }
    }

    /// Read plugin folders again; (re)start services that are switched on.
    pub fn reload(&mut self, off: &[String]) {
        self.stop_services();
        let _ = std::fs::create_dir_all(folder());
        let mut defs = vec![];
        if let Ok(rd) = std::fs::read_dir(folder()) {
            for e in rd.flatten() {
                let dir = e.path();
                let manifest = dir.join("plugin.json");
                if !manifest.exists() {
                    continue;
                }
                let mut d: PluginDef = match std::fs::read_to_string(&manifest)
                    .map_err(|e| e.to_string())
                    .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                {
                    Ok(d) => d,
                    Err(err) => PluginDef { error: Some(format!("plugin.json: {err}")), ..Default::default() },
                };
                d.dir = dir.clone();
                if d.name.is_empty() {
                    d.name = d.id();
                }
                if d.error.is_none() && d.command.trim().is_empty() {
                    d.error = Some("plugin.json has no \"command\"".into());
                }
                defs.push(d);
            }
        }
        defs.sort_by_key(|a| a.name.to_lowercase());
        self.defs = defs;
        self.start_services(off);
    }

    fn start_services(&mut self, off: &[String]) {
        for d in self.defs.iter_mut() {
            if !d.is_service() || d.error.is_some() || off.contains(&d.id()) {
                continue;
            }
            match command(d).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
                Ok(mut child) => {
                    if let Some(o) = child.stdout.take() {
                        pump(d.name.clone(), o, self.cmd_tx.clone(), self.log.clone());
                    }
                    if let Some(e) = child.stderr.take() {
                        pump(d.name.clone(), e, self.cmd_tx.clone(), self.log.clone());
                    }
                    let stdin = child.stdin.take();
                    self.services.insert(d.id(), Service { child, stdin });
                    self.log.lock().push(format!("Plugin started: {}", d.name));
                }
                Err(e) => d.error = Some(format!("couldn't start \"{}\": {e}", d.command)),
            }
        }
    }

    pub fn stop_services(&mut self) {
        for (_, mut s) in self.services.drain() {
            drop(s.stdin.take()); // closing stdin asks a well-behaved service to exit
            thread::sleep(Duration::from_millis(50));
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }

    pub fn running(&self, def: &PluginDef) -> bool {
        self.services.contains_key(&def.id())
    }

    pub fn id_of(def: &PluginDef) -> String {
        def.id()
    }

    /// Tell every interested plugin that something happened.
    pub fn emit(&mut self, event: &str, data: Value, off: &[String]) {
        if self.defs.is_empty() {
            return;
        }
        let mut msg = json!({ "event": event, "time": chrono::Local::now().to_rfc3339() });
        if let (Some(m), Some(d)) = (msg.as_object_mut(), data.as_object()) {
            for (k, v) in d {
                m.insert(k.clone(), v.clone());
            }
        }
        let line = msg.to_string();
        let mut dead = vec![];
        for d in &self.defs {
            if d.error.is_some() || !d.wants(event) || off.contains(&d.id()) {
                continue;
            }
            if d.is_service() {
                if let Some(s) = self.services.get_mut(&d.id()) {
                    let ok = s.stdin.as_mut().is_some_and(|w| writeln!(w, "{line}").and_then(|_| w.flush()).is_ok());
                    if !ok {
                        dead.push(d.id());
                    }
                }
                continue;
            }
            // event plugin: one run per event
            let mut c = command(d);
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("RM_EVENT", event);
            for (k, v) in msg.as_object().into_iter().flatten() {
                if let Some(s) = v.as_str() {
                    c.env(format!("RM_{}", k.to_uppercase()), s);
                } else if v.is_number() || v.is_boolean() {
                    c.env(format!("RM_{}", k.to_uppercase()), v.to_string());
                }
            }
            // started on a worker thread: launching a process can take tens of milliseconds on Windows,
            // and this runs while a track starts
            let (name, command, cmd_tx, log, line) =
                (d.name.clone(), d.command.clone(), self.cmd_tx.clone(), self.log.clone(), line.clone());
            thread::spawn(move || {
                match c.spawn() {
                    Ok(mut child) => {
                        if let Some(mut w) = child.stdin.take() {
                            let _ = writeln!(w, "{line}"); // one short line: fits the pipe, never blocks
                        }
                        if let Some(o) = child.stdout.take() {
                            pump(name.clone(), o, cmd_tx.clone(), log.clone());
                        }
                        if let Some(e) = child.stderr.take() {
                            pump(name.clone(), e, cmd_tx.clone(), log.clone());
                        }
                        // don't leave zombies; give up on a plugin that hangs
                        for _ in 0..600 {
                            match child.try_wait() {
                                Ok(Some(st)) => {
                                    if !st.success() {
                                        log.lock().push(format!("[{name}] exited with {st}"));
                                    }
                                    return;
                                }
                                Ok(None) => thread::sleep(Duration::from_millis(100)),
                                Err(_) => return,
                            }
                        }
                        let _ = child.kill();
                        log.lock().push(format!("[{name}] stopped after 60 s"));
                    }
                    Err(e) => log.lock().push(format!("[{name}] couldn't run \"{command}\": {e}")),
                }
            });
        }
        for id in dead {
            if let Some(mut s) = self.services.remove(&id) {
                let _ = s.child.kill();
                self.log.lock().push(format!("Plugin {id} stopped running"));
            }
        }
    }

    /// Station commands plugins have asked for since last time.
    pub fn take_commands(&self) -> Vec<(String, String)> {
        self.cmd_rx.try_iter().collect()
    }
}

impl Drop for Plugins {
    fn drop(&mut self) {
        self.stop_services();
    }
}

/// Write a small working example the first time the plugins folder is opened.
pub fn write_example() -> std::io::Result<PathBuf> {
    let dir = folder().join("example-now-playing");
    std::fs::create_dir_all(&dir)?;
    let manifest = dir.join("plugin.json");
    if !manifest.exists() {
        #[cfg(windows)]
        let (cmd, args, file, script) = (
            "powershell",
            r#"["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "now-playing.ps1"]"#,
            "now-playing.ps1",
            "# Writes the song that just started to nowplaying.txt (for OBS, a website, …)\r\n$e = [Console]::In.ReadLine() | ConvertFrom-Json\r\n\"$($e.artist) - $($e.title)\" | Set-Content -Encoding UTF8 nowplaying.txt\r\nWrite-Output \"wrote $($e.title)\"\r\n",
        );
        #[cfg(not(windows))]
        let (cmd, args, file, script) = (
            "sh",
            r#"["now-playing.sh"]"#,
            "now-playing.sh",
            "# Writes the song that just started to nowplaying.txt (for OBS, a website, ...)\nprintf '%s - %s\\n' \"$RM_ARTIST\" \"$RM_TITLE\" > nowplaying.txt\necho \"wrote $RM_TITLE\"\n",
        );
        std::fs::write(dir.join(file), script)?;
        std::fs::write(
            &manifest,
            format!(
                "{{\n  \"name\": \"Example: now playing file\",\n  \"description\": \"Writes the current song to nowplaying.txt in this folder.\",\n  \"command\": \"{cmd}\",\n  \"args\": {args},\n  \"events\": [\"track_started\"],\n  \"mode\": \"event\"\n}}\n"
            ),
        )?;
    }
    Ok(folder())
}

/// Open a folder in the file manager.
pub fn open_folder(p: &Path) {
    #[cfg(windows)]
    let _ = Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(p).spawn();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn event_plugin_runs_and_sends_commands() {
        std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join("rm_test_cfg"));
        let dir = folder().join("test-plugin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.sh"), "read line\necho \"$line\" > got.json\necho \"title is $RM_TITLE\"\necho 'RM: VOLUME 42'\n")
            .unwrap();
        std::fs::write(dir.join("plugin.json"), r#"{"name":"Test","command":"sh","args":["p.sh"],"events":["track_started"]}"#).unwrap();
        let log: LogSink = Arc::new(parking_lot::Mutex::new(vec![]));
        let mut p = Plugins::new(log.clone());
        assert!(p.defs.iter().any(|d| d.name == "Test" && d.error.is_none()));
        p.emit("hour", json!({}), &[]); // not subscribed: nothing runs
        p.emit("track_started", json!({ "title": "Bloom" }), &[]);
        let mut cmds = vec![];
        for _ in 0..50 {
            cmds.extend(p.take_commands());
            if !cmds.is_empty() && dir.join("got.json").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(cmds, vec![("Test".to_string(), "VOLUME 42".to_string())]);
        thread::sleep(Duration::from_millis(100));
        let got: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("got.json")).unwrap()).unwrap();
        assert_eq!(got["event"], "track_started");
        assert_eq!(got["title"], "Bloom");
        assert!(log.lock().iter().any(|l| l == "[Test] title is Bloom"), "{:?}", log.lock());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
