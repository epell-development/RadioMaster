use chrono::{DateTime, Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ActionKind {
    #[default]
    LoadPlaylistAndPlay,
    PlayFile,
    InsertNext,
    Stop,
    StartEncoder,
    StopEncoder,
    SetVolume,
    /// make a new playlist from a generator template (`path` = template name)
    GenerateReplace,
    /// add a generator template's tracks after the playlist (`path` = template name)
    GenerateAppend,
    /// run an advert / jingle rule now (`arg` = rule id)
    RunRule,
    /// put a playlist tab on air (`path` = playlist name)
    SwitchPlaylist,
    /// any station command, e.g. "VOLUME 80" (`path`)
    Command,
    /// play the on-air playlist again from its first row
    RestartPlaylist,
}

impl ActionKind {
    pub const ALL: [ActionKind; 13] = [
        ActionKind::GenerateReplace,
        ActionKind::GenerateAppend,
        ActionKind::RunRule,
        ActionKind::SwitchPlaylist,
        ActionKind::RestartPlaylist,
        ActionKind::LoadPlaylistAndPlay,
        ActionKind::PlayFile,
        ActionKind::InsertNext,
        ActionKind::Stop,
        ActionKind::StartEncoder,
        ActionKind::StopEncoder,
        ActionKind::SetVolume,
        ActionKind::Command,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ActionKind::LoadPlaylistAndPlay => "Load playlist (M3U) and play",
            ActionKind::PlayFile => "Play file now",
            ActionKind::InsertNext => "Insert file as next",
            ActionKind::Stop => "Stop playback",
            ActionKind::StartEncoder => "Start encoder #",
            ActionKind::StopEncoder => "Stop encoder #",
            ActionKind::SetVolume => "Set volume (0-1)",
            ActionKind::GenerateReplace => "Generate a new playlist from a template",
            ActionKind::GenerateAppend => "Add tracks from a template after the playlist",
            ActionKind::RunRule => "Play adverts / jingles from a rule",
            ActionKind::SwitchPlaylist => "Put a playlist tab on air",
            ActionKind::Command => "Run a station command",
            ActionKind::RestartPlaylist => "Play the on-air playlist again from the top",
        }
    }
    /// the action takes a generator template name
    pub fn needs_template(self) -> bool {
        matches!(self, ActionKind::GenerateReplace | ActionKind::GenerateAppend)
    }
    pub fn needs_path(self) -> bool {
        matches!(self, ActionKind::LoadPlaylistAndPlay | ActionKind::PlayFile | ActionKind::InsertNext)
    }
    pub fn needs_arg(self) -> bool {
        matches!(self, ActionKind::StartEncoder | ActionKind::StopEncoder | ActionKind::SetVolume)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Event {
    pub name: String,
    pub enabled: bool,
    pub hour: u32,
    pub minute: u32,
    /// Monday..Sunday
    pub days: [bool; 7],
    pub action: ActionKind,
    pub path: String,
    pub arg: f32,
    /// runs only from / to these dates (YYYY-MM-DD, empty = no limit)
    pub from_date: String,
    pub to_date: String,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            name: "New event".into(),
            enabled: true,
            hour: 12,
            minute: 0,
            days: [true; 7],
            action: ActionKind::GenerateReplace,
            path: String::new(),
            arg: 0.0,
            from_date: String::new(),
            to_date: String::new(),
        }
    }
}

impl Event {
    pub fn due(&self, now: &DateTime<Local>) -> bool {
        let today = now.format("%Y-%m-%d").to_string();
        self.enabled
            && self.hour == now.hour()
            && self.minute == now.minute()
            && self.days[now.weekday().num_days_from_monday() as usize]
            && (self.from_date.trim().is_empty() || today.as_str() >= self.from_date.trim())
            && (self.to_date.trim().is_empty() || today.as_str() <= self.to_date.trim())
    }
}
