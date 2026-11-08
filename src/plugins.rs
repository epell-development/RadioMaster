//! Plugins: small programs (any language) that RadioMaster runs when things happen on air, and
//! that can add their own controls to the app.
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
//! A plugin talks back by printing lines that start with `RM:`:
//! * `RM: VOLUME 60` — a station command (the same commands as "Add command…" rows);
//! * `RM: {"do": "…", …}` — a JSON message: add menu items, toolbar buttons, playlist right-click
//!   items or a window of controls, show a notice, ask for the playlist, put files in it…
//!   (see `PLUGINS.md` for the full list).
//!
//! The manifest can declare menu items, buttons, a panel and settings up front, so even a plugin
//! that only runs on events can have them. Everything else a plugin prints goes to the Log tab.

use crate::live::LogSink;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashMap},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

/// Every event a plugin can subscribe to ("*" = all of them).
pub const EVENTS: [(&str, &str); 11] = [
    ("app_start", "RadioMaster started"),
    ("app_exit", "RadioMaster is closing"),
    ("track_started", "a song, voice track or jingle from the playlist started"),
    ("item_started", "another playlist row started (stream, line input, speech…)"),
    ("playback_stopped", "playback stopped"),
    ("paused", "playback was paused"),
    ("resumed", "playback carried on after a pause"),
    ("cart", "a pad on the cart wall was fired"),
    ("mic", "the microphone was switched on or off"),
    ("hour", "the top of every hour"),
    ("encoder", "a stream encoder connected, failed or stopped"),
];

/// Events sent to one plugin only, whatever its "events" list says.
pub const DIRECT_EVENTS: [(&str, &str); 3] = [
    ("action", "someone used one of the plugin's menu items, buttons or controls, or a PLUGIN command"),
    ("reply", "the answer to a \"get\" request"),
    ("settings", "the plugin's settings (when it starts and whenever they change)"),
];

/// A menu item, toolbar button or playlist right-click item a plugin adds.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct UiItem {
    /// sent back in the "action" event
    pub id: String,
    pub label: String,
    pub tooltip: String,
    /// a station command to run instead of sending an "action" event
    pub command: String,
}

/// A setting shown under the plugin in Settings → Plugins.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Setting {
    pub id: String,
    pub label: String,
    /// text (default), password, number, bool, choice
    #[serde(rename = "type")]
    pub kind: String,
    pub default: Value,
    /// for "choice"
    pub options: Vec<String>,
    pub help: String,
}

/// A window of controls declared in plugin.json (a plugin can also send one later).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PanelDef {
    pub title: String,
    pub items: Vec<Value>,
    /// open it when RadioMaster starts
    pub open: bool,
}

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
    pub menu: Vec<UiItem>,
    pub buttons: Vec<UiItem>,
    pub playlist_menu: Vec<UiItem>,
    pub settings: Vec<Setting>,
    pub panel: Option<PanelDef>,
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

/// What a plugin has added to the app (from plugin.json, then replaced by what it sends).
#[derive(Clone, Debug, Default)]
pub struct PluginUi {
    pub name: String,
    pub menu: Vec<UiItem>,
    pub buttons: Vec<UiItem>,
    pub playlist_menu: Vec<UiItem>,
    pub panel_title: String,
    /// the panel's controls (JSON objects with a "type"; see PLUGINS.md)
    pub panel: Vec<Value>,
    pub open: bool,
    /// a short line the plugin shows under its name in Settings → Plugins
    pub status: String,
    /// current value of each input control, by id (what the user typed or picked)
    pub values: Map<String, Value>,
    /// the value the plugin last sent for each control: a panel sent again with the same value
    /// doesn't undo what the user is typing
    sent: HashMap<String, Value>,
}

impl PluginUi {
    pub fn has_panel(&self) -> bool {
        !self.panel.is_empty()
    }

    fn take_values(&mut self, items: &[Value]) {
        for w in items {
            if let (Some(id), Some(v)) = (w.get("id").and_then(Value::as_str), w.get("value")) {
                if self.sent.get(id) != Some(v) {
                    self.values.insert(id.to_string(), v.clone());
                    self.sent.insert(id.to_string(), v.clone());
                }
            }
            if let Some(inner) = w.get("items").and_then(Value::as_array) {
                self.take_values(inner);
            }
        }
    }
}

/// Merge `fields` into the control with this id (anywhere in the panel).
fn set_widget(items: &mut [Value], id: &str, fields: &Map<String, Value>) -> bool {
    for w in items.iter_mut() {
        if w.get("id").and_then(Value::as_str) == Some(id) {
            if let Some(o) = w.as_object_mut() {
                for (k, v) in fields {
                    o.insert(k.clone(), v.clone());
                }
            }
            return true;
        }
        if let Some(inner) = w.get_mut("items").and_then(Value::as_array_mut) {
            if set_widget(inner, id, fields) {
                return true;
            }
        }
    }
    false
}

fn items_of(v: &Value) -> Vec<UiItem> {
    serde_json::from_value(v.get("items").cloned().unwrap_or(Value::Null)).unwrap_or_default()
}

struct Service {
    child: Child,
    stdin: Option<ChildStdin>,
}

/// (plugin id, plugin name, what it printed after "RM:")
type Line = (String, String, String);

pub struct Plugins {
    pub defs: Vec<PluginDef>,
    services: HashMap<String, Service>,
    cmd_tx: mpsc::Sender<Line>,
    cmd_rx: mpsc::Receiver<Line>,
    log: LogSink,
    /// what each plugin has added to the app, by plugin id
    pub ui: BTreeMap<String, PluginUi>,
    /// saved settings, by plugin id (Config::plugin_settings)
    saved: BTreeMap<String, Map<String, Value>>,
}

pub fn folder() -> PathBuf {
    crate::config::data_dir().join("plugins")
}

fn env_key(k: &str) -> String {
    k.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect()
}

fn env_val(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(v.to_string()),
        _ => None,
    }
}

fn command(def: &PluginDef, settings: &Map<String, Value>) -> Command {
    let mut c = Command::new(&def.command);
    c.args(&def.args).current_dir(&def.dir);
    for (k, v) in settings {
        if let Some(s) = env_val(v) {
            c.env(format!("RM_SETTING_{}", env_key(k)), s);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Read a plugin's output: `RM:` lines are commands and messages, the rest is logged.
fn pump(id: String, name: String, out: impl std::io::Read + Send + 'static, tx: mpsc::Sender<Line>, log: LogSink) {
    thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let l = line.trim();
            if let Some(cmd) = l.strip_prefix("RM:") {
                let _ = tx.send((id.clone(), name.clone(), cmd.trim().to_string()));
            } else if !l.is_empty() {
                log.lock().push(format!("[{name}] {l}"));
            }
        }
    });
}

impl Plugins {
    pub fn new(log: LogSink, off: &[String], saved: BTreeMap<String, Map<String, Value>>) -> Self {
        let mut p = Self::none(log);
        p.saved = saved;
        p.reload(off);
        p
    }

    /// No plugins at all (a playout that mirrors a server: the server runs the plugins).
    pub fn none(log: LogSink) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        Plugins { defs: vec![], services: HashMap::new(), cmd_tx, cmd_rx, log, ui: BTreeMap::new(), saved: BTreeMap::new() }
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
        // what each plugin adds to the app starts as its plugin.json says (a panel that is open stays open)
        let mut ui = BTreeMap::new();
        for d in defs.iter().filter(|d| d.error.is_none() && !off.contains(&d.id())) {
            let was_open = self.ui.get(&d.id()).is_some_and(|u| u.open);
            let mut u = PluginUi {
                name: d.name.clone(),
                menu: d.menu.clone(),
                buttons: d.buttons.clone(),
                playlist_menu: d.playlist_menu.clone(),
                ..Default::default()
            };
            if let Some(p) = &d.panel {
                u.panel_title = if p.title.is_empty() { d.name.clone() } else { p.title.clone() };
                u.panel = p.items.clone();
                u.open = p.open || was_open;
                let items = u.panel.clone();
                u.take_values(&items);
            }
            ui.insert(d.id(), u);
        }
        self.ui = ui;
        self.defs = defs;
        self.start_services(off);
    }

    fn start_services(&mut self, off: &[String]) {
        for i in 0..self.defs.len() {
            let d = &self.defs[i];
            if !d.is_service() || d.error.is_some() || off.contains(&d.id()) {
                continue;
            }
            let settings = self.settings_of(d);
            let d = &mut self.defs[i];
            match command(d, &settings).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
                Ok(mut child) => {
                    if let Some(o) = child.stdout.take() {
                        pump(d.id(), d.name.clone(), o, self.cmd_tx.clone(), self.log.clone());
                    }
                    if let Some(e) = child.stderr.take() {
                        pump(d.id(), d.name.clone(), e, self.cmd_tx.clone(), self.log.clone());
                    }
                    let mut stdin = child.stdin.take();
                    // first line: its settings
                    let first = json!({ "event": "settings", "time": chrono::Local::now().to_rfc3339(), "settings": settings });
                    if let Some(w) = stdin.as_mut() {
                        let _ = writeln!(w, "{first}").and_then(|_| w.flush());
                    }
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

    /// A plugin's settings: what was saved, with the plugin.json defaults for the rest.
    pub fn settings_of(&self, def: &PluginDef) -> Map<String, Value> {
        let saved = self.saved.get(&def.id());
        let mut m = Map::new();
        for s in &def.settings {
            let v = saved.and_then(|m| m.get(&s.id)).cloned().unwrap_or_else(|| s.default.clone());
            m.insert(s.id.clone(), v);
        }
        m
    }

    /// Settings saved from Settings → Plugins (sent to a service that is running).
    pub fn set_saved(&mut self, saved: BTreeMap<String, Map<String, Value>>, off: &[String]) {
        let changed: Vec<String> = self.defs.iter().map(|d| d.id()).filter(|id| self.saved.get(id) != saved.get(id)).collect();
        self.saved = saved;
        for id in changed {
            if let Some(d) = self.defs.iter().find(|d| d.id() == id) {
                if d.is_service() {
                    let s = Value::Object(self.settings_of(d));
                    self.send(&id, "settings", json!({ "settings": s }), off);
                }
            }
        }
    }

    /// Find the plugin a PLUGIN command names: "<folder or name> <action…>".
    pub fn find(&self, text: &str) -> Option<(String, String)> {
        let lower = text.trim().to_lowercase();
        let mut best: Option<(usize, String)> = None;
        for d in &self.defs {
            for key in [d.id(), d.name.clone()] {
                let k = key.to_lowercase();
                if (lower == k || lower.starts_with(&format!("{k} "))) && best.as_ref().is_none_or(|b| k.len() > b.0) {
                    best = Some((k.len(), d.id()));
                }
            }
        }
        best.map(|(n, id)| (id, text.trim().get(n..).unwrap_or_default().trim().to_string()))
    }

    fn message(event: &str, data: Value) -> Value {
        let mut msg = json!({ "event": event, "time": chrono::Local::now().to_rfc3339() });
        if let (Some(m), Some(d)) = (msg.as_object_mut(), data.as_object()) {
            for (k, v) in d {
                m.insert(k.clone(), v.clone());
            }
        }
        msg
    }

    /// Tell every interested plugin that something happened.
    pub fn emit(&mut self, event: &str, data: Value, off: &[String]) {
        if self.defs.is_empty() {
            return;
        }
        let msg = Self::message(event, data);
        let targets: Vec<usize> = (0..self.defs.len()).filter(|&i| self.defs[i].wants(event)).collect();
        self.deliver(&targets, &msg, off);
    }

    /// Send an event to one plugin only (an action on its controls, a reply, its settings).
    pub fn send(&mut self, id: &str, event: &str, data: Value, off: &[String]) {
        let msg = Self::message(event, data);
        let targets: Vec<usize> = (0..self.defs.len()).filter(|&i| self.defs[i].id() == id).collect();
        self.deliver(&targets, &msg, off);
    }

    /// Someone used one of a plugin's controls: tell it, with the values of its panel's inputs.
    pub fn action(&mut self, id: &str, action: &str, source: &str, extra: Value, off: &[String]) {
        let values = self.ui.get(id).map(|u| Value::Object(u.values.clone())).unwrap_or_else(|| json!({}));
        let mut data = json!({ "id": action, "source": source, "values": values });
        if let (Some(m), Some(x)) = (data.as_object_mut(), extra.as_object()) {
            for (k, v) in x {
                m.insert(k.clone(), v.clone());
            }
        }
        self.send(id, "action", data, off);
    }

    fn deliver(&mut self, targets: &[usize], msg: &Value, off: &[String]) {
        let event = msg["event"].as_str().unwrap_or_default().to_string();
        let mut dead = vec![];
        for &i in targets {
            let d = &self.defs[i];
            if d.error.is_some() || off.contains(&d.id()) {
                continue;
            }
            if d.is_service() {
                if let Some(s) = self.services.get_mut(&d.id()) {
                    let line = msg.to_string();
                    let ok = s.stdin.as_mut().is_some_and(|w| writeln!(w, "{line}").and_then(|_| w.flush()).is_ok());
                    if !ok {
                        dead.push(d.id());
                    }
                }
                continue;
            }
            // event plugin: one run per event, with its settings
            let settings = self.settings_of(d);
            let mut msg = msg.clone();
            msg["settings"] = Value::Object(settings.clone());
            let line = msg.to_string();
            let mut c = command(d, &settings);
            c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("RM_EVENT", &event);
            for (k, v) in msg.as_object().into_iter().flatten() {
                if let Some(s) = env_val(v) {
                    c.env(format!("RM_{}", env_key(k)), s);
                }
            }
            // started on a worker thread: launching a process can take tens of milliseconds on Windows,
            // and this runs while a track starts
            let (id, name, command, cmd_tx, log) = (d.id(), d.name.clone(), d.command.clone(), self.cmd_tx.clone(), self.log.clone());
            thread::spawn(move || {
                match c.spawn() {
                    Ok(mut child) => {
                        if let Some(mut w) = child.stdin.take() {
                            // in a thread of its own: a message with a big panel's values may not fit the pipe
                            thread::spawn(move || {
                                let _ = writeln!(w, "{line}");
                            });
                        }
                        if let Some(o) = child.stdout.take() {
                            pump(id.clone(), name.clone(), o, cmd_tx.clone(), log.clone());
                        }
                        if let Some(e) = child.stderr.take() {
                            pump(id.clone(), name.clone(), e, cmd_tx.clone(), log.clone());
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

    /// What plugins have asked for since last time: (plugin id, plugin name, message).
    /// Messages about the plugin's own controls are dealt with here; what is left needs the
    /// playout: `{"do": "command", "text": …}` for station commands, and the other JSON messages.
    pub fn take_requests(&mut self) -> Vec<(String, String, Value)> {
        let lines: Vec<Line> = self.cmd_rx.try_iter().collect();
        let mut out = vec![];
        for (id, name, text) in lines {
            if !text.starts_with('{') {
                out.push((id, name, json!({ "do": "command", "text": text })));
                continue;
            }
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => {
                    if !self.apply_ui(&id, &v) {
                        out.push((id, name, v));
                    }
                }
                Err(e) => self.log.lock().push(format!("[{name}] couldn't read message: {e}")),
            }
        }
        out
    }

    /// Messages that change a plugin's menu items, buttons or panel. False when it isn't one.
    fn apply_ui(&mut self, id: &str, v: &Value) -> bool {
        let what = v.get("do").and_then(Value::as_str).unwrap_or_default();
        let name = self.defs.iter().find(|d| d.id() == id).map(|d| d.name.clone()).unwrap_or_else(|| id.to_string());
        if what == "log" {
            let text = v.get("text").and_then(Value::as_str).unwrap_or_default();
            self.log.lock().push(format!("[{name}] {text}"));
            return true;
        }
        if !matches!(what, "menu" | "buttons" | "playlist_menu" | "panel" | "open" | "close" | "set" | "status") {
            return false;
        }
        let u = self.ui.entry(id.to_string()).or_insert_with(|| PluginUi { name: name.clone(), ..Default::default() });
        match what {
            "menu" => u.menu = items_of(v),
            "buttons" => u.buttons = items_of(v),
            "playlist_menu" => u.playlist_menu = items_of(v),
            "panel" => {
                if let Some(t) = v.get("title").and_then(Value::as_str) {
                    u.panel_title = t.to_string();
                }
                if u.panel_title.is_empty() {
                    u.panel_title = name;
                }
                u.panel = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                let items = u.panel.clone();
                u.take_values(&items);
                if let Some(o) = v.get("open").and_then(Value::as_bool) {
                    u.open = o;
                }
            }
            "open" => u.open = true,
            "close" => u.open = false,
            "status" => u.status = v.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
            _ => {
                // "set": change one control
                let wid = v.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let mut fields = v.as_object().cloned().unwrap_or_default();
                fields.remove("do");
                fields.remove("id");
                if set_widget(&mut u.panel, &wid, &fields) {
                    if let Some(val) = fields.get("value") {
                        u.values.insert(wid.clone(), val.clone());
                        u.sent.insert(wid, val.clone());
                    }
                }
            }
        }
        true
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
            r#"# Writes the song that just started to a file (for OBS, a website, ...)
$e = [Console]::In.ReadLine() | ConvertFrom-Json
$file = $e.settings.file
if (-not $file) { $file = "nowplaying.txt" }
if ($e.event -eq "action") {
    # someone picked "Show the now playing file" in the Plugins menu
    $t = (Get-Content -TotalCount 1 $file -ErrorAction SilentlyContinue)
    Write-Output ("RM: " + (@{ do = "notify"; text = "Now playing file: $t" } | ConvertTo-Json -Compress))
    exit
}
"$($e.artist) - $($e.title)" | Set-Content -Encoding UTF8 $file
Write-Output "wrote $($e.title)"
"#
            .replace('\n', "\r\n"),
        );
        #[cfg(not(windows))]
        let (cmd, args, file, script) = (
            "sh",
            r#"["now-playing.sh"]"#,
            "now-playing.sh",
            r#"# Writes the song that just started to a file (for OBS, a website, ...)
FILE="${RM_SETTING_FILE:-nowplaying.txt}"
if [ "$RM_EVENT" = "action" ]; then
    # someone picked "Show the now playing file" in the Plugins menu
    t=$(head -n 1 "$FILE" 2>/dev/null | tr -d '"\\')
    echo "RM: {\"do\": \"notify\", \"text\": \"Now playing file: $t\"}"
    exit 0
fi
printf '%s - %s\n' "$RM_ARTIST" "$RM_TITLE" > "$FILE"
echo "wrote $RM_TITLE"
"#
            .to_string(),
        );
        std::fs::write(dir.join(file), script)?;
        std::fs::write(
            &manifest,
            format!(
                r#"{{
  "name": "Example: now playing file",
  "description": "Writes the current song to a file in this folder. Adds \"Show the now playing file\" to the Plugins menu.",
  "command": "{cmd}",
  "args": {args},
  "events": ["track_started"],
  "mode": "event",
  "settings": [
    {{ "id": "file", "label": "File to write", "default": "nowplaying.txt" }}
  ],
  "menu": [
    {{ "id": "show", "label": "Show the now playing file" }}
  ]
}}
"#
            ),
        )?;
    }
    Ok(folder())
}

/// Open a folder in the file manager (or a file in its usual app).
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

    /// The tests share the plugins folder: one at a time, each starting from an empty one.
    static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn fresh_folder() -> parking_lot::MutexGuard<'static, ()> {
        let g = LOCK.lock();
        std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join("rm_test_cfg"));
        let _ = std::fs::remove_dir_all(folder());
        g
    }

    #[test]
    fn event_plugin_runs_and_sends_commands() {
        let _g = fresh_folder();
        let dir = folder().join("test-plugin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.sh"), "read line\necho \"$line\" > got.json\necho \"title is $RM_TITLE\"\necho 'RM: VOLUME 42'\n")
            .unwrap();
        std::fs::write(dir.join("plugin.json"), r#"{"name":"Test","command":"sh","args":["p.sh"],"events":["track_started"]}"#).unwrap();
        let log: LogSink = Arc::new(parking_lot::Mutex::new(vec![]));
        let mut p = Plugins::new(log.clone(), &[], BTreeMap::new());
        assert!(p.defs.iter().any(|d| d.name == "Test" && d.error.is_none()));
        p.emit("hour", json!({}), &[]); // not subscribed: nothing runs
        p.emit("track_started", json!({ "title": "Bloom" }), &[]);
        let mut cmds = vec![];
        for _ in 0..50 {
            cmds.extend(p.take_requests());
            if !cmds.is_empty() && dir.join("got.json").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(cmds, vec![("test-plugin".to_string(), "Test".to_string(), json!({ "do": "command", "text": "VOLUME 42" }))]);
        thread::sleep(Duration::from_millis(100));
        let got: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("got.json")).unwrap()).unwrap();
        assert_eq!(got["event"], "track_started");
        assert_eq!(got["title"], "Bloom");
        assert!(log.lock().iter().any(|l| l == "[Test] title is Bloom"), "{:?}", log.lock());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugins_add_controls_and_get_actions() {
        let _g = fresh_folder();
        let dir = folder().join("test-ui-plugin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a service: adds a button and a panel, then echoes every event it gets back as a notice
        std::fs::write(
            dir.join("s.sh"),
            concat!(
                "echo 'RM: {\"do\":\"buttons\",\"items\":[{\"id\":\"go\",\"label\":\"Go\"}]}'\n",
                "echo 'RM: {\"do\":\"panel\",\"title\":\"Studio\",\"open\":true,\"items\":[{\"type\":\"text\",\"id\":\"msg\",\"value\":\"hi\"}]}'\n",
                "while read line; do echo \"RM: {\\\"do\\\":\\\"notify\\\",\\\"text\\\":$(printf '%s' \"$line\" | wc -c)}\"; echo \"$line\" >> got.txt; done\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            r#"{"name":"UI Test","command":"sh","args":["s.sh"],"mode":"service","events":[],
                "settings":[{"id":"station","default":"Dova FM"}],"menu":[{"id":"m1","label":"Menu one"}]}"#,
        )
        .unwrap();
        let log: LogSink = Arc::new(parking_lot::Mutex::new(vec![]));
        let mut p = Plugins::new(log.clone(), &[], BTreeMap::new());
        let id = "test-ui-plugin";
        assert_eq!(p.ui[id].menu[0].label, "Menu one");
        let mut left = vec![];
        for _ in 0..60 {
            left.extend(p.take_requests());
            if p.ui[id].has_panel() && !p.ui[id].buttons.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let u = &p.ui[id];
        assert_eq!(u.buttons[0].id, "go");
        assert_eq!(u.panel_title, "Studio");
        assert!(u.open);
        assert_eq!(u.values["msg"], "hi");
        // the user types; the same panel sent again doesn't undo it, a new value does
        p.ui.get_mut(id).unwrap().values.insert("msg".into(), json!("typed"));
        assert!(p.apply_ui(id, &json!({"do":"panel","items":[{"type":"text","id":"msg","value":"hi"}]})));
        assert_eq!(p.ui[id].values["msg"], "typed");
        assert!(p.apply_ui(id, &json!({"do":"set","id":"msg","value":"new"})));
        assert_eq!(p.ui[id].values["msg"], "new");
        // an action reaches it (with the panel's values); its settings came first
        p.action(id, "go", "button", json!({}), &[]);
        for _ in 0..60 {
            if std::fs::read_to_string(dir.join("got.txt")).unwrap_or_default().contains("\"action\"") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        // (another test's playout may run this service too, so lines are looked up, not counted)
        let got = std::fs::read_to_string(dir.join("got.txt")).unwrap();
        let lines: Vec<Value> = got.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["event"], "settings");
        assert_eq!(lines[0]["settings"]["station"], "Dova FM");
        let act = lines.iter().find(|l| l["event"] == "action").expect("an action event");
        assert_eq!(act["id"], "go");
        assert_eq!(act["source"], "button");
        assert_eq!(act["values"]["msg"], "new");
        // changed settings are sent to the running service
        let mut saved = BTreeMap::new();
        saved.insert(id.to_string(), json!({ "station": "Radio Two" }).as_object().unwrap().clone());
        p.set_saved(saved, &[]);
        for _ in 0..60 {
            if std::fs::read_to_string(dir.join("got.txt")).unwrap_or_default().contains("Radio Two") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let got = std::fs::read_to_string(dir.join("got.txt")).unwrap();
        let last: Value = serde_json::from_str(got.lines().last().unwrap()).unwrap();
        assert_eq!(last["event"], "settings");
        assert_eq!(last["settings"]["station"], "Radio Two");
        thread::sleep(Duration::from_millis(100));
        left.extend(p.take_requests());
        assert!(left.iter().all(|(i, _, m)| i == id && m["do"] == "notify"), "{left:?}");
        // PLUGIN commands find it by folder or name
        assert_eq!(p.find("ui test go now"), Some((id.to_string(), "go now".to_string())));
        assert_eq!(p.find("test-ui-plugin go"), Some((id.to_string(), "go".to_string())));
        assert_eq!(p.find("nobody go"), None);
        p.stop_services();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
