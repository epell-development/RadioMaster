//! Tiny HTTP remote-control API.
//!   GET /api/status  /api/play  /api/pause  /api/stop  /api/next  /api/prev
//!   GET /api/volume?v=0.8   /api/seek?s=30   /api/cart?i=0   /api/insert?path=C:\x.mp3
//! Add `&pw=PASSWORD` to every call if a password is set.

use crate::playout::{PlayState, Playout};
use parking_lot::Mutex;
use std::{path::PathBuf, sync::Arc, thread};

pub fn spawn(core: Arc<Mutex<Playout>>, port: u16, password: String) {
    thread::spawn(move || {
        let server = match tiny_http::Server::http(("0.0.0.0", port)) {
            Ok(s) => s,
            Err(e) => {
                core.lock().say(format!("Remote API failed to start on port {port}: {e}"));
                return;
            }
        };
        core.lock().say(format!("Remote API listening on port {port}"));
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
            let param = |k: &str| -> Option<String> {
                query.split('&').find_map(|kv| {
                    let (a, b) = kv.split_once('=')?;
                    (a == k).then(|| urlencoding::decode(b).ok().map(|c| c.to_string())).flatten()
                })
            };
            let body = if !password.is_empty() && param("pw").as_deref() != Some(password.as_str()) {
                serde_json::json!({"ok": false, "error": "unauthorized"})
            } else {
                let mut c = core.lock();
                match path {
                    "/api/status" => {
                        let cur = c.current.and_then(|i| c.playlist.get(i)).map(|e| e.track.clone());
                        let nxt = c.current.map(|i| i + 1).unwrap_or(0);
                        let next = c.playlist.get(nxt).map(|e| e.track.label());
                        serde_json::json!({
                            "ok": true,
                            "state": match c.state { PlayState::Playing => "playing", PlayState::Paused => "paused", PlayState::Stopped => "stopped" },
                            "artist": cur.as_ref().map(|t| t.artist.clone()),
                            "title": cur.as_ref().map(|t| t.title.clone()),
                            "position": c.status.pos, "duration": c.status.total,
                            "volume": c.cfg.volume, "next": next,
                            "playlist_length": c.playlist.len(),
                        })
                    }
                    "/api/play" => {
                        c.play_pressed(None);
                        serde_json::json!({"ok": true})
                    }
                    "/api/pause" => {
                        c.pause_toggle();
                        serde_json::json!({"ok": true})
                    }
                    "/api/stop" => {
                        c.stop();
                        serde_json::json!({"ok": true})
                    }
                    "/api/next" => {
                        c.next();
                        serde_json::json!({"ok": true})
                    }
                    "/api/prev" => {
                        c.prev();
                        serde_json::json!({"ok": true})
                    }
                    "/api/volume" => {
                        if let Some(v) = param("v").and_then(|v| v.parse::<f32>().ok()) {
                            c.set_volume(v.clamp(0.0, 1.0));
                        }
                        serde_json::json!({"ok": true, "volume": c.cfg.volume})
                    }
                    "/api/seek" => {
                        if let Some(s) = param("s").and_then(|v| v.parse::<f32>().ok()) {
                            c.seek(s);
                        }
                        serde_json::json!({"ok": true})
                    }
                    "/api/cart" => {
                        if let Some(i) = param("i").and_then(|v| v.parse::<usize>().ok()) {
                            c.trigger_cart(i);
                        }
                        serde_json::json!({"ok": true})
                    }
                    "/api/insert" => {
                        if let Some(p) = param("path") {
                            c.insert_next(vec![PathBuf::from(p)]);
                        }
                        serde_json::json!({"ok": true})
                    }
                    _ => serde_json::json!({"ok": false, "error": "unknown endpoint"}),
                }
            };
            let hdr = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let _ = req.respond(tiny_http::Response::from_string(body.to_string()).with_header(hdr));
        }
    });
}
