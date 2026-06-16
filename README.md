# Hako

Performance-first, Valorant-only game clip recorder (a lighter Medal.tv alternative).

**Stack:** Tauri v2 + Rust · React 19 + TanStack Router/Query · TailwindCSS v4 + shadcn/ui.

## Prerequisites

- [Bun](https://bun.sh)
- Rust (stable, `x86_64-pc-windows-msvc`) + MSVC C++ build tools
- WebView2 runtime (ships with Windows 11)

## Develop

```sh
bun install
pwsh -File scripts/fetch-ffmpeg.ps1   # one-time: fetch the bundled FFmpeg build
bun run tauri dev                     # runs Vite + the Rust core, opens the window
```

The FFmpeg 8.1 shared build (NVENC/AMF/QSV) lives in `src-tauri/ffmpeg/` and is
linked by `rusty_ffmpeg` (paths set in `.cargo/config.toml`). It's gitignored
except `binding.rs` (the version-pinned prebuilt FFI binding); `build.rs` copies
the DLLs next to the built exe.

## Build

```sh
bun run tauri build   # release exe + installer
```

Useful while iterating without the bundler:

```sh
bun run build                                       # type-check + Vite build → dist/
cargo build --manifest-path src-tauri/Cargo.toml    # compile the Rust core
```

## Layout

- `src/` — React UI (routes, components/ui shadcn, hooks, lib/api invoke wrappers)
- `src-tauri/src/` — Rust core (`core/` capture·encode·buffer, `valorant/` Riot
  integration, `library/` clips)
- `src-tauri/app-icon.png` — source image for `bun tauri icon`

## Behavior notes

- **Closing the window hides to tray; the recorder keeps running.** Use the tray
  menu's **Quit** to fully stop it.
- The dashboard shows a live heartbeat pushed from Rust every ~2s — that's the
  invoke/event round-trip wired end to end.

> Uses [Bun](https://bun.sh) as the package manager / script runner.
