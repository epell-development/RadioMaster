//! Work-zone panels: the left / right / lower-right panels of the main window, Additional windows
//! (real OS windows, as many as you like), and the tabs that can live in any of them.

use super::*;
use crate::items::ItemKind;
use crate::workzones::{aux_title, AuxWindow, Pane};
use eframe::egui::ViewportBuilder;

/// Per-panel state of a web tab.
#[derive(Default)]
pub(super) struct WebState {
    tab: Option<Tab>,
    entry: String,
    target: String,
}

impl App {
    /// Tab caption: web panels show the name the user gave them.
    pub(super) fn tab_name(&self, tab: Tab) -> String {
        match tab.web_index().and_then(|i| self.workzones.web.get(i)) {
            Some(w) => format!("🌐 {}", w.name),
            None => tr(tab.label()).to_string(),
        }
    }

    fn website_default(&self, tab: Tab) -> String {
        tab.web_index().and_then(|i| self.workzones.web.get(i)).map(|w| w.url.clone()).unwrap_or_default()
    }

    fn pane_tabs(&mut self, ui: &mut egui::Ui, pane: Pane) -> Option<Tab> {
        let Some(pt) = self.workzones.pane(pane).cloned() else { return None };
        let mut change: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut move_to: Option<(usize, Pane)> = None;
        let mut add: Option<Tab> = None;
        let mut new_window: Option<usize> = None;
        let targets: Vec<Pane> = self.workzones.visible_panes().into_iter().filter(|p| *p != pane).collect();
        ui.horizontal(|ui| {
            ui.menu_button("➕", |ui| {
                for tab in Tab::PANEL {
                    if ui.add_enabled(!pt.tabs.contains(&tab), egui::Button::new(self.tab_name(tab))).clicked() {
                        add = Some(tab);
                        ui.close_menu();
                    }
                }
                ui.separator();
                for i in 0..self.workzones.web.len() {
                    let tab = Tab::web(i);
                    if ui.add_enabled(!pt.tabs.contains(&tab), egui::Button::new(self.tab_name(tab))).clicked() {
                        add = Some(tab);
                        ui.close_menu();
                    }
                }
                if self.workzones.web.len() < 64 && ui.button(tr("🌐 New web panel")).clicked() {
                    let n = self.workzones.web.len();
                    self.workzones.web.push(crate::workzones::WebSite { name: format!("Web {}", n + 1), url: String::new() });
                    add = Some(Tab::web(n));
                    ui.close_menu();
                }
            })
            .response
            .on_hover_text(tr("Add a tab to this panel"));
            egui::ScrollArea::horizontal().id_salt(("pane_tabs", pane.key())).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, tab) in pt.tabs.iter().copied().enumerate() {
                        let r = ui.selectable_label(pt.active == i, self.tab_name(tab));
                        if r.clicked() {
                            change = Some(i);
                        }
                        r.context_menu(|ui| {
                            for to in &targets {
                                if ui.button(format!("Move to {}", to.label())).clicked() {
                                    move_to = Some((i, *to));
                                    ui.close_menu();
                                }
                            }
                            if ui.button(tr("Move to a new Additional window")).clicked() {
                                new_window = Some(i);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button(tr("Close tab")).clicked() {
                                close = Some(i);
                                ui.close_menu();
                            }
                        });
                    }
                });
            });
        });
        let wz = &mut self.workzones;
        if let Some(t) = add {
            if let Some(p) = wz.pane_mut(pane) {
                p.add(t);
            }
        }
        if let Some(i) = change {
            if let Some(p) = wz.pane_mut(pane) {
                p.active = i;
            }
        }
        if let Some((i, to)) = move_to {
            wz.move_tab(pane, i, to);
        } else if let Some(i) = new_window {
            if let Some(t) = wz.pane_mut(pane).and_then(|p| p.remove(i)) {
                wz.new_aux(t);
            }
        } else if let Some(i) = close {
            if let Some(p) = wz.pane_mut(pane) {
                p.remove(i);
            }
        }
        ui.separator();
        self.workzones.pane(pane).and_then(|p| p.selected())
    }

    /// Whether the signed-in account may use a panel tab.
    pub(super) fn tab_allowed(&self, tab: Tab) -> bool {
        use crate::auth::tool;
        let p = &self.perm;
        match tab {
            Tab::Library => p.tool(tool::LIBRARY),
            Tab::Segment => p.tool(tool::NOTEPAD),
            Tab::FileExplorer => p.tool(tool::FILE_EXPLORER),
            Tab::SongRequests => p.tool(tool::SONG_REQUESTS),
            Tab::Carts => p.jingles,
            t if t.is_web() => p.tool(tool::WEB),
            _ => true,
        }
    }

    pub(super) fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane, core: &mut Playout) {
        let selected = self.pane_tabs(ui, pane);
        if let Some(tab) = selected.filter(|t| !self.tab_allowed(*t)) {
            ui.vertical_centered(|ui| {
                ui.add_space(28.0);
                ui.label(RichText::new(format!("🔒 {}", self.tab_name(tab))).strong().color(GRAY));
                ui.label(
                    RichText::new(tr("Your account isn't allowed to use this. An administrator can change that in Users and permissions."))
                        .small()
                        .color(GRAY),
                );
            });
            return;
        }
        match selected {
            Some(tab) if tab.is_web() => self.web_pane(ui, pane, tab),
            Some(Tab::AuxPlaylist) => self.tab_aux_playlist(ui, pane, core),
            Some(tab) => {
                // `both`: a wide tool scrolls sideways instead of pushing the panel wider
                egui::ScrollArea::both().id_salt(("pane", pane.key())).auto_shrink([false, false]).show(ui, |ui| match tab {
                    Tab::Library => self.tab_library(ui, core),
                    Tab::Carts => self.tab_carts(ui, core),
                    Tab::Clock => self.tab_clock(ui, core),
                    Tab::Segment => self.tab_segment(ui, core),
                    Tab::Queue => self.tab_queue(ui, core),
                    Tab::Search => self.tab_search(ui, core),
                    Tab::Statistics => self.tab_statistics(ui, core),
                    Tab::FileExplorer => self.tab_file_explorer(ui, pane, core),
                    Tab::Log => self.tab_log(ui, core),
                    Tab::SongRequests => self.tab_song_requests(ui, core),
                    Tab::TrackInfo => self.tab_track_info(ui, core),
                    _ => {}
                });
            }
            None => {
                ui.vertical_centered(|ui| {
                    ui.add_space(28.0);
                    ui.label(RichText::new(tr("No tabs in this panel")).color(GRAY));
                    ui.label(tr("Click ➕ to add a tab."));
                });
            }
        }
    }

    pub(super) fn right_workspace(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if self.workzones.panels < 2 {
            return;
        }
        let sw = ctx.screen_rect().width();
        let left = if self.workzones.show_left { self.left_w } else { 0.0 };
        let max_w = (sw - left - 380.0).clamp(260.0, (sw * 0.55).max(260.0));
        let three = self.workzones.panels >= 3;
        let r = egui::SidePanel::right("workspace")
            .resizable(true)
            .default_width((sw * 0.30).clamp(300.0, 520.0).min(max_w))
            .width_range(260.0..=max_w)
            .show(ctx, |ui| {
                if three {
                    let h = ui.available_height();
                    let split = self.workzones.split;
                    egui::TopBottomPanel::bottom("workspace_bottom")
                        .resizable(true)
                        .default_height((h * split).clamp(120.0, (h - 140.0).max(120.0)))
                        .height_range(110.0..=(h - 130.0).max(120.0))
                        .show_inside(ui, |ui| self.pane_ui(ui, Pane::Bottom, core));
                }
                egui::CentralPanel::default().show_inside(ui, |ui| self.pane_ui(ui, Pane::Top, core));
            });
        self.right_w = r.response.rect.width();
    }

    pub(super) fn left_workspace(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if !self.workzones.show_left {
            return;
        }
        let sw = ctx.screen_rect().width();
        let right = if self.workzones.panels >= 2 { self.right_w } else { 0.0 };
        let max_w = (sw - right - 380.0).clamp(190.0, (sw * 0.45).max(190.0));
        let r = egui::SidePanel::left("studio_left")
            .resizable(true)
            .default_width((sw * 0.22).clamp(240.0, 380.0).min(max_w))
            .width_range(190.0..=max_w)
            .show(ctx, |ui| self.pane_ui(ui, Pane::Left, core));
        self.left_w = r.response.rect.width();
    }

    /// Hide a window's web views while any egui popup, menu or window overlaps them.
    pub(super) fn guard_web_views(&mut self, ctx: &egui::Context, window: &str, force: bool) {
        let mut covered = force || ctx.memory(|m| m.any_popup_open());
        if !covered {
            'outer: for r in self.browsers.rects(window) {
                let r = r.shrink(1.0);
                for ix in 0..=10 {
                    for iy in 0..=10 {
                        let p = egui::pos2(r.min.x + r.width() * ix as f32 / 10.0, r.min.y + r.height() * iy as f32 / 10.0);
                        if matches!(ctx.layer_id_at(p), Some(l) if l.order != egui::Order::Background) {
                            covered = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if covered {
            self.browsers.block(window);
        }
    }

    /// Additional windows: each is a real OS window with its own tabbed panel.
    pub(super) fn aux_windows(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let wins: Vec<AuxWindow> = self.workzones.aux.clone();
        let mut closed = vec![];
        for w in wins {
            let title = aux_title(w.id);
            // The builder must stay identical frame to frame: egui turns any change into a move/resize
            // command, so the window is placed once from the saved layout and then left alone.
            let (size0, pos0) = *self.aux_spawn.entry(w.id).or_insert((w.size.unwrap_or([520.0, 600.0]), w.pos));
            let mut builder =
                ViewportBuilder::default().with_title(title.clone()).with_inner_size(size0).with_min_inner_size([260.0, 200.0]);
            if let Some(icon) = self.icon.clone() {
                builder = builder.with_icon(icon);
            }
            if let Some(p) = pos0 {
                builder = builder.with_position(p);
            }
            let id = egui::ViewportId::from_hash_of(("aux_window", w.id));
            let pane = Pane::Aux(w.id);
            ctx.show_viewport_immediate(id, builder, |ctx, class| {
                if class == egui::ViewportClass::Embedded {
                    // this platform can't open extra windows: show it inside the main window instead
                    let mut open = true;
                    egui::Window::new(title.clone())
                        .id(egui::Id::new(("aux_embedded", w.id)))
                        .open(&mut open)
                        .default_size([420.0, 480.0])
                        .show(ctx, |ui| {
                            self.pane_ui(ui, pane, core);
                        });
                    if !open {
                        closed.push(w.id);
                    }
                    return;
                }
                if ctx.input(|i| i.pointer.any_pressed()) && !ctx.input(|i| i.focused) {
                    self.browsers.focus_parent(&title);
                }
                egui::CentralPanel::default().show(ctx, |ui| self.pane_ui(ui, pane, core));
                self.guard_web_views(ctx, &title, false);
                let (close, outer, inner) =
                    ctx.input(|i| (i.viewport().close_requested(), i.viewport().outer_rect, i.viewport().inner_rect));
                // remember where the user put it (rounded, so sub-pixel jitter doesn't count as a change)
                if let Some(a) = self.workzones.aux.iter_mut().find(|a| a.id == w.id) {
                    if let Some(r) = outer {
                        let p = [r.min.x.round(), r.min.y.round()];
                        if a.pos != Some(p) {
                            a.pos = Some(p);
                        }
                    }
                    if let Some(r) = inner {
                        let sz = [r.width().round(), r.height().round()];
                        if a.size != Some(sz) {
                            a.size = Some(sz);
                        }
                    }
                }
                if close {
                    closed.push(w.id);
                }
            });
        }
        for id in closed {
            self.aux_spawn.remove(&id);
            self.browsers.close_window(&aux_title(id));
            self.workzones.close_aux(id);
        }
    }

    // ------------------------------------------------------------------ web
    fn web_pane(&mut self, ui: &mut egui::Ui, pane: Pane, tab: Tab) {
        let key = pane.key();
        let window = pane.window_key();
        let default = self.website_default(tab);
        let st = self.web_state.entry(key.clone()).or_default();
        if st.tab != Some(tab) {
            st.entry = default.clone();
            st.target = default.clone();
            st.tab = Some(tab);
        }
        let mut navigate = false;
        let mut open_ext = false;
        let mut delete_web: Option<usize> = None;
        ui.horizontal(|ui| {
            if ui.small_button("◀").on_hover_text(tr("Back")).clicked() {
                self.browsers.back(&key);
            }
            if ui.small_button("▶").on_hover_text(tr("Forward")).clicked() {
                self.browsers.forward(&key);
            }
            if ui.small_button("⟳").on_hover_text(tr("Reload")).clicked() {
                self.browsers.reload(&key);
            }
            if ui.small_button("🏠").on_hover_text(tr("Home")).clicked() {
                if let Some(st) = self.web_state.get_mut(&key) {
                    st.entry = default.clone();
                }
                navigate = true;
            }
            if ui.small_button(tr("Go")).clicked() {
                navigate = true;
            }
            if ui.small_button("↗").on_hover_text(tr("Open in your browser")).clicked() {
                open_ext = true;
            }
        });
        let st = self.web_state.get_mut(&key).unwrap();
        let response = ui.add(egui::TextEdit::singleline(&mut st.entry).desired_width(f32::INFINITY));
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            navigate = true;
        }
        if navigate {
            let entered = st.entry.trim().to_string();
            // only web addresses (no javascript:, file: or local paths)
            if (entered.starts_with("https://") || entered.starts_with("http://"))
                && entered.split("//").nth(1).is_some_and(|host| !host.is_empty() && !host.starts_with('/'))
            {
                st.target = entered.clone();
                if let Some(w) = tab.web_index().and_then(|i| self.workzones.web.get_mut(i)) {
                    w.url = entered;
                }
            }
        }
        let target = self.web_state.get(&key).map(|s| s.target.clone()).unwrap_or_default();
        if open_ext && !target.is_empty() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(target.clone()));
        }
        ui.horizontal(|ui| {
            if let Some(w) = tab.web_index().and_then(|i| self.workzones.web.get_mut(i)) {
                ui.label(RichText::new(tr("Name")).small().color(GRAY));
                ui.add(egui::TextEdit::singleline(&mut w.name).desired_width(110.0)).on_hover_text(tr("Name shown on this tab"));
            }
            // delete this web panel (asks once)
            if let Some(i) = tab.web_index().filter(|_| self.workzones.web.len() > 1) {
                if super::delete_button(ui, ("web", i), tr("this web panel")) {
                    delete_web = Some(i);
                }
            }
        });
        if let Some(i) = delete_web {
            self.workzones.remove_web(i);
            self.web_state.remove(&key);
            return;
        }
        if target.trim().is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("No web address set for this panel.")).color(GRAY));
                ui.label(RichText::new(tr("Type one above (https://…) and press Enter.")).small().color(GRAY));
            });
            return;
        }
        if let Some(err) = self.browsers.error(&key) {
            ui.colored_label(AMBER, format!("Web page can't be shown here: {err}"));
            if ui.button(tr("Open page in your browser ↗")).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(target));
            }
            return;
        }
        let size = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        // the native web view is positioned over this rectangle once the frame is laid out
        let ppp = ui.ctx().pixels_per_point();
        self.browsers.mark(&key, &window, rect.intersect(ui.clip_rect()), ppp, &target);
    }

    // ---------------------------------------------------------- track info
    /// Last / on air / next with their details, plus notes the presenter keeps about each song and artist.
    fn tab_track_info(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let cur = core.current;
        let st = core.status.clone();
        let last =
            cur.and_then(|c| core.playlist[..c.min(core.playlist.len())].iter().rev().find(|e| e.is_file())).map(|e| e.track.clone());
        let now = core.on_air.clone().filter(|_| st.has_track);
        let from = cur.map(|c| c + 1).unwrap_or(0);
        let next = core.playlist.iter().skip(from).find(|e| e.is_file()).map(|e| e.track.clone());
        let mut which = self.info_pick;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut which, 0, tr("◀ Last"));
            ui.selectable_value(&mut which, 1, RichText::new(tr("● On air")).strong());
            ui.selectable_value(&mut which, 2, tr("Next ▶"));
        });
        self.info_pick = which;
        ui.separator();
        let t = match which {
            0 => last,
            2 => next,
            _ => now,
        };
        let Some(t) = t else {
            ui.label(RichText::new(["Nothing has played yet.", "Nothing is on air.", "Nothing is coming up."][which.min(2)]).color(GRAY));
            return;
        };
        ui.horizontal(|ui| {
            if which == 1 {
                self.album_art(ui, Some(t.path.as_path()), 84.0);
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(if t.title.is_empty() { t.label() } else { t.title.clone() }).size(18.0).strong());
                if !t.artist.is_empty() {
                    ui.label(RichText::new(&t.artist).size(15.0));
                }
                let mut bits = vec![];
                if !t.album.is_empty() {
                    bits.push(t.album.clone());
                }
                if !t.year.is_empty() {
                    bits.push(t.year.clone());
                }
                if !t.genre.is_empty() {
                    bits.push(t.genre.clone());
                }
                bits.push(fmt(t.duration));
                ui.label(RichText::new(bits.join(" · ")).color(GRAY));
                if which == 1 && st.total > 0.0 {
                    ui.add(
                        egui::ProgressBar::new((st.pos / st.total).clamp(0.0, 1.0))
                            .desired_width(180.0)
                            .text(format!("{} left", fmt(st.total - st.pos))),
                    );
                }
            });
        });
        if !t.comment.trim().is_empty() {
            ui.label(RichText::new(format!("Tag comment: {}", t.comment.trim())).small().color(GRAY));
        }
        if let Some(n) = core.plays.get(&t.path).and_then(|&ts| chrono::DateTime::from_timestamp(ts, 0)) {
            ui.label(RichText::new(format!("Last played {}", n.with_timezone(&Local).format("%a %d %b %H:%M"))).small().color(GRAY));
        }
        ui.add_space(6.0);
        let song_key = t.path.to_string_lossy().to_string();
        let artist_key = t.artist.trim().to_lowercase();
        ui.strong(tr("About this song"));
        let w = ui.available_width().clamp(160.0, 640.0) - 8.0;
        let song = self.notes.songs.entry(song_key.clone()).or_default();
        if ui
            .add(
                egui::TextEdit::multiline(song).desired_rows(3).desired_width(w).hint_text(tr("Talking points, chart facts, dedications…")),
            )
            .changed()
        {
            self.notes_dirty = true;
        }
        if !artist_key.is_empty() {
            ui.strong(format!("About {}", t.artist.trim()));
            let a = self.notes.artists.entry(artist_key.clone()).or_default();
            if ui
                .add(egui::TextEdit::multiline(a).desired_rows(3).desired_width(w).hint_text(tr("Shared by every song by this artist")))
                .changed()
            {
                self.notes_dirty = true;
            }
        }
        ui.label(RichText::new(t.path.display().to_string()).small().color(GRAY));
        // don't keep empty notes around
        if self.notes.songs.get(&song_key).is_some_and(|v| v.is_empty()) {
            self.notes.songs.remove(&song_key);
        }
        if self.notes.artists.get(&artist_key).is_some_and(|v| v.is_empty()) {
            self.notes.artists.remove(&artist_key);
        }
    }

    // ------------------------------------------------------- file explorer
    fn tab_file_explorer(&mut self, ui: &mut egui::Ui, pane: Pane, core: &mut Playout) {
        let key = pane.key();
        let start = || {
            core.cfg
                .library_folders
                .first()
                .map(PathBuf::from)
                .or_else(dirs::audio_dir)
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("/"))
        };
        let dir = self.fx_dir.entry(key.clone()).or_insert_with(start).clone();
        let mut go: Option<PathBuf> = None;
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("⬆").on_hover_text(tr("Up one folder")).clicked() {
                if let Some(p) = dir.parent() {
                    go = Some(p.to_path_buf());
                }
            }
            for root in crate::library::drive_roots() {
                if ui.small_button(root.display().to_string().trim_end_matches('\\')).clicked() {
                    go = Some(root.clone());
                }
            }
            if ui.add_enabled(self.perm.edit_playlists, egui::Button::new(tr("Add folder to playlist")).small()).clicked() {
                self.add_files_later(self.view_pl, Some(usize::MAX), vec![dir.clone()]);
            }
        });
        ui.label(RichText::new(dir.display().to_string()).small().color(GRAY));
        ui.separator();
        // the folder is listed when it changes, on Refresh, or every few seconds — not every frame, and
        // off the UI thread (network and cloud drives can take seconds to answer)
        let stale = self.fx_list.get(&key).is_none_or(|(d, t, _)| *d != dir || t.elapsed() > Duration::from_secs(5));
        if stale && self.fx_loading.get(&key) != Some(&dir) {
            self.fx_loading.insert(key.clone(), dir.clone());
            let (k, d) = (key.clone(), dir.clone());
            self.dlg(move || {
                let mut list: Vec<(bool, PathBuf)> = std::fs::read_dir(&d)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| (e.file_type().map(|t| t.is_dir()).unwrap_or(false), e.path()))
                            .filter(|(d, p)| *d || crate::library::is_audio(p))
                            .collect()
                    })
                    .unwrap_or_default();
                list.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.file_name().cmp(&b.1.file_name())));
                Some(super::Dlg::FolderList(k, d, list))
            });
        }
        let listing = self.fx_list.get(&key).is_none_or(|(d, _, _)| *d != dir);
        if listing {
            ui.spinner();
        }
        let entries = self.fx_list.get(&key).filter(|(d, _, _)| *d == dir).map(|(_, _, l)| l.clone()).unwrap_or_default();
        let mut add: Vec<PathBuf> = vec![];
        // only the rows in view are drawn; the others just take their space (big folders stay smooth)
        let row_h = (ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y)
            .max(ui.spacing().interact_size.y)
            + ui.spacing().item_spacing.y;
        let mut skipped = 0.0;
        for (is_dir, p) in entries.iter().take(2000) {
            let row = egui::Rect::from_min_size(ui.cursor().min + egui::vec2(0.0, skipped), egui::vec2(1.0, row_h));
            if !ui.is_rect_visible(row) {
                skipped += row_h;
                continue;
            }
            if skipped > 0.0 {
                ui.add_space(skipped);
                skipped = 0.0;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let r =
                ui.selectable_label(false, if *is_dir { format!("📁 {name}") } else { format!("🎵 {name}") }).interact(egui::Sense::drag());
            if r.drag_started() && self.perm.edit_playlists {
                // a folder is listed when it is dropped, off the UI thread
                r.dnd_set_drag_payload(super::plview::DragFiles(vec![p.clone()]));
            }
            if r.double_clicked() {
                if *is_dir {
                    go = Some(p.clone());
                } else {
                    add.push(p.clone());
                }
            }
            r.context_menu(|ui| {
                if !self.perm.edit_playlists {
                    ui.label(RichText::new(tr("Your account can't change playlists")).small().color(GRAY));
                    return;
                }
                if ui.button(tr("Add to playlist")).clicked() {
                    add.push(p.clone());
                    ui.close_menu();
                }
                if !*is_dir && ui.button(tr("Insert after selected row")).clicked() {
                    let at = self.insert_pos(core);
                    let rows: Vec<PlEntry> = vec![core.entry_quick(p)];
                    core.with_tab(self.view_pl, |c| c.insert_rows(at, rows));
                    ui.close_menu();
                }
            });
        }
        if skipped > 0.0 {
            ui.add_space(skipped);
        }
        if entries.is_empty() && !listing {
            ui.label(RichText::new(tr("No folders or audio files here.")).color(GRAY));
        }
        if !add.is_empty() && self.perm.edit_playlists {
            self.add_files_later(self.view_pl, Some(usize::MAX), add);
        }
        if let Some(p) = go {
            self.fx_dir.insert(key, p);
        }
        ui.small(tr("Double-click a folder to open it, or a file to add it to the playlist you're viewing."));
    }

    // ------------------------------------------------------------------ log
    fn tab_log(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.strong(tr("LOG"));
            if ui.small_button(tr("Clear")).clicked() {
                core.log.clear();
            }
        });
        egui::Grid::new("log_grid").striped(true).num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
            for l in core.log.iter().rev().take(400) {
                let c = if l.error { RED } else { ui.visuals().text_color() };
                ui.label(RichText::new(&l.time).monospace().color(GRAY));
                ui.label(RichText::new(&l.text).color(c));
                ui.end_row();
            }
        });
        if core.log.is_empty() {
            ui.label(RichText::new(tr("Nothing logged yet.")).color(GRAY));
        }
    }

    // ------------------------------------------------------- song requests
    fn tab_song_requests(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        self.az_requests_panel(ui, core);
    }

    // --------------------------------------------------------- AUX playlist
    /// Another view of any playlist (handy on a second screen). Double-click plays a row.
    fn tab_aux_playlist(&mut self, ui: &mut egui::Ui, pane: Pane, core: &mut Playout) {
        let key = pane.key();
        let n_tabs = core.tabs.len();
        let mut which = *self.aux_pl.entry(key.clone()).or_insert(core.playing_tab);
        if which >= n_tabs {
            which = core.playing_tab;
        }
        ui.horizontal_wrapped(|ui| {
            for i in 0..n_tabs {
                let live = i == core.playing_tab;
                let name = if live { format!("▶ {}", core.tabs[i].name) } else { core.tabs[i].name.clone() };
                if ui.selectable_label(which == i, name).clicked() {
                    which = i;
                }
            }
        });
        self.aux_pl.insert(key.clone(), which);
        let on_air = which == core.playing_tab;
        let (starts, cur): (Vec<Option<crate::playout::Start>>, Option<usize>) =
            core.with_tab(which, |c| (c.projected_starts(on_air), c.current));
        let state = core.state;
        let rows = core.tab_rows(which);
        let sel = self.aux_sel.entry(key.clone()).or_default();
        let mut play: Option<usize> = None;
        let mut remove = false;
        let playing = state != PlayState::Stopped;
        let dark = ui_dark(&self.workzones);
        TableBuilder::new(ui)
            .id_salt(("aux_pl", key.clone()))
            .striped(true)
            .sense(egui::Sense::click())
            .column(Column::exact(34.0))
            .column(Column::remainder().at_least(80.0).clip(true))
            .column(Column::exact(52.0))
            .column(Column::exact(70.0))
            .header(20.0, |mut h| {
                for t in ["#", "Title", "Length", "Start"] {
                    h.col(|ui| {
                        ui.strong(tr(t));
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, rows.len(), |mut row| {
                    let i = row.index();
                    let e = &rows[i];
                    let is_cur = on_air && playing && cur == Some(i);
                    row.set_selected(sel.contains(&i));
                    let st = starts.get(i).copied().flatten();
                    let col = row_text_color(e, is_cur, st.is_some_and(|s| s.skipped), dark);
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        ui.label(RichText::new(if is_cur { "▶".to_string() } else { (i + 1).to_string() }).color(col));
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        ui.add(egui::Label::new(RichText::new(e.title()).color(col)).truncate());
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        ui.label(RichText::new(e.length_text()).color(col));
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        let t = match st {
                            Some(s) if s.skipped => "skip".to_string(),
                            Some(s) => s.at.format("%H:%M:%S").to_string(),
                            None => String::new(),
                        };
                        ui.label(RichText::new(t).color(col));
                    });
                    let r = row.response();
                    if r.clicked() {
                        sel.clear();
                        sel.insert(i);
                    }
                    if r.double_clicked() {
                        play = Some(i);
                    }
                    r.context_menu(|ui| {
                        if ui.button(tr("▶ Play now")).clicked() {
                            play = Some(i);
                            ui.close_menu();
                        }
                        if ui.button(tr("🗑 Remove")).clicked() {
                            sel.clear();
                            sel.insert(i);
                            remove = true;
                            ui.close_menu();
                        }
                    });
                });
            });
        if let Some(i) = play {
            let xf = if core.state == PlayState::Playing { core.cfg.crossfade_secs } else { 0.0 };
            core.with_tab(which, |c| c.play_index(i, xf));
        }
        if remove {
            let s = std::mem::take(sel);
            core.with_tab(which, |c| c.remove(&s));
        }
    }
}

pub(super) fn ui_dark(w: &WorkzoneSet) -> bool {
    w.dark
}

/// Text colour of a playlist row.
pub(super) fn row_text_color(e: &PlEntry, is_cur: bool, skipped: bool, dark: bool) -> Color32 {
    if matches!(e.kind, ItemKind::HourInfo { .. }) {
        return if dark { Color32::from_rgb(255, 150, 200) } else { Color32::from_rgb(170, 30, 110) };
    }
    let normal = if dark { Color32::from_gray(225) } else { Color32::from_rgb(28, 34, 46) };
    if is_cur {
        return if dark { Color32::from_rgb(110, 230, 130) } else { Color32::from_rgb(15, 120, 40) };
    }
    if skipped || e.played {
        return Color32::from_gray(140);
    }
    if e.is_file() && crate::library::file_missing(&e.track.path) {
        return Color32::from_rgb(200, 50, 50);
    }
    match e.kind {
        ItemKind::HourMarker { .. } => {
            if dark {
                Color32::from_rgb(255, 170, 170)
            } else {
                Color32::from_rgb(150, 20, 30)
            }
        }
        ItemKind::Comment(_) => {
            if dark {
                Color32::from_rgb(230, 210, 140)
            } else {
                Color32::from_rgb(120, 90, 0)
            }
        }
        _ => normal,
    }
}

/// RadioBOSS-style row tints: pink hour markers, yellow comments, blue for other special items.
pub(super) fn row_background(ui: &mut egui::Ui, e: &PlEntry, is_cur: bool) {
    let dark = ui.visuals().dark_mode;
    let c = if is_cur {
        Some(if dark { Color32::from_rgba_unmultiplied(60, 160, 80, 60) } else { Color32::from_rgb(214, 245, 214) })
    } else {
        match e.kind {
            ItemKind::HourMarker { .. } => {
                Some(if dark { Color32::from_rgba_unmultiplied(200, 60, 80, 70) } else { Color32::from_rgb(255, 214, 214) })
            }
            ItemKind::Comment(_) => {
                Some(if dark { Color32::from_rgba_unmultiplied(200, 170, 40, 50) } else { Color32::from_rgb(255, 248, 205) })
            }
            ItemKind::HourInfo { .. } => {
                Some(if dark { Color32::from_rgba_unmultiplied(220, 80, 160, 40) } else { Color32::from_rgb(252, 232, 244) })
            }
            ItemKind::Track | ItemKind::VoiceTrack => None,
            _ => Some(if dark { Color32::from_rgba_unmultiplied(70, 120, 200, 45) } else { Color32::from_rgb(226, 238, 252) }),
        }
    };
    if let Some(c) = c {
        ui.painter().rect_filled(ui.max_rect().expand2(egui::vec2(4.0, 1.0)), 0.0, c);
    }
}
