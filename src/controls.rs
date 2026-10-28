//! Shortcuts and controllers: the actions a key or a MIDI control can fire, the keys each one
//! has (the defaults, or what the user set in Help → Shortcuts and MIDI), the user's own command
//! shortcuts, and the MIDI bindings. Kept per computer in controls.json: a MIDI controller and
//! a keyboard belong to this machine, not to the station a desktop app may be connected to.

use crate::config::data_dir;
use eframe::egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Something a shortcut or MIDI control can do.
pub struct ActionDef {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    /// the keys it has until the user changes them
    pub keys: &'static [&'static str],
    /// whether holding the key down repeats it
    pub repeat: bool,
}

const fn a(id: &'static str, group: &'static str, label: &'static str, keys: &'static [&'static str]) -> ActionDef {
    ActionDef { id, group, label, keys, repeat: false }
}
const fn rep(id: &'static str, group: &'static str, label: &'static str, keys: &'static [&'static str]) -> ActionDef {
    ActionDef { id, group, label, keys, repeat: true }
}

pub const PLAYBACK: &str = "Playback";
pub const PLAYLIST: &str = "Playlist";
pub const TOOLS: &str = "Tools and windows";
pub const GROUPS: [&str; 3] = [PLAYBACK, PLAYLIST, TOOLS];

/// Every built-in action, in the order Help → Shortcuts and MIDI lists them.
pub const ACTIONS: &[ActionDef] = &[
    a("play", PLAYBACK, "Play (the selected row, or carry on)", &["F5", "Enter"]),
    a("pause", PLAYBACK, "Pause / resume", &["F6"]),
    a("stop", PLAYBACK, "Stop", &["F7"]),
    a("next", PLAYBACK, "Next", &["F8"]),
    a("prev", PLAYBACK, "Previous", &["F9"]),
    rep("volume_up", PLAYBACK, "Volume up", &["Alt+ArrowUp"]),
    rep("volume_down", PLAYBACK, "Volume down", &["Alt+ArrowDown"]),
    rep("seek_back", PLAYBACK, "Back 30 seconds in the track", &["Alt+ArrowLeft"]),
    rep("seek_forward", PLAYBACK, "Forward 30 seconds in the track", &["Alt+ArrowRight"]),
    a("seek_to", PLAYBACK, "Go to a time in the track", &["Ctrl+T"]),
    a("mic", PLAYBACK, "Microphone on / off", &["Ctrl+M"]),
    a("stop_jingles", PLAYBACK, "Stop all jingles", &["Ctrl+J"]),
    a("cart1", PLAYBACK, "Cart pad 1", &["Shift+F1"]),
    a("cart2", PLAYBACK, "Cart pad 2", &["Shift+F2"]),
    a("cart3", PLAYBACK, "Cart pad 3", &["Shift+F3"]),
    a("cart4", PLAYBACK, "Cart pad 4", &["Shift+F4"]),
    a("cart5", PLAYBACK, "Cart pad 5", &["Shift+F5"]),
    a("cart6", PLAYBACK, "Cart pad 6", &["Shift+F6"]),
    a("cart7", PLAYBACK, "Cart pad 7", &["Shift+F7"]),
    a("cart8", PLAYBACK, "Cart pad 8", &["Shift+F8"]),
    a("cart9", PLAYBACK, "Cart pad 9", &["Shift+F9"]),
    a("cart10", PLAYBACK, "Cart pad 10", &["Shift+F10"]),
    a("cart11", PLAYBACK, "Cart pad 11", &["Shift+F11"]),
    a("cart12", PLAYBACK, "Cart pad 12", &["Shift+F12"]),
    a("add_files", PLAYLIST, "Add files", &["Insert", "Alt+F"]),
    a("add_folder", PLAYLIST, "Add folder", &["Alt+D"]),
    a("add_url", PLAYLIST, "Add URL", &["Alt+U"]),
    a("add_command", PLAYLIST, "Add command", &["Alt+C"]),
    a("add_tts", PLAYLIST, "Add text to speech", &["Alt+T"]),
    a("add_comment", PLAYLIST, "Add comment", &["Alt+W"]),
    a("add_pause", PLAYLIST, "Add timed pause", &["Ctrl+P"]),
    a("add_hour", PLAYLIST, "Add hour marker", &["Ctrl+H"]),
    a("insert_voice", PLAYLIST, "Insert voice track", &["Ctrl+W"]),
    a("record_voice", PLAYLIST, "Record voice track", &["Ctrl+R"]),
    a("new_playlist", PLAYLIST, "New playlist", &["Ctrl+N"]),
    a("open_playlist", PLAYLIST, "Open playlist (M3U)", &["Ctrl+O"]),
    a("save_playlist", PLAYLIST, "Save playlist (M3U)", &["Ctrl+S"]),
    rep("move_up", PLAYLIST, "Move the selected rows up", &["Ctrl+ArrowUp"]),
    rep("move_down", PLAYLIST, "Move the selected rows down", &["Ctrl+ArrowDown"]),
    a("next_tab", PLAYLIST, "Next playlist tab", &["Ctrl+Tab"]),
    a("prev_tab", PLAYLIST, "Previous playlist tab", &["Ctrl+Shift+Tab"]),
    a("find", PLAYLIST, "Find", &["Ctrl+F"]),
    a("generator", TOOLS, "Playlist generator", &["Ctrl+G"]),
    a("track_editor", TOOLS, "Track editor for the selected row", &["Ctrl+E"]),
    a("scheduler", TOOLS, "Scheduler", &["F3"]),
    a("audio_fx", TOOLS, "Audio processing", &["F4"]),
    a("notepad", TOOLS, "Notepad", &["T"]),
    a("settings", TOOLS, "Settings", &["Ctrl+Comma"]),
    a("panels1", TOOLS, "Work zone with 1 panel", &["Ctrl+1"]),
    a("panels2", TOOLS, "Work zone with 2 panels", &["Ctrl+2"]),
    a("panels3", TOOLS, "Work zone with 3 panels", &["Ctrl+3"]),
    a("new_window", TOOLS, "New Additional window", &["Ctrl+4"]),
    a("left_panel", TOOLS, "Left panel on / off", &["Ctrl+L"]),
    a("file_explorer", TOOLS, "File explorer in the left panel", &["F1"]),
    a("search", TOOLS, "Search in the left panel", &["F2"]),
    a("fullscreen", TOOLS, "Full screen", &["F11"]),
    a("performance", TOOLS, "Performance numbers", &["Ctrl+Shift+P"]),
    a("shortcuts", TOOLS, "Shortcuts and MIDI (this list)", &["Ctrl+Slash"]),
];

/// A MIDI fader or knob can set the volume directly (not a key action, so it isn't in `ACTIONS`).
pub const VOLUME_FADER: &str = "volume_fader";

pub fn action(id: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// A key with its modifiers. "Ctrl" is Cmd on a Mac.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Option<Self> {
        let mut c = KeyCombo { ctrl: false, shift: false, alt: false, key: Key::A };
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => c.ctrl = true,
                "shift" => c.shift = true,
                "alt" | "option" => c.alt = true,
                _ => key = Some(Key::from_name(part)?),
            }
        }
        c.key = key?;
        Some(c)
    }

    pub fn from_event(key: Key, m: Modifiers) -> Self {
        KeyCombo { ctrl: m.command, shift: m.shift, alt: m.alt, key }
    }

    /// For saving, e.g. "Ctrl+Shift+F5".
    pub fn id(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.shift, "Shift+"), (self.alt, "Alt+")] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(self.key.name());
        s
    }

    /// For showing, e.g. "Ctrl+↑" (Cmd on a Mac).
    pub fn label(&self) -> String {
        let mut s = String::new();
        let ctrl = if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" };
        for (on, name) in [(self.ctrl, ctrl), (self.shift, "Shift+"), (self.alt, "Alt+")] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(self.key.symbol_or_name());
        s
    }
}

/// A shortcut the user made: runs a playlist command line such as `PLAYLIST Evening`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct CustomCmd {
    pub id: u64,
    pub name: String,
    pub command: String,
}

impl CustomCmd {
    pub fn action_id(&self) -> String {
        format!("cmd:{}", self.id)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MidiKind {
    Note,
    Cc,
    Program,
}

/// Which MIDI control: a key/pad (note), a knob/fader/button (CC) or a program change.
/// `channel` is 0–15.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MidiCtl {
    pub kind: MidiKind,
    pub channel: u8,
    pub num: u8,
}

impl MidiCtl {
    pub fn label(&self) -> String {
        let what = match self.kind {
            MidiKind::Note => format!("Note {}", note_name(self.num)),
            MidiKind::Cc => format!("CC {}", self.num),
            MidiKind::Program => format!("Program {}", self.num),
        };
        format!("{what} · ch {}", self.channel + 1)
    }
}

fn note_name(n: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[n as usize % 12], n as i32 / 12 - 1)
}

/// One message from a controller, as far as RadioMaster cares.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MidiEvent {
    pub ctl: MidiCtl,
    /// note velocity or CC value, 0–127 (0 = released)
    pub value: u8,
}

impl MidiEvent {
    /// Reads a raw MIDI message; anything else (clock, aftertouch, sysex…) is None.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        let channel = status & 0x0f;
        let b1 = *bytes.get(1)? & 0x7f;
        let ctl = |kind| MidiCtl { kind, channel, num: b1 };
        match status & 0xf0 {
            0x90 => Some(MidiEvent { ctl: ctl(MidiKind::Note), value: *bytes.get(2)? & 0x7f }),
            0x80 => Some(MidiEvent { ctl: ctl(MidiKind::Note), value: 0 }),
            0xb0 => Some(MidiEvent { ctl: ctl(MidiKind::Cc), value: *bytes.get(2)? & 0x7f }),
            0xc0 => Some(MidiEvent { ctl: ctl(MidiKind::Program), value: 127 }),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MidiBinding {
    pub ctl: MidiCtl,
    /// an `ACTIONS` id, a `cmd:<id>` custom command or `VOLUME_FADER`
    pub action: String,
}

/// controls.json
#[derive(Serialize, Deserialize, Clone, PartialEq, Default, Debug)]
#[serde(default)]
pub struct Controls {
    /// keys the user changed, by action id (an empty list = no key); actions not here keep their defaults
    pub keys: BTreeMap<String, Vec<String>>,
    pub custom: Vec<CustomCmd>,
    /// listen to MIDI controllers
    pub midi_enabled: bool,
    pub midi: Vec<MidiBinding>,
}

impl Controls {
    fn path() -> std::path::PathBuf {
        data_dir().join("controls.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(j) = serde_json::to_string_pretty(self) {
            crate::playout::write_in_background(Self::path(), j);
        }
    }

    /// The keys an action has now.
    pub fn keys_of(&self, id: &str) -> Vec<KeyCombo> {
        match self.keys.get(id) {
            Some(k) => k.iter().filter_map(|s| KeyCombo::parse(s)).collect(),
            None => action(id).map(|a| a.keys.iter().filter_map(|s| KeyCombo::parse(s)).collect()).unwrap_or_default(),
        }
    }

    /// The first key of an action, for menus ("" when it has none).
    pub fn key_label(&self, id: &str) -> String {
        self.keys_of(id).first().map(KeyCombo::label).unwrap_or_default()
    }

    pub fn set_keys(&mut self, id: &str, keys: Vec<KeyCombo>) {
        let keys: Vec<String> = keys.iter().map(KeyCombo::id).collect();
        let default: Option<Vec<String>> = action(id).map(|a| a.keys.iter().map(|s| s.to_string()).collect());
        if default.as_ref() == Some(&keys) {
            self.keys.remove(id);
        } else {
            self.keys.insert(id.to_string(), keys);
        }
    }

    /// Every action id: the built-in ones, then the custom commands.
    pub fn all_ids(&self) -> Vec<String> {
        ACTIONS.iter().map(|a| a.id.to_string()).chain(self.custom.iter().map(CustomCmd::action_id)).collect()
    }

    /// The action that has this key, if any.
    pub fn owner_of(&self, k: KeyCombo) -> Option<String> {
        self.all_ids().into_iter().find(|id| self.keys_of(id).contains(&k))
    }

    /// Gives `k` to `id`, taking it off whichever action had it. Returns that action.
    pub fn bind_key(&mut self, id: &str, k: KeyCombo) -> Option<String> {
        let prev = self.owner_of(k).filter(|o| o != id);
        if let Some(o) = &prev {
            let rest = self.keys_of(o).into_iter().filter(|x| *x != k).collect();
            self.set_keys(o, rest);
        }
        let mut mine = self.keys_of(id);
        if !mine.contains(&k) {
            mine.push(k);
        }
        self.set_keys(id, mine);
        prev
    }

    /// Key → action, rebuilt when the bindings change.
    pub fn keymap(&self) -> HashMap<KeyCombo, String> {
        let mut m = HashMap::new();
        for id in self.all_ids() {
            for k in self.keys_of(&id) {
                m.entry(k).or_insert_with(|| id.clone());
            }
        }
        m
    }

    /// The display name of any action id.
    pub fn name_of(&self, id: &str) -> String {
        if id == VOLUME_FADER {
            return "Volume (fader or knob)".into();
        }
        if let Some(a) = action(id) {
            return crate::i18n::tr(a.label).to_string();
        }
        self.custom.iter().find(|c| c.action_id() == id).map(|c| c.name.clone()).unwrap_or_else(|| id.to_string())
    }

    pub fn midi_of(&self, id: &str) -> Vec<MidiCtl> {
        self.midi.iter().filter(|b| b.action == id).map(|b| b.ctl).collect()
    }

    /// Gives a MIDI control to `id`, taking it off whichever action had it. Returns that action.
    pub fn bind_midi(&mut self, id: &str, ctl: MidiCtl) -> Option<String> {
        let prev = self.midi.iter().find(|b| b.ctl == ctl && b.action != id).map(|b| b.action.clone());
        self.midi.retain(|b| b.ctl != ctl);
        self.midi.push(MidiBinding { ctl, action: id.to_string() });
        prev
    }

    pub fn new_custom_id(&self) -> u64 {
        self.custom.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    /// Removes a custom command and its keys and MIDI controls.
    pub fn remove_custom(&mut self, id: u64) {
        let aid = format!("cmd:{id}");
        self.custom.retain(|c| c.id != id);
        self.keys.remove(&aid);
        self.midi.retain(|b| b.action != aid);
    }
}

/// Turns MIDI messages into actions: notes and program changes fire on press; a CC fires when
/// it goes from below half to half or more (a button), unless it's bound to the volume fader.
#[derive(Default)]
pub struct MidiRouter {
    last_cc: HashMap<(u8, u8), u8>,
}

pub enum MidiAction {
    Fire(String),
    /// 0.0–1.0
    Volume(f32),
}

impl MidiRouter {
    pub fn route(&mut self, c: &Controls, e: MidiEvent) -> Option<MidiAction> {
        let prev = match e.ctl.kind {
            MidiKind::Cc => self.last_cc.insert((e.ctl.channel, e.ctl.num), e.value),
            _ => None,
        };
        let b = c.midi.iter().find(|b| b.ctl == e.ctl)?;
        if b.action == VOLUME_FADER {
            return (e.ctl.kind == MidiKind::Cc).then(|| MidiAction::Volume(e.value as f32 / 127.0));
        }
        let pressed = match e.ctl.kind {
            MidiKind::Note | MidiKind::Program => e.value > 0,
            MidiKind::Cc => e.value >= 64 && prev.is_none_or(|p| p < 64),
        };
        pressed.then(|| MidiAction::Fire(b.action.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keys_all_parse_and_dont_clash() {
        let c = Controls::default();
        let mut seen = HashMap::new();
        for a in ACTIONS {
            assert_eq!(c.keys_of(a.id).len(), a.keys.len(), "{} has a key that doesn't parse", a.id);
            for k in c.keys_of(a.id) {
                assert!(seen.insert(k, a.id).is_none(), "{} is on {} and {}", k.id(), a.id, seen[&k]);
            }
        }
    }

    #[test]
    fn key_text_round_trips() {
        for s in ["Ctrl+Shift+F5", "Alt+ArrowUp", "T", "Ctrl+Comma", "Ctrl+Slash", "Shift+Alt+Space"] {
            let k = KeyCombo::parse(s).unwrap();
            assert_eq!(KeyCombo::parse(&k.id()), Some(k), "{s}");
        }
        assert_eq!(KeyCombo::parse("ctrl+g").unwrap(), KeyCombo { ctrl: true, shift: false, alt: false, key: Key::G });
        assert!(KeyCombo::parse("Ctrl+Nope").is_none());
    }

    #[test]
    fn rebinding_moves_a_key_and_reset_forgets_it() {
        let mut c = Controls::default();
        let f8 = KeyCombo::parse("F8").unwrap();
        assert_eq!(c.bind_key("stop", f8).as_deref(), Some("next"));
        assert!(c.keys_of("next").is_empty());
        assert_eq!(c.keymap()[&f8], "stop");
        c.set_keys("next", vec![f8]);
        c.set_keys("stop", vec![KeyCombo::parse("F7").unwrap()]);
        assert!(c.keys.is_empty(), "back to the defaults: nothing saved");
    }

    #[test]
    fn custom_commands_get_keys_and_go_away_cleanly() {
        let mut c = Controls::default();
        let id = c.new_custom_id();
        c.custom.push(CustomCmd { id, name: "Evening".into(), command: "PLAYLIST Evening".into() });
        let k = KeyCombo::parse("Ctrl+Shift+E").unwrap();
        c.bind_key("cmd:1", k);
        c.bind_midi("cmd:1", MidiCtl { kind: MidiKind::Note, channel: 0, num: 36 });
        assert_eq!(c.keymap()[&k], "cmd:1");
        c.remove_custom(id);
        assert!(c.keys.is_empty() && c.midi.is_empty() && !c.keymap().contains_key(&k));
    }

    #[test]
    fn midi_messages_and_routing() {
        assert_eq!(MidiEvent::parse(&[0x91, 36, 100]).unwrap().ctl, MidiCtl { kind: MidiKind::Note, channel: 1, num: 36 });
        assert_eq!(MidiEvent::parse(&[0x90, 36, 0]).unwrap().value, 0);
        assert!(MidiEvent::parse(&[0xf8]).is_none());
        let mut c = Controls::default();
        let pad = MidiCtl { kind: MidiKind::Note, channel: 0, num: 36 };
        let btn = MidiCtl { kind: MidiKind::Cc, channel: 0, num: 20 };
        let fader = MidiCtl { kind: MidiKind::Cc, channel: 0, num: 7 };
        c.bind_midi("next", pad);
        c.bind_midi("stop", btn);
        c.bind_midi(VOLUME_FADER, fader);
        let mut r = MidiRouter::default();
        let fire = |r: &mut MidiRouter, ctl, value| match r.route(&c, MidiEvent { ctl, value }) {
            Some(MidiAction::Fire(a)) => Some(a),
            _ => None,
        };
        assert_eq!(fire(&mut r, pad, 90).as_deref(), Some("next"));
        assert_eq!(fire(&mut r, pad, 0), None, "release does nothing");
        assert_eq!(fire(&mut r, btn, 127).as_deref(), Some("stop"));
        assert_eq!(fire(&mut r, btn, 127), None, "held: fires once");
        assert_eq!(fire(&mut r, btn, 0), None);
        assert_eq!(fire(&mut r, btn, 127).as_deref(), Some("stop"));
        assert!(matches!(r.route(&c, MidiEvent { ctl: fader, value: 127 }), Some(MidiAction::Volume(v)) if v == 1.0));
    }
}
