//! Playlist Generator: categories, templates (pattern, time-of-day patterns, length, separation) and a preview of the running order before it goes in.
use super::adsui::section;
use super::*;
use crate::config::{Category, Daypart, GenLength, PatternItem, Template};

#[derive(Default)]
pub(super) struct GenUi {
    pub preview: Option<GenPreview>,
    /// bring the preview into view on the next frame
    pub show_preview: bool,
}

/// A running order made by "Preview" that can go into the playlist as it is.
pub(super) struct GenPreview {
    pub template: String,
    pub append: bool,
    pub rows: Vec<PlEntry>,
    /// (start time, length) of each row
    pub times: Vec<(chrono::DateTime<Local>, f32)>,
}

fn hhmm(ui: &mut egui::Ui, h: &mut u32, m: &mut u32) {
    ui.add(egui::DragValue::new(h).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
    ui.label(":");
    ui.add(egui::DragValue::new(m).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
}

/// The steps of a pattern; returns nothing, edits in place.
fn pattern_edit(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + Copy,
    pat: &mut Vec<PatternItem>,
    cats: &[String],
    rules: &[(u64, String, bool)],
) {
    let mut pdel = None;
    for (k, p) in pat.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}.", k + 1));
            ui.add(egui::DragValue::new(&mut p.count).range(1..=20).suffix(" ×"));
            source_combo(ui, ("pat", id, k), &mut p.category, cats, rules);
            if ui.small_button("✖").on_hover_text(tr("Remove this step")).clicked() {
                pdel = Some(k);
            }
        });
    }
    if let Some(k) = pdel {
        pat.remove(k);
    }
    if ui.button(tr("➕ Add pattern step")).clicked() {
        pat.push(PatternItem { category: cats.first().cloned().unwrap_or_else(|| "Music".into()), count: 1 });
    }
}

impl App {
    pub(super) fn tab_generator(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let cat_names: Vec<String> = core.cfg.categories.iter().map(|c| c.name.clone()).collect();
        let tpl_names: Vec<String> = core.cfg.templates.iter().map(|t| t.name.clone()).collect();
        let rules: Vec<(u64, String, bool)> = core.cfg.ad_rules.iter().map(|r| (r.id, r.name.clone(), r.adverts)).collect();
        let mut gen: Option<(String, bool)> = None;
        let mut preview: Option<(String, bool)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.strong(tr("Auto-playlist"));
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut core.cfg.auto_enabled, tr("Keep the playlist filled automatically"));
                ui.label(tr("using"));
                name_combo(ui, "autotpl", &mut core.cfg.auto_template, &tpl_names);
                ui.label(tr("when under"));
                ui.add(egui::DragValue::new(&mut core.cfg.auto_min_mins).range(1..=600).suffix(" min left"));
            });
            ui.label(
                RichText::new(tr("The template's times of day follow the clock from where the new tracks start."))
                    .small()
                    .color(GRAY),
            );
            ui.separator();

            // ---- categories
            egui::CollapsingHeader::new(RichText::new(tr("Categories  (what each pool of tracks is made from)")).strong())
                .id_salt("gen_cats")
                .default_open(true)
                .show(ui, |ui| {
                    let mut del = None;
                    for i in 0..core.cfg.categories.len() {
                        let c = &mut core.cfg.categories[i];
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(110.0));
                                ui.label(tr("folder"));
                                ui.add(egui::TextEdit::singleline(&mut c.folder).desired_width(240.0).hint_text(tr("any")));
                                if ui.button("…").on_hover_text(tr("Browse")).clicked() {
                                    self.pick(Pick::Folder, tr("Category folder"), move |_, v| Some(Dlg::CatFolder(i, v.into_iter().next()?)));
                                }
                                if delete_button(ui, ("category", i), tr("this category")) {
                                    del = Some(i);
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(tr("genre"));
                                ui.add(egui::TextEdit::singleline(&mut c.genre).desired_width(120.0).hint_text(tr("any")))
                                    .on_hover_text(tr("Several genres: separate them with commas (Rock, Pop)"));
                                ui.label(tr("length"));
                                ui.add(egui::DragValue::new(&mut c.min_secs).range(0.0..=3600.0).suffix("s"));
                                ui.label("–");
                                ui.add(egui::DragValue::new(&mut c.max_secs).range(0.0..=7200.0).suffix("s"));
                                ui.label(tr("years"));
                                ui.add(egui::DragValue::new(&mut c.min_year).range(0..=2100).custom_formatter(|v, _| {
                                    if v == 0.0 {
                                        "any".into()
                                    } else {
                                        format!("{v}")
                                    }
                                }));
                                ui.label("–");
                                ui.add(egui::DragValue::new(&mut c.max_year).range(0..=2100).custom_formatter(|v, _| {
                                    if v == 0.0 {
                                        "any".into()
                                    } else {
                                        format!("{v}")
                                    }
                                }));
                                ui.label(tr("leave out"));
                                ui.add(egui::TextEdit::singleline(&mut c.exclude).desired_width(120.0).hint_text(tr("words, e.g. live, remix")))
                                    .on_hover_text(tr("Tracks whose artist, title, genre or file name has one of these words are left out"));
                            });
                        });
                    }
                    if let Some(i) = del {
                        core.cfg.categories.remove(i);
                    }
                    if ui.button(tr("➕ Add category")).clicked() {
                        core.cfg.categories.push(Category { name: format!("Category {}", core.cfg.categories.len() + 1), ..Default::default() });
                    }
                });
            ui.separator();

            // ---- templates
            ui.strong(tr("Templates"));
            ui.horizontal_wrapped(|ui| {
                for (i, n) in tpl_names.iter().enumerate() {
                    ui.selectable_value(&mut self.tpl_sel, i, n);
                }
                if ui.button(tr("➕ New")).clicked() {
                    core.cfg.templates.push(Template { name: format!("Template {}", core.cfg.templates.len() + 1), ..Default::default() });
                    self.tpl_sel = core.cfg.templates.len() - 1;
                }
                if let Some(t) = core.cfg.templates.get(self.tpl_sel) {
                    if ui.button(tr("Duplicate")).on_hover_text(tr("A copy of this template to change")).clicked() {
                        let mut c = t.clone();
                        c.name = format!("{} (copy)", t.name);
                        core.cfg.templates.push(c);
                        self.tpl_sel = core.cfg.templates.len() - 1;
                    }
                }
            });
            if self.tpl_sel >= core.cfg.templates.len() {
                self.tpl_sel = core.cfg.templates.len().saturating_sub(1);
            }
            let mut del_tpl = false;
            let Some(t) = core.cfg.templates.get_mut(self.tpl_sel) else { return };
            ui.add_space(4.0);

            section(ui, "1", tr("Length and separation"));
            egui::Grid::new("gen_basics").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(tr("Name"));
                ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(180.0));
                ui.end_row();
                ui.label(tr("Length"));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut t.length, GenLength::Minutes, "");
                        ui.add_enabled(t.length == GenLength::Minutes, egui::DragValue::new(&mut t.minutes).range(5..=1440).suffix(" min"));
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut t.length, GenLength::Until, tr("until"));
                        ui.add_enabled_ui(t.length == GenLength::Until, |ui| hhmm(ui, &mut t.until.0, &mut t.until.1));
                        ui.label(RichText::new(tr("the next time the clock reaches it")).small().color(GRAY));
                    });
                });
                ui.end_row();
                ui.label(tr("Artist separation"));
                ui.add(egui::DragValue::new(&mut t.artist_gap).range(0..=50).suffix(" tracks"));
                ui.end_row();
                ui.label(tr("Song separation"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut t.song_gap_mins).range(0..=1440).custom_formatter(|v, _| {
                        if v == 0.0 {
                            "off".into()
                        } else {
                            format!("{v} min")
                        }
                    }));
                    ui.label(RichText::new(tr("the same song doesn't come back sooner")).small().color(GRAY));
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut t.hour_markers, tr("Hour marker at the top of every hour"));
                ui.end_row();
            });
            ui.add_space(8.0);

            section(ui, "2", tr("Pattern"));
            ui.label(RichText::new(tr("Repeats until the length is reached:")).color(GRAY));
            pattern_edit(ui, "main", &mut t.pattern, &cat_names, &rules);
            ui.add_space(8.0);

            section(ui, "3", tr("Times of day"));
            ui.label(
                RichText::new(tr("A different pattern for part of the day. The first one that covers the time is used; outside them the pattern above is."))
                    .color(GRAY),
            );
            let mut ddel = None;
            for (k, d) in t.dayparts.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(120.0));
                        ui.label(tr("from"));
                        ui.add(egui::DragValue::new(&mut d.from_hour).range(0..=23).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                        ui.label(tr("to"));
                        ui.add(egui::DragValue::new(&mut d.to_hour).range(0..=23).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                        if d.from_hour == d.to_hour {
                            ui.label(RichText::new(tr("(all day)")).small().color(GRAY));
                        }
                        if delete_button(ui, ("daypart", self.tpl_sel, k), tr("this time of day")) {
                            ddel = Some(k);
                        }
                    });
                    adsui::days_edit(ui, &mut d.days);
                    pattern_edit(ui, ("dp", k), &mut d.pattern, &cat_names, &rules);
                });
            }
            if let Some(k) = ddel {
                t.dayparts.remove(k);
            }
            if ui.button(tr("➕ Add time of day")).clicked() {
                t.dayparts.push(Daypart { pattern: t.pattern.clone(), ..Default::default() });
            }
            ui.add_space(8.0);

            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("👁 Preview")).on_hover_text(tr("See the running order first; nothing changes until you use it")).clicked() {
                    preview = Some((t.name.clone(), false));
                }
                if ui.button(RichText::new(tr("▶ Generate (replace playlist)")).color(GREEN)).clicked() {
                    gen = Some((t.name.clone(), false));
                }
                if ui.button(tr("Append to playlist")).clicked() {
                    gen = Some((t.name.clone(), true));
                }
                if delete_button(ui, ("template", self.tpl_sel), tr("this template")) {
                    del_tpl = true;
                }
            });
            if del_tpl {
                core.cfg.templates.remove(self.tpl_sel);
            }
            self.gen_preview_ui(ui, core);
        });
        if let Some((n, append)) = preview {
            self.make_preview(core, &n, append);
        }
        if let Some((n, append)) = gen {
            self.gen.preview = None;
            let c = core.generate_playlist(&n, append);
            if c > 0 {
                core.say(format!("Generated {c} tracks from '{n}'"));
            }
        }
    }

    fn make_preview(&mut self, core: &mut Playout, name: &str, append: bool) {
        let Some(rows) = core.generated_rows(name, append) else {
            self.gen.preview = None;
            return;
        };
        let mut at = core.generate_start(append);
        let mut times = vec![];
        for r in &rows {
            if let crate::items::ItemKind::HourMarker { hour, minute, .. } = r.kind {
                at = crate::playout::marker_target(hour, minute, at).max(at);
            }
            let span = core.row_span(r);
            times.push((at, span));
            at += chrono::Duration::milliseconds((span * 1000.0) as i64);
        }
        self.gen.preview = Some(GenPreview { template: name.to_string(), append, rows, times });
        self.gen.show_preview = true;
    }

    fn gen_preview_ui(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let Some(p) = &self.gen.preview else { return };
        let mut close = false;
        let mut use_it = false;
        let mut again: Option<bool> = None;
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let songs = p.rows.iter().filter(|r| r.is_file()).count();
            let total: f32 = p.times.iter().map(|t| t.1).sum();
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} — {}", tr("Preview"), p.template));
                ui.label(RichText::new(format!("{songs} {} · {}", tr("tracks"), crate::items::mmss(total))).color(GRAY));
            });
            let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
            let out = egui::ScrollArea::vertical()
                .id_salt("gen_preview")
                .max_height(280.0)
                .min_scrolled_height(240.0)
                .auto_shrink([false, true])
                .show_rows(ui, row_h, p.rows.len(), |ui, range| {
                    for k in range {
                        let (r, (at, _)) = (&p.rows[k], &p.times[k]);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(at.format("%H:%M:%S").to_string()).monospace());
                            ui.label(RichText::new(format!("{:>8}", r.length_text())).monospace().color(GRAY));
                            ui.add_sized([70.0, row_h], egui::Label::new(RichText::new(tr(r.type_label())).small().color(GRAY)));
                            ui.label(if r.is_file() { r.track.label() } else { r.title() });
                        });
                    }
                });
            if self.gen.show_preview {
                ui.scroll_to_rect(out.inner_rect, Some(egui::Align::Center));
            }
            ui.horizontal_wrapped(|ui| {
                if core.link.is_some() {
                    ui.label(RichText::new(tr("Connected to a server: Generate makes a new running order there.")).small().color(AMBER));
                } else if ui.button(RichText::new(tr("✔ Put it in the playlist")).color(GREEN)).clicked() {
                    use_it = true;
                }
                let mut append = p.append;
                if ui.checkbox(&mut append, tr("after the playlist (instead of replacing it)")).changed() {
                    again = Some(append);
                }
                if ui.button(tr("⟳ Again")).on_hover_text(tr("Another running order from the same template")).clicked() {
                    again = Some(p.append);
                }
                if ui.button(tr("Close")).clicked() {
                    close = true;
                }
            });
        });
        self.gen.show_preview = false;
        if let Some(a) = again {
            let n = p.template.clone();
            self.make_preview(core, &n, a);
        } else if use_it {
            let Some(p) = self.gen.preview.take() else { return };
            let n = p.rows.iter().filter(|r| r.is_file()).count();
            core.use_generated(p.rows, p.append);
            core.say(format!("Generated {n} tracks from '{}'", p.template));
        } else if close {
            self.gen.preview = None;
        }
    }
}
