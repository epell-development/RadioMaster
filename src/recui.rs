//! Show recording: what the recorder is doing and its settings (Tools → Show recorder, and
//! Settings → Show recording). The recording itself runs in the background (see recorder.rs).
use super::*;
use crate::config::{RecFormat, SEGMENT_MINS};
use chrono::TimeZone;

const MP3_RATES: [u32; 7] = [64, 96, 128, 160, 192, 256, 320];

fn mins_label(m: u32) -> String {
    match m {
        60 => tr("1 hour").to_string(),
        m if m % 60 == 0 => format!("{} {}", m / 60, tr("hours")),
        m => format!("{m} {}", tr("minutes")),
    }
}

fn size_label(bytes: f64) -> String {
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 10e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.1} MB", bytes / 1e6)
    }
}

/// Roughly how many bytes an hour of recording takes.
fn bytes_per_hour(f: RecFormat, kbps: u32) -> f64 {
    match f {
        RecFormat::Mp3 => kbps as f64 * 1000.0 / 8.0 * 3600.0,
        RecFormat::Flac => 340e6,
        RecFormat::Wav => crate::engine::SR as f64 * 4.0 * 3600.0,
    }
}

impl App {
    pub(super) fn tab_recording(&mut self, ui: &mut egui::Ui, core: &mut Playout) {
        self.recording_status(ui, core);
        ui.separator();
        let connected = self.srv.connected();
        let rec = &mut core.cfg.recording;
        ui.checkbox(&mut rec.enabled, RichText::new(tr("Record the station's output")).strong());
        ui.label(
            RichText::new(tr(
                "Everything that goes on air is saved, one file per hour (or the length chosen below), as proof of broadcast. It records in the background while RadioMaster runs.",
            ))
            .small()
            .color(GRAY),
        );
        if connected {
            ui.label(
                RichText::new(tr("You're connected to a RadioMaster Server: the server records, into a folder on the server."))
                    .small()
                    .color(AMBER),
            );
        }
        ui.add_space(6.0);
        let mut pick = false;
        egui::Grid::new("rec_settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tr("Folder"));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rec.folder)
                            .desired_width(320.0)
                            .hint_text(data_dir().join("recordings").display().to_string()),
                    );
                    pick = ui.button(tr("Choose…")).clicked();
                });
                ui.label(
                    RichText::new(tr(
                        "A folder on this computer, a network folder (\\\\server\\share) or a cloud folder that syncs (OneDrive, Dropbox, Google Drive). Each day gets its own folder. If the folder can't be written, that hour goes into the data folder's recordings folder instead.",
                    ))
                    .small()
                    .color(GRAY),
                );
            });
            ui.end_row();

            ui.label(tr("Format"));
            egui::ComboBox::from_id_salt("rec_format").selected_text(tr(rec.format.label())).width(320.0).show_ui(ui, |ui| {
                for f in RecFormat::ALL {
                    ui.selectable_value(&mut rec.format, f, tr(f.label()));
                }
            });
            ui.end_row();

            if rec.format == RecFormat::Mp3 {
                ui.label(tr("Bitrate"));
                egui::ComboBox::from_id_salt("rec_bitrate").selected_text(format!("{} kbps", rec.bitrate_kbps)).show_ui(ui, |ui| {
                    for k in MP3_RATES {
                        ui.selectable_value(&mut rec.bitrate_kbps, k, format!("{k} kbps"));
                    }
                });
                ui.end_row();
            }

            ui.label(tr("Each file"));
            let seg = rec.segment();
            egui::ComboBox::from_id_salt("rec_segment").selected_text(mins_label(seg)).show_ui(ui, |ui| {
                for m in SEGMENT_MINS {
                    if rec.format == RecFormat::Wav && m > 360 {
                        continue; // a WAV file can't be bigger than 4 GB
                    }
                    if ui.selectable_label(seg == m, mins_label(m)).clicked() {
                        rec.segment_mins = m;
                    }
                }
            });
            ui.end_row();

            ui.label(tr("Keep"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut rec.keep_days).range(0..=3650).suffix(format!(" {}", tr("days"))));
                ui.label(
                    RichText::new(if rec.keep_days == 0 {
                        tr("0: recordings are never deleted")
                    } else {
                        tr("older recordings are deleted (only RadioMaster's own files)")
                    })
                    .small()
                    .color(GRAY),
                );
            });
            ui.end_row();

            ui.label(tr("Days"));
            super::adsui::days_edit(ui, &mut rec.days);
            ui.end_row();

            ui.label(tr("Hours"));
            ui.horizontal(|ui| {
                let mut all_day = rec.from_hour == rec.to_hour;
                if ui.checkbox(&mut all_day, tr("all day")).changed() {
                    (rec.from_hour, rec.to_hour) = if all_day { (0, 0) } else { (6, 22) };
                }
                if !all_day {
                    ui.label(tr("from"));
                    ui.add(egui::DragValue::new(&mut rec.from_hour).range(0..=23).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                    ui.label(tr("until"));
                    ui.add(egui::DragValue::new(&mut rec.to_hour).range(0..=24).custom_formatter(|v, _| format!("{:02}:00", v as u32)));
                    if rec.to_hour == 24 {
                        rec.to_hour = 0;
                    }
                }
            });
            ui.end_row();
        });
        let per_hour = bytes_per_hour(rec.format, rec.bitrate_kbps);
        let hours_a_day = if rec.from_hour == rec.to_hour { 24 } else { (rec.to_hour + 24 - rec.from_hour) % 24 };
        let days_a_week = rec.days.iter().filter(|d| **d).count() as f64;
        let mut size = format!(
            "{} {} {}, {} {}",
            tr("About"),
            size_label(per_hour),
            tr("an hour"),
            size_label(per_hour * hours_a_day as f64),
            tr("a day")
        );
        if rec.keep_days > 0 {
            let total = per_hour * hours_a_day as f64 * days_a_week / 7.0 * rec.keep_days as f64;
            size = format!("{size}; {} {}", size_label(total), tr("for all the days kept"));
        }
        ui.add_space(4.0);
        ui.label(RichText::new(size).small().color(GRAY));
        if pick {
            self.pick(Pick::Folder, tr("Recordings folder"), |_, v| Some(Dlg::RecFolder(v.into_iter().next()?)));
        }
    }

    /// The recorder's state: the file being written and how far into it.
    fn recording_status(&mut self, ui: &mut egui::Ui, core: &Playout) {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        let Some(st) = core.recording() else {
            let pending = core.cfg.recording.enabled;
            ui.label(RichText::new(if pending { tr("Starting…") } else { tr("Not recording") }).strong().color(GRAY));
            return;
        };
        let (text, color) = match st.state.as_str() {
            "recording" => (tr("● Recording"), RED),
            "waiting" => (tr("Waiting: outside the days and hours set"), GRAY),
            "starting" => (tr("Starting: waiting for sound from the mixer"), GRAY),
            "error" => (tr("Not recording"), AMBER),
            _ => (tr("Not recording"), GRAY),
        };
        ui.label(RichText::new(text).strong().size(18.0).color(color));
        if st.state == "recording" {
            let name = std::path::Path::new(&st.file).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ui.label(RichText::new(name).monospace());
            let now = core.now().timestamp();
            let len = (st.seg_end - st.seg_start).max(1);
            let done = ((now - st.seg_start) as f32 / len as f32).clamp(0.0, 1.0);
            let at = |t: i64| Local.timestamp_opt(t, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default();
            ui.add(egui::ProgressBar::new(done).desired_width(ui.available_width().min(460.0)).text(format!(
                "{} – {}  ·  {} {}",
                at(st.seg_start),
                at(st.seg_end),
                fmt(st.secs as f32),
                tr("recorded")
            )));
            ui.label(
                RichText::new(format!("{}  ·  {} {}", size_label(st.bytes as f64), st.files_done, tr("files finished since it started")))
                    .small()
                    .color(GRAY),
            );
        }
        if !st.error.is_empty() {
            ui.label(RichText::new(&st.error).small().color(AMBER));
        }
        if !self.srv.connected() && ui.button(tr("📁 Open the recordings folder")).clicked() {
            let dir = std::path::Path::new(&st.file)
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| core.cfg.recording.folder_path());
            crate::plugins::open_folder(&dir);
        }
    }
}
