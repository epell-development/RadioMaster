//! RadioMaster Server: the station without a window, for Linux and Windows Server.
//! It plays, schedules and streams on its own; desktop apps connect with File → Connect to Server.
// The server shares the desktop app's modules and uses only part of each.
#![allow(dead_code)]
mod audio;
mod auth;
mod azuracast;
mod config;
mod dsp;
mod engine;
mod fade;
mod generator;
mod items;
mod library;
#[cfg(test)]
#[path = "link.rs"]
mod link;
mod live;
mod mic;
mod playout;
mod plugins;
mod remote;
mod scheduler;
mod server;
mod stream;

use parking_lot::Mutex;
use server::ServerCfg;
use std::{
    io::{BufRead, IsTerminal, Write},
    path::PathBuf,
    sync::Arc,
    thread,
    time::Duration,
};

const HELP: &str = "RadioMaster Server — radio automation without a window

Usage: RadioMaster-Server [options]

  --setup               set the station name, username, password and port, then exit
  --user NAME           username for --setup (asked for when left out)
  --password PASSWORD   password for --setup (or set RADIOMASTER_PASSWORD; asked for when left out)
  --name NAME           station name shown in the app (with --setup)
  --port N              port to listen on (default 9259; saved with --setup)
  --bind ADDRESS        address to listen on (default 0.0.0.0, every network)
  --data FOLDER         data folder (default: the RadioMaster-Server folder in your settings folder)
  --no-audio            don't use a sound card: stream only
  --list-devices        list the sound cards and exit
  --version             print the version and exit

Desktop apps connect with File → Connect to Server, using this machine's address and port.";

/// Work for the main thread (which owns the sound cards).
enum Ctl {
    Quit,
    /// the sound card settings changed
    Audio,
}

struct Args {
    setup: bool,
    user: Option<String>,
    password: Option<String>,
    name: Option<String>,
    port: Option<u16>,
    bind: Option<String>,
    data: Option<PathBuf>,
    no_audio: bool,
    list_devices: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        setup: false,
        user: None,
        password: None,
        name: None,
        port: None,
        bind: None,
        data: None,
        no_audio: false,
        list_devices: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |what: &str| it.next().ok_or_else(|| format!("{arg} needs {what}"));
        match arg.as_str() {
            "--setup" => a.setup = true,
            "--user" => a.user = Some(value("a name")?),
            "--password" => a.password = Some(value("a password")?),
            "--name" => a.name = Some(value("a name")?),
            "--port" => a.port = Some(value("a number")?.parse().map_err(|_| "--port needs a number from 1 to 65535".to_string())?),
            "--bind" => a.bind = Some(value("an address")?),
            "--data" => a.data = Some(PathBuf::from(value("a folder")?)),
            "--no-audio" => a.no_audio = true,
            "--list-devices" => a.list_devices = true,
            "--version" | "-V" => {
                println!("RadioMaster Server {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--help" | "-h" | "/?" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other} (see --help)")),
        }
    }
    if a.password.is_none() {
        a.password = std::env::var("RADIOMASTER_PASSWORD").ok().filter(|p| !p.is_empty());
    }
    Ok(a)
}

fn ask(prompt: &str, default: &str) -> String {
    if default.is_empty() {
        print!("{prompt}: ");
    } else {
        print!("{prompt} [{default}]: ");
    }
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    let line = line.trim();
    if line.is_empty() {
        default.to_string()
    } else {
        line.to_string()
    }
}

/// Station name, username, password and port: from the options, else asked for at the terminal.
fn setup(args: &Args, old: Option<ServerCfg>) -> Result<ServerCfg, String> {
    let mut cfg = old.unwrap_or_default();
    let tty = std::io::stdin().is_terminal();
    if tty {
        println!("RadioMaster Server setup — desktop apps sign in with this username and password.");
    }
    cfg.name = match &args.name {
        Some(n) => n.clone(),
        None if tty => ask("Station name", &cfg.name),
        None => cfg.name,
    };
    cfg.user = match &args.user {
        Some(u) => u.clone(),
        None if tty => ask("Username", &cfg.user),
        None => cfg.user,
    };
    if cfg.user.trim().is_empty() {
        return Err("a username is needed (--user NAME)".into());
    }
    cfg.user = cfg.user.trim().to_string();
    let password = match &args.password {
        Some(p) => p.clone(),
        None if tty => loop {
            let p = rpassword::prompt_password("Password (at least 12 characters): ").map_err(|e| e.to_string())?;
            if p.chars().count() < 12 {
                println!("That is too short.");
                continue;
            }
            if rpassword::prompt_password("Password again: ").map_err(|e| e.to_string())? == p {
                break p;
            }
            println!("The passwords don't match.");
        },
        None => return Err("a password is needed (--password, or set RADIOMASTER_PASSWORD)".into()),
    };
    cfg.password_hash = auth::hash(&password)?;
    cfg.port = match args.port {
        Some(p) => p,
        None if tty => ask("Port", &cfg.port.to_string()).parse().map_err(|_| "the port must be a number from 1 to 65535".to_string())?,
        None => cfg.port,
    };
    if let Some(b) = &args.bind {
        cfg.bind = b.clone();
    }
    Ok(cfg)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    config::set_data_dir(
        args.data.clone().unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("RadioMaster-Server")),
    );
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{} RadioMaster Server crashed: {info}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        eprintln!("{msg}");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::data_dir().join("crash.log")) {
            let _ = f.write_all(msg.as_bytes());
        }
    }));
    if args.list_devices {
        let def = audio::default_output_name().unwrap_or_default();
        for d in audio::list_outputs() {
            println!("{}{d}", if d == def { "* " } else { "  " });
        }
        return;
    }

    let saved = ServerCfg::load();
    let mut scfg = if args.setup || !saved.as_ref().is_some_and(|s| s.ready()) {
        match setup(&args, saved) {
            Ok(c) => {
                if let Err(e) = c.save() {
                    eprintln!("Could not save {}: {e}", ServerCfg::path().display());
                    std::process::exit(1);
                }
                println!("Saved {}", ServerCfg::path().display());
                if args.setup {
                    return;
                }
                c
            }
            Err(e) => {
                eprintln!("Setup: {e}\nRun RadioMaster-Server --setup at a terminal, or see --help.");
                std::process::exit(2);
            }
        }
    } else {
        saved.unwrap_or_default()
    };
    if let Some(p) = args.port {
        scfg.port = p;
    }
    if let Some(b) = &args.bind {
        scfg.bind = b.clone();
    }

    println!("RadioMaster Server {} — data folder {}", env!("CARGO_PKG_VERSION"), config::data_dir().display());
    let engine = engine::Engine::new_detached();
    let cfg = config::Config::load();
    engine.set_volume(cfg.volume);
    let remote = cfg.remote.clone();
    let core = Arc::new(Mutex::new(playout::Playout::new(engine, cfg)));
    // sound cards (or a silent clock, so streaming still runs in real time). They are opened on this
    // thread; the API asks for it again when the settings change in the app.
    let mut audio = audio::AudioManager::default();
    let audio_report: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let open_audio = |audio: &mut audio::AudioManager, want: bool| {
        let (engine, devices) = {
            let c = core.lock();
            (c.engine.clone(), c.cfg.audio_devices.clone())
        };
        if want && !args.no_audio {
            audio.apply(&engine, &devices);
        } else {
            audio.silence(&engine);
        }
        for (dev, st) in &audio.report {
            println!("Audio: {dev}: {st}");
        }
        *audio_report.lock() = audio.report.clone();
    };
    if args.no_audio {
        println!("Audio: off (--no-audio), streaming only");
    }
    open_audio(&mut audio, scfg.audio);
    if remote.enabled {
        remote::spawn(core.clone(), remote.port, remote.password);
    }
    {
        let c = core.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(50));
            c.lock().tick();
        });
    }
    let (ctl_tx, ctl_rx) = std::sync::mpsc::channel::<Ctl>();
    let host = {
        let tx = Mutex::new(ctl_tx.clone());
        server::Host {
            reapply_audio: Box::new(move || {
                let _ = tx.lock().send(Ctl::Audio);
            }),
            audio_report: audio_report.clone(),
            forced_silent: args.no_audio,
        }
    };
    match server::spawn(core.clone(), scfg.clone(), host) {
        Ok(addr) => println!("Listening on {addr} — sign in as '{}' from File → Connect to Server", scfg.user),
        Err(e) => {
            eprintln!("Could not listen on {}:{}: {e}", scfg.bind, scfg.port);
            std::process::exit(1);
        }
    }
    {
        let mut c = core.lock();
        c.plugin_event("app_start", serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "server": true }));
        if scfg.autoplay && !c.playlist.is_empty() {
            c.play_pressed(None);
        }
    }

    if let Err(e) = ctrlc::set_handler(move || {
        let _ = ctl_tx.send(Ctl::Quit);
    }) {
        eprintln!("Ctrl+C handler: {e}");
    }
    // the station log goes to the console (and to journald / the service log)
    let mut printed = core.lock().log_total;
    loop {
        let quit = match ctl_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ctl::Quit) => true,
            Ok(Ctl::Audio) => {
                open_audio(&mut audio, ServerCfg::load().is_none_or(|c| c.audio));
                false
            }
            Err(_) => false,
        };
        {
            let c = core.lock();
            let new = (c.log_total - printed) as usize;
            for l in &c.log[c.log.len().saturating_sub(new)..] {
                println!("{} {}{}", l.time, if l.error { "ERROR " } else { "" }, l.text);
            }
            printed = c.log_total;
        }
        if quit {
            break;
        }
    }
    println!("Stopping…");
    let mut c = core.lock();
    c.plugin_event("app_exit", serde_json::json!({}));
    for i in 0..c.cfg.encoders.len() {
        c.stop_encoder(i);
    }
    c.save_all();
    c.plugins.stop_services();
    drop(c);
    playout::flush_writes();
}
