# Hako Performance-Optimization Handoff

**Goal:** minimal CPU/GPU/RAM while the game (Valorant) runs, esp. while hidden to
tray. Capture/encode hot path is already excellent — wins are in *what runs while
hidden*. **Verify every candidate against code + primary sources before acting;**
several "obvious" wins were debunked (see *Dead ends*). Measure, don't reason
(PresentMon for game frame-times, Process Explorer for Hako CPU/GPU).

## Status

All changes **in working tree, UNCOMMITTED** (`main`, ahead of origin). Builds clean
(`cargo build … --bins` → 0, no warnings; test target compiles).

## Implemented

1. **WebView2 `TrySuspend` while hidden** (`main.rs` `set_webview_suspended`) — pauses
   renderer timers/scripts to ~0 CPU + releases RAM. `hide()` before suspend.
2. **EcoQoS while hidden to tray** (`core/mod.rs` `set_process_eco_qos` +
   `protect_thread_high_qos`; toggled in `set_webview_suspended` — the one chokepoint
   all 4 hide/show paths route through). Process → EcoQoS on hide (UI/WebView2/tokio
   park on efficiency cores); RT threads **encode / hook-source / audio** tagged
   HighQoS so the encode path is never throttled. Windows only auto-throttles a
   hidden window *on battery*, so this is the only lever on AC. Best-effort + logged.
3. **`capture-stats` not emitted to a hidden window** (`capture.rs` `emit_loop`) —
   skips per-tick IPC while hidden, backs poll 300 ms → 1 s, keeps rate baselines.
4. **Audio `POLL_MS` 5 → 10** (`audio.rs`) — engine period is ~10 ms; 5 ms spun on
   empty. Halves idle wakeups, ≥20 ms buffers give headroom. Fixed 3 stale
   "loopback can't be event-driven" comments (false since Win10 1703).
5. **Cheapest `sysinfo` refreshes** — `valorant/service.rs`, `hook/host.rs`,
   `audio.rs` ×2: `System::new()` + `refresh_processes_specifics(All,true,nothing())`
   instead of `new_all()` (names come from base enumeration).
6. **tokio capped to 2 workers** (`main.rs`, via `async_runtime::set` first in `main`).

## Dead ends — do NOT chase (verified against primary sources)

- **Watchdog `Map` readback** (`capture.rs::probe_center_hash`): blocks *Hako's own
  capture thread*, NOT the game — separate D3D devices/contexts; a `Map` is a CPU-side
  wait, no barrier onto the game's context. 1 Hz 16 KB copy is negligible GPU work.
  Not a game-FPS issue.
- **Timer-resolution footgun:** tao/winit/wry call `timeBeginPeriod` **zero** times
  (grepped). tao spinlocks the last 1 ms instead. Fiction.
- **NVENC `splitEncodeMode`:** only auto-triggers ~4K+ on multi-NVENC GPUs. Never at
  1080p/1440p H.264. Irrelevant.
- **Thread-priority flip (encode `ABOVE_NORMAL`→`NORMAL`):** ambiguous — current boost
  has a defensible anti-starvation rationale; flipping blind risks the game starving
  the encoder. Only with measurement.
- **`additionalBrowserArgs`, `--disable-gpu`, `--disable-*-backgrounding`:** all
  counterproductive or unsupported (see prior research).

## Open avenues (ranked; measure first)

1. **Measure** game FPS + Hako CPU/GPU before/after while recording a match. Gates
   everything below, incl. EcoQoS's real (hardware-dependent) payoff.
2. **Event-driven WASAPI** (`audio.rs`): supported on Win10 1703+; OBS uses
   `EVENTCALLBACK|LOOPBACK` + `SetEventHandle` + a finite ~10 ms loopback wait
   (`win-wasapi.cpp:739,841,1131`). Cuts wakeups further. Structural, sync-critical —
   not a quick change.
3. **Audio alloc churn:** `TrackMixer::drain_ready` / `AacEncoder::drain` allocate a
   `Vec` per block/packet (~47/s/track). Reusable scratch buffers. Safe hygiene.
4. **MMCSS "Audio" on the audio thread** instead of static boost — OBS does this
   (audio-only; never on encode/capture). Self-yielding, gentler than `ABOVE_NORMAL`.
5. **Idle wakeups out of match:** orchestrator polls presence every 2 s always
   (`valorant/orchestrator.rs`); widen in `MENUS`. `match-state-changed` could be
   edge-triggered.

## Already optimal — don't redo

Encoder (`encode.rs`): NVENC p1/rc-lookahead=0/multipass=off, QSV low_power/veryfast/
async_depth=1, AMF; never software x264. Capture (`capture.rs`): zero-copy GPU→GPU,
bounded staging pool + frame-drop backpressure, FPS pacing. Release profile
(`Cargo.toml`): lto, codegen-units=1, panic=abort, strip, mimalloc v2 (v3 pinned off).
Tauri (`tauri.conf.json`): withGlobalTauri:false, csp:null, no devtools in release.

## Build / test

PowerShell + Bun (never npm/pnpm). Rust:
```
$env:FFMPEG_DIR = "$PWD\src-tauri\ffmpeg"
cargo build --manifest-path src-tauri/Cargo.toml --bins
cargo test  --manifest-path src-tauri/Cargo.toml --bins   # expect 95 pass / 13 fail
```
**13 test failures are pre-existing + environmental** (FFmpeg DLLs not co-located /
no QSV hw) — identical on clean `main`. Pipes mask exit codes; read `test result:`.

## Verified facts (primary-source; don't re-derive)

- MS Loopback Recording doc: event-driven loopback supported **Win10 1703+**.
- MS QoS doc: minimized/occluded window → Low QoS, but throttle effect is **"On
  battery"** only; **Eco** = "Always". → explicit EcoQoS needed on AC.
- MS SetProcessInformation: EcoQoS = efficient cores/low clock; "should not be used
  for performance critical or foreground" → carve out RT threads. `windows` 0.61 +
  `Win32_System_Threading` already exposes all QoS symbols (no new dep).
- OBS (`74c1065`): MMCSS only on audio (`L"Audio"`); encoder thread at NORMAL.
- Map(READ) blocking wait is resource-scoped (CPU-side), not a pipeline flush.
