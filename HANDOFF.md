# Refactor session handoff

Context: after reviewing recent commits, we decided **not** to adopt a state-management
library (TanStack Query + local state is the right fit for this Tauri app). Instead we
generalized the per-game code over the existing `GameId` registry and knocked out a
batch of "collapse the scattered duplicates" cleanups surfaced by two survey agents.

Everything below is **committed on `main`** and verified (tsc + `vite build` for
frontend; `cargo check` + 202 tests + clippy for Rust). Working tree is clean.

## Follow-up session (branch `refactor/integration-engine`)

Picked up #7 and #8. Net outcome: **#7's provably-identical half landed**; the
**#7 run-loop engine and all of #8 stay deferred** — see the revised, sharper
rationale under each below. Not yet merged to `main`.

| Commit | What |
|--------|------|
| `066792d` | Extract the shared `recording::finish_and_cut` match-finalizer tail (the reconcile-and-cut half of `end_match`) — collapses the verbatim tail out of all six games. Verified: `cargo check` + 202 tests + clippy clean on touched files. |

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

### #7 — `end_match` tail: **DONE** (`066792d`). Run-loop engine: still deferred.
- **Done:** the reconcile-and-cut tail every `end_match` inlined verbatim is now
  `recording::finish_and_cut(app, rec, MatchCut, pad_for)`. Per-game knobs
  (`game_label`, `MAX_AUTOCLIP_SECS`, `PLACEMENT_TOL_SECS`, merge-after, title
  suffix, clip context) ride in `MatchCut`; `pad_for` is a closure yielding each
  kind's `(before, after)` from the game's `EventTimings`. PUBG maps its replay
  Unix-ms → capture-clock ticks and applies its toggle filter when *building*
  `MatchCut.events` (the live-feed games already filter at receipt), so the shared
  tail is uniform. This was the safe, `cargo`-verifiable half.
- **Scope left (the risky half):** the ~250-line `run()` loop scaffold is still
  ~90% identical across the six games. The genuinely game-specific part is only the
  "drain the event source" block in the middle. Target: a generic
  `run_live_feed(ctx, driver)` engine owning the loop, with each game implementing a
  small driver trait (`poll_events`, `clip_context`, `game_label`).
- **Why still deferred:** this rewrites the capture-orchestration hot path. `cargo
  test` covers event-diffing/parsing/timeline reconciliation, **not** the async
  run-loop timing/state transitions. A subtle regression = clips silently not
  recording (or wrong windows) during live matches. Needs real-game verification.
  `finish_and_cut` (this session) + `manage_full_session`/`finish_full_session`
  (`4df3701`) mean the remaining engine is now purely the loop scaffold + the
  per-source drain — smaller, but still the un-testable part.

### #8 — `EventToggles` / `EventTimings` as data — deferred, and now **recommend against**
- **Scope:** each game hand-writes a bool-per-variant `*EventToggles` + `*EventTimings`
  struct + `Default` + `enabled`/`for_kind`/`max_after`/`ALL_KINDS`. Adding one
  `EventKind` variant to a game = 6–7 parallel edits in one file. Target was a generic
  serde-friendly `EventConfig` (map keyed by `EventKind`) + a `&'static [EventKind]`
  "kinds this game supports" slice per game.
- **Revisited this session — the design is worse than it first looked:**
  - The on-disk field names are **not** a function of `EventKind`. LoL persists
    `dragon`/`baron`/`herald`/`turret`/`inhibitor` for `DragonKill`/`BaronKill`/
    `HeraldKill`/`TurretKilled`/`InhibKilled`. So a map "keyed by `EventKind`" can't
    derive its keys — each game needs a **bespoke `EventKind ↔ "field_name"` table**
    just to round-trip existing configs. One wrong row silently resets that toggle
    for every existing user on their next launch.
  - `EventKind`'s own serde is PascalCase (`"DoubleKill"`) and is **also** the clip-DB
    persisted form (`event.rs` header), so it can't be re-tagged to match the
    snake_case settings fields without a second, larger migration.
  - Even done, adding a variant becomes ~3 edits (name-table row + default row +
    supported-kinds entry), not 0 — modest payoff on a *rare* operation.
  - Cannot be verified here (no real `settings.json`). Golden round-trip tests only
    prove the cases we model; a real saved config that differs still risks a silent
    reset. Asymmetric risk: no user-visible upside, silent-data-loss downside.
- **Recommendation:** leave the per-game structs. They're verbose but they're the most
  boring, obvious, zero-migration-risk code in the repo — the verbosity is a feature
  for a rarely-touched, back-compat-critical surface. If ever revisited: custom serde
  with per-game name tables + exhaustive legacy-JSON round-trip tests (full / partial /
  unknown-field cases), on a branch, with a manual "open settings, confirm nothing
  reset" pass on a real upgraded profile before merge.

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
