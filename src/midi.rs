//! MIDI controllers: a background thread listens to every MIDI input on this computer (and picks
//! up ones plugged in later) and hands the messages to the UI, which turns them into actions with
//! `controls::MidiRouter`. Nothing here runs on the UI thread except draining the channel.

use crate::controls::MidiEvent;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the list of MIDI inputs is checked for controllers plugged in or out.
const RESCAN: Duration = Duration::from_secs(3);

#[derive(Default, Clone)]
pub struct MidiStatus {
    pub enabled: bool,
    /// inputs listened to now
    pub ports: Vec<String>,
    /// why MIDI isn't available (empty when it is)
    pub error: String,
}

pub struct Midi {
    rx: Receiver<MidiEvent>,
    status: Arc<Mutex<MidiStatus>>,
}

impl Midi {
    pub fn start(enabled: bool, ctx: egui::Context) -> Self {
        let (tx, rx) = unbounded();
        let status = Arc::new(Mutex::new(MidiStatus { enabled, ..Default::default() }));
        let st = status.clone();
        let _ = std::thread::Builder::new().name("midi".into()).spawn(move || listen(st, tx, ctx));
        Midi { rx, status }
    }

    pub fn set_enabled(&self, on: bool) {
        self.status.lock().enabled = on;
    }

    /// The messages that came in since the last call.
    pub fn drain(&self) -> Vec<MidiEvent> {
        self.rx.try_iter().collect()
    }

    pub fn status(&self) -> MidiStatus {
        self.status.lock().clone()
    }
}

fn listen(status: Arc<Mutex<MidiStatus>>, tx: Sender<MidiEvent>, ctx: egui::Context) {
    let mut conns: HashMap<String, midir::MidiInputConnection<()>> = HashMap::new();
    let mut last_scan: Option<Instant> = None;
    let mut was_on = false;
    loop {
        let on = status.lock().enabled;
        if !on {
            if was_on {
                conns.clear();
                let mut s = status.lock();
                s.ports.clear();
                s.error.clear();
            }
            was_on = false;
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        if !was_on || last_scan.is_none_or(|t| t.elapsed() >= RESCAN) {
            was_on = true;
            last_scan = Some(Instant::now());
            let err = scan(&mut conns, &tx, &ctx).err().unwrap_or_default();
            let mut s = status.lock();
            s.error = err;
            s.ports = conns.keys().cloned().collect();
            s.ports.sort();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Connects to inputs that appeared and drops the ones that went away.
fn scan(conns: &mut HashMap<String, midir::MidiInputConnection<()>>, tx: &Sender<MidiEvent>, ctx: &egui::Context) -> Result<(), String> {
    let probe = midir::MidiInput::new("RadioMaster").map_err(|e| e.to_string())?;
    let names: Vec<String> = probe.ports().iter().filter_map(|p| probe.port_name(p).ok()).collect();
    conns.retain(|name, _| names.contains(name));
    for name in names {
        if conns.contains_key(&name) {
            continue;
        }
        let mut input = midir::MidiInput::new("RadioMaster").map_err(|e| e.to_string())?;
        input.ignore(midir::Ignore::All);
        let Some(port) = input.ports().into_iter().find(|p| input.port_name(p).is_ok_and(|n| n == name)) else {
            continue;
        };
        let (tx, ctx) = (tx.clone(), ctx.clone());
        let on_msg = move |_: u64, bytes: &[u8], _: &mut ()| {
            if let Some(e) = MidiEvent::parse(bytes) {
                let _ = tx.send(e);
                ctx.request_repaint();
            }
        };
        // a port that won't open now (in use, unplugged) is tried again at the next scan
        if let Ok(c) = input.connect(&port, "RadioMaster in", on_msg, ()) {
            conns.insert(name, c);
        }
    }
    Ok(())
}
