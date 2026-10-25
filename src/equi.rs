//! The equalizer in Audio processing: the classic 12-band graphic EQ, or a parametric EQ with
//! free bands, over a response curve. Each mode keeps its own settings and presets.
use super::*;
use crate::dsp::{default_peq_bands, eq_response, peq_presets, EqMode, FilterKind, PeqBand, EQ_FREQS, PEQ_MAX_BANDS};

/// Built-in graphic EQ curves (31 Hz … 16 kHz, dB).
const EQ_PRESETS: &[(&str, [f32; 12])] = &[
    ("Flat", [0.0; 12]),
    ("Broadcast (FM warmth)", [2.0, 2.5, 1.5, 0.0, -1.0, -0.5, 0.5, 1.5, 2.0, 2.5, 2.0, 1.0]),
    ("Bass boost", [5.0, 4.5, 3.5, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass cut", [-6.0, -4.5, -3.0, -1.5, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Voice clarity", [-4.0, -3.0, -1.5, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 0.5, 0.0, -1.0]),
    ("Loudness", [4.0, 3.0, 1.5, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0, 3.5, 3.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.5, 4.5]),
    ("Treble cut", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -3.0, -4.5, -5.5, -6.0]),
    ("Dance", [4.5, 4.0, 2.0, 0.0, -1.0, -1.5, 0.0, 1.0, 2.0, 3.0, 2.5, 1.5]),
    ("Electronic", [4.0, 3.5, 1.0, 0.0, -1.5, 0.5, 0.0, 1.0, 2.5, 3.5, 4.0, 3.5]),
    ("Hip-hop", [5.0, 4.5, 2.5, 1.0, -0.5, -1.0, 0.5, 0.0, 1.0, 1.5, 2.0, 2.0]),
    ("R&B", [3.5, 3.0, 2.0, 0.5, -1.0, -0.5, 1.0, 1.5, 1.5, 2.0, 2.5, 2.5]),
    ("Pop", [-1.0, 0.0, 1.5, 2.5, 3.0, 2.0, 0.5, -0.5, -0.5, 0.0, 0.5, 1.0]),
    ("Rock", [3.0, 2.5, 1.0, -0.5, -1.0, 0.0, 1.5, 2.5, 3.0, 3.0, 2.5, 2.0]),
    ("Country", [1.0, 1.5, 1.0, 0.0, -0.5, 0.5, 2.0, 2.5, 2.5, 2.0, 1.5, 1.0]),
    ("Jazz", [2.5, 2.0, 1.0, 1.5, -1.0, -1.0, 0.0, 1.0, 2.0, 2.5, 2.5, 2.0]),
    ("Classical", [3.0, 2.5, 1.5, 1.0, 0.0, 0.0, 0.0, -0.5, 0.0, 1.5, 2.5, 3.0]),
    ("Acoustic", [2.5, 2.0, 1.5, 0.5, 1.0, 1.0, 1.5, 2.0, 2.0, 1.5, 1.0, 0.5]),
    ("Oldies / AM", [-6.0, -4.0, -1.0, 1.0, 2.0, 2.5, 2.0, 1.0, -1.0, -4.0, -8.0, -10.0]),
    ("Talk / podcast", [-6.0, -4.0, -2.0, 0.0, 1.0, 2.0, 2.5, 2.0, 1.0, 0.0, -1.0, -2.0]),
    ("Vocal presence", [-2.0, -1.5, -1.0, 0.0, 0.5, 1.5, 3.0, 3.5, 3.0, 1.0, 0.0, 0.0]),
    ("Late night (soft)", [1.5, 1.0, 0.5, 0.0, 0.0, -0.5, -1.0, -2.0, -2.5, -3.0, -3.5, -4.0]),
    ("Small speakers", [-3.0, -1.0, 2.0, 3.0, 2.0, 1.0, 0.0, 0.5, 1.0, 1.5, 1.0, 0.0]),
    ("Headphones", [2.0, 1.5, 0.5, 0.0, -0.5, 0.0, 0.5, -1.0, -1.5, 0.5, 1.5, 1.0]),
];

/// The curve's range: 20 Hz … 20 kHz, ±18 dB.
const F_LO: f32 = 20.0;
const F_HI: f32 = 20000.0;
const DB_RANGE: f32 = 18.0;

fn hz(f: f32) -> String {
    if f >= 1000.0 {
        format!("{}k", (f / 100.0).round() / 10.0)
    } else {
        format!("{}", f.round())
    }
}

impl App {
    pub(super) fn eq_ui(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        let mut save_preset = false;
        let mut delete_preset: Option<usize> = None;
        {
            let graphic_custom = core.cfg.eq_presets.clone();
            let peq_custom = core.cfg.peq_presets.clone();
            let d = &mut core.cfg.dsp;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut d.eq_enabled, tr("Equalizer"));
                ui.selectable_value(&mut d.eq_mode, EqMode::Graphic, tr("12-band"))
                    .on_hover_text(tr("Twelve sliders at fixed frequencies"));
                ui.selectable_value(&mut d.eq_mode, EqMode::Parametric, tr("Parametric"))
                    .on_hover_text(tr("Up to 16 bands; each picks its filter, frequency, gain and width"));
                ui.separator();
                egui::ComboBox::from_id_salt("eq_preset").selected_text(tr("Presets…")).width(150.0).height(400.0).show_ui(ui, |ui| {
                    match d.eq_mode {
                        EqMode::Graphic => {
                            for (name, g) in EQ_PRESETS {
                                if ui.selectable_label(false, *name).clicked() {
                                    d.eq_gains = *g;
                                    d.eq_enabled = true;
                                }
                            }
                        }
                        EqMode::Parametric => {
                            for (name, bands) in peq_presets() {
                                if ui.selectable_label(false, name).clicked() {
                                    d.peq_bands = bands;
                                    d.eq_enabled = true;
                                }
                            }
                        }
                    }
                    let names: Vec<&String> = match d.eq_mode {
                        EqMode::Graphic => graphic_custom.iter().map(|(n, _)| n).collect(),
                        EqMode::Parametric => peq_custom.iter().map(|(n, _)| n).collect(),
                    };
                    if !names.is_empty() {
                        ui.separator();
                    }
                    for (k, name) in names.into_iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(false, name).clicked() {
                                match d.eq_mode {
                                    EqMode::Graphic => d.eq_gains = graphic_custom[k].1,
                                    EqMode::Parametric => d.peq_bands = peq_custom[k].1.clone(),
                                }
                                d.eq_enabled = true;
                            }
                            if ui.small_button("🗑").on_hover_text(tr("Delete this preset")).clicked() {
                                delete_preset = Some(k);
                            }
                        });
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.eq_name).hint_text(tr("preset name")).desired_width(110.0));
                save_preset = ui.add_enabled(!self.eq_name.trim().is_empty(), egui::Button::new(tr("Save preset"))).clicked();
                if ui.button(tr("Flat")).clicked() {
                    match d.eq_mode {
                        EqMode::Graphic => d.eq_gains = [0.0; 12],
                        EqMode::Parametric => d.peq_bands = default_peq_bands(),
                    }
                    d.eq_preamp_db = 0.0;
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("Preamp"));
                ui.add(egui::Slider::new(&mut d.eq_preamp_db, -12.0..=12.0).suffix(" dB").fixed_decimals(1))
                    .on_hover_text(tr("Turn the level down before boosting, so loud tracks don't clip"));
            });
            self.eq_curve(ui, d);
            match d.eq_mode {
                EqMode::Graphic => graphic_sliders(ui, d),
                EqMode::Parametric => self.peq_table(ui, d),
            }
        }
        let mode = core.cfg.dsp.eq_mode;
        if save_preset {
            let name = self.eq_name.trim().to_string();
            match mode {
                EqMode::Graphic => {
                    let g = core.cfg.dsp.eq_gains;
                    core.cfg.eq_presets.retain(|(n, _)| *n != name);
                    core.cfg.eq_presets.push((name.clone(), g));
                }
                EqMode::Parametric => {
                    let b = core.cfg.dsp.peq_bands.clone();
                    core.cfg.peq_presets.retain(|(n, _)| *n != name);
                    core.cfg.peq_presets.push((name.clone(), b));
                }
            }
            core.say(format!("EQ preset \"{name}\" saved"));
            self.eq_name.clear();
        }
        if let Some(k) = delete_preset {
            match mode {
                EqMode::Graphic => {
                    core.cfg.eq_presets.remove(k);
                }
                EqMode::Parametric => {
                    core.cfg.peq_presets.remove(k);
                }
            }
        }
    }

    /// The response curve. In parametric mode each band has a handle: drag it to change the
    /// frequency and gain, scroll over it to change the width.
    fn eq_curve(&mut self, ui: &mut egui::Ui, d: &mut crate::dsp::DspConfig) {
        let w = ui.available_width().clamp(200.0, 900.0);
        let (resp, painter) = ui.allocate_painter(egui::vec2(w, 150.0), egui::Sense::click_and_drag());
        let r = resp.rect;
        let vis = ui.visuals();
        painter.rect_filled(r, 4.0, vis.extreme_bg_color);
        let (llo, lhi) = (F_LO.log10(), F_HI.log10());
        let x_of = |f: f32| r.left() + r.width() * (f.max(F_LO).log10() - llo) / (lhi - llo);
        let f_of = |x: f32| 10f32.powf(llo + (x - r.left()) / r.width() * (lhi - llo)).clamp(F_LO, F_HI);
        let y_of = |db: f32| r.center().y - db.clamp(-DB_RANGE, DB_RANGE) / DB_RANGE * (r.height() / 2.0 - 6.0);
        let db_of = |y: f32| ((r.center().y - y) / (r.height() / 2.0 - 6.0) * DB_RANGE).clamp(-DB_RANGE, DB_RANGE);
        let grid = egui::Stroke::new(1.0_f32, vis.weak_text_color().gamma_multiply(0.35));
        let font = egui::FontId::proportional(10.0);
        for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
            let x = x_of(f);
            painter.line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], grid);
            painter.text(egui::pos2(x + 2.0, r.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, hz(f), font.clone(), vis.weak_text_color());
        }
        for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
            let y = y_of(db);
            let s = if db == 0.0 { egui::Stroke::new(1.0_f32, vis.weak_text_color().gamma_multiply(0.7)) } else { grid };
            painter.line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], s);
            painter.text(
                egui::pos2(r.left() + 2.0, y - 1.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{db:+}"),
                font.clone(),
                vis.weak_text_color(),
            );
        }
        let n = (r.width() / 3.0) as usize;
        let freqs: Vec<f32> = (0..=n).map(|i| f_of(r.left() + r.width() * i as f32 / n as f32)).collect();
        let db = eq_response(d, &freqs);
        let color = if d.eq_enabled { ACCENT } else { GRAY };
        let pts: Vec<egui::Pos2> = freqs.iter().zip(&db).map(|(&f, &g)| egui::pos2(x_of(f), y_of(g))).collect();
        painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0_f32, color)));
        if d.eq_mode != EqMode::Parametric {
            for (i, f) in EQ_FREQS.iter().enumerate() {
                painter.circle_filled(egui::pos2(x_of(*f), y_of(d.eq_gains[i] + d.eq_preamp_db)), 2.5, color);
            }
            return;
        }
        // handles
        let handle = |b: &PeqBand| egui::pos2(x_of(b.freq), y_of(if b.kind.has_gain() { b.gain_db } else { 0.0 }));
        let drag_id = ui.id().with("peq_drag");
        let near = |bands: &[PeqBand], p: egui::Pos2| {
            bands
                .iter()
                .enumerate()
                .map(|(i, b)| (i, handle(b).distance(p)))
                .filter(|(_, dist)| *dist < 14.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        if resp.drag_started() {
            // where the button went down: the pointer may already be well away by the time a drag starts
            let hit = ui.input(|i| i.pointer.press_origin()).and_then(|p| near(&d.peq_bands, p));
            ui.data_mut(|m| m.insert_temp(drag_id, hit));
            if let Some(i) = hit {
                self.peq_sel = i;
            }
        }
        let dragging: Option<usize> = ui.data(|m| m.get_temp(drag_id)).flatten();
        if let (Some(i), Some(p)) = (dragging.filter(|_| resp.dragged()), resp.interact_pointer_pos()) {
            if let Some(b) = d.peq_bands.get_mut(i) {
                b.freq = f_of(p.x.clamp(r.left(), r.right())).round();
                if b.kind.has_gain() {
                    b.gain_db = (db_of(p.y) * 10.0).round() / 10.0;
                }
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|m| m.remove::<Option<usize>>(drag_id));
        }
        if resp.clicked() {
            if let Some(i) = resp.interact_pointer_pos().and_then(|p| near(&d.peq_bands, p)) {
                self.peq_sel = i;
            }
        }
        let hovered = resp.hover_pos().and_then(|p| near(&d.peq_bands, p));
        if let Some(i) = hovered {
            let scroll = ui.input(|inp| inp.smooth_scroll_delta.y);
            if scroll != 0.0 {
                // the width changes instead of the page scrolling
                ui.input_mut(|inp| inp.smooth_scroll_delta.y = 0.0);
                let b = &mut d.peq_bands[i];
                b.q = (b.q * (1.0 + scroll * 0.004)).clamp(0.1, 30.0);
            }
            let b = &d.peq_bands[i];
            resp.clone().on_hover_text(format!("{} {} Hz, {:+.1} dB, Q {:.2}", tr(b.kind.label()), b.freq.round(), b.gain_db, b.q));
        }
        for (i, b) in d.peq_bands.iter().enumerate() {
            let p = handle(b);
            let sel = i == self.peq_sel;
            let fill = if !b.on {
                GRAY
            } else if sel || hovered == Some(i) {
                AMBER
            } else {
                color
            };
            painter.circle(p, if sel { 8.0_f32 } else { 7.0 }, fill, egui::Stroke::new(1.0_f32, vis.extreme_bg_color));
            painter.text(p, egui::Align2::CENTER_CENTER, format!("{}", i + 1), egui::FontId::proportional(9.0), Color32::BLACK);
        }
        ui.label(
            RichText::new(tr("Drag a numbered handle to set its frequency and gain; scroll over it to make it wider or narrower."))
                .small()
                .color(GRAY),
        );
    }

    fn peq_table(&mut self, ui: &mut egui::Ui, d: &mut crate::dsp::DspConfig) {
        let mut remove: Option<usize> = None;
        egui::Grid::new("peq_bands").num_columns(7).spacing([10.0, 4.0]).striped(true).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new(tr("On")).small());
            ui.label(RichText::new(tr("Filter")).small());
            ui.label(RichText::new(tr("Frequency")).small());
            ui.label(RichText::new(tr("Gain")).small());
            ui.label(RichText::new(tr("Width (Q)")).small());
            ui.label("");
            ui.end_row();
            for (i, b) in d.peq_bands.iter_mut().enumerate() {
                if ui.selectable_label(i == self.peq_sel, RichText::new(format!("{}", i + 1)).strong()).clicked() {
                    self.peq_sel = i;
                }
                ui.checkbox(&mut b.on, "");
                egui::ComboBox::from_id_salt(("peq_kind", i)).selected_text(tr(b.kind.label())).width(100.0).show_ui(ui, |ui| {
                    for k in FilterKind::ALL {
                        ui.selectable_value(&mut b.kind, k, tr(k.label()));
                    }
                });
                let speed = (b.freq * 0.01).max(0.5);
                ui.add(egui::DragValue::new(&mut b.freq).range(F_LO..=F_HI).speed(speed).max_decimals(0).suffix(" Hz"));
                ui.add_enabled(
                    b.kind.has_gain(),
                    egui::DragValue::new(&mut b.gain_db).range(-18.0..=18.0).speed(0.05).fixed_decimals(1).suffix(" dB"),
                );
                ui.add(egui::DragValue::new(&mut b.q).range(0.1..=30.0).speed(0.01).fixed_decimals(2));
                if delete_button(ui, ("peq_band", i), tr("this band")) {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            d.peq_bands.remove(i);
            self.peq_sel = self.peq_sel.min(d.peq_bands.len().saturating_sub(1));
        }
        ui.horizontal(|ui| {
            let full = d.peq_bands.len() >= PEQ_MAX_BANDS;
            if ui.add_enabled(!full, egui::Button::new(tr("➕ Add band"))).on_disabled_hover_text(tr("16 bands at most")).clicked() {
                // put it in the widest gap so it's easy to grab
                let mut fs: Vec<f32> = d.peq_bands.iter().map(|b| b.freq.max(F_LO).log10()).collect();
                fs.push(F_LO.log10());
                fs.push(F_HI.log10());
                fs.sort_by(f32::total_cmp);
                let gap = fs.windows(2).max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0]))).map(|w| (w[0] + w[1]) / 2.0).unwrap_or(3.0);
                d.peq_bands.push(PeqBand { freq: 10f32.powf(gap).round(), ..Default::default() });
                self.peq_sel = d.peq_bands.len() - 1;
            }
            ui.label(
                RichText::new(tr("Bell lifts or cuts around a frequency; shelves move everything above or below it; cuts remove it."))
                    .small()
                    .color(GRAY),
            );
        });
    }
}

fn graphic_sliders(ui: &mut egui::Ui, d: &mut crate::dsp::DspConfig) {
    ui.horizontal(|ui| {
        for i in 0..12 {
            ui.vertical(|ui| {
                ui.set_width(46.0);
                ui.add(egui::Slider::new(&mut d.eq_gains[i], -12.0..=12.0).vertical().show_value(false));
                // exact value in dB: drag it, or double-click to type
                ui.add_sized([46.0, 20.0], egui::DragValue::new(&mut d.eq_gains[i]).range(-12.0..=12.0).speed(0.05).fixed_decimals(1))
                    .on_hover_text(tr("Drag, or double-click to type an exact value"));
                ui.label(RichText::new(hz(EQ_FREQS[i])).small());
            });
        }
    });
}
