//! Library cleanup tool: duplicate files, lost and unreadable tracks, and tidying genre and artist
//! tags. Scans run in the background. Every change is listed first and only happens after
//! "Do it"; removed files go to the Removed tracks folder unless "Delete for good" is chosen.
use super::*;
use crate::cleanup::{self, Count, DupGroup, Health, Merge, Msg, TagFix};
use std::collections::HashSet;
use std::path::Path;

#[derive(Default, PartialEq, Clone, Copy)]
pub(super) enum CleanView {
    #[default]
    Duplicates,
    Missing,
    Genres,
    Artists,
}

/// A change waiting for "Do it": what it does, and one line per file for the preview.
struct Pending {
    what: String,
    lines: Vec<String>,
    /// red "Do it" button (files are deleted for good)
    danger: bool,
    act: Act,
}

enum Act {
    /// move or delete files; `relink`: playlist rows etc. of a removed copy go to the kept copy;
    /// `rows`: also take the files' playlist rows out
    Remove {
        files: Vec<PathBuf>,
        delete: bool,
        relink: Vec<(PathBuf, PathBuf)>,
        rows: bool,
    },
    Tags(Vec<(PathBuf, TagFix)>),
    Relink(Vec<(PathBuf, PathBuf)>),
    /// take out of the library and the playlists (files that are gone)
    Forget(Vec<PathBuf>),
}

/// Genre or artist spellings, worked out again whenever the library changes.
#[derive(Default)]
struct TagState {
    ver: u64,
    counts: Vec<Count>,
    /// (apply?, merge with an editable target)
    sugg: Vec<(bool, Merge)>,
    /// spellings ticked in the full list, and what to merge them into
    sel: BTreeSet<String>,
    target: String,
    filter: String,
    /// artists only: library tracks with no artist and an "Artist - Title" title
    split: Vec<usize>,
}

#[derive(Default)]
pub(super) struct CleanUi {
    view: CleanView,
    rx: Option<mpsc::Receiver<Msg>>,
    /// (what is running, done, of — 0 when unknown)
    busy: Option<(String, usize, usize)>,
    /// for the files being removed: (relink pairs, take playlist rows out)
    after_remove: (Vec<(PathBuf, PathBuf)>, bool),
    pending: Option<Pending>,
    /// delete files for good instead of moving them to the Removed tracks folder
    delete: bool,
    note: String,
    // duplicates
    dups: Option<Vec<DupGroup>>,
    /// groups left out of "Remove"
    dup_skip: BTreeSet<usize>,
    /// different files of the same song too (not only identical copies)
    dup_any: bool,
    dup_init: bool,
    // missing / broken
    health: Option<Health>,
    lost_on: Vec<bool>,
    lost_pick: Vec<usize>,
    broken_on: Vec<bool>,
    pub extra: Option<PathBuf>,
    genres: TagState,
    artists: TagState,
}

impl CleanUi {
    /// Take files that have been dealt with off the missing / unreadable lists.
    fn drop_checked(&mut self, gone: &HashSet<PathBuf>) {
        let Some(h) = &mut self.health else { return };
        let keep: Vec<bool> = h.lost.iter().map(|l| !gone.contains(&l.path)).collect();
        retain_by(&mut h.lost, &keep);
        retain_by(&mut self.lost_on, &keep);
        retain_by(&mut self.lost_pick, &keep);
        let keep: Vec<bool> = h.broken.iter().map(|b| !gone.contains(&b.path)).collect();
        retain_by(&mut h.broken, &keep);
        retain_by(&mut self.broken_on, &keep);
    }
}

/// Keep the items whose `keep` flag (same position) is set.
fn retain_by<T>(v: &mut Vec<T>, keep: &[bool]) {
    let mut k = keep.iter();
    v.retain(|_| k.next().copied().unwrap_or(true));
}

fn genre_of(t: &Track) -> &str {
    &t.genre
}
fn artist_of(t: &Track) -> &str {
    &t.artist
}

fn short(p: &Path) -> String {
    p.display().to_string()
}

impl App {
    /// Pick up results from a cleanup job (also when its window has been closed).
    pub(super) fn clean_poll(&mut self, core: &mut Playout) {
        let Some(rx) = &self.clean.rx else { return };
        let mut msgs = vec![];
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        let c = &mut self.clean;
        for m in msgs {
            match m {
                Msg::Progress(what, i, n) => c.busy = Some((what, i, n)),
                Msg::Dups(g) => {
                    c.note = format!("{} {}", g.len(), tr("groups of duplicates found"));
                    c.dup_skip.clear();
                    c.dups = Some(g);
                    c.busy = None;
                }
                Msg::Health(h) => {
                    c.note = format!(
                        "{} {} · {} {} · {} {}",
                        h.checked,
                        tr("files checked"),
                        h.lost.len(),
                        tr("missing"),
                        h.broken.len(),
                        tr("unreadable")
                    );
                    c.lost_on = h.lost.iter().map(|l| !l.found.is_empty()).collect();
                    c.lost_pick = vec![0; h.lost.len()];
                    c.broken_on = vec![true; h.broken.len()];
                    c.health = Some(h);
                    c.busy = None;
                }
                Msg::Removed(res) => {
                    let done: HashSet<PathBuf> = res.iter().filter(|(_, r)| r.is_ok()).map(|(p, _)| p.clone()).collect();
                    let failed: Vec<String> =
                        res.iter().filter_map(|(p, r)| r.as_ref().err().map(|e| format!("{}: {e}", short(p)))).collect();
                    let (relink, rows) = std::mem::take(&mut c.after_remove);
                    let relink: Vec<(PathBuf, Track)> = relink
                        .into_iter()
                        .filter(|(from, _)| done.contains(from))
                        .map(|(from, to)| (from, Track { path: to, ..Default::default() }))
                        .collect();
                    if !relink.is_empty() {
                        core.relink(relink);
                    }
                    if rows {
                        core.remove_rows_for(&done);
                    }
                    core.forget_tracks(&done);
                    // drop what was removed from the lists on screen
                    if let Some(g) = &mut c.dups {
                        for grp in g.iter_mut() {
                            let keep = grp.copies[grp.keep].path.clone();
                            grp.copies.retain(|x| !done.contains(&x.path));
                            grp.keep = grp.copies.iter().position(|x| x.path == keep).unwrap_or(0);
                        }
                        g.retain(|grp| grp.copies.len() > 1);
                        c.dup_skip.clear();
                    }
                    c.drop_checked(&done);
                    c.note = format!(
                        "{} {}",
                        done.len(),
                        if res.iter().any(|(_, r)| matches!(r, Ok(None))) {
                            tr("files deleted")
                        } else {
                            tr("files moved to the Removed tracks folder")
                        }
                    );
                    if !failed.is_empty() {
                        c.note += &format!(" · {} {}: {}", failed.len(), tr("failed"), failed[0]);
                    }
                    c.busy = None;
                }
                Msg::Written(res) => {
                    let ok: Vec<Track> = res.iter().filter_map(|(_, r)| r.as_ref().ok().cloned()).collect();
                    let failed: Vec<String> =
                        res.iter().filter_map(|(p, r)| r.as_ref().err().map(|e| format!("{}: {e}", short(p)))).collect();
                    c.note = format!("{} {}", ok.len(), tr("files retagged"));
                    if !failed.is_empty() {
                        c.note += &format!(" · {} {}: {}", failed.len(), tr("failed"), failed[0]);
                    }
                    core.tags_written(ok);
                    c.busy = None;
                }
                Msg::Relinked(pairs) => {
                    let fixed: HashSet<PathBuf> = pairs.iter().map(|(p, _)| p.clone()).collect();
                    c.note = format!("{} {}", fixed.len(), tr("tracks pointed at their new place"));
                    core.relink(pairs);
                    c.drop_checked(&fixed);
                    c.busy = None;
                }
            }
        }
        if c.busy.is_none() {
            c.rx = None;
        }
    }

    fn clean_job(&mut self, what: &str, job: impl FnOnce(mpsc::Sender<Msg>, egui::Context) + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        self.clean.rx = Some(rx);
        self.clean.busy = Some((what.to_string(), 0, 0));
        self.clean.note.clear();
        let ctx = self.ctx.clone();
        thread::spawn(move || job(tx, ctx));
    }

    pub(super) fn tab_cleanup(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal_wrapped(|ui| {
            for (v, l) in [
                (CleanView::Duplicates, tr("👯 Duplicates")),
                (CleanView::Missing, tr("🔗 Missing & broken")),
                (CleanView::Genres, tr("🏷 Genres")),
                (CleanView::Artists, tr("🎤 Artists")),
            ] {
                if ui.selectable_label(self.clean.view == v, RichText::new(l).strong()).clicked() && self.clean.pending.is_none() {
                    self.clean.view = v;
                }
            }
        });
        ui.separator();
        if let Some((what, i, n)) = &self.clean.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                let what = match what.as_str() {
                    "Checking files" => tr("Checking files"),
                    "Comparing files" => tr("Comparing files"),
                    "Comparing quality" => tr("Comparing quality"),
                    "Looking for lost files" => tr("Looking for lost files"),
                    "Removing files" => tr("Removing files"),
                    "Writing tags" => tr("Writing tags"),
                    _ => tr("Working…"),
                };
                if *n > 0 {
                    ui.add(egui::ProgressBar::new(*i as f32 / *n as f32).desired_width(220.0).text(format!("{what} {i} / {n}")));
                } else {
                    ui.label(format!("{what}… {}", if *i > 0 { i.to_string() } else { String::new() }));
                }
            });
            return;
        }
        if !self.clean.note.is_empty() {
            ui.label(RichText::new(&self.clean.note).color(AMBER));
        }
        if self.clean.pending.is_some() {
            self.clean_confirm(ui, core);
            return;
        }
        if core.library.is_empty() && matches!(self.clean.view, CleanView::Duplicates | CleanView::Genres | CleanView::Artists) {
            ui.label(tr("The library is empty. Add a music folder (File → Add library folder…) and scan it first."));
            return;
        }
        if core.scanning {
            ui.label(RichText::new(tr("The library is being scanned; results may change when it finishes.")).color(GRAY));
        }
        match self.clean.view {
            CleanView::Duplicates => self.clean_dups(ui, core),
            CleanView::Missing => self.clean_missing(ui, core),
            CleanView::Genres => self.clean_tags(ui, core, false),
            CleanView::Artists => self.clean_tags(ui, core, true),
        }
    }

    /// The preview of a change, with "Do it" / "Cancel" above the list so they stay on screen.
    fn clean_confirm(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let p = self.clean.pending.as_ref().unwrap();
        ui.label(RichText::new(&p.what).strong());
        let mut go = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let text = if p.danger { RichText::new(tr("Delete!")).color(RED).strong() } else { RichText::new(tr("✔ Do it")).strong() };
            go = ui.button(text).clicked();
            cancel = ui.button(tr("Cancel")).clicked();
            ui.label(RichText::new(tr("Every change is listed in cleanup-log.csv in the data folder.")).small().color(GRAY));
        });
        ui.separator();
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        egui::ScrollArea::vertical().id_salt("clean_preview").auto_shrink([false, false]).show_rows(
            ui,
            row_h,
            p.lines.len(),
            |ui, range| {
                for l in &p.lines[range] {
                    ui.add(egui::Label::new(l).truncate());
                }
            },
        );
        if cancel {
            self.clean.pending = None;
        }
        if !go {
            return;
        }
        let p = self.clean.pending.take().unwrap();
        match p.act {
            Act::Remove { files, delete, relink, rows } => {
                self.clean.after_remove = (relink, rows);
                self.clean_job("Removing files", move |tx, ctx| cleanup::remove_files(files, delete, tx, || ctx.request_repaint()));
            }
            Act::Tags(fixes) => self.clean_job("Writing tags", move |tx, ctx| cleanup::write_tags(fixes, tx, || ctx.request_repaint())),
            Act::Relink(pairs) => self.clean_job("Relinking", move |tx, ctx| cleanup::relink(pairs, tx, || ctx.request_repaint())),
            Act::Forget(paths) => {
                let set: HashSet<PathBuf> = paths.into_iter().collect();
                let rows = core.remove_rows_for(&set);
                core.forget_tracks(&set);
                self.clean.drop_checked(&set);
                self.clean.note =
                    format!("{} {} · {rows} {}", set.len(), tr("tracks taken out of the library"), tr("playlist rows removed"));
            }
        }
    }

    /// "Move to the Removed tracks folder" / "Delete for good".
    fn clean_remove_mode(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("Removed files:"));
            ui.radio_value(&mut self.clean.delete, false, tr("move to the Removed tracks folder (can be put back)"));
            ui.radio_value(&mut self.clean.delete, true, RichText::new(tr("delete for good")).color(RED));
            if ui.small_button("📂").on_hover_text(tr("Open the Removed tracks folder")).clicked() {
                let d = cleanup::removed_folder();
                let _ = std::fs::create_dir_all(&d);
                crate::plugins::open_folder(&d);
            }
        });
    }

    fn clean_dups(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if !self.clean.dup_init {
            self.clean.dup_any = true;
            self.clean.dup_init = true;
        }
        let mut scan = false;
        ui.horizontal_wrapped(|ui| {
            scan = ui.button(RichText::new(tr("🔍 Find duplicates")).strong()).clicked();
            ui.checkbox(&mut self.clean.dup_any, tr("Also the same song in different files"))
                .on_hover_text(tr("Same artist and title, and lengths within 3 seconds. Off: only byte-for-byte identical copies."));
        });
        if scan {
            let lib = core.library.clone();
            let any = self.clean.dup_any;
            self.clean.dups = None;
            self.clean_job("Checking files", move |tx, ctx| cleanup::scan_duplicates(lib, any, 3.0, tx, || ctx.request_repaint()));
            return;
        }
        let Some(groups) = &self.clean.dups else {
            ui.label(
                RichText::new(tr(
                    "Looks through the library for copies of the same track. Nothing is removed until you have checked the list.",
                ))
                .color(GRAY),
            );
            return;
        };
        if groups.is_empty() {
            ui.label(tr("No duplicates found."));
            return;
        }
        let n_groups = groups.len();
        let extra: usize =
            groups.iter().enumerate().filter(|(i, _)| !self.clean.dup_skip.contains(i)).map(|(_, g)| g.copies.len() - 1).sum();
        ui.label(
            RichText::new(tr(
                "The best copy (lossless, then highest bitrate) is kept; pick another with its button. Untick a group to leave it alone.",
            ))
            .small()
            .color(GRAY),
        );
        let mut remove = false;
        ui.horizontal_wrapped(|ui| {
            remove = ui.add_enabled(extra > 0, egui::Button::new(format!("{} ({extra})…", tr("Remove the other copies")))).clicked();
            if ui.small_button(tr("All")).clicked() {
                self.clean.dup_skip.clear();
            }
            if ui.small_button(tr("None")).clicked() {
                self.clean.dup_skip = (0..n_groups).collect();
            }
        });
        self.clean_remove_mode(ui);
        let c = &mut self.clean;
        let delete = c.delete;
        let groups = c.dups.as_mut().unwrap();
        ui.separator();
        egui::ScrollArea::vertical().id_salt("clean_dups").auto_shrink([false, false]).show(ui, |ui| {
            for (gi, g) in groups.iter_mut().enumerate() {
                ui.push_id(("dup", gi), |ui| {
                    let mut on = !c.dup_skip.contains(&gi);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, RichText::new(&g.copies[0].label).strong()).changed() {
                            if on {
                                c.dup_skip.remove(&gi);
                            } else {
                                c.dup_skip.insert(gi);
                            }
                        }
                        if g.copies.iter().all(|x| x.identical) {
                            ui.label(RichText::new(tr("identical files")).small().color(GRAY));
                        }
                    });
                    ui.indent("copies", |ui| {
                        for (i, x) in g.copies.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let label = if g.keep == i { tr("Keep") } else { "" };
                                ui.radio_value(&mut g.keep, i, label);
                                let col = if g.keep == i || !on { ui.visuals().text_color() } else { RED };
                                ui.label(RichText::new(&x.info).monospace().small().color(col));
                                ui.add(egui::Label::new(RichText::new(short(&x.path)).small().color(col)).truncate())
                                    .on_hover_text(short(&x.path));
                            });
                        }
                    });
                });
            }
        });
        if remove {
            let mut files = vec![];
            let mut relink = vec![];
            let mut lines = vec![];
            for (_, g) in c.dups.as_ref().unwrap().iter().enumerate().filter(|(i, _)| !c.dup_skip.contains(i)) {
                let keep = &g.copies[g.keep].path;
                for (_, x) in g.copies.iter().enumerate().filter(|(i, _)| *i != g.keep) {
                    files.push(x.path.clone());
                    relink.push((x.path.clone(), keep.clone()));
                    lines.push(format!("✖ {}   ({} {})", short(&x.path), tr("keeping"), short(keep)));
                }
            }
            let what = if delete {
                format!("{} {} {}", tr("Delete"), files.len(), tr("files for good? Playlist rows that use them will play the kept copy."))
            } else {
                format!(
                    "{} {} {}",
                    tr("Move"),
                    files.len(),
                    tr("files to the Removed tracks folder? Playlist rows that use them will play the kept copy.")
                )
            };
            c.pending = Some(Pending { what, lines, danger: delete, act: Act::Remove { files, delete, relink, rows: false } });
        }
    }

    fn clean_missing(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal_wrapped(|ui| {
            if ui.button(RichText::new(tr("🔍 Check all tracks")).strong()).clicked() {
                let lib = core.library.clone();
                let rows = core.playlist_files();
                let mut folders: Vec<PathBuf> = core.cfg.library_folders.iter().map(PathBuf::from).collect();
                folders.extend(self.clean.extra.clone());
                self.clean.health = None;
                self.clean_job("Checking files", move |tx, ctx| cleanup::scan_health(lib, rows, folders, tx, || ctx.request_repaint()));
                return;
            }
            if ui
                .button(tr("📂 Also look in…"))
                .on_hover_text(tr("Another folder to look for moved files in (besides the library folders)"))
                .clicked()
            {
                self.dlg(|| rfd::FileDialog::new().pick_folder().map(Dlg::CleanupFolder));
            }
            if let Some(p) = &self.clean.extra {
                ui.label(RichText::new(short(p)).small().color(GRAY));
                if ui.small_button("✖").clicked() {
                    self.clean.extra = None;
                }
            }
        });
        let Some(h) = self.clean.health.take() else {
            ui.label(
                RichText::new(tr(
                    "Checks every library track and playlist row: files that have gone (and where they may have moved to) and files that can't be played.",
                ))
                .color(GRAY),
            );
            return;
        };
        if h.lost.is_empty() && h.broken.is_empty() {
            ui.label(tr("Every file is there and can be read."));
            self.clean.health = Some(h);
            return;
        }
        let (n_lost, n_broken) = (h.lost.len(), h.broken.len());
        let mut act: Option<&str> = None;
        let avail = ui.available_height();
        if n_lost > 0 {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} ({n_lost})", tr("Missing files")));
                let c = &self.clean;
                let fixable = (0..n_lost).filter(|&i| c.lost_on[i] && !h.lost[i].found.is_empty()).count();
                let on = c.lost_on.iter().filter(|x| **x).count();
                if ui.add_enabled(fixable > 0, egui::Button::new(format!("{} ({fixable})…", tr("🔗 Use the found files")))).clicked() {
                    act = Some("relink");
                }
                if ui.add_enabled(on > 0, egui::Button::new(format!("{} ({on})…", tr("Take out of the library and playlists")))).clicked()
                {
                    act = Some("forget");
                }
            });
            let c = &mut self.clean;
            egui::ScrollArea::vertical()
                .id_salt("clean_lost")
                .max_height(if n_broken > 0 { avail * 0.5 } else { f32::INFINITY })
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, l) in h.lost.iter().enumerate() {
                        ui.push_id(("lost", i), |ui| {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut c.lost_on[i], RichText::new(&l.label).strong());
                                let mut wh = vec![];
                                if l.in_library {
                                    wh.push(tr("library").to_string());
                                }
                                if l.rows > 0 {
                                    wh.push(format!("{} {}", l.rows, tr("playlist row(s)")));
                                }
                                ui.label(RichText::new(wh.join(", ")).small().color(GRAY));
                            });
                            ui.indent("lost_detail", |ui| {
                                ui.add(egui::Label::new(RichText::new(format!("✖ {}", short(&l.path))).small().color(RED)).truncate());
                                match l.found.len() {
                                    0 => {
                                        ui.label(RichText::new(tr("Not found anywhere")).small().color(GRAY));
                                    }
                                    1 => {
                                        ui.add(
                                            egui::Label::new(RichText::new(format!("→ {}", short(&l.found[0]))).small().color(GREEN))
                                                .truncate(),
                                        );
                                    }
                                    _ => {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new("→").color(GREEN));
                                            let pick = &mut c.lost_pick[i];
                                            egui::ComboBox::from_id_salt("found")
                                                .width(ui.available_width().min(520.0))
                                                .selected_text(RichText::new(short(&l.found[*pick])).small())
                                                .show_ui(ui, |ui| {
                                                    for (k, f) in l.found.iter().enumerate() {
                                                        ui.selectable_value(pick, k, short(f));
                                                    }
                                                });
                                        });
                                    }
                                }
                            });
                        });
                    }
                });
        }
        if n_broken > 0 {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} ({n_broken})", tr("Files that can't be played")));
                let on = self.clean.broken_on.iter().filter(|x| **x).count();
                if ui.add_enabled(on > 0, egui::Button::new(format!("{} ({on})…", tr("Remove")))).clicked() {
                    act = Some("remove");
                }
            });
            self.clean_remove_mode(ui);
            let c = &mut self.clean;
            egui::ScrollArea::vertical().id_salt("clean_broken").auto_shrink([false, false]).show(ui, |ui| {
                for (i, b) in h.broken.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut c.broken_on[i], &b.label);
                        ui.label(RichText::new(b.why).small().color(AMBER));
                        ui.add(egui::Label::new(RichText::new(short(&b.path)).small().color(GRAY)).truncate());
                    });
                }
            });
        }
        let c = &mut self.clean;
        match act {
            Some("relink") => {
                let pairs: Vec<(PathBuf, PathBuf)> = (0..h.lost.len())
                    .filter(|&i| c.lost_on[i] && !h.lost[i].found.is_empty())
                    .map(|i| (h.lost[i].path.clone(), h.lost[i].found[c.lost_pick[i]].clone()))
                    .collect();
                let lines = pairs.iter().map(|(a, b)| format!("{}  →  {}", short(a), short(b))).collect();
                let what = format!(
                    "{} {} {}",
                    tr("Point"),
                    pairs.len(),
                    tr("missing tracks at the files found? The library and playlist rows will use the new place.")
                );
                c.pending = Some(Pending { what, lines, danger: false, act: Act::Relink(pairs) });
            }
            Some("forget") => {
                let paths: Vec<PathBuf> = (0..h.lost.len()).filter(|&i| c.lost_on[i]).map(|i| h.lost[i].path.clone()).collect();
                let lines = paths.iter().map(|p| format!("✖ {}", short(p))).collect();
                let what = format!(
                    "{} {} {}",
                    tr("Take"),
                    paths.len(),
                    tr("missing tracks out of the library and remove their playlist rows (except the one on air)?")
                );
                c.pending = Some(Pending { what, lines, danger: false, act: Act::Forget(paths) });
            }
            Some("remove") => {
                let files: Vec<PathBuf> = (0..h.broken.len()).filter(|&i| c.broken_on[i]).map(|i| h.broken[i].path.clone()).collect();
                let lines = files.iter().map(|p| format!("✖ {}", short(p))).collect();
                let what = if c.delete {
                    format!(
                        "{} {} {}",
                        tr("Delete"),
                        files.len(),
                        tr("unplayable files for good, and take them out of the library and playlists?")
                    )
                } else {
                    format!(
                        "{} {} {}",
                        tr("Move"),
                        files.len(),
                        tr("unplayable files to the Removed tracks folder, and take them out of the library and playlists?")
                    )
                };
                let delete = c.delete;
                c.pending = Some(Pending { what, lines, danger: delete, act: Act::Remove { files, delete, relink: vec![], rows: true } });
            }
            _ => {}
        }
        c.health = Some(h);
    }

    /// Genres (`artist` false) or artists: suggested merges, and every spelling to merge by hand.
    fn clean_tags(&mut self, ui: &mut egui::Ui, core: &mut Playout, artist: bool) {
        let ver = core.library_ver;
        let s = if artist { &mut self.clean.artists } else { &mut self.clean.genres };
        if s.ver != ver {
            let field = if artist { artist_of } else { genre_of };
            s.counts = cleanup::counts(&core.library, field);
            let sugg = if artist { cleanup::artist_suggestions(&s.counts) } else { cleanup::genre_suggestions(&s.counts) };
            s.sugg = sugg.into_iter().map(|m| (true, m)).collect();
            s.split = if artist {
                core.library.iter().enumerate().filter(|(_, t)| cleanup::split_artist_title(t).is_some()).map(|(i, _)| i).collect()
            } else {
                vec![]
            };
            s.sel.retain(|x| s.counts.iter().any(|(c, _)| c == x));
            s.ver = ver;
        }
        // (question, changes, preview lines)
        type Fixes = (String, Vec<(PathBuf, TagFix)>, Vec<String>);
        let mut fixes: Option<Fixes> = None;
        let mk = |to: &str| if artist { TagFix::Artist(to.to_string()) } else { TagFix::Genre(to.to_string()) };
        let field = if artist { artist_of } else { genre_of };
        // every track whose tag is one of `map`'s keys gets the value
        let build = |map: &HashMap<String, String>| {
            let mut v = vec![];
            let mut lines = vec![];
            for t in &core.library {
                if let Some(to) = map.get(field(t)).filter(|to| to.as_str() != field(t)) {
                    v.push((t.path.clone(), mk(to)));
                    lines.push(format!("{}:  \"{}\"  →  \"{to}\"", t.label(), field(t)));
                }
            }
            (v, lines)
        };

        let avail = ui.available_height();
        // suggestions
        let n_on: usize = s.sugg.iter().filter(|(on, _)| *on).map(|(_, m)| m.changes()).sum();
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} ({})", tr("Suggested fixes"), s.sugg.len()));
            if ui.add_enabled(n_on > 0, egui::Button::new(format!("{} ({n_on} {})…", tr("Apply ticked"), tr("tracks")))).clicked() {
                let map: HashMap<String, String> = s
                    .sugg
                    .iter()
                    .filter(|(on, m)| *on && !m.to.trim().is_empty())
                    .flat_map(|(_, m)| m.from.iter().map(|(f, _)| (f.clone(), m.to.trim().to_string())))
                    .collect();
                let (v, l) = build(&map);
                fixes = Some((tr("Rewrite the tags of these tracks?").to_string(), v, l));
            }
            if ui.small_button(tr("All")).clicked() {
                s.sugg.iter_mut().for_each(|x| x.0 = true);
            }
            if ui.small_button(tr("None")).clicked() {
                s.sugg.iter_mut().for_each(|x| x.0 = false);
            }
        });
        ui.label(
            RichText::new(if artist {
                tr("Spellings of the same artist (case, spaces, \"The\", feat./ft.) — the most used one wins. You can change the result.")
            } else {
                tr("Spellings of the same genre (case, spaces, Hip-Hop / Hip Hop, R&B / RnB) — the most used one wins. You can change the result.")
            })
            .small()
            .color(GRAY),
        );
        egui::ScrollArea::vertical().id_salt(("clean_sugg", artist)).max_height(avail * 0.4).auto_shrink([false, true]).show(ui, |ui| {
            if s.sugg.is_empty() {
                ui.label(RichText::new(tr("Nothing to fix.")).color(GRAY));
            }
            for (i, (on, m)) in s.sugg.iter_mut().enumerate() {
                ui.push_id(("sugg", i), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(on, "");
                        let from: Vec<String> = m.from.iter().map(|(f, n)| format!("\"{f}\" ({n})")).collect();
                        ui.label(from.join(", "));
                        ui.label("→");
                        ui.add(egui::TextEdit::singleline(&mut m.to).desired_width(180.0));
                    });
                });
            }
        });
        if artist && !s.split.is_empty() {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} ({})", tr("No artist, title is \"Artist - Title\""), s.split.len()));
                if ui.button(tr("Split into artist and title…")).clicked() {
                    let mut v = vec![];
                    let mut lines = vec![];
                    for &i in &s.split {
                        let Some(t) = core.library.get(i) else { continue };
                        if let Some((a, b)) = cleanup::split_artist_title(t) {
                            lines.push(format!("\"{}\"  →  {}: \"{a}\", {}: \"{b}\"", t.title, tr("Artist"), tr("Title")));
                            v.push((t.path.clone(), TagFix::ArtistTitle(a, b)));
                        }
                    }
                    fixes = Some((tr("Fill in the artist from the title for these tracks?").to_string(), v, lines));
                }
            });
        }
        ui.separator();
        // every spelling, to merge or rename by hand
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} ({})", if artist { tr("All artists") } else { tr("All genres") }, s.counts.len()));
            ui.add(egui::TextEdit::singleline(&mut s.filter).desired_width(140.0).hint_text(tr("🔍 filter")));
            ui.label(tr("Change ticked to"));
            ui.add(egui::TextEdit::singleline(&mut s.target).desired_width(160.0));
            let n: usize = s.counts.iter().filter(|(c, _)| s.sel.contains(c) && *c != s.target.trim()).map(|(_, n)| n).sum();
            if ui.add_enabled(n > 0 && !s.target.trim().is_empty(), egui::Button::new(format!("{} ({n})…", tr("Apply")))).clicked() {
                let to = s.target.trim().to_string();
                let map: HashMap<String, String> = s.sel.iter().map(|f| (f.clone(), to.clone())).collect();
                let (v, l) = build(&map);
                fixes = Some((tr("Rewrite the tags of these tracks?").to_string(), v, l));
            }
            if !s.sel.is_empty() && ui.small_button(tr("Clear")).clicked() {
                s.sel.clear();
            }
        });
        let f = s.filter.to_lowercase();
        let shown: Vec<usize> = (0..s.counts.len()).filter(|&i| f.is_empty() || s.counts[i].0.to_lowercase().contains(&f)).collect();
        let row_h = ui.spacing().interact_size.y + 2.0;
        egui::ScrollArea::vertical().id_salt(("clean_all", artist)).auto_shrink([false, false]).show_rows(
            ui,
            row_h,
            shown.len(),
            |ui, range| {
                for &i in &shown[range] {
                    let (name, n) = &s.counts[i];
                    let mut on = s.sel.contains(name);
                    if ui.checkbox(&mut on, format!("{name}   ({n})")).changed() {
                        if on {
                            if s.target.trim().is_empty() {
                                s.target = name.clone();
                            }
                            s.sel.insert(name.clone());
                        } else {
                            s.sel.remove(name);
                        }
                    }
                }
            },
        );
        if let Some((what, v, lines)) = fixes {
            if v.is_empty() {
                self.clean.note = tr("Nothing would change.").to_string();
            } else {
                let what = format!("{what} ({} {})", v.len(), tr("files"));
                self.clean.pending = Some(Pending { what, lines, danger: false, act: Act::Tags(v) });
            }
        }
    }
}
