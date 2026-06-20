# Handoff — Cloud Upload: START HERE to continue

Quick resume pointer for the next session. Full design lives in
`docs/cloud-upload-plan.md`; the detailed build log is in
`handoff-cloud-upload.md`. This file = the 60-second "where we are / what's next".

> **Next chapter (Phase 2/3 — Google Drive / Dropbox / OneDrive OAuth): see
> `handoff-cloud-phase2.md`.** v1 ("bring your own bucket") is code-complete; the only
> v1 remainder is a live-bucket smoke test (needs creds). If you're here to build the
> consumer OAuth clouds, start in that file — it has the verified OpenDAL APIs, the
> deps, the OAuth flow, and the one critical design decision (consumer clouds can't
> presign, which breaks evicted-clip playback unless you handle it).

## Where we are

**Milestones 1, 3, 4, 5, 6, 7 are DONE and verified.** The whole feature is built
and compiles — providers, the upload engine, the auto-upload hook, the entire
frontend (settings + hooks + clip-card badge + corner toast), and retention
("free up space"). It has **not** been run against a live bucket yet.

Verified clean this session:
- `cargo check` — no warnings
- `cargo test cloud` — **7 passed** (incl. new `b2_carries_bucket_id`)
- `tsc --noEmit` — clean

**M8 update (this session):** B2 + GCS provider code is now finalized (B2 `bucket_id`
field + GCS base64 credential) — see "Next steps" §2. Live B2/GCS cred test still pending.

**Nothing is committed.** All work is in the working tree (see "Uncommitted" below).

## Key decisions already locked (don't re-litigate)

- **Retention = additive `evicted` flag**, not the plan's nullable-path. `clips.path`
  stays a `String`; evicted clips keep the row with `evicted=1` and NULL
  thumb/filmstrip, and play from the presigned `remote_url`.
- **Editor (`clip-viewer.tsx`) fully handles evicted clips** (changed this session).
  It plays cloud-only clips from the presigned `remote_url`, and a **"Download to edit"**
  button re-fetches the file (`cloud_download_clip`) with a progress bar, clears
  `evicted`, and re-enables trim/export. Mirrors Medal's download-to-edit flow; we skip
  Medal's re-upload + "lose the original" confirm because our download is non-destructive.
- **Retention is opt-in, Recycle-Bin by default**, auto-runs after upload only when
  `cloud_free_up_space_enabled`, plus a manual "Free up space now" button.

## Next steps (in order)

1. **Live R2 smoke test** (highest value — needs real creds the dev didn't have):
   - Settings → **Cloud Upload** → add an R2 provider (account id, bucket, R2 access
     key + secret) → **Test** (expect a green check).
   - A clip card → ⋯ → **Upload to cloud**. Watch the corner toast (progress + MB/s +
     "+N") and the card badge. Confirm the `cloud_uploads` row hits `done` with a
     `remote_url`, and the object lands at `<prefix>/<yyyy>/<mm>/hako_clip_<ms>.mp4`.
   - Toggle **auto-upload** on; save a clip; confirm it enqueues automatically.
   - Toggle **Free up space** on with a tiny budget; confirm the gauge + eviction
     (clip becomes cloud-only and still plays from the URL in the grid).
2. **M8** — code is now **finalized** for B2 + GCS (still needs a live cred test):
   - **B2**: `ProviderKind::B2` now carries a non-secret `bucket_id` (B2's opaque
     id, required by the API to resolve the upload URL); wired into
     `build_operator` (`.bucket_id(...)`), mirrored in `api.ts`, and the add-provider
     form has a **Bucket ID** field (required for B2). Verified against OpenDAL 0.54
     B2 docs.
   - **GCS**: `build_operator` now **base64-encodes** the pasted service-account JSON
     before `.credential(...)` (OpenDAL 0.54 expects base64). Keyring still stores
     readable raw JSON. `base64 = "0.22"` was already a dep.
   - Still TODO: a live smoke test against a real B2 bucket + a real GCS bucket
     (needs creds). R2/S3 remain the only end-to-end-verified path.
   - Touch points: `cloud/operator.rs` (`build_operator`), `cloud/providers.rs`
     (`ProviderKind::B2`), `settings/cloud-providers.tsx`, `lib/api.ts`.
3. **Editor for evicted clips** — DONE this session (playback **and** download-to-edit).
   - Playback: `clip-viewer.tsx` plays evicted clips from the presigned `remote_url`
     (range-capable, mirroring `clip-card.tsx::ClipPreview`).
   - **Download to edit** (matches Medal's cloud-clip edit flow, minus the re-upload):
     a `cloud_download_clip(clip_id)` command (`src-tauri/src/cloud/download.rs`)
     re-fetches the object (ranged 8 MiB GETs via `op.read_with().range()`, throttled
     progress over `cloud-download-{progress,status}` events) back to the clip's
     original path, regenerates thumb+filmstrip, and clears `evicted` via
     `db::mark_rehydrated`. The editor shows a **"Download to edit"** button →
     progress bar; on completion the clip is local and trim/export re-enable.
   - Non-destructive (download only restores local bytes), so no "you'll lose the
     original" confirm — that's why we skip Medal's CONFIRM step.
   - New FE: `useDownloadClip`/`useClipDownload` + download store in `use-cloud.ts`;
     `cloudDownloadClip` + events/types in `api.ts`. `cargo`/`tsc` clean, 7 cloud tests.

## Gotchas (bit us this session)

- **Kill running `hako.exe` before any `cargo` build** — it locks `avcodec-*.dll`.
- **No `"` inside SQL string literals in Rust** — a `--` comment containing `"free up
  space"` closed the Rust string and broke the build. Keep SQL comments quote-free.
- **TS typecheck:** `& ".\node_modules\.bin\tsc.exe" --noEmit` (Bun project; `npx tsc`
  grabs a decoy). `settings.tsx` already has a local `fmtBytes` — don't import another.
- **OpenDAL pinned 0.54**, no `rustls` feature (TLS via reqwest unification). See the
  note in `Cargo.toml`.

## Uncommitted files (this feature)

Modified: `Cargo.toml`, `Cargo.lock`, `commands.rs`, `events.rs`, `library/db.rs`,
`main.rs`, `settings.rs`, `app-layout.tsx`, `clips/clip-card.tsx`,
`clips/clip-viewer.tsx`, `cloud/operator.rs`, `cloud/providers.rs`, `lib/api.ts`,
`routes/settings.tsx`, `settings/cloud-providers.tsx`.

New: `src-tauri/src/cloud/` (`mod.rs`, `operator.rs`, `providers.rs`, `retention.rs`,
`upload.rs`, `download.rs`), `src/hooks/use-cloud.ts`, `src/components/clips/{clip-upload-badge,
upload-toast,cloud-format}.tsx?`, `src/components/settings/cloud-providers.tsx`.

## Verify commands

```sh
# Rust (kill running hako first if DLL-locked)
cargo check  --manifest-path src-tauri/Cargo.toml
cargo test   --manifest-path src-tauri/Cargo.toml         # 122 pass

# Frontend typecheck (PowerShell, Bun project)
& ".\node_modules\.bin\tsc.exe" --noEmit
```
