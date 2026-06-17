import { useEffect, useState } from "react";
import { useRouter } from "@tanstack/react-router";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowLeft,
  ArrowRight,
  GameController,
  Minus,
  Square,
  X,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Separator } from "@/components/ui/separator";
import { useRecorderStatus } from "@/hooks/use-recorder";

/** Shared style for the min/maximize/close caption buttons. */
const CONTROL_CLS =
  "flex h-8 w-10 items-center justify-center rounded-md transition-colors hover:bg-secondary hover:text-foreground";

/** A keycap-style chip used for the hotkey hints. */
function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded bg-secondary px-1.5 py-0.5 text-[10px] font-medium text-secondary-foreground">
      {children}
    </span>
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
  const { data } = useRecorderStatus();
  const detected = data?.valorant_detected ?? false;

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
      <div data-tauri-drag-region className="flex items-center gap-4">
        <div className="flex items-center gap-1 text-muted-foreground">
          <button
            type="button"
            aria-label="Back"
            onClick={() => router.history.back()}
            className="flex size-6 items-center justify-center rounded transition-colors hover:text-foreground"
          >
            <ArrowLeft className="size-4" />
          </button>
          <button
            type="button"
            aria-label="Forward"
            onClick={() => router.history.forward()}
            className="flex size-6 items-center justify-center rounded transition-colors hover:text-foreground"
          >
            <ArrowRight className="size-4" />
          </button>
        </div>

        <Separator orientation="vertical" className="h-4" />

        <div className="pointer-events-none flex items-center gap-2.5">
          {detected ? (
            <>
              <span className="relative flex size-2">
                <span className="absolute inline-flex size-full animate-ping rounded-full bg-success/70" />
                <span className="relative inline-flex size-2 rounded-full bg-success" />
              </span>
              <div className="flex flex-col leading-tight">
                <span className="text-[10px] font-medium tracking-wide text-muted-foreground uppercase">
                  Now Clipping
                </span>
                <span className="text-sm font-semibold text-foreground">
                  Valorant
                </span>
              </div>
            </>
          ) : (
            <div className="flex items-center gap-2 text-sm font-medium text-foreground/60">
              <GameController className="size-4" weight="regular" />
              Waiting For Game
            </div>
          )}
        </div>

        <Separator orientation="vertical" className="ml-1 h-4" />

        <div className="pointer-events-none ml-1 flex items-center gap-4 text-xs text-muted-foreground">
          <span className="flex items-center gap-1.5">
            <Kbd>F9</Kbd>
            <span>Clip 30s</span>
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>ALT</Kbd>
            <Kbd>F7</Kbd>
            <span>Long Recording</span>
          </span>
          <span className="flex items-center gap-1.5">
            <span>Auto Clip</span>
            <span className="font-medium text-foreground">ON</span>
          </span>
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
