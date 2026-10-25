//! RadioMaster Server updating itself: asks GitHub for the latest release, downloads the server
//! package for this system, puts the new program in place of the running one and starts again.

use serde::Deserialize;
use std::{io::Read, path::PathBuf, time::Duration};

pub const REPO: &str = "epell-development/RadioMaster";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

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

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version(&self) -> String {
        self.tag_name.trim_start_matches(['v', 'V']).to_string()
    }

    /// The server package for this system (None: there is none, e.g. on macOS).
    pub fn server_asset(&self) -> Option<&Asset> {
        let suffix = package_suffix()?;
        self.assets.iter().find(|a| a.name.starts_with("RadioMaster-Server-") && a.name.ends_with(suffix))
    }
}

/// The end of the server package's name for this system.
fn package_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("-linux-x64.tar.gz"),
        ("windows", "x86_64") => Some("-windows-x64.zip"),
        _ => None,
    }
}

/// The program's file name inside the package.
fn program_name() -> &'static str {
    if cfg!(windows) {
        "RadioMaster-Server.exe"
    } else {
        "RadioMaster-Server"
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(60)).build()
}

/// The latest published release (blocks). RADIOMASTER_UPDATE_URL points it at a test server.
pub fn latest() -> Result<Release, String> {
    let url = std::env::var("RADIOMASTER_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let r: Release = agent()
        .get(&url)
        .set("User-Agent", &format!("RadioMaster-Server/{CURRENT}"))
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "no release has been published yet".to_string(),
            e => e.to_string(),
        })?
        .into_json()
        .map_err(|e| e.to_string())?;
    if r.draft || r.prerelease {
        return Err("the latest release is not final".into());
    }
    Ok(r)
}

/// Download `asset` and put its program in place of the one running (blocks). `progress` gets a
/// short status line now and then. The running program carries on until it is started again.
pub fn install(asset: &Asset, progress: &dyn Fn(String)) -> Result<(), String> {
    let resp = agent()
        .get(&asset.browser_download_url)
        .set("User-Agent", &format!("RadioMaster-Server/{CURRENT}"))
        .call()
        .map_err(|e| e.to_string())?;
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(asset.size).max(1);
    let mut reader = resp.into_reader().take(512 * 1024 * 1024);
    let mut data = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    let mut shown = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let pct = data.len() as u64 * 100 / total;
        if pct >= shown + 10 {
            shown = pct;
            progress(format!("Downloading {}… {pct}%", asset.name));
        }
    }
    progress("Installing…".into());
    let program = if asset.name.ends_with(".zip") { unzip(&data, program_name()) } else { untar_gz(&data, program_name()) }
        .ok_or_else(|| format!("{} has no {} in it", asset.name, program_name()))?;
    let looks_right = if cfg!(windows) { program.starts_with(b"MZ") } else { program.starts_with(b"\x7fELF") };
    if program.len() < 512 * 1024 || !looks_right {
        return Err("the download doesn't look like RadioMaster Server".into());
    }
    replace_program(&program)
}

/// The file called `name` (in any folder) in a .tar.gz.
fn untar_gz(data: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(data).take(1024 * 1024 * 1024).read_to_end(&mut tar).ok()?;
    let mut at = 0;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let field = |r: std::ops::Range<usize>| String::from_utf8_lossy(&h[r]).trim_end_matches(['\0', ' ']).to_string();
        let size = u64::from_str_radix(field(124..136).trim(), 8).ok()? as usize;
        let mut path = field(0..100);
        let prefix = field(345..500);
        if !prefix.is_empty() {
            path = format!("{prefix}/{path}");
        }
        let start = at + 512;
        if h[156] != b'5' && path.rsplit('/').next() == Some(name) {
            return tar.get(start..start + size).map(|b| b.to_vec());
        }
        at = start + size.div_ceil(512) * 512;
    }
    None
}

/// The file called `name` (in any folder) in a .zip, read through its central directory.
fn unzip(data: &[u8], name: &str) -> Option<Vec<u8>> {
    let u16_at = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // the end-of-central-directory record is in the last 64 KB
    let from = data.len().saturating_sub(65_557);
    let eocd = (from..data.len().saturating_sub(21)).rev().find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let (count, mut at) = (u16_at(eocd + 10)?, u32_at(eocd + 16)?);
    for _ in 0..count {
        if data.get(at..at + 4)? != [0x50, 0x4b, 0x01, 0x02] {
            return None;
        }
        let (method, csize, size) = (u16_at(at + 10)?, u32_at(at + 20)?, u32_at(at + 24)?);
        let (nlen, xlen, clen, local) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?, u32_at(at + 42)?);
        let path = String::from_utf8_lossy(data.get(at + 46..at + 46 + nlen)?).replace('\\', "/");
        if path.rsplit('/').next() == Some(name) {
            let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
            let raw = data.get(start..start + csize)?;
            return match method {
                0 => Some(raw.to_vec()),
                8 => {
                    let mut out = Vec::with_capacity(size);
                    flate2::read::DeflateDecoder::new(raw).take(size as u64).read_to_end(&mut out).ok()?;
                    (out.len() == size).then_some(out)
                }
                _ => None,
            };
        }
        at += 46 + nlen + xlen + clen;
    }
    None
}

/// Put `program` where the running program is. (Windows lets a running program be renamed, not
/// overwritten; elsewhere the new file is renamed over the old one.)
fn replace_program(program: &[u8]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let denied = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!(
                "the server's account may not change {} — give it that file (e.g. chown it to the service account) or update by hand",
                exe.display()
            )
        } else {
            format!("{}: {e}", exe.display())
        }
    };
    let new = exe.with_extension("new");
    std::fs::write(&new, program).map_err(denied)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(denied)?;
    }
    if cfg!(windows) {
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(denied)?;
        if let Err(e) = std::fs::rename(&new, &exe) {
            let _ = std::fs::rename(&old, &exe);
            return Err(denied(e));
        }
    } else {
        std::fs::rename(&new, &exe).map_err(denied)?;
    }
    Ok(())
}

/// A service manager starts the server again when it ends with `RESTART_EXIT` (systemd sets
/// INVOCATION_ID; set RADIOMASTER_SUPERVISED=1 for others, such as NSSM).
pub fn supervised() -> bool {
    std::env::var_os("INVOCATION_ID").is_some() || std::env::var("RADIOMASTER_SUPERVISED").is_ok_and(|v| v == "1")
}

/// The exit code that asks the service manager for a restart (any failure code does).
pub const RESTART_EXIT: i32 = 75;

/// Set in a copy started by `start_again`: it waits for the old one to let go of the ports.
pub const RESTARTED_ENV: &str = "RADIOMASTER_RESTARTED";

/// Start a new copy of this program with the same options (this one then exits).
pub fn start_again() -> Result<(), String> {
    let exe: PathBuf = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).args(std::env::args_os().skip(1)).env(RESTARTED_ENV, "1").spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.2.10", "0.2.9"));
        assert!(!is_newer("v0.2.9", "0.2.9"));
        assert!(!is_newer("0.2.0", "0.2.1"));
    }

    #[test]
    fn finds_the_program_in_a_tar_gz() {
        let mut tar = Vec::new();
        for (name, body) in [("pkg/README.md", &b"hello"[..]), ("pkg/RadioMaster-Server", &b"\x7fELF program"[..])] {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
            h[156] = b'0';
            tar.extend_from_slice(&h);
            tar.extend_from_slice(body);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        let data = gz.finish().unwrap();
        assert_eq!(untar_gz(&data, "RadioMaster-Server").unwrap(), b"\x7fELF program");
        assert!(untar_gz(&data, "missing").is_none());
    }

    #[test]
    fn finds_the_program_in_a_zip() {
        // one deflated entry, written the way zip tools lay it out
        let body = b"MZ the program, the program, the program";
        let mut packed = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        packed.write_all(body).unwrap();
        let packed = packed.finish().unwrap();
        let name = b"pkg\\RadioMaster-Server.exe";
        let mut z = Vec::new();
        z.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        z.extend_from_slice(&(body.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(name);
        z.extend_from_slice(&packed);
        let cd = z.len();
        z.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        z.extend_from_slice(&(body.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&[0; 12]);
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(name);
        let cd_len = z.len() - cd;
        z.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend_from_slice(&(cd_len as u32).to_le_bytes());
        z.extend_from_slice(&(cd as u32).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(unzip(&z, "RadioMaster-Server.exe").unwrap(), body);
    }
}
