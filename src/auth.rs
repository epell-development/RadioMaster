//! Local operator accounts. Passwords are Argon2id hashed and never written in plaintext.
use crate::config::data_dir;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Operator {
    pub name: String,
    pub password_hash: String,
    pub admin: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Accounts {
    pub users: Vec<Operator>,
}

impl Accounts {
    fn path() -> std::path::PathBuf {
        data_dir().join("operators.json")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn add(&mut self, name: &str, password: &str, admin: bool) -> Result<(), String> {
        let name = name.trim();
        if name.len() < 2 || name.len() > 40 {
            return Err("Username must contain 2–40 characters".into());
        }
        if password.len() < 12 {
            return Err("Use a password of at least 12 characters".into());
        }
        if self.users.iter().any(|u| u.name.eq_ignore_ascii_case(name)) {
            return Err("User already exists".into());
        }
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default().hash_password(password.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string();
        self.users.push(Operator { name: name.into(), password_hash: hash, admin });
        if let Err(e) = self.save() {
            self.users.pop();
            return Err(e);
        }
        Ok(())
    }
    pub fn login(&self, name: &str, password: &str) -> Option<Operator> {
        self.users
            .iter()
            .find(|u| {
                u.name == name
                    && PasswordHash::new(&u.password_hash)
                        .ok()
                        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
                        .unwrap_or(false)
            })
            .cloned()
    }
}
