# Handoff — clip-editor audio mixer "broken" (really: slow decode) + reverted RAM flags

## TL;DR
The per-stem audio mixer (mute/solo/volume in the clip editor) appears broken
for the **first ~30s** after loading/refreshing a multi-track clip, then starts
working. **It is not broken — it's decode latency.** The mixer falls back to the
native `<video>` master track until all stems finish decoding; until then the
per-stem controls are inert by design. The real bug is that decode takes ~30s,
which is far too slow.

This is **unrelated** to the WebView2 RAM work committed this session
(`614e112`) and **unrelated** to React Compiler.

## Repro
1. Open a **multi-track** clip in the editor (one with audio stems; single-track
   clips never engage the mixer — native audio only, expected).
2. Immediately try mute/solo/volume on a stem → **no audible effect**.
3. Wait ~30s → the controls start affecting the sound.
4. Refresh / reopen → broken again for ~30s (decode restarts).

## Root cause (confirmed)
`src/hooks/use-track-mixer.ts` decodes **every stem fully into an AudioBuffer**
before it sets `active = true` and takes over playback. Its own doc comment
(lines 23-27) states the fallback: *"until it finishes … `active` is false and
the caller leaves the native `<video>` audio playing."* So during decode you hear
track 0 and the stem controls do nothing.

The slowness is in the decode→IPC read path:
- `decodeStem()` (use-track-mixer.ts:76) iterates `sink.buffers()` (mediabunny)
  over the whole track.
- mediabunny pulls bytes through `CustomSource.read` →
  `readClipRange(clipId, start, end)` (src/lib/api.ts:326) → `invoke("read_clip_range")`.
- **Prime suspect:** `read_clip_range` (src-tauri/src/commands.rs:675) does
  `std::fs::File::open(&rec.path)` + `metadata()` **on every call**. mediabunny
  issues many small range reads per track, so this is potentially hundreds/
  thousands of file-opens + a fresh `Vec` + an IPC round-trip each, for every
  stem. That, not the codec, is almost certainly the ~30s.

## Fix ideas (ranked, unverified)
1. **Stop re-opening the file per read.** Cache an open `File`/handle (or mmap)
   keyed by clip id on the Rust side, or read the stem's whole byte region once.
   Likely the biggest win.
2. **Cut IPC round-trips.** Have mediabunny request larger chunks, or prefetch
   the stem ranges in fewer/bigger reads.
3. **UX honesty while decoding.** Show a "preparing live mix…" state and
   disable/annotate the per-stem controls while `active === false`, so it reads
   as "loading," not "broken." Cheap, ships independently of the perf fix.
4. Consider incremental/streaming decode so mixing engages before the entire
   track is buffered.

## Ruled out (do not re-chase)
- **This session's RAM commit `614e112`** — 100% Rust (`main.rs`, `overlay.rs`).
  The webview runs byte-identical frontend with or without it. `git diff --stat`
  confirmed zero frontend files.
- **React Compiler** — the RC mixer fix in commit `31c5f2a` is present and
  *working*; it's literally why the mixer works after decode. The miscompile that
  froze gains was already fixed (ref-write moved into an effect).
- **`additionalBrowserArgs` flags** — see below; they were reverted but were
  **never the cause** (we were testing inside the 30s decode window each time).

## ⚠️ Opportunity we dropped on a false alarm: the RAM browser flags
While chasing this "mixer bug" I reverted the `additionalBrowserArgs` from all
three windows in `tauri.conf.json`, suspecting `--disable-features=…
AudioServiceOutOfProcess…` broke audio. **That diagnosis was wrong** — the mixer
issue is decode latency, independent of the flags. Before reverting, the flags
had **measured real wins**: `WebView2: Hako` (main) **268.9MB → 147.6MB**, group
**569.8MB → 443.4MB**, and all 3 windows still rendered fine.

**Action:** re-introduce the flags and re-measure, testing the mixer *after* the
30s decode window to confirm they're innocent. The arg string used (identical on
all 3 windows — required for a shared user-data folder), minus the one genuinely
risky token:
```
--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,MediaSessionService,HardwareMediaKeyHandling --disable-speech-api --edge-webview-enable-builtin-background-extensions=0 --autoplay-policy=no-user-gesture-required
```
(`msWebOOUI,msPdfOOUI,msSmartScreenProtection` + autoplay = wry's defaults you
must preserve when overriding. `--js-flags=--expose-gc` optional, for heap
profiling. `AudioServiceOutOfProcess` left out as the one to re-add last/with a
deliberate mixer check, since it touches audio process topology.)

## Shipped this session (committed `614e112`)
- Overlay WebView2 renderer suspends when hidden / resumes on show + startup
  suspend (~3s). Verified: overlay at **0% CPU** when idle.
- `set_webview_suspended` refactored to reuse `suspend_window_webview()`.
- Reload-on-idle-hide for the main window (MS "periodic refresh"; skipped while
  capturing). Tradeoff: manual idle hide→reopen loses route state — drop this
  part if it's annoying.

## Notes
- `read_clip_range` returns `tauri::ipc::Response::new(buf)` (efficient raw
  binary IPC — that part is fine; the cost is the per-call file open + round-trip
  count, not serialization).
- Build/test env: PowerShell + Bun; `cargo check --bins` from `src-tauri`
  (`FFMPEG_DIR` already set via `.cargo/config.toml`). cargo check passed clean
  for the committed changes.
- The older `handoff.md` in this repo is a **separate, mostly-shipped** doc (the
  hidden-to-tray CPU/EcoQoS work) — don't confuse it with this one.
