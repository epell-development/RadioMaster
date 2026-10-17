//! AzuraCast REST API client + background now-playing poller.
//! Public endpoints (now playing) need no key; admin endpoints use `X-API-Key`.

use crate::config::AzuraConfig;
use anyhow::{bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use parking_lot::Mutex;
use serde::Deserialize;
use std::{path::Path, sync::Arc, thread, time::Duration};

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Song {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub text: String,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Entry {
    pub song: Song,
    pub elapsed: f64,
    pub duration: f64,
    pub played_at: i64,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Listeners {
    pub current: u32,
    pub unique: u32,
    pub total: u32,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Live {
    pub is_live: bool,
    pub streamer_name: String,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct StationInfo {
    pub name: String,
    pub shortcode: String,
    pub listen_url: String,
    pub frontend: String,
    pub backend: String,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct NowPlaying {
    pub station: StationInfo,
    pub listeners: Listeners,
    pub live: Live,
    pub is_online: bool,
    pub now_playing: Entry,
    pub playing_next: Option<Entry>,
    pub song_history: Vec<Entry>,
}

#[derive(Default)]
pub struct AzuraState {
    pub np: Option<NowPlaying>,
    pub error: Option<String>,
}

pub struct Client {
    base: String,
    key: String,
    station: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(c: &AzuraConfig) -> Self {
        Client {
            base: c.base_url.trim().trim_end_matches('/').to_string(),
            key: c.api_key.trim().to_string(),
            station: c.station.trim().to_string(),
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build(),
        }
    }

    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = self.agent.request(method, &format!("{}{}", self.base, path));
        if self.key.is_empty() {
            r
        } else {
            r.set("X-API-Key", &self.key)
        }
    }

    pub fn now_playing(&self) -> Result<NowPlaying> {
        Ok(self.req("GET", &format!("/api/nowplaying/{}", self.station)).call()?.into_json()?)
    }

    /// Any API call. `path` is relative to `/api` and may contain `{st}` for the station.
    /// Returns the JSON reply (`Null` for an empty body); errors carry AzuraCast's own message.
    pub fn call(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value> {
        if self.base.is_empty() {
            bail!("set the AzuraCast address first");
        }
        let path = path.replace("{st}", &self.station);
        let r = self.req(method, &format!("/api{path}")).set("Accept", "application/json");
        let res = match body {
            Some(b) => r.send_json(b),
            None => r.call(),
        };
        match res {
            Ok(resp) => {
                let text = resp.into_string()?;
                if text.trim().is_empty() {
                    Ok(serde_json::Value::Null)
                } else {
                    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
                }
            }
            Err(ureq::Error::Status(code, resp)) => {
                let text = resp.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v.get("formatted_message").or(v.get("message")).and_then(|m| m.as_str()).map(String::from))
                    .unwrap_or_else(|| text.chars().take(160).collect());
                match code {
                    401 | 403 => bail!(
                        "access denied ({code}) — check the API key and its station permissions{}",
                        if msg.is_empty() { String::new() } else { format!(": {msg}") }
                    ),
                    _ => bail!("HTTP {code}: {msg}"),
                }
            }
            Err(e) => bail!("{e}"),
        }
    }

    /// Skip the current AutoDJ song (requires an API key with station permissions).
    pub fn skip(&self) -> Result<()> {
        self.req("POST", &format!("/api/station/{}/backend/skip", self.station)).call()?;
        Ok(())
    }

    /// Upload a local audio file into the station's media library.
    pub fn upload(&self, local: &Path, remote_dir: &str) -> Result<()> {
        if self.key.is_empty() {
            bail!("an API key is required to upload");
        }
        let name = local.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let dir = remote_dir.trim_matches('/');
        let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
        let body = serde_json::json!({ "path": path, "file": B64.encode(std::fs::read(local)?) });
        self.req("POST", &format!("/api/station/{}/files", self.station)).send_json(body)?;
        Ok(())
    }
}

pub fn spawn_poller(cfg: Arc<Mutex<AzuraConfig>>, state: Arc<Mutex<AzuraState>>) {
    thread::spawn(move || loop {
        let c = cfg.lock().clone();
        let wait = if c.enabled && !c.base_url.trim().is_empty() && !c.station.trim().is_empty() {
            match Client::new(&c).now_playing() {
                Ok(np) => {
                    let mut s = state.lock();
                    s.np = Some(np);
                    s.error = None;
                }
                Err(e) => state.lock().error = Some(e.to_string()),
            }
            c.poll_secs.max(2)
        } else {
            2
        };
        thread::sleep(Duration::from_secs(wait as u64));
    });
}
