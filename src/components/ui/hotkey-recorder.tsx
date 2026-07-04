/* eslint-disable react-refresh/only-export-components --
   `formatAccelerator` is a tiny display helper co-located with the recorder it
   serves. */
import { useCallback, useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";

/**
 * A keybind capture + display kit.
 *
 * `HotkeyRecorder` records a keystroke and emits a `global-hotkey` accelerator
 * string (the format Tauri's global-shortcut plugin parses): modifiers and a key
 * joined by "+", e.g. `"F9"`, `"Alt+F7"`, `"Control+Shift+C"`. `KeyCombo` renders
 * an accelerator string as keycaps so the titlebar, settings, and the recorder
 * itself all display combos the same way.
 */

// Codes we accept as the *main* key of a shortcut — mirrors what the
// global-hotkey parser understands (letters, digits, F1–F24, and a few named
// keys). Modifiers and anything else are rejected so a combo always has a key.
function codeToKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3); // KeyA -> A
  if (/^Digit[0-9]$/.test(code)) return code.slice(5); // Digit1 -> 1
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code; // F1..F24
  switch (code) {
    case "Space":
      return "Space";
    case "Enter":
    case "NumpadEnter":
      return "Enter";
    case "Tab":
      return "Tab";
    case "ArrowUp":
      return "Up";
    case "ArrowDown":
      return "Down";
    case "ArrowLeft":
      return "Left";
    case "ArrowRight":
      return "Right";
    default:
      return null;
  }
}

/** The modifiers currently held, in a stable canonical order. */
function eventMods(e: KeyboardEvent): string[] {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  return mods;
}

/** Friendly display labels for accelerator tokens (display only). */
const LABELS: Record<string, string> = {
  Control: "Ctrl",
  CommandOrControl: "Ctrl",
  CmdOrCtrl: "Ctrl",
  Alt: "Alt",
  Option: "Alt",
  Shift: "Shift",
  Super: "Win",
  Meta: "Win",
  Command: "Win",
  Cmd: "Win",
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
};

/** Split an accelerator string into display labels (e.g. "Alt+F7" -> ["Alt","F7"]). */
export function formatAccelerator(accel: string): string[] {
  return accel
    .split("+")
    .map((t) => t.trim())
    .filter(Boolean)
    .map((t) => LABELS[t] ?? t);
}

/** Render an accelerator string as keycaps. `lg` is the bold, plus-separated
 *  style used in the titlebar popovers; the default chip style suits inline use. */
export function KeyCombo({
  accel,
  size = "sm",
  className,
}: {
  accel: string;
  size?: "sm" | "lg";
  className?: string;
}) {
  const keys = formatAccelerator(accel);
  if (keys.length === 0) {
    return <span className="text-xs text-muted-foreground">Not set</span>;
  }
  const lg = size === "lg";
  return (
    <span className={cn("inline-flex items-center", lg ? "gap-2" : "gap-1.5", className)}>
      {keys.map((k, i) => (
        <span key={i} className={cn("inline-flex items-center", lg ? "gap-2" : "gap-1.5")}>
          {i > 0 && lg && <span className="text-sm font-normal text-muted-foreground">+</span>}
          {lg ? (
            <span className="text-lg font-semibold tracking-wide text-foreground">{k}</span>
          ) : (
            <kbd className="rounded bg-background/60 px-1.5 py-0.5 text-[11px] font-semibold text-foreground">
              {k}
            </kbd>
          )}
        </span>
      ))}
    </span>
  );
}

export function HotkeyRecorder({
  value,
  onChange,
  size = "sm",
  allowClear = true,
  className,
  "aria-label": ariaLabel,
}: {
  /** Current accelerator string. */
  value: string;
  /** Called with the new accelerator, or "" when cleared. */
  onChange: (accel: string) => void;
  /** `lg` is the full-width popover button; `sm` is a compact field. */
  size?: "sm" | "lg";
  /** Allow Backspace/Delete to clear the binding (emit ""). */
  allowClear?: boolean;
  className?: string;
  "aria-label"?: string;
}) {
  const [recording, setRecording] = useState(false);
  // Modifiers held while recording, before the main key lands — shown live.
  const [liveMods, setLiveMods] = useState<string[]>([]);
  const btnRef = useRef<HTMLButtonElement>(null);

  const stop = useCallback(() => {
    setRecording(false);
    setLiveMods([]);
    btnRef.current?.blur();
  }, []);

  useEffect(() => {
    if (!recording) return;

    const onKeyDown = (e: KeyboardEvent) => {
      // Swallow the keystroke so it neither types nor triggers other shortcuts
      // (incl. the popover's own Escape-to-close) while we're capturing.
      e.preventDefault();
      e.stopPropagation();

      if (e.code === "Escape") return stop(); // cancel, keep the old binding
      if (allowClear && (e.code === "Backspace" || e.code === "Delete")) {
        onChange("");
        return stop();
      }

      const key = codeToKey(e.code);
      if (!key) {
        // Only modifiers so far — reflect them live and keep waiting for a key.
        setLiveMods(eventMods(e));
        return;
      }
      onChange([...eventMods(e), key].join("+"));
      stop();
    };

    const onKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      setLiveMods(eventMods(e));
    };

    // Capture phase + window scope so we see the keys before anything else.
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [recording, onChange, allowClear, stop]);

  const lg = size === "lg";
  return (
    <button
      ref={btnRef}
      type="button"
      aria-label={ariaLabel ?? "Record hotkey"}
      onClick={() => setRecording(true)}
      onBlur={() => recording && stop()}
      className={cn(
        "inline-flex select-none items-center justify-center gap-2 rounded-md border outline-none transition-colors",
        lg ? "h-14 w-full px-4" : "h-9 min-w-[7rem] px-3 text-sm font-medium",
        recording
          ? "border-primary/70 bg-primary/10 ring-2 ring-primary/30"
          : "border-border/70 bg-secondary text-foreground hover:bg-[#323236]",
        className,
      )}
    >
      {recording ? (
        <span className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          {liveMods.length > 0 && <KeyCombo accel={liveMods.join("+")} size={size} />}
          <span className="animate-pulse">Press a key…</span>
        </span>
      ) : value ? (
        <KeyCombo accel={value} size={size} />
      ) : (
        <span className="text-sm text-muted-foreground">Click to set</span>
      )}
    </button>
  );
}
