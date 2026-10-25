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
    /// put a playlist item (time announcement, speech, stream, track list…) in as the next row
    /// (`Event::item`; `Event::play_now` = start it straight away)
    InsertItem,
}

impl ActionKind {
    pub const ALL: [ActionKind; 14] = [
        ActionKind::InsertItem,
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
            ActionKind::InsertItem => "Insert a playlist item (time announcement, speech, stream…)",
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
    /// 0 = once at hour:minute; otherwise again every this many minutes until `until`
    pub repeat_mins: u32,
    /// (hour, minute) the repeats stop at (inclusive; before the start = runs over midnight)
    pub until: (u32, u32),
    /// Monday..Sunday
    pub days: [bool; 7],
    pub action: ActionKind,
    pub path: String,
    pub arg: f32,
    /// the row `ActionKind::InsertItem` puts in
    pub item: Option<crate::items::PlEntry>,
    /// `ActionKind::InsertItem`: start it now instead of after the track on air
    pub play_now: bool,
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
            repeat_mins: 0,
            until: (23, 59),
            days: [true; 7],
            action: ActionKind::GenerateReplace,
            path: String::new(),
            arg: 0.0,
            item: None,
            play_now: false,
            from_date: String::new(),
            to_date: String::new(),
        }
    }
}

impl Event {
    pub fn due(&self, now: &DateTime<Local>) -> bool {
        self.on_day(now) && self.at_time(now.hour(), now.minute())
    }

    /// Enabled, and today is one of its days inside its dates.
    pub fn on_day(&self, now: &DateTime<Local>) -> bool {
        let today = now.format("%Y-%m-%d").to_string();
        self.enabled
            && self.days[now.weekday().num_days_from_monday() as usize]
            && (self.from_date.trim().is_empty() || today.as_str() >= self.from_date.trim())
            && (self.to_date.trim().is_empty() || today.as_str() <= self.to_date.trim())
    }

    /// It goes off at `h:m` (ignoring days and dates).
    pub fn at_time(&self, h: u32, m: u32) -> bool {
        let start = self.hour * 60 + self.minute;
        let now = h * 60 + m;
        if self.repeat_mins == 0 {
            return now == start;
        }
        let end = self.until.0.min(23) * 60 + self.until.1.min(59);
        let off = (now + 1440 - start) % 1440;
        let span = if end == start { 1439 } else { (end + 1440 - start) % 1440 };
        off <= span && off % self.repeat_mins == 0
    }

    /// "at 12:00", or "every 60 min from 06:00 to 22:00".
    pub fn when_text(&self) -> String {
        if self.repeat_mins == 0 {
            return format!("at {:02}:{:02}", self.hour, self.minute);
        }
        let m = self.repeat_mins;
        let every = if m % 60 == 0 { format!("{} h", m / 60) } else { format!("{m} min") };
        format!("every {every} from {:02}:{:02} to {:02}:{:02}", self.hour, self.minute, self.until.0, self.until.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_and_repeating_times() {
        let once = Event { hour: 12, minute: 30, ..Default::default() };
        assert!(once.at_time(12, 30));
        assert!(!once.at_time(13, 30));

        // every hour on the hour, all day
        let hourly = Event { hour: 0, minute: 0, repeat_mins: 60, until: (23, 59), ..Default::default() };
        assert!((0..24).all(|h| hourly.at_time(h, 0)));
        assert!(!hourly.at_time(10, 30));

        // every 15 minutes from 06:00 to 09:00
        let morning = Event { hour: 6, minute: 0, repeat_mins: 15, until: (9, 0), ..Default::default() };
        assert!(morning.at_time(6, 0) && morning.at_time(6, 45) && morning.at_time(9, 0));
        assert!(!morning.at_time(5, 45) && !morning.at_time(9, 15) && !morning.at_time(7, 10));

        // every 30 minutes from 22:00 over midnight to 02:00
        let night = Event { hour: 22, minute: 0, repeat_mins: 30, until: (2, 0), ..Default::default() };
        assert!(night.at_time(22, 0) && night.at_time(23, 30) && night.at_time(0, 0) && night.at_time(2, 0));
        assert!(!night.at_time(2, 30) && !night.at_time(12, 0));
    }

    #[test]
    fn old_events_load_as_once() {
        let e: Event = serde_json::from_str(r#"{"name":"News","hour":9,"minute":0,"action":"Stop"}"#).unwrap();
        assert_eq!(e.repeat_mins, 0);
        assert!(e.at_time(9, 0) && !e.at_time(10, 0));
    }
}
