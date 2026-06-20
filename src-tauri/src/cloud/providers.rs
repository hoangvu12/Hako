//! Provider configs (non-secret, persisted to `cloud_providers.json`) and their
//! secrets (stored in the OS keyring). The split is deliberate: only the config
//! is safe to write to disk in cleartext; keys / service-account JSON go to the
//! Windows Credential Manager via `keyring`, keyed by the provider id.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Keyring "service" all hako provider secrets live under — one entry per
/// provider id, holding a JSON-encoded [`Secrets`].
const KEYRING_SERVICE: &str = "hako-cloud";

/// Non-secret, kind-specific provider config. serde-tagged on `kind`
/// (snake_case: `s3` | `r2` | `b2` | `gcs` | `gdrive` | `dropbox` | `onedrive`).
/// Mirrors `ProviderKind` in api.ts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderKind {
    /// Generic S3-compatible (AWS, MinIO, Wasabi, …). `region` may be empty →
    /// treated as `auto`.
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        prefix: String,
    },
    /// Cloudflare R2 — S3 under the hood, endpoint derived from `account_id`.
    R2 {
        account_id: String,
        bucket: String,
        prefix: String,
    },
    /// Backblaze B2 (native service). `bucket_id` is B2's opaque bucket
    /// identifier (distinct from the human `bucket` name) — required by the B2
    /// API to resolve an upload URL; it's non-secret, so it lives in the config.
    B2 {
        bucket: String,
        bucket_id: String,
        prefix: String,
    },
    /// Google Cloud Storage (native service, service-account JSON auth).
    Gcs { bucket: String, prefix: String },

    // --- Phase 2/3: consumer OAuth clouds ---------------------------------
    // These are *folder*-rooted, not bucket+key: the upload engine sets the
    // OpenDAL operator `root` to `folder` and writes a root-relative key (so
    // `prefix()` is empty for them). Auth is a stored refresh token (see
    // `Secrets`); OpenDAL refreshes the access token itself. None can presign
    // (see `supports_presign`).
    /// Google Drive (`drive.file` scope — app-created files only).
    Gdrive { folder: String },
    /// Dropbox (scoped app, offline/refresh token).
    Dropbox { folder: String },
    /// Microsoft OneDrive (personal / consumers tenant).
    Onedrive { folder: String },
}

impl ProviderKind {
    /// The configured key prefix (may be empty). Surrounding slashes are the
    /// caller's to normalize (see `operator::remote_key`). Folder-rooted clouds
    /// (Drive/Dropbox/OneDrive) carry their path in the operator `root`, not the
    /// key, so they report an empty prefix here.
    pub fn prefix(&self) -> &str {
        match self {
            ProviderKind::S3 { prefix, .. }
            | ProviderKind::R2 { prefix, .. }
            | ProviderKind::B2 { prefix, .. }
            | ProviderKind::Gcs { prefix, .. } => prefix,
            ProviderKind::Gdrive { .. }
            | ProviderKind::Dropbox { .. }
            | ProviderKind::Onedrive { .. } => "",
        }
    }

    /// Whether this backend can presign a read URL (OpenDAL `presign_read`). The
    /// S3-family + GCS can; the consumer OAuth clouds (token-based APIs with no
    /// signed-URL concept) cannot. Cloud retention only evicts a clip's local
    /// copy when at least one of its completed uploads is to a presign-capable
    /// provider, so an evicted clip can always stream-play from its `remote_url`.
    pub fn supports_presign(&self) -> bool {
        matches!(
            self,
            ProviderKind::S3 { .. }
                | ProviderKind::R2 { .. }
                | ProviderKind::B2 { .. }
                | ProviderKind::Gcs { .. }
        )
    }
}

/// A configured cloud target. `id` is the stable handle used as the DB
/// `provider_id` and the keyring entry key. Mirrors `ProviderConfig` in api.ts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub label: String,
    pub kind: ProviderKind,
}

/// Secrets for a provider. Only the fields a given kind needs are populated; the
/// rest stay empty. Stored as a JSON blob in the keyring, never on disk.
/// Mirrors `ProviderSecrets` in api.ts (all fields optional there).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Secrets {
    /// S3/R2 access-key id, or B2 application-key id.
    #[serde(default)]
    pub access_key_id: String,
    /// S3/R2 secret access key, or B2 application key.
    #[serde(default)]
    pub secret_access_key: String,
    /// GCS service-account JSON (the whole file contents).
    #[serde(default)]
    pub gcs_credential_json: String,

    // --- Phase 2/3: consumer OAuth clouds ---------------------------------
    // Populated by the OAuth connect flow (`cloud::oauth`) for Drive/Dropbox/
    // OneDrive; empty for the S3-family. OpenDAL refreshes the short-lived access
    // token from these on every session, so we persist only the long-lived
    // refresh token + the app's client id/secret.
    /// OAuth refresh token (long-lived; the source of every future access token).
    #[serde(default)]
    pub refresh_token: String,
    /// OAuth app client id (public; needed to refresh).
    #[serde(default)]
    pub client_id: String,
    /// OAuth app client secret (where the provider issues one; may be empty).
    #[serde(default)]
    pub client_secret: String,
}

/// `cloud_providers.json` next to `settings.json` in the app config dir.
pub fn providers_file(config_dir: &Path) -> PathBuf {
    config_dir.join("cloud_providers.json")
}

/// Load the configured providers (no secrets). Best-effort: missing / unreadable
/// / invalid file ⇒ empty list (cloud config should never block anything).
pub fn load_providers(config_dir: &Path) -> Vec<ProviderConfig> {
    let path = providers_file(config_dir);
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!("cloud_providers.json parse failed ({e}); using empty list");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Persist the provider list (creates the parent dir). Secrets are NOT here.
pub fn save_providers(config_dir: &Path, providers: &[ProviderConfig]) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("create config dir: {e}"))?;
    let json =
        serde_json::to_string_pretty(providers).map_err(|e| format!("serialize providers: {e}"))?;
    std::fs::write(providers_file(config_dir), json)
        .map_err(|e| format!("write cloud_providers.json: {e}"))
}

/// Look up one provider's config by id.
pub fn find_provider(config_dir: &Path, id: &str) -> Option<ProviderConfig> {
    load_providers(config_dir).into_iter().find(|p| p.id == id)
}

// --- keyring secret I/O ----------------------------------------------------

fn entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, id).map_err(|e| format!("keyring entry: {e}"))
}

/// Store a provider's secrets as a JSON blob in the OS keyring.
pub fn set_secrets(id: &str, secrets: &Secrets) -> Result<(), String> {
    let blob = serde_json::to_string(secrets).map_err(|e| format!("serialize secrets: {e}"))?;
    entry(id)?
        .set_password(&blob)
        .map_err(|e| format!("keyring set: {e}"))
}

/// Read a provider's secrets back from the keyring.
pub fn get_secrets(id: &str) -> Result<Secrets, String> {
    let blob = entry(id)?
        .get_password()
        .map_err(|e| format!("keyring get ({id}): {e}"))?;
    serde_json::from_str(&blob).map_err(|e| format!("parse secrets: {e}"))
}

/// Remove a provider's secrets. A missing entry is not an error (idempotent).
pub fn delete_secrets(id: &str) -> Result<(), String> {
    match entry(id)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_tag_round_trips() {
        let cfg = ProviderConfig {
            id: "r2-main".into(),
            label: "My R2".into(),
            kind: ProviderKind::R2 {
                account_id: "abc123".into(),
                bucket: "clips".into(),
                prefix: "hako".into(),
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        // serde-tagged on `kind`, snake_case variant.
        assert!(json.contains("\"kind\":\"r2\""), "{json}");
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind.prefix(), "hako");
    }

    #[test]
    fn b2_carries_bucket_id() {
        let cfg = ProviderConfig {
            id: "b2-1".into(),
            label: "My B2".into(),
            kind: ProviderKind::B2 {
                bucket: "clips".into(),
                bucket_id: "deadbeef0000".into(),
                prefix: "hako".into(),
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"kind\":\"b2\""), "{json}");
        assert!(json.contains("\"bucket_id\":\"deadbeef0000\""), "{json}");
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        match back.kind {
            ProviderKind::B2 { bucket_id, .. } => assert_eq!(bucket_id, "deadbeef0000"),
            other => panic!("expected B2, got {other:?}"),
        }
    }

    #[test]
    fn providers_json_round_trips_through_disk() {
        let dir = std::env::temp_dir().join("hako_cloud_providers_test");
        let _ = std::fs::remove_file(providers_file(&dir));
        assert!(load_providers(&dir).is_empty());

        let list = vec![ProviderConfig {
            id: "s3-1".into(),
            label: "S3".into(),
            kind: ProviderKind::S3 {
                endpoint: "https://s3.amazonaws.com".into(),
                region: "us-east-1".into(),
                bucket: "b".into(),
                prefix: String::new(),
            },
        }];
        save_providers(&dir, &list).unwrap();
        let back = load_providers(&dir);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, "s3-1");
        let _ = std::fs::remove_file(providers_file(&dir));
    }

    #[test]
    fn secrets_default_is_empty() {
        let s = Secrets::default();
        assert!(s.access_key_id.is_empty() && s.gcs_credential_json.is_empty());
    }
}
