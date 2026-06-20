//! Interactive OAuth login for the consumer clouds (Google Drive / Dropbox /
//! OneDrive). One parameterized Authorization-Code + **PKCE** flow over a
//! loopback redirect; the only per-provider differences are the endpoints,
//! scopes, and the extra params that make the provider return a *refresh* token.
//!
//! Flow (`authorize`):
//! 1. `tauri-plugin-oauth` starts a `127.0.0.1:<port>` server and hands us each
//!    redirect URL via a callback. We bridge that callback to async with a
//!    oneshot channel.
//! 2. We build the provider's consent URL (`oauth2` crate, PKCE challenge,
//!    `redirect_uri = http://127.0.0.1:<port>`) and open it in the system browser.
//! 3. The user consents → the provider redirects to the loopback with `?code=..`
//!    → we validate `state` against our CSRF token → exchange the code for an
//!    **access + refresh** token.
//! 4. We persist the *refresh token* (+ client id/secret) in the OS keyring via
//!    [`providers::set_secrets`] and write the [`ProviderConfig`]. From then on
//!    OpenDAL refreshes the short-lived access token itself (see `operator`).
//!
//! Credentials: the *app's* OAuth client id/secret are not user secrets — they
//! identify Hako to the provider. They're read from env vars (overridable per
//! machine) with an optional compile-time fallback baked in via `option_env!`.
//! Until they're configured, the connect commands return an actionable error.
//! See `handoff-cloud-phase2.md` §3 for the per-provider console setup.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    Scope, TokenResponse, TokenUrl,
};
use tauri::AppHandle;

use super::providers::{self, ProviderConfig, ProviderKind, Secrets};

/// How long we wait for the user to finish the browser consent before giving up.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Fixed loopback ports tried in order. Fixed (not random) so the redirect URI
/// is predictable: providers that require an exactly-registered redirect URI
/// (Dropbox, Microsoft) should have each `http://127.0.0.1:<port>` registered.
/// Google's "Desktop app" client special-cases loopback and accepts any port.
const REDIRECT_PORTS: &[u16] = &[51900, 51901, 51902, 51903];

/// Static, per-provider OAuth parameters.
struct OAuthProvider {
    /// Display name used in error messages and the default provider label.
    display: &'static str,
    /// Authorization (consent) endpoint.
    auth_url: &'static str,
    /// Token endpoint (code → tokens, and later refresh).
    token_url: &'static str,
    /// Requested scopes (least-privilege; see the per-provider docs).
    scopes: &'static [&'static str],
    /// Extra auth-request params that make the provider issue a refresh token
    /// (Google `access_type=offline` + `prompt=consent`; Dropbox
    /// `token_access_type=offline`; OneDrive uses the `offline_access` scope).
    extra_auth_params: &'static [(&'static str, &'static str)],
    /// Env var holding the app's OAuth client id, and its compile-time fallback.
    client_id_env: &'static str,
    client_id_builtin: Option<&'static str>,
    /// Env var holding the app's OAuth client secret, and its fallback. Optional:
    /// public desktop clients (OneDrive personal, Dropbox PKCE) issue none.
    client_secret_env: &'static str,
    client_secret_builtin: Option<&'static str>,
}

const GOOGLE: OAuthProvider = OAuthProvider {
    display: "Google Drive",
    auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    // `drive.file`: app-created files only — least-privilege, smallest review
    // surface for Google's app verification.
    scopes: &["https://www.googleapis.com/auth/drive.file"],
    extra_auth_params: &[("access_type", "offline"), ("prompt", "consent")],
    client_id_env: "HAKO_GOOGLE_CLIENT_ID",
    client_id_builtin: option_env!("HAKO_GOOGLE_CLIENT_ID"),
    client_secret_env: "HAKO_GOOGLE_CLIENT_SECRET",
    client_secret_builtin: option_env!("HAKO_GOOGLE_CLIENT_SECRET"),
};

const DROPBOX: OAuthProvider = OAuthProvider {
    display: "Dropbox",
    auth_url: "https://www.dropbox.com/oauth2/authorize",
    token_url: "https://api.dropboxapi.com/oauth2/token",
    scopes: &["files.content.write", "files.content.read", "account_info.read"],
    extra_auth_params: &[("token_access_type", "offline")],
    client_id_env: "HAKO_DROPBOX_CLIENT_ID",
    client_id_builtin: option_env!("HAKO_DROPBOX_CLIENT_ID"),
    client_secret_env: "HAKO_DROPBOX_CLIENT_SECRET",
    client_secret_builtin: option_env!("HAKO_DROPBOX_CLIENT_SECRET"),
};

const ONEDRIVE: OAuthProvider = OAuthProvider {
    display: "OneDrive",
    // `consumers` tenant — personal Microsoft accounts (OneDrive personal).
    auth_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
    // `offline_access` is what makes Microsoft return a refresh token.
    scopes: &["Files.ReadWrite", "offline_access"],
    extra_auth_params: &[],
    client_id_env: "HAKO_MICROSOFT_CLIENT_ID",
    client_id_builtin: option_env!("HAKO_MICROSOFT_CLIENT_ID"),
    client_secret_env: "HAKO_MICROSOFT_CLIENT_SECRET",
    client_secret_builtin: option_env!("HAKO_MICROSOFT_CLIENT_SECRET"),
};

/// The tokens + app credentials we persist after a successful connect.
struct Connected {
    refresh_token: String,
    client_id: String,
    client_secret: Option<String>,
}

/// Resolve the app's OAuth client id (required) and secret (optional) from the
/// environment, falling back to any compile-time-embedded values.
fn resolve_credentials(p: &OAuthProvider) -> Result<(String, Option<String>), String> {
    let pick = |env: &str, builtin: Option<&'static str>| -> Option<String> {
        std::env::var(env)
            .ok()
            .or_else(|| builtin.map(str::to_string))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let client_id = pick(p.client_id_env, p.client_id_builtin).ok_or_else(|| {
        format!(
            "{} OAuth is not configured on this build. Set the {} environment variable \
             (and {} where the provider requires a secret) to your registered OAuth app's \
             credentials — see handoff-cloud-phase2.md §3.",
            p.display, p.client_id_env, p.client_secret_env
        )
    })?;
    let client_secret = pick(p.client_secret_env, p.client_secret_builtin);
    Ok((client_id, client_secret))
}

/// What we pull out of the loopback redirect.
struct Redirect {
    code: String,
    state: String,
}

/// Parse the loopback redirect URL into `(code, state)`, or surface the
/// provider's `error`/`error_description` if the user denied consent.
fn parse_redirect(raw: &str) -> Result<Redirect, String> {
    let url = url::Url::parse(raw).map_err(|e| format!("parse redirect url: {e}"))?;
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_desc = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => error_desc = Some(v.into_owned()),
            _ => {}
        }
    }
    if let Some(err) = error {
        return Err(format!(
            "authorization denied: {}{}",
            err,
            error_desc.map(|d| format!(" ({d})")).unwrap_or_default()
        ));
    }
    Ok(Redirect {
        code: code.ok_or("redirect had no authorization code")?,
        state: state.unwrap_or_default(),
    })
}

/// Run the full interactive flow for one provider and return the refresh token
/// (+ the app credentials, so they can be stored alongside for refresh).
async fn authorize(p: &OAuthProvider) -> Result<Connected, String> {
    let (client_id, client_secret) = resolve_credentials(p)?;

    // 1. Loopback server. Bridge its (sync) per-redirect callback to async via a
    //    oneshot the closure fills exactly once.
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Redirect, String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let cb_tx = tx.clone();
    let config = tauri_plugin_oauth::OauthConfig {
        ports: Some(REDIRECT_PORTS.to_vec()),
        response: Some(SUCCESS_HTML.into()),
    };
    let port = tauri_plugin_oauth::start_with_config(config, move |url| {
        if let Ok(mut slot) = cb_tx.lock() {
            if let Some(sender) = slot.take() {
                let _ = sender.send(parse_redirect(&url));
            }
        }
    })
    .map_err(|e| format!("start loopback oauth server: {e}"))?;

    // Ensure the server is torn down on every exit path from here on.
    let _guard = ServerGuard(port);

    let redirect_uri = format!("http://127.0.0.1:{port}");

    // 2. Build the oauth2 client + the consent URL with a PKCE challenge.
    let mut client = BasicClient::new(ClientId::new(client_id.clone()))
        .set_auth_uri(AuthUrl::new(p.auth_url.to_string()).map_err(|e| format!("auth url: {e}"))?)
        .set_token_uri(
            TokenUrl::new(p.token_url.to_string()).map_err(|e| format!("token url: {e}"))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri).map_err(|e| format!("redirect url: {e}"))?,
        );
    if let Some(secret) = &client_secret {
        client = client.set_client_secret(ClientSecret::new(secret.clone()));
    }

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut auth = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    for s in p.scopes {
        auth = auth.add_scope(Scope::new((*s).to_string()));
    }
    for (k, v) in p.extra_auth_params {
        auth = auth.add_extra_param(*k, *v);
    }
    let (auth_url, csrf) = auth.url();

    // 3. Hand the user off to their browser, then wait for the redirect.
    opener::open_browser(auth_url.as_str())
        .map_err(|e| format!("open browser for consent: {e}"))?;

    let redirect = tokio::time::timeout(CONSENT_TIMEOUT, rx)
        .await
        .map_err(|_| "timed out waiting for authorization (no consent within 5 minutes)".to_string())?
        .map_err(|_| "authorization was canceled".to_string())??;

    // CSRF: the returned state must match the token we generated.
    if redirect.state != *csrf.secret() {
        return Err("authorization state mismatch (possible CSRF) — aborted".into());
    }

    // 4. Exchange the code for tokens. Redirects disabled (SSRF guard).
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("build oauth http client: {e}"))?;
    let token = client
        .exchange_code(AuthorizationCode::new(redirect.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http)
        .await
        .map_err(|e| format!("exchange authorization code: {e}"))?;

    let refresh_token = token
        .refresh_token()
        .map(|t| t.secret().to_string())
        .ok_or_else(|| {
            format!(
                "{} did not return a refresh token. Re-check that offline access is enabled \
                 for the OAuth app (see handoff-cloud-phase2.md §3).",
                p.display
            )
        })?;

    Ok(Connected {
        refresh_token,
        client_id,
        client_secret,
    })
}

/// Persist a freshly-connected provider: secrets to the keyring, config to
/// `cloud_providers.json`. Returns the stored config (with its assigned id).
fn finish_connect(
    app: &AppHandle,
    kind: ProviderKind,
    label: Option<String>,
    tokens: Connected,
) -> Result<ProviderConfig, String> {
    let dir = super::config_dir(app)?;
    let id = super::new_provider_id();
    let label = label
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| default_label(&kind));
    let config = ProviderConfig {
        id: id.clone(),
        label,
        kind,
    };

    let secrets = Secrets {
        refresh_token: tokens.refresh_token,
        client_id: tokens.client_id,
        client_secret: tokens.client_secret.unwrap_or_default(),
        ..Default::default()
    };
    // Secrets first, so a config that lands on disk always has its keyring entry.
    providers::set_secrets(&id, &secrets)?;

    let mut list = providers::load_providers(&dir);
    list.push(config.clone());
    providers::save_providers(&dir, &list)?;
    Ok(config)
}

fn default_label(kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::Gdrive { .. } => "Google Drive",
        ProviderKind::Dropbox { .. } => "Dropbox",
        ProviderKind::Onedrive { .. } => "OneDrive",
        _ => "Cloud",
    }
    .to_string()
}

/// Normalize the user-entered folder, defaulting to `/Hako` when blank. Stored
/// as-is on the config; `operator::build_operator` turns it into the OpenDAL
/// `root`.
fn folder_or_default(folder: Option<String>) -> String {
    folder
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "/Hako".to_string())
}

// --- commands --------------------------------------------------------------

/// Connect a Google Drive account via OAuth. Opens the system browser for
/// consent, stores the refresh token in the keyring, and returns the new config.
#[tauri::command]
pub async fn cloud_connect_gdrive(
    app: AppHandle,
    folder: Option<String>,
    label: Option<String>,
) -> Result<ProviderConfig, String> {
    let tokens = authorize(&GOOGLE).await?;
    finish_connect(
        &app,
        ProviderKind::Gdrive {
            folder: folder_or_default(folder),
        },
        label,
        tokens,
    )
}

/// Connect a Dropbox account via OAuth (offline/refresh token).
#[tauri::command]
pub async fn cloud_connect_dropbox(
    app: AppHandle,
    folder: Option<String>,
    label: Option<String>,
) -> Result<ProviderConfig, String> {
    let tokens = authorize(&DROPBOX).await?;
    finish_connect(
        &app,
        ProviderKind::Dropbox {
            folder: folder_or_default(folder),
        },
        label,
        tokens,
    )
}

/// Connect a Microsoft OneDrive (personal) account via OAuth.
#[tauri::command]
pub async fn cloud_connect_onedrive(
    app: AppHandle,
    folder: Option<String>,
    label: Option<String>,
) -> Result<ProviderConfig, String> {
    let tokens = authorize(&ONEDRIVE).await?;
    finish_connect(
        &app,
        ProviderKind::Onedrive {
            folder: folder_or_default(folder),
        },
        label,
        tokens,
    )
}

/// Cancels the loopback server when the flow ends (success, error, or timeout)
/// so a dropped flow never leaves a port bound.
struct ServerGuard(u16);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = tauri_plugin_oauth::cancel(self.0);
    }
}

/// The page shown in the browser tab after the redirect lands.
const SUCCESS_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Hako — Connected</title>
<style>
  body { font-family: -apple-system, Segoe UI, sans-serif; background:#0f0f12; color:#e6e6ea;
         display:flex; align-items:center; justify-content:center; height:100vh; margin:0; }
  .card { text-align:center; }
  h1 { font-size:1.4rem; margin:0 0 .5rem; }
  p { color:#9a9aa6; }
</style></head>
<body><div class="card">
  <h1>✓ Connected to Hako</h1>
  <p>You can close this tab and return to the app.</p>
</div></body></html>"#;
