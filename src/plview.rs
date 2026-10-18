//! The playlist view: RadioBOSS-style playlist tabs (one is on air), the table of rows with
//! start times, and row actions.

use super::panes::{row_background, row_text_color};
use super::*;
use crate::items::{ItemKind, PlEntry};

/// Playlist rows being dragged (from playlist `tab`).
#[derive(Clone)]
pub(super) struct DragRows {
    pub tab: usize,
    pub rows: BTreeSet<usize>,
}

/// Audio files being dragged from the library or the file explorer.
#[derive(Clone)]
pub(super) struct DragFiles(pub Vec<PathBuf>);

/// Draw "what's being dragged" next to the pointer.
/// The payload of type `T` dropped on `r` this frame. (egui's own `dnd_release_payload` takes
/// whatever is being dragged, even of another type, so checking for one type would lose the other.)
pub(super) fn released<T: std::any::Any + Send + Sync>(r: &egui::Response) -> Option<std::sync::Arc<T>> {
    if egui::DragAndDrop::has_payload_of_type::<T>(&r.ctx) {
        r.dnd_release_payload::<T>()
    } else {
        None
    }
}

/// Take a dropped payload of type `T` (and leave other types alone).
pub(super) fn take<T: std::any::Any + Send + Sync>(ctx: &egui::Context) -> Option<std::sync::Arc<T>> {
    if egui::DragAndDrop::has_payload_of_type::<T>(ctx) {
        egui::DragAndDrop::take_payload::<T>(ctx)
    } else {
        None
    }
}

pub(super) fn drag_label(ctx: &egui::Context, text: &str) {
    if let Some(p) = ctx.pointer_interact_pos() {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag_label")));
        let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), Color32::WHITE);
        let r = egui::Rect::from_min_size(p + egui::vec2(14.0, 8.0), galley.size() + egui::vec2(12.0, 6.0));
        painter.rect_filled(r, 4.0, Color32::from_rgb(20, 120, 210));
        painter.galley(r.min + egui::vec2(6.0, 3.0), galley, Color32::WHITE);
    }
}

/// Changes to the playlist tab strip, applied after the view has been drawn.
pub(super) enum TabOp {
    View(usize),
    New,
    Rename(usize, String),
    Duplicate(usize),
    Close(usize),
    Clear(usize),
    MoveTab(usize, usize),
    PlayTab(usize),
    SaveM3u(usize),
    LoadM3u(usize),
    Rows(usize, Vec<PlEntry>),
    /// rows dragged onto another playlist's tab: (from tab, rows, to tab)
    MoveRows(usize, BTreeSet<usize>, usize),
    /// rows of one playlist copied into another at a position: (from, rows, to, at)
    CopyInto(usize, BTreeSet<usize>, usize, usize),
}

/// Small type icon for the "#" column.
pub(super) fn kind_icon(e: &PlEntry) -> &'static str {
    match e.kind {
        ItemKind::Track => "",
        ItemKind::VoiceTrack => "🎤",
        ItemKind::Url { .. } => "🌐",
        ItemKind::LineIn { .. } => "🔌",
        ItemKind::Command(_) => "⚙",
        ItemKind::Pause(_) => "⏸",
        ItemKind::Teaser { .. } => "⏩",
        ItemKind::TimeAnnounce => "🕐",
        ItemKind::Comment(_) => "💬",
        ItemKind::Container { .. } => "📦",
        ItemKind::TrackList { .. } => "🔀",
        ItemKind::HourMarker { .. } => "⏰",
        ItemKind::HourInfo { .. } => "🕒",
        ItemKind::Tts { .. } => "🔊",
    }
}

impl App {
    /// Where "Add …" / "Insert …" puts new rows: after the selection, or at the end.
    pub(super) fn insert_pos(&self, core: &mut Playout) -> usize {
        let len = core.tab_len(self.view_pl.min(core.tabs.len().saturating_sub(1)));
        match self.sel.iter().next_back() {
            Some(&i) => (i + 1).min(len),
            None => len,
        }
    }

    /// Insert rows into the playlist being viewed (after the selection, or at the end) and select them.
    pub(super) fn insert_rows_here(&mut self, core: &mut Playout, rows: Vec<PlEntry>) {
        if rows.is_empty() {
            return;
        }
        if !self.perm.edit_playlists {
            *self.notify.lock() = tr("Your account isn't allowed to edit playlists").into();
            return;
        }
        let at = self.insert_pos(core);
        let n = rows.len();
        core.with_tab(self.view_pl, |c| c.insert_rows(at, rows));
        self.sel = (at..at + n).collect();
        self.anchor = Some(at);
        self.scroll_to = Some(at);
    }

    pub(super) fn playlist(&mut self, ctx: &egui::Context, core: &mut Playout) {
        if self.view_pl >= core.tabs.len() {
            self.view_pl = core.playing_tab; // first frame, or the viewed tab was closed
        }
        let strip: Vec<(String, usize, f32)> =
            (0..core.tabs.len()).map(|i| (core.tabs[i].name.clone(), core.tab_len(i), core.tab_secs(i))).collect();
        let v = self.view_pl;
        let on_air_tab = core.playing_tab;
        core.with_tab(v, |core| self.playlist_view(ctx, core, &strip, on_air_tab));
        self.apply_tab_ops(core);
    }

    pub(super) fn apply_tab_ops(&mut self, core: &mut Playout) {
        for op in std::mem::take(&mut self.tab_ops) {
            match op {
                TabOp::View(i) => {
                    if i != self.view_pl {
                        self.view_pl = i;
                        self.sel.clear();
                        self.anchor = None;
                    }
                }
                TabOp::New => {
                    self.view_pl = core.new_tab(None);
                    self.sel.clear();
                }
                TabOp::Rename(i, n) => {
                    if let Some(t) = core.tabs.get_mut(i) {
                        if !n.trim().is_empty() {
                            t.name = n.trim().to_string();
                        }
                    }
                }
                TabOp::Duplicate(i) => {
                    self.view_pl = core.duplicate_tab(i);
                    self.sel.clear();
                }
                TabOp::Close(i) => {
                    if core.close_tab(i) {
                        if self.view_pl >= i && self.view_pl > 0 {
                            self.view_pl -= 1;
                        }
                        self.sel.clear();
                    } else {
                        core.say("The playlist that is on air can't be closed");
                    }
                }
                TabOp::Clear(i) => {
                    core.with_tab(i, |c| c.clear_playlist());
                    self.sel.clear();
                }
                TabOp::MoveTab(i, to) => {
                    core.move_tab(i, to);
                    if self.view_pl == i {
                        self.view_pl = to;
                    } else if self.view_pl == to {
                        self.view_pl = i;
                    }
                }
                TabOp::PlayTab(i) => core.put_tab_on_air(i),
                TabOp::SaveM3u(i) => {
                    self.view_pl = i;
                    let name = format!("{}.m3u", core.tabs[i].name);
                    self.dlg(move || {
                        rfd::FileDialog::new().add_filter("Playlist", &["m3u"]).set_file_name(&name).save_file().map(Dlg::SaveM3u)
                    });
                }
                TabOp::LoadM3u(i) => {
                    self.view_pl = i;
                    self.dlg(|| rfd::FileDialog::new().add_filter("Playlist", &["m3u", "m3u8"]).pick_file().map(Dlg::LoadM3u));
                }
                TabOp::Rows(i, rows) => core.append_to_tab(i, rows),
                TabOp::CopyInto(from, rows, to, at) => {
                    let taken: Vec<PlEntry> = rows.iter().filter_map(|&i| core.tab_rows(from).get(i)).map(|e| e.fresh()).collect();
                    let n = taken.len();
                    core.with_tab(to, |c| c.insert_rows(at, taken));
                    if to == self.view_pl {
                        self.sel = (at..at + n).collect();
                    }
                }
                TabOp::MoveRows(from, rows, to) => {
                    if from != to && to < core.tabs.len() {
                        let taken: Vec<PlEntry> = core.with_tab(from, |c| {
                            let v = rows.iter().filter_map(|&i| c.playlist.get(i)).map(|e| e.fresh()).collect();
                            c.remove(&rows);
                            v
                        });
                        let n = taken.len();
                        core.append_to_tab(to, taken);
                        core.say(format!("{n} item(s) moved to {}", core.tabs[to].name));
                        if from == self.view_pl {
                            self.sel.clear();
                        }
                    }
                }
            }
        }
    }

    /// RadioBOSS-style playlist tabs: ▶ marks the one on air.
    fn playlist_tab_strip(&mut self, ui: &mut egui::Ui, strip: &[(String, usize, f32)], on_air_tab: usize) {
        let n = strip.len();
        ui.horizontal_wrapped(|ui| {
            for (i, (name, len, secs)) in strip.iter().enumerate() {
                if let Some((ri, buf)) = &mut self.renaming {
                    if *ri == i {
                        let r = ui.add(egui::TextEdit::singleline(buf).desired_width(110.0));
                        r.request_focus();
                        if ui.input(|k| k.key_pressed(egui::Key::Escape)) {
                            self.renaming = None;
                        } else if r.lost_focus() || ui.input(|k| k.key_pressed(egui::Key::Enter)) {
                            let new = buf.clone();
                            self.tab_ops.push(TabOp::Rename(i, new));
                            self.renaming = None;
                        }
                        continue;
                    }
                }
                let live = i == on_air_tab;
                let mut text = RichText::new(if live { format!("▶ {name}") } else { name.clone() });
                if live {
                    text = text.strong().color(Color32::from_rgb(20, 140, 50));
                }
                let r = ui.selectable_label(self.view_pl == i, text).on_hover_text(format!(
                    "{len} items · {}{}\nDouble-click to rename, right-click for more",
                    fmt(*secs),
                    if live { " · ON AIR" } else { "" }
                ));
                if r.clicked() {
                    self.tab_ops.push(TabOp::View(i));
                }
                if let Some(d) = r.dnd_hover_payload::<DragRows>() {
                    if d.tab != i {
                        ui.painter().rect_stroke(r.rect.expand(2.0), 4.0, egui::Stroke::new(2.0_f32, ACCENT), egui::StrokeKind::Outside);
                    }
                }
                if let Some(d) = released::<DragRows>(&r) {
                    if d.tab != i {
                        self.tab_ops.push(TabOp::MoveRows(d.tab, d.rows.clone(), i));
                    }
                }
                if let Some(f) = released::<DragFiles>(&r) {
                    let rows: Vec<PlEntry> = f.0.iter().map(|p| PlEntry::file(crate::library::Track::from_path(p))).collect();
                    self.tab_ops.push(TabOp::Rows(i, rows));
                }
                if !self.perm.edit_playlists {
                    continue;
                }
                if r.double_clicked() {
                    self.renaming = Some((i, name.clone()));
                }
                r.context_menu(|ui| {
                    if ui.add_enabled(!live, egui::Button::new(tr("▶ Put this playlist on air"))).clicked() {
                        self.tab_ops.push(TabOp::PlayTab(i));
                        self.tab_ops.push(TabOp::View(i));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(tr("✏ Rename…")).clicked() {
                        self.renaming = Some((i, name.clone()));
                        ui.close_menu();
                    }
                    if ui.button(tr("Duplicate")).clicked() {
                        self.tab_ops.push(TabOp::Duplicate(i));
                        ui.close_menu();
                    }
                    if ui.button(tr("💾 Save as M3U…")).clicked() {
                        self.tab_ops.push(TabOp::SaveM3u(i));
                        ui.close_menu();
                    }
                    if ui.button(tr("📂 Load M3U into this tab…")).clicked() {
                        self.tab_ops.push(TabOp::LoadM3u(i));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(i > 0, egui::Button::new(tr("◀ Move left"))).clicked() {
                        self.tab_ops.push(TabOp::MoveTab(i, i - 1));
                        ui.close_menu();
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new(tr("Move right ▶"))).clicked() {
                        self.tab_ops.push(TabOp::MoveTab(i, i + 1));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(!live, egui::Button::new(tr("Clear"))).clicked() {
                        self.tab_ops.push(TabOp::Clear(i));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(!live && n > 1, egui::Button::new(tr("✖ Close")))
                        .on_disabled_hover_text(tr("The playlist that is on air can't be closed"))
                        .clicked()
                    {
                        self.tab_ops.push(TabOp::Close(i));
                        ui.close_menu();
                    }
                });
            }
            if self.perm.edit_playlists && ui.small_button("➕").on_hover_text(tr("New playlist")).clicked() {
                self.tab_ops.push(TabOp::New);
            }
        });
    }

    fn playlist_view(&mut self, ctx: &egui::Context, core: &mut Playout, strip: &[(String, usize, f32)], on_air_tab: usize) {
        let viewing_on_air = self.view_pl == on_air_tab;
        if !ctx.wants_keyboard_input() {
            if self.perm.edit_playlists && ctx.input(|i| i.key_pressed(egui::Key::Delete)) && !self.sel.is_empty() {
                let s = std::mem::take(&mut self.sel);
                core.remove(&s);
            }
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
                self.sel = (0..core.playlist.len()).collect();
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            self.playlist_tab_strip(ui, strip, on_air_tab);
            ui.horizontal_wrapped(|ui| {
                if !viewing_on_air {
                    ui.label(RichText::new(tr("not on air")).small().color(Color32::from_rgb(190, 120, 0)))
                        .on_hover_text(tr("Edit this playlist freely. Play any row (or right-click the tab) to put it on air."));
                }
                ui.label(RichText::new(format!("{} items · {}", core.playlist.len(), fmt(core.total_secs()))).color(GRAY));
                ui.checkbox(&mut core.cfg.loop_playlist, tr("Loop"));
                ui.checkbox(&mut core.cfg.auto_enabled, tr("Auto-playlist"));
                ui.checkbox(&mut core.cfg.random_fill, tr("Auto-fill from library"));
                ui.checkbox(&mut self.workzones.follow, tr("Follow"))
                    .on_hover_text(tr("Keep the playing track near the top as songs finish (the last one played stays in view)"));
                if ui
                    .checkbox(&mut core.cfg.hour_lines, tr("Hour lines"))
                    .on_hover_text(tr("Show a line where each new hour starts"))
                    .changed()
                {
                    self.hour_check = None;
                }
            });

            let starts = core.projected_starts(viewing_on_air);
            let mods = ui.input(|i| i.modifiers);
            let view_tab = self.view_pl;
            let avail_w = ui.available_width();
            let wide = avail_w > 700.0;
            let very_wide = avail_w > 900.0;
            let mut acts: Vec<PlAct> = vec![];
            let cur = core.current;
            let playing = core.state != PlayState::Stopped;
            let dark = self.workzones.dark;
            let mut scroll_to = self.scroll_to.take().map(|r| (r, egui::Align::Center));
            // follow the on-air track: when it changes, put the last played row at the top
            let key = (view_tab, cur);
            if viewing_on_air && self.workzones.follow && self.followed != Some(key) && cur.is_some() && scroll_to.is_none() {
                scroll_to = cur.map(|c| (c.saturating_sub(1), egui::Align::TOP));
            }
            self.followed = Some(key);
            let may_edit = self.perm.edit_playlists;
            let may_play = self.perm.playback;
            let dragging_rows = egui::DragAndDrop::payload::<DragRows>(ctx);
            let dragging_files = egui::DragAndDrop::payload::<DragFiles>(ctx);
            let mut drop_line: Option<(f32, egui::Rangef)> = None;
            let mut drop_at: Option<usize> = None;
            let mut drop_files: Option<(usize, Vec<PathBuf>)> = None;
            let mut drop_rows: Option<(DragRows, usize)> = None;
            let mut drag_start: Option<usize> = None;
            let n_rows = core.playlist.len();
            let plays = &core.plays;
            let sel = &mut self.sel;
            let anchor = &mut self.anchor;
            let pl = &core.playlist;

            let mut tb = TableBuilder::new(ui)
                .id_salt(("playlist_table", (avail_w / 24.0) as i32, wide, very_wide))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click_and_drag())
                .column(Column::exact(52.0))
                .column(Column::remainder().at_least(120.0).clip(true))
                .column(Column::exact(64.0))
                .column(Column::exact(82.0));
            if wide {
                tb = tb.column(Column::exact(140.0));
            }
            if very_wide {
                tb = tb.column(Column::exact(90.0));
            }
            if let Some((r, align)) = scroll_to {
                tb = tb.scroll_to_row(r, Some(align));
            }
            tb.header(24.0, |mut h| {
                let mut heads = vec!["#", "Track Title", "Duration", "Start time"];
                if wide {
                    heads.push("Last played");
                }
                if very_wide {
                    heads.push("Type");
                }
                for t in heads {
                    h.col(|ui| {
                        ui.strong(tr(t));
                    });
                }
            })
            .body(|body| {
                body.rows(24.0, pl.len(), |mut row| {
                    let i = row.index();
                    let e = &pl[i];
                    let is_cur = cur == Some(i) && playing && viewing_on_air;
                    let st = starts.get(i).copied().flatten();
                    let skipped = st.is_some_and(|s| s.skipped);
                    let col = row_text_color(e, is_cur, skipped, dark);
                    row.set_selected(sel.contains(&i));
                    let txt = |s: String| {
                        let mut r = RichText::new(s).color(col);
                        if is_cur || matches!(e.kind, ItemKind::HourMarker { .. }) {
                            r = r.strong();
                        }
                        if matches!(e.kind, ItemKind::Comment(_)) {
                            r = r.italics();
                        }
                        if skipped {
                            r = r.strikethrough();
                        }
                        r
                    };
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        let n = if is_cur { "▶".to_string() } else { (i + 1).to_string() };
                        ui.label(txt(format!("{} {n}", kind_icon(e)).trim().to_string()));
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        ui.add(egui::Label::new(txt(e.title())).truncate());
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        ui.label(txt(e.length_text()));
                    });
                    row.col(|ui| {
                        row_background(ui, e, is_cur);
                        let t = match st {
                            Some(s) if skipped => format!("skip {}", s.at.format("%H:%M")),
                            Some(s) => s.at.format("%H:%M:%S").to_string(),
                            None => String::new(),
                        };
                        ui.label(txt(t));
                    });
                    if wide {
                        row.col(|ui| {
                            row_background(ui, e, is_cur);
                            let lp = if e.is_file() {
                                plays
                                    .get(&e.track.path)
                                    .and_then(|&ts| chrono::DateTime::from_timestamp(ts, 0))
                                    .map(|d| d.with_timezone(&Local).format("%d/%m/%Y %H:%M").to_string())
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            };
                            ui.label(txt(lp));
                        });
                    }
                    if very_wide {
                        row.col(|ui| {
                            row_background(ui, e, is_cur);
                            ui.label(RichText::new(e.type_label()).small().color(GRAY));
                        });
                    }
                    let resp = row.response();
                    // drag rows to reorder them (or onto another playlist's tab)
                    if resp.drag_started() && may_edit {
                        drag_start = Some(i);
                        if !sel.contains(&i) {
                            sel.clear();
                            sel.insert(i);
                            *anchor = Some(i);
                        }
                        resp.dnd_set_drag_payload(DragRows { tab: view_tab, rows: sel.clone() });
                    }
                    if may_edit && (dragging_rows.is_some() || dragging_files.is_some()) && resp.contains_pointer() {
                        let below = resp.hover_pos().or(resp.ctx.pointer_hover_pos()).is_some_and(|p| p.y > resp.rect.center().y);
                        let at = if below { i + 1 } else { i };
                        drop_at = Some(at);
                        // on the boundary between the two rows (rows have a small gap between them)
                        let gap = resp.ctx.style().spacing.item_spacing.y * 0.5;
                        let y = if below { resp.rect.bottom() + gap } else { resp.rect.top() - gap };
                        drop_line = Some((y, resp.rect.x_range()));
                        if let Some(d) = released::<DragRows>(&resp) {
                            drop_rows = Some(((*d).clone(), at));
                        }
                        if let Some(f) = released::<DragFiles>(&resp) {
                            drop_files = Some((at, f.0.clone()));
                        }
                    }
                    if resp.clicked() {
                        if mods.shift && anchor.is_some() {
                            let a = anchor.unwrap();
                            sel.clear();
                            sel.extend(a.min(i)..=a.max(i));
                        } else if mods.command {
                            if !sel.remove(&i) {
                                sel.insert(i);
                            }
                            *anchor = Some(i);
                        } else {
                            sel.clear();
                            sel.insert(i);
                            *anchor = Some(i);
                        }
                    }
                    if resp.secondary_clicked() && !sel.contains(&i) {
                        sel.clear();
                        sel.insert(i);
                    }
                    if resp.double_clicked() && may_play {
                        acts.push(PlAct::Play(i));
                    }
                    resp.context_menu(|ui| {
                        if !may_edit {
                            if may_play && ui.button(tr("▶ Play now")).clicked() {
                                acts.push(PlAct::Play(i));
                                ui.close_menu();
                            }
                            ui.label(RichText::new(tr("Your account can't change playlists")).small().color(GRAY));
                            return;
                        }
                        if may_play && ui.button(tr("▶ Play now")).clicked() {
                            acts.push(PlAct::Play(i));
                            ui.close_menu();
                        }
                        if e.is_file() {
                            if ui.button(tr("✏ Track editor (tags, cues, fades)…")).clicked() {
                                acts.push(PlAct::Fade(i));
                                ui.close_menu();
                            }
                        } else if !matches!(e.kind, ItemKind::TimeAnnounce) && ui.button(tr("✏ Edit…")).clicked() {
                            acts.push(PlAct::Edit(i));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(tr("⬆ Move up")).clicked() {
                            acts.push(PlAct::Up);
                            ui.close_menu();
                        }
                        if ui.button(tr("⬇ Move down")).clicked() {
                            acts.push(PlAct::Down);
                            ui.close_menu();
                        }
                        if strip.len() > 1 {
                            ui.menu_button(tr("Copy to playlist"), |ui| {
                                for (t, (name, _, _)) in strip.iter().enumerate() {
                                    if t != view_tab && ui.button(name).clicked() {
                                        acts.push(PlAct::CopyTo(t));
                                        ui.close_menu();
                                    }
                                }
                            });
                            ui.menu_button(tr("Move to playlist"), |ui| {
                                for (t, (name, _, _)) in strip.iter().enumerate() {
                                    if t != view_tab && ui.button(name).clicked() {
                                        acts.push(PlAct::MoveTo(t));
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                        if e.is_file() && ui.button(tr("☁ Upload to AzuraCast")).clicked() {
                            acts.push(PlAct::Upload);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(tr("🗑 Remove")).clicked() {
                            acts.push(PlAct::Remove);
                            ui.close_menu();
                        }
                    });
                });
            });

            if let Some((y, xr)) = drop_line {
                let p = ui.painter();
                p.hline(xr, y, egui::Stroke::new(2.5_f32, ACCENT));
                // little end caps make the drop point easy to see
                for x in [xr.min, xr.max] {
                    p.circle_filled(egui::pos2(x + if x == xr.min { 4.0 } else { -4.0 }, y), 3.5, ACCENT);
                }
            }
            if let Some(d) = &dragging_rows {
                let _ = drag_start;
                drag_label(ctx, &format!("{} row(s)", d.rows.len()));
            } else if let Some(f) = &dragging_files {
                drag_label(ctx, &format!("+ {} file(s)", f.0.len()));
            }
            // released below the last row: drop at the end
            if may_edit && drop_at.is_none() && ui.input(|i| i.pointer.any_released()) && ui.rect_contains_pointer(ui.max_rect()) {
                if let Some(d) = take::<DragRows>(ctx) {
                    drop_rows = Some(((*d).clone(), n_rows));
                } else if let Some(f) = take::<DragFiles>(ctx) {
                    drop_files = Some((n_rows, f.0.clone()));
                }
            }
            if let Some((d, at)) = drop_rows {
                if d.tab == view_tab {
                    self.sel = core.move_rows(&d.rows, at);
                } else {
                    // rows from another playlist (e.g. an Additional window's AUX playlist): copied in
                    self.tab_ops.push(TabOp::CopyInto(d.tab, d.rows.clone(), view_tab, at));
                }
            }
            if let Some((at, files)) = drop_files {
                let rows: Vec<PlEntry> = files.iter().map(|p| core.entry_for(p)).collect();
                let n = rows.len();
                core.insert_rows(at, rows);
                self.sel = (at..at + n).collect();
            }

            for a in acts {
                match a {
                    PlAct::Play(i) => {
                        let xf = core.cfg.crossfade_secs;
                        core.play_index(i, if core.state == PlayState::Playing { xf } else { 0.0 });
                    }
                    PlAct::Remove => {
                        let s = std::mem::take(&mut self.sel);
                        core.remove(&s);
                    }
                    PlAct::Up => self.sel = core.move_sel(&self.sel.clone(), true),
                    PlAct::Down => self.sel = core.move_sel(&self.sel.clone(), false),
                    PlAct::Fade(i) => self.open_fade_editor(core, Some(i)),
                    PlAct::Edit(i) => {
                        if let Some(e) = core.playlist.get(i).cloned() {
                            self.edit_item(i, &e);
                        }
                    }
                    PlAct::Upload => {
                        let paths = self
                            .sel
                            .iter()
                            .filter_map(|&i| core.playlist.get(i))
                            .filter(|e| e.is_file())
                            .map(|e| e.track.path.clone())
                            .collect();
                        Self::upload(core, paths);
                    }
                    PlAct::CopyTo(t) | PlAct::MoveTo(t) => {
                        let rows: Vec<PlEntry> = self.sel.iter().filter_map(|&i| core.playlist.get(i)).map(|e| e.fresh()).collect();
                        let n = rows.len();
                        let moving = matches!(a, PlAct::MoveTo(_));
                        if moving {
                            let s = std::mem::take(&mut self.sel);
                            core.remove(&s);
                        }
                        self.tab_ops.push(TabOp::Rows(t, rows));
                        core.say(format!("{n} item(s) {} {}", if moving { "moved to" } else { "copied to" }, strip[t].0));
                    }
                }
            }
        });
    }
}
