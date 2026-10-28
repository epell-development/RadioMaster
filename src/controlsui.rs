//! Shortcuts and MIDI: firing actions from keys and MIDI controllers, and Help → Shortcuts and
//! MIDI, where every action's keys and MIDI controls can be changed and the user can add their
//! own command shortcuts.
use super::pltools::DlgKind;
use super::*;
use crate::controls::{self, action, CustomCmd, KeyCombo, MidiAction, MidiCtl, ACTIONS, GROUPS, VOLUME_FADER};

/// The settings of Help → Shortcuts and MIDI that live only while the app runs.
#[derive(Default)]
pub(super) struct ControlsUi {
    /// waiting for a key for this action
    pub key_rec: Option<String>,
    /// waiting for a MIDI control for this action
    pub midi_learn: Option<String>,
    /// the last MIDI message, so the user can see their controller is heard
    pub midi_last: Option<controls::MidiEvent>,
    /// what the last change did (e.g. took a key from another action)
    pub note: String,
    pub filter: String,
    pub router: controls::MidiRouter,
}

impl App {
    /// After the bindings change: new key map, save, MIDI on or off.
    fn controls_changed(&mut self) {
        self.keymap = self.controls.keymap();
        self.controls.save();
        self.midi.set_enabled(self.controls.midi_enabled);
    }

    /// Keyboard shortcuts. Ignored while typing in a text box; each needs the matching permission.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context, core: &mut Playout) {
        use egui::{Key, Modifiers};
        if ctx.wants_keyboard_input() || self.cui.key_rec.is_some() {
            return;
        }
        // the user's keys, matched exactly (Ctrl+Shift+P isn't Ctrl+P)
        let keymap = &self.keymap;
        let fired: Vec<String> = ctx.input_mut(|i| {
            let mut out = vec![];
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed: true, repeat, modifiers, .. } => match keymap.get(&KeyCombo::from_event(*key, *modifiers))
                {
                    Some(id) => {
                        if !*repeat || action(id).is_some_and(|a| a.repeat) {
                            out.push(id.clone());
                        }
                        false
                    }
                    None => true,
                },
                _ => true,
            });
            out
        });
        for id in fired {
            self.run_action(ctx, core, &id);
        }
        // ---- moving around (fixed keys)
        let press = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let len = core.tab_len(self.view_pl);
        for (k, up) in [(Key::ArrowUp, true), (Key::ArrowDown, false)] {
            let shift = ctx.input(|i| i.modifiers.shift);
            if (press(Modifiers::NONE, k) || press(Modifiers::SHIFT, k)) && len > 0 {
                let from = if up { self.sel.iter().next().copied() } else { self.sel.iter().next_back().copied() };
                let to = match from {
                    Some(i) if up => i.saturating_sub(1),
                    Some(i) => (i + 1).min(len - 1),
                    None => core.current.unwrap_or(0).min(len - 1),
                };
                if shift {
                    self.sel.insert(to);
                } else {
                    self.sel.clear();
                    self.sel.insert(to);
                    self.anchor = Some(to);
                }
                self.scroll_to = Some(to);
            }
        }
    }

    /// MIDI controllers: fire the bound actions (also while typing), or learn a control.
    pub(super) fn midi_input(&mut self, ctx: &egui::Context, core: &mut Playout) {
        for e in self.midi.drain() {
            self.cui.midi_last = Some(e);
            if let Some(id) = self.cui.midi_learn.clone() {
                // learn from a press, not from the release that follows it
                if e.value > 0 {
                    self.cui.midi_learn = None;
                    let prev = self.controls.bind_midi(&id, e.ctl);
                    self.cui.note = match prev {
                        Some(p) => format!(
                            "{} {} → {} ({} {})",
                            tr("MIDI"),
                            e.ctl.label(),
                            self.controls.name_of(&id),
                            tr("taken from"),
                            self.controls.name_of(&p)
                        ),
                        None => format!("{} {} → {}", tr("MIDI"), e.ctl.label(), self.controls.name_of(&id)),
                    };
                    self.controls_changed();
                }
                continue;
            }
            match self.cui.router.route(&self.controls, e) {
                Some(MidiAction::Fire(id)) => self.run_action(ctx, core, &id),
                Some(MidiAction::Volume(v)) if self.perm.playback => core.set_volume(v),
                _ => {}
            }
        }
    }

    /// Does what a shortcut or MIDI control is bound to, if the account may.
    pub(super) fn run_action(&mut self, ctx: &egui::Context, core: &mut Playout, id: &str) {
        let p = self.perm;
        if let Some(n) = id.strip_prefix("cmd:") {
            let cmd = self.controls.custom.iter().find(|c| c.id.to_string() == n).map(|c| c.command.clone());
            if let Some(cmd) = cmd.filter(|c| !c.trim().is_empty()) {
                if p.playback {
                    core.run_command(&cmd);
                } else {
                    core.say(tr("Your account isn't allowed to do this"));
                }
            }
            return;
        }
        if let Some(n) = id.strip_prefix("cart").and_then(|n| n.parse::<usize>().ok()) {
            if p.playback && p.jingles && n >= 1 {
                core.trigger_cart(n - 1);
            }
            return;
        }
        let edit = p.edit_playlists;
        match id {
            // ---- playback
            "play" if p.playback => {
                let s = if self.view_pl == core.playing_tab || self.sel.is_empty() { self.sel.iter().next().copied() } else { None };
                match s {
                    Some(i) if self.view_pl != core.playing_tab => {
                        let xf = if core.state == PlayState::Playing { core.cfg.crossfade_secs } else { 0.0 };
                        core.with_tab(self.view_pl, |c| c.play_index(i, xf));
                    }
                    Some(i) if core.state == PlayState::Playing => {
                        let xf = core.cfg.crossfade_secs;
                        core.play_index(i, xf);
                    }
                    _ => core.play_pressed(s),
                }
            }
            "pause" if p.playback => core.pause_toggle(),
            "stop" if p.playback => core.stop(),
            "next" if p.playback => core.next(),
            "prev" if p.playback => core.prev(),
            "volume_up" | "volume_down" if p.playback => {
                let d = if id == "volume_up" { 0.05 } else { -0.05 };
                let v = (core.cfg.volume + d).clamp(0.0, 1.0);
                core.set_volume(v);
            }
            "seek_back" if p.playback => core.seek_by(-30.0),
            "seek_forward" if p.playback => core.seek_by(30.0),
            "seek_to" if p.playback => self.open_seek_to(ctx),
            "mic" if p.playback => self.toggle_mic(core),
            "stop_jingles" if p.playback && p.jingles => core.stop_carts(),
            // ---- playlist editing
            "add_files" if edit => {
                self.pick(Pick::Files(AUDIO.0, AUDIO.1), tr("Add files"), |_, v| Some(Dlg::AddFiles(v)));
            }
            "add_folder" if edit => self.pick(Pick::Folder, tr("Add folder"), |fs, v| Some(Dlg::add_folder(fs, v.into_iter().next()?))),
            "add_url" if edit => self.open_item_dlg(DlgKind::Url),
            "add_command" if edit => self.open_item_dlg(DlgKind::Command),
            "add_tts" if edit => self.open_item_dlg(DlgKind::Tts),
            "add_comment" if edit => self.open_item_dlg(DlgKind::Comment),
            "add_pause" if edit => self.open_item_dlg(DlgKind::Pause),
            "add_hour" if edit => self.open_item_dlg(DlgKind::HourMarker),
            "insert_voice" if edit => {
                self.pick(Pick::Files(AUDIO.0, AUDIO.1), tr("Insert voice track"), |_, v| Some(Dlg::InsertVoice(v)));
            }
            "record_voice" if edit && p.tool(crate::auth::tool::VOICE_TRACK) => self.show_vt = true,
            "new_playlist" if edit => self.tab_ops.push(super::plview::TabOp::New),
            "open_playlist" if edit => {
                self.pick(Pick::File(M3U.0, M3U.1), tr("Open playlist"), |fs, v| Some(Dlg::load_m3u(fs, v.into_iter().next()?)));
            }
            "save_playlist" if edit => {
                let name = format!("{}.m3u", core.tabs.get(self.view_pl).map(|t| t.name.clone()).unwrap_or_else(|| "playlist".into()));
                self.dlg(move || {
                    rfd::FileDialog::new().add_filter("Playlist", &["m3u"]).set_file_name(&name).save_file().map(Dlg::SaveM3u)
                });
            }
            "move_up" | "move_down" if edit && !self.sel.is_empty() => {
                let sel = self.sel.clone();
                self.sel = core.with_tab(self.view_pl, |c| c.move_sel(&sel, id == "move_up"));
                self.scroll_to = self.sel.iter().next().copied();
            }
            "next_tab" if !core.tabs.is_empty() => self.tab_ops.push(super::plview::TabOp::View((self.view_pl + 1) % core.tabs.len())),
            "prev_tab" if !core.tabs.is_empty() => {
                self.tab_ops.push(super::plview::TabOp::View((self.view_pl + core.tabs.len() - 1) % core.tabs.len()))
            }
            "find" => {
                self.show_find = true;
                self.find_focus = true;
            }
            // ---- tools and windows
            "generator" if p.tools => self.open_tool(ToolWin::Generator),
            "track_editor" if p.tools => {
                let row = self.sel.iter().next().copied();
                core.with_tab(self.view_pl, |c| self.open_fade_editor(c, row));
            }
            "scheduler" => self.open_tool(ToolWin::Scheduler),
            "audio_fx" => self.open_tool(ToolWin::AudioFx),
            "notepad" => self.open_tool(ToolWin::Notepad),
            "settings" if p.settings => self.show_settings = true,
            "panels1" => self.workzones.panels = 1,
            "panels2" => self.workzones.panels = 2,
            "panels3" => self.workzones.panels = 3,
            "new_window" => {
                self.workzones.new_aux(Tab::AuxPlaylist);
            }
            "left_panel" => self.workzones.show_left = !self.workzones.show_left,
            "file_explorer" | "search" => {
                self.workzones.show_left = true;
                self.workzones.left.add(if id == "search" { Tab::Search } else { Tab::FileExplorer });
            }
            "fullscreen" => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            "performance" => self.perf = if self.perf.is_some() { None } else { Some(Perf::default()) },
            "shortcuts" => self.show_keys = !self.show_keys,
            _ => {}
        }
    }

    /// Takes the next key press for the action being recorded (Esc cancels).
    fn record_key(&mut self, ctx: &egui::Context) {
        let Some(id) = self.cui.key_rec.clone() else {
            return;
        };
        let got = ctx.input_mut(|i| {
            let mut got = None;
            i.events.retain(|e| match e {
                egui::Event::Key { key, pressed, modifiers, .. } => {
                    if *pressed && got.is_none() {
                        got = Some(KeyCombo::from_event(*key, *modifiers));
                    }
                    false
                }
                _ => true,
            });
            got
        });
        let Some(k) = got else {
            return;
        };
        self.cui.key_rec = None;
        if k.key == egui::Key::Escape && !k.ctrl && !k.shift && !k.alt {
            return;
        }
        let prev = self.controls.bind_key(&id, k);
        self.cui.note = match prev {
            Some(p) => format!("{} → {} ({} {})", k.label(), self.controls.name_of(&id), tr("taken from"), self.controls.name_of(&p)),
            None => format!("{} → {}", k.label(), self.controls.name_of(&id)),
        };
        self.controls_changed();
    }

    /// Help → Shortcuts and MIDI.
    pub(super) fn keys_window(&mut self, ctx: &egui::Context) {
        if !self.show_keys {
            self.cui.key_rec = None;
            self.cui.midi_learn = None;
            return;
        }
        self.record_key(ctx);
        if self.cui.key_rec.is_some() || self.cui.midi_learn.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut open = true;
        let mut changed = false;
        Self::window_for(ctx, tr("Shortcuts and MIDI"), [820.0, 640.0]).id(egui::Id::new("keys_window")).open(&mut open).show(ctx, |ui| {
            // ---- MIDI on/off and what it hears
            ui.horizontal_wrapped(|ui| {
                changed |= ui.checkbox(&mut self.controls.midi_enabled, tr("Listen to MIDI controllers")).changed();
                if self.controls.midi_enabled {
                    let st = self.midi.status();
                    if !st.error.is_empty() {
                        ui.label(RichText::new(format!("{} {}", tr("MIDI isn't available:"), st.error)).color(AMBER));
                    } else if st.ports.is_empty() {
                        ui.label(RichText::new(tr("No MIDI controller found — plug one in and it's picked up within a few seconds.")).color(GRAY));
                        ctx.request_repaint_after(std::time::Duration::from_secs(1));
                    } else {
                        ui.label(RichText::new(format!("{} {}", tr("Listening to:"), st.ports.join(", "))).color(GREEN));
                    }
                    if let Some(e) = self.cui.midi_last {
                        ui.label(RichText::new(format!("{} {} = {}", tr("last:"), e.ctl.label(), e.value)).small().color(GRAY));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.cui.filter).hint_text(tr("🔍 search actions")).desired_width(220.0));
                if !self.cui.note.is_empty() {
                    ui.label(RichText::new(&self.cui.note).color(ACCENT));
                }
            });
            ui.label(
                RichText::new(tr(
                    "Click ➕ and press the keys, or click Learn and press a pad, key or button on your MIDI controller. A key or control can only do one thing; giving it to another action moves it.",
                ))
                .small()
                .color(GRAY),
            );
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let f = self.cui.filter.to_lowercase();
                let shows = |name: &str| f.is_empty() || name.to_lowercase().contains(&f);
                for group in GROUPS {
                    let rows: Vec<(String, String)> =
                        ACTIONS.iter().filter(|a| a.group == group).map(|a| (a.id.to_string(), tr(a.label).to_string())).filter(|(_, n)| shows(n)).collect();
                    let fader = group == controls::PLAYBACK && shows(&self.controls.name_of(VOLUME_FADER));
                    if rows.is_empty() && !fader {
                        continue;
                    }
                    ui.add_space(6.0);
                    ui.strong(tr(group));
                    egui::Grid::new(("keys", group)).num_columns(3).spacing([14.0, 4.0]).striped(true).min_col_width(120.0).show(ui, |ui| {
                        for (id, name) in &rows {
                            ui.label(name);
                            changed |= self.key_cell(ui, id);
                            changed |= self.midi_cell(ui, id);
                            ui.end_row();
                        }
                        if fader {
                            ui.label(self.controls.name_of(VOLUME_FADER));
                            ui.label(RichText::new(tr("MIDI only")).small().color(GRAY));
                            changed |= self.midi_cell(ui, VOLUME_FADER);
                            ui.end_row();
                        }
                    });
                }
                // ---- the user's own shortcuts
                ui.add_space(10.0);
                ui.strong(tr("My command shortcuts"));
                ui.label(
                    RichText::new(tr(
                        "Each runs a playlist command, e.g.  PLAYLIST Evening,  VOLUME 60,  GENERATE Drive time,  RULE Top of hour jingle  or  STOP.",
                    ))
                    .small()
                    .color(GRAY),
                );
                let mut remove = None;
                let ids: Vec<u64> = self.controls.custom.iter().map(|c| c.id).collect();
                egui::Grid::new("custom_keys").num_columns(5).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
                    for cid in ids {
                        let Some(k) = self.controls.custom.iter().position(|c| c.id == cid) else {
                            continue;
                        };
                        let c = &mut self.controls.custom[k];
                        changed |= ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut c.name).hint_text(tr("name"))).lost_focus();
                        changed |= ui.add_sized([220.0, 20.0], egui::TextEdit::singleline(&mut c.command).hint_text(tr("command, e.g. PLAYLIST Evening"))).lost_focus();
                        let aid = c.action_id();
                        changed |= self.key_cell(ui, &aid);
                        changed |= self.midi_cell(ui, &aid);
                        if delete_button(ui, ("custom_key", cid), tr("this shortcut")) {
                            remove = Some(cid);
                        }
                        ui.end_row();
                    }
                });
                if let Some(cid) = remove {
                    self.controls.remove_custom(cid);
                    changed = true;
                }
                if ui.button(tr("➕ Add command shortcut")).clicked() {
                    let id = self.controls.new_custom_id();
                    self.controls.custom.push(CustomCmd { id, name: format!("{} {id}", tr("Command")), command: String::new() });
                    changed = true;
                }
                // ---- fixed keys
                ui.add_space(10.0);
                ui.strong(tr("Fixed keys"));
                egui::Grid::new("fixed_keys").num_columns(2).spacing([18.0, 4.0]).striped(true).show(ui, |ui| {
                    for (k, what) in [
                        ("↑ / ↓", tr("Select the row above / below (Shift adds to the selection)")),
                        ("Delete", tr("Remove the selected rows")),
                        ("Ctrl+A", tr("Select all")),
                        (tr("Drag rows"), tr("Reorder, or drop on another playlist's tab to move them there")),
                    ] {
                        ui.label(RichText::new(k).monospace().color(ACCENT));
                        ui.label(what);
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ask = egui::Id::new("keys_reset_ask");
                    if ui.data(|d| d.get_temp::<bool>(ask)).unwrap_or(false) {
                        if ui.button(RichText::new(tr("Reset!")).color(RED)).clicked() {
                            self.controls.keys.clear();
                            self.cui.note = tr("Keys are back to the defaults.").into();
                            changed = true;
                            ui.data_mut(|d| d.remove::<bool>(ask));
                        }
                        if ui.button(tr("Keep")).clicked() {
                            ui.data_mut(|d| d.remove::<bool>(ask));
                        }
                    } else if ui.button(tr("Reset all keys to the defaults")).clicked() {
                        ui.data_mut(|d| d.insert_temp(ask, true));
                    }
                });
                ui.label(
                    RichText::new(tr("Shortcuts don't fire while you're typing in a text box (MIDI controls still do). Some need permission for your account."))
                        .small()
                        .color(GRAY),
                );
            });
        });
        if changed {
            self.controls_changed();
        }
        self.show_keys = open;
    }

    /// An action's keys, each with a ✖, and ➕ to record another. Returns true when it changed.
    fn key_cell(&mut self, ui: &mut egui::Ui, id: &str) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let keys = self.controls.keys_of(id);
            for k in &keys {
                ui.label(RichText::new(k.label()).monospace().color(ACCENT));
                if ui.small_button("✖").on_hover_text(tr("Remove this key")).clicked() {
                    let rest = keys.iter().copied().filter(|x| x != k).collect();
                    self.controls.set_keys(id, rest);
                    changed = true;
                }
            }
            let rec = self.cui.key_rec.as_deref() == Some(id);
            let b = if rec { RichText::new(tr("Press a key… (Esc cancels)")).color(AMBER) } else { RichText::new("➕") };
            if ui.small_button(b).on_hover_text(tr("Add a key")).clicked() {
                self.cui.key_rec = if rec { None } else { Some(id.to_string()) };
                self.cui.midi_learn = None;
            }
            if self.controls.keys.contains_key(id)
                && action(id).is_some()
                && ui.small_button("↺").on_hover_text(tr("Back to the default keys")).clicked()
            {
                self.controls.keys.remove(id);
                changed = true;
            }
        });
        changed
    }

    /// An action's MIDI controls, each with a ✖, and Learn to add one. Returns true when it changed.
    fn midi_cell(&mut self, ui: &mut egui::Ui, id: &str) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let ctls: Vec<MidiCtl> = self.controls.midi_of(id);
            for c in ctls {
                ui.label(RichText::new(c.label()).small().color(ACCENT));
                if ui.small_button("✖").on_hover_text(tr("Remove this MIDI control")).clicked() {
                    self.controls.midi.retain(|b| b.ctl != c);
                    changed = true;
                }
            }
            let learning = self.cui.midi_learn.as_deref() == Some(id);
            let text =
                if learning { RichText::new(tr("Move a control… (click to cancel)")).color(AMBER) } else { RichText::new(tr("Learn")) };
            let on = self.controls.midi_enabled;
            if ui
                .add_enabled(on, egui::Button::new(text).small())
                .on_disabled_hover_text(tr("Turn on Listen to MIDI controllers first"))
                .clicked()
            {
                self.cui.midi_learn = if learning { None } else { Some(id.to_string()) };
                self.cui.key_rec = None;
            }
        });
        changed
    }
}
