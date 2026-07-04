import * as React from "react";

import { MIN_TRIM } from "./constants";

/**
 * Global keyboard shortcuts for the clip viewer, extracted from `ViewerStage`.
 * Ignored while editing the title or in the save dialog. Shortcuts: Esc close,
 * ←/→ browse, Del/Backspace delete, Space/k play-pause, m mute, f fullscreen,
 * i/o set in/out points at the current time.
 */
export function useClipKeyboard({
  hasPrev,
  hasNext,
  onPrev,
  onNext,
  onClose,
  onDelete,
  togglePlay,
  toggleFullscreen,
  saveOpen,
  setSaveOpen,
  trimStart,
  trimEnd,
  setTrimStart,
  setTrimEnd,
  setTouched,
  setMuted,
  videoRef,
}: {
  hasPrev: boolean;
  hasNext: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onDelete: () => void;
  togglePlay: () => void;
  toggleFullscreen: () => void | Promise<void>;
  saveOpen: boolean;
  setSaveOpen: (open: boolean) => void;
  trimStart: number;
  trimEnd: number;
  setTrimStart: (t: number) => void;
  setTrimEnd: (t: number) => void;
  setTouched: (touched: boolean) => void;
  setMuted: React.Dispatch<React.SetStateAction<boolean>>;
  videoRef: React.RefObject<HTMLVideoElement | null>;
}) {
  React.useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      if (saveOpen) {
        if (e.key === "Escape") setSaveOpen(false);
        return;
      }
      switch (e.key) {
        case "Escape":
          if (!document.fullscreenElement) onClose();
          break;
        case "ArrowLeft":
          if (hasPrev) onPrev();
          break;
        case "ArrowRight":
          if (hasNext) onNext();
          break;
        case "Delete":
        case "Backspace":
          onDelete();
          break;
        case " ":
        case "k":
          e.preventDefault();
          togglePlay();
          break;
        case "m":
          setMuted((m) => !m);
          break;
        case "f":
          void toggleFullscreen();
          break;
        case "i": {
          const t = videoRef.current?.currentTime ?? 0;
          setTouched(true);
          setTrimStart(Math.min(t, trimEnd - MIN_TRIM));
          break;
        }
        case "o": {
          const t = videoRef.current?.currentTime ?? 0;
          setTouched(true);
          setTrimEnd(Math.max(t, trimStart + MIN_TRIM));
          break;
        }
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    hasPrev,
    hasNext,
    onPrev,
    onNext,
    onClose,
    onDelete,
    togglePlay,
    toggleFullscreen,
    saveOpen,
    trimStart,
    trimEnd,
    // setters + videoRef are stable; listed deps mirror the original effect.
    setSaveOpen,
    setTrimStart,
    setTrimEnd,
    setTouched,
    setMuted,
    videoRef,
  ]);
}
