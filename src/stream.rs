//! Source client for Icecast (PUT, as used by modern Icecast and AzuraCast) and
//! Shoutcast v2 DJ ("OK2") connections, with a built-in LAME MP3 encoder. AAC, Opus, Ogg Vorbis
//! and FLAC are made by ffmpeg, and any other program can be used as a custom encoder: it reads
//! 16-bit 44.1 kHz stereo PCM on stdin and writes the encoded stream to stdout.

use crate::config::{EncoderConfig, ServerKind, StreamFormat};
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
    /// `ffmpeg`: the ffmpeg program to use for non-MP3 formats ("" = look for it).
    pub fn start(cfg: EncoderConfig, engine: Engine, song: Arc<Mutex<String>>, ffmpeg: String) -> Self {
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
                match run(&cfg, &ffmpeg, &rx, &s2, &st2, &song, &mut put_first) {
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

    /// A handle that only shows another machine's encoder status (nothing is encoded here).
    pub fn mirror(state: EncState, bytes: u64, reconnects: u32) -> Self {
        EncoderHandle {
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(true)),
            status: Arc::new(Mutex::new(EncStatus { state, bytes, reconnects })),
        }
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
    let ctype = cfg.format.content_type(&cfg.content_type);
    let auth = B64.encode(format!("{}:{}", cfg.username, cfg.password));
    let (proto, expect) = if method == "PUT" { ("HTTP/1.1", "Expect: 100-continue\r\n") } else { ("HTTP/1.0", "") };
    let req = format!(
        "{method} {mount} {proto}\r\nHost: {host}:{port}\r\nAuthorization: Basic {auth}\r\n\
         User-Agent: RadioMaster/{ver}\r\nContent-Type: {ctype}\r\n\
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
                "icy-name:{}\r\nicy-genre:{}\r\nicy-url:{}\r\nicy-pub:{}\r\nicy-br:{}\r\ncontent-type:{}\r\n\r\n",
                cfg.stream_name,
                cfg.genre,
                cfg.url,
                cfg.public as u8,
                kbps,
                cfg.format.content_type(&cfg.content_type)
            );
            s.write_all(hdr.as_bytes())?;
            Ok(s)
        }
    }
}

/// Where to find ffmpeg: the chosen path, next to RadioMaster, in the settings folder, or on the PATH.
pub fn find_ffmpeg(chosen: &str) -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut cands: Vec<std::path::PathBuf> = vec![];
    if !chosen.trim().is_empty() {
        cands.push(chosen.trim().into());
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        cands.push(d.join(exe));
        cands.push(d.join("ffmpeg").join(exe));
    }
    cands.push(crate::config::data_dir().join("ffmpeg").join(exe));
    if let Some(p) = cands.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    // on the PATH?
    let mut c = std::process::Command::new(exe);
    c.arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.status().ok().filter(|s| s.success()).map(|_| exe.into())
}

/// Split a command line into words ("quoted parts" stay together).
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// An encoder program fed through pipes: PCM in, encoded stream out.
struct PipeEncoder {
    child: std::process::Child,
    pcm_tx: Option<crossbeam_channel::Sender<Vec<u8>>>,
    out_rx: Receiver<Vec<u8>>,
    errors: Arc<Mutex<String>>,
}

impl PipeEncoder {
    fn start(program: &str, args: &[String]) -> Result<Self> {
        use std::process::{Command, Stdio};
        let mut c = Command::new(program);
        c.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn().map_err(|e| anyhow!("can't start the encoder \"{program}\": {e}"))?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("encoder has no input"))?;
        let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("encoder has no output"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("encoder has no error output"))?;
        let (pcm_tx, pcm_rx) = bounded::<Vec<u8>>(256);
        let (out_tx, out_rx) = bounded::<Vec<u8>>(1024);
        thread::spawn(move || {
            for b in pcm_rx {
                if stdin.write_all(&b).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out_tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let errors = Arc::new(Mutex::new(String::new()));
        let e2 = errors.clone();
        thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            *e2.lock() = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(300).collect();
        });
        Ok(PipeEncoder { child, pcm_tx: Some(pcm_tx), out_rx, errors })
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<()> {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let gone = self.pcm_tx.as_ref().is_none_or(|tx| tx.send(bytes).is_err());
        while let Ok(b) = self.out_rx.try_recv() {
            out.extend_from_slice(&b);
        }
        if gone || self.child.try_wait().ok().flatten().is_some() {
            thread::sleep(Duration::from_millis(100)); // let its last message arrive
            let msg = self.errors.lock().clone();
            bail!("the encoder stopped{}", if msg.is_empty() { String::new() } else { format!(": {msg}") });
        }
        Ok(())
    }
}

impl Drop for PipeEncoder {
    fn drop(&mut self) {
        self.pcm_tx = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum StreamEncoder {
    Lame(Box<mp3lame_encoder::Encoder>),
    Pipe(PipeEncoder),
}

impl StreamEncoder {
    fn new(cfg: &EncoderConfig, ffmpeg: &str) -> Result<Self> {
        match cfg.format {
            StreamFormat::Mp3 => {
                let mut b = Builder::new().ok_or_else(|| anyhow!("LAME init failed"))?;
                b.set_num_channels(2).map_err(|e| anyhow!("{e:?}"))?;
                b.set_sample_rate(SR).map_err(|e| anyhow!("{e:?}"))?;
                b.set_brate(bitrate(cfg.bitrate_kbps)).map_err(|e| anyhow!("{e:?}"))?;
                b.set_quality(Quality::Good).map_err(|e| anyhow!("{e:?}"))?;
                Ok(StreamEncoder::Lame(Box::new(b.build().map_err(|e| anyhow!("{e:?}"))?)))
            }
            StreamFormat::Custom => {
                let line = cfg.command.replace("{bitrate}", &cfg.bitrate_kbps.to_string());
                let words = split_args(&line);
                let (prog, args) = words.split_first().ok_or_else(|| anyhow!("type the custom encoder's command line"))?;
                Ok(StreamEncoder::Pipe(PipeEncoder::start(prog, args)?))
            }
            f => {
                let prog = find_ffmpeg(ffmpeg).ok_or_else(|| {
                    anyhow!("{} needs ffmpeg: put ffmpeg next to RadioMaster or choose it in Streams → ffmpeg", f.label())
                })?;
                let mut args: Vec<String> =
                    ["-hide_banner", "-loglevel", "error", "-f", "s16le", "-ar", &SR.to_string(), "-ac", "2", "-i", "-"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                args.extend(f.ffmpeg_args(cfg.bitrate_kbps));
                // send each packet on straight away (live stream, not a file)
                args.extend(["-flush_packets", "1", "-"].map(String::from));
                Ok(StreamEncoder::Pipe(PipeEncoder::start(&prog.to_string_lossy(), &args)?))
            }
        }
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<()> {
        match self {
            StreamEncoder::Lame(enc) => {
                out.reserve(mp3lame_encoder::max_required_buffer_size(pcm.len()));
                let n = enc.encode(InterleavedPcm(pcm), out.spare_capacity_mut()).map_err(|e| anyhow!("encode: {e:?}"))?;
                unsafe { out.set_len(out.len() + n) };
                Ok(())
            }
            StreamEncoder::Pipe(p) => p.encode(pcm, out),
        }
    }
}

fn run(
    cfg: &EncoderConfig,
    ffmpeg: &str,
    rx: &Receiver<Arc<Vec<i16>>>,
    stop: &AtomicBool,
    status: &Mutex<EncStatus>,
    song: &Mutex<String>,
    put_first: &mut bool,
) -> Result<()> {
    // start the encoder first: a missing ffmpeg is reported without touching the server
    let mut enc = StreamEncoder::new(cfg, ffmpeg)?;
    let mut sock = connect(cfg, put_first)?;
    let connected = std::time::Instant::now();

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
                enc.encode(&pcm[..], &mut out)?;
                let n = out.len();
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

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(
            split_args(r#"ffmpeg -i - -metadata title="My Station" -f adts -"#),
            vec!["ffmpeg", "-i", "-", "-metadata", "title=My Station", "-f", "adts", "-"]
        );
        assert_eq!(split_args(r#""C:\Program Files\enc.exe" ''  -b 128"#), vec!["C:\\Program Files\\enc.exe", "", "-b", "128"]);
    }

    #[test]
    fn ffmpeg_presets_make_audio() {
        let Some(ff) = find_ffmpeg("") else { return };
        for f in [StreamFormat::Aac, StreamFormat::Vorbis, StreamFormat::Flac] {
            let cfg = EncoderConfig { format: f, ..Default::default() };
            let mut enc = StreamEncoder::new(&cfg, &ff.to_string_lossy()).unwrap();
            let tone: Vec<i16> = (0..SR as usize * 6).map(|k| ((k as f32 * 0.05).sin() * 8000.0) as i16).collect();
            let mut out = vec![];
            for chunk in tone.chunks(4096) {
                enc.encode(chunk, &mut out).unwrap();
                thread::sleep(Duration::from_millis(5));
            }
            for _ in 0..40 {
                thread::sleep(Duration::from_millis(50));
                let _ = enc.encode(&[], &mut out);
            }
            assert!(out.len() > 1000, "{f:?} produced {} bytes", out.len());
            if f != StreamFormat::Aac {
                assert_eq!(&out[..4], b"OggS", "{f:?} is an Ogg stream");
            }
        }
    }

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
        let format = match std::env::var("RM_FORMAT").as_deref() {
            Ok("opus") => StreamFormat::Opus,
            Ok("aac") => StreamFormat::Aac,
            Ok("vorbis") => StreamFormat::Vorbis,
            _ => StreamFormat::Mp3,
        };
        let cfg = EncoderConfig { mount: format!("/test{}", format.extension()), format, ..cfg };
        let h = EncoderHandle::start(cfg, engine, Arc::new(Mutex::new(String::new())), String::new());
        thread::sleep(Duration::from_secs(14)); // longer than Icecast's 10 s source timeout
        let st = h.status.lock();
        assert_eq!(st.state, EncState::Live, "{:?}", st.state);
        // silence: MP3 is constant bitrate, Opus / AAC shrink silence to almost nothing
        let want = if std::env::var("RM_FORMAT").is_ok() { 1_000 } else { 100_000 };
        assert!(st.bytes > want, "only {} bytes sent", st.bytes);
        assert_eq!(st.reconnects, 0);
        drop(st);
        h.stop();
    }
}
