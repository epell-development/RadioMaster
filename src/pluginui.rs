//! What plugins add to the app: a Plugins menu, toolbar buttons, playlist right-click items,
//! their own windows of controls, and their settings in Settings → Plugins.
//! The plugins themselves run in plugins.rs; PLUGINS.md describes what they can send.
use super::*;
use crate::plugins::UiItem;
use serde_json::{json, Map, Value};

/// A control in a plugin's window was used.
struct PanelAct {
    id: String,
    /// a station command to run instead of telling the plugin
    command: String,
    value: Value,
}

fn color_of(name: &str) -> Option<Color32> {
    match name.to_lowercase().as_str() {
        "red" => Some(RED),
        "green" => Some(GREEN),
        "amber" | "yellow" | "orange" => Some(AMBER),
        "gray" | "grey" => Some(GRAY),
        "blue" => Some(Color32::from_rgb(20, 150, 235)),
        h if h.len() == 7 && h.starts_with('#') => {
            let n = u32::from_str_radix(&h[1..], 16).ok()?;
            Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
        }
        _ => None,
    }
}

/// Draw a plugin's controls (see "Windows of controls" in PLUGINS.md).
fn widgets(ui: &mut egui::Ui, items: &[Value], values: &mut Map<String, Value>, acts: &mut Vec<PanelAct>) {
    for (i, w) in items.iter().enumerate() {
        let s = |k: &str| w.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let b = |k: &str| w.get(k).and_then(Value::as_bool).unwrap_or(false);
        let n = |k: &str, d: f64| w.get(k).and_then(Value::as_f64).unwrap_or(d);
        let id = s("id");
        let tip = s("tooltip");
        let kind = s("type");
        let label = s("label");
        match kind.as_str() {
            "heading" => {
                ui.heading(s("text"));
            }
            "label" | "" => {
                let mut t = RichText::new(s("text"));
                if b("bold") {
                    t = t.strong();
                }
                if b("small") {
                    t = t.small();
                }
                if b("mono") {
                    t = t.monospace();
                }
                if let Some(c) = color_of(&s("color")) {
                    t = t.color(c);
                }
                ui.label(t);
            }
            "separator" => {
                ui.separator();
            }
            "space" => ui.add_space(n("size", 8.0) as f32),
            "button" => {
                let r = ui.button(&label);
                let r = if tip.is_empty() { r } else { r.on_hover_text(&tip) };
                if r.clicked() {
                    acts.push(PanelAct { id, command: s("command"), value: Value::Null });
                }
            }
            "text" => {
                let mut v = values.get(&id).and_then(Value::as_str).map(String::from).unwrap_or_else(|| s("value"));
                let multi = b("multiline");
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.label(&label);
                    }
                    let te = if multi { egui::TextEdit::multiline(&mut v).desired_rows(3) } else { egui::TextEdit::singleline(&mut v) };
                    let r = ui.add(te.hint_text(s("hint")).password(b("password")).desired_width(ui.available_width()));
                    if r.changed() {
                        values.insert(id.clone(), Value::String(v.clone()));
                    }
                    if !multi && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        acts.push(PanelAct { id: id.clone(), command: String::new(), value: Value::String(v.clone()) });
                    }
                });
            }
            "number" | "slider" => {
                let (lo, hi) = (n("min", 0.0), n("max", 100.0));
                let mut v = values.get(&id).and_then(Value::as_f64).unwrap_or_else(|| n("value", lo));
                let r = ui
                    .horizontal(|ui| {
                        if kind == "number" {
                            if !label.is_empty() {
                                ui.label(&label);
                            }
                            ui.add(egui::DragValue::new(&mut v).range(lo..=hi).speed(n("step", 1.0)))
                        } else {
                            ui.add(egui::Slider::new(&mut v, lo..=hi).text(&label))
                        }
                    })
                    .inner;
                if r.changed() {
                    values.insert(id.clone(), json!(v));
                }
                // told once the drag ends, not on every step of it
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    acts.push(PanelAct { id, command: String::new(), value: json!(v) });
                }
            }
            "checkbox" => {
                let mut v = values.get(&id).and_then(Value::as_bool).unwrap_or_else(|| b("value"));
                let r = ui.checkbox(&mut v, &label);
                let r = if tip.is_empty() { r } else { r.on_hover_text(&tip) };
                if r.changed() {
                    values.insert(id.clone(), json!(v));
                    acts.push(PanelAct { id, command: String::new(), value: json!(v) });
                }
            }
            "choice" => {
                let cur = values.get(&id).and_then(Value::as_str).map(String::from).unwrap_or_else(|| s("value"));
                let options: Vec<String> = w
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|o| o.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    if !label.is_empty() {
                        ui.label(&label);
                    }
                    egui::ComboBox::from_id_salt(("plugin_choice", &id)).selected_text(&cur).show_ui(ui, |ui| {
                        for o in options {
                            if ui.selectable_label(o == cur, &o).clicked() && o != cur {
                                values.insert(id.clone(), json!(o));
                                acts.push(PanelAct { id: id.clone(), command: String::new(), value: json!(o) });
                            }
                        }
                    });
                });
            }
            "progress" => {
                let mut p = egui::ProgressBar::new(n("value", 0.0).clamp(0.0, 1.0) as f32);
                if !s("text").is_empty() {
                    p = p.text(s("text"));
                }
                ui.add(p);
            }
            "link" => {
                ui.hyperlink_to(s("text"), s("url"));
            }
            "row" => {
                let inner = w.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                ui.horizontal_wrapped(|ui| widgets(ui, &inner, values, acts));
            }
            "group" => {
                let inner = w.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                egui::CollapsingHeader::new(s("title"))
                    .id_salt(ui.id().with(("plugin_group", i)))
                    .default_open(!b("collapsed"))
                    .show(ui, |ui| widgets(ui, &inner, values, acts));
            }
            "table" => {
                let cols: Vec<String> = w
                    .get("columns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect())
                    .unwrap_or_default();
                let rows = w.get("rows").and_then(Value::as_array).cloned().unwrap_or_default();
                let cell = |c: &Value| match c {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                egui::Grid::new(ui.id().with(("plugin_table", i))).striped(true).num_columns(cols.len().max(1)).show(ui, |ui| {
                    if !cols.is_empty() {
                        for c in &cols {
                            ui.strong(c);
                        }
                        ui.end_row();
                    }
                    for (r, row) in rows.iter().enumerate() {
                        let cells: Vec<String> = row.as_array().map(|a| a.iter().map(cell).collect()).unwrap_or_else(|| vec![cell(row)]);
                        for (k, c) in cells.iter().enumerate() {
                            // with an id, the first cell of each row can be clicked
                            if k == 0 && !id.is_empty() {
                                if ui.link(c).clicked() {
                                    acts.push(PanelAct { id: id.clone(), command: String::new(), value: json!(r) });
                                }
                            } else {
                                ui.label(c);
                            }
                        }
                        ui.end_row();
                    }
                });
            }
            other => {
                ui.label(RichText::new(format!("(unknown control \"{other}\")")).small().color(GRAY));
            }
        }
    }
}

impl App {
    fn may_use_plugins(&self) -> bool {
        self.perm.tool(crate::auth::tool::PLUGINS)
    }

    /// A plugin's menu item or button: its station command, or an "action" event to the plugin.
    fn plugin_act(core: &mut Playout, plugin: &str, item: &UiItem, source: &str, extra: Value) {
        if !item.command.trim().is_empty() {
            core.run_command(&item.command);
        } else {
            let off = core.cfg.plugins_off.clone();
            core.plugins.action(plugin, &item.id, source, extra, &off);
        }
    }

    /// The Plugins menu in the menu bar (only there when a plugin has added something to it).
    pub(super) fn plugins_menu(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let entries: Vec<(String, String, Option<String>, Vec<UiItem>)> = core
            .plugins
            .ui
            .iter()
            .filter(|(_, u)| !u.menu.is_empty() || u.has_panel())
            .map(|(id, u)| (id.clone(), u.name.clone(), u.has_panel().then(|| u.panel_title.clone()), u.menu.clone()))
            .collect();
        if entries.is_empty() {
            return;
        }
        ui.menu_button(tr("Plugins"), |ui| {
            if !self.may_use_plugins() {
                ui.label(RichText::new(tr("Your account isn't allowed to use plugins")).small().color(AMBER));
                ui.disable();
            }
            let many = entries.len() > 1;
            let mut act: Option<(String, UiItem)> = None;
            let mut open: Option<String> = None;
            for (k, (id, name, panel, menu)) in entries.iter().enumerate() {
                if many {
                    if k > 0 {
                        ui.separator();
                    }
                    ui.label(RichText::new(name).small().color(GRAY));
                }
                if let Some(title) = panel {
                    if ui.button(format!("{title}…")).clicked() {
                        open = Some(id.clone());
                        ui.close_menu();
                    }
                }
                for it in menu {
                    let r = ui.button(&it.label);
                    let r = if it.tooltip.is_empty() { r } else { r.on_hover_text(&it.tooltip) };
                    if r.clicked() {
                        act = Some((id.clone(), it.clone()));
                        ui.close_menu();
                    }
                }
            }
            ui.separator();
            self.tool_menu_item(ui, core, ToolWin::Plugins);
            if let Some(u) = open.and_then(|id| core.plugins.ui.get_mut(&id)) {
                u.open = true;
            }
            if let Some((id, it)) = act {
                Self::plugin_act(core, &id, &it, "menu", json!({}));
            }
        });
    }

    /// Buttons plugins add to the toolbar.
    pub(super) fn plugin_buttons(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let items: Vec<(String, UiItem)> =
            core.plugins.ui.iter().flat_map(|(id, u)| u.buttons.iter().map(move |b| (id.clone(), b.clone()))).collect();
        if items.is_empty() {
            return;
        }
        ui.separator();
        let may = self.may_use_plugins();
        for (id, it) in items {
            let r = ui.add_enabled(may, egui::Button::new(&it.label));
            let r = if it.tooltip.is_empty() { r } else { r.on_hover_text(&it.tooltip) };
            if r.clicked() {
                Self::plugin_act(core, &id, &it, "button", json!({}));
            }
        }
    }

    /// Playlist right-click items plugins add: (plugin id, item).
    pub(super) fn plugin_row_items(&self, core: &Playout) -> Vec<(String, UiItem)> {
        if !self.may_use_plugins() {
            return vec![];
        }
        core.plugins.ui.iter().flat_map(|(id, u)| u.playlist_menu.iter().map(move |b| (id.clone(), b.clone()))).collect()
    }

    /// A plugin's playlist right-click item was picked: it gets the selected rows.
    pub(super) fn plugin_row_act(&self, core: &mut Playout, plugin: &str, item: &UiItem) {
        let rows: Vec<Value> = self
            .sel
            .iter()
            .filter_map(|&i| core.playlist.get(i).map(|e| (i, e)))
            .map(|(i, e)| {
                json!({
                    "row": i,
                    "label": e.title(),
                    "title": e.track.title,
                    "artist": e.track.artist,
                    "path": if e.is_file() { e.track.path.to_string_lossy().to_string() } else { String::new() },
                })
            })
            .collect();
        let playlist = core.tabs.get(self.view_pl).map(|t| t.name.clone()).unwrap_or_default();
        Self::plugin_act(core, plugin, item, "playlist", json!({ "rows": rows, "playlist": playlist }));
    }

    /// The windows plugins open (their own controls).
    pub(super) fn plugin_windows(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let ids: Vec<String> = core.plugins.ui.iter().filter(|(_, u)| u.open && u.has_panel()).map(|(id, _)| id.clone()).collect();
        let may = self.may_use_plugins();
        for id in ids {
            let Some(u) = core.plugins.ui.get_mut(&id) else { continue };
            let mut open = true;
            let mut acts: Vec<PanelAct> = vec![];
            Self::window_for(ctx, &u.panel_title, [380.0, 340.0]).id(egui::Id::new(("plugin_panel", &id))).open(&mut open).show(
                ctx,
                |ui| {
                    if !may {
                        ui.label(RichText::new(tr("Your account isn't allowed to use plugins")).small().color(AMBER));
                        ui.disable();
                    }
                    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| widgets(ui, &u.panel, &mut u.values, &mut acts));
                },
            );
            if !open {
                u.open = false;
            }
            let off = core.cfg.plugins_off.clone();
            for a in acts {
                if !a.command.trim().is_empty() {
                    core.run_command(&a.command);
                } else {
                    core.plugins.action(&id, &a.id, "panel", json!({ "value": a.value }), &off);
                }
            }
            if !open {
                core.plugins.action(&id, "panel_closed", "panel", json!({}), &off);
            }
        }
    }

    /// A plugin's settings, under it in Settings → Plugins. True when one was changed (and should be sent).
    pub(super) fn plugin_settings_ui(ui: &mut egui::Ui, def: &crate::plugins::PluginDef, saved: &mut Map<String, Value>) -> bool {
        let mut send = false;
        egui::CollapsingHeader::new(tr("Settings")).id_salt(("plugin_settings", &def.dir)).show(ui, |ui| {
            egui::Grid::new(("plugin_settings_grid", &def.dir)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for st in &def.settings {
                    let label = if st.label.is_empty() { st.id.clone() } else { st.label.clone() };
                    let r = ui.label(&label);
                    if !st.help.is_empty() {
                        r.on_hover_text(&st.help);
                    }
                    let cur = saved.get(&st.id).cloned().unwrap_or_else(|| st.default.clone());
                    match st.kind.as_str() {
                        "bool" | "checkbox" => {
                            let mut v = cur.as_bool().unwrap_or(false);
                            if ui.checkbox(&mut v, "").changed() {
                                saved.insert(st.id.clone(), json!(v));
                                send = true;
                            }
                        }
                        "number" => {
                            let mut v = cur.as_f64().unwrap_or(0.0);
                            let r = ui.add(egui::DragValue::new(&mut v));
                            if r.changed() {
                                saved.insert(st.id.clone(), json!(v));
                            }
                            send |= r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus());
                        }
                        "choice" => {
                            let c = cur.as_str().unwrap_or_default().to_string();
                            egui::ComboBox::from_id_salt(("plugin_setting", &def.dir, &st.id)).selected_text(&c).show_ui(ui, |ui| {
                                for o in &st.options {
                                    if ui.selectable_label(*o == c, o).clicked() && *o != c {
                                        saved.insert(st.id.clone(), json!(o));
                                        send = true;
                                    }
                                }
                            });
                        }
                        kind => {
                            // text / password: sent once the field is left
                            let mut v = match &cur {
                                Value::String(s) => s.clone(),
                                Value::Null => String::new(),
                                other => other.to_string(),
                            };
                            let r = ui.add(egui::TextEdit::singleline(&mut v).password(kind == "password").desired_width(260.0));
                            if r.changed() {
                                saved.insert(st.id.clone(), json!(v));
                            }
                            send |= r.lost_focus();
                        }
                    }
                    ui.end_row();
                }
            });
        });
        send
    }
}
