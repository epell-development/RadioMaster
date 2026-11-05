//! "Getting started": a short tour of the main parts of RadioMaster. It opens by itself the first
//! time RadioMaster starts on a computer, can be skipped or closed at any step, and comes back from
//! Help → Getting started. Also the notice shown when the system file dialog couldn't open.
use super::*;

/// The file whose presence means the tour was finished or skipped on this computer.
fn seen_marker() -> PathBuf {
    data_dir().join("tutorial-seen")
}

/// Show the tour at start-up: only on a computer that hasn't seen it yet.
pub(super) fn first_run() -> Option<usize> {
    (!seen_marker().exists()).then_some(0)
}

/// What a step's button opens.
#[derive(Clone, Copy)]
enum Show {
    Tool(ToolWin),
    Settings(SettingsPage),
}

/// One page of the tour.
struct Step {
    icon: &'static str,
    title: &'static str,
    text: Vec<&'static str>,
    show: Option<(&'static str, Show)>,
}

fn steps() -> [Step; 9] {
    [
        Step {
            icon: "👋",
            title: tr("Welcome to RadioMaster"),
            text: vec![
                tr("RadioMaster plays your station: music, jingles, adverts and live shows, on a timetable or by hand, and sends it to your speakers and your stream."),
                tr("This short tour shows the main parts. Each step says where things are; the buttons open them so you can look while you read."),
                tr("You can skip the tour or close this window at any time. It's always in Help → Getting started."),
            ],
            show: None,
        },
        Step {
            icon: "🎵",
            title: tr("Add your music"),
            text: vec![
                tr("The Library panel on the left lists your tracks. Click ➕ Folder… to add a music folder; RadioMaster scans it and keeps it up to date (⟳ Rescan looks again)."),
                tr("Type in the search box to find a track by artist, title or album."),
                tr("You can also add music folders in Settings → Library."),
            ],
            show: Some((tr("Open Settings → Library"), Show::Settings(SettingsPage::Library))),
        },
        Step {
            icon: "📋",
            title: tr("Build a playlist"),
            text: vec![
                tr("Double-click a track in the Library, or drag tracks onto the playlist, to add them. Drag rows to change the order."),
                tr("Each playlist has its own tab: ➕ makes a new one; double-click a tab to rename it, right-click it for more (save or load an M3U file, put it on air)."),
                tr("File → Open playlist… and Save playlist as… load and save M3U files. The Playlist menu adds URLs, time announcements, text to speech, voice tracks and more."),
            ],
            show: None,
        },
        Step {
            icon: "▶",
            title: tr("Play on air"),
            text: vec![
                tr("The buttons at the top left play, pause, stop and skip to the next track. Vol is the master volume, and Fade sets how one track blends into the next."),
                tr("The coloured boxes show the last track, the track on air and what's coming up next."),
                tr("Pick your sound card (and a second one for headphones or a studio monitor) in Settings → Audio devices."),
            ],
            show: Some((tr("Open Settings → Audio devices"), Show::Settings(SettingsPage::Devices))),
        },
        Step {
            icon: "🔊",
            title: tr("Jingles and the cart wall"),
            text: vec![
                tr("The Cart wall is a grid of pads for jingles, sound effects and drops: click a pad to play it over the music. Right-click a pad to give it a sound file, a name and a colour."),
                tr("The Jingles menu plays the first pads, stops all jingles, and brings the Cart wall back with Show cart wall."),
            ],
            show: None,
        },
        Step {
            icon: "⏰",
            title: tr("Things that happen on time: the Event scheduler"),
            text: vec![
                tr("An event does something at a set time, on the days you pick, and can repeat (for example every hour). Click ➕ Add event, set the time, then choose what it does under Do."),
                tr("For “Load playlist (M3U) and play”, click … to choose the playlist file, or type or paste its full path into the box. “▶ Run now” tries the event straight away."),
                tr("➕ Hourly time announcement adds a ready-made event that says the time every hour."),
            ],
            show: Some((tr("Open the Event scheduler"), Show::Tool(ToolWin::Scheduler))),
        },
        Step {
            icon: "🎲",
            title: tr("Fill the hours: generator and adverts"),
            text: vec![
                tr("The Playlist generator builds playlists from your music categories with templates (for example two current hits, then a classic, then a jingle)."),
                tr("The Advertisement scheduler plays adverts and jingles in breaks: every so many songs or minutes, or at set times, and prints a schedule for each advert."),
            ],
            show: Some((tr("Open the Playlist generator"), Show::Tool(ToolWin::Generator))),
        },
        Step {
            icon: "📡",
            title: tr("Go live on the internet and record shows"),
            text: vec![
                tr("Streams / encoders sends your station to an Icecast or Shoutcast server (MP3, AAC, Opus and more), so listeners can tune in online."),
                tr("The Show recorder saves what goes on air to files, for podcasts or your logs. Both are in the Tools menu."),
            ],
            show: Some((tr("Open Streams / encoders"), Show::Tool(ToolWin::Streams))),
        },
        Step {
            icon: "✅",
            title: tr("You're ready"),
            text: vec![
                tr("Every other tool is in the Tools menu: the track editor, reports and proof of play, play history, library cleanup and more. ⚙ opens Settings."),
                tr("Questions? Ask on the RadioMaster Discord, linked from the website. You can take this tour again from Help → Getting started."),
            ],
            show: None,
        },
    ]
}

impl App {
    /// Start the tour from the first page (Help → Getting started).
    pub(super) fn open_tutorial(&mut self) {
        self.tutorial = Some(0);
    }

    pub(super) fn tutorial_window(&mut self, ctx: &egui::Context, core: &mut Playout) {
        let Some(at) = self.tutorial else { return };
        let steps = steps();
        let at = at.min(steps.len() - 1);
        let step = &steps[at];
        let last = at + 1 == steps.len();
        let mut open = true;
        let mut go: Option<usize> = None;
        let mut done = false;
        let mut show = None;
        Self::window_for(ctx, tr("Getting started"), [560.0, 400.0]).id(egui::Id::new("tutorial")).collapsible(false).open(&mut open).show(
            ctx,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(step.icon).size(26.0));
                    ui.heading(step.title);
                });
                ui.label(RichText::new(format!("{} {} / {}", tr("Step"), at + 1, steps.len())).small().color(GRAY));
                ui.separator();
                // the text scrolls, so Back / Next always stay on screen
                egui::ScrollArea::vertical().max_height((ui.available_height() - 48.0).max(80.0)).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        for p in &step.text {
                            ui.label(*p);
                            ui.add_space(4.0);
                        }
                        if let Some((label, what)) = step.show {
                            ui.add_space(2.0);
                            if ui.button(format!("👉 {label}")).clicked() {
                                show = Some(what);
                            }
                        }
                    },
                );
                ui.separator();
                ui.horizontal(|ui| {
                    if !last
                        && ui.button(tr("Skip tutorial")).on_hover_text(tr("Close it; it's always in Help → Getting started")).clicked()
                    {
                        done = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if last {
                            if ui.button(RichText::new(tr("   Finish   ")).strong()).clicked() {
                                done = true;
                            }
                        } else if ui.button(RichText::new(tr("Next ▶")).strong()).clicked() {
                            go = Some(at + 1);
                        }
                        if at > 0 && ui.button(tr("◀ Back")).clicked() {
                            go = Some(at - 1);
                        }
                    });
                });
            },
        );
        if self.tutorial_raise > 0 {
            // what a step opened goes on top first; put the tour back in front of it
            self.tutorial_raise -= 1;
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, egui::Id::new("tutorial")));
        }
        if show.is_some() {
            self.tutorial_raise = 2;
        }
        match show {
            Some(Show::Tool(w)) => self.launch_tool(core, w),
            Some(Show::Settings(page)) if self.perm.settings => {
                self.settings_page = page;
                self.show_settings = true;
            }
            Some(Show::Settings(_)) => core.say("Your account isn't allowed to change the settings"),
            None => {}
        }
        if !open || done {
            self.tutorial = None;
            // remember it on this computer (off the UI thread), so it doesn't open by itself again
            thread::spawn(|| {
                let p = seen_marker();
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(p, "");
            });
        } else if let Some(g) = go {
            self.tutorial = Some(g);
        }
    }

    /// Shown when a system file dialog couldn't open (see `sys_dlg`), instead of nothing happening.
    pub(super) fn no_file_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.no_file_dialog {
            return;
        }
        let mut open = true;
        let mut ok = false;
        Self::window_for(ctx, tr("The file dialog didn't open"), [460.0, 220.0])
            .id(egui::Id::new("no_file_dialog"))
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(tr("RadioMaster asked the system to show its file dialog, but it didn't open."));
                ui.label(tr("Where there's a box next to the button, you can type or paste the file's full path into it instead."));
                if cfg!(target_os = "linux") {
                    ui.label(
                        RichText::new(tr("On Linux the dialog needs xdg-desktop-portal (with its GTK or KDE part) or zenity. Install one of them, for example: sudo apt install zenity"))
                            .small()
                            .color(GRAY),
                    );
                }
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ok = ui.button(RichText::new(tr("   OK   ")).strong()).clicked();
                });
            });
        self.no_file_dialog = open && !ok;
    }
}
