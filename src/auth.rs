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
    /// play / pause / stop / next, cart wall, microphone
    pub playback: bool,
    /// add, remove, move and edit playlist rows; playlist tabs
    pub edit_playlists: bool,
    /// generator, adverts, scheduler, track editor (tags and cues)
    pub tools: bool,
    /// start / stop / change stream encoders
    pub streaming: bool,
    /// AzuraCast station manager
    pub azuracast: bool,
    /// settings, audio devices, audio processing, station profiles
    pub settings: bool,
}

impl Default for Perms {
    /// Accounts made before permissions existed keep full access.
    fn default() -> Self {
        Perms { playback: true, edit_playlists: true, tools: true, streaming: true, azuracast: true, settings: true }
    }
}

impl Perms {
    /// A sensible starting point for a new presenter account.
    pub fn presenter() -> Self {
        Perms { playback: true, edit_playlists: true, tools: false, streaming: false, azuracast: false, settings: false }
    }
    pub const LIST: [(&'static str, &'static str); 6] = [
        ("Playback", "Play, pause, stop, next, cart wall, microphone"),
        ("Edit playlists", "Add, remove, move and edit playlist rows and tabs"),
        ("Tools", "Generator, adverts, scheduler, track editor"),
        ("Streaming", "Start, stop and change stream encoders"),
        ("AzuraCast", "The AzuraCast station manager"),
        ("Settings", "Settings, audio devices and processing, station profiles"),
    ];
    pub fn get_mut(&mut self, k: usize) -> &mut bool {
        match k {
            0 => &mut self.playback,
            1 => &mut self.edit_playlists,
            2 => &mut self.tools,
            3 => &mut self.streaming,
            4 => &mut self.azuracast,
            _ => &mut self.settings,
        }
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

fn hash(password: &str) -> Result<String, String> {
    if password.len() < 12 {
        return Err("Use a password of at least 12 characters".into());
    }
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string())
}

impl Accounts {
    fn path() -> std::path::PathBuf {
        data_dir().join("operators.json")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
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
        self.users
            .iter()
            .find(|u| {
                !u.disabled
                    && u.name == name
                    && PasswordHash::new(&u.password_hash)
                        .ok()
                        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
                        .unwrap_or(false)
            })
            .cloned()
    }
}
