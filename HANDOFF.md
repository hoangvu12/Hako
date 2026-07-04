# Refactor session handoff

Context: after reviewing recent commits, we decided **not** to adopt a state-management
library (TanStack Query + local state is the right fit for this Tauri app). Instead we
generalized the per-game code over the existing `GameId` registry and knocked out a
batch of "collapse the scattered duplicates" cleanups surfaced by two survey agents.

Everything below is **committed on `main`** and verified (tsc + `vite build` for
frontend; `cargo check` + 202 tests + clippy for Rust). Working tree is clean.

## Follow-up session (branch `refactor/integration-engine`)

Picked up #7 and #8. Net outcome: **#7 fully implemented** (engine + finalizer) and
**#8 fully implemented** (the `event_config!` macro). **Both are now merged to `main`
and shipped as `v1.8.0`** (see below) — the refactor plan is complete.

> **⚠ Open verification for the next session:** `v1.8.0` shipped the #7 run-loop
> engine (`4d41200`) *without* real-game testing — a deliberate call (solo app, no
> other users, owner will test on live games). `cargo test` can't cover the async
> run-loop timing, so the auto-clip capture path per game is **unverified in
> practice**. If the owner reports a game where auto-capture doesn't start or a clip
> doesn't land, that's the first place to look. #8 is fully verified and carries no
> such risk (on-disk format byte-identical, golden tests).

| Commit | What |
|--------|------|
| `066792d` | Extract the shared `recording::finish_and_cut` match-finalizer tail (the reconcile-and-cut half of `end_match`) — collapses the verbatim tail out of all six games. |
| `4d41200` | Generic `games::engine::run_live_feed(ctx, driver)` engine + per-game `LiveDriver` — collapses the ~130-line run loop each game duplicated. **The whole of #7 is now implemented.** |
| `e5bb1da` | `games::event_config::event_config!` macro replaces the six hand-written `*EventToggles`/`*EventTimings` triplets — the whole of #8. On-disk format unchanged; golden tests added. |

All verified here: `cargo check` + `games::` tests 66/66 (incl. 4 new golden) +
clippy (26 bin warnings, was 27 baseline, none in touched files). The full-suite
FFmpeg/QSV hardware tests flake under parallel contention (pass in isolation) — pre-
existing env noise, unrelated. **`4d41200` still needs real-game verification before
merge** — it rewrites the async run-loop timing that `cargo test` doesn't cover.

## Done (committed this session, oldest → newest)

| Commit | What |
|--------|------|
| `3e05569` | Collapse ~40 per-game Auto-Capture handler props → 5 generic `SmartGameKey`-keyed handlers (frontend `settings.tsx` / `auto-section.tsx`). |
| `26604e7` | `clipGame` if-chain → registry `BY_ID` lookup (`games/registry.tsx`). |
| `3ba71ba` | Centralize all TanStack Query keys in `src/lib/query-keys.ts`; deleted the duplicate `["clips"]` defs; removed `hooks/cloud/keys.ts`. |
| `58d7fe3` | gitignore `.mimir/` (was accidentally swept into a commit, then removed via amend). |
| `2a12102` | Dedupe byte/time formatters → `src/lib/format.ts` (`formatBytes`, `formatTime`); renamed the settings coarse formatter to `fmtBytesCoarse`. |
| `4112ce8` | Split `ViewerStage` (928→747 lines): extracted `useTrimEditor`, `useStemMix`, `useClipKeyboard` hooks under `components/clips/clip-viewer/`. Pure relocation. |
| `ea85e4c` | `GameAssetsProvider` context replaces prop-drilling of the assets bundle through the clips grid. `use-game-assets.ts` → `.tsx`. |
| `69fbe36` | Backend mirror of `3e05569`: replaced six `*_auto_mode()` settings methods + per-integration `current_auto_mode`/`current_capture_disabled` wrappers with `Settings::game_auto_mode(GameId)` / `game_disabled(GameId)` and shared `recording::game_auto_mode` / `game_capture_disabled` (called with `ctx.id()`). |
| `4df3701` | Extracted shared `recording::manage_full_session` + `finish_full_session` (were duplicated verbatim across all six smart integrations; session name now derived from `ctx.id()`). |

## Remaining — NOT done, and why

These two are genuinely worth doing but were **deliberately deferred** because they
can't be verified in this environment (no real game + GPU capture, no real user
`settings.json`), and a blind commit to `main` risks a silent, hard-to-detect
regression. They should be done on a branch by someone who can test on real games.

### #7 — **fully implemented** (`066792d` + `4d41200`). Pending real-game verify.
- **`end_match` tail (`066792d`):** the reconcile-and-cut tail every `end_match`
  inlined verbatim is now `recording::finish_and_cut(app, rec, MatchCut, pad_for)`.
  Per-game knobs (`game_label`, `MAX_AUTOCLIP_SECS`, `PLACEMENT_TOL_SECS`,
  merge-after, title suffix, clip context) ride in `MatchCut`; `pad_for` yields each
  kind's `(before, after)`. PUBG maps its replay Unix-ms → capture-clock ticks and
  applies its toggle filter when building `MatchCut.events`.
- **Run-loop engine (`4d41200`):** `games::engine::run_live_feed(ctx, driver)` owns
  the shared scaffold (sleep, auto-capture, mode, full-session roll, mode-flip /
  config-restart teardown, recorder-status emit, the want/grace latch `Wanting`, and
  session open). Each game implements `LiveDriver`: `id` / `refresh_settings` /
  `begin(rec) -> Active` / `discard` / `finish` / `async drive(...)`. `drive` is the
  per-game middle moved over verbatim (GSI drain, live-feed poll, log tail, HUD
  poll, demo-watch), each `take()`-ing its source handle so it can `&mut self` while
  finalizing mid-loop. `finish` delegates to `finish_and_cut`.
- **Deliberate, harmless differences to confirm on real games** (see `4d41200`
  message): LoL cadence is window-based not feed-based (1s vs 5s only during the
  loading/post-game window; already 1s once live); LoL live-match context mirror can
  lag ≤1s on the first tick a session opens; some log strings use the game id /
  display name. No functional effect intended.
- **Why it still needs real-game verification before merge:** it rewrites the
  capture-orchestration hot path. `cargo test` covers event-diffing/parsing/timeline
  reconciliation, **not** the async run-loop timing/state transitions. A subtle
  regression = clips silently not recording (or wrong windows) during live matches.
  **Verify per game:** play a match, confirm auto-capture starts on window detect,
  a clip lands in the library with the right window/tags, and Session/FullMatch
  modes + the mid-match config-restart path still behave.

### #8 — `EventToggles` / `EventTimings` as data — **done via a declarative macro**
- **Scope:** each game hand-wrote a bool-per-variant `*EventToggles` + `*EventTimings`
  struct + `Default` + `enabled`/`for_kind`/`max_after`/`ALL_KINDS`. Adding one
  `EventKind` variant to a game was 6–7 parallel edits in one file.
- **Why the *map-keyed-by-`EventKind`* idea was dropped (the original plan):** the
  on-disk field names are **not** a function of `EventKind`. LoL persists
  `dragon`/`baron`/`herald`/`turret`/`inhibitor` for `DragonKill`/`BaronKill`/
  `HeraldKill`/`TurretKilled`/`InhibKilled`; `EventKind`'s own serde is PascalCase
  (`"DoubleKill"`) and is *also* the clip-DB persisted form. A map keyed by
  `EventKind` therefore can't derive its keys and would silently reset every
  existing user's toggles unless it re-implemented a bespoke name table anyway.
- **What we did instead (`games/event_config.rs`):** a `event_config!` declarative
  macro that takes a per-game table of
  `field => EventKind::Variant, on: <bool>, window: (before, after)` rows and expands
  to the *exact same* `$Toggles` / `$Timing` / `$Timings` triplet + `Default` /
  `enabled` / `for_kind` / `max_after`. Adding an event is now **one row**.
  - **Zero migration risk:** the field identifiers are written verbatim at each call
    site — they stay the on-disk `settings.json` keys — so the serialized form is
    byte-identical. The (non-uniform) field↔kind mapping sits right beside each field
    rather than being derived, so it's explicit, not fragile.
  - **All six converted:** cs2, dota2, lol, warthunder, pubg, rematch. Valorant
    (`reconcile.rs`, unprefixed `EventToggles`/`EventTimings`) is a different shape and
    was left alone.
  - **Golden tests (lol/events.rs):** pin the non-uniform field↔kind mapping + the
    defaults, and assert legacy-JSON round-trips and `#[serde(default)]` additivity —
    so a future field rename fails the build instead of silently resetting users.
- **Verified:** `cargo check` clean; `games::` tests 66/66 (incl. 4 new golden);
  clippy 26 warnings (was 27 baseline), none in touched files. Net ~550 fewer lines
  across the six `events.rs` for a ~135-line macro. On-disk format unchanged, so no
  frontend change and nothing to re-verify on a real profile.

## Verify commands
- Frontend: `npm run build` (runs `tsc --noEmit && vite build`).
- Rust: `cd src-tauri && cargo check && cargo test && cargo clippy`.
- Note: repo is **not** `rustfmt`-clean at baseline — do **not** run `cargo fmt` (it
  produces a huge unrelated diff). Match surrounding style by hand.
- Note: many files show harmless LF→CRLF git warnings on Windows; ignore.

## Survey notes (from the two recon agents, for reference)
- Frontend was otherwise well-factored; the registry/presenter system is the intended
  pattern. Non-issues checked: `clip-presenter.ts` (intentional per-game polymorphism —
  leave), the 7 `*_EVENT_LABELS` tables (data, not logic), the parallel valorant/lol
  asset hooks (merging adds complexity).
- Rust: the per-game `events.rs`/`parse.rs`/`api.rs`/`classify` (event *derivation*) is
  genuinely game-specific — leave it. The duplication is all in `integration.rs`
  orchestration, which #7 addresses.
