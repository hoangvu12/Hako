// The in-game overlay toast host. Runs in its own transparent, click-through,
// always-on-top window that the Rust core positions over the game and shows
// only while capturing. Rust emits `overlay-notify`; this component queues +
// renders animated toasts and auto-dismisses them. It owns no business logic —
// Rust decides *what* to show (mirrors Medal's OverlayPayload contract).
//
// Styling is inline (like the updater splash) so the bundle stays tiny and the
// window never inherits an opaque background.
import { useEffect, useState, type CSSProperties } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Record as RecordIcon, CheckCircle, WarningCircle, type Icon } from "@phosphor-icons/react";

/** Mirrors the Rust `OverlayKind` (src-tauri/src/overlay.rs); snake_case serde. */
type OverlayKind = "recording_started" | "recording_stopped" | "clip_saved" | "disk_low";

/** Mirrors the Rust `OverlayNotice` — the `overlay-notify` event payload. */
interface OverlayNotice {
  kind: OverlayKind;
  title: string;
  subtitle: string | null;
  ttlMs: number;
}

interface Toast extends OverlayNotice {
  id: number;
}

/** Corner the stack sits in (mirrors Rust `overlay_position`). */
type OverlayPosition = "top_left" | "top_right" | "bottom_left" | "bottom_right";

/** Mirrors the Rust `OverlayConfig` — the `overlay-config` event payload. */
interface OverlayConfig {
  position: OverlayPosition;
}

/** Most toasts shown at once; older ones drop off the top of the stack. */
const MAX_TOASTS = 3;
/** Fallback auto-dismiss when a notice carries no (or a zero) ttl. */
const DEFAULT_TTL_MS = 3500;
/** Exit-animation duration; the toast is removed from state after this. */
const EXIT_MS = 200;

const ICONS: Record<OverlayKind, Icon> = {
  recording_started: RecordIcon,
  recording_stopped: RecordIcon,
  clip_saved: CheckCircle,
  disk_low: WarningCircle,
};

// One semantic accent per kind, used sparingly: the status icon and the
// time-to-dismiss bar, nothing decorative. OKLCH, kept off full-neon so it sits
// on a dark chip without screaming.
const ACCENTS: Record<OverlayKind, string> = {
  recording_started: "oklch(0.63 0.2 25)", // brand red — now recording
  recording_stopped: "oklch(0.72 0.02 60)", // warm grey — stopped
  clip_saved: "oklch(0.74 0.16 155)", // green — saved
  disk_low: "oklch(0.79 0.15 75)", // amber — low disk
};

export function OverlayApp() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  // Ids currently playing their exit animation (rendered, but on the way out).
  const [leaving, setLeaving] = useState<Set<number>>(new Set());
  // Corner placement. Rust pushes this (overlay-config) whenever the overlay is
  // shown and on settings changes; we also read it once on mount as a seed.
  const [position, setPosition] = useState<OverlayPosition>("top_right");

  useEffect(() => {
    // All clicks pass straight through to the game behind the overlay. Set on
    // mount as a belt-and-braces companion to the window's `focus: false`.
    void getCurrentWindow().setIgnoreCursorEvents(true);

    let cancelled = false;
    // Seed the corner from saved settings (Rust re-pushes it on each show).
    void invoke<{ overlay_position?: OverlayPosition }>("get_settings")
      .then((s) => {
        if (!cancelled && s.overlay_position) setPosition(s.overlay_position);
      })
      .catch(() => {});

    const unlistenConfig = listen<OverlayConfig>("overlay-config", (event) => {
      if (!cancelled) setPosition(event.payload.position);
    });

    let nextId = 0;
    const unlistenNotify = listen<OverlayNotice>("overlay-notify", (event) => {
      if (cancelled) return;
      const id = ++nextId;
      setToasts((prev) => [...prev, { ...event.payload, id }].slice(-MAX_TOASTS));

      const ttl = event.payload.ttlMs > 0 ? event.payload.ttlMs : DEFAULT_TTL_MS;
      window.setTimeout(() => {
        // Begin the exit animation, then drop the toast once it finishes.
        setLeaving((prev) => new Set(prev).add(id));
        window.setTimeout(() => {
          setToasts((prev) => prev.filter((t) => t.id !== id));
          setLeaving((prev) => {
            const next = new Set(prev);
            next.delete(id);
            return next;
          });
        }, EXIT_MS);
      }, ttl);
    });

    return () => {
      cancelled = true;
      void unlistenConfig.then((fn) => fn());
      void unlistenNotify.then((fn) => fn());
    };
  }, []);

  return (
    <div style={{ ...styles.stack, ...cornerStyle(position) }}>
      {toasts.map((t) => (
        <ToastPill key={t.id} toast={t} leaving={leaving.has(t.id)} />
      ))}
    </div>
  );
}

function ToastPill({ toast, leaving }: { toast: Toast; leaving: boolean }) {
  const Glyph = ICONS[toast.kind];
  const accent = ACCENTS[toast.kind];
  return (
    <div
      style={{
        ...styles.card,
        animation: `${leaving ? "hako-toast-out" : "hako-toast-in"} ${
          leaving ? EXIT_MS : 200
        }ms cubic-bezier(0.16, 1, 0.3, 1) both`,
      }}
    >
      {/* The mascot, used as itself: a crisp avatar, no ring, no glow. */}
      <img src="/logo.png" alt="" style={styles.logo} draggable={false} />

      <div style={styles.text}>
        <div style={styles.titleRow}>
          <Glyph size={16} weight="fill" style={{ color: accent, flexShrink: 0 }} />
          <span style={styles.title}>{toast.title}</span>
        </div>
        {toast.subtitle ? <div style={styles.subtitle}>{toast.subtitle}</div> : null}
      </div>
    </div>
  );
}

/** Anchor the stack to a corner. Always a `column`, so the newest toast sits
 *  nearest the chosen corner (top corners grow down, bottom corners grow up). */
function cornerStyle(position: OverlayPosition): CSSProperties {
  const top = position.startsWith("top");
  const left = position.endsWith("left");
  return {
    top: top ? 22 : undefined,
    bottom: top ? undefined : 22,
    left: left ? 22 : undefined,
    right: left ? undefined : 22,
    alignItems: left ? "flex-start" : "flex-end",
  };
}

const styles: Record<string, CSSProperties> = {
  // The stack; its corner anchor is applied per-render via `cornerStyle`. The
  // whole surface ignores pointer events so nothing blocks the game.
  stack: {
    position: "fixed",
    display: "flex",
    flexDirection: "column",
    gap: 10,
    pointerEvents: "none",
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif',
  },
  // The brand mascot art is the surface. A single left→right scrim in the same
  // violet hue darkens the text side and lets the artwork breathe on the right —
  // a legibility scrim, not a decorative gradient. One drop shadow for lift, a
  // hairline edge for definition.
  card: {
    position: "relative",
    display: "flex",
    alignItems: "center",
    gap: 13,
    width: 336,
    padding: "13px 16px",
    borderRadius: 12,
    overflow: "hidden",
    backgroundColor: "oklch(0.4 0.11 300)",
    backgroundImage:
      "linear-gradient(90deg, oklch(0.17 0.04 300 / 0.94) 0%, oklch(0.19 0.05 300 / 0.8) 50%, oklch(0.2 0.05 300 / 0.62) 100%), url('/overlay-bg.jpg')",
    // Scrim pinned to the full card (no bright strip at the edges); the artwork
    // covers beneath it and stays visible through the lighter right end.
    backgroundSize: "100% 100%, cover",
    backgroundPosition: "0 0, center right",
    backgroundRepeat: "no-repeat, no-repeat",
    boxShadow: "0 10px 28px -10px oklch(0 0 0 / 0.7)",
  },
  logo: {
    flexShrink: 0,
    width: 44,
    height: 44,
    borderRadius: 9,
    objectFit: "cover",
    display: "block",
    // Seat the asset without a decorative ring.
    boxShadow: "inset 0 0 0 1px oklch(1 0 0 / 0.12)",
  },
  text: {
    minWidth: 0,
    display: "flex",
    flexDirection: "column",
    gap: 2,
  },
  titleRow: {
    display: "flex",
    alignItems: "center",
    gap: 7,
    minWidth: 0,
  },
  title: {
    fontSize: 14.5,
    fontWeight: 650,
    letterSpacing: "-0.005em",
    lineHeight: 1.2,
    color: "oklch(0.98 0.004 300)",
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
  subtitle: {
    fontSize: 12.5,
    fontWeight: 450,
    lineHeight: 1.25,
    color: "oklch(0.84 0.02 300)",
    whiteSpace: "nowrap",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
};

// Enter/exit keyframes (inline styles can't hold @keyframes). Enter: a quick
// ease-out slide from the right; exit: fade + small slide.
const keyframes = document.createElement("style");
keyframes.textContent = `
@keyframes hako-toast-in {
  0%   { opacity: 0; transform: translateX(24px); }
  100% { opacity: 1; transform: translateX(0); }
}
@keyframes hako-toast-out {
  0%   { opacity: 1; transform: translateX(0); }
  100% { opacity: 0; transform: translateX(12px); }
}`;
document.head.appendChild(keyframes);
