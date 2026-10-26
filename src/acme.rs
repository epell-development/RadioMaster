//! Free HTTPS certificates from Let's Encrypt (or another ACME authority) for RadioMaster Server.
//!
//! The server asks for a certificate for its domain names, proves it controls them by answering
//! Let's Encrypt's check at http://<domain>/.well-known/acme-challenge/… (the "http-01" challenge:
//! port 80 has to reach the server), and renews the certificate on its own well before it runs out.
//! The new certificate is swapped in while the server runs.
//!
//! This follows RFC 8555: requests are JSON Web Signatures made with an ECDSA P-256 account key, which
//! is kept in the data folder with the account's address.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64U, Engine as _};
use parking_lot::Mutex;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// token → key authorization, answered at /.well-known/acme-challenge/<token> while a check runs.
pub type Challenges = Arc<Mutex<HashMap<String, String>>>;

/// How the certificate is getting on, for the app's Server settings and the log.
#[derive(Clone, Default, Debug)]
pub struct Status {
    /// "", "working", "ok" or "error"
    pub state: String,
    pub message: String,
}

pub type Shared = Arc<Mutex<Status>>;

/// What to ask for.
#[derive(Clone, Debug)]
pub struct Order {
    pub domains: Vec<String>,
    pub email: String,
    /// the authority's directory address (Let's Encrypt, its staging server, or another)
    pub directory: String,
}

/// "radio.example.com, www.radio.example.com" → the names, tidied.
pub fn domains(list: &str) -> Vec<String> {
    list.split([',', ' ', ';'])
        .map(|d| d.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect()
}

/// account.json in the data folder: the account key (PKCS#8, base64) and its address at the authority.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Account {
    directory: String,
    key: String,
    kid: String,
}

struct Client {
    agent: ureq::Agent,
    key: EcdsaKeyPair,
    rng: SystemRandom,
    kid: String,
    nonce: Option<String>,
    new_nonce: String,
}

/// A problem document from the authority, or a network error.
fn problem(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let v: Value = r.into_json().unwrap_or_default();
            let detail = v["detail"].as_str().unwrap_or("");
            let sub: Vec<String> = v["subproblems"]
                .as_array()
                .map(|a| a.iter().filter_map(|p| p["detail"].as_str().map(String::from)).collect())
                .unwrap_or_default();
            let kind = v["type"].as_str().unwrap_or("").trim_start_matches("urn:ietf:params:acme:error:");
            let mut msg = format!("HTTP {code}{}", if kind.is_empty() { String::new() } else { format!(" {kind}") });
            if !detail.is_empty() {
                msg += &format!(": {detail}");
            }
            for s in sub {
                msg += &format!("; {s}");
            }
            msg
        }
        e => e.to_string(),
    }
}

impl Client {
    fn jwk(&self) -> Value {
        // uncompressed point: 0x04 | x | y
        let p = self.key.public_key().as_ref();
        json!({ "crv": "P-256", "kty": "EC", "x": B64U.encode(&p[1..33]), "y": B64U.encode(&p[33..65]) })
    }

    /// RFC 7638: SHA-256 of the key's required members in this exact order, without spaces.
    fn thumbprint(&self) -> String {
        use sha2::Digest;
        let j = self.jwk();
        let text =
            format!(r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#, j["x"].as_str().unwrap_or(""), j["y"].as_str().unwrap_or(""));
        B64U.encode(sha2::Sha256::digest(text.as_bytes()))
    }

    fn nonce(&mut self) -> Result<String, String> {
        if let Some(n) = self.nonce.take() {
            return Ok(n);
        }
        let r = self.agent.head(&self.new_nonce).call().map_err(problem)?;
        r.header("Replay-Nonce").map(String::from).ok_or_else(|| "no nonce from the authority".to_string())
    }

    /// A signed POST. `payload` None is a "POST-as-GET" (an empty payload).
    fn post(&mut self, url: &str, payload: Option<&Value>) -> Result<ureq::Response, String> {
        let mut tries = 0;
        loop {
            tries += 1;
            let mut protected = json!({ "alg": "ES256", "nonce": self.nonce()?, "url": url });
            if self.kid.is_empty() {
                protected["jwk"] = self.jwk();
            } else {
                protected["kid"] = json!(self.kid);
            }
            let protected = B64U.encode(protected.to_string());
            let payload = payload.map(|p| B64U.encode(p.to_string())).unwrap_or_default();
            let sig = self.key.sign(&self.rng, format!("{protected}.{payload}").as_bytes()).map_err(|_| "signing failed".to_string())?;
            let body = json!({ "protected": protected, "payload": payload, "signature": B64U.encode(sig.as_ref()) });
            let r = self.agent.post(url).set("Content-Type", "application/jose+json").send_string(&body.to_string());
            let nonce = match &r {
                Ok(r) => r.header("Replay-Nonce"),
                Err(ureq::Error::Status(_, r)) => r.header("Replay-Nonce"),
                Err(_) => None,
            };
            self.nonce = nonce.map(String::from);
            match r {
                Ok(r) => return Ok(r),
                // a stale nonce is normal now and then: the answer carries a fresh one
                Err(ureq::Error::Status(400, r)) if tries < 3 => {
                    let text = r.into_string().unwrap_or_default();
                    if !text.contains("badNonce") {
                        return Err(format!("HTTP 400: {text}"));
                    }
                }
                Err(e) => return Err(problem(e)),
            }
        }
    }

    fn post_json(&mut self, url: &str, payload: Option<&Value>) -> Result<(Value, Option<String>), String> {
        let r = self.post(url, payload)?;
        let location = r.header("Location").map(String::from);
        Ok((r.into_json().map_err(|e| e.to_string())?, location))
    }

    /// POST-as-GET `url` until its status is no longer pending/processing.
    fn wait(&mut self, url: &str, what: &str) -> Result<Value, String> {
        for _ in 0..90 {
            let (v, _) = self.post_json(url, None)?;
            match v["status"].as_str().unwrap_or("") {
                "pending" | "processing" => thread::sleep(Duration::from_secs(2)),
                _ => return Ok(v),
            }
        }
        Err(format!("the {what} took too long"))
    }
}

/// Get a certificate. Returns (certificate chain PEM, private key PEM).
pub fn obtain(
    order: &Order,
    dir: &Path,
    agent: ureq::Agent,
    challenges: &Challenges,
    say: &dyn Fn(String),
) -> Result<(String, String), String> {
    if order.domains.is_empty() {
        return Err("no domain name given".into());
    }
    let d: Value = agent.get(&order.directory).call().map_err(problem)?.into_json().map_err(|e| e.to_string())?;
    let url = |k: &str| d[k].as_str().map(String::from).ok_or_else(|| format!("the authority's directory has no {k}"));
    let (new_nonce, new_account, new_order) = (url("newNonce")?, url("newAccount")?, url("newOrder")?);

    // the account: made once per authority and kept
    let path = dir.join("acme-account.json");
    let mut acct: Account = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let rng = SystemRandom::new();
    if acct.key.is_empty() || acct.directory != order.directory {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|_| "can't make an account key".to_string())?;
        acct = Account { directory: order.directory.clone(), key: B64U.encode(pkcs8.as_ref()), kid: String::new() };
    }
    let der = B64U.decode(&acct.key).map_err(|e| e.to_string())?;
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng).map_err(|e| format!("the account key: {e}"))?;
    let mut c = Client { agent, key, rng, kid: String::new(), nonce: None, new_nonce };
    // signing up again with the same key just finds the account
    let mut reg = json!({ "termsOfServiceAgreed": true });
    if !order.email.trim().is_empty() {
        reg["contact"] = json!([format!("mailto:{}", order.email.trim())]);
    }
    let (_, kid) = c.post_json(&new_account, Some(&reg))?;
    c.kid = kid.ok_or("the authority didn't give an account address")?;
    acct.kid = c.kid.clone();
    crate::tls::write_private(&path, &serde_json::to_string_pretty(&acct).unwrap_or_default()).map_err(|e| e.to_string())?;

    let ids: Vec<Value> = order.domains.iter().map(|d| json!({ "type": "dns", "value": d })).collect();
    let (o, order_url) = c.post_json(&new_order, Some(&json!({ "identifiers": ids })))?;
    let order_url = order_url.ok_or("the authority didn't give an order address")?;
    let thumb = c.thumbprint();
    let authz: Vec<String> =
        o["authorizations"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut tokens = vec![];
    let result = (|| {
        for a in &authz {
            let (v, _) = c.post_json(a, None)?;
            let domain = v["identifier"]["value"].as_str().unwrap_or("?").to_string();
            if v["status"] == "valid" {
                continue;
            }
            let ch = v["challenges"].as_array().and_then(|l| l.iter().find(|x| x["type"] == "http-01")).ok_or_else(|| {
                format!("{domain}: the authority offers no http-01 check (wildcard names need DNS checks, which aren't supported)")
            })?;
            let token = ch["token"].as_str().ok_or("a check without a token")?.to_string();
            let ch_url = ch["url"].as_str().ok_or("a check without an address")?.to_string();
            challenges.lock().insert(token.clone(), format!("{token}.{thumb}"));
            tokens.push(token);
            say(format!("Let's Encrypt: checking http://{domain}/.well-known/acme-challenge/…"));
            c.post_json(&ch_url, Some(&json!({})))?;
            let done = c.wait(a, "domain check")?;
            if done["status"] != "valid" {
                let why = done["challenges"]
                    .as_array()
                    .and_then(|l| l.iter().find_map(|x| x["error"]["detail"].as_str().map(String::from)))
                    .unwrap_or_else(|| done["status"].as_str().unwrap_or("failed").to_string());
                return Err(format!("{domain}: {why} (port 80 of this name has to reach this server)"));
            }
        }
        Ok(())
    })();
    {
        let mut ch = challenges.lock();
        for t in &tokens {
            ch.remove(t);
        }
    }
    result?;

    // the certificate's own key, and the request for it
    let kp = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let csr = rcgen::CertificateParams::new(order.domains.clone())
        .and_then(|p| p.serialize_request(&kp))
        .map_err(|e| format!("can't make the certificate request: {e}"))?;
    let finalize = o["finalize"].as_str().ok_or("the order has no finalize address")?.to_string();
    let (f, _) = c.post_json(&finalize, Some(&json!({ "csr": B64U.encode(csr.der()) })))?;
    let done = if f["status"] == "valid" { f } else { c.wait(&order_url, "certificate")? };
    if done["status"] != "valid" {
        return Err(format!("the certificate wasn't issued ({})", done["status"].as_str().unwrap_or("?")));
    }
    let cert_url = done["certificate"].as_str().ok_or("the order has no certificate address")?.to_string();
    let chain = c.post(&cert_url, None)?.into_string().map_err(|e| e.to_string())?;
    if !chain.contains("BEGIN CERTIFICATE") {
        return Err("the authority sent something that isn't a certificate".into());
    }
    Ok((chain, kp.serialize_pem()))
}

/// Renew with 30 days left (a third of the 90 days a Let's Encrypt certificate lasts).
fn due(info: &crate::tls::CertInfo) -> bool {
    info.not_after.is_none_or(|end| end - chrono::Utc::now().timestamp() < 30 * 86400)
}

/// Keep a Let's Encrypt certificate in `certs`: get one now if there is none (or it runs out soon),
/// then look twice a day. `want` gives the order (it can change in the app while the server runs);
/// None while it is switched off.
pub fn keep(
    certs: Arc<crate::tls::CertSwap>,
    dir: PathBuf,
    want: Box<dyn Fn() -> Option<Order> + Send>,
    challenges: Challenges,
    status: Shared,
    say: Box<dyn Fn(String) + Send>,
) {
    thread::spawn(move || {
        let mut failures = 0u32;
        loop {
            let Some(order) = want() else {
                // switched off (it can be switched on again in the app)
                *status.lock() = Status::default();
                thread::sleep(Duration::from_secs(30));
                continue;
            };
            let info = certs.info();
            let (cf, kf) = crate::tls::acme_files(&dir);
            let names_changed = !info.self_signed && info.cert_file == cf && covers(&cf) != order.domains;
            if info.cert_file != cf || due(&info) || names_changed {
                *status.lock() = Status { state: "working".into(), message: format!("asking for {}", order.domains.join(", ")) };
                let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
                let r = obtain(&order, &dir, agent, &challenges, &*say).and_then(|(chain, key)| {
                    std::fs::write(&cf, chain).map_err(|e| e.to_string())?;
                    crate::tls::write_private(&kf, &key).map_err(|e| e.to_string())?;
                    std::fs::write(names_file(&cf), order.domains.join("\n")).map_err(|e| e.to_string())?;
                    certs.load(&cf, &kf, false)
                });
                match r {
                    Ok(i) => {
                        failures = 0;
                        let until =
                            i.not_after.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.format("%Y-%m-%d").to_string());
                        let msg = format!("certificate for {} until {}", order.domains.join(", "), until.unwrap_or_default());
                        say(format!("Let's Encrypt: {msg}"));
                        *status.lock() = Status { state: "ok".into(), message: msg };
                    }
                    Err(e) => {
                        failures += 1;
                        say(format!("Let's Encrypt: {e}"));
                        *status.lock() = Status { state: "error".into(), message: e };
                    }
                }
            } else if status.lock().state.is_empty() {
                let until = info.not_after.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.format("%Y-%m-%d").to_string());
                *status.lock() = Status {
                    state: "ok".into(),
                    message: format!("certificate for {} until {}", order.domains.join(", "), until.unwrap_or_default()),
                };
            }
            // after a failure try again sooner (but not so often that Let's Encrypt's limits are hit)
            let wait = if failures == 0 { 12 * 3600 } else { (failures.min(6) as u64) * 1800 };
            let mut slept = 0;
            while slept < wait {
                thread::sleep(Duration::from_secs(30));
                slept += 30;
                // the domains changed in the app: start over at once
                match want() {
                    Some(o) if o.domains == order.domains && o.directory == order.directory => {}
                    _ => break,
                }
            }
        }
    });
}

fn names_file(cert: &Path) -> PathBuf {
    cert.with_extension("names")
}

/// The domain names the saved certificate was asked for.
fn covers(cert: &Path) -> Vec<String> {
    std::fs::read_to_string(names_file(cert)).map(|s| s.lines().map(String::from).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}

/// Answer port 80: Let's Encrypt's checks, and everything else sent to the https:// address
/// (the web portal when it is on). Returns the address bound.
pub fn serve_http(bind: &str, port: u16, challenges: Challenges, https_port: u16) -> Result<std::net::SocketAddr, String> {
    let server = tiny_http::Server::http((bind, port)).map_err(|e| format!("port {port}: {e}"))?;
    let addr = server.server_addr().to_ip().ok_or("not an IP address")?;
    let server = Arc::new(server);
    for _ in 0..2 {
        let (server, challenges) = (server.clone(), challenges.clone());
        thread::spawn(move || {
            while let Ok(req) = server.recv() {
                let url = req.url().to_string();
                let reply = match url.strip_prefix("/.well-known/acme-challenge/").and_then(|t| challenges.lock().get(t).cloned()) {
                    Some(answer) => tiny_http::Response::from_string(answer).with_status_code(200),
                    None => {
                        let host =
                            req.headers().iter().find(|h| h.field.equiv("Host")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
                        let name = host_name(&host);
                        if name.is_empty() || !url.starts_with('/') {
                            tiny_http::Response::from_string("Not found").with_status_code(404)
                        } else {
                            let port = if https_port == 443 { String::new() } else { format!(":{https_port}") };
                            let to = format!("https://{name}{port}{url}");
                            tiny_http::Response::from_string("")
                                .with_status_code(301)
                                .with_header(tiny_http::Header::from_bytes(&b"Location"[..], to.as_bytes()).unwrap())
                        }
                    }
                };
                let _ = req.respond(reply);
            }
        });
    }
    Ok(addr)
}

/// "radio.example.com:80" → "radio.example.com" (only safe characters).
fn host_name(host: &str) -> String {
    let name = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or("").to_string()
    };
    if name.chars().all(|c| c.is_ascii_alphanumeric() || ".-[]:".contains(c)) {
        name
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_list_is_tidied() {
        assert_eq!(domains(" Radio.Example.com, https://www.example.com/ ;"), vec!["radio.example.com", "www.example.com"]);
        assert!(domains("  ").is_empty());
        assert_eq!(host_name("radio.example.com:80"), "radio.example.com");
        assert_eq!(host_name("[::1]:80"), "[::1]");
        assert_eq!(host_name("evil.com\r\nX: y"), "");
    }

    #[test]
    fn requests_are_signed_with_the_account_key() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let public = key.public_key().as_ref().to_vec();
        let c = Client { agent: ureq::agent(), key, rng: SystemRandom::new(), kid: String::new(), nonce: None, new_nonce: String::new() };
        // the JWK carries the same point, and the thumbprint is a base64url SHA-256
        let j = c.jwk();
        let mut point = vec![4u8];
        point.extend(B64U.decode(j["x"].as_str().unwrap()).unwrap());
        point.extend(B64U.decode(j["y"].as_str().unwrap()).unwrap());
        assert_eq!(point, public);
        assert_eq!(B64U.decode(c.thumbprint()).unwrap().len(), 32);
        // ES256 signatures are r|s (64 bytes) and check out against the public key
        let sig = c.key.sign(&c.rng, b"a.b").unwrap();
        assert_eq!(sig.as_ref().len(), 64);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &public).verify(b"a.b", sig.as_ref()).unwrap();
    }
}

/// Against Pebble, Let's Encrypt's test server, when it is installed: PEBBLE=/path/to/pebble and
/// PEBBLE_DIR=its source folder (with test/certs). Pebble checks http-01 on port 5002.
#[cfg(test)]
mod pebble {
    use super::*;
    use rustls::pki_types::pem::PemObject;

    #[test]
    fn gets_a_certificate_from_pebble() {
        let (Ok(bin), Ok(src)) = (std::env::var("PEBBLE"), std::env::var("PEBBLE_DIR")) else {
            eprintln!("PEBBLE / PEBBLE_DIR not set: skipped");
            return;
        };
        let src = PathBuf::from(src);
        let mut child = std::process::Command::new(bin)
            .args(["-config", "test/config/pebble-config.json"])
            .current_dir(&src)
            .env("PEBBLE_VA_NOSLEEP", "1")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_secs(2));
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls::pki_types::CertificateDer::pem_file_iter(src.join("test/certs/pebble.minica.pem")).unwrap() {
            roots.add(c.unwrap()).unwrap();
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let agent = ureq::AgentBuilder::new().tls_config(Arc::new(tls)).build();
        let challenges: Challenges = Default::default();
        serve_http("127.0.0.1", 5002, challenges.clone(), 443).unwrap();
        let dir = std::env::temp_dir().join(format!("rm-acme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let order =
            Order { domains: vec!["localhost".into()], email: "dj@example.com".into(), directory: "https://localhost:14000/dir".into() };
        let r = obtain(&order, &dir, agent.clone(), &challenges, &|m| eprintln!("{m}"));
        // a second time with the same account
        let again = r.as_ref().ok().map(|_| obtain(&order, &dir, agent, &challenges, &|_| {}));
        let _ = child.kill();
        let _ = child.wait();
        let (chain, key) = r.unwrap();
        assert!(again.unwrap().is_ok());
        assert!(chain.contains("BEGIN CERTIFICATE") && key.contains("PRIVATE KEY"));
        assert!(challenges.lock().is_empty(), "the checks are cleared afterwards");
        // the server can use it straight away
        let (cf, kf) = crate::tls::acme_files(&dir);
        std::fs::write(&cf, &chain).unwrap();
        std::fs::write(&kf, &key).unwrap();
        let t = crate::tls::server_tls("", "", &dir, true).unwrap();
        let info = t.info();
        assert!(!info.self_signed);
        assert!(info.not_after.unwrap() > chrono::Utc::now().timestamp());
    }
}
