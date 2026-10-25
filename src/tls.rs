//! HTTPS for RadioMaster Server and Connect to Server.
//!
//! The server side wraps the server's plain HTTP listener: connections are accepted on the public
//! port, the TLS is taken off here and the requests are passed to the HTTP server on a loopback port.
//! A certificate can be given (e.g. from Let's Encrypt); otherwise a self-signed one is made once and
//! kept in the data folder.
//!
//! The app side trusts a certificate that a public authority signed, as browsers do, or one whose
//! SHA-256 fingerprint the user accepted when first connecting (it is remembered per server, and a
//! different certificate later is refused until it is accepted again).

// shared by the app and the server, which each use their own half
#![allow(dead_code)]

use parking_lot::{Mutex, RwLock};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    thread,
    time::Duration,
};

fn provider() -> Arc<CryptoProvider> {
    static P: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    P.get_or_init(|| Arc::new(ring::default_provider())).clone()
}

/// "AB:CD:…" — the SHA-256 of a certificate, as browsers show it.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

// ---------------------------------------------------------------------------------------------
// Server side

/// The certificate the server uses. It can be swapped while the server runs (a renewed Let's Encrypt
/// certificate, or the files changing on disk), without dropping connections.
pub struct ServerTls {
    pub config: Arc<rustls::ServerConfig>,
    pub certs: Arc<CertSwap>,
}

impl ServerTls {
    pub fn info(&self) -> CertInfo {
        self.certs.info()
    }
}

/// What the server tells apps and the log about its certificate.
#[derive(Clone, Default, Debug)]
pub struct CertInfo {
    /// SHA-256 of the first certificate in the chain
    pub fingerprint: String,
    /// made by the server itself (the app asks the user to trust it)
    pub self_signed: bool,
    /// when it stops being valid (unix seconds)
    pub not_after: Option<i64>,
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
}

/// The certificate handed to every new connection.
#[derive(Debug)]
pub struct CertSwap {
    key: RwLock<Arc<CertifiedKey>>,
    info: Mutex<CertInfo>,
}

impl ResolvesServerCert for CertSwap {
    fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.key.read().clone())
    }
}

impl CertSwap {
    pub fn info(&self) -> CertInfo {
        self.info.lock().clone()
    }

    /// Use the certificate and key in these files from now on (when they load and match).
    pub fn load(&self, cert_file: &Path, key_file: &Path, self_signed: bool) -> Result<CertInfo, String> {
        let (key, info) = load_pair(cert_file, key_file, self_signed)?;
        *self.key.write() = key;
        *self.info.lock() = info.clone();
        Ok(info)
    }

    /// Load the files again when they changed on disk, e.g. renewed by certbot or win-acme (checked
    /// every few minutes; the server keeps running).
    pub fn watch(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        thread::spawn(move || {
            let stamp = |i: &CertInfo| {
                let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                (m(&i.cert_file), m(&i.key_file))
            };
            let mut last = me.upgrade().map(|s| stamp(&s.info())).unwrap_or_default();
            loop {
                thread::sleep(Duration::from_secs(300));
                let Some(s) = me.upgrade() else { break };
                let i = s.info();
                let now = stamp(&i);
                if now != last && now.0.is_some() && now.1.is_some() {
                    // both files may be written one after the other: try again next time if they don't match yet
                    if s.load(&i.cert_file, &i.key_file, i.self_signed).is_ok() {
                        last = now;
                    }
                }
            }
        });
    }
}

fn load_pair(cert_file: &Path, key_file: &Path, self_signed: bool) -> Result<(Arc<CertifiedKey>, CertInfo), String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_file)
        .and_then(|it| it.collect::<Result<_, _>>())
        .map_err(|e| format!("can't read the certificate {}: {e}", cert_file.display()))?;
    let Some(first) = certs.first() else { return Err(format!("no certificate in {}", cert_file.display())) };
    let info = CertInfo {
        fingerprint: fingerprint(first),
        self_signed,
        not_after: not_after(first),
        cert_file: cert_file.to_path_buf(),
        key_file: key_file.to_path_buf(),
    };
    let key = PrivateKeyDer::from_pem_file(key_file).map_err(|e| format!("can't read the private key {}: {e}", key_file.display()))?;
    let ck = CertifiedKey::from_der(certs, key, &provider()).map_err(|e| format!("the certificate and key don't work together: {e}"))?;
    Ok((Arc::new(ck), info))
}

/// Files of the Let's Encrypt certificate in the data folder.
pub fn acme_files(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("acme-cert.pem"), dir.join("acme-key.pem"))
}

/// Load `cert`/`key` (PEM), or with both empty the self-signed pair in `dir` (made the first time).
/// With `acme`, the Let's Encrypt certificate in `dir` once there is one (the self-signed one until then).
pub fn server_tls(cert: &str, key: &str, dir: &Path, acme: bool) -> Result<ServerTls, String> {
    let (ac, ak) = acme_files(dir);
    let loaded = if acme && ac.exists() && ak.exists() { load_pair(&ac, &ak, false).ok() } else { None };
    let (key, info) = match loaded {
        Some(l) => l,
        None if acme || (cert.trim().is_empty() && key.trim().is_empty()) => {
            let (c, k) = (dir.join("tls-cert.pem"), dir.join("tls-key.pem"));
            if !c.exists() || !k.exists() {
                make_self_signed(&c, &k)?;
            }
            load_pair(&c, &k, true)?
        }
        None if cert.trim().is_empty() || key.trim().is_empty() => {
            return Err("give both the certificate and its private key (--cert and --key)".into())
        }
        None => load_pair(Path::new(cert.trim()), Path::new(key.trim()), false)?,
    };
    let certs = Arc::new(CertSwap { key: RwLock::new(key), info: Mutex::new(info) });
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(certs.clone());
    Ok(ServerTls { config: Arc::new(config), certs })
}

/// When a certificate stops being valid (unix seconds), read from its DER.
pub fn not_after(der: &[u8]) -> Option<i64> {
    // tag, contents, what follows
    fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let tag = *b.first()?;
        let l0 = *b.get(1)? as usize;
        let (len, head) = if l0 < 0x80 {
            (l0, 2)
        } else {
            let n = l0 & 0x7f;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *b.get(2 + i)? as usize;
            }
            (len, 2 + n)
        };
        Some((tag, b.get(head..head + len)?, b.get(head + len..)?))
    }
    let (_, cert, _) = tlv(der)?;
    let (_, tbs, _) = tlv(cert)?;
    let (tag, _, mut rest) = tlv(tbs)?;
    // [0] version is optional; then serial number, signature algorithm and issuer come before the validity
    let skip = if tag == 0xA0 { 3 } else { 2 };
    for _ in 0..skip {
        rest = tlv(rest)?.2;
    }
    let (_, validity, _) = tlv(rest)?;
    let (_, _, after) = tlv(validity)?;
    let (tag, t, _) = tlv(after)?;
    let t = std::str::from_utf8(t).ok()?.trim_end_matches('Z');
    let full = match tag {
        // UTCTime: YYMMDDHHMMSS
        0x17 if t.len() >= 12 => {
            let yy: i32 = t[..2].parse().ok()?;
            format!("{}{t}", if yy < 50 { "20" } else { "19" })
        }
        // GeneralizedTime: YYYYMMDDHHMMSS
        0x18 if t.len() >= 14 => t.to_string(),
        _ => return None,
    };
    chrono::NaiveDateTime::parse_from_str(&full[..14], "%Y%m%d%H%M%S").ok().map(|d| d.and_utc().timestamp())
}

fn make_self_signed(cert: &Path, key: &Path) -> Result<(), String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(h) = hostname() {
        names.push(h);
    }
    let ck = rcgen::generate_simple_self_signed(names).map_err(|e| format!("can't make a certificate: {e}"))?;
    std::fs::write(cert, ck.cert.pem()).map_err(|e| format!("can't save {}: {e}", cert.display()))?;
    write_private(key, &ck.signing_key.serialize_pem()).map_err(|e| format!("can't save {}: {e}", key.display()))?;
    Ok(())
}

fn hostname() -> Option<String> {
    let h = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())?;
    let h = h.trim().to_lowercase();
    (!h.is_empty()).then_some(h)
}

/// The private key is readable by its owner only (on Unix).
pub fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    // Windows: the data folder is in the user's own profile, whose permissions it inherits; the file
    // is also hidden so it isn't picked up by accident
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        o.attributes(FILE_ATTRIBUTE_HIDDEN);
    }
    o.open(path)?.write_all(text.as_bytes())
}

/// Connections being served at once; more wait in the accept queue.
const MAX_CONNECTIONS: usize = 128;

/// Accept HTTPS connections on `listener` and hand their requests to the HTTP server at `backend`.
pub fn serve(listener: TcpListener, backend: SocketAddr, config: Arc<rustls::ServerConfig>) {
    let open = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(tcp) = stream else {
                thread::sleep(Duration::from_millis(50));
                continue;
            };
            while open.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                thread::sleep(Duration::from_millis(20));
            }
            open.fetch_add(1, Ordering::Relaxed);
            let (open, config) = (open.clone(), config.clone());
            thread::spawn(move || {
                let _ = connection(tcp, backend, config);
                open.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
}

/// A connection stays open this long without a byte from the app or browser.
const IDLE: Duration = Duration::from_secs(120);

fn connection(tcp: TcpStream, backend: SocketAddr, config: Arc<rustls::ServerConfig>) -> io::Result<()> {
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut first = [0u8; 1];
    if tcp.peek(&mut first)? == 0 {
        return Ok(());
    }
    // 0x16 starts a TLS handshake; anything else is someone typing http://, or a reverse proxy
    if first[0] != 0x16 {
        return plain_http(tcp, backend);
    }
    let mut conn = rustls::ServerConnection::new(config).map_err(io::Error::other)?;
    let mut raw = tcp.try_clone()?;
    // the same for the handshake: done within a few seconds or dropped
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while conn.is_handshaking() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        raw.set_read_timeout(Some(left))?;
        conn.complete_io(&mut raw)?;
    }
    tcp.set_read_timeout(Some(IDLE))?;
    let mut back = TcpStream::connect(backend)?;
    back.set_nodelay(true)?;
    // the first request often arrives with the end of the handshake: pass on what is already here
    flush(&mut conn, &mut raw)?;
    let mut early = Vec::new();
    match conn.reader().read_to_end(&mut early) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }
    if !early.is_empty() {
        back.write_all(&early)?;
    }
    let conn = Arc::new(Mutex::new(conn));
    // when the server last sent something: a quiet client is fine while its answer still flows
    let sent = Arc::new(Mutex::new(std::time::Instant::now()));

    // the server's answers → the client
    let writer = {
        let (conn, mut back, mut out, tcp, sent) = (conn.clone(), back.try_clone()?, tcp.try_clone()?, tcp.try_clone()?, sent.clone());
        thread::spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let n = match back.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                *sent.lock() = std::time::Instant::now();
                let mut c = conn.lock();
                if c.writer().write_all(&buf[..n]).is_err() || flush(&mut c, &mut out).is_err() {
                    break;
                }
            }
            let mut c = conn.lock();
            c.send_close_notify();
            let _ = flush(&mut c, &mut out);
            let _ = tcp.shutdown(Shutdown::Both);
        })
    };

    // the client's requests → the server
    let mut raw = tcp.try_clone()?;
    let mut to_back = back.try_clone()?;
    let mut buf = vec![0u8; 16 * 1024];
    let mut plain = Vec::with_capacity(16 * 1024);
    'outer: loop {
        let n = match raw.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) && sent.lock().elapsed() < IDLE => continue,
            Err(_) => break,
        };
        let mut rest = &buf[..n];
        {
            let mut c = conn.lock();
            while !rest.is_empty() {
                if c.read_tls(&mut rest).is_err() {
                    break 'outer;
                }
                let state = match c.process_new_packets() {
                    Ok(s) => s,
                    Err(_) => {
                        let _ = flush(&mut c, &mut raw);
                        break 'outer;
                    }
                };
                let want = state.plaintext_bytes_to_read();
                if want > 0 {
                    let at = plain.len();
                    plain.resize(at + want, 0);
                    let _ = c.reader().read_exact(&mut plain[at..]);
                }
                if state.peer_has_closed() {
                    rest = &[];
                }
            }
            if c.wants_write() {
                let _ = flush(&mut c, &mut raw);
            }
        }
        if !plain.is_empty() {
            if to_back.write_all(&plain).is_err() {
                break;
            }
            plain.clear();
        }
    }
    let _ = back.shutdown(Shutdown::Both);
    let _ = writer.join();
    Ok(())
}

fn flush(c: &mut rustls::ServerConnection, out: &mut TcpStream) -> io::Result<()> {
    while c.wants_write() {
        c.write_tls(out)?;
    }
    Ok(())
}

/// Plain HTTP on the HTTPS port: tell the app, and send a browser to the https:// address. Requests
/// from a reverse proxy or tunnel that took the HTTPS off itself (IIS, nginx, Caddy, Cloudflare…) are
/// served as they are: sending those to https:// would only bring them back here (a redirect loop).
/// So are Let's Encrypt's checks, which always come over plain HTTP.
fn plain_http(mut tcp: TcpStream, backend: SocketAddr) -> io::Result<()> {
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    // the whole request head within a few seconds: a client dripping bytes doesn't keep the thread
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        tcp.set_read_timeout(Some(left))?;
        let n = tcp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
    if behind_https_proxy(&text) || path.starts_with("/.well-known/acme-challenge/") {
        return pass_plain(tcp, backend, &head);
    }
    let host = request_header(&text, "host").filter(|h| h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)));
    let reply = match host {
        Some(h) if !path.starts_with("/api/") && path.starts_with('/') && !path.contains(['\r', '\n']) => {
            format!("HTTP/1.1 301 Moved Permanently\r\nLocation: https://{h}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        }
        h => {
            let body = serde_json::json!({ "ok": false, "https": true,
                "error": format!("{HTTPS_ONLY}: type https://{} as the address", h.unwrap_or_else(|| "…".into())) })
            .to_string();
            format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    };
    tcp.write_all(reply.as_bytes())?;
    let _ = tcp.shutdown(Shutdown::Both);
    Ok(())
}

/// A header of a request head (the first one with that name).
fn request_header(head: &str, name: &str) -> Option<String> {
    head.lines()
        .skip(1)
        .take_while(|l| !l.is_empty())
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
}

/// The request came through a proxy whose own connection to the browser is HTTPS. IIS (Application
/// Request Routing) adds X-ARR-SSL; most others X-Forwarded-Proto; Cloudflare CF-Visitor.
pub fn behind_https_proxy(head: &str) -> bool {
    let h = |n: &str| request_header(head, n).map(|v| v.to_ascii_lowercase());
    h("x-forwarded-proto").is_some_and(|v| v.split(',').next().is_some_and(|p| p.trim() == "https"))
        || h("x-forwarded-ssl").is_some_and(|v| v == "on")
        || h("front-end-https").is_some_and(|v| v == "on")
        || h("x-arr-ssl").is_some_and(|v| !v.is_empty())
        || h("cf-visitor").is_some_and(|v| v.contains("\"https\""))
        || h("forwarded").is_some_and(|v| v.split([';', ',']).any(|p| p.trim() == "proto=https"))
}

/// Hand a plain connection to the HTTP server as it is (`head` was already read from it).
fn pass_plain(tcp: TcpStream, backend: SocketAddr, head: &[u8]) -> io::Result<()> {
    let mut back = TcpStream::connect(backend)?;
    back.set_nodelay(true)?;
    back.write_all(head)?;
    tcp.set_read_timeout(Some(IDLE))?;
    let sent = Arc::new(Mutex::new(std::time::Instant::now()));
    let writer = {
        let (mut from, mut to, sent) = (back.try_clone()?, tcp.try_clone()?, sent.clone());
        thread::spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n @ 1..) = from.read(&mut buf) {
                *sent.lock() = std::time::Instant::now();
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            let _ = to.shutdown(Shutdown::Both);
        })
    };
    let (mut from, mut to) = (tcp, back.try_clone()?);
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match from.read(&mut buf) {
            // the client is done asking: its answer still comes
            Ok(0) => {
                let _ = back.shutdown(Shutdown::Write);
                let _ = writer.join();
                return Ok(());
            }
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            // a quiet client is fine while its answer (e.g. the station's sound) still flows
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) && sent.lock().elapsed() < IDLE => {}
            Err(_) => break,
        }
    }
    let _ = back.shutdown(Shutdown::Both);
    let _ = writer.join();
    Ok(())
}

/// The start of the error a plain-HTTP request to the HTTPS port gets (the app retries with https://).
pub const HTTPS_ONLY: &str = "This server uses HTTPS";

// ---------------------------------------------------------------------------------------------
// App side

/// The certificate the server last showed, for "Trust this certificate?".
pub type Seen = Arc<Mutex<String>>;

#[derive(Debug)]
struct Verifier {
    public: Arc<rustls::client::WebPkiServerVerifier>,
    /// fingerprint the user accepted for this server (empty: none)
    pin: String,
    seen: Seen,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        *self.seen.lock() = fp.clone();
        if !self.pin.is_empty() && fp == self.pin {
            return Ok(ServerCertVerified::assertion());
        }
        self.public
            .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
            .map_err(|_| rustls::Error::General(if self.pin.is_empty() { UNTRUSTED.into() } else { CHANGED.into() }))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &provider().signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &provider().signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider().signature_verification_algorithms.supported_schemes()
    }
}

/// Marks in the connection error: the certificate isn't one a public authority signed / isn't the
/// one accepted before.
pub const UNTRUSTED: &str = "RM-UNTRUSTED-CERT";
pub const CHANGED: &str = "RM-CHANGED-CERT";

/// TLS settings for talking to a server: trusts public certificates and the one pinned.
pub fn client_config(pin: &str, seen: Seen) -> Arc<rustls::ClientConfig> {
    static PUBLIC: OnceLock<Arc<rustls::client::WebPkiServerVerifier>> = OnceLock::new();
    let public = PUBLIC
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
                .build()
                .expect("the public root certificates")
        })
        .clone();
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Verifier { public, pin: pin.to_string(), seen }))
        .with_no_client_auth();
    Arc::new(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert_until(year: i32) -> Vec<u8> {
        let mut p = rcgen::CertificateParams::new(vec!["radio.example.com".to_string()]).unwrap();
        p.not_after = rcgen::date_time_ymd(year, 3, 4);
        let key = rcgen::KeyPair::generate().unwrap();
        p.self_signed(&key).unwrap().der().to_vec()
    }

    #[test]
    fn reads_when_a_certificate_runs_out() {
        // before 2050 the date is a UTCTime, after it a GeneralizedTime
        let at = |y| chrono::NaiveDate::from_ymd_opt(y, 3, 4).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        assert_eq!(not_after(&cert_until(2031)), Some(at(2031)));
        assert_eq!(not_after(&cert_until(2077)), Some(at(2077)));
        assert_eq!(not_after(b"\x30\x03\x02\x01"), None);
    }

    #[test]
    fn knows_a_proxy_that_took_off_the_https() {
        let req = |h: &str| format!("GET / HTTP/1.1\r\nHost: radio.example.com\r\n{h}\r\n\r\n");
        assert!(behind_https_proxy(&req("X-Forwarded-Proto: https")));
        assert!(behind_https_proxy(&req("x-forwarded-proto: HTTPS, http")));
        assert!(behind_https_proxy(&req("X-ARR-SSL: 2048|256|CN=R11")));
        assert!(behind_https_proxy(&req("Forwarded: for=1.2.3.4;proto=https")));
        assert!(!behind_https_proxy(&req("X-Forwarded-Proto: http")));
        assert!(!behind_https_proxy(&req("X-Other: X-Forwarded-Proto: https")));
    }
}
