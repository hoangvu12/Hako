# Vendored OBS `graphics-hook` binaries

These are the GPL'd **OBS Studio `win-capture`** binaries Hako reuses for the
game-capture (injection) path — the same approach Medal ships (Medal's
`medal-hook64.dll` is this DLL recompiled). The Rust host orchestration in
`src-tauri/src/core/hook/` drives them; we do not rebuild the in-process hook.

## Provenance

- **Source:** OBS Studio **32.1.0** install, copied from
  `C:\Program Files\obs-studio\data\obs-plugins\win-capture\`.
- **License:** GPLv2 (`gplv2.txt`, copied from the OBS install). Hako is
  GPL-licensed, so redistribution alongside these binaries is compliant — this is
  exactly what Medal does.

## Files

| File | Role |
|---|---|
| `graphics-hook64.dll` / `graphics-hook32.dll` | The in-process hook injected into the game; Detours `IDXGISwapChain::Present`, copies the backbuffer to a shared texture. |
| `inject-helper64.exe` / `inject-helper32.exe` | Injector. Hako always uses the **safe** path: `inject-helper64.exe "<dll>" 1 <thread_id>` → `SetWindowsHookEx` (no `OpenProcess`/`WriteProcessMemory`/`CreateRemoteThread` against the game), which is what anti-cheats tolerate. |
| `get-graphics-offsets64.exe` / `get-graphics-offsets32.exe` | Computes module-relative vtable function offsets the hook Detours; run at session start and parsed into `HookInfo.offsets`. |

## Updating

If you bump the vendored OBS version, re-copy all of the above **and** re-verify
`contract::HOOK_VER_MAJOR`/`HOOK_VER_MINOR` against the new DLL — a hook ABI
version mismatch makes capture silently never go ready.

## Bundling

At runtime the host resolves this directory via, in order:
1. `HAKO_OBS_HOOK_DIR` env override,
2. `<exe dir>/vendor/obs-hook` (Tauri resource bundle), then
3. this in-repo path (dev).

TODO: add `src-tauri/vendor/obs-hook/*` to Tauri `bundle.resources` so packaged
builds ship them next to the exe.
