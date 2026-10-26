//! Update check: asks GitHub for the latest RadioMaster release and, when it's newer than this
//! build, offers it. Installed copies download the setup .exe and run it; portable copies open
//! the release page.

use parking_lot::Mutex;
use serde::Deserialize;
use std::{io::Read, path::PathBuf, sync::Arc, thread, time::Duration};

pub const REPO: &str = "epell-development/RadioMaster";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Release {
    tag_name: String,
    name: String,
    body: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug)]
pub struct Available {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub page: String,
    pub installer: Option<(String, u64)>,
}

#[derive(Clone, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Failed(String),
    Downloading {
        done: u64,
        total: u64,
    },
    Ready(PathBuf),
}

#[derive(Default)]
pub struct Updates {
    pub available: Option<Available>,
    pub phase: Phase,
    /// the user asked to check (show "up to date" / errors)
    pub manual: bool,
}

pub type Shared = Arc<Mutex<Updates>>;

/// "v0.2.10" → [0, 2, 10]
pub fn parse_version(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    let (mut a, mut b) = (parse_version(latest), parse_version(current));
    a.resize(3, 0);
    b.resize(3, 0);
    a > b
}

/// True when this copy was put there by the installer (it has an uninstaller next to it).
pub fn installed_copy() -> bool {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("unins000.exe").exists())).unwrap_or(false)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(60)).build()
}

/// Check GitHub in the background; `repaint` is called when the answer is in.
pub fn check(state: Shared, repaint: impl Fn() + Send + 'static, manual: bool) {
    {
        let mut s = state.lock();
        if matches!(s.phase, Phase::Checking | Phase::Downloading { .. }) {
            return;
        }
        s.phase = Phase::Checking;
        s.manual = manual;
    }
    thread::spawn(move || {
        let r = agent()
            // RADIOMASTER_UPDATE_URL points the check at a test server
            .get(
                &std::env::var("RADIOMASTER_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest")),
            )
            .set("User-Agent", &format!("RadioMaster/{CURRENT}"))
            .set("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(404, _) => "no release has been published yet".to_string(),
                e => e.to_string(),
            })
            .and_then(|r| r.into_json::<Release>().map_err(|e| e.to_string()));
        let mut s = state.lock();
        match r {
            Ok(rel) if !rel.draft && !rel.prerelease && is_newer(&rel.tag_name, CURRENT) => {
                let installer = rel
                    .assets
                    .iter()
                    .find(|a| a.name.starts_with("RadioMaster-Setup") && a.name.ends_with(".exe"))
                    .map(|a| (a.browser_download_url.clone(), a.size));
                s.available = Some(Available {
                    version: rel.tag_name.trim_start_matches('v').to_string(),
                    title: if rel.name.is_empty() { rel.tag_name.clone() } else { rel.name.clone() },
                    notes: rel.body.replace("\r\n", "\n"),
                    page: rel.html_url,
                    installer,
                });
                s.phase = Phase::Idle;
            }
            Ok(_) => {
                s.available = None;
                s.phase = Phase::UpToDate;
            }
            Err(e) => s.phase = Phase::Failed(e),
        }
        drop(s);
        repaint();
    });
}

/// Download the installer to the temp folder, reporting progress.
pub fn download(state: Shared, repaint: impl Fn() + Send + 'static) {
    let Some((url, size)) = state.lock().available.as_ref().and_then(|a| a.installer.clone()) else { return };
    let version = state.lock().available.as_ref().map(|a| a.version.clone()).unwrap_or_default();
    state.lock().phase = Phase::Downloading { done: 0, total: size };
    thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let resp = agent().get(&url).set("User-Agent", &format!("RadioMaster/{CURRENT}")).call().map_err(|e| e.to_string())?;
            let total = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(size);
            let mut reader = resp.into_reader();
            let mut data = Vec::with_capacity(total as usize);
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                state.lock().phase = Phase::Downloading { done: data.len() as u64, total };
                repaint();
            }
            if data.len() < 1024 * 100 || &data[..2] != b"MZ" {
                return Err("the download doesn't look like a Windows installer".into());
            }
            let path = std::env::temp_dir().join(format!("RadioMaster-Setup-{version}.exe"));
            std::fs::write(&path, &data).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        state.lock().phase = match result {
            Ok(p) => Phase::Ready(p),
            Err(e) => Phase::Failed(format!("Download failed: {e}")),
        };
        repaint();
    });
}

/// Start the installer (it closes RadioMaster, updates it and starts it again).
pub fn run_installer(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new(path)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/RELAUNCH=1"])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.2.10", "0.2.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.1"));
        assert!(is_newer("v0.3", "0.2.9"));
    }
}
