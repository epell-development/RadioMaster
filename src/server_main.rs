//! RadioMaster Server: the station without a window, for Linux and Windows Server.
//! It plays, schedules and streams on its own; desktop apps connect with File → Connect to Server.
// The server shares the desktop app's modules and uses only part of each.
#![allow(dead_code)]
mod acme;
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
mod selfupdate;
mod server;
mod stream;
mod tls;
mod tunnel;
mod updater;

use auth::Accounts;
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

  --setup               set the station name, an administrator's username and password, and the port, then exit
  --user NAME           administrator's username for --setup (asked for when left out); more accounts
                        are added in the app (File → Server users) by an administrator
  --password PASSWORD   password for --setup (or set RADIOMASTER_PASSWORD; asked for when left out)
  --name NAME           station name shown in the app (with --setup)
  --port N              port to listen on (default 9259; saved with --setup)
  --bind ADDRESS        address to listen on (default 0.0.0.0, every network)
  --https / --no-https  serve over HTTPS (saved with --setup)
  --cert FILE           PEM certificate (chain) for HTTPS, e.g. Let's Encrypt's fullchain.pem
  --key FILE            its PEM private key (without --cert/--key a self-signed certificate is made)
  --acme DOMAINS        HTTPS with a free Let's Encrypt certificate for these names (comma-separated);
                        port 80 of each name has to reach this server (saved with --setup)
  --acme-email ADDRESS  where Let's Encrypt writes about the certificate (optional)
  --acme-staging        use Let's Encrypt's staging server (for trying it out)
  --acme-http-port N    port for Let's Encrypt's checks (default 80; 0: a proxy in front passes them on)
  --no-acme             stop using Let's Encrypt
  --tunnel / --no-tunnel  put the web portal on the internet through a Cloudflare Tunnel (saved with --setup)
  --tunnel-token TOKEN  the tunnel's token from the Cloudflare dashboard (better: set CLOUDFLARE_TUNNEL_TOKEN,
                        which other users can't see; without one: a quick trycloudflare.com tunnel). Saved
                        with --setup in tunnel-token in the data folder, readable by its owner only
  --cloudflared FILE    the cloudflared program (default: next to the server, or on the PATH)
  --portal / --no-portal  the web portal: run the station from a browser (saved with --setup)
  --portal-port N       port for the web portal (default 9260)
  --data FOLDER         data folder (default: the RadioMaster-Server folder in your settings folder)
  --no-audio            don't use a sound card: stream only
  --no-update-check     don't look for newer releases on GitHub (saved with --setup)
  --list-devices        list the sound cards and exit
  --version             print the version and exit

Desktop apps connect with File → Connect to Server, using this machine's address and port.
Options given without --setup apply to this run only.

Administrators can update, restart and stop the server from the app. Under systemd it is started again
by the service (Restart=on-failure); under another service manager, set RADIOMASTER_SUPERVISED=1 so
it leaves restarting to that manager instead of starting a new copy itself.";

/// Work for the main thread (which owns the sound cards).
enum Ctl {
    Quit,
    /// stop and start again (an administrator asked, or an update was installed)
    Restart,
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
    no_update_check: bool,
    list_devices: bool,
    https: Option<bool>,
    cert: Option<String>,
    key: Option<String>,
    portal: Option<bool>,
    portal_port: Option<u16>,
    acme: Option<String>,
    no_acme: bool,
    acme_email: Option<String>,
    acme_staging: bool,
    acme_http_port: Option<u16>,
    tunnel: Option<bool>,
    tunnel_token: Option<String>,
    cloudflared: Option<String>,
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
        no_update_check: false,
        list_devices: false,
        https: None,
        cert: None,
        key: None,
        portal: None,
        portal_port: None,
        acme: None,
        no_acme: false,
        acme_email: None,
        acme_staging: false,
        acme_http_port: None,
        tunnel: None,
        tunnel_token: None,
        cloudflared: None,
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
            "--no-update-check" => a.no_update_check = true,
            "--https" => a.https = Some(true),
            "--no-https" => a.https = Some(false),
            "--cert" => a.cert = Some(value("a file")?),
            "--key" => a.key = Some(value("a file")?),
            "--portal" => a.portal = Some(true),
            "--no-portal" => a.portal = Some(false),
            "--portal-port" => {
                a.portal_port = Some(value("a number")?.parse().map_err(|_| "--portal-port needs a number from 1 to 65535".to_string())?)
            }
            "--acme" => a.acme = Some(value("domain names")?),
            "--no-acme" => a.no_acme = true,
            "--acme-email" => a.acme_email = Some(value("an email address")?),
            "--acme-staging" => a.acme_staging = true,
            "--acme-http-port" => {
                a.acme_http_port =
                    Some(value("a number")?.parse().map_err(|_| "--acme-http-port needs a number from 0 to 65535".to_string())?)
            }
            "--tunnel" => a.tunnel = Some(true),
            "--no-tunnel" => a.tunnel = Some(false),
            "--tunnel-token" => a.tunnel_token = Some(value("a token")?),
            "--cloudflared" => a.cloudflared = Some(value("a file")?),
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
    if a.tunnel_token.is_none() {
        a.tunnel_token = std::env::var("CLOUDFLARE_TUNNEL_TOKEN").ok().filter(|p| !p.is_empty());
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

/// Station name, an administrator's username and password, and the port: from the options, else
/// asked for at the terminal.
fn setup(args: &Args, old: Option<ServerCfg>, accounts: &mut Accounts) -> Result<ServerCfg, String> {
    let mut cfg = old.unwrap_or_default();
    cfg.move_user(accounts);
    let tty = std::io::stdin().is_terminal();
    if tty {
        println!("RadioMaster Server setup — desktop apps sign in with this username and password.");
    }
    cfg.name = match &args.name {
        Some(n) => n.clone(),
        None if tty => ask("Station name", &cfg.name),
        None => cfg.name,
    };
    let admin = accounts.users.iter().find(|u| u.admin && !u.disabled).map(|u| u.name.clone()).unwrap_or_default();
    let user = match &args.user {
        Some(u) => u.clone(),
        None if tty => ask("Administrator username", &admin),
        None => admin,
    };
    let user = user.trim().to_string();
    if user.chars().count() < 2 {
        return Err("a username of at least 2 characters is needed (--user NAME)".into());
    }
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
    let password_hash = auth::hash(&password)?;
    match accounts.users.iter_mut().find(|u| u.name == user) {
        Some(u) => {
            u.password_hash = password_hash;
            u.admin = true;
            u.disabled = false;
        }
        None => accounts.users.push(auth::Operator { name: user, password_hash, admin: true, perms: Default::default(), disabled: false }),
    }
    cfg.port = match args.port {
        Some(p) => p,
        None if tty => ask("Port", &cfg.port.to_string()).parse().map_err(|_| "the port must be a number from 1 to 65535".to_string())?,
        None => cfg.port,
    };
    if let Some(b) = &args.bind {
        cfg.bind = b.clone();
    }
    cfg.https = match args.https {
        Some(h) => h,
        None if tty => yes_no("Serve over HTTPS (a self-signed certificate is made unless you give --cert and --key)", cfg.https),
        None => cfg.https,
    };
    if cfg.https && args.acme.is_none() && !args.no_acme && args.cert.is_none() && tty {
        let d = ask("Domain name for a free Let's Encrypt certificate (port 80 must reach this machine; blank: none)", &cfg.acme_domains);
        cfg.acme = !d.trim().is_empty();
        cfg.acme_domains = acme::domains(&d).join(", ");
        if cfg.acme {
            cfg.acme_email = ask("Email for Let's Encrypt's notices (optional)", &cfg.acme_email);
        }
    }
    cfg.portal = match args.portal {
        Some(p) => p,
        None if tty => yes_no("Turn on the web portal (run the station from a browser)", cfg.portal),
        None => cfg.portal,
    };
    cfg.portal_port = match args.portal_port {
        Some(p) => p,
        None if tty && cfg.portal => ask("Web portal port", &cfg.portal_port.to_string())
            .parse()
            .map_err(|_| "the port must be a number from 1 to 65535".to_string())?,
        None => cfg.portal_port,
    };
    if args.no_update_check {
        cfg.check_updates = false;
    }
    apply_tls_args(args, &mut cfg);
    Ok(cfg)
}

fn yes_no(prompt: &str, default: bool) -> bool {
    let a = ask(&format!("{prompt} (y/n)"), if default { "y" } else { "n" });
    a.to_lowercase().starts_with('y')
}

/// --cert and --key (a given certificate turns HTTPS on), Let's Encrypt and the Cloudflare Tunnel.
fn apply_tls_args(args: &Args, cfg: &mut ServerCfg) {
    if let Some(c) = &args.cert {
        cfg.cert_file = c.clone();
        cfg.https = args.https.unwrap_or(true);
        cfg.acme = false;
    }
    if let Some(k) = &args.key {
        cfg.key_file = k.clone();
    }
    if let Some(d) = &args.acme {
        cfg.acme_domains = acme::domains(d).join(", ");
        cfg.acme = !cfg.acme_domains.is_empty();
        cfg.https = args.https.unwrap_or(true);
    }
    if args.no_acme {
        cfg.acme = false;
    }
    if let Some(e) = &args.acme_email {
        cfg.acme_email = e.trim().to_string();
    }
    if args.acme_staging {
        cfg.acme_staging = true;
    }
    if let Some(p) = args.acme_http_port {
        cfg.acme_http_port = p;
    }
    if let Some(t) = args.tunnel {
        cfg.tunnel = t;
    }
    if let Some(t) = &args.tunnel_token {
        cfg.tunnel_token = t.trim().to_string();
        cfg.tunnel = args.tunnel.unwrap_or(true);
    }
    if let Some(c) = &args.cloudflared {
        cfg.cloudflared = c.clone();
    }
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
        let mut accounts = Accounts::load();
        match setup(&args, saved, &mut accounts) {
            Ok(c) => {
                if let Err(e) = accounts.save().map_err(std::io::Error::other).and_then(|_| c.save()) {
                    eprintln!("Could not save the settings in {}: {e}", config::data_dir().display());
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
        let mut c = saved.unwrap_or_default();
        // a server set up before v0.2.9: its one account becomes the first administrator
        let mut accounts = Accounts::load();
        if c.move_user(&mut accounts) {
            match accounts.save().map_err(std::io::Error::other).and_then(|_| c.save()) {
                Ok(()) => println!("Moved the server's account to {}", config::data_dir().join("operators.json").display()),
                Err(e) => eprintln!("Could not move the server's account to operators.json: {e}"),
            }
        }
        c
    };
    if args.no_update_check {
        scfg.check_updates = false;
    }
    if let Some(p) = args.port {
        scfg.port = p;
    }
    if let Some(b) = &args.bind {
        scfg.bind = b.clone();
    }
    if let Some(h) = args.https {
        scfg.https = h;
    }
    if let Some(p) = args.portal {
        scfg.portal = p;
    }
    if let Some(p) = args.portal_port {
        scfg.portal_port = p;
    }
    apply_tls_args(&args, &mut scfg);
    if scfg.portal && scfg.portal_port == scfg.port {
        eprintln!("The web portal needs a port of its own (--portal-port), not {}", scfg.port);
        std::process::exit(2);
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
    let host = || {
        let tx = Mutex::new(ctl_tx.clone());
        let control = Mutex::new(ctl_tx.clone());
        server::Host {
            control: Box::new(move |c| {
                let _ = control.lock().send(match c {
                    server::Control::Stop => Ctl::Quit,
                    server::Control::Restart => Ctl::Restart,
                });
            }),
            reapply_audio: Box::new(move || {
                let _ = tx.lock().send(Ctl::Audio);
            }),
            audio_report: audio_report.clone(),
            forced_silent: args.no_audio,
        }
    };
    // a copy started by a restart waits for the old one to let go of the ports
    let restarted = std::env::var_os(selfupdate::RESTARTED_ENV).is_some();
    let started = std::time::Instant::now();
    let listening = loop {
        match server::spawn(core.clone(), scfg.clone(), host()) {
            Err(_) if restarted && started.elapsed() < Duration::from_secs(30) => thread::sleep(Duration::from_millis(500)),
            r => break r,
        }
    };
    match listening {
        Ok(l) => {
            let scheme = if l.fingerprint.is_some() { "https" } else { "http" };
            println!("Listening on {scheme}://{} — sign in from File → Connect to Server", l.addr);
            if let Some(fp) = &l.fingerprint {
                if scfg.acme_order().is_some() && l.self_signed {
                    println!("HTTPS: getting a Let's Encrypt certificate for {} (self-signed until then)", scfg.acme_domains);
                } else if l.self_signed {
                    println!("HTTPS: self-signed certificate. The app asks once whether to trust it; check that it shows");
                    println!("       SHA-256 {fp}");
                } else {
                    println!("HTTPS: certificate {fp}");
                }
            }
            if let Some(p) = l.portal {
                println!("Web portal: {scheme}://{p} (sign in with the same username and password)");
            }
            if scfg.tunnel {
                println!("Cloudflare Tunnel: starting cloudflared ({})", tunnel::find(&scfg.cloudflared).display());
            }
        }
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
    let mut restart = false;
    loop {
        let quit = match ctl_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ctl::Quit) => true,
            Ok(Ctl::Restart) => {
                restart = true;
                true
            }
            Ok(Ctl::Audio) => {
                open_audio(&mut audio, ServerCfg::load().is_none_or(|c| c.audio));
                false
            }
            Err(_) => false,
        };
        if let Some(msg) = audio.watchdog() {
            println!("Audio: {msg}");
            *audio_report.lock() = audio.report.clone();
        }
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
    if restart {
        if selfupdate::supervised() {
            println!("Restarting (the service manager starts it again)…");
            std::process::exit(selfupdate::RESTART_EXIT);
        }
        match selfupdate::start_again() {
            Ok(()) => println!("Restarting…"),
            Err(e) => eprintln!("Could not start again: {e}"),
        }
    }
}
