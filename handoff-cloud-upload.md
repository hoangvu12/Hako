# Handoff — Cloud Upload (R2 vertical slice)

Resume point for the multi-provider cloud-upload feature. Full design lives in
`docs/cloud-upload-plan.md`. This file = what's actually built + decisions + the
gotchas you'd otherwise re-discover.

## Status: milestones 1, 3, 4, 5, 6, 7 DONE and verified

`cargo check` clean (no warnings) · 122 unit tests pass (incl. cloud + retention) ·
`tsc --noEmit` clean.

The upload path is de-risked end-to-end against **Cloudflare R2**, with the full
M5 (auto-upload hook), M6 (frontend UI: settings, hooks, clip-card badge, corner
toast), and M7 (retention / "free up space") now built. Still pending: the live
**E2E R2 smoke test** (needs real creds) and **M8** (finalize/test B2 + GCS).

### M5/M6/M7 decisions (this session)
- **Retention model = additive `evicted` flag**, NOT the plan's nullable-path. Keeps
  `clips.path` a `String` everywhere (no ripple through the Rust/TS path consumers);
  evicted clips keep their row + path-for-reference, with `evicted=1` and
  `thumb_path`/`filmstrip_path` NULLed. Playback falls back to the presigned
  `remote_url`.
- **Editor left as-is**: the cloud-only playback fallback is wired only in the
  library grid (`clip-card.tsx`). The detail/editor view (`clip-viewer.tsx`) still
  assumes a local file — opening an evicted clip there will show broken playback and
  trim/export would fail at the Rust layer. Deferred to a later milestone.
- **Retention is opt-in + Recycle-Bin by default**; auto-runs after a successful
  upload only when `cloud_free_up_space_enabled`, plus a manual "Free up space now"
  button. Deletes use the `trash` crate unless `cloud_delete_to_recycle_bin` is off.

## Decisions locked (the plan's §10 + a UX question)

| Q | Decision |
|---|----------|
| Remote path scheme | **Date-bucketed**: `<prefix>/<yyyy>/<mm>/hako_clip_<unix_ms>.mp4` |
| Presigned playback | **Yes** for S3-family in v1 (`presign_read`, 7-day TTL, stored in `remote_url`) |
| Concurrent clip uploads | **1 at a time** (each already uses 4 parallel multipart parts) |
| Auto-upload default | **Off** (opt-in) |
| Free-up-space in v1 | Deferred to M7; settings fields kept now (additive) |
| Upload UX (from studying Medal) | **Background-first**: corner toast (progress + MB/s + "+N" queue badge) + per-clip-card badge; auto-retry transient failures silently, surface `error` only after retries exhausted, manual re-upload; **no** queue-manager panel |

### How Medal does upload UX (reverse-engineered, for the M6 UI)
- Background, non-blocking corner toast `medal-upload-toast`: progress bar, `X MB/s`,
  file size, queue-count badge. Plus inline per-clip status ("Uploading…",
  "Upload complete!", "Failed to upload clip!").
- States: `content-received → uploaded-content → complete`, plus `error` /
  `UPLOAD_CANCELLED` / `UPLOAD_PAUSED`. Auto-retry via their `RetryHandler` (= our
  OpenDAL `RetryLayer`). No persistent "failed uploads" panel — user re-triggers.
- Auto-upload setting is `OFF` (default) / `ON` / `ON_GAME_EXIT`.
- Editing a clip mid-upload warns + cancels the upload.
- Artifacts to re-check if needed: `.medal-decomp/` (C#) and
  `C:\Users\HP MEDIA\AppData\Local\Medal\app-2624.234.1\resources\app\`
  (`renderer.min.js`, `main.min.js` — grep string literals, it's minified).

## CRITICAL verified API facts (OpenDAL 0.54, via Context7)

- **No `rustls` feature on opendal.** Since 0.46 OpenDAL removed all its own reqwest
  TLS flags. The plan's `features=[…,"rustls"]` would NOT compile. We omit it; TLS
  comes from the project's existing `reqwest { default-features=false,
  features=["rustls-tls"] }` via Cargo feature unification. Verified: the build pulls
  **rustls / hyper-rustls / tokio-rustls, NO OpenSSL**. (See the long note in
  `src-tauri/Cargo.toml`.)
- **Pin 0.54** — `0.56` renamed `services-*` → `service-*` (singular). Latest is 0.57.
- Builder style is consuming-chaining: `services::S3::default().endpoint(..).region(..)
  .bucket(..)…` then `Operator::new(b)?.finish()`, layers on the finished Operator.
  (`region` if/else needs `.as_str()` on both arms — `region` is `&String`.)
- Writer: `op.writer_with(key).chunk(8MiB).concurrent(4).await?` → `w.write(Vec<u8>)
  .await?` → `w.close().await?`; `w.abort().await` to cancel. Transparent multipart.
- `op.check().await` for the test command. Capability gate for presign is
  `op.info().native_capability().presign_read` (field exists, used in `presign_get`).

## Files changed / added

**Rust (`src-tauri/`)**
- `Cargo.toml` — added `opendal 0.54` (features `services-s3/b2/gcs`, NO rustls) +
  `keyring 3` (feature `windows-native`).
- `src/library/db.rs` — `cloud_uploads` table (additive); `PRAGMA foreign_keys=ON`
  added to the pragma batch (needed for the cascade); `CloudUpload` struct;
  `cloud_status` consts; CRUD: `cloud_enqueue`, `cloud_mark_uploading`,
  `cloud_set_progress`, `cloud_mark_done`, `cloud_mark_failed`, `cloud_status`;
  `row_to_cloud_upload`; test `cloud_upload_lifecycle_and_cascade`.
- `src/settings.rs` — 5 `cloud_*` fields + defaults (auto-upload off, retention 5 GiB,
  recycle-bin on). Uses container `#[serde(default)]` + `Default` impl (NOT per-field
  default fns — matches codebase convention).
- `src/events.rs` — `CLOUD_UPLOAD_PROGRESS`, `CLOUD_UPLOAD_STATUS` consts.
- `src/cloud/mod.rs` — module glue + provider commands (`cloud_list_providers`,
  `cloud_add_provider`, `cloud_remove_provider`, `cloud_test_provider`), `config_dir`,
  `new_provider_id`.
- `src/cloud/providers.rs` — `ProviderKind` (serde-tagged `kind`, snake_case
  s3/r2/b2/gcs), `ProviderConfig`, `Secrets`; `cloud_providers.json` load/save;
  keyring get/set/delete (service `hako-cloud`, key = provider id). Tests.
- `src/cloud/operator.rs` — `build_operator` (S3/R2/B2/GCS), `remote_key`
  (date-bucketed, chrono-free `year_month_utc` Hinnant algo), `presign_get`,
  `friendly_error`. Tests.
- `src/cloud/upload.rs` — `CloudState` (mpsc queue + `queued`/`cancels` sets),
  `spawn_worker` (serial drain), `run_job`/`stream_upload` (8MiB/4-concurrent,
  throttled progress w/ bytes_per_sec, cancel between chunks, presign on success),
  commands `cloud_upload_clip` / `cloud_cancel_upload` / `cloud_upload_status`.
- `src/main.rs` — `mod cloud;`, registered 7 commands in `generate_handler!`,
  `init_cloud()` (`app.manage` + `spawn_worker`) in `.setup()` after library/settings.

**Frontend (`src/lib/api.ts`)** — `Settings` fields; `Events.CloudUpload*`;
`ProviderKind`/`ProviderConfig`/`ProviderSecrets`/`CloudUploadState`/`CloudUpload`/
`CloudUploadProgress`/`CloudUploadStatus`; wrappers `cloudListProviders`,
`cloudAddProvider`, `cloudRemoveProvider`, `cloudTestProvider`, `cloudUploadClip`,
`cloudCancelUpload`, `cloudUploadStatus`.

## Gotchas / env notes

- **DLL lock on build:** a running `target\debug\hako.exe` locks `ffmpeg\bin\
  avcodec-62.dll` and the build script fails with "used by another process". Kill the
  running hako before `cargo check`/build.
- **TS typecheck:** Bun project. `npx tsc` grabs a DECOY package. Use
  `& ".\node_modules\.bin\tsc.exe" --noEmit` (PowerShell). `node` is not on the Git
  Bash PATH.
- Keyring secret round-trip is NOT unit-tested (would hit the real Credential
  Manager); only serialization is. Test it manually via `cloud_add_provider` +
  `cloud_test_provider`.
- B2/GCS code is now finalized (M8): B2 carries a `bucket_id` config field wired into
  `.bucket_id(...)`; GCS base64-encodes the service-account JSON before `.credential(...)`.
  Both verified against OpenDAL 0.54 docs but NOT yet tested against a live bucket
  (needs creds). R2/S3 remain the only end-to-end-verified path.

## What M5/M6/M7 added (this session)

**M5 — auto-upload hook (Rust)**
- `cloud/upload.rs` — extracted `enqueue()` (shared) from `cloud_upload_clip`; new
  `maybe_auto_upload(app, clip_id)` (try_state-guarded, best-effort, logs + swallows).
- `commands.rs` — call `maybe_auto_upload` after the clip row is inserted in BOTH
  `save_clip_full` (manual/F9) and `finalize_auto_clip` (Valorant auto-clips).

**M6 — frontend**
- `src/hooks/use-cloud.ts` — providers (`useCloudProviders` + add/remove/test),
  upload actions (`useUploadClip`/`useCancelUpload`), live state via
  `useCloudEventBridge` (status → query cache; progress → tiny external store so 4×/s
  ticks don't churn RQ), `useClipUpload`/`useActiveUploads`/`useClipRemoteUrl`.
- `src/components/clips/clip-upload-badge.tsx` — thumbnail status pill (queued /
  uploading % / uploaded / failed), fades out on hover.
- `src/components/clips/upload-toast.tsx` — Medal-style bottom-right corner toast
  (active clip progress + MB/s + "+N" queue badge + completion flash). Mounted in
  `app-layout.tsx`; bridge also mounted there.
- `src/components/clips/cloud-format.ts` — `fmtBytes`/`fmtRate`/`pctOf` helpers.
- `src/components/settings/cloud-providers.tsx` — provider list (Test/Remove) + add
  form (per-kind fields, secrets write-only).
- `src/routes/settings.tsx` — new **Cloud Upload** section (`CloudSection`): auto-
  upload switch, default-provider select, `<CloudProviders/>`, retention gauge +
  "Free up space now" button. New nav item under System.
- `clip-card.tsx` — renders the badge; actions menu gains Upload / Cancel / Retry /
  Upload-again / Copy-cloud-link; `ClipPreview` plays evicted clips from `remote_url`.

**M7 — retention (Rust + FE)**
- `library/db.rs` — additive `evicted` column (+ migration); `ClipRecord.evicted`;
  `local_footprint`, `evictable_clips` (oldest-first, `done`+`uploaded_at` gate),
  `mark_evicted`; `EvictRow`; test `retention_only_evicts_uploaded_clips`.
- `cloud/retention.rs` — `EvictStats`, `cloud_retention_stats` + `cloud_free_up_space`
  commands, `maybe_free_up_space` (auto, gated), lock-free file deletes via `trash`,
  emits `CLIP_CREATED` for evicted rows so the grid updates.
- `Cargo.toml` — `trash = "5"`. `main.rs` — 2 commands registered. `upload.rs` —
  retention hook replaces the old M7 TODO in the `Done` branch.
- `api.ts` — `ClipRecord.evicted`, `EvictStats`, `cloudRetentionStats`/`cloudFreeUpSpace`.

## Next steps (in order)

1. **End-to-end R2 smoke test** (still highest value, needs real creds): add an R2
   provider in Settings → Cloud Upload, hit **Test** (expect the green check), then
   use a clip card's ⋯ → **Upload to cloud**. Watch the corner toast + badge, confirm
   the `cloud_uploads` row reaches `done` with a `remote_url` and the object lands at
   `<prefix>/<yyyy>/<mm>/…`. Then flip on auto-upload + free-up-space and verify the
   gauge + eviction (clip becomes cloud-only, plays from the presigned URL in the grid).
2. **M8** — finalize B2 (`bucket_id`) + GCS (base64 credential) and test all backends.
3. **Editor for evicted clips** (deferred): `clip-viewer.tsx` still assumes a local
   file. Either wire the `remote_url` playback fallback there too, or gate edit/trim
   for `clip.evicted` with a "download to edit" affordance (re-download not built yet).

## Quick verify commands

```sh
# Rust (kill running hako first if DLL-locked)
cd src-tauri && cargo check
cd src-tauri && cargo test cloud      # 6 tests

# Frontend typecheck (PowerShell, Bun project)
& ".\node_modules\.bin\tsc.exe" --noEmit
```
