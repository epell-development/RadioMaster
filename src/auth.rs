//! Local operator accounts. Passwords are Argon2id hashed and never written in plaintext.
use crate::config::data_dir;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use serde::{Deserialize, Serialize};

/// What an operator may do. Administrators may do everything, including managing users.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct Perms {
    /// play / pause / stop / next, microphone
    pub playback: bool,
    /// cart wall and the Jingles menu
    pub jingles: bool,
    /// add, remove, move and edit playlist rows; playlist tabs
    pub edit_playlists: bool,
    /// (before v0.2.4: generator, adverts, scheduler, track editor — now per tool in `denied`)
    pub tools: bool,
    /// start / stop / change stream encoders
    pub streaming: bool,
    /// (before v0.2.4: the AzuraCast tool — now per tool in `denied`)
    pub azuracast: bool,
    /// settings, audio devices, station profiles
    pub settings: bool,
    /// tools this account can't open: bit `i` = `TOOLS[i]`
    pub denied: u32,
    /// 0 = made before per-tool access (migrated on load)
    #[serde(default)]
    pub v: u8,
}

impl Default for Perms {
    /// Accounts made before permissions existed keep full access.
    fn default() -> Self {
        Perms {
            playback: true,
            jingles: true,
            edit_playlists: true,
            tools: true,
            streaming: true,
            azuracast: true,
            settings: true,
            denied: 0,
            v: 2,
        }
    }
}

/// Tools an administrator can allow or block one by one.
pub mod tool {
    pub const TRACK_EDITOR: usize = 0;
    pub const GENERATOR: usize = 1;
    pub const ADS: usize = 2;
    pub const AUDIO_FX: usize = 3;
    pub const AZURACAST: usize = 4;
    pub const REPORTS: usize = 5;
    pub const HISTORY: usize = 6;
    pub const NOTEPAD: usize = 7;
    pub const PLUGINS: usize = 8;
    pub const SCHEDULER: usize = 9;
    pub const VOICE_TRACK: usize = 10;
    pub const LIBRARY: usize = 11;
    pub const FILE_EXPLORER: usize = 12;
    pub const SONG_REQUESTS: usize = 13;
    pub const WEB: usize = 14;
    pub const LIBRARY_CLEANUP: usize = 15;
}

pub const TOOLS: [(&str, &str); 16] = [
    ("Track editor", "Tags, cue points and fades"),
    ("Playlist generator", "Make playlists from categories and rules"),
    ("Advertisement scheduler", "Adverts, ad breaks and the every-N-songs rules"),
    ("Audio processing (Audio FX)", "EQ, compressor, limiter and silence trimming"),
    ("AzuraCast station manager", "Everything in the AzuraCast tool"),
    ("Reports", "Play, advert and jingle reports and exports"),
    ("Play history", "The list of what has played"),
    ("Notepad", "The notepad and segment notes"),
    ("Plugins", "Turn plugins on and off, open the plugins folder"),
    ("Event scheduler", "Timed events (the Scheduler tab)"),
    ("Voice track recorder", "Record voice tracks into the playlist"),
    ("Library", "The music library tab"),
    ("File explorer", "Browse the computer's folders"),
    ("Song requests", "The song requests tab"),
    ("Web panels", "Web pages inside RadioMaster"),
    ("Library cleanup", "Remove duplicates, fix lost tracks, tidy genre and artist tags"),
];

impl Perms {
    /// A sensible starting point for a new presenter account.
    pub fn presenter() -> Self {
        use tool::*;
        let mut p = Perms { streaming: false, settings: false, ..Default::default() };
        for t in [GENERATOR, ADS, AUDIO_FX, AZURACAST, PLUGINS, SCHEDULER, LIBRARY_CLEANUP] {
            p.set_tool(t, false);
        }
        p
    }
    /// The on/off permissions (not the tools).
    pub const LIST: [(&'static str, &'static str); 5] = [
        ("Playback", "Play, pause, stop, next, microphone"),
        ("Jingles", "Cart wall and the Jingles menu"),
        ("Edit playlists", "Add, remove, move and edit playlist rows and tabs"),
        ("Streaming", "Start, stop and change stream encoders"),
        ("Settings", "Settings, audio devices, station profiles"),
    ];
    pub fn get_mut(&mut self, k: usize) -> &mut bool {
        match k {
            0 => &mut self.playback,
            1 => &mut self.jingles,
            2 => &mut self.edit_playlists,
            3 => &mut self.streaming,
            _ => &mut self.settings,
        }
    }
    pub fn tool(&self, t: usize) -> bool {
        self.denied & (1 << t) == 0
    }
    pub fn set_tool(&mut self, t: usize, allowed: bool) {
        if allowed {
            self.denied &= !(1 << t);
        } else {
            self.denied |= 1 << t;
        }
    }
    pub fn tools_allowed(&self) -> usize {
        (0..TOOLS.len()).filter(|&t| self.tool(t)).count()
    }
    /// Accounts from before per-tool access: carry the old group switches over.
    fn migrate(&mut self) {
        use tool::*;
        if self.v >= 1 {
            // v2 added Library cleanup (it deletes files and rewrites tags): accounts that can't
            // change settings don't get it until an administrator allows it
            if self.v < 2 && !self.settings {
                self.set_tool(LIBRARY_CLEANUP, false);
            }
            self.v = self.v.max(2);
            return;
        }
        if !self.tools {
            for t in [TRACK_EDITOR, GENERATOR, ADS, SCHEDULER] {
                self.set_tool(t, false);
            }
        }
        if !self.azuracast {
            self.set_tool(AZURACAST, false);
        }
        if !self.settings {
            for t in [AUDIO_FX, PLUGINS] {
                self.set_tool(t, false);
            }
        }
        if !self.settings {
            self.set_tool(LIBRARY_CLEANUP, false);
        }
        self.tools = true;
        self.azuracast = true;
        self.v = 2;
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Operator {
    pub name: String,
    pub password_hash: String,
    pub admin: bool,
    #[serde(default)]
    pub perms: Perms,
    /// a disabled account can't sign in
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Accounts {
    pub users: Vec<Operator>,
}

pub fn hash(password: &str) -> Result<String, String> {
    if password.len() < 12 {
        return Err("Use a password of at least 12 characters".into());
    }
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string())
}

/// Check a password against an Argon2 hash made by `hash`.
pub fn verify(password_hash: &str, password: &str) -> bool {
    PasswordHash::new(password_hash).map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()).unwrap_or(false)
}

impl Accounts {
    fn path() -> std::path::PathBuf {
        data_dir().join("operators.json")
    }
    pub fn load() -> Self {
        let mut a: Self = std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for u in a.users.iter_mut() {
            u.perms.migrate();
        }
        a
    }
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn add(&mut self, name: &str, password: &str, admin: bool) -> Result<(), String> {
        self.add_with(name, password, admin, if admin { Perms::default() } else { Perms::presenter() })
    }
    pub fn add_with(&mut self, name: &str, password: &str, admin: bool, perms: Perms) -> Result<(), String> {
        let name = name.trim();
        if name.len() < 2 || name.len() > 40 {
            return Err("Username must contain 2–40 characters".into());
        }
        if self.users.iter().any(|u| u.name.eq_ignore_ascii_case(name)) {
            return Err("User already exists".into());
        }
        let h = hash(password)?;
        self.users.push(Operator { name: name.into(), password_hash: h, admin, perms, disabled: false });
        if let Err(e) = self.save() {
            self.users.pop();
            return Err(e);
        }
        Ok(())
    }
    fn admins(&self) -> usize {
        self.users.iter().filter(|u| u.admin && !u.disabled).count()
    }
    /// Change role, permissions or the disabled flag. The last active administrator can't lose that.
    pub fn update(&mut self, name: &str, admin: bool, perms: Perms, disabled: bool) -> Result<(), String> {
        let lone_admin = self.admins() == 1;
        let u = self.users.iter_mut().find(|u| u.name == name).ok_or("No such user")?;
        if u.admin && !u.disabled && (!admin || disabled) && lone_admin {
            return Err("At least one active administrator is needed".into());
        }
        u.admin = admin;
        u.perms = perms;
        u.disabled = disabled;
        self.save()
    }
    pub fn set_password(&mut self, name: &str, password: &str) -> Result<(), String> {
        let h = hash(password)?;
        let u = self.users.iter_mut().find(|u| u.name == name).ok_or("No such user")?;
        u.password_hash = h;
        self.save()
    }
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let i = self.users.iter().position(|u| u.name == name).ok_or("No such user")?;
        if self.users[i].admin && !self.users[i].disabled && self.admins() == 1 {
            return Err("The last administrator can't be deleted".into());
        }
        self.users.remove(i);
        self.save()
    }
    pub fn login(&self, name: &str, password: &str) -> Option<Operator> {
        self.users.iter().find(|u| !u.disabled && u.name == name && verify(&u.password_hash, password)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_permissions_carry_over_to_tools() {
        let old = r#"{"playback":true,"edit_playlists":true,"tools":false,"streaming":false,"azuracast":false,"settings":false}"#;
        let mut p: Perms = serde_json::from_str(old).unwrap();
        assert_eq!(p.v, 0);
        p.migrate();
        assert!(p.jingles, "jingles default to allowed");
        assert!(!p.tool(tool::TRACK_EDITOR) && !p.tool(tool::ADS) && !p.tool(tool::AZURACAST) && !p.tool(tool::AUDIO_FX));
        assert!(p.tool(tool::REPORTS) && p.tool(tool::NOTEPAD) && p.tool(tool::LIBRARY));
        let full: Perms = serde_json::from_str("{}").unwrap();
        let mut f = full;
        f.migrate();
        assert_eq!(f.denied, 0);
        let pr = Perms::presenter();
        assert!(pr.tool(tool::TRACK_EDITOR) && !pr.tool(tool::AUDIO_FX) && pr.edit_playlists && !pr.settings);
        assert!(!pr.tool(tool::LIBRARY_CLEANUP));
    }

    #[test]
    fn library_cleanup_needs_settings_access() {
        let mut presenter: Perms = serde_json::from_str(r#"{"settings":false,"denied":0,"v":1}"#).unwrap();
        presenter.migrate();
        assert!(!presenter.tool(tool::LIBRARY_CLEANUP) && presenter.tool(tool::TRACK_EDITOR));
        let mut manager: Perms = serde_json::from_str(r#"{"settings":true,"denied":0,"v":1}"#).unwrap();
        manager.migrate();
        assert!(manager.tool(tool::LIBRARY_CLEANUP));
        // an administrator's later choice sticks
        let mut allowed: Perms = serde_json::from_str(r#"{"settings":false,"denied":0,"v":2}"#).unwrap();
        allowed.migrate();
        assert!(allowed.tool(tool::LIBRARY_CLEANUP));
    }
}
