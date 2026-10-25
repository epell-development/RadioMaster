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

use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime};
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

/// The certificate the server uses.
pub struct ServerTls {
    pub config: Arc<rustls::ServerConfig>,
    /// of the first certificate in the chain
    pub fingerprint: String,
    /// made by the server itself (the app asks the user to trust it)
    pub self_signed: bool,
    pub cert_file: PathBuf,
}

/// Load `cert`/`key` (PEM), or with both empty the self-signed pair in `dir` (made the first time).
pub fn server_tls(cert: &str, key: &str, dir: &Path) -> Result<ServerTls, String> {
    let (cert_file, key_file, self_signed) = if cert.trim().is_empty() && key.trim().is_empty() {
        let (c, k) = (dir.join("tls-cert.pem"), dir.join("tls-key.pem"));
        if !c.exists() || !k.exists() {
            make_self_signed(&c, &k)?;
        }
        (c, k, true)
    } else if cert.trim().is_empty() || key.trim().is_empty() {
        return Err("give both the certificate and its private key (--cert and --key)".into());
    } else {
        (PathBuf::from(cert.trim()), PathBuf::from(key.trim()), false)
    };
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert_file)
        .and_then(|it| it.collect::<Result<_, _>>())
        .map_err(|e| format!("can't read the certificate {}: {e}", cert_file.display()))?;
    let Some(first) = certs.first() else { return Err(format!("no certificate in {}", cert_file.display())) };
    let fingerprint = fingerprint(first);
    let key = PrivateKeyDer::from_pem_file(&key_file).map_err(|e| format!("can't read the private key {}: {e}", key_file.display()))?;
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("the certificate and key don't work together: {e}"))?;
    Ok(ServerTls { config: Arc::new(config), fingerprint, self_signed, cert_file })
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
fn write_private(path: &Path, text: &str) -> io::Result<()> {
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
    // 0x16 starts a TLS handshake; anything else is someone typing http://
    if first[0] != 0x16 {
        return plain_http(tcp);
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

/// Plain HTTP on the HTTPS port: tell the app, and send a browser to the https:// address.
fn plain_http(mut tcp: TcpStream) -> io::Result<()> {
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
    let head = String::from_utf8_lossy(&head);
    let path = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
    let host = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("host")).map(|(_, v)| v.trim().to_string()))
        .filter(|h| h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)));
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
