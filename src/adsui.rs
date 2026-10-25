//! Advertisement scheduler: one list of advert / jingle rules (every N songs, every N minutes or
//! hours, at set times — each limited to dates, days and hours) with an editor beside it, and the
//! Adverts list (each advert with its own run dates).
use super::*;
use crate::config::{Ad, AdRule, RotPick, Trigger};

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const ADVERT: Color32 = Color32::from_rgb(214, 120, 31);
const JINGLE: Color32 = Color32::from_rgb(126, 87, 194);

#[derive(Default, PartialEq, Clone, Copy)]
pub(super) enum AdsView {
    #[default]
    Rules,
    Adverts,
}

#[derive(Default)]
pub(super) struct AdsUi {
    pub view: AdsView,
    /// rule being edited (by id)
    pub sel: Option<u64>,
    /// time to add to a "set times" rule
    pub new_time: (u32, u32),
    /// minutes or hours for "every N"
    pub hours_unit: bool,
}

/// Toggle buttons for Monday … Sunday, plus "every day" / "weekdays" shortcuts.
pub(super) fn days_edit(ui: &mut egui::Ui, days: &mut [bool; 7]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (d, name) in days.iter_mut().zip(DAYS) {
            ui.toggle_value(d, tr(name));
        }
        ui.add_space(6.0);
        if ui.small_button(tr("Every day")).clicked() {
            *days = [true; 7];
        }
        if ui.small_button(tr("Weekdays")).clicked() {
            *days = [true, true, true, true, true, false, false];
        }
        if ui.small_button(tr("Weekends")).clicked() {
            *days = [false, false, false, false, false, true, true];
        }
    });
}

fn kind_tag(ui: &mut egui::Ui, adverts: bool) {
    let (t, c) = if adverts { (tr("ADVERT"), ADVERT) } else { (tr("JINGLE"), JINGLE) };
    egui::Frame::new().fill(c).corner_radius(3.0).inner_margin(egui::Margin::symmetric(5, 1)).show(ui, |ui| {
        ui.label(RichText::new(t).small().strong().color(Color32::WHITE));
    });
}

impl App {
    pub(super) fn tab_ads(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ads.view, AdsView::Rules, RichText::new(tr("📋 Rules")).strong());
            ui.selectable_value(
                &mut self.ads.view,
                AdsView::Adverts,
                RichText::new(format!("📢 {} ({})", tr("Adverts list"), core.cfg.ads.len())).strong(),
            );
            ui.label(
                RichText::new(match self.ads.view {
                    AdsView::Rules => tr("What plays, how often, and when. Adverts and jingles never count as songs."),
                    AdsView::Adverts => tr("Your adverts, each with the dates and days it may run. Rules can play from this list."),
                })
                .small()
                .color(GRAY),
            );
        });
        ui.separator();
        match self.ads.view {
            AdsView::Rules => self.ads_rules(ui, core),
            AdsView::Adverts => self.ads_list(ui, core),
        }
    }

    fn ads_rules(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if self.ads.sel.is_none_or(|id| !core.cfg.ad_rules.iter().any(|r| r.id == id)) {
            self.ads.sel = core.cfg.ad_rules.first().map(|r| r.id);
        }
        // ---- the rules
        let mut add: Option<bool> = None;
        let mut act: Option<(&str, usize)> = None;
        egui::SidePanel::left("ads_rule_list").resizable(true).default_width(330.0).width_range(240.0..=520.0).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button(tr("➕ Advert rule")).clicked() {
                    add = Some(true);
                }
                if ui.button(tr("➕ Jingle rule")).clicked() {
                    add = Some(false);
                }
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().id_salt("ads_rules_scroll").auto_shrink([false, false]).show(ui, |ui| {
                let now = Local::now();
                for (i, r) in core.cfg.ad_rules.iter_mut().enumerate() {
                    let selected = self.ads.sel == Some(r.id);
                    let fill = if selected { ui.visuals().selection.bg_fill } else { ui.visuals().faint_bg_color };
                    let resp = egui::Frame::group(ui.style())
                        .fill(fill)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut r.enabled, "").on_hover_text(tr("On / off"));
                                kind_tag(ui, r.adverts);
                                ui.add(egui::Label::new(RichText::new(&r.name).strong()).truncate());
                            });
                            let live = r.in_force(&now);
                            let state = if !r.enabled {
                                RichText::new(tr("off")).small().color(GRAY)
                            } else if live {
                                RichText::new(tr("● in force now")).small().color(GREEN)
                            } else {
                                RichText::new(tr("○ waiting (dates, days or hours)")).small().color(AMBER)
                            };
                            ui.label(RichText::new(r.summary()).small());
                            ui.label(state);
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        self.ads.sel = Some(r.id);
                    }
                    resp.context_menu(|ui| {
                        for (l, k) in [
                            ("▶ Play it now (after this song)", "fire"),
                            ("Duplicate", "dup"),
                            ("⬆ Move up", "up"),
                            ("⬇ Move down", "down"),
                        ] {
                            if ui.button(tr(l)).clicked() {
                                act = Some((k, i));
                                ui.close_menu();
                            }
                        }
                    });
                }
                if core.cfg.ad_rules.is_empty() {
                    ui.label(RichText::new(tr("No rules yet. Add an advert or jingle rule above.")).color(GRAY));
                }
            });
        });
        if let Some(adverts) = add {
            let id = core.cfg.new_rule_id();
            core.cfg.ad_rules.push(AdRule {
                id,
                name: if adverts { "Adverts".into() } else { "Jingles".into() },
                enabled: true,
                adverts,
                every_songs: if adverts { 5 } else { 3 },
                count: if adverts { 2 } else { 1 },
                use_ad_list: adverts && !core.cfg.ads.is_empty(),
                ..Default::default()
            });
            self.ads.sel = Some(id);
        }
        if let Some((what, i)) = act {
            self.rule_action(core, what, i);
        }

        // ---- the selected rule
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let Some(i) = self.ads.sel.and_then(|id| core.cfg.ad_rules.iter().position(|r| r.id == id)) else {
                ui.label(RichText::new(tr("Pick a rule on the left, or add one.")).color(GRAY));
                return;
            };
            egui::ScrollArea::vertical().id_salt("ads_rule_edit").auto_shrink([false, false]).show(ui, |ui| self.rule_editor(ui, core, i));
        });
    }

    fn rule_action(&mut self, core: &mut Playout, what: &str, i: usize) {
        let n = core.cfg.ad_rules.len();
        if i >= n {
            return;
        }
        match what {
            "fire" => {
                let id = core.cfg.ad_rules[i].id;
                core.fire_rule(id, None);
            }
            "dup" => {
                let mut r = core.cfg.ad_rules[i].clone();
                r.id = core.cfg.new_rule_id();
                r.name = format!("{} (copy)", r.name);
                self.ads.sel = Some(r.id);
                core.cfg.ad_rules.insert(i + 1, r);
            }
            "up" if i > 0 => core.cfg.ad_rules.swap(i, i - 1),
            "down" if i + 1 < n => core.cfg.ad_rules.swap(i, i + 1),
            "del" => {
                core.cfg.ad_rules.remove(i);
                self.ads.sel = None;
            }
            _ => {}
        }
        core.save_all();
    }

    fn rule_editor(&mut self, ui: &mut egui::Ui, core: &mut Playout, i: usize) {
        let ads_n = core.cfg.ads.len();
        let r = &mut core.cfg.ad_rules[i];
        let mut act: Option<&str> = None;
        ui.horizontal(|ui| {
            ui.checkbox(&mut r.enabled, "");
            ui.add(egui::TextEdit::singleline(&mut r.name).font(egui::TextStyle::Heading).desired_width(220.0));
            ui.selectable_value(&mut r.adverts, true, tr("Adverts"));
            ui.selectable_value(&mut r.adverts, false, tr("Jingles"));
        });
        ui.label(RichText::new(r.summary()).color(GRAY));
        ui.horizontal(|ui| {
            if ui.button(tr("▶ Play it now")).on_hover_text(tr("Queue them after the song on air")).clicked() {
                act = Some("fire");
            }
            if ui.button(tr("Duplicate")).clicked() {
                act = Some("dup");
            }
            if delete_button(ui, ("rule", r.id), tr("this rule")) {
                act = Some("del");
            }
        });
        ui.add_space(6.0);

        // ---- 1. what plays
        section(ui, "1", tr("What plays"));
        egui::Grid::new("rule_what").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tr("From"));
            ui.vertical(|ui| {
                if r.adverts {
                    ui.radio_value(
                        &mut r.use_ad_list,
                        true,
                        format!("{} ({} {})", tr("the Adverts list — each advert only in its own dates and days"), ads_n, tr("adverts")),
                    );
                    ui.radio_value(&mut r.use_ad_list, false, tr("a folder or chosen files"));
                }
                if !(r.adverts && r.use_ad_list) {
                    if r.files.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut r.folder).desired_width(280.0).hint_text(tr("folder")));
                            if ui.button("…").on_hover_text(tr("Choose a folder")).clicked() {
                                self.pick(Pick::Folder, tr("Choose a folder"), move |_, v| Some(Dlg::RotFolder(i, v.into_iter().next()?)));
                            }
                        });
                    }
                    let mut fdel = None;
                    let mut up = None;
                    for (k, f) in r.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("✖").on_hover_text(tr("Remove")).clicked() {
                                fdel = Some(k);
                            }
                            if k > 0 && ui.small_button("⬆").on_hover_text(tr("Play earlier (in turn)")).clicked() {
                                up = Some(k);
                            }
                            let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let mark = if r.pick == RotPick::InOrder && k == r.next % r.files.len().max(1) { "▶ " } else { "" };
                            ui.label(format!("{mark}{name}")).on_hover_text(f);
                        });
                    }
                    if let Some(k) = fdel {
                        r.files.remove(k);
                    }
                    if let Some(k) = up {
                        r.files.swap(k, k - 1);
                    }
                    ui.horizontal(|ui| {
                        if ui.button(if r.files.is_empty() { tr("…or choose files") } else { tr("➕ Add files") }).clicked() {
                            self.pick(Pick::Files(AUDIO.0, AUDIO.1), tr("Choose files"), move |_, v| Some(Dlg::RotFiles(i, v)));
                        }
                        if !r.files.is_empty() {
                            ui.label(RichText::new(tr("(chosen files are used instead of the folder)")).small().color(GRAY));
                        }
                    });
                }
            });
            ui.end_row();
            ui.label(tr("Play"));
            ui.horizontal(|ui| {
                if r.pick != RotPick::All {
                    ui.add(egui::DragValue::new(&mut r.count).range(1..=20));
                }
                ui.radio_value(&mut r.pick, RotPick::Random, tr("random"));
                ui.radio_value(&mut r.pick, RotPick::InOrder, tr("taking turns"));
                ui.radio_value(&mut r.pick, RotPick::All, tr("all of them"));
            });
            ui.end_row();
        });
        ui.add_space(8.0);

        // ---- 2. how often
        section(ui, "2", tr("How often"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut r.trigger, Trigger::Songs, tr("After every"));
            ui.add_enabled(r.trigger == Trigger::Songs, egui::DragValue::new(&mut r.every_songs).range(1..=100));
            ui.label(tr("songs"));
        });
        ui.horizontal(|ui| {
            ui.radio_value(&mut r.trigger, Trigger::Minutes, tr("Every"));
            ui.add_enabled_ui(r.trigger == Trigger::Minutes, |ui| {
                let hours = r.every_mins >= 60 && r.every_mins.is_multiple_of(60) || self.ads.hours_unit;
                if hours {
                    let mut h = (r.every_mins / 60).max(1);
                    if ui.add(egui::DragValue::new(&mut h).range(1..=24)).changed() {
                        r.every_mins = h * 60;
                    }
                } else {
                    ui.add(egui::DragValue::new(&mut r.every_mins).range(1..=1440));
                }
                let mut unit_hours = hours;
                if ui.selectable_value(&mut unit_hours, false, tr("minutes")).clicked() {
                    self.ads.hours_unit = false;
                    r.every_mins = r.every_mins.clamp(1, 59);
                }
                if ui.selectable_value(&mut unit_hours, true, tr("hours")).clicked() {
                    self.ads.hours_unit = true;
                    r.every_mins = (r.every_mins / 60).max(1) * 60;
                }
                ui.checkbox(&mut r.on_clock, tr("on the clock"))
                    .on_hover_text(tr("Every 30 minutes on the clock = at :00 and :30. Off = 30 minutes after the last time."));
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut r.trigger, Trigger::Times, tr("At"));
            ui.add_enabled_ui(r.trigger == Trigger::Times, |ui| {
                let mut tdel = None;
                for (k, (h, m)) in r.times.iter().enumerate() {
                    if ui.button(format!("{h:02}:{m:02}  ✖")).on_hover_text(tr("Remove this time")).clicked() {
                        tdel = Some(k);
                    }
                }
                if let Some(k) = tdel {
                    r.times.remove(k);
                }
                let (h, m) = &mut self.ads.new_time;
                ui.add(egui::DragValue::new(h).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as u32)));
                ui.label(":");
                ui.add(egui::DragValue::new(m).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as u32)));
                if ui.button(tr("➕ Add time")).clicked() && !r.times.contains(&(*h, *m)) {
                    r.times.push((*h, *m));
                    r.times.sort();
                }
            });
        });
        ui.label(
            RichText::new(match r.trigger {
                Trigger::Songs => tr("Added straight after the song that starts."),
                _ => tr("Queued after the song on air when the time comes (the song isn't cut)."),
            })
            .small()
            .color(GRAY),
        );
        ui.add_space(8.0);

        // ---- 3. when the rule is in force
        section(ui, "3", tr("When it's in force"));
        egui::Grid::new("rule_when").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tr("Dates"));
            ui.horizontal(|ui| {
                ui.label(tr("from"));
                date_edit(ui, ("rule_from", r.id), &mut r.from_date, true);
                ui.label(tr("to"));
                date_edit(ui, ("rule_to", r.id), &mut r.to_date, true);
            });
            ui.end_row();
            ui.label(tr("Days"));
            days_edit(ui, &mut r.days);
            ui.end_row();
            ui.label(tr("Hours"));
            ui.horizontal(|ui| {
                let mut all_day = r.from_hour == r.to_hour;
                if ui.checkbox(&mut all_day, tr("all day")).changed() {
                    (r.from_hour, r.to_hour) = if all_day { (0, 0) } else { (6, 22) };
                }
                if !all_day {
                    ui.label(tr("from"));
                    ui.add(egui::DragValue::new(&mut r.from_hour).range(0..=23).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                    ui.label(tr("until"));
                    ui.add(egui::DragValue::new(&mut r.to_hour).range(0..=24).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                    if r.to_hour == 24 {
                        r.to_hour = 0;
                    }
                }
            });
            ui.end_row();
        });
        if let Some(a) = act {
            self.rule_action(core, a, i);
        }
    }

    fn ads_list(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        ui.horizontal(|ui| {
            if ui.button(tr("➕ Add advert")).clicked() {
                core.cfg.ads.push(Ad::default());
            }
            let users: Vec<String> = core.cfg.ad_rules.iter().filter(|r| r.adverts && r.use_ad_list).map(|r| r.name.clone()).collect();
            ui.label(
                RichText::new(if users.is_empty() {
                    tr("No rule plays from this list yet — in a rule, choose \"From: the Adverts list\".").to_string()
                } else {
                    format!("{} {}", tr("Played by:"), users.join(", "))
                })
                .small()
                .color(GRAY),
            );
        });
        ui.add_space(4.0);
        let now = Local::now();
        let mut del = None;
        egui::ScrollArea::vertical().id_salt("ads_list_scroll").auto_shrink([false, false]).show(ui, |ui| {
            for i in 0..core.cfg.ads.len() {
                let a: &mut Ad = &mut core.cfg.ads[i];
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut a.enabled, "");
                        ui.add(egui::TextEdit::singleline(&mut a.name).desired_width(170.0).font(egui::TextStyle::Body));
                        let live = a.active(&now);
                        ui.label(if !a.enabled {
                            RichText::new(tr("off")).small().color(GRAY)
                        } else if live {
                            RichText::new(tr("● running today")).small().color(GREEN)
                        } else {
                            RichText::new(tr("○ not today")).small().color(AMBER)
                        });
                        ui.add(egui::TextEdit::singleline(&mut a.path).desired_width(260.0).hint_text(tr("audio file")));
                        if ui.button("…").on_hover_text(tr("Pick the audio file")).clicked() {
                            self.pick(Pick::File(AUDIO.0, AUDIO.1), tr("Pick the audio file"), move |_, v| {
                                Some(Dlg::AdPath(i, v.into_iter().next()?))
                            });
                        }
                        if delete_button(ui, ("advert", i), tr("this advert")) {
                            del = Some(i);
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Runs from"));
                        date_edit(ui, ("ad_from", i), &mut a.start_date, true);
                        ui.label(tr("to"));
                        date_edit(ui, ("ad_to", i), &mut a.end_date, true);
                        ui.add_space(10.0);
                        days_edit(ui, &mut a.days);
                    });
                });
            }
            if core.cfg.ads.is_empty() {
                ui.label(RichText::new(tr("No adverts yet. Add one, pick its audio file and the dates it should run.")).color(GRAY));
            }
        });
        if let Some(i) = del {
            core.cfg.ads.remove(i);
        }
    }
}

/// Numbered heading for a part of an editor.
pub(super) fn section(ui: &mut egui::Ui, n: &str, title: &str) {
    ui.horizontal(|ui| {
        egui::Frame::new().fill(ui.visuals().selection.bg_fill).corner_radius(9.0).inner_margin(egui::Margin::symmetric(6, 0)).show(
            ui,
            |ui| {
                ui.label(RichText::new(n).strong());
            },
        );
        ui.label(RichText::new(title).strong().size(15.0));
    });
    ui.add_space(2.0);
}
