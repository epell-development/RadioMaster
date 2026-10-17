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
}

impl ActionKind {
    pub const ALL: [ActionKind; 7] = [
        ActionKind::LoadPlaylistAndPlay,
        ActionKind::PlayFile,
        ActionKind::InsertNext,
        ActionKind::Stop,
        ActionKind::StartEncoder,
        ActionKind::StopEncoder,
        ActionKind::SetVolume,
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
        }
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
}

impl Default for Event {
    fn default() -> Self {
        Self {
            name: "New event".into(),
            enabled: true,
            hour: 12,
            minute: 0,
            days: [true; 7],
            action: ActionKind::default(),
            path: String::new(),
            arg: 0.0,
        }
    }
}

impl Event {
    pub fn due(&self, now: &DateTime<Local>) -> bool {
        self.enabled && self.hour == now.hour() && self.minute == now.minute() && self.days[now.weekday().num_days_from_monday() as usize]
    }
}
