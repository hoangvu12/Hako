import * as React from "react";

import { MIN_TRIM } from "./constants";

/**
 * Trim-selection state + pointer handling for the clip editor, extracted from
 * `ViewerStage`.
 *
 * The filmstrip bar isn't click-to-seek; you grab the playhead or a trim handle.
 * Pointer capture is set on the bar so its move/up handlers receive the whole
 * drag. `duration` is the live value (the `<video>`'s reported duration once
 * loaded, else the stored fallback); `clipDuration` seeds the initial out point.
 *
 * Time is not tracked in React state — seeking sets `videoRef.currentTime`, and
 * the playhead/readout/overlay bar subscribe to the element's own events. Only
 * the selection bounds (`trimStart`/`trimEnd`) and the active `drag` live here.
 */
export function useTrimEditor({
  videoRef,
  barRef,
  duration,
  clipDuration,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  barRef: React.RefObject<HTMLDivElement | null>;
  duration: number;
  clipDuration: number;
}) {
  const [trimStart, setTrimStart] = React.useState(0);
  const [trimEnd, setTrimEnd] = React.useState(clipDuration);
  const [touched, setTouched] = React.useState(false); // user moved a handle
  const [drag, setDrag] = React.useState<null | "seek" | "start" | "end">(null);

  function timeFromX(clientX: number): number {
    const bar = barRef.current;
    if (!bar || !Number.isFinite(duration) || duration <= 0) return 0;
    const rect = bar.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return frac * duration;
  }

  function seekTo(clientX: number) {
    const v = videoRef.current;
    if (!v) return;
    // Keep the playhead inside the selection — the selection IS the clip now.
    const t = Math.min(Math.max(timeFromX(clientX), trimStart), trimEnd);
    // Setting currentTime fires `seeking`/`seeked`, which the playhead, readout,
    // and overlay seek bar subscribe to — no `current` state to push here.
    v.currentTime = t;
  }

  // Seek to an absolute time, clamped to the selection. Used by the fullscreen
  // overlay scrubber, which has no filmstrip bar to measure against.
  function seekToTime(t: number) {
    const v = videoRef.current;
    if (!v) return;
    const clamped = Math.min(Math.max(t, trimStart), trimEnd);
    v.currentTime = clamped;
  }

  // Keep the playhead inside the selection. This used to live in a useEffect
  // watching [trimStart, trimEnd], which runs a frame late (the playhead lagged
  // between the trim commit and the clamp). Doing it inline in the one handler
  // that tightens the range clamps in the same render.
  function clampPlayheadInto(start: number, end: number) {
    const v = videoRef.current;
    if (!v) return;
    if (v.currentTime < start) {
      v.currentTime = start;
    } else if (v.currentTime > end) {
      v.currentTime = end;
    }
  }
  function onBarPointerMove(e: React.PointerEvent) {
    if (!drag) return;
    if (drag === "seek") {
      seekTo(e.clientX);
    } else if (drag === "start") {
      const next = Math.min(Math.max(timeFromX(e.clientX), 0), trimEnd - MIN_TRIM);
      setTrimStart(next);
      clampPlayheadInto(next, trimEnd);
    } else {
      const next = Math.max(Math.min(timeFromX(e.clientX), duration), trimStart + MIN_TRIM);
      setTrimEnd(next);
      clampPlayheadInto(trimStart, next);
    }
  }
  function endDrag(e: React.PointerEvent) {
    setDrag(null);
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* not captured */
    }
  }
  function startHandle(which: "start" | "end", e: React.PointerEvent) {
    e.stopPropagation();
    e.preventDefault();
    barRef.current?.setPointerCapture(e.pointerId);
    setTouched(true);
    setDrag(which);
  }
  function startSeek(e: React.PointerEvent) {
    e.stopPropagation();
    e.preventDefault();
    barRef.current?.setPointerCapture(e.pointerId);
    setDrag("seek");
  }
  // Clicking / dragging the ruler scrubs the playhead (kept inside the range).
  function onRulerPointerDown(e: React.PointerEvent) {
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag("seek");
    seekTo(e.clientX);
  }

  return {
    trimStart,
    trimEnd,
    setTrimStart,
    setTrimEnd,
    touched,
    setTouched,
    drag,
    seekToTime,
    onBarPointerMove,
    endDrag,
    startHandle,
    startSeek,
    onRulerPointerDown,
  };
}
