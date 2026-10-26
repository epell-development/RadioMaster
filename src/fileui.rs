//! Picking files and folders. On this computer that is the system's own dialog; while connected to
//! RadioMaster Server it is a browser of the server's folders (the files the station plays are
//! there), with an Upload button for files from this computer.
use super::*;
use crate::link::Fs;
use crate::playout::sync::FsList;
use std::path::Path;

/// What a dialog picks. Files are filtered by (filter name, extensions); no extensions: any file.
#[derive(Clone, Copy)]
pub(super) enum Pick {
    Folder,
    File(&'static str, &'static [&'static str]),
    Files(&'static str, &'static [&'static str]),
}

/// Audio files, for `Pick::File` / `Pick::Files`.
pub(super) const AUDIO: (&str, &[&str]) = ("Audio", &crate::library::EXTS);
/// Playlists.
pub(super) const M3U: (&str, &[&str]) = ("Playlist", &["m3u", "m3u8"]);

/// What to do with the paths picked (on a background thread, so it may read the disk or the server).
type Then = Box<dyn FnOnce(&Fs, Vec<PathBuf>) -> Option<Dlg> + Send>;

/// The server's file browser, while it is open.
struct Picker {
    what: Pick,
    title: String,
    then: Then,
    /// the folder shown (empty until the server says where browsing starts)
    dir: PathBuf,
    typed: String,
    list: Option<Arc<FsList>>,
    loading: Option<mpsc::Receiver<FsList>>,
    selected: Vec<PathBuf>,
    all: bool,
    /// uploads from this computer under way: (done, total, errors)
    uploading: Option<mpsc::Receiver<(usize, usize, Vec<String>)>>,
    upload_msg: String,
}

#[derive(Default)]
pub(super) struct FileUi {
    picker: Option<Picker>,
    /// asked for by `pick` (which only borrows the app), shown from the next frame
    asked: std::cell::RefCell<Option<Picker>>,
    /// where the server's file browser was last (it opens there again)
    last_dir: PathBuf,
}

impl FileUi {
    pub(super) fn window_open(&self) -> bool {
        self.picker.is_some() || self.asked.borrow().is_some()
    }

    /// Forget the last folder (another server, or back to this computer).
    pub(super) fn reset(&mut self) {
        self.picker = None;
        self.asked.get_mut().take();
        self.last_dir = PathBuf::new();
    }
}

impl Pick {
    fn files(self) -> bool {
        !matches!(self, Pick::Folder)
    }
    fn exts(self) -> &'static [&'static str] {
        match self {
            Pick::Folder => &[],
            Pick::File(_, e) | Pick::Files(_, e) => e,
        }
    }
    /// The file can be picked (its extension is one of the filter's).
    fn fits(self, p: &Path) -> bool {
        let exts = self.exts();
        exts.is_empty() || p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
}

impl App {
    /// Where picked files live now: the server while connected, else this computer.
    pub(super) fn fs(&self) -> Fs {
        match self.srv.session() {
            Some(s) => Fs::Server(s),
            None => Fs::Local,
        }
    }

    /// Pick files or a folder, then `then` turns the paths into a dialog result. While connected the
    /// server's folders are shown; otherwise the system's dialog.
    pub(super) fn pick(&self, what: Pick, title: &str, then: impl FnOnce(&Fs, Vec<PathBuf>) -> Option<Dlg> + Send + 'static) {
        if self.srv.connected() {
            let dir = self.files.last_dir.clone();
            *self.files.asked.borrow_mut() = Some(Picker {
                what,
                title: title.to_string(),
                then: Box::new(then),
                typed: dir.display().to_string(),
                dir,
                list: None,
                loading: None,
                selected: vec![],
                all: false,
                uploading: None,
                upload_msg: String::new(),
            });
            return;
        }
        self.dlg(move || {
            let mut d = rfd::FileDialog::new().set_title(title_text(what));
            if let Pick::File(name, exts) | Pick::Files(name, exts) = what {
                if !exts.is_empty() {
                    d = d.add_filter(name, exts);
                }
            }
            let picked = match what {
                Pick::Folder => d.pick_folder().map(|p| vec![p]),
                Pick::File(..) => d.pick_file().map(|p| vec![p]),
                Pick::Files(..) => d.pick_files(),
            }?;
            then(&Fs::Local, picked)
        });
    }

    pub(super) fn server_picker_window(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.files.asked.get_mut().take() {
            self.files.picker = Some(p);
        }
        let Some(mut p) = self.files.picker.take() else { return };
        let fs = self.fs();
        if !fs.is_server() {
            return; // disconnected meanwhile
        }
        if let Some(rx) = &p.loading {
            if let Ok(list) = rx.try_recv() {
                p.loading = None;
                p.dir = list.path.clone();
                p.typed = list.path.display().to_string();
                p.selected.retain(|s| list.entries.iter().any(|e| &e.path == s));
                p.list = Some(Arc::new(list));
            }
        }
        if let Some(rx) = &p.uploading {
            if let Ok((done, total, errors)) = rx.try_recv() {
                p.upload_msg = if errors.is_empty() { format!("Uploaded {done} of {total}") } else { errors.join("; ") };
                if done + errors.len() >= total {
                    p.uploading = None;
                    p.list = None; // list the folder again, with the new files
                }
            }
        }
        if p.list.is_none() && p.loading.is_none() {
            let (tx, rx) = mpsc::channel();
            let (fs, dir, all, ctx, folders) =
                (fs.clone(), p.dir.clone(), p.all, ctx.clone(), self.core.lock().cfg.library_folders.clone());
            thread::spawn(move || {
                let _ = tx.send(fs.list(&dir, all, &folders));
                ctx.request_repaint();
            });
            p.loading = Some(rx);
        }
        let mut open = true;
        let mut done: Option<Vec<PathBuf>> = None;
        let mut cancel = false;
        let mut go: Option<PathBuf> = None;
        let mut upload = false;
        let title = format!("{} — {}", p.title, tr("on the server"));
        Self::window_for(ctx, &title, [640.0, 520.0]).id(egui::Id::new("server_picker")).collapsible(false).open(&mut open).show(
            ctx,
            |ui| {
                let list = p.list.clone();
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("⬆").on_hover_text(tr("Up one folder")).clicked() {
                        go = list.as_ref().and_then(|l| l.parent.clone());
                    }
                    for place in list.iter().flat_map(|l| l.places.iter()) {
                        let label =
                            place.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| place.display().to_string());
                        if ui.small_button(label).on_hover_text(place.display().to_string()).clicked() {
                            go = Some(place.clone());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut p.typed).desired_width(ui.available_width() - 150.0));
                    if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.button(tr("Go")).clicked() {
                        go = Some(PathBuf::from(p.typed.trim()));
                    }
                    if ui.checkbox(&mut p.all, tr("All files")).on_hover_text(tr("Show every file, not only audio")).changed() {
                        p.list = None;
                    }
                });
                ui.separator();
                let footer = 70.0;
                egui::ScrollArea::vertical().max_height((ui.available_height() - footer).max(80.0)).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        let Some(list) = &list else {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(tr("Asking the server…"));
                            });
                            return;
                        };
                        if !list.error.is_empty() {
                            ui.colored_label(RED, format!("{}: {}", list.path.display(), list.error));
                        }
                        for e in list.entries.iter().take(5000) {
                            let name = e.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let pickable = if e.dir { !p.what.files() } else { p.what.files() && p.what.fits(&e.path) };
                            let sel = p.selected.contains(&e.path);
                            let text = if e.dir {
                                RichText::new(format!("📁 {name}"))
                            } else if pickable {
                                RichText::new(format!("🎵 {name}"))
                            } else {
                                RichText::new(format!("   {name}")).color(GRAY)
                            };
                            let r = ui.selectable_label(sel, text);
                            if r.double_clicked() {
                                if e.dir {
                                    go = Some(e.path.clone());
                                } else if pickable {
                                    done = Some(vec![e.path.clone()]);
                                }
                            } else if r.clicked() && pickable {
                                let multi = matches!(p.what, Pick::Files(..)) && ui.input(|i| i.modifiers.command || i.modifiers.shift);
                                if multi {
                                    if sel {
                                        p.selected.retain(|s| s != &e.path);
                                    } else {
                                        p.selected.push(e.path.clone());
                                    }
                                } else {
                                    p.selected = vec![e.path.clone()];
                                }
                            }
                        }
                        if list.entries.is_empty() && list.error.is_empty() {
                            ui.label(RichText::new(tr("Nothing here.")).color(GRAY));
                        }
                    },
                );
                ui.separator();
                ui.label(
                    RichText::new(match p.what {
                        Pick::Folder => tr("Open a folder, then choose it (or select one of its folders)."),
                        Pick::File(..) => tr("Double-click a file, or select it and choose it."),
                        Pick::Files(..) => tr("Ctrl+click to select several files."),
                    })
                    .small()
                    .color(GRAY),
                );
                ui.horizontal(|ui| {
                    let label = match p.what {
                        Pick::Folder if p.selected.is_empty() => tr("Choose this folder"),
                        _ => tr("Choose"),
                    };
                    let ready = p.list.as_ref().is_some_and(|l| l.error.is_empty()) && (!p.what.files() || !p.selected.is_empty());
                    if ui.add_enabled(ready, egui::Button::new(RichText::new(label).strong())).clicked() {
                        done = Some(if p.selected.is_empty() { vec![p.dir.clone()] } else { p.selected.clone() });
                    }
                    if ui
                        .add_enabled(p.uploading.is_none() && p.list.is_some(), egui::Button::new(tr("Upload from this computer…")))
                        .on_hover_text(tr("Copy files from this computer into this folder on the server"))
                        .clicked()
                    {
                        upload = true;
                    }
                    if p.uploading.is_some() {
                        ui.spinner();
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
                if !p.upload_msg.is_empty() {
                    ui.label(RichText::new(&p.upload_msg).small());
                }
            },
        );
        if let Some(dir) = go {
            p.dir = dir;
            p.list = None;
            p.loading = None;
            p.selected.clear();
        }
        if upload {
            let (tx, rx) = mpsc::channel();
            let (fs, dir, ctx, what) = (fs.clone(), p.dir.clone(), ctx.clone(), p.what);
            thread::spawn(move || {
                let mut d = rfd::FileDialog::new();
                if let Pick::File(name, exts) | Pick::Files(name, exts) = what {
                    if !exts.is_empty() {
                        d = d.add_filter(name, exts);
                    }
                }
                let files = d.pick_files().unwrap_or_default();
                let mut errors = vec![];
                let _ = tx.send((0, files.len(), vec![]));
                for (i, f) in files.iter().enumerate() {
                    if let Err(e) = fs.upload(f, &dir) {
                        errors.push(e);
                    }
                    let _ = tx.send((i + 1 - errors.len(), files.len(), errors.clone()));
                    ctx.request_repaint();
                }
                ctx.request_repaint();
            });
            p.uploading = Some(rx);
            p.upload_msg = tr("Uploading…").into();
        }
        if !p.dir.as_os_str().is_empty() {
            self.files.last_dir = p.dir.clone();
        }
        if let Some(paths) = done {
            let (tx, ctx, then) = (self.dlg_tx.clone(), ctx.clone(), p.then);
            thread::spawn(move || {
                if let Some(d) = then(&fs, paths) {
                    let _ = tx.send(d);
                    ctx.request_repaint();
                }
            });
            return;
        }
        if open && !cancel {
            self.files.picker = Some(p);
        }
    }
}

fn title_text(what: Pick) -> String {
    match what {
        Pick::Folder => tr("Choose a folder"),
        Pick::File(..) => tr("Choose a file"),
        Pick::Files(..) => tr("Choose files"),
    }
    .into()
}
