import * as React from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useNavigate } from "@tanstack/react-router";
import {
  X,
  CaretLeft,
  CaretRight,
  Play,
  Pause,
  SpeakerSimpleHigh,
  SpeakerSimpleX,
  CornersOut,
  CornersIn,
  Lightning,
  Scissors,
  PencilSimple,
  Copy,
  Check,
  Trash,
  Gauge,
  FloppyDisk,
  CircleNotch,
  ArrowCounterClockwise,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  useClips,
  useDeleteClip,
  useRenameClip,
  useTrimClip,
} from "@/hooks/use-library";
import type { ClipRecord, TrimMode } from "@/lib/api";

const SPEEDS = [1, 1.5, 2, 0.5] as const;
const MIN_TRIM = 0.3; // shortest selectable range, seconds
/** Tiles in the Rust-generated sprite-sheet filmstrip (commands.rs FILMSTRIP_TILES). */
const FILMSTRIP_TILES = 16;
/**
 * Custom range-aware streaming scheme (src-tauri/src/media.rs). The clip video
 * loads through this instead of the `asset:` protocol so WebView2 gets proper
 * `206 Partial Content` seeking and doesn't starve during playback.
 */
const STREAM_SCHEME = "hakoclip";

function fmtTime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function fmtClock(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const whole = Math.floor(secs);
  const tenth = Math.floor((secs - whole) * 10);
  return `${fmtTime(whole)}.${tenth}`;
}

function fmtSize(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

function fmtDate(unixMs: number): string {
  return new Date(unixMs).toLocaleDateString(undefined, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

/** Pick a "nice" ruler step (≈8 ticks) for a given duration. */
function rulerStep(duration: number): number {
  const target = duration / 8;
  const steps = [1, 2, 5, 8, 10, 15, 20, 30, 60, 120, 300];
  return steps.find((s) => s >= target) ?? Math.ceil(target);
}

export function ClipViewer({ clipId }: { clipId: string }) {
  const navigate = useNavigate();
  const { data: clips, isLoading } = useClips();
  const del = useDeleteClip();
  const rename = useRenameClip();
  const trim = useTrimClip();

  const list = clips ?? [];
  const index = list.findIndex((c) => String(c.id) === clipId);
  const clip = index >= 0 ? list[index] : undefined;
  const prev = index > 0 ? list[index - 1] : undefined;
  const next = index >= 0 && index < list.length - 1 ? list[index + 1] : undefined;

  const close = React.useCallback(() => navigate({ to: "/clips" }), [navigate]);
  const goto = React.useCallback(
    (c?: ClipRecord) =>
      c && navigate({ to: "/clips/$clipId", params: { clipId: String(c.id) } }),
    [navigate],
  );

  const handleDelete = React.useCallback(() => {
    if (!clip) return;
    if (!window.confirm(`Delete “${clip.title || "Untitled"}”? This removes the file.`))
      return;
    const fallback = next ?? prev;
    del.mutate(clip.id, {
      onSuccess: () => (fallback ? goto(fallback) : close()),
    });
  }, [clip, next, prev, del, goto, close]);

  // Run a trim; on "copy" jump to the freshly-created clip.
  const handleTrim = React.useCallback(
    async (args: { start: number; end: number; dropAudio: boolean; mode: TrimMode }) => {
      if (!clip) return;
      const rec = await trim.mutateAsync({ id: clip.id, ...args });
      if (args.mode === "copy") goto(rec);
    },
    [clip, trim, goto],
  );

  return (
    <div className="fixed inset-x-0 bottom-0 top-12 z-50 flex bg-black/85 backdrop-blur-sm">
      {/* Backdrop click-to-close (content stops propagation) */}
      <button
        type="button"
        aria-label="Close"
        onClick={close}
        className="absolute inset-0 cursor-default"
      />

      {!clip ? (
        <div className="relative z-10 flex flex-1 items-center justify-center text-sm text-muted-foreground">
          {isLoading ? "Loading…" : "Clip not found — it may have been deleted."}
          <button
            type="button"
            onClick={close}
            className="absolute top-4 right-4 flex size-9 items-center justify-center rounded-full bg-white/10 text-white transition-colors hover:bg-white/20"
          >
            <X weight="bold" className="size-4" />
          </button>
        </div>
      ) : (
        <ViewerStage
          // size in the key → an in-place overwrite remounts the editor fresh.
          key={`${clip.id}:${clip.size_bytes}`}
          clip={clip}
          hasPrev={!!prev}
          hasNext={!!next}
          onPrev={() => goto(prev)}
          onNext={() => goto(next)}
          onClose={close}
          onDelete={handleDelete}
          onRename={(title) =>
            title && title !== clip.title && rename.mutate({ id: clip.id, title })
          }
          onTrim={handleTrim}
          trimPending={trim.isPending}
          trimError={trim.error ? String(trim.error) : null}
        />
      )}
    </div>
  );
}

function ViewerStage({
  clip,
  hasPrev,
  hasNext,
  onPrev,
  onNext,
  onClose,
  onDelete,
  onRename,
  onTrim,
  trimPending,
  trimError,
}: {
  clip: ClipRecord;
  hasPrev: boolean;
  hasNext: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
  onTrim: (args: {
    start: number;
    end: number;
    dropAudio: boolean;
    mode: TrimMode;
  }) => Promise<void>;
  trimPending: boolean;
  trimError: string | null;
}) {
  const stageRef = React.useRef<HTMLDivElement>(null);
  const videoRef = React.useRef<HTMLVideoElement>(null);
  const barRef = React.useRef<HTMLDivElement>(null);

  // Cache-bust so an overwrite (same path, new bytes) actually reloads. The video
  // streams over our range-aware scheme; images stay on the plain asset protocol.
  const src = React.useMemo(
    () => `${convertFileSrc(clip.path, STREAM_SCHEME)}?v=${clip.size_bytes}`,
    [clip.path, clip.size_bytes],
  );
  const poster = clip.thumb_path
    ? `${convertFileSrc(clip.thumb_path)}?v=${clip.size_bytes}`
    : undefined;
  const filmstripUrl = clip.filmstrip_path
    ? `${convertFileSrc(clip.filmstrip_path)}?v=${clip.size_bytes}`
    : undefined;

  const [playing, setPlaying] = React.useState(false);
  const [muted, setMuted] = React.useState(false);
  const [volume, setVolume] = React.useState(1);
  const [current, setCurrent] = React.useState(0);
  const [duration, setDuration] = React.useState(clip.duration_secs);
  const [fullscreen, setFullscreen] = React.useState(false);
  const [speedIdx, setSpeedIdx] = React.useState(0);

  // Editor state
  const [trimStart, setTrimStart] = React.useState(0);
  const [trimEnd, setTrimEnd] = React.useState(clip.duration_secs);
  const [touched, setTouched] = React.useState(false); // user moved a handle
  const [audioEnabled, setAudioEnabled] = React.useState(true);
  const [drag, setDrag] = React.useState<null | "seek" | "start" | "end">(null);
  const [saveOpen, setSaveOpen] = React.useState(false);

  // Reflect muted/volume/speed onto the element (React doesn't track these).
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted;
  }, [muted]);
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.volume = volume;
  }, [volume]);
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.playbackRate = SPEEDS[speedIdx];
  }, [speedIdx]);

  // Adjusting the selection past the playhead snaps it back inside the range.
  React.useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.currentTime < trimStart) {
      v.currentTime = trimStart;
      setCurrent(trimStart);
    } else if (v.currentTime > trimEnd) {
      v.currentTime = trimEnd;
      setCurrent(trimEnd);
    }
  }, [trimStart, trimEnd]);

  const togglePlay = React.useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      // Start playback from the selection if we're outside it.
      if (v.currentTime < trimStart - 0.05 || v.currentTime >= trimEnd - 0.05)
        v.currentTime = trimStart;
      void v.play().catch(() => {});
    } else v.pause();
  }, [trimStart, trimEnd]);

  const toggleFullscreen = React.useCallback(async () => {
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await stageRef.current?.requestFullscreen();
    } catch {
      /* fullscreen unavailable */
    }
  }, []);

  React.useEffect(() => {
    const onChange = () =>
      setFullscreen(document.fullscreenElement === stageRef.current);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

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
    v.currentTime = t;
    setCurrent(t);
  }

  // --- playhead / trim-handle pointer handling ---
  // The bar isn't click-to-seek; you grab the playhead or a trim handle. Capture
  // is set on the bar so its move/up handlers receive the whole drag.
  function onBarPointerMove(e: React.PointerEvent) {
    if (!drag) return;
    if (drag === "seek") {
      seekTo(e.clientX);
    } else if (drag === "start") {
      setTrimStart(Math.min(Math.max(timeFromX(e.clientX), 0), trimEnd - MIN_TRIM));
    } else {
      setTrimEnd(Math.max(Math.min(timeFromX(e.clientX), duration), trimStart + MIN_TRIM));
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

  // Keyboard shortcuts — ignored while editing the title or in the save dialog.
  React.useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable))
        return;
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
        case "i":
          setTouched(true);
          setTrimStart(Math.min(current, trimEnd - MIN_TRIM));
          break;
        case "o":
          setTouched(true);
          setTrimEnd(Math.max(current, trimStart + MIN_TRIM));
          break;
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    hasPrev, hasNext, onPrev, onNext, onClose, onDelete, togglePlay,
    toggleFullscreen, saveOpen, current, trimStart, trimEnd,
  ]);

  const progress = duration > 0 ? (current / duration) * 100 : 0;
  const startPct = duration > 0 ? (trimStart / duration) * 100 : 0;
  const endPct = duration > 0 ? (trimEnd / duration) * 100 : 100;
  // Ruler ticks depend only on duration — memoize so playhead updates (which fire
  // a few times a second) don't rebuild these arrays on every render.
  const { ticks, minorTicks } = React.useMemo(() => {
    const step = rulerStep(duration);
    const ticks: number[] = [];
    for (let t = 0; t <= duration + 0.001; t += step) ticks.push(t);
    // Finer ticks (4 per major step) for the ruler's measure marks.
    const minorStep = step / 4;
    const minorTicks: { pct: number; major: boolean }[] = [];
    for (let i = 0; i * minorStep <= duration + 0.001; i++) {
      minorTicks.push({ pct: ((i * minorStep) / duration) * 100, major: i % 4 === 0 });
    }
    return { ticks, minorTicks };
  }, [duration]);

  const trimmed = clip.event != null;
  const selDuration = trimEnd - trimStart;
  const edited =
    trimStart > 0.05 || trimEnd < duration - 0.05 || !audioEnabled;

  return (
    <div className="relative z-10 flex min-w-0 flex-1">
      {/* ---- Stage (player + editor) ---- */}
      <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-4 p-6 pr-2">
        {hasPrev ? <NavArrow side="left" onClick={onPrev} /> : null}
        {hasNext ? <NavArrow side="right" onClick={onNext} /> : null}

        {/* Video + overlay controls (this element goes fullscreen) */}
        <div
          ref={stageRef}
          className={cn(
            "group/video relative flex min-h-0 w-full flex-1 items-center justify-center",
            fullscreen && "bg-black",
          )}
        >
          <video
            ref={videoRef}
            src={src}
            poster={poster}
            autoPlay
            playsInline
            onClick={togglePlay}
            onLoadedMetadata={(e) => {
              const d = e.currentTarget.duration;
              if (Number.isFinite(d) && d > 0) {
                setDuration(d);
                if (!touched) setTrimEnd(d);
              }
            }}
            onTimeUpdate={(e) => {
              const v = e.currentTarget;
              if (drag !== "seek") setCurrent(v.currentTime);
              // Confine playback to the selection: loop at the out point, and
              // never play through the trimmed-away head.
              if (!v.paused && !drag && (v.currentTime >= trimEnd - 0.02 || v.currentTime < trimStart))
                v.currentTime = trimStart;
            }}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            className="max-h-full max-w-full rounded-lg bg-black object-contain shadow-2xl"
          />

          {/* Overlay control bar */}
          <div className="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex items-center gap-3 rounded-b-lg bg-gradient-to-t from-black/80 via-black/30 to-transparent px-4 pt-10 pb-3 text-white opacity-0 transition-opacity group-hover/video:opacity-100 [&>*]:pointer-events-auto">
            <CtrlButton label={playing ? "Pause" : "Play"} onClick={togglePlay}>
              {playing ? (
                <Pause weight="fill" className="size-5" />
              ) : (
                <Play weight="fill" className="size-5" />
              )}
            </CtrlButton>

            <div className="group/vol flex items-center gap-2">
              <CtrlButton
                label={muted ? "Unmute" : "Mute"}
                onClick={() => setMuted((m) => !m)}
              >
                {muted || volume === 0 ? (
                  <SpeakerSimpleX weight="fill" className="size-5" />
                ) : (
                  <SpeakerSimpleHigh weight="fill" className="size-5" />
                )}
              </CtrlButton>
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={muted ? 0 : volume}
                onChange={(e) => {
                  const v = Number(e.target.value);
                  setVolume(v);
                  setMuted(v === 0);
                }}
                aria-label="Volume"
                className="h-1 w-0 cursor-pointer appearance-none rounded-full bg-white/30 opacity-0 transition-all duration-200 outline-none group-hover/vol:w-20 group-hover/vol:opacity-100 [&::-webkit-slider-thumb]:size-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white"
              />
            </div>

            <span className="font-mono text-xs tabular-nums text-white/85">
              {fmtTime(current)} / {fmtTime(duration)}
            </span>

            <span className="flex-1" />

            <CtrlButton
              label={`Speed ${SPEEDS[speedIdx]}×`}
              onClick={() => setSpeedIdx((i) => (i + 1) % SPEEDS.length)}
            >
              <Gauge weight="bold" className="size-5" />
              <span className="ml-1 text-xs font-semibold tabular-nums">
                {SPEEDS[speedIdx]}×
              </span>
            </CtrlButton>
            <CtrlButton
              label={fullscreen ? "Exit fullscreen" : "Fullscreen"}
              onClick={toggleFullscreen}
            >
              {fullscreen ? (
                <CornersIn weight="bold" className="size-5" />
              ) : (
                <CornersOut weight="bold" className="size-5" />
              )}
            </CtrlButton>
          </div>
        </div>

        {/* ---- Trim editor (filmstrip + toolbar) ---- */}
        {!fullscreen ? (
          <div className="w-full max-w-6xl shrink-0">
            {/* Big current-time readout */}
            <div className="mb-2 flex items-baseline justify-center gap-2">
              <span className="font-mono text-3xl font-semibold tabular-nums tracking-tight">
                {fmtClock(current)}
              </span>
              <span className="font-mono text-lg tabular-nums text-muted-foreground">
                / {fmtTime(duration)}
              </span>
            </div>

            {/* Padded so the trim handles have room at the very ends */}
            <div className="px-4">
              {/* Ruler — click / drag anywhere here to move the playhead */}
              <div
                onPointerDown={onRulerPointerDown}
                onPointerMove={onBarPointerMove}
                onPointerUp={endDrag}
                className="relative h-9 cursor-pointer touch-none select-none"
              >
                {ticks.map((t) => (
                  <span
                    key={t}
                    className="pointer-events-none absolute top-0 -translate-x-1/2 font-mono text-xs tabular-nums text-muted-foreground"
                    style={{ left: `${(t / duration) * 100}%` }}
                  >
                    {fmtTime(t)}
                  </span>
                ))}
                {/* measure ticks */}
                <div className="pointer-events-none absolute inset-x-0 bottom-0 h-3.5">
                  {minorTicks.map((m, i) => (
                    <span
                      key={i}
                      className="absolute bottom-0 w-px -translate-x-1/2 bg-muted-foreground/35"
                      style={{ left: `${m.pct}%`, height: m.major ? "100%" : "55%" }}
                    />
                  ))}
                </div>
              </div>

              {/* Filmstrip — frames clipped; selection chrome on an unclipped layer */}
              <div className="relative mt-1.5 h-24 select-none">
                {/* Clipped frame strip */}
                <div
                  ref={barRef}
                  onPointerMove={onBarPointerMove}
                  onPointerUp={endDrag}
                  className="absolute inset-0 touch-none overflow-hidden rounded-lg border border-white/10 bg-black/40"
                >
                  {/* Frames — sliced out of the Rust sprite-sheet (no webview decode) */}
                  <FilmstripStrip sprite={filmstripUrl} poster={poster} />

                  {/* Dim the trimmed-away regions (outside the selection) */}
                  <div
                    className="pointer-events-none absolute inset-y-0 left-0 bg-black/60"
                    style={{ width: `${startPct}%` }}
                  />
                  <div
                    className="pointer-events-none absolute inset-y-0 right-0 bg-black/60"
                    style={{ width: `${100 - endPct}%` }}
                  />
                </div>

                {/* Unclipped selection chrome: a cohesive rounded frame whose
                    sides ARE the grab handles, plus the playhead. */}
                <div className="pointer-events-none absolute inset-0">
                  {/* Selection frame */}
                  <div
                    className="absolute inset-y-0 rounded-lg border-[3px] border-white"
                    style={{ left: `${startPct}%`, right: `${100 - endPct}%` }}
                  />

                  {/* Handles integrated into the frame edges */}
                  <TrimHandle side="start" pct={startPct} onPointerDown={(e) => startHandle("start", e)} />
                  <TrimHandle side="end" pct={endPct} onPointerDown={(e) => startHandle("end", e)} />

                  {/* Draggable playhead (flag + needle) */}
                  <div
                    onPointerDown={startSeek}
                    role="slider"
                    aria-label="Playhead"
                    className="pointer-events-auto absolute -top-2 -bottom-2 z-40 flex w-5 -translate-x-1/2 cursor-ew-resize justify-center touch-none"
                    style={{ left: `${progress}%` }}
                  >
                    <span className="h-full w-0.5 bg-primary shadow-[0_0_8px] shadow-primary/50" />
                    <span className="absolute -top-1 left-1/2 h-3.5 w-3 -translate-x-1/2 rounded-sm bg-primary shadow" />
                  </div>
                </div>
              </div>
            </div>

            {/* Editor toolbar */}
            <div className="mt-4 flex items-center gap-3 px-4">
              <button
                type="button"
                onClick={() => setAudioEnabled((a) => !a)}
                className={cn(
                  "flex items-center gap-2 rounded-lg border px-4 py-2.5 text-sm font-medium transition-colors",
                  audioEnabled
                    ? "border-border/70 bg-card/50 text-foreground hover:bg-card"
                    : "border-border/50 bg-transparent text-muted-foreground hover:text-foreground",
                )}
                title="Include audio in the saved clip"
              >
                {audioEnabled ? (
                  <SpeakerSimpleHigh weight="fill" className="size-5" />
                ) : (
                  <SpeakerSimpleX weight="fill" className="size-5" />
                )}
                Audio {audioEnabled ? "On" : "Off"}
              </button>

              <div className="flex items-center gap-2 font-mono text-sm tabular-nums text-muted-foreground">
                <Scissors weight="bold" className="size-4 text-primary-text" />
                <span className="text-foreground">{fmtClock(trimStart)}</span>
                <span>–</span>
                <span className="text-foreground">{fmtClock(trimEnd)}</span>
                <span className="text-muted-foreground/70">({fmtClock(selDuration)})</span>
              </div>

              <span className="flex-1" />

              {edited ? (
                <button
                  type="button"
                  onClick={() => {
                    setTouched(false);
                    setTrimStart(0);
                    setTrimEnd(duration);
                    setAudioEnabled(true);
                  }}
                  className="flex items-center gap-2 rounded-lg px-4 py-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
                >
                  <ArrowCounterClockwise weight="bold" className="size-5" />
                  Reset
                </button>
              ) : null}

              <button
                type="button"
                disabled={!edited}
                onClick={() => setSaveOpen(true)}
                className="flex items-center gap-2 rounded-lg bg-primary px-6 py-2.5 text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <FloppyDisk weight="bold" className="size-5" />
                Save
              </button>
            </div>

            {/* Navigation hint */}
            <p className="mt-3 text-center text-xs text-muted-foreground/70">
              <Kbd>I</Kbd>/<Kbd>O</Kbd> set in/out · <Kbd>Space</Kbd> play ·{" "}
              <Kbd>←</Kbd> <Kbd>→</Kbd> browse · <Kbd>Del</Kbd> delete ·{" "}
              <Kbd>Esc</Kbd> close
            </p>
          </div>
        ) : null}
      </div>

      {/* ---- Details panel ---- */}
      <aside className="scrollbar-thin flex w-[340px] shrink-0 flex-col overflow-y-auto border-l border-panel-border bg-panel">
        <div className="flex items-center justify-between border-b border-panel-border px-5 py-4">
          <h2 className="text-sm font-semibold">Details</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="flex size-8 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
          >
            <X weight="bold" className="size-4" />
          </button>
        </div>

        <div className="flex flex-1 flex-col gap-5 p-5">
          <EditableTitle title={clip.title} onCommit={onRename} />

          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>{fmtDate(clip.created_unix_ms)}</span>
            <span className="size-[3px] rounded-full bg-secondary" />
            <span className="font-mono tabular-nums">{fmtTime(clip.duration_secs)}</span>
          </div>

          <div className="flex flex-wrap gap-2">
            {trimmed ? (
              <span className="inline-flex items-center gap-1.5 rounded-md bg-warning/15 px-2.5 py-1 text-xs font-medium text-warning">
                <Scissors weight="fill" className="size-3.5" />
                {clip.event}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 rounded-md bg-info/15 px-2.5 py-1 text-xs font-medium text-info">
                <Lightning weight="fill" className="size-3.5" />
                Auto Clip
              </span>
            )}
          </div>

          <dl className="space-y-2.5 rounded-lg border border-border/60 bg-card/40 p-4 text-xs">
            <SpecRow label="Resolution" value={`${clip.width}×${clip.height}`} />
            <SpecRow label="Duration" value={fmtTime(clip.duration_secs)} />
            <SpecRow label="File size" value={fmtSize(clip.size_bytes)} />
            <SpecRow label="Saved" value={fmtDate(clip.created_unix_ms)} />
          </dl>

          <CopyPath path={clip.path} />

          <div className="mt-auto" />
          <button
            type="button"
            onClick={onDelete}
            className="flex items-center justify-center gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-2.5 text-sm font-medium text-destructive transition-colors hover:bg-destructive/20"
          >
            <Trash weight="bold" className="size-4" />
            Delete clip
          </button>
        </div>
      </aside>

      {/* ---- Save dialog ---- */}
      {saveOpen ? (
        <SaveDialog
          title={clip.title}
          selDuration={selDuration}
          audioEnabled={audioEnabled}
          pending={trimPending}
          error={trimError}
          onCancel={() => setSaveOpen(false)}
          onChoose={async (mode) => {
            // Overwriting replaces the file the webview has open — release it
            // first so the rename can't lose a fight with a Windows file lock.
            const v = videoRef.current;
            if (mode === "overwrite" && v) {
              v.pause();
              v.removeAttribute("src");
              v.load();
            }
            try {
              await onTrim({ start: trimStart, end: trimEnd, dropAudio: !audioEnabled, mode });
              setSaveOpen(false);
              // On success an overwrite bumps size_bytes → the stage remounts
              // and reloads on its own; nothing else to do here.
            } catch {
              // Restore playback if the overwrite failed (error shown in dialog).
              if (mode === "overwrite" && v) {
                v.src = src;
                v.load();
              }
            }
          }}
        />
      ) : null}
    </div>
  );
}

/**
 * The scrubber's frame strip. Renders `FILMSTRIP_TILES` slots, each showing one
 * tile of the Rust-generated sprite sheet via `background-position` — so there's
 * no second `<video>` decoding in the webview (which used to contend with
 * playback for the hardware decoder). Memoized: playhead ticks don't touch it.
 * Falls back to a repeated poster for clips saved before filmstrips existed.
 */
const FilmstripStrip = React.memo(function FilmstripStrip({
  sprite,
  poster,
}: {
  sprite?: string;
  poster?: string;
}) {
  if (sprite) {
    return (
      <div className="pointer-events-none absolute inset-0 flex">
        {Array.from({ length: FILMSTRIP_TILES }, (_, i) => (
          <div
            key={i}
            className="h-full min-w-0 flex-1"
            style={{
              backgroundImage: `url(${sprite})`,
              // Stretch the sprite to N slot-widths, then step to tile i — exact:
              // tile i lands flush in slot i (see media/filmstrip layout).
              backgroundSize: `${FILMSTRIP_TILES * 100}% 100%`,
              backgroundPosition: `${(i / (FILMSTRIP_TILES - 1)) * 100}% 0`,
              backgroundRepeat: "no-repeat",
            }}
          />
        ))}
      </div>
    );
  }
  return (
    <div className="pointer-events-none absolute inset-0">
      {poster ? (
        <div
          className="size-full opacity-40"
          style={{
            backgroundImage: `url(${poster})`,
            backgroundSize: "auto 100%",
            backgroundRepeat: "repeat-x",
          }}
        />
      ) : (
        <div className="size-full bg-white/5" />
      )}
    </div>
  );
});

function TrimHandle({
  side,
  pct,
  onPointerDown,
}: {
  side: "start" | "end";
  pct: number;
  onPointerDown: (e: React.PointerEvent) => void;
}) {
  const isStart = side === "start";
  return (
    <div
      onPointerDown={onPointerDown}
      role="slider"
      aria-label={isStart ? "Trim start" : "Trim end"}
      // The handle straddles the frame edge so it reads as the frame's thickened
      // side (Medal-style). White to match the frame; a grey grip line inside.
      className={cn(
        "pointer-events-auto absolute inset-y-0 z-30 flex w-3 -translate-x-1/2 cursor-ew-resize items-center justify-center touch-none bg-white shadow-md",
        isStart ? "rounded-l-lg" : "rounded-r-lg",
      )}
      style={{ left: `${pct}%` }}
    >
      <span className="h-6 w-0.5 rounded-full bg-zinc-400" />
    </div>
  );
}

function SaveDialog({
  title,
  selDuration,
  audioEnabled,
  pending,
  error,
  onCancel,
  onChoose,
}: {
  title: string;
  selDuration: number;
  audioEnabled: boolean;
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onChoose: (mode: TrimMode) => void;
}) {
  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center">
      <button
        type="button"
        aria-label="Cancel"
        onClick={pending ? undefined : onCancel}
        className="absolute inset-0 cursor-default bg-black/60 backdrop-blur-sm"
      />
      <div className="relative z-10 w-[380px] rounded-2xl border border-border bg-popover p-6 shadow-2xl">
        <h3 className="text-base font-semibold">Save trim</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          {fmtClock(selDuration)} selected · audio {audioEnabled ? "kept" : "removed"}.
          Choose how to save “{title || "Untitled"}”.
        </p>

        {error ? (
          <p className="mt-3 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </p>
        ) : null}

        <div className="mt-5 flex flex-col gap-2">
          <button
            type="button"
            disabled={pending}
            onClick={() => onChoose("copy")}
            className="flex items-center gap-3 rounded-lg border border-border bg-card/50 px-4 py-3 text-left transition-colors hover:bg-card disabled:opacity-50"
          >
            <Copy weight="bold" className="size-5 shrink-0 text-primary-text" />
            <span>
              <span className="block text-sm font-medium">Save as a copy</span>
              <span className="block text-xs text-muted-foreground">
                Keep the original, add a new trimmed clip
              </span>
            </span>
          </button>
          <button
            type="button"
            disabled={pending}
            onClick={() => onChoose("overwrite")}
            className="flex items-center gap-3 rounded-lg border border-border bg-card/50 px-4 py-3 text-left transition-colors hover:bg-card disabled:opacity-50"
          >
            <FloppyDisk weight="bold" className="size-5 shrink-0 text-warning" />
            <span>
              <span className="block text-sm font-medium">Overwrite original</span>
              <span className="block text-xs text-muted-foreground">
                Replace the clip — this can’t be undone
              </span>
            </span>
          </button>
        </div>

        <div className="mt-4 flex items-center justify-between">
          <button
            type="button"
            disabled={pending}
            onClick={onCancel}
            className="text-sm text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
          >
            Cancel
          </button>
          {pending ? (
            <span className="flex items-center gap-2 text-sm text-muted-foreground">
              <CircleNotch weight="bold" className="size-4 animate-spin" />
              Saving…
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function NavArrow({ side, onClick }: { side: "left" | "right"; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={side === "left" ? "Previous clip" : "Next clip"}
      className={cn(
        "absolute top-1/2 z-30 flex size-11 -translate-y-1/2 items-center justify-center rounded-full bg-black/50 text-white backdrop-blur-sm transition-colors hover:bg-black/75",
        side === "left" ? "left-3" : "right-3",
      )}
    >
      {side === "left" ? (
        <CaretLeft weight="bold" className="size-5" />
      ) : (
        <CaretRight weight="bold" className="size-5" />
      )}
    </button>
  );
}

function CtrlButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className="flex items-center text-white/90 transition-opacity hover:text-white hover:opacity-100"
    >
      {children}
    </button>
  );
}

function EditableTitle({
  title,
  onCommit,
}: {
  title: string;
  onCommit: (title: string) => void;
}) {
  const [editing, setEditing] = React.useState(false);
  const [draft, setDraft] = React.useState(title);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => setDraft(title), [title]);
  React.useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  function commit() {
    setEditing(false);
    const v = draft.trim();
    if (v) onCommit(v);
    else setDraft(title);
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") {
            setDraft(title);
            setEditing(false);
          }
        }}
        className="w-full rounded-md border border-border bg-field px-2.5 py-1.5 text-lg font-semibold outline-none focus:border-ring"
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      className="group/title flex items-start gap-2 text-left"
    >
      <span className="text-lg font-semibold leading-tight">{title || "Untitled"}</span>
      <PencilSimple className="mt-1 size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/title:opacity-100" />
    </button>
  );
}

function SpecRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="font-mono tabular-nums text-foreground">{value}</dd>
    </div>
  );
}

function CopyPath({ path }: { path: string }) {
  const [copied, setCopied] = React.useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  return (
    <button
      type="button"
      onClick={copy}
      className="flex items-center justify-center gap-2 rounded-lg border border-border/60 bg-card/40 px-4 py-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
    >
      {copied ? <Check className="size-4 text-success" /> : <Copy className="size-4" />}
      {copied ? "Copied path" : "Copy file path"}
    </button>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="rounded border border-border/70 bg-card/60 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
      {children}
    </kbd>
  );
}
