//! Reusable Game-State-Integration (GSI) harness for Valve games.
//!
//! Valve's official GSI lets a game POST its live state as JSON to a localhost
//! port *we* host: drop a `gamestate_integration_*.cfg` into the game's `cfg`
//! dir telling it our URI + a shared token, and the game fires a POST on every
//! state change. This is the inbound counterpart to Hako's existing HTTP
//! *clients* — [`GsiServer`] binds `127.0.0.1:<port>`, validates the embedded
//! `auth.token`, and forwards each raw JSON body to the owning integration over
//! an [`mpsc`] channel, which drains it on its normal loop tick (exactly like
//! League drains its live feed).
//!
//! This module is game-agnostic: CS2 and Dota 2 share the server, the KeyValues
//! [`config_file`] writer, the [`shared_token`], and the Steam-path resolution.
//! Each game only differs in its cfg subpath, port, enabled components, and the
//! serde structs it parses the forwarded body into.
//!
//! We reproduce the public cfg format Valve documents (and Medal emits); we do
//! not copy Medal's code, tokens, or ports — we self-generate a token and pick
//! our own ports so a co-installed Medal coexists (Valve loads *every*
//! `gamestate_integration_*.cfg`, and each tool uses its own port).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::games::process_snapshot;

/// How long the accept loop blocks per iteration before re-checking the stop
/// flag (so dropping the server tears the thread down within this window).
const ACCEPT_TIMEOUT: Duration = Duration::from_millis(500);

/// A running GSI HTTP server bound to `127.0.0.1:port`. Accepts POSTs, validates
/// the `auth.token`, and forwards each valid raw JSON body to the caller's
/// channel. Stops (and joins its thread) on drop, so an integration just holds
/// the handle for as long as the game is running and drops it when it exits.
pub struct GsiServer {
    pub port: u16,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl GsiServer {
    /// Bind `127.0.0.1:port` and start accepting. Each POST whose `auth.token`
    /// equals `token` has its raw body sent to `tx`; everything else is dropped.
    /// We still answer every request `200` with an empty body (Valve GSI ignores
    /// the response), so the game's sender never stalls.
    pub fn start(port: u16, token: String, tx: Sender<String>) -> std::io::Result<GsiServer> {
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let running = Arc::new(AtomicBool::new(true));
        let run_flag = running.clone();
        let handle = std::thread::Builder::new()
            .name(format!("hako-gsi-{port}"))
            .spawn(move || serve(server, token, tx, run_flag))?;
        tracing::info!("gsi: listening on 127.0.0.1:{port}");
        Ok(GsiServer {
            port,
            running,
            handle: Some(handle),
        })
    }
}

impl Drop for GsiServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        tracing::info!("gsi: stopped listening on 127.0.0.1:{}", self.port);
    }
}

/// The accept loop: block up to [`ACCEPT_TIMEOUT`] for a request, forward valid
/// payloads, and re-check the stop flag between iterations.
fn serve(server: tiny_http::Server, token: String, tx: Sender<String>, running: Arc<AtomicBool>) {
    while running.load(Ordering::SeqCst) {
        let mut req = match server.recv_timeout(ACCEPT_TIMEOUT) {
            Ok(Some(req)) => req,
            Ok(None) => continue, // timeout — re-check `running`
            Err(e) => {
                tracing::debug!("gsi: accept error: {e}");
                continue;
            }
        };
        let mut body = String::new();
        let read_ok = req.as_reader().read_to_string(&mut body).is_ok();
        // Always answer (empty 200) so the game's POST completes promptly.
        let _ = req.respond(tiny_http::Response::empty(200));
        if !read_ok {
            continue;
        }
        if payload_token(&body).as_deref() == Some(token.as_str()) {
            // Receiver gone (integration dropped its end) → nothing to serve.
            if tx.send(body).is_err() {
                break;
            }
        }
    }
}

/// The `auth.token` embedded in a GSI JSON body, if present. Game-agnostic: we
/// only reach into `auth.token`, never the game-specific payload shape.
fn payload_token(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("auth")?
        .get("token")?
        .as_str()
        .map(|s| s.to_string())
}

// ===========================================================================
// Config-file generation (Valve KeyValues)
// ===========================================================================

/// Build a Valve KeyValues `gamestate_integration_*.cfg` telling the game to
/// POST the enabled `components` to `http://127.0.0.1:<port>/` with `token`.
/// Reproduces the public/Medal format; the header names the tool ("hako").
pub fn config_file(header: &str, port: u16, token: &str, components: &[&str]) -> String {
    let mut s = String::new();
    s.push_str(&format!("\"{header}\"\n{{\n"));
    s.push_str(&format!("\t\"uri\"\t\t\"http://127.0.0.1:{port}/\"\n"));
    s.push_str("\t\"timeout\"\t\"0.1\"\n");
    s.push_str("\t\"buffer\"\t\"0.1\"\n");
    s.push_str("\t\"throttle\"\t\"0.1\"\n");
    s.push_str("\t\"heartbeat\"\t\"30\"\n");
    s.push_str("\t\"auth\"\n\t{\n");
    s.push_str(&format!("\t\t\"token\"\t\"{token}\"\n"));
    s.push_str("\t}\n");
    s.push_str("\t\"data\"\n\t{\n");
    for c in components {
        s.push_str(&format!("\t\t\"{c}\"\t\"1\"\n"));
    }
    s.push_str("\t}\n}\n");
    s
}

/// Write `contents` to `path` (creating parent dirs) only if it differs from
/// what's already there, so we don't rewrite the cfg every game tick. Returns
/// `Ok(true)` when a write happened, `Ok(false)` when it was already current.
pub fn write_config_if_changed(path: &Path, contents: &str) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == contents {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(true)
}

// ===========================================================================
// Steam install-dir resolution + shared token
// ===========================================================================

/// The Steam install base (`…\steamapps\common\<installdir>`) of the first
/// running process whose exe name is in `process_names`, resolved from its exe
/// path via the shared [`crate::games::generic::steam`] ancestor-walk. `None`
/// when the game isn't running or isn't a Steam install. Callers join the
/// game-specific cfg subpath onto this.
pub fn steam_install_base(process_names: &[&str]) -> Option<PathBuf> {
    for (_pid, name, exe) in process_snapshot::processes_with_paths(process_snapshot::PATHS_MAX_AGE)
    {
        if !process_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if let Some((steamapps, installdir)) =
            crate::games::generic::steam::steam_library_from_exe(&exe)
        {
            return Some(steamapps.join("common").join(installdir));
        }
    }
    None
}

/// The shared GSI auth token for this install: a random token persisted to
/// `<app config dir>/gsi_token` and reused across launches (and across GSI
/// games — each cfg embeds it, each server validates against it). Generated on
/// first use. Cached for the process lifetime.
pub fn shared_token(app: &AppHandle) -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| load_or_create_token(app)).clone()
}

fn load_or_create_token(app: &AppHandle) -> String {
    let path = app.path().app_config_dir().ok().map(|d| d.join("gsi_token"));
    if let Some(p) = &path {
        if let Ok(t) = std::fs::read_to_string(p) {
            let t = t.trim().to_string();
            if !t.is_empty() {
                return t;
            }
        }
    }
    let token = generate_token();
    if let Some(p) = &path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, &token);
    }
    token
}

/// A 128-bit hex token from time + QPC entropy. Not cryptographic, but ample for
/// a localhost-only shared secret gating a GSI endpoint no remote host can reach.
fn generate_token() -> String {
    let mut acc: u128 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..8 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        acc = acc
            .wrapping_mul(0x0000_0100_0000_01B3)
            .wrapping_add(nanos);
        acc ^= (crate::core::clock::now_ticks() as u128).rotate_left(17);
    }
    format!("hako{acc:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_has_uri_token_and_components() {
        let cfg = config_file("hako", 31761, "abc123", &["map", "round", "provider"]);
        assert!(cfg.contains("\"uri\"\t\t\"http://127.0.0.1:31761/\""));
        assert!(cfg.contains("\"token\"\t\"abc123\""));
        assert!(cfg.contains("\"map\"\t\"1\""));
        assert!(cfg.contains("\"round\"\t\"1\""));
        assert!(cfg.contains("\"provider\"\t\"1\""));
        // Balanced-ish braces (header, auth, data) → 3 open, 3 close.
        assert_eq!(cfg.matches('{').count(), 3);
        assert_eq!(cfg.matches('}').count(), 3);
    }

    #[test]
    fn extracts_auth_token_from_body() {
        let body = r#"{"auth":{"token":"secret"},"map":{"name":"de_dust2"}}"#;
        assert_eq!(payload_token(body).as_deref(), Some("secret"));
        assert_eq!(payload_token("{}"), None);
        assert_eq!(payload_token("not json"), None);
    }

    #[test]
    fn generated_tokens_are_nonempty_and_prefixed() {
        let t = generate_token();
        assert!(t.starts_with("hako"));
        assert!(t.len() > 8);
    }
}
