// React render profiling via react-scan — DEV ONLY.
//
// The live overlay highlights what's re-rendering and why ("unnecessary"
// renders show gray). Toggle/inspect via the toolbar.
//
// This module is only ever pulled in through a DEV-gated dynamic import in
// `main.tsx`, so react-scan is fully eliminated from production builds — zero
// shipped overhead. The internal `import.meta.env.DEV` guard below is just
// defense-in-depth.

import { scan } from "react-scan";

export function initReactScan(): void {
  if (!import.meta.env.DEV) return;

  scan({
    enabled: true,
    // Gray-outline renders that changed nothing in the DOM — the prime suspects
    // for "re-rendering a lot". Adds some overhead, fine for a dev profiling run.
    trackUnnecessaryRenders: true,
    // Keep the console quiet; the overlay is the signal.
    log: false,
  });
}
