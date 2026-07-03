//! `ProviderConfig` + `Secrets` → an OpenDAL [`Operator`], plus the remote-key
//! scheme and a presign helper. The Operator is the single upload/test code path
//! shared by every provider.
//!
//! API note (OpenDAL 0.54, verified against its docs): builders use the
//! consuming-chaining style (`S3::default().bucket(..).region(..)`); `Operator::
//! new(builder)?.finish()` yields the Operator, and layers are applied on the
//! finished Operator. The `RetryLayer` retries `Error::is_temporary()` failures
//! internally — our analogue of Medal's `RetryHandler`.

use std::path::Path;
use std::time::Duration;

use opendal::layers::{RetryLayer, TimeoutLayer};
use opendal::{services, Operator};

use super::providers::{ProviderConfig, ProviderKind, Secrets};

/// `impl Display` error → `String`, to match the codebase's `Result<T, String>`.
fn e2s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Build a retry+timeout-wrapped [`Operator`] for a provider and its secrets.
///
/// The remote *key* (object path) is computed separately by [`remote_key`] and
/// passed to writer/read calls — we don't set the builder `root`, so the key is
/// the full path under the bucket.
pub fn build_operator(cfg: &ProviderConfig, secrets: &Secrets) -> Result<Operator, String> {
    let op = match &cfg.kind {
        ProviderKind::S3 {
            endpoint,
            region,
            bucket,
            ..
        } => {
            let mut builder = services::S3::default()
                .region(if region.is_empty() {
                    "auto"
                } else {
                    region.as_str()
                })
                .bucket(bucket)
                .access_key_id(&secrets.access_key_id)
                .secret_access_key(&secrets.secret_access_key);
            // Only set a custom endpoint when one was given (empty ⇒ real AWS).
            // Normalize it so a scheme-less host (e.g. `s3.example.com`) doesn't
            // make reqwest hang on connect instead of failing fast.
            let endpoint = normalize_endpoint(endpoint);
            if !endpoint.is_empty() {
                builder = builder.endpoint(&endpoint);
            }
            Operator::new(builder).map_err(e2s)?.finish()
        }
        ProviderKind::R2 {
            account_id, bucket, ..
        } => {
            // R2 is S3-compatible; region is always "auto" and the endpoint is
            // derived from the account id.
            let builder = services::S3::default()
                .endpoint(&format!("https://{account_id}.r2.cloudflarestorage.com"))
                .region("auto")
                .bucket(bucket)
                .access_key_id(&secrets.access_key_id)
                .secret_access_key(&secrets.secret_access_key);
            Operator::new(builder).map_err(e2s)?.finish()
        }
        ProviderKind::B2 {
            bucket, bucket_id, ..
        } => {
            // B2 native service. It needs BOTH the human bucket name and B2's
            // opaque `bucket_id` (used to fetch the per-bucket upload URL); the
            // key pair is the application-key id + application key.
            let builder = services::B2::default()
                .application_key_id(&secrets.access_key_id)
                .application_key(&secrets.secret_access_key)
                .bucket(bucket)
                .bucket_id(bucket_id);
            Operator::new(builder).map_err(e2s)?.finish()
        }
        ProviderKind::Gcs { bucket, .. } => {
            // GCS native service, service-account JSON auth. OpenDAL's
            // `credential` expects the JSON *base64-encoded* (verified against
            // the 0.54 service docs), so we encode the pasted JSON here and keep
            // the keyring copy as readable raw JSON.
            use base64::Engine;
            let credential = base64::engine::general_purpose::STANDARD
                .encode(secrets.gcs_credential_json.trim());
            let builder = services::Gcs::default()
                .bucket(bucket)
                .credential(&credential);
            Operator::new(builder).map_err(e2s)?.finish()
        }
        // --- Phase 2/3: consumer OAuth clouds -----------------------------
        // All three are folder-rooted (we set `.root(folder)` and write a
        // root-relative key — see `remote_key`, whose `prefix()` is empty for
        // these). Auth is the stored refresh token + the app's client id/secret;
        // OpenDAL mints and refreshes the short-lived access token itself.
        ProviderKind::Gdrive { folder } => {
            let builder = services::Gdrive::default()
                .root(&normalize_root(folder))
                .refresh_token(&secrets.refresh_token)
                .client_id(&secrets.client_id)
                .client_secret(&secrets.client_secret);
            Operator::new(builder).map_err(e2s)?.finish()
        }
        ProviderKind::Dropbox { folder } => {
            let builder = services::Dropbox::default()
                .root(&normalize_root(folder))
                .refresh_token(&secrets.refresh_token)
                .client_id(&secrets.client_id)
                .client_secret(&secrets.client_secret);
            Operator::new(builder).map_err(e2s)?.finish()
        }
        ProviderKind::Onedrive { folder } => {
            let builder = services::Onedrive::default()
                .root(&normalize_root(folder))
                .refresh_token(&secrets.refresh_token)
                .client_id(&secrets.client_id)
                .client_secret(&secrets.client_secret);
            Operator::new(builder).map_err(e2s)?.finish()
        }
    };

    Ok(op
        // Retries transient/`is_temporary` errors (network blips, 5xx, throttling)
        // with exponential backoff + jitter before surfacing a terminal failure.
        .layer(RetryLayer::new().with_max_times(4).with_jitter())
        .layer(TimeoutLayer::new()))
}

/// Normalize a user-entered S3 endpoint: trim, drop a trailing slash, and add
/// `https://` when no scheme was typed (a bare host otherwise yields a URL
/// reqwest can't dial, so the upload hangs on connect instead of erroring).
/// Empty stays empty (⇒ default AWS endpoint).
fn normalize_endpoint(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.is_empty() || e.starts_with("http://") || e.starts_with("https://") {
        e.to_string()
    } else {
        format!("https://{e}")
    }
}

/// Normalize a user-entered folder into an OpenDAL `root`: a single leading
/// slash, no trailing slash, never empty. `"Hako"` / `"/Hako/"` → `"/Hako"`;
/// `""` → `"/"` (the account root). Folder-rooted backends (Drive/Dropbox/
/// OneDrive) take this; the date-bucketed object key is then root-relative.
fn normalize_root(folder: &str) -> String {
    let f = folder.trim().trim_matches('/');
    if f.is_empty() {
        "/".to_string()
    } else {
        format!("/{f}")
    }
}

/// The remote object key for a clip under a provider's prefix:
/// `<prefix>/<yyyy>/<mm>/<file-name>` — date-bucketed (derived from the clip's
/// unix-ms creation time) so the bucket is browsable by month. `prefix` is
/// trimmed of surrounding slashes so the key never doubles or leads with `/`.
pub fn remote_key(kind: &ProviderKind, created_unix_ms: i64, local_path: &str) -> String {
    let name = Path::new(local_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("clip.mp4");
    let (year, month) = year_month_utc(created_unix_ms);
    let prefix = kind.prefix().trim_matches('/');
    if prefix.is_empty() {
        format!("{year}/{month:02}/{name}")
    } else {
        format!("{prefix}/{year}/{month:02}/{name}")
    }
}

/// Classify an OpenDAL error into a short, user-facing message. The RetryLayer
/// has already handled transient cases by the time a terminal error reaches here,
/// so these map the *permanent* failures the user can act on.
pub fn friendly_error(e: &opendal::Error) -> String {
    use opendal::ErrorKind;
    let hint = match e.kind() {
        ErrorKind::NotFound => "not found — check the bucket name",
        ErrorKind::PermissionDenied => "permission denied — check your keys and bucket policy",
        ErrorKind::RateLimited => "rate limited by the provider",
        ErrorKind::Unsupported => "operation not supported by this provider",
        ErrorKind::ConfigInvalid => "invalid provider configuration",
        _ => "cloud error",
    };
    format!("{hint}: {e}")
}

/// Presign a GET URL for `key`, valid for `expire`. Returns `Ok(None)` when the
/// provider can't presign (then the upload row's `remote_url` stays null). S3 /
/// R2 / B2 support presign; we store the URL as a refreshable convenience for
/// playing cloud-only clips, never as a source of truth (presigned URLs expire).
pub async fn presign_get(
    op: &Operator,
    key: &str,
    expire: Duration,
) -> Result<Option<String>, String> {
    if !op.info().native_capability().presign_read {
        return Ok(None);
    }
    let req = op.presign_read(key, expire).await.map_err(e2s)?;
    Ok(Some(req.uri().to_string()))
}

/// UTC `(year, month)` from a unix-ms timestamp via Howard Hinnant's
/// days→civil algorithm — avoids pulling in `chrono` just for two fields.
fn year_month_utc(unix_ms: i64) -> (i64, u32) {
    let days = unix_ms.div_euclid(86_400_000); // days since 1970-01-01 (UTC)
    let z = days + 719_468; // shift epoch to 0000-03-01
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day-of-era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // month, shifted [0, 11]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_month_matches_known_dates() {
        // 2026-06-20T00:00:00Z = 1_781_913_600_000 ms.
        assert_eq!(year_month_utc(1_781_913_600_000), (2026, 6));
        // Epoch.
        assert_eq!(year_month_utc(0), (1970, 1));
        // 2000-03-01 (just after a leap day) = 951_868_800_000 ms.
        assert_eq!(year_month_utc(951_868_800_000), (2000, 3));
        // 2000-02-29 (leap day) = 951_782_400_000 ms.
        assert_eq!(year_month_utc(951_782_400_000), (2000, 2));
    }

    #[test]
    fn remote_key_layout() {
        let with_prefix = ProviderKind::R2 {
            account_id: "a".into(),
            bucket: "b".into(),
            prefix: "/hako/".into(), // surrounding slashes normalized
        };
        assert_eq!(
            remote_key(&with_prefix, 1_781_913_600_000, r"C:\vids\hako_clip_42.mp4"),
            "hako/2026/06/hako_clip_42.mp4"
        );

        let no_prefix = ProviderKind::S3 {
            endpoint: String::new(),
            region: String::new(),
            bucket: "b".into(),
            prefix: String::new(),
        };
        assert_eq!(
            remote_key(&no_prefix, 0, "/home/u/clip.mp4"),
            "1970/01/clip.mp4"
        );
    }

    #[test]
    fn folder_backends_use_root_not_key_prefix() {
        // Folder-rooted clouds carry their path in the operator `root`, so the
        // object key is root-relative (no folder doubling) — `prefix()` is empty.
        let gdrive = ProviderKind::Gdrive {
            folder: "/Hako/".into(),
        };
        assert_eq!(gdrive.prefix(), "");
        assert_eq!(
            remote_key(&gdrive, 1_781_913_600_000, r"C:\vids\hako_clip_42.mp4"),
            "2026/06/hako_clip_42.mp4"
        );
        assert!(!gdrive.supports_presign());
    }

    #[test]
    fn normalize_endpoint_adds_scheme() {
        assert_eq!(
            normalize_endpoint("s3.example.com"),
            "https://s3.example.com"
        );
        assert_eq!(
            normalize_endpoint("https://s3.example.com/"),
            "https://s3.example.com"
        );
        assert_eq!(
            normalize_endpoint("http://localhost:9000"),
            "http://localhost:9000"
        );
        assert_eq!(normalize_endpoint("  "), "");
        assert_eq!(normalize_endpoint(""), "");
    }

    #[test]
    fn normalize_root_forms() {
        assert_eq!(normalize_root("Hako"), "/Hako");
        assert_eq!(normalize_root("/Hako/"), "/Hako");
        assert_eq!(normalize_root("  /a/b/ "), "/a/b");
        assert_eq!(normalize_root(""), "/");
        assert_eq!(normalize_root("/"), "/");
    }
}
