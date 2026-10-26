//! Reports tool: what played (songs, adverts, jingles…) in a period, per item, with CSV and PDF
//! exports — a summary report, or proof of play for one item or for each item.
use super::*;
use crate::report::{self, Filter, Meta, Play, Row};
use chrono::{Datelike, NaiveDate};

/// Which plays a report shows.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum RepKind {
    All,
    Songs,
    Adverts,
    Jingles,
    Other,
}

impl RepKind {
    const ALL: [RepKind; 5] = [RepKind::All, RepKind::Songs, RepKind::Adverts, RepKind::Jingles, RepKind::Other];
    fn label(self) -> &'static str {
        match self {
            RepKind::All => "Everything",
            RepKind::Songs => "Songs",
            RepKind::Adverts => "Adverts",
            RepKind::Jingles => "Jingles",
            RepKind::Other => "Speech, voice & other",
        }
    }
    fn title(self) -> &'static str {
        match self {
            RepKind::All => "Play report",
            RepKind::Songs => "Music report",
            RepKind::Adverts => "Advert report",
            RepKind::Jingles => "Jingle report",
            RepKind::Other => "Speech & other report",
        }
    }
    fn kinds(self) -> Vec<String> {
        let v: &[&str] = match self {
            RepKind::All => &[],
            RepKind::Songs => &["song"],
            RepKind::Adverts => &["advert"],
            RepKind::Jingles => &["jingle"],
            RepKind::Other => &["voice", "speech", "time", "stream", "live", "teaser", "other"],
        };
        v.iter().map(|s| s.to_string()).collect()
    }
}

/// What the next save dialog writes.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Export {
    SummaryPdf,
    SummaryCsv,
    PlaysCsv,
    /// proof of play for every item, one page each
    ProofAllPdf,
    /// proof of play for the selected item
    ProofOnePdf,
    /// every play of the selected item
    PlaysOneCsv,
}

pub(super) struct RepUi {
    pub from: String,
    pub to: String,
    pub kind: RepKind,
    pub search: String,
    pub exclude: String,
    /// the whole history file (read on open / Refresh)
    all: Vec<Play>,
    loaded: bool,
    /// being read in the background
    loading: bool,
    /// plays in the report (indexes into `all`)
    sel: Vec<usize>,
    pub rows: Vec<Row>,
    /// filter the rows were made with
    key: String,
    /// selected item (index into `rows`)
    pub pick: Option<usize>,
    pub export: Option<Export>,
    pub msg: String,
}

impl Default for RepUi {
    fn default() -> Self {
        let today = Local::now().date_naive();
        RepUi {
            from: (today - chrono::Duration::days(6)).format("%Y-%m-%d").to_string(),
            to: today.format("%Y-%m-%d").to_string(),
            kind: RepKind::All,
            search: String::new(),
            exclude: String::new(),
            all: vec![],
            loaded: false,
            loading: false,
            sel: vec![],
            rows: vec![],
            key: String::new(),
            pick: None,
            export: None,
            msg: String::new(),
        }
    }
}

impl RepUi {
    /// Read the history again next time (it is another station's after connecting or disconnecting).
    pub(super) fn forget(&mut self) {
        self.loaded = false;
        self.all.clear();
        self.key.clear();
    }

    /// The history file, read in the background.
    pub(super) fn set_history(&mut self, all: Vec<Play>) {
        self.all = all;
        self.loaded = true;
        self.loading = false;
        self.key.clear();
    }

    fn filter(&self) -> Option<Filter> {
        let d = |s: &str| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
        let (from, to) = (d(&self.from)?, d(&self.to)?);
        Some(Filter {
            from: from.min(to),
            to: to.max(from),
            kinds: self.kind.kinds(),
            search: self.search.clone(),
            exclude: self.exclude.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
        })
    }

    fn refresh(&mut self) {
        let Some(f) = self.filter() else { return };
        let key = format!("{:?}{}", f, self.all.len());
        if key == self.key {
            return;
        }
        let picked = self.pick.and_then(|i| self.rows.get(i)).map(|r| (r.name.clone(), r.kind.clone()));
        self.sel = self.all.iter().enumerate().filter(|(_, p)| f.matches(p)).map(|(i, _)| i).collect();
        let plays: Vec<&Play> = self.sel.iter().map(|&i| &self.all[i]).collect();
        self.rows = report::summarize(&plays);
        self.pick = picked.and_then(|(n, k)| self.rows.iter().position(|r| r.name == n && r.kind == k));
        self.key = key;
    }

    fn plays(&self) -> Vec<&Play> {
        self.sel.iter().map(|&i| &self.all[i]).collect()
    }

    fn set_period(&mut self, from: NaiveDate, to: NaiveDate) {
        self.from = from.format("%Y-%m-%d").to_string();
        self.to = to.format("%Y-%m-%d").to_string();
    }

    /// Write the export chosen before the save dialog opened.
    pub(super) fn write_export(&mut self, path: &std::path::Path, station: String) -> Result<String, String> {
        let what = self.export.take().ok_or("nothing to export")?;
        let f = self.filter().ok_or("check the dates (YYYY-MM-DD)")?;
        let plays = self.plays();
        let one = self.pick.and_then(|i| self.rows.get(i)).cloned();
        let meta = Meta { station, title: self.kind.title().to_string(), filter: f };
        let only = |r: &Row| -> Vec<&Play> { plays.iter().copied().filter(|p| p.text == r.name && p.kind == r.kind).collect() };
        let bytes: Vec<u8> = match what {
            Export::SummaryPdf => report::pdf_summary(&meta, &self.rows, &plays),
            Export::SummaryCsv => report::csv_summary(&self.rows).into_bytes(),
            Export::PlaysCsv => report::csv_plays(&plays).into_bytes(),
            Export::ProofAllPdf => report::pdf_proof(&meta, &self.rows, &plays),
            Export::ProofOnePdf => {
                let r = one.ok_or("select an item first")?;
                report::pdf_proof(&meta, std::slice::from_ref(&r), &only(&r))
            }
            Export::PlaysOneCsv => {
                let r = one.ok_or("select an item first")?;
                report::csv_plays(&only(&r)).into_bytes()
            }
        };
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        Ok(format!("Saved {}", path.display()))
    }
}

impl App {
    pub(super) fn station_name(core: &Playout) -> String {
        core.cfg.encoders.iter().map(|e| e.stream_name.trim()).find(|n| !n.is_empty()).unwrap_or("RadioMaster").to_string()
    }

    fn ask_export(&mut self, what: Export) {
        self.rep.export = Some(what);
        let kind = self.rep.kind.title().to_lowercase().replace(' ', "-");
        let item = self
            .rep
            .pick
            .and_then(|i| self.rep.rows.get(i))
            .map(|r| r.name.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect::<String>().trim().replace(' ', "-"))
            .unwrap_or_default();
        let period = format!("{}_{}", self.rep.from.trim(), self.rep.to.trim());
        let (name, ext) = match what {
            Export::SummaryPdf => (format!("{kind}_{period}.pdf"), "pdf"),
            Export::SummaryCsv => (format!("{kind}_{period}.csv"), "csv"),
            Export::PlaysCsv => (format!("{kind}_every-play_{period}.csv"), "csv"),
            Export::ProofAllPdf => (format!("proof-of-play_{period}.pdf"), "pdf"),
            Export::ProofOnePdf => (format!("proof-of-play_{item}_{period}.pdf"), "pdf"),
            Export::PlaysOneCsv => (format!("plays_{item}_{period}.csv"), "csv"),
        };
        self.dlg(move || {
            rfd::FileDialog::new().add_filter(ext.to_uppercase(), &[ext]).set_file_name(&name).save_file().map(Dlg::SaveReport)
        });
    }

    pub(super) fn tab_reports(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        if !self.rep.loaded && !self.rep.loading {
            // a station's history grows for years: read it off the UI thread
            self.rep.loading = true;
            // while connected, the server's history (the station plays there)
            let server = self.srv.session();
            self.dlg(move || {
                Some(super::Dlg::History(match server {
                    Some(s) => match crate::link::Link::plays(&s) {
                        Ok(csv) => report::parse(&csv),
                        Err(_) => vec![],
                    },
                    None => report::load(),
                }))
            });
        }
        // ---- what
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Report")).strong());
            for k in RepKind::ALL {
                ui.selectable_value(&mut self.rep.kind, k, tr(k.label()));
            }
        });
        // ---- when
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Period")).strong());
            super::date_edit(ui, "rep_from", &mut self.rep.from, false);
            ui.label("–");
            super::date_edit(ui, "rep_to", &mut self.rep.to, false);
            let today = Local::now().date_naive();
            let month_start = today.with_day(1).unwrap_or(today);
            let last_month_end = month_start - chrono::Duration::days(1);
            let quick: [(&str, NaiveDate, NaiveDate); 6] = [
                ("Today", today, today),
                ("Yesterday", today - chrono::Duration::days(1), today - chrono::Duration::days(1)),
                ("This week", report::week_start(today), today),
                ("Last 7 days", today - chrono::Duration::days(6), today),
                ("This month", month_start, today),
                ("Last month", last_month_end.with_day(1).unwrap_or(last_month_end), last_month_end),
            ];
            for (label, a, b) in quick {
                if ui.small_button(tr(label)).clicked() {
                    self.rep.set_period(a, b);
                }
            }
            if ui.small_button("⟳").on_hover_text(tr("Read the play history again")).clicked() {
                self.rep.loaded = false;
            }
        });
        // ---- narrow down
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Only")).strong());
            ui.add(egui::TextEdit::singleline(&mut self.rep.search).desired_width(170.0).hint_text(tr("items containing…")));
            ui.label(RichText::new(tr("Leave out")).strong());
            ui.add(egui::TextEdit::singleline(&mut self.rep.exclude).desired_width(170.0).hint_text(tr("e.g. station id, news")));
        });
        self.rep.refresh();
        let bad_dates = self.rep.filter().is_none();
        // ---- exports and totals (left to right: a right-aligned layout would stretch the window)
        ui.horizontal_wrapped(|ui| {
            let has_pick = self.rep.pick.is_some();
            ui.add_enabled_ui(!bad_dates, |ui| {
                if ui.button(tr("📄 PDF…")).on_hover_text(tr("Save this report as a PDF")).clicked() {
                    self.ask_export(Export::SummaryPdf);
                }
                ui.menu_button(tr("⬇ Export"), |ui| {
                    let mut pick = None;
                    ui.label(RichText::new(tr("This report")).small().color(GRAY));
                    if ui.button(tr("📄 Report (PDF)…")).clicked() {
                        pick = Some(Export::SummaryPdf);
                    }
                    if ui.button(tr("Report (CSV / Excel)…")).clicked() {
                        pick = Some(Export::SummaryCsv);
                    }
                    if ui.button(tr("Every play (CSV / Excel)…")).clicked() {
                        pick = Some(Export::PlaysCsv);
                    }
                    if ui.button(tr("📄 Proof of play, each item on its own page (PDF)…")).clicked() {
                        pick = Some(Export::ProofAllPdf);
                    }
                    ui.separator();
                    ui.label(RichText::new(tr("Selected item")).small().color(GRAY));
                    if ui.add_enabled(has_pick, egui::Button::new(tr("📄 Proof of play (PDF)…"))).clicked() {
                        pick = Some(Export::ProofOnePdf);
                    }
                    if ui.add_enabled(has_pick, egui::Button::new(tr("Every play (CSV / Excel)…"))).clicked() {
                        pick = Some(Export::PlaysOneCsv);
                    }
                    if !has_pick {
                        ui.label(RichText::new(tr("Click an item in the list to select it")).small().color(GRAY));
                    }
                    if let Some(p) = pick {
                        self.ask_export(p);
                        ui.close_menu();
                    }
                });
            });
            let plays = self.rep.sel.len();
            let secs: f32 = self.rep.rows.iter().map(|r| r.secs).sum();
            if bad_dates {
                ui.colored_label(RED, tr("Check the dates (YYYY-MM-DD)"));
            } else {
                ui.label(
                    RichText::new(format!(
                        "{} {} · {plays} {} · {} {}",
                        self.rep.rows.len(),
                        tr("items"),
                        tr("plays"),
                        report::airtime(secs),
                        tr("on air")
                    ))
                    .color(GRAY),
                );
            }
        });
        if !self.rep.msg.is_empty() {
            ui.label(RichText::new(&self.rep.msg).small().color(GRAY));
        }
        let _ = core;
        ui.separator();

        // ---- the items; the selected one's plays underneath
        let detail_h = if self.rep.pick.is_some() { 160.0 } else { 0.0 };
        let mut clicked: Option<usize> = None;
        let mut export_one: Option<(usize, Export)> = None;
        let table_h = (ui.available_height() - detail_h).max(120.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), table_h), |ui| {
            let rows = &self.rep.rows;
            let pick = self.rep.pick;
            TableBuilder::new(ui)
                .id_salt("report_table")
                .striped(true)
                .sense(egui::Sense::click())
                .max_scroll_height(table_h)
                .column(Column::remainder().at_least(160.0).clip(true))
                .column(Column::exact(86.0))
                .column(Column::exact(48.0))
                .column(Column::exact(70.0))
                .column(Column::exact(110.0))
                .header(20.0, |mut h| {
                    for t in ["Item", "Type", "Plays", "Airtime", "Last played"] {
                        h.col(|ui| {
                            ui.strong(tr(t));
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, rows.len(), |mut row| {
                        let i = row.index();
                        let r = &rows[i];
                        row.set_selected(pick == Some(i));
                        row.col(|ui| {
                            ui.add(egui::Label::new(&r.name).truncate());
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(tr(report::kind_name(&r.kind))).color(GRAY));
                        });
                        row.col(|ui| {
                            ui.label(r.plays.to_string());
                        });
                        row.col(|ui| {
                            ui.label(report::airtime(r.secs));
                        });
                        row.col(|ui| {
                            ui.label(r.last.format("%d/%m %H:%M").to_string());
                        });
                        let resp = row.response();
                        if resp.clicked() || resp.secondary_clicked() {
                            clicked = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button(tr("📄 Proof of play (PDF)…")).clicked() {
                                export_one = Some((i, Export::ProofOnePdf));
                                ui.close_menu();
                            }
                            if ui.button(tr("Every play (CSV / Excel)…")).clicked() {
                                export_one = Some((i, Export::PlaysOneCsv));
                                ui.close_menu();
                            }
                        });
                    });
                });
        });
        if let Some(i) = clicked {
            self.rep.pick = if self.rep.pick == Some(i) && export_one.is_none() { None } else { Some(i) };
        }
        if let Some((i, e)) = export_one {
            self.rep.pick = Some(i);
            self.ask_export(e);
        }
        if self.rep.rows.is_empty() && !bad_dates {
            ui.label(RichText::new(tr("Nothing in the play history for this period.")).color(GRAY));
        }
        if let Some(r) = self.rep.pick.and_then(|i| self.rep.rows.get(i)).cloned() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new(&r.name).strong());
                ui.label(RichText::new(format!("· {} {}", r.plays, tr("plays"))).color(GRAY));
                if ui.small_button(tr("📄 Proof of play (PDF)…")).clicked() {
                    self.ask_export(Export::ProofOnePdf);
                }
                if ui.small_button("✖").on_hover_text(tr("Close")).clicked() {
                    self.rep.pick = None;
                }
            });
            let times: Vec<String> = self
                .rep
                .plays()
                .into_iter()
                .filter(|p| p.text == r.name && p.kind == r.kind)
                .map(|p| p.at.format("%a %d/%m %H:%M").to_string())
                .collect();
            egui::ScrollArea::vertical().id_salt("report_times").max_height(110.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for t in times.iter().rev() {
                        ui.add(egui::Label::new(RichText::new(t).monospace().small()).extend());
                        ui.add_space(8.0);
                    }
                });
            });
        }
    }
}
