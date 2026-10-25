#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
mod app;
mod audio;
mod auth;
mod azuracast;
mod browser;
mod cleanup;
mod config;
mod dsp;
mod engine;
mod fade;
mod generator;
mod i18n;
mod items;
mod library;
mod live;
mod mic;
mod pdf;
mod playout;
mod plugins;
mod remote;
mod report;
mod scheduler;
mod stream;
mod theme;
mod updater;
mod workzones;

use parking_lot::Mutex;

/// The RadioMaster icon (window title bar / taskbar).
pub fn app_icon() -> Option<eframe::egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?.to_rgba8();
    Some(eframe::egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}
use std::{sync::Arc, thread, time::Duration};

fn main() -> eframe::Result<()> {
    // release builds have no console window: keep a record of any crash
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{} RadioMaster crashed: {info}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        eprintln!("{msg}");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(config::data_dir().join("crash.log")) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
    }));
    let engine = engine::Engine::new_detached();
    let cfg = config::Config::load();
    if let Err(e) = i18n::load(&cfg.language) {
        eprintln!("language: {e}");
    }
    engine.set_volume(cfg.volume);
    let remote = cfg.remote.clone();
    let core = Arc::new(Mutex::new(playout::Playout::new(engine, cfg)));
    if remote.enabled {
        remote::spawn(core.clone(), remote.port, remote.password);
    }

    // Automation thread: keeps playing/advancing/scheduling even if the window is minimized.
    {
        let c = core.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(50));
            c.lock().tick();
        });
    }

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([760.0, 480.0])
        .with_title("RadioMaster — Radio Automation")
        .with_app_id("RadioMaster");
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    // vsync off: with Additional windows open, each window would otherwise wait for its own screen refresh
    // in turn and the whole UI would slow down. egui only redraws when something changes, so this costs nothing.
    let opts = eframe::NativeOptions { viewport, vsync: false, ..Default::default() };
    eframe::run_native("RadioMaster", opts, Box::new(move |cc| Ok(Box::new(app::App::new(cc, core)))))
}
