//! Multi-provider cloud upload, built on Apache OpenDAL so one code path serves
//! S3 / Cloudflare R2 / Backblaze B2 / GCS (v1; gdrive/dropbox/onedrive in
//! Phase 2/3). Provider configs live in `cloud_providers.json`; their secrets in
//! the OS keyring; per-clip upload state in the `cloud_uploads` SQLite table.
//!
//! - [`providers`] — config + secret (de)serialization (keyring + JSON).
//! - [`operator`]  — config → `opendal::Operator`, remote-key scheme, presign.
//! - [`upload`]    — the queued, chunked, cancelable upload engine + its state.
//!
//! The `#[tauri::command]` handlers here are thin: they resolve the config dir,
//! build an Operator, and delegate. They return `Result<T, String>` to match the
//! rest of the command surface and are registered in `main.rs`.

#![allow(dead_code)]

pub mod download;
pub mod oauth;
pub mod operator;
pub mod providers;
pub mod retention;
pub mod upload;

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub use providers::{ProviderConfig, Secrets};
pub use upload::CloudState;

/// App config dir (where `settings.json` and `cloud_providers.json` live).
fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("resolve config dir: {e}"))
}

/// A fresh, unique provider id (used when the caller didn't supply one). Derived
/// from the wall clock in nanos — no uuid dep, and providers are added rarely
/// enough by hand that collision is a non-issue.
fn new_provider_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("provider-{nanos}")
}

// --- provider management commands ------------------------------------------

/// Configured providers (no secrets).
#[tauri::command]
pub fn cloud_list_providers(app: AppHandle) -> Result<Vec<ProviderConfig>, String> {
    Ok(providers::load_providers(&config_dir(&app)?))
}

/// Add (or replace, by id) a provider: secrets to the keyring, config to
/// `cloud_providers.json`. Returns the stored config (with its assigned id).
#[tauri::command]
pub fn cloud_add_provider(
    app: AppHandle,
    mut config: ProviderConfig,
    secrets: Secrets,
) -> Result<ProviderConfig, String> {
    let dir = config_dir(&app)?;
    if config.id.trim().is_empty() {
        config.id = new_provider_id();
    }
    // Secrets first, so a config that lands on disk always has its keyring entry.
    providers::set_secrets(&config.id, &secrets)?;

    let mut list = providers::load_providers(&dir);
    match list.iter_mut().find(|p| p.id == config.id) {
        Some(slot) => *slot = config.clone(),
        None => list.push(config.clone()),
    }
    providers::save_providers(&dir, &list)?;
    Ok(config)
}

/// Remove a provider (config + keyring secrets). Idempotent.
#[tauri::command]
pub fn cloud_remove_provider(app: AppHandle, id: String) -> Result<(), String> {
    let dir = config_dir(&app)?;
    let mut list = providers::load_providers(&dir);
    list.retain(|p| p.id != id);
    providers::save_providers(&dir, &list)?;
    let _ = providers::delete_secrets(&id); // best-effort; a stale entry is harmless
    Ok(())
}

/// Test a configured provider's connectivity + credentials via `op.check()`.
#[tauri::command]
pub async fn cloud_test_provider(app: AppHandle, id: String) -> Result<(), String> {
    let dir = config_dir(&app)?;
    let cfg =
        providers::find_provider(&dir, &id).ok_or_else(|| format!("no such provider: {id}"))?;
    let secrets = providers::get_secrets(&id)?;
    let op = operator::build_operator(&cfg, &secrets)?;
    op.check().await.map_err(|e| operator::friendly_error(&e))
}
