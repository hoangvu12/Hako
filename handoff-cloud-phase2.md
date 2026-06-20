# Handoff — Cloud Upload Phase 2/3 (consumer OAuth clouds): START HERE

This is the next chapter after v1 ("bring your own bucket"). **v1 is code-complete**
(S3 / R2 / B2 / GCS, upload engine, retention, evicted-clip playback + download-to-edit).
See `handoff-cloud-upload-next.md` + `docs/cloud-upload-plan.md` §8 for the v1 state and
the original Phase-2 sketch. This file = everything you need to build Phase 2/3 without
re-discovering the traps.

**Phase 2 = Google Drive. Phase 3 = Dropbox + OneDrive.** Same module/command surface
as v1; only auth (OAuth) and one playback wrinkle differ.

> Nothing about v1 is committed yet — it's all in the working tree. Decide whether to
> commit v1 before starting Phase 2 (recommended: commit v1 first so this is a clean base).

---

## 0. The ONE design decision to make first (don't skip)

**Consumer clouds can't presign.** OpenDAL's `presign_read` is **unsupported** on
Gdrive, Dropbox, and OneDrive (verified against the 0.54 service docs — OneDrive's doc
says it outright; Gdrive/Dropbox are token-based APIs with no signed-URL concept).

v1's evicted-clip story leans entirely on presign:
- `cloud/operator.rs::presign_get` → stored as `cloud_uploads.remote_url`.
- The library grid (`clip-card.tsx::ClipPreview`) and the editor (`clip-viewer.tsx`)
  play **evicted** clips straight from that `remote_url`.

For a Drive/Dropbox/OneDrive-backed clip, `presign_get` returns `None` → `remote_url`
is null → **an evicted clip can't stream-play**. You must pick one:

1. **Gate retention on presign support (simplest, ship Phase 2 fast).** Don't evict
   clips whose only cloud copy is a non-presign provider. Add a `supports_presign()`
   to `ProviderKind` and filter `db::evictable_clips` / the retention candidate query.
   Cost: Drive/Dropbox/OneDrive give *backup* but not *local-space reclamation*. Honest
   and safe; document it in the Cloud settings UI.

2. **Local streaming proxy (the "right" fix, more work).** Register a custom URI scheme
   (mirror the existing `hakoclip://` range reader in `src-tauri/src/media.rs`) that
   proxies HTTP range requests to `op.read_with(key).range(..)`. Then evicted-clip
   playback works for *every* backend by pointing `<video src>` at e.g.
   `hakocloud://<clip_id>` instead of a presigned URL. `cloud/download.rs::stream_to_file`
   is already 90% of the read loop — refactor its ranged-GET core into a shared reader
   the proxy and the downloader both call.

3. **Download-before-play.** Reuse the new `cloud_download_clip` to rehydrate on open.
   Heavy (full file before first frame); only acceptable as a last resort.

**Recommendation: ship Phase 2 with option 1, then do option 2 as a fast-follow** so
free-up-space works uniformly. Wire the decision into `EvictRow`/`evictable_clips` and
the settings copy. Everything else below assumes this is settled.

---

## 1. Verified OpenDAL 0.54 API facts (via Context7 — re-verify, builders drift)

Builders, consuming-chaining style like v1 (`X::default().a(..).b(..)` →
`Operator::new(b)?.finish()`), layers on the finished op:

```rust
// Google Drive — token auth. The doc example shows only `.access_token(..)`, but the
// service ALSO accepts refresh_token + client_id + client_secret for auto-refresh
// (that's what we want for desktop). VERIFY these three setter names against the
// 0.54 gdrive builder source before relying on them.
services::Gdrive::default().root("/Hako").access_token(tok)        // short-lived
// or (preferred, auto-refreshing):
services::Gdrive::default().root("/Hako")
    .refresh_token(rt).client_id(id).client_secret(secret)

// OneDrive — CONFIRMED: refresh_token + client_id (+ optional client_secret), auto-refresh.
services::Onedrive::default().root("/Hako").refresh_token(rt).client_id(id)

// Dropbox — doc shows `.access_token(..)`. Dropbox short-lived tokens expire in 4h;
// long-lived needs refresh_token + app key/secret. VERIFY whether 0.54 Dropbox builder
// exposes refresh_token/client_id/client_secret; if not, you must refresh tokens
// yourself with the `oauth2` crate and feed a fresh access_token each session.
services::Dropbox::default().root("/Hako").access_token(tok)
```

- **Presign: unsupported on all three.** `op.info().native_capability().presign_read`
  is false → `presign_get` already returns `Ok(None)` (no code change needed; this is
  exactly why §0 matters).
- **opendal features** (Cargo.toml, keep the `services-*` plural for 0.54):
  add `"services-gdrive"`, `"services-dropbox"`, `"services-onedrive"`. Still NO
  `rustls` feature on opendal — TLS comes from the project's reqwest (see the long note
  already in Cargo.toml). Don't bump opendal past 0.54 (0.56 renamed to `service-*`).
- `op.check()` (test command) and `op.read_with().range()` / `op.writer_with()` (upload
  + download) work identically across backends — the v1 upload/download/test code paths
  are backend-agnostic and need **zero** changes for Phase 2.

---

## 2. Dependencies to add (`src-tauri/Cargo.toml`)

```toml
# Phase 2/3: consumer-cloud OAuth. opendal handles token *refresh*; these handle the
# interactive *login* (browser consent → authorization code → refresh token).
oauth2 = "5"
tauri-plugin-oauth = "2"   # Tauri v2 — matches this project's plugins. Loopback server.
```
The project is **Tauri v2** (all plugins are v2), so `tauri-plugin-oauth = "2"` fits.
Register the plugin in `main.rs` (`.plugin(tauri_plugin_oauth::init())` or per its v2
README — check the exact init for the pinned version).

---

## 3. OAuth flow (one implementation, parameterized per provider)

New `src-tauri/src/cloud/oauth.rs` (the plan reserved this slot). Authorization Code +
**PKCE**, loopback redirect:

1. `tauri-plugin-oauth` starts a `127.0.0.1:<random>` server and returns the port.
2. Build the provider's auth URL (`oauth2` crate, `AuthorizationCode` + `PkceCodeChallenge`)
   with `redirect_uri = http://127.0.0.1:<port>`; open it in the system browser
   (`tauri_plugin_opener` / `shell open` — check what's already available; v1 uses none).
3. User consents → provider redirects to the loopback with `?code=...` → plugin hands
   you the code → exchange (`oauth2` token endpoint) for **access + refresh tokens**.
4. Store the **refresh token** (+ client_id/secret) in the keyring via the existing
   `providers::set_secrets` (extend `Secrets`, see §4). Create the `ProviderConfig`.
5. From then on OpenDAL auto-refreshes the access token (Gdrive/OneDrive). For Dropbox,
   if the 0.54 builder lacks refresh support, refresh via `oauth2` on each session and
   pass a fresh `access_token` into `build_operator` (resolve in `cloud/oauth.rs`).

**Per-provider console setup (external, do early — has lead time):**
- **Google**: Cloud Console → OAuth consent screen + "Desktop app" client → client
  id/secret. Scope: `https://www.googleapis.com/auth/drive.file` (app-created files only;
  least-privilege, avoids the worst of verification). **Plan for Google's app
  verification** (unverified test-user cap ~100; review takes days–weeks). This is
  calendar time, not code — kick it off first.
- **Dropbox**: App Console → scoped app → app key/secret; enable `files.content.write`
  / `files.content.read`; turn on refresh tokens (offline access).
- **Microsoft**: Entra/Azure app registration → client id; redirect `http://localhost`;
  scopes `Files.ReadWrite offline_access`. Personal-account (consumers) vs work/school
  matters — OneDrive personal uses the `consumers` / `common` tenant.

---

## 4. Code touch points (maps onto the existing v1 structures)

**Rust**
- `cloud/providers.rs`
  - `ProviderKind`: add `GDrive { folder: String }`, `Dropbox { folder: String }`,
    `OneDrive { folder: String }` (serde snake_case → `g_drive`? — choose explicit
    `rename` tags: `gdrive`/`dropbox`/`onedrive` to match api.ts). Update `prefix()` —
    these use a *folder/root*, not an S3 key prefix; decide how `remote_key` maps
    (likely set the builder `.root(folder)` and keep `remote_key` relative). NOTE: v1's
    `remote_key` is currently absolute-from-bucket; for root-based backends you may want
    `build_operator` to set `.root(folder)` and pass a root-relative key.
  - `Secrets`: add `#[serde(default)] refresh_token`, `client_id`, `client_secret`
    (all `String`). Keep them empty for S3-family. Keyring storage already handles
    arbitrary `Secrets` JSON — no I/O changes.
  - `supports_presign()` helper on `ProviderKind` (for §0 option 1).
- `cloud/operator.rs::build_operator`: add the three match arms (see §1). Set `.root()`
  from the folder. `presign_get` needs no change (returns None for these).
- `cloud/oauth.rs` (NEW): the §3 flow + `cloud_connect_gdrive` / `_dropbox` / `_onedrive`
  commands (open browser, run flow, write secrets+config, return `ProviderConfig`).
- `cloud/retention.rs` + `library/db.rs::evictable_clips`: apply the §0 decision (gate
  on presign support) so we never evict a clip we then can't play.
- `main.rs`: register the new oauth commands + `.plugin(tauri_plugin_oauth::init())`.
- `Cargo.toml`: §2 deps + the three opendal `services-*` features.

**Frontend**
- `lib/api.ts`: extend `ProviderKind` union (`gdrive`/`dropbox`/`onedrive` variants),
  `ProviderSecrets` (refresh_token/client_id/client_secret optional), add
  `cloudConnect*` wrappers. Mirror the Rust serde tags EXACTLY.
- `components/settings/cloud-providers.tsx`: the `AddProviderForm` currently renders
  key/secret fields per kind. For consumer clouds replace the secret fields with a
  **"Connect <provider>"** button that calls the oauth command (no manual key entry);
  on success the provider appears in the list like any other. Add the three to
  `KIND_LABELS` + `describe()`. Show a "stored in your OS keyring" note.
- Everything else (upload badge, corner toast, `use-cloud.ts`, retention gauge,
  download-to-edit) is backend-agnostic — **no changes** beyond the union types.

---

## 5. Build order

| # | Step | Risk |
|---|------|------|
| A | External: start Google app verification; create Dropbox + MS apps | external/calendar |
| B | Decide §0 (presign/retention) — gate eviction on `supports_presign()` | **design, do first** |
| C | Deps + opendal `services-*` features; `build_operator` arms; provider/secret types | low |
| D | `cloud/oauth.rs` + tauri-plugin-oauth wiring; one provider end-to-end (Drive) | **med** (OAuth loopback + PKCE) |
| E | FE: union types + "Connect Drive" button; test upload/download/play | low |
| F | Repeat D/E for Dropbox + OneDrive (mostly endpoints/scopes) | low–med |
| G | (Fast-follow) §0 option 2: `hakocloud://` streaming proxy so eviction works everywhere | med |

Suggested first slice: **Google Drive only, B→E**, proving the OAuth loop end-to-end
before adding Dropbox/OneDrive (they're copy-paste with different endpoints/scopes).

---

## 6. Gotchas / notes

- **Presign is the whole ballgame** — re-read §0. If you forget it, evicted Drive clips
  silently become unplayable black boxes.
- **Google verification is the long pole** — start it on day 1; it gates public release,
  not dev. `drive.file` scope minimizes the review surface.
- **Dropbox tokens are short-lived** — confirm the 0.54 Dropbox builder's refresh story
  (§1); if absent, refresh in `oauth.rs` and inject a fresh `access_token` per session.
- **Keyring blob is forward-compatible** — `Secrets` uses `#[serde(default)]` per field,
  so adding token fields won't break existing S3/R2/B2/GCS entries.
- **`remote_key` vs `root`** — S3-family keys are bucket-absolute; Drive/Dropbox/OneDrive
  are root-relative. Set `.root(folder)` in `build_operator` and keep keys relative, or
  the date-bucketed path will double up. Add a test like `remote_key_layout`.
- **Build env**: kill running `hako.exe` before `cargo` (DLL lock); TS typecheck is
  `& ".\node_modules\.bin\tsc.exe" --noEmit` (Bun project; `npx tsc` grabs a decoy).
- **Don't bump opendal past 0.54** (feature rename) and **don't add opendal's `rustls`
  feature** (doesn't exist; TLS via reqwest unification).

---

## 7. Verify commands

```sh
# Rust (kill running hako first if DLL-locked)
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml cloud

# Frontend typecheck (PowerShell, Bun project)
& ".\node_modules\.bin\tsc.exe" --noEmit
```

Live test needs a real Google/Dropbox/Microsoft account + the configured OAuth app —
same "needs creds" blocker as the v1 S3-family smoke test, plus a browser consent step.
