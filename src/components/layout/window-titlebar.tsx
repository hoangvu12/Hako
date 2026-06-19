import { useEffect, useState } from "react";
import { useRouter, useRouterState } from "@tanstack/react-router";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowLeft, ArrowRight, Minus, Square, X } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Separator } from "@/components/ui/separator";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import { RecorderStatusPopover } from "@/components/layout/recorder-status-popover";
import {
  ClipHotkeyPopover,
  RecordingHotkeyPopover,
} from "@/components/layout/hotkey-popovers";

/** Shared style for the min/maximize/close caption buttons. */
const CONTROL_CLS =
  "flex h-8 w-10 items-center justify-center rounded-md text-foreground/70 transition-colors hover:bg-secondary hover:text-foreground";

/**
 * Back/forward history control. Lit up (and clickable) only when there's
 * somewhere to go in that direction, dimmed and disabled otherwise — like a
 * browser's nav arrows.
 */
function NavButton({
  label,
  enabled,
  onClick,
  children,
}: {
  label: string;
  enabled: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      disabled={!enabled}
      onClick={onClick}
      className={cn(
        "flex size-6 items-center justify-center rounded transition-colors",
        enabled
          ? "text-foreground/70 hover:bg-secondary hover:text-foreground"
          : "cursor-not-allowed text-foreground/25"
      )}
    >
      {children}
    </button>
  );
}

/**
 * "Auto Clip" toggle pill. Reflects whether the Valorant orchestrator
 * auto-captures (`auto_capture_mode !== "manual"`); clicking flips between the
 * default `highlights` mode and `manual`. Persists through the settings mutation.
 */
function AutoClipToggle() {
  const { data: settings } = useSettings();
  const update = useUpdateSettings();
  const on = (settings?.auto_capture_mode ?? "highlights") !== "manual";
  const toggle = () => {
    if (!settings) return;
    update.mutate({
      ...settings,
      auto_capture_mode: on ? "manual" : "highlights",
    });
  };
  return (
    <button
      type="button"
      onClick={toggle}
      aria-pressed={on}
      className="flex h-8 items-center gap-2 rounded-lg border border-border bg-secondary/50 px-3 text-xs font-medium text-foreground transition-colors hover:bg-secondary"
    >
      <span>Auto Clip</span>
      <span
        className={cn(
          "font-semibold",
          on ? "text-success" : "text-muted-foreground"
        )}
      >
        {on ? "ON" : "OFF"}
      </span>
    </button>
  );
}

/** Windows-style "restore down" glyph (two stacked squares). */
function RestoreGlyph({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" className={className} aria-hidden>
      <path
        d="M5 5V3.5A1.5 1.5 0 0 1 6.5 2h6A1.5 1.5 0 0 1 14 3.5v6A1.5 1.5 0 0 1 12.5 11H11"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <rect
        x="2"
        y="5"
        width="9"
        height="9"
        rx="1.5"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

/** Best-effort Tauri window control; no-op outside the desktop shell. */
async function windowAction(action: "minimize" | "toggleMaximize" | "close") {
  try {
    await getCurrentWindow()[action]();
  } catch {
    /* running in a plain browser (vite dev) — ignore */
  }
}

export function WindowTitlebar() {
  const router = useRouter();
  // `router.history` is a stable object whose `canGoBack()`/`length` mutate in
  // place — impure reads. Computing them in the render body lets React Compiler
  // cache them against the (never-changing) `router`, freezing the arrows. Doing
  // it inside `useRouterState`'s selector instead recomputes on every router
  // state change (structural sharing keeps the result stable when unchanged).
  const { canBack, canForward } = useRouterState({
    select: (s) => {
      // No `canGoForward` in the history API — derive it: a forward entry exists
      // when the current index isn't the last one in the session history.
      const idx = (s.location.state as { __TSR_index?: number }).__TSR_index ?? 0;
      return {
        canBack: router.history.canGoBack(),
        canForward: idx < router.history.length - 1,
      };
    },
  });

  // Mirror the OS window's maximized state so the control swaps maximize/restore.
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    (async () => {
      try {
        const win = getCurrentWindow();
        setMaximized(await win.isMaximized());
        const off = await win.onResized(async () => {
          setMaximized(await win.isMaximized());
        });
        if (cancelled) off();
        else unlisten = off;
      } catch {
        /* plain browser — no window backend */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return (
    <header
      data-tauri-drag-region
      className="flex h-12 shrink-0 items-center justify-between border-b border-panel-border bg-panel px-4"
    >
      {/* Left: history, game status, hotkey hints */}
      <div data-tauri-drag-region className="flex items-center gap-3">
        <div className="flex items-center gap-1">
          <NavButton
            label="Back"
            enabled={canBack}
            onClick={() => router.history.back()}
          >
            <ArrowLeft className="size-4" />
          </NavButton>
          <NavButton
            label="Forward"
            enabled={canForward}
            onClick={() => router.history.forward()}
          >
            <ArrowRight className="size-4" />
          </NavButton>
        </div>

        <Separator orientation="vertical" className="h-4" />

        <div className="flex items-center gap-2">
          <RecorderStatusPopover />
          <ClipHotkeyPopover />
          <RecordingHotkeyPopover />
          <AutoClipToggle />
        </div>
      </div>

      {/* Right: window controls */}
      <div
        data-tauri-drag-region
        className="flex items-center gap-1 text-muted-foreground"
      >
        <button
          type="button"
          aria-label="Minimize"
          onClick={() => windowAction("minimize")}
          className={CONTROL_CLS}
        >
          <Minus className="size-[18px]" />
        </button>
        <button
          type="button"
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={() => windowAction("toggleMaximize")}
          className={CONTROL_CLS}
        >
          {maximized ? (
            <RestoreGlyph className="size-4" />
          ) : (
            <Square className="size-4" />
          )}
        </button>
        <button
          type="button"
          aria-label="Close"
          onClick={() => windowAction("close")}
          className={cn(CONTROL_CLS, "hover:bg-destructive hover:text-white")}
        >
          <X className="size-[18px]" />
        </button>
      </div>
    </header>
  );
}
