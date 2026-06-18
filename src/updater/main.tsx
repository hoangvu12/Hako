// Discord-style auto-update splash. This is its own tiny window/entry (see
// `updater.html` + the `updater` window in `tauri.conf.json`) so it stays
// decoupled from the main app bundle and paints instantly on launch.
//
// Flow: check GitHub Releases → if a signed update exists, download it (showing
// progress) and relaunch into it; otherwise (or on any error/timeout) tell the
// Rust side to reveal the already-restored main window and close this splash.
// The guiding rule is *never block launch*: every failure path falls through to
// `finish_to_main`.
import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";

/** Cap on the *check* phase only (a hung network request must not strand the
 *  user on the splash). The download phase is bounded by its own progress, not
 *  this timeout. */
const CHECK_TIMEOUT_MS = 12_000;

const BRAND = "#e3231a";
const MUTED = "#a1a1aa";

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Resolve `p`, or `fallback` if it doesn't settle within `ms`. */
function withTimeout<T>(p: Promise<T>, ms: number, fallback: T): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((resolve) => setTimeout(() => resolve(fallback), ms)),
  ]);
}

/** Reveal the main window and close this splash (Rust command). */
async function finishToMain() {
  try {
    await invoke("finish_to_main");
  } catch (err) {
    console.error("finish_to_main failed", err);
  }
}

type Phase =
  | { kind: "checking" }
  | { kind: "downloading"; percent: number; version: string }
  | { kind: "installing"; version: string }
  | { kind: "uptodate" }
  | { kind: "error" };

function statusLine(phase: Phase): string {
  switch (phase.kind) {
    case "checking":
      return "Checking for updates…";
    case "downloading":
      return `Downloading update ${phase.version}`;
    case "installing":
      return "Installing update…";
    case "uptodate":
      return "You're up to date";
    case "error":
      return "Couldn't check for updates";
  }
}

/** The whole update lifecycle, driven once on mount. */
async function runUpdate(setPhase: (p: Phase) => void): Promise<void> {
  try {
    setPhase({ kind: "checking" });
    const update = await withTimeout(check(), CHECK_TIMEOUT_MS, null);

    if (!update) {
      setPhase({ kind: "uptodate" });
      await delay(600); // let the message register before the app appears
      await finishToMain();
      return;
    }

    let total = 0;
    let downloaded = 0;
    setPhase({ kind: "downloading", percent: 0, version: update.version });

    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? 0;
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          setPhase({
            kind: "downloading",
            percent: total ? Math.min(100, (downloaded / total) * 100) : 0,
            version: update.version,
          });
          break;
        case "Finished":
          setPhase({ kind: "installing", version: update.version });
          break;
      }
    });

    // Installed — restart into the freshly installed version. (`relaunch`
    // doesn't return on success; the process is replaced.)
    await relaunch();
  } catch (err) {
    console.error("update failed", err);
    setPhase({ kind: "error" });
    await delay(1200);
    await finishToMain();
  }
}

/** A fake run for `?demo` (browser preview of the splash, no Tauri calls). */
async function runDemo(setPhase: (p: Phase) => void): Promise<void> {
  setPhase({ kind: "checking" });
  await delay(1100);
  setPhase({ kind: "downloading", percent: 0, version: "0.2.0" });
  for (let p = 0; p <= 100; p += 4) {
    setPhase({ kind: "downloading", percent: p, version: "0.2.0" });
    await delay(60);
  }
  setPhase({ kind: "installing", version: "0.2.0" });
}

function Splash() {
  const [phase, setPhase] = useState<Phase>({ kind: "checking" });

  useEffect(() => {
    const demo = import.meta.env.DEV && location.search.includes("demo");
    if (demo) {
      void runDemo(setPhase);
      return;
    }
    // In dev (run via `tauri dev`) there's no installed binary to replace, so
    // skip the network round-trip and reveal the app immediately. Remove this
    // guard (or use `?demo`) to exercise the splash.
    if (import.meta.env.DEV) {
      void finishToMain();
      return;
    }
    void runUpdate(setPhase);
  }, []);

  const determinate = phase.kind === "downloading";
  const percent = phase.kind === "downloading" ? phase.percent : 0;

  return (
    <div style={styles.shell}>
      <img src="/logo.png" alt="Hako" width={72} height={72} style={styles.logo} />
      <div style={styles.wordmark}>Hako</div>

      <div style={styles.status}>{statusLine(phase)}</div>

      <div style={styles.track}>
        {determinate ? (
          <div style={{ ...styles.fill, width: `${percent}%` }} />
        ) : (
          <div style={styles.indeterminate} />
        )}
      </div>

      <div style={styles.meta}>
        {phase.kind === "downloading" ? `${Math.round(percent)}%` : " "}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  shell: {
    width: "100%",
    height: "100%",
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    gap: 0,
    padding: 28,
    boxSizing: "border-box",
    fontFamily:
      'system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
    color: "#ededf0",
    textAlign: "center",
  },
  logo: {
    borderRadius: 16,
    marginBottom: 18,
    boxShadow: `0 8px 30px -8px ${BRAND}66`,
  },
  wordmark: {
    fontSize: 22,
    fontWeight: 700,
    letterSpacing: "-0.01em",
    marginBottom: 26,
  },
  status: {
    fontSize: 13.5,
    color: MUTED,
    marginBottom: 14,
    minHeight: 18,
  },
  track: {
    position: "relative",
    width: 220,
    height: 4,
    borderRadius: 999,
    background: "#27272a",
    overflow: "hidden",
  },
  fill: {
    height: "100%",
    borderRadius: 999,
    background: BRAND,
    transition: "width 120ms linear",
  },
  indeterminate: {
    position: "absolute",
    top: 0,
    left: 0,
    height: "100%",
    width: "40%",
    borderRadius: 999,
    background: BRAND,
    animation: "hako-indeterminate 1.1s ease-in-out infinite",
  },
  meta: {
    marginTop: 12,
    fontSize: 12,
    color: MUTED,
    fontVariantNumeric: "tabular-nums",
  },
};

// Keyframes for the indeterminate bar (inline styles can't hold @keyframes).
const keyframes = document.createElement("style");
keyframes.textContent = `
@keyframes hako-indeterminate {
  0%   { left: -40%; }
  100% { left: 100%; }
}`;
document.head.appendChild(keyframes);

ReactDOM.createRoot(document.getElementById("updater-root") as HTMLElement).render(
  <React.StrictMode>
    <Splash />
  </React.StrictMode>
);
