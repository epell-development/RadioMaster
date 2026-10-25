//! Optional Cloudflare Tunnel for RadioMaster Server: puts the web portal (and the API, which the
//! portal port also serves) on the internet through Cloudflare, with no port forwarding and no
//! certificate to look after.
//!
//! The server runs Cloudflare's own `cloudflared` program and starts it again if it stops. With a
//! tunnel token (made in the Cloudflare dashboard, under Zero Trust → Networks → Tunnels) the tunnel
//! uses your own domain name; without one, Cloudflare gives a random *.trycloudflare.com address that
//! changes each time (handy for trying it out).

use parking_lot::Mutex;
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Default, Debug)]
pub struct Status {
    /// "", "starting", "running" or "error"
    pub state: String,
    /// the public address, when cloudflared said it (quick tunnels)
    pub url: String,
    pub message: String,
}

pub type Shared = Arc<Mutex<Status>>;

/// cloudflared: the one given, else next to the server, else on the PATH.
pub fn find(given: &str) -> PathBuf {
    if !given.trim().is_empty() {
        return PathBuf::from(given.trim());
    }
    let exe = if cfg!(windows) { "cloudflared.exe" } else { "cloudflared" };
    if let Some(here) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe))) {
        if here.exists() {
            return here;
        }
    }
    PathBuf::from(exe)
}

/// A running tunnel; stopped when dropped.
pub struct Tunnel {
    child: Arc<Mutex<Option<Child>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Start cloudflared for `origin` (e.g. http://127.0.0.1:9260) and keep it running.
pub fn start(program: PathBuf, token: String, origin: String, status: Shared, say: Arc<dyn Fn(String) + Send + Sync>) -> Tunnel {
    let child: Arc<Mutex<Option<Child>>> = Default::default();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let t = Tunnel { child: child.clone(), stop: stop.clone() };
    thread::spawn(move || {
        let mut backoff = 2u64;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            *status.lock() = Status { state: "starting".into(), ..Default::default() };
            let mut cmd = Command::new(&program);
            cmd.args(["tunnel", "--no-autoupdate"]);
            if token.trim().is_empty() {
                cmd.args(["--url", &origin]);
            } else {
                // the token goes in the environment, so it doesn't show in the process list
                cmd.arg("run").env("TUNNEL_TOKEN", token.trim());
            }
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let started = Instant::now();
            match cmd.spawn() {
                Ok(mut c) => {
                    let err = c.stderr.take();
                    *child.lock() = Some(c);
                    say(format!("Cloudflare Tunnel: started {}", program.display()));
                    // cloudflared logs to stderr: pick out the address and whether it connected
                    if let Some(err) = err {
                        for line in BufReader::new(err).lines().map_while(Result::ok) {
                            if let Some(url) = quick_url(&line) {
                                say(format!("Cloudflare Tunnel: {url}"));
                                let mut s = status.lock();
                                s.url = url;
                                s.state = "running".into();
                            } else if line.contains("Registered tunnel connection") {
                                let mut s = status.lock();
                                if s.state != "running" {
                                    say("Cloudflare Tunnel: connected".into());
                                }
                                s.state = "running".into();
                            } else if line.contains(" ERR ") {
                                status.lock().message = line.split(" ERR ").nth(1).unwrap_or(&line).trim().chars().take(200).collect();
                            }
                        }
                    }
                    let code = child.lock().take().and_then(|mut c| c.wait().ok());
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let msg = format!("cloudflared stopped ({})", code.map(|c| c.to_string()).unwrap_or_else(|| "killed".into()));
                    say(format!("Cloudflare Tunnel: {msg}; starting it again"));
                    let mut s = status.lock();
                    s.state = "error".into();
                    if s.message.is_empty() {
                        s.message = msg;
                    }
                }
                Err(e) => {
                    let msg = format!("can't start {}: {e} (install cloudflared, or give its path)", program.display());
                    say(format!("Cloudflare Tunnel: {msg}"));
                    *status.lock() = Status { state: "error".into(), url: String::new(), message: msg };
                }
            }
            // ran for a good while: it was a blip, start again quickly
            if started.elapsed() > Duration::from_secs(300) {
                backoff = 2;
            }
            for _ in 0..backoff {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
            backoff = (backoff * 2).min(120);
        }
    });
    t
}

/// The https://….trycloudflare.com address in a cloudflared log line.
fn quick_url(line: &str) -> Option<String> {
    let at = line.find("https://")?;
    let url: String = line[at..].chars().take_while(|c| !c.is_whitespace() && *c != '|').collect();
    url.trim_end_matches('/').ends_with(".trycloudflare.com").then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_quick_tunnel_address() {
        let l = "2026-10-25T10:00:00Z INF |  https://calm-river-1234.trycloudflare.com                                 |";
        assert_eq!(quick_url(l).unwrap(), "https://calm-river-1234.trycloudflare.com");
        assert!(quick_url("INF Visit https://www.cloudflare.com/ for help").is_none());
    }

    /// A program that isn't there: the status says so instead of the server failing.
    #[test]
    fn missing_program_is_reported() {
        let status: Shared = Default::default();
        let _t =
            start(PathBuf::from("/nonexistent/cloudflared"), String::new(), "http://127.0.0.1:1".into(), status.clone(), Arc::new(|_| {}));
        let t = Instant::now();
        while status.lock().state != "error" {
            assert!(t.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(20));
        }
        assert!(status.lock().message.contains("can't start"));
    }
}
