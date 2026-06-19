// React render profiling via react-scan — DEV ONLY.
//
// Two audiences:
//   1. You (human): the live overlay highlights what's re-rendering and why
//      ("unnecessary" renders show gray). Toggle/inspect via the toolbar.
//   2. An agent: it can't see the WebView2 overlay or its console, so we ship
//      react-scan's cumulative `getReport()` (per-component render counts +
//      timings) to a Rust command that writes `react-scan-report.json` next to
//      the dev working dir. The agent just `Read`s that file.
//
// The whole module is gated on `import.meta.env.DEV`, so `scan()` and this code
// are tree-shaken out of the production bundle — zero shipped overhead.

import { scan } from "react-scan";
import { invoke } from "@tauri-apps/api/core";

interface RenderStat {
  /** Component display name (falls back to "Unknown"). */
  name: string;
  /** Total renders accumulated this session. */
  count: number;
  /** Total render time (ms) accumulated this session. */
  timeMs: number;
}

// react-scan 0.5.7's `getReport()` only accumulates while a component is
// click-focused in the inspector, so it's empty for a global view. Instead we
// aggregate every commit ourselves from the `onRender` callback, keyed by
// component name.
const renderStats = new Map<string, { count: number; timeMs: number }>();

/** Flatten the accumulated render stats into a count-sorted array. */
function collectStats(): RenderStat[] {
  const stats: RenderStat[] = [];
  for (const [name, data] of renderStats) {
    stats.push({
      name,
      count: data.count,
      timeMs: Math.round(data.timeMs * 100) / 100,
    });
  }
  return stats.sort((a, b) => b.count - a.count);
}

/**
 * Serialize the current render report and write it to disk via Rust.
 * Returns the absolute path written, or null if there was nothing to report.
 * Exposed on `window.__dumpRenderStats()` for manual use from devtools.
 */
async function dumpRenderStats(): Promise<string | null> {
  const stats = collectStats();
  if (stats.length === 0) return null;
  const payload = {
    capturedAt: new Date().toISOString(),
    totalRenders: stats.reduce((sum, s) => sum + s.count, 0),
    components: stats,
  };
  try {
    const path = await invoke<string>("dump_render_stats", {
      json: JSON.stringify(payload, null, 2),
    });
    return path;
  } catch (err) {
    console.error("[react-scan] dump failed:", err);
    return null;
  }
}

export function initReactScan(): void {
  if (!import.meta.env.DEV) return;

  scan({
    enabled: true,
    // Gray-outline renders that changed nothing in the DOM — the prime suspects
    // for "re-rendering a lot". Adds some overhead, fine for a dev profiling run.
    trackUnnecessaryRenders: true,
    // Keep the console quiet by default; the overlay + JSON dump are the signal.
    log: false,
    // Aggregate every commit into our own session-wide tally (see renderStats).
    onRender(_fiber, renders) {
      for (const r of renders) {
        const name = r.componentName ?? "Unknown";
        const prev = renderStats.get(name) ?? { count: 0, timeMs: 0 };
        prev.count += r.count ?? 1;
        prev.timeMs += r.time ?? 0;
        renderStats.set(name, prev);
      }
    },
  });

  // Manual trigger from devtools: `await window.__dumpRenderStats()`.
  (window as unknown as { __dumpRenderStats: typeof dumpRenderStats }).__dumpRenderStats =
    dumpRenderStats;

  // Hotkey for humans: Ctrl+Alt+R writes the report on demand.
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.altKey && e.code === "KeyR") {
      e.preventDefault();
      void dumpRenderStats().then((path) => {
        if (path) console.info(`[react-scan] report → ${path}`);
      });
    }
  });

  // Hands-off path for an agent: auto-write whenever new renders have landed.
  // getReport() is cumulative, so we only invoke Rust when the total grew —
  // an idle UI writes nothing.
  let lastTotal = 0;
  setInterval(() => {
    const total = collectStats().reduce((sum, s) => sum + s.count, 0);
    if (total > lastTotal) {
      lastTotal = total;
      void dumpRenderStats();
    }
  }, 4000);
}
