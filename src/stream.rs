//! Source client for Icecast (PUT, as used by modern Icecast and AzuraCast) and
//! Shoutcast v2 DJ ("OK2") connections, with a built-in LAME MP3 encoder.

use crate::config::{EncoderConfig, ServerKind};
use crate::engine::{Engine, SR};
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use mp3lame_encoder::{Bitrate, Builder, InterleavedPcm, Quality};
use parking_lot::Mutex;
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
pub enum EncState {
    Idle,
    Connecting,
    Live,
    Error(String),
}

pub struct EncStatus {
    pub state: EncState,
    pub bytes: u64,
    pub reconnects: u32,
}

pub struct EncoderHandle {
    stop: Arc<AtomicBool>,
    pub running: Arc<AtomicBool>,
    pub status: Arc<Mutex<EncStatus>>,
}

impl EncoderHandle {
    pub fn start(cfg: EncoderConfig, engine: Engine, song: Arc<Mutex<String>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let status = Arc::new(Mutex::new(EncStatus { state: EncState::Connecting, bytes: 0, reconnects: 0 }));
        let (s2, r2, st2) = (stop.clone(), running.clone(), status.clone());
        thread::spawn(move || {
            let mut first = true;
            let mut put_first = false;
            let mut wait_secs = 3u64;
            while !s2.load(Ordering::Relaxed) {
                {
                    let mut st = st2.lock();
                    st.state = EncState::Connecting;
                    if !first {
                        st.reconnects += 1;
                    }
                }
                first = false;
                let (tx, rx) = bounded::<Arc<Vec<i16>>>(512);
                engine.add_tap(tx);
                let started = std::time::Instant::now();
                match run(&cfg, &rx, &s2, &st2, &song, &mut put_first) {
                    Ok(()) => break,
                    Err(e) => st2.lock().state = EncState::Error(e.to_string()),
                }
                drop(rx);
                // back off when the server keeps dropping us straight away, retry quickly after a long session
                wait_secs = if started.elapsed() > Duration::from_secs(60) { 3 } else { (wait_secs * 2).min(30) };
                for _ in 0..wait_secs * 10 {
                    if s2.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            st2.lock().state = EncState::Idle;
            r2.store(false, Ordering::Relaxed);
        });
        EncoderHandle { stop, running, status }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

fn bitrate(k: u32) -> Bitrate {
    match k {
        0..=64 => Bitrate::Kbps64,
        65..=96 => Bitrate::Kbps96,
        97..=128 => Bitrate::Kbps128,
        129..=160 => Bitrate::Kbps160,
        161..=192 => Bitrate::Kbps192,
        193..=256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    }
}

fn norm_mount(m: &str) -> String {
    if m.starts_with('/') {
        m.to_string()
    } else {
        format!("/{m}")
    }
}

fn read_headers(s: &mut TcpStream) -> Result<String> {
    let mut buf = Vec::new();
    let mut b = [0u8; 1];
    while buf.len() < 8192 {
        if s.read(&mut b)? == 0 {
            break;
        }
        buf.push(b[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn open_socket(cfg: &EncoderConfig) -> Result<TcpStream> {
    let addr = (cfg.host.trim(), cfg.port).to_socket_addrs()?.next().ok_or_else(|| anyhow!("cannot resolve {}", cfg.host))?;
    let s =
        TcpStream::connect_timeout(&addr, Duration::from_secs(8)).map_err(|e| anyhow!("can't reach {}:{} ({e})", cfg.host, cfg.port))?;
    s.set_read_timeout(Some(Duration::from_secs(8)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    s.set_nodelay(true).ok();
    Ok(s)
}

/// Icecast-style login. `SOURCE` (HTTP/1.0) is what Icecast 2, Liquidsoap/AzuraCast DJ ports and
/// BUTT all speak; `PUT` (HTTP/1.1) is the newer Icecast form, used if a server refuses SOURCE.
fn icecast_login(cfg: &EncoderConfig, method: &str) -> Result<TcpStream> {
    let mut s = open_socket(cfg)?;
    let kbps = cfg.bitrate_kbps;
    let auth = B64.encode(format!("{}:{}", cfg.username, cfg.password));
    let (proto, expect) = if method == "PUT" { ("HTTP/1.1", "Expect: 100-continue\r\n") } else { ("HTTP/1.0", "") };
    let req = format!(
        "{method} {mount} {proto}\r\nHost: {host}:{port}\r\nAuthorization: Basic {auth}\r\n\
         User-Agent: RadioMaster/{ver}\r\nContent-Type: audio/mpeg\r\n\
         ice-name: {name}\r\nice-public: {public}\r\nice-description: {desc}\r\n\
         ice-genre: {genre}\r\nice-url: {url}\r\nice-bitrate: {kbps}\r\n\
         ice-audio-info: ice-samplerate={SR};ice-bitrate={kbps};ice-channels=2\r\n{expect}\r\n",
        mount = norm_mount(&cfg.mount),
        host = cfg.host.trim(),
        port = cfg.port,
        ver = env!("CARGO_PKG_VERSION"),
        name = cfg.stream_name,
        public = cfg.public as u8,
        desc = cfg.description,
        genre = cfg.genre,
        url = cfg.url,
    );
    s.write_all(req.as_bytes())?;
    let resp = read_headers(&mut s)?;
    let line = resp.lines().next().unwrap_or("").to_string();
    if line.contains(" 100 ") || line.contains(" 200 ") {
        Ok(s)
    } else if line.contains("401") {
        bail!("the server rejected the username/password (401)")
    } else if line.contains("403") {
        bail!("mount point in use or not allowed (403) — is another source already connected? {line}")
    } else if line.is_empty() {
        bail!("no reply from the server")
    } else {
        bail!("server replied: {line}")
    }
}

fn connect(cfg: &EncoderConfig, put_first: &mut bool) -> Result<TcpStream> {
    match cfg.kind {
        ServerKind::Icecast => {
            let order: [&str; 2] = if *put_first { ["PUT", "SOURCE"] } else { ["SOURCE", "PUT"] };
            match icecast_login(cfg, order[0]) {
                Ok(s) => Ok(s),
                // a wrong password won't get better with the other method
                Err(e) if e.to_string().contains("401") || e.to_string().contains("can't reach") => Err(e),
                Err(first) => match icecast_login(cfg, order[1]) {
                    Ok(s) => {
                        *put_first = !*put_first;
                        Ok(s)
                    }
                    Err(_) => Err(first),
                },
            }
        }
        ServerKind::Shoutcast => {
            let mut s = open_socket(cfg)?;
            let kbps = cfg.bitrate_kbps;
            let login =
                if cfg.username.is_empty() { format!("{}\r\n", cfg.password) } else { format!("{}:{}\r\n", cfg.username, cfg.password) };
            s.write_all(login.as_bytes())?;
            let resp = read_headers(&mut s)?;
            if !resp.contains("OK2") {
                bail!("Shoutcast login failed: {}", resp.lines().next().unwrap_or("no reply"));
            }
            let hdr = format!(
                "icy-name:{}\r\nicy-genre:{}\r\nicy-url:{}\r\nicy-pub:{}\r\nicy-br:{}\r\ncontent-type:audio/mpeg\r\n\r\n",
                cfg.stream_name, cfg.genre, cfg.url, cfg.public as u8, kbps
            );
            s.write_all(hdr.as_bytes())?;
            Ok(s)
        }
    }
}

fn run(
    cfg: &EncoderConfig,
    rx: &Receiver<Arc<Vec<i16>>>,
    stop: &AtomicBool,
    status: &Mutex<EncStatus>,
    song: &Mutex<String>,
    put_first: &mut bool,
) -> Result<()> {
    let mut sock = connect(cfg, put_first)?;
    let connected = std::time::Instant::now();
    let mut b = Builder::new().ok_or_else(|| anyhow!("LAME init failed"))?;
    b.set_num_channels(2).map_err(|e| anyhow!("{e:?}"))?;
    b.set_sample_rate(SR).map_err(|e| anyhow!("{e:?}"))?;
    b.set_brate(bitrate(cfg.bitrate_kbps)).map_err(|e| anyhow!("{e:?}"))?;
    b.set_quality(Quality::Good).map_err(|e| anyhow!("{e:?}"))?;
    let mut enc = b.build().map_err(|e| anyhow!("{e:?}"))?;

    while rx.try_recv().is_ok() {} // drop audio buffered while connecting
    {
        let mut st = status.lock();
        st.state = EncState::Live;
        st.bytes = 0;
    }
    if cfg.send_metadata {
        let (c, t) = (cfg.clone(), song.lock().clone());
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1500));
            if !t.is_empty() {
                let _ = send_metadata(&c, &t);
            }
        });
    }

    let mut out: Vec<u8> = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(pcm) => {
                out.clear();
                out.reserve(mp3lame_encoder::max_required_buffer_size(pcm.len()));
                let n = enc.encode(InterleavedPcm(&pcm[..]), out.spare_capacity_mut()).map_err(|e| anyhow!("encode: {e:?}"))?;
                unsafe { out.set_len(n) };
                if n > 0 {
                    if let Err(e) = sock.write_all(&out) {
                        // 10053 / 10054 / broken pipe: the server dropped us
                        let early = connected.elapsed() < Duration::from_secs(15);
                        bail!(
                            "{} ({e}) — reconnecting{}",
                            if early { "the server closed the connection right after login" } else { "connection lost" },
                            if early { ". Check the mount point and that no other source/DJ is connected" } else { "" }
                        );
                    }
                    status.lock().bytes += n as u64;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("audio engine stopped"),
        }
    }
}

/// Update the "now playing" text on the server.
pub fn send_metadata(cfg: &EncoderConfig, song: &str) -> Result<()> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(6)).build();
    let auth = format!("Basic {}", B64.encode(format!("{}:{}", cfg.username, cfg.password)));
    match cfg.kind {
        ServerKind::Icecast => {
            agent
                .get(&format!("http://{}:{}/admin/metadata", cfg.host, cfg.port))
                .set("Authorization", &auth)
                .query("mount", &norm_mount(&cfg.mount))
                .query("mode", "updinfo")
                .query("song", song)
                .query("charset", "UTF-8")
                .call()?;
        }
        ServerKind::Shoutcast => {
            agent
                .get(&format!("http://{}:{}/admin.cgi", cfg.host, cfg.port))
                .query("sid", "1")
                .query("mode", "updinfo")
                .query("pass", &cfg.password)
                .query("song", song)
                .call()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a local Icecast on :8765 with source password "hackme": `cargo test -- --ignored stream`.
    #[test]
    #[ignore]
    fn streams_to_local_icecast() {
        let engine = Engine::new_detached();
        // pull the master bus in real time, as a sound card would
        let mut bus = engine.primary_source();
        thread::spawn(move || loop {
            for _ in 0..(SR as usize * 2 / 50) {
                let _ = bus.next();
            }
            thread::sleep(Duration::from_millis(20));
        });
        let cfg = EncoderConfig {
            host: "127.0.0.1".into(),
            port: 8765,
            mount: "/test.mp3".into(),
            username: "source".into(),
            password: "hackme".into(),
            send_metadata: false,
            ..Default::default()
        };
        let h = EncoderHandle::start(cfg, engine, Arc::new(Mutex::new(String::new())));
        thread::sleep(Duration::from_secs(14)); // longer than Icecast's 10 s source timeout
        let st = h.status.lock();
        assert_eq!(st.state, EncState::Live, "{:?}", st.state);
        assert!(st.bytes > 100_000, "only {} bytes sent", st.bytes);
        assert_eq!(st.reconnects, 0);
        drop(st);
        h.stop();
    }
}
