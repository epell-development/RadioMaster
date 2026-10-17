//! The playlist view: RadioBOSS-style playlist tabs (one is on air), the table of rows with
//! start times, and row actions.

use super::panes::{row_background, row_text_color};
use super::*;
use crate::items::{ItemKind, PlEntry};

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
                if r.double_clicked() {
                    self.renaming = Some((i, name.clone()));
                }
                r.context_menu(|ui| {
                    if ui.add_enabled(!live, egui::Button::new("▶ Put this playlist on air")).clicked() {
                        self.tab_ops.push(TabOp::PlayTab(i));
                        self.tab_ops.push(TabOp::View(i));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("✏ Rename…").clicked() {
                        self.renaming = Some((i, name.clone()));
                        ui.close_menu();
                    }
                    if ui.button("Duplicate").clicked() {
                        self.tab_ops.push(TabOp::Duplicate(i));
                        ui.close_menu();
                    }
                    if ui.button("💾 Save as M3U…").clicked() {
                        self.tab_ops.push(TabOp::SaveM3u(i));
                        ui.close_menu();
                    }
                    if ui.button("📂 Load M3U into this tab…").clicked() {
                        self.tab_ops.push(TabOp::LoadM3u(i));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(i > 0, egui::Button::new("◀ Move left")).clicked() {
                        self.tab_ops.push(TabOp::MoveTab(i, i - 1));
                        ui.close_menu();
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("Move right ▶")).clicked() {
                        self.tab_ops.push(TabOp::MoveTab(i, i + 1));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add_enabled(!live, egui::Button::new("Clear")).clicked() {
                        self.tab_ops.push(TabOp::Clear(i));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(!live && n > 1, egui::Button::new("✖ Close"))
                        .on_disabled_hover_text("The playlist that is on air can't be closed")
                        .clicked()
                    {
                        self.tab_ops.push(TabOp::Close(i));
                        ui.close_menu();
                    }
                });
            }
            if ui.small_button("➕").on_hover_text("New playlist").clicked() {
                self.tab_ops.push(TabOp::New);
            }
        });
    }

    fn playlist_view(&mut self, ctx: &egui::Context, core: &mut Playout, strip: &[(String, usize, f32)], on_air_tab: usize) {
        let viewing_on_air = self.view_pl == on_air_tab;
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) && !self.sel.is_empty() {
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
                    ui.label(RichText::new("not on air").small().color(Color32::from_rgb(190, 120, 0)))
                        .on_hover_text("Edit this playlist freely. Play any row (or right-click the tab) to put it on air.");
                }
                ui.label(RichText::new(format!("{} items · {}", core.playlist.len(), fmt(core.total_secs()))).color(GRAY));
                ui.checkbox(&mut core.cfg.loop_playlist, "Loop");
                ui.checkbox(&mut core.cfg.auto_enabled, "Auto-playlist");
                ui.checkbox(&mut core.cfg.random_fill, "Auto-fill from library");
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
            let scroll_to = self.scroll_to.take();
            let plays = &core.plays;
            let sel = &mut self.sel;
            let anchor = &mut self.anchor;
            let pl = &core.playlist;

            let mut tb = TableBuilder::new(ui)
                .id_salt(("playlist_table", (avail_w / 24.0) as i32, wide, very_wide))
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
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
            if let Some(r) = scroll_to {
                tb = tb.scroll_to_row(r, Some(egui::Align::Center));
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
                        ui.strong(t);
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
                    if resp.double_clicked() {
                        acts.push(PlAct::Play(i));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("▶ Play now").clicked() {
                            acts.push(PlAct::Play(i));
                            ui.close_menu();
                        }
                        if e.is_file() {
                            if ui.button("✏ Track editor (tags, cues, fades)…").clicked() {
                                acts.push(PlAct::Fade(i));
                                ui.close_menu();
                            }
                        } else if !matches!(e.kind, ItemKind::TimeAnnounce) && ui.button("✏ Edit…").clicked() {
                            acts.push(PlAct::Edit(i));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("⬆ Move up").clicked() {
                            acts.push(PlAct::Up);
                            ui.close_menu();
                        }
                        if ui.button("⬇ Move down").clicked() {
                            acts.push(PlAct::Down);
                            ui.close_menu();
                        }
                        if strip.len() > 1 {
                            ui.menu_button("Copy to playlist", |ui| {
                                for (t, (name, _, _)) in strip.iter().enumerate() {
                                    if t != view_tab && ui.button(name).clicked() {
                                        acts.push(PlAct::CopyTo(t));
                                        ui.close_menu();
                                    }
                                }
                            });
                            ui.menu_button("Move to playlist", |ui| {
                                for (t, (name, _, _)) in strip.iter().enumerate() {
                                    if t != view_tab && ui.button(name).clicked() {
                                        acts.push(PlAct::MoveTo(t));
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                        if e.is_file() && ui.button("☁ Upload to AzuraCast").clicked() {
                            acts.push(PlAct::Upload);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("🗑 Remove").clicked() {
                            acts.push(PlAct::Remove);
                            ui.close_menu();
                        }
                    });
                });
            });

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
