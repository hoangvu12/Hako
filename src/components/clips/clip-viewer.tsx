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
  GearSix,
  FloppyDisk,
  CircleNotch,
  DownloadSimple,
  ArrowCounterClockwise,
  FolderOpen,
  Faders,
  Sparkle,
  Skull,
  Knife,
  Bomb,
  Trophy,
  Fire,
  Handshake,
  ShieldCheck,
  type Icon as PhosphorIcon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  useClips,
  useClipAudioTracks,
  useDeleteClip,
  useRemuxClip,
  useRenameClip,
  useTrimClip,
} from "@/hooks/use-library";
import { Slider } from "@/components/ui/slider";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useTrackMixer } from "@/hooks/use-track-mixer";
import {
  useClipDownload,
  useClipRemoteUrl,
  useDownloadClip,
} from "@/hooks/use-cloud";
import { useValorantAssets, mapNameFromPath } from "@/hooks/use-valorant-assets";
import { revealClip } from "@/lib/api";
import type {
  AudioTrackInfo,
  ClipRecord,
  EventMark,
  TrackVolume,
  TrimMode,
} from "@/lib/api";

/** Per-stem editor state. Solo overrides mute across the stem set. */
interface TrackCtl {
  muted: boolean;
  solo: boolean;
  /** 0–100. */
  volume: number;
  /** Offline noise suppression (RNNoise) on this stem — the mic's "noise
   *  cancel". Applied in the live preview *and* baked into the export. */
  denoise: boolean;
}
// Noise cancel is opt-in (off by default) on every stem — the editor never
// re-encodes or loads the denoiser unless the user turns it on for a track.
const DEFAULT_CTL: TrackCtl = { muted: false, solo: false, volume: 100, denoise: false };

/** Playback-rate choices in the settings popover (YouTube-style, ascending). */
const SPEED_OPTIONS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
const MIN_TRIM = 0.3; // shortest selectable range, seconds
/** Tiles in the Rust-generated sprite-sheet filmstrip (commands.rs FILMSTRIP_TILES). */
const FILMSTRIP_TILES = 16;
/** How many frames the scrubber actually draws — fewer than the sprite has, so
 *  each slot is wide enough to show a frame (almost) uncropped (Medal-style). At
 *  the ~50px strip height a 16:9 frame is ~89px wide, so ~13 of them tile the bar
 *  with each one showing essentially edge-to-edge (matches Medal's compact bar). */
const FILMSTRIP_VISIBLE = 13;
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

/** Icon + tint for a seek-bar event marker, keyed off the EventKind label. */
function eventIconFor(label: string): { Icon: PhosphorIcon; tint: string } {
  const l = label.toLowerCase();
  if (l.includes("victory")) return { Icon: Trophy, tint: "text-warning" };
  if (l.includes("clutch")) return { Icon: Fire, tint: "text-warning" };
  if (l.includes("knife")) return { Icon: Knife, tint: "text-white" };
  if (l.includes("defus")) return { Icon: ShieldCheck, tint: "text-info" };
  if (l.includes("spike") || l.includes("detonat"))
    return { Icon: Bomb, tint: "text-destructive" };
  if (l.includes("death")) return { Icon: Skull, tint: "text-destructive" };
  if (l.includes("assist")) return { Icon: Handshake, tint: "text-info" };
  // Kills (single + multi-kill + ace) and anything unrecognised.
  return { Icon: Skull, tint: "text-white" };
}

/** Pick a "nice" ruler step (≈8 ticks) for a given duration. */
function rulerStep(duration: number): number {
  const target = duration / 8;
  const steps = [1, 2, 5, 8, 10, 15, 20, 30, 60, 120, 300];
  return steps.find((s) => s >= target) ?? Math.ceil(target);
}

/**
 * Subscribe to a <video>'s playback position. Returns the current time, updated
 * from the element's own `timeupdate` (during playback) and `seeking`/`seeked`
 * (during scrubbing). Keeping this in small leaf components — instead of one
 * `current` state on `ViewerStage` — means the heavy editor (filmstrip, ruler,
 * details panel) no longer re-renders ~10×/sec while a clip plays; only the
 * playhead, the time readout, and the overlay seek bar do.
 */
function useVideoTime(
  videoRef: React.RefObject<HTMLVideoElement | null>,
): number {
  const [time, setTime] = React.useState(0);
  React.useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const sync = () => setTime(v.currentTime);
    sync();
    v.addEventListener("timeupdate", sync);
    v.addEventListener("seeking", sync);
    v.addEventListener("seeked", sync);
    return () => {
      v.removeEventListener("timeupdate", sync);
      v.removeEventListener("seeking", sync);
      v.removeEventListener("seeked", sync);
    };
  }, [videoRef]);
  return time;
}

/** Live `0:03 / 0:12` readout — isolated so it, not the player, ticks. */
function TimeReadout({
  videoRef,
  duration,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  duration: number;
}) {
  const current = useVideoTime(videoRef);
  return (
    <span className="font-mono text-xs tabular-nums text-white/85">
      {fmtTime(current)} / {fmtTime(duration)}
    </span>
  );
}

/** The draggable filmstrip playhead — positioned from the live playback time. */
function Playhead({
  videoRef,
  duration,
  onPointerDown,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  duration: number;
  onPointerDown: (e: React.PointerEvent) => void;
}) {
  const current = useVideoTime(videoRef);
  const progress = duration > 0 ? (current / duration) * 100 : 0;
  return (
    <div
      onPointerDown={onPointerDown}
      role="slider"
      aria-label="Playhead"
      aria-valuemin={0}
      aria-valuemax={Math.round(duration)}
      aria-valuenow={Math.round(current)}
      aria-valuetext={fmtTime(current)}
      className="pointer-events-auto absolute -top-2 -bottom-2 z-40 flex w-5 -translate-x-1/2 cursor-ew-resize justify-center touch-none"
      style={{ left: `${progress}%` }}
    >
      <span className="h-full w-0.5 bg-primary shadow-[0_0_8px] shadow-primary/50" />
      <span className="absolute -top-1 left-1/2 h-3.5 w-3 -translate-x-1/2 rounded-sm bg-primary shadow" />
    </div>
  );
}

export function ClipViewer({ clipId }: { clipId: string }) {
  const navigate = useNavigate();
  const { data: clips, isLoading } = useClips();
  const del = useDeleteClip();
  const rename = useRenameClip();
  const trim = useTrimClip();
  const remux = useRemuxClip();

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

  // Run the export; on "copy" jump to the freshly-created clip. When `tracks`
  // is provided (a per-track audio mix), re-mux; otherwise it's a loss-less
  // stream-copy trim that keeps every existing audio track.
  const handleExport = React.useCallback(
    async (args: {
      start: number;
      end: number;
      dropAudio: boolean;
      tracks: TrackVolume[] | null;
      mode: TrimMode;
    }) => {
      if (!clip) return;
      const rec = args.tracks
        ? await remux.mutateAsync({
            id: clip.id,
            start: args.start,
            end: args.end,
            tracks: args.tracks,
            mode: args.mode,
          })
        : await trim.mutateAsync({
            id: clip.id,
            start: args.start,
            end: args.end,
            dropAudio: args.dropAudio,
            mode: args.mode,
          });
      if (args.mode === "copy") goto(rec);
    },
    [clip, trim, remux, goto],
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
          onExport={handleExport}
          exportPending={trim.isPending || remux.isPending}
          exportError={
            trim.error || remux.error ? String(trim.error ?? remux.error) : null
          }
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
  onExport,
  exportPending,
  exportError,
}: {
  clip: ClipRecord;
  hasPrev: boolean;
  hasNext: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
  onExport: (args: {
    start: number;
    end: number;
    dropAudio: boolean;
    tracks: TrackVolume[] | null;
    mode: TrimMode;
  }) => Promise<void>;
  exportPending: boolean;
  exportError: string | null;
}) {
  const stageRef = React.useRef<HTMLDivElement>(null);
  const videoRef = React.useRef<HTMLVideoElement>(null);
  const barRef = React.useRef<HTMLDivElement>(null);

  // Cloud-only (evicted) clips have no local file — "free up space" deleted it.
  // Play them straight from the presigned remote URL (range-capable, so seeking
  // still works); local clips stream over our range-aware `hakoclip://` scheme.
  const cloudUrl = useClipRemoteUrl(clip.id);
  // "Download to edit": re-fetch the evicted file so trim/export can run locally.
  const download = useDownloadClip();
  const dl = useClipDownload(clip.id);
  // Cache-bust so an overwrite (same path, new bytes) actually reloads. The video
  // streams over our range-aware scheme; images stay on the plain asset protocol.
  const src = React.useMemo(
    () =>
      clip.evicted
        ? (cloudUrl ?? undefined)
        : `${convertFileSrc(clip.path, STREAM_SCHEME)}?v=${clip.size_bytes}`,
    [clip.evicted, cloudUrl, clip.path, clip.size_bytes],
  );
  const poster = clip.thumb_path
    ? `${convertFileSrc(clip.thumb_path)}?v=${clip.size_bytes}`
    : undefined;
  const filmstripUrl = clip.filmstrip_path
    ? `${convertFileSrc(clip.filmstrip_path)}?v=${clip.size_bytes}`
    : undefined;

  // Stored duration is the render-time fallback; the <video>'s reported duration
  // (genuinely new data) wins once loaded. Reading the prop directly avoids the
  // stale copy a `useState(clip.duration_secs)` would hold if `clip` changed.
  const [muted, setMuted] = React.useState(false);
  const [volume, setVolume] = React.useState(1);
  const [videoDuration, setVideoDuration] = React.useState<number | null>(null);
  const duration = videoDuration ?? clip.duration_secs;
  const [fullscreen, setFullscreen] = React.useState(false);
  // Speed lives in <SettingsButton> (it has no audio coupling), but mute/volume stay
  // here: they feed the live audio mixer's master gain *and* the "m" shortcut, so
  // isolating them safely needs a shared store rather than a ref bridge.

  // Editor state
  const [trimStart, setTrimStart] = React.useState(0);
  const [trimEnd, setTrimEnd] = React.useState(clip.duration_secs);
  const [touched, setTouched] = React.useState(false); // user moved a handle
  const [audioEnabled, setAudioEnabled] = React.useState(true);
  const [drag, setDrag] = React.useState<null | "seek" | "start" | "end">(null);
  const [saveOpen, setSaveOpen] = React.useState(false);

  // Multi-track audio: stems are the audio tracks past the master (index 0).
  // When a clip has stems the editor offers per-track mute/solo/volume, applied
  // on export via a re-mux (browsers can't switch MP4 audio tracks live).
  const { data: audioTracks } = useClipAudioTracks(clip.id);
  const stems = React.useMemo<AudioTrackInfo[]>(
    () => (audioTracks ?? []).filter((t) => t.index >= 1),
    [audioTracks],
  );
  const hasStems = stems.length > 0;
  const [trackCtl, setTrackCtl] = React.useState<Record<number, TrackCtl>>({});
  const ctlOf = React.useCallback(
    (idx: number): TrackCtl => trackCtl[idx] ?? DEFAULT_CTL,
    [trackCtl],
  );
  const patchTrack = React.useCallback(
    (idx: number, patch: Partial<TrackCtl>) =>
      setTrackCtl((prev) => ({
        ...prev,
        [idx]: { ...(prev[idx] ?? DEFAULT_CTL), ...patch },
      })),
    [],
  );
  // Stable handlers so the memoized <AudioSettingsPopover> doesn't re-render on
  // unrelated stage updates (trim drags, etc.) — only when the stem state it
  // actually reads changes.
  const toggleAudio = React.useCallback(() => setAudioEnabled((a) => !a), []);
  const onStemMute = React.useCallback(
    (idx: number) => patchTrack(idx, { muted: !ctlOf(idx).muted }),
    [patchTrack, ctlOf],
  );
  const onStemSolo = React.useCallback(
    (idx: number) => patchTrack(idx, { solo: !ctlOf(idx).solo }),
    [patchTrack, ctlOf],
  );
  const onStemVolume = React.useCallback(
    (idx: number, v: number) => patchTrack(idx, { volume: v }),
    [patchTrack],
  );
  const onStemDenoise = React.useCallback(
    (idx: number) => patchTrack(idx, { denoise: !ctlOf(idx).denoise }),
    [patchTrack, ctlOf],
  );

  const soloActive = stems.some((s) => ctlOf(s.index).solo);
  // A stem is audible when soloed (if any solo is active) or simply un-muted.
  const audibleStems = stems.filter((s) =>
    soloActive ? ctlOf(s.index).solo : !ctlOf(s.index).muted,
  );
  // The mix differs from the recorded master when a stem is muted/soloed, its
  // volume moved, or noise cancel is on — otherwise we keep the loss-less stream
  // copy. Uses `ctlOf` (not raw `trackCtl`) so the mic's default-on noise cancel
  // counts even before the user touches anything.
  const tracksEdited =
    hasStems &&
    stems.some((s) => {
      const c = ctlOf(s.index);
      return c.muted || c.solo || c.volume !== 100 || c.denoise;
    });
  // Stem indices to noise-cancel in the live preview — kept in lockstep with the
  // export's per-stem `denoise` flag so what you hear matches what you save.
  const denoiseStemIdx = React.useMemo(
    () => stems.filter((s) => ctlOf(s.index).denoise).map((s) => s.index),
    [stems, ctlOf],
  );

  // Live per-stem mixing: decode the stems and play them through a Web Audio
  // gain graph synced to the (muted) <video>, so mute/solo/volume are *heard*
  // during preview — not just applied on export. `active` is false (native
  // <video> audio kept) for no-stems clips or until/unless the decode succeeds.
  const stemGains = React.useMemo(() => {
    const m = new Map<number, number>();
    for (const s of stems) {
      const c = ctlOf(s.index);
      const audible = soloActive ? c.solo : !c.muted;
      m.set(s.index, audible ? c.volume / 100 : 0);
    }
    return m;
  }, [stems, ctlOf, soloActive]);
  // Top-bar mute/volume is the monitor level (preview-only; not in the export mix).
  const masterMonitorGain = muted ? 0 : volume;
  const {
    active: liveMix,
    decoding: mixDecoding,
    denoisingIdx,
  } = useTrackMixer({
    clipId: clip.id,
    fileSize: clip.size_bytes,
    stems,
    videoRef,
    stemGains,
    masterGain: masterMonitorGain,
    denoiseStemIdx,
  });

  // Reflect muted/volume onto the element (React doesn't track these). While live
  // mixing is active the element stays muted (Web Audio carries the sound); the
  // top-bar mute/volume then drives the graph's master gain instead.
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.muted = liveMix || muted;
  }, [muted, liveMix]);
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.volume = volume;
  }, [volume]);

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

  // --- playhead / trim-handle pointer handling ---
  // The bar isn't click-to-seek; you grab the playhead or a trim handle. Capture
  // is set on the bar so its move/up handlers receive the whole drag.
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
    hasPrev, hasNext, onPrev, onNext, onClose, onDelete, togglePlay,
    toggleFullscreen, saveOpen, trimStart, trimEnd,
  ]);

  const startPct = duration > 0 ? (trimStart / duration) * 100 : 0;
  const endPct = duration > 0 ? (trimEnd / duration) * 100 : 100;
  // Ruler ticks depend only on duration — memoize so playhead updates (which fire
  // a few times a second) don't rebuild these arrays on every render.
  const { ticks, minorTicks } = React.useMemo(() => {
    const step = rulerStep(duration);
    const ticks: number[] = [];
    for (let t = 0; t <= duration + 0.001; t += step) ticks.push(t);
    // Finer ticks (10 per major step) for the ruler's measure marks.
    const minorStep = step / 10;
    const minorTicks: { pct: number; major: boolean }[] = [];
    for (let i = 0; i * minorStep <= duration + 0.001; i++) {
      minorTicks.push({ pct: ((i * minorStep) / duration) * 100, major: i % 10 === 0 });
    }
    return { ticks, minorTicks };
  }, [duration]);

  const selDuration = trimEnd - trimStart;
  const edited =
    trimStart > 0.05 || trimEnd < duration - 0.05 || !audioEnabled || tracksEdited;

  return (
    <div className="relative z-10 flex min-w-0 flex-1">
      {/* ---- Stage (player + editor) ---- */}
      <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-4 p-6 pr-2">
        {/* Video + overlay controls (this element goes fullscreen) */}
        <div
          ref={stageRef}
          className={cn(
            "group/video relative flex min-h-0 w-full flex-1 items-center justify-center",
            fullscreen && "bg-black",
          )}
        >
          {/* Prev/next anchored to the player's own edges (hidden in fullscreen) */}
          {hasPrev && !fullscreen ? <NavArrow side="left" onClick={onPrev} /> : null}
          {hasNext && !fullscreen ? <NavArrow side="right" onClick={onNext} /> : null}

          <video
            ref={videoRef}
            src={src}
            poster={poster}
            // Seed the element's intrinsic size from the stored dimensions. A
            // <video> defaults to 300×150 until its poster/metadata loads, so
            // without this the player paints tiny on mount and then jumps to
            // full size — the "small for a moment, then zoom in" flash. With the
            // real aspect known up front, `max-w/h-full object-contain` fits it
            // at its final size from the first frame.
            width={clip.width || undefined}
            height={clip.height || undefined}
            autoPlay
            playsInline
            onClick={togglePlay}
            onLoadedMetadata={(e) => {
              const d = e.currentTarget.duration;
              if (Number.isFinite(d) && d > 0) {
                setVideoDuration(d);
                if (!touched) setTrimEnd(d);
              }
            }}
            onTimeUpdate={(e) => {
              const v = e.currentTarget;
              // The playhead/readout/seek bar track time via their own
              // `useVideoTime` subscription — nothing to push here. We only
              // confine playback to the selection: loop at the out point, and
              // never play through the trimmed-away head.
              if (!v.paused && !drag && (v.currentTime >= trimEnd - 0.02 || v.currentTime < trimStart))
                v.currentTime = trimStart;
            }}
            className={cn(
              "bg-black",
              fullscreen
                // Fill the whole screen, cropping to fit (no letterboxing).
                ? "absolute inset-0 size-full object-cover"
                : "max-h-full max-w-full rounded-lg object-contain shadow-2xl",
            )}
          />

          {/* Overlay control bar — always visible, with a seek bar on top (the
              filmstrip editor below is the precise scrubber; this is the quick one). */}
          <div className="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex flex-col gap-1.5 rounded-b-lg bg-gradient-to-t from-black/85 via-black/35 to-transparent px-4 pt-12 pb-3 text-white [&>*]:pointer-events-auto">
            <OverlaySeekBar
              videoRef={videoRef}
              start={trimStart}
              end={trimEnd}
              marks={clip.event_marks}
              onSeek={seekToTime}
            />
            <div className="flex items-center gap-3">
            <PlayPauseButton videoRef={videoRef} onToggle={togglePlay} />

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

            <TimeReadout videoRef={videoRef} duration={duration} />

            <span className="flex-1" />

            <SettingsButton videoRef={videoRef} />
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
        </div>

        {/* ---- Trim editor (filmstrip + toolbar) ---- */}
        {!fullscreen ? (
          <div className="w-full max-w-6xl shrink-0">
            {/* Padded so the trim handles have room at the very ends */}
            <div className="px-4">
              {/* Ruler — click / drag anywhere here to move the playhead */}
              <div
                onPointerDown={onRulerPointerDown}
                onPointerMove={onBarPointerMove}
                onPointerUp={endDrag}
                className="group/ruler relative h-9 cursor-pointer touch-none select-none"
              >
                {ticks.map((t) => (
                  <span
                    key={t}
                    className="pointer-events-none absolute top-0 -translate-x-1/2 font-sans text-[11px] font-medium tabular-nums text-white"
                    style={{ left: `${(t / duration) * 100}%` }}
                  >
                    {fmtTime(t)}
                  </span>
                ))}
                {/* measure strip — a contained "tape" surface so the ticks read
                    as a ruler instead of floating marks on the backdrop */}
                <div className="pointer-events-none absolute inset-x-0 bottom-0 h-4 overflow-hidden rounded-md bg-gradient-to-b from-white/[0.06] to-white/[0.015] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]">
                  {minorTicks.map((m, i) => (
                    <span
                      key={i}
                      className={cn(
                        "absolute bottom-0 w-px -translate-x-1/2",
                        m.major ? "bg-white" : "bg-white/45",
                      )}
                      style={{ left: `${m.pct}%`, height: "45%" }}
                    />
                  ))}
                </div>
              </div>

              {/* Filmstrip — frames clipped; selection chrome on an unclipped layer */}
              <div className="relative mt-1.5 h-[50px] select-none">
                {/* Clipped frame strip */}
                <div
                  ref={barRef}
                  onPointerMove={onBarPointerMove}
                  onPointerUp={endDrag}
                  className="absolute inset-0 touch-none overflow-hidden rounded-lg border border-white/10 bg-black/40"
                >
                  {/* Frames — sliced out of the Rust sprite-sheet (no webview decode) */}
                  <FilmstripStrip sprite={filmstripUrl} poster={poster} />

                  {/* Subtle base veil over the whole strip (Medal-style) so the
                      white selection frame and the playhead read clearly against
                      the frames. The trimmed-away regions get an extra dim on top. */}
                  <div className="pointer-events-none absolute inset-0 bg-black/20" />

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
                  <Playhead
                    videoRef={videoRef}
                    duration={duration}
                    onPointerDown={startSeek}
                  />
                </div>
              </div>
            </div>

            {/* Editor toolbar */}
            <div className="mt-3 flex items-center gap-2.5 px-4">
              <AudioSettingsPopover
                audioEnabled={audioEnabled}
                onToggleAudio={toggleAudio}
                hasStems={hasStems}
                stems={stems}
                decoding={mixDecoding}
                denoisingIdx={denoisingIdx}
                ctlOf={ctlOf}
                soloActive={soloActive}
                onMute={onStemMute}
                onSolo={onStemSolo}
                onVolume={onStemVolume}
                onDenoise={onStemDenoise}
              />

              <div className="flex items-center gap-1.5 font-mono text-xs tabular-nums text-muted-foreground">
                <Scissors weight="bold" className="size-3.5 text-primary-text" />
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
                    setTrackCtl({});
                  }}
                  className="flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
                >
                  <ArrowCounterClockwise weight="bold" className="size-4" />
                  Reset
                </button>
              ) : null}

              <button
                type="button"
                disabled={!edited || clip.evicted}
                onClick={() => setSaveOpen(true)}
                title={
                  clip.evicted
                    ? "This clip is stored in the cloud only — editing needs its local file"
                    : undefined
                }
                className="flex items-center gap-1.5 rounded-lg bg-primary px-5 py-1.5 text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <FloppyDisk weight="bold" className="size-4" />
                Save
              </button>
            </div>

            {/* Navigation hint — or, for an evicted clip, the download-to-edit
                affordance (re-fetch the cloud copy so trim/export can run). */}
            {clip.evicted ? (
              <div className="mt-3 flex flex-col items-center gap-2">
                {dl.downloading || download.isPending ? (
                  <div className="flex w-64 flex-col gap-1.5">
                    <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
                      <div
                        className="h-full rounded-full bg-primary transition-[width] duration-200"
                        style={{ width: `${Math.max(4, dl.pct)}%` }}
                      />
                    </div>
                    <p className="text-center text-xs text-muted-foreground/80">
                      Downloading from cloud… {Math.round(dl.pct)}%
                    </p>
                  </div>
                ) : (
                  <>
                    <button
                      type="button"
                      onClick={() => download.mutate(clip.id)}
                      className="flex items-center gap-1.5 rounded-lg bg-primary px-4 py-1.5 text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90"
                    >
                      <DownloadSimple weight="bold" className="size-4" />
                      Download to edit
                    </button>
                    <p className="max-w-sm text-center text-xs text-muted-foreground/70">
                      Cloud-only clip — its local copy was freed up to save space.
                      Playing from the cloud; download it to trim or export.
                    </p>
                    {download.error ? (
                      <p className="text-center text-xs text-destructive">
                        {String(download.error)}
                      </p>
                    ) : null}
                  </>
                )}
              </div>
            ) : (
              <p className="mt-3 text-center text-xs text-muted-foreground/70">
                <Kbd>I</Kbd>/<Kbd>O</Kbd> set in/out · <Kbd>Space</Kbd> play ·{" "}
                <Kbd>←</Kbd> <Kbd>→</Kbd> browse · <Kbd>Del</Kbd> delete ·{" "}
                <Kbd>Esc</Kbd> close
              </p>
            )}
          </div>
        ) : null}
      </div>

      {/* ---- Details panel ---- */}
      <DetailsPanel
        clip={clip}
        onClose={onClose}
        onRename={onRename}
        onDelete={onDelete}
      />

      {/* ---- Save dialog ---- */}
      {saveOpen ? (
        <SaveDialog
          title={clip.title}
          selDuration={selDuration}
          audioSummary={
            !audioEnabled
              ? "audio removed"
              : (tracksEdited
                  ? audibleStems.length === 0
                    ? "all tracks muted"
                    : `${audibleStems.length} of ${stems.length} audio track${
                        stems.length === 1 ? "" : "s"
                      } mixed`
                  : hasStems
                    ? "all audio tracks kept"
                    : "audio kept") +
                // Note noise cancel when it's on for a stem that'll be heard.
                (audibleStems.some((s) => ctlOf(s.index).denoise)
                  ? " · noise cancelled"
                  : "")
          }
          pending={exportPending}
          error={exportError}
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
            // A per-track mix re-muxes; otherwise it's a loss-less trim that
            // keeps every existing audio track.
            const tracks: TrackVolume[] | null =
              audioEnabled && hasStems && tracksEdited
                ? audibleStems.map((s) => ({
                    index: s.index,
                    volume: ctlOf(s.index).volume,
                    denoise: ctlOf(s.index).denoise,
                  }))
                : null;
            try {
              await onExport({
                start: trimStart,
                end: trimEnd,
                dropAudio: !audioEnabled,
                tracks,
                mode,
              });
              setSaveOpen(false);
              // On success an overwrite bumps size_bytes → the stage remounts
              // and reloads on its own; nothing else to do here.
            } catch {
              // Restore playback if the overwrite failed (error shown in dialog).
              // Only reachable for local clips (export is disabled when evicted),
              // so `src` is always a string here; coalesce to satisfy the type.
              if (mode === "overwrite" && v) {
                v.src = src ?? "";
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
    // Show fewer, *wider* frames (Medal-style): 16 frames packed across the bar
    // makes each slot far narrower than a 16:9 frame, so ~30% of every frame gets
    // cropped. Drawing ~10 evenly-sampled frames makes each slot about as wide as
    // a full frame, so each one shows (almost) edge-to-edge. We still sample from
    // all 16 sprite tiles, so the strip still spans the whole clip.
    return (
      <div className="pointer-events-none absolute inset-0 flex">
        {Array.from({ length: FILMSTRIP_VISIBLE }, (_, j) => {
          // Map this slot to a sprite tile, spread evenly across the 16 tiles.
          const tile = Math.round((j / (FILMSTRIP_VISIBLE - 1)) * (FILMSTRIP_TILES - 1));
          return (
            // The sprite is one image of N tiles, each at the video's *native*
            // aspect (thumbs.rs scales tile_h from src) — render it height-fitted
            // (`h-full max-w-none` → natural width, no squash) and slide the chosen
            // tile to the slot centre. `translateX -%` is relative to the image's
            // own width, so this stays correct at any video aspect or screen size;
            // `overflow-hidden` center-crops whatever little overflows.
            <div key={j} className="relative h-full min-w-0 flex-1 overflow-hidden">
              <img
                src={sprite}
                alt=""
                draggable={false}
                className="absolute top-0 left-1/2 h-full max-w-none select-none"
                style={{
                  transform: `translateX(-${((tile + 0.5) / FILMSTRIP_TILES) * 100}%)`,
                }}
              />
            </div>
          );
        })}
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
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
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

/**
 * Per-track mute/solo/volume for a multi-track clip's stems. The recorded clip
 * carries a master mix (track 0, what the player uses) plus raw stems; these
 * controls choose how the stems are re-mixed into the exported master. Solo
 * overrides mute: if any stem is soloed, only soloed stems are audible.
 */
/**
 * The toolbar's "Audio" control: a popover holding the master include-audio
 * toggle plus, for multi-track clips, the per-stem mute/solo/volume mixer. The
 * recorded clip carries a master mix (track 0, what the player uses) plus raw
 * stems; these controls choose how the stems are re-mixed into the export. Solo
 * overrides mute: if any stem is soloed, only soloed stems are audible.
 */
const AudioSettingsPopover = React.memo(function AudioSettingsPopover({
  audioEnabled,
  onToggleAudio,
  hasStems,
  stems,
  decoding,
  denoisingIdx,
  ctlOf,
  soloActive,
  onMute,
  onSolo,
  onVolume,
  onDenoise,
}: {
  audioEnabled: boolean;
  onToggleAudio: () => void;
  hasStems: boolean;
  stems: AudioTrackInfo[];
  /** Decoding the stems for the live preview mix; controls aren't audible yet. */
  decoding: boolean;
  /** Stem indices whose noise-cancel preview is still being computed (spinner). */
  denoisingIdx: number[];
  ctlOf: (idx: number) => TrackCtl;
  soloActive: boolean;
  onMute: (idx: number) => void;
  onSolo: (idx: number) => void;
  onVolume: (idx: number, v: number) => void;
  onDenoise: (idx: number) => void;
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          title="Audio settings"
          className={cn(
            "flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-sm font-medium transition-colors",
            audioEnabled
              ? "border-border/70 bg-card/50 text-foreground hover:bg-card"
              : "border-border/50 bg-transparent text-muted-foreground hover:text-foreground",
          )}
        >
          {audioEnabled ? (
            <SpeakerSimpleHigh weight="fill" className="size-4" />
          ) : (
            <SpeakerSimpleX weight="fill" className="size-4" />
          )}
          Audio
          <Faders weight="bold" className="size-4 opacity-70" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[24rem] p-0">
        {/* Master toggle: include this clip's audio in the export */}
        <button
          type="button"
          role="switch"
          aria-checked={audioEnabled}
          onClick={onToggleAudio}
          className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-white/5"
        >
          <span className="min-w-0">
            <span className="block text-sm font-medium text-foreground">
              Include audio
            </span>
            <span className="block text-xs text-muted-foreground">
              {audioEnabled ? "Saved with sound" : "Saved without sound"}
            </span>
          </span>
          <span
            className={cn(
              "relative h-5 w-9 shrink-0 rounded-full transition-colors",
              audioEnabled ? "bg-primary" : "bg-muted-foreground/30",
            )}
          >
            <span
              className={cn(
                "absolute top-0.5 left-0.5 size-4 rounded-full bg-white transition-transform",
                audioEnabled && "translate-x-4",
              )}
            />
          </span>
        </button>

        {/* Per-stem mixer (multi-track clips only, when audio is kept) */}
        {hasStems && audioEnabled ? (
          <div className="border-t border-panel-border px-4 py-3">
            <div className="mb-2.5 flex items-center gap-2 text-xs font-medium text-muted-foreground">
              Tracks
              {decoding ? (
                <span className="flex items-center gap-1.5 font-normal text-muted-foreground/70">
                  <CircleNotch weight="bold" className="size-3 animate-spin" />
                  Decoding…
                </span>
              ) : denoisingIdx.length ? (
                // Text, not just the spinner: the OS "reduce motion" setting
                // freezes every CSS animation, so a lone spinner reads as idle —
                // the label is what tells the user noise cancel is working.
                <span className="flex items-center gap-1.5 font-normal text-info/80">
                  <CircleNotch weight="bold" className="size-3 animate-spin" />
                  Cancelling noise…
                </span>
              ) : null}
            </div>
            <div
              className={cn(
                "flex flex-col gap-2.5",
                // While decoding, the per-stem controls can't be *heard* yet
                // (native master audio plays meanwhile) — blur + dim them so it
                // reads as "preparing," not "broken." Snap it on/off: animating
                // the `filter` repaints every frame and stutters.
                decoding && "pointer-events-none select-none opacity-50 blur-[2px]",
              )}
              aria-busy={decoding}
            >
              {stems.map((s) => {
                const c = ctlOf(s.index);
                const audible = soloActive ? c.solo : !c.muted;
                const denoising = denoisingIdx.includes(s.index);
                return (
                  <div key={s.index} className="flex items-center gap-2">
                    <span
                      className={cn(
                        "w-20 shrink-0 truncate text-[13px]",
                        audible ? "text-foreground" : "text-muted-foreground/60",
                      )}
                      title={s.name}
                    >
                      {s.name}
                    </span>
                    <button
                      type="button"
                      onClick={() => onMute(s.index)}
                      aria-label={c.muted ? "Unmute track" : "Mute track"}
                      title={c.muted ? "Unmute" : "Mute"}
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border transition-colors",
                        c.muted
                          ? "border-destructive/40 bg-destructive/10 text-destructive"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {c.muted ? (
                        <SpeakerSimpleX weight="fill" className="size-3.5" />
                      ) : (
                        <SpeakerSimpleHigh weight="fill" className="size-3.5" />
                      )}
                    </button>
                    <button
                      type="button"
                      onClick={() => onSolo(s.index)}
                      aria-label={c.solo ? "Unsolo track" : "Solo track"}
                      title="Solo"
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border text-xs font-bold transition-colors",
                        c.solo
                          ? "border-primary/50 bg-primary/15 text-primary-text"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      S
                    </button>
                    <button
                      type="button"
                      onClick={() => onDenoise(s.index)}
                      aria-label={c.denoise ? "Disable noise cancel" : "Enable noise cancel"}
                      aria-pressed={c.denoise}
                      aria-busy={denoising}
                      title={
                        denoising
                          ? "Preparing noise cancel…"
                          : c.denoise
                            ? "Noise cancel on (removes background noise on export)"
                            : "Noise cancel off"
                      }
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border transition-colors",
                        c.denoise
                          ? "border-info/50 bg-info/15 text-info"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {denoising ? (
                        <CircleNotch weight="bold" className="size-3.5 animate-spin" />
                      ) : (
                        <Sparkle
                          weight={c.denoise ? "fill" : "regular"}
                          className="size-3.5"
                        />
                      )}
                    </button>
                    <Slider
                      min={0}
                      // 200% = +6 dB boost. gain (volume/100) is unclamped in
                      // both preview (Web Audio GainNode) and export
                      // (`remux_with_tracks`), so a stem can be amplified, not
                      // just attenuated; >100% may clip if the stem is already hot.
                      max={200}
                      value={[c.volume]}
                      onValueChange={([v]) => onVolume(s.index, v)}
                      disabled={!audible}
                      aria-label={`${s.name} volume`}
                      className="min-w-0 flex-1"
                    />
                    <span className="w-9 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
                      {c.volume}%
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        ) : null}
      </PopoverContent>
    </Popover>
  );
});

function SaveDialog({
  title,
  selDuration,
  audioSummary,
  pending,
  error,
  onCancel,
  onChoose,
}: {
  title: string;
  selDuration: number;
  audioSummary: string;
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
          {fmtClock(selDuration)} selected · {audioSummary}. Choose how to save “
          {title || "Untitled"}”.
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
        "absolute top-1/2 z-30 flex size-11 -translate-y-1/2 items-center justify-center rounded-full bg-secondary text-secondary-foreground backdrop-blur-sm transition-colors hover:bg-secondary/80",
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

/**
 * Play/pause button — owns the `playing` flag itself, subscribed to the video's
 * own `play`/`pause` events. Keeping it here (instead of as `ViewerStage` state)
 * means toggling playback re-renders only this button, not the whole player +
 * editor + details panel.
 */
function PlayPauseButton({
  videoRef,
  onToggle,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  onToggle: () => void;
}) {
  const [playing, setPlaying] = React.useState(false);
  React.useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const onPlay = () => setPlaying(true);
    const onPause = () => setPlaying(false);
    setPlaying(!v.paused);
    v.addEventListener("play", onPlay);
    v.addEventListener("pause", onPause);
    return () => {
      v.removeEventListener("play", onPlay);
      v.removeEventListener("pause", onPause);
    };
  }, [videoRef]);
  return (
    <CtrlButton label={playing ? "Pause" : "Play"} onClick={onToggle}>
      {playing ? (
        <Pause weight="fill" className="size-5" />
      ) : (
        <Play weight="fill" className="size-5" />
      )}
    </CtrlButton>
  );
}

/** Settings gear → a YouTube-style popover. Today it holds just playback speed
 *  (a two-level menu: "Playback speed" row → the rate list). Owns its own state
 *  so changing speed doesn't re-render the stage; writes `playbackRate` straight
 *  onto the element. */
function SettingsButton({
  videoRef,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
}) {
  const [open, setOpen] = React.useState(false);
  const [view, setView] = React.useState<"main" | "speed">("main");
  const [speed, setSpeed] = React.useState(1);
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.playbackRate = speed;
  }, [speed, videoRef]);
  const label = speed === 1 ? "Normal" : `${speed}×`;
  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        // Always reopen on the top-level menu, never the speed sub-list.
        if (!o) setView("main");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Settings"
          title="Settings"
          className="flex items-center text-white/90 transition hover:text-white"
        >
          <GearSix
            weight="fill"
            className={cn(
              "size-6 transition-transform duration-200",
              open && "rotate-45",
            )}
          />
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="end" sideOffset={12} className="w-72 p-0">
        {view === "main" ? (
          <button
            type="button"
            onClick={() => setView("speed")}
            className="flex w-full items-center justify-between gap-3 px-4 py-3.5 text-left transition-colors hover:bg-white/5"
          >
            <span className="flex items-center gap-2.5 text-sm font-medium text-foreground">
              <Gauge weight="bold" className="size-5 text-muted-foreground" />
              Playback speed
            </span>
            <span className="flex items-center gap-1 text-sm text-muted-foreground">
              {label}
              <CaretRight weight="bold" className="size-4" />
            </span>
          </button>
        ) : (
          <div>
            <button
              type="button"
              onClick={() => setView("main")}
              className="flex w-full items-center gap-2 border-b border-panel-border px-4 py-3.5 text-sm font-semibold text-foreground transition-colors hover:bg-white/5"
            >
              <CaretLeft weight="bold" className="size-4" />
              Playback speed
            </button>
            <ScrollArea viewportClassName="max-h-72">
              <div className="py-1">
              {SPEED_OPTIONS.map((s) => {
                const active = s === speed;
                return (
                  <button
                    key={s}
                    type="button"
                    onClick={() => {
                      setSpeed(s);
                      setView("main");
                    }}
                    className={cn(
                      "flex w-full items-center gap-2.5 px-4 py-2.5 text-left text-sm transition-colors hover:bg-white/5",
                      active ? "font-semibold text-foreground" : "text-muted-foreground",
                    )}
                  >
                    <Check
                      weight="bold"
                      className={cn(
                        "size-4 text-primary-text",
                        active ? "opacity-100" : "opacity-0",
                      )}
                    />
                    {s === 1 ? "Normal" : `${s}×`}
                  </button>
                );
              })}
              </div>
            </ScrollArea>
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}

function EditableTitle({
  title,
  onCommit,
}: {
  title: string;
  onCommit: (title: string) => void;
}) {
  // `draft` doubles as the editing flag: null = not editing (render `title`
  // straight from the prop), a string = the working copy being edited. It's
  // seeded from `title` in the click handler, so no prop is copied into state on
  // mount and there's no re-sync effect that would flash a stale title.
  const [draft, setDraft] = React.useState<string | null>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const editing = draft !== null;

  React.useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  function commit() {
    const v = (draft ?? "").trim();
    setDraft(null);
    if (v && v !== title) onCommit(v);
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft ?? ""}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setDraft(null);
        }}
        className="w-full rounded-md border border-border bg-field px-2.5 py-1.5 text-lg font-semibold outline-none focus:border-ring"
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => setDraft(title)}
      className="group/title flex items-start gap-2 text-left"
    >
      <span className="text-lg font-semibold leading-tight">{title || "Untitled"}</span>
      <PencilSimple className="mt-1 size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/title:opacity-100" />
    </button>
  );
}

/**
 * Slim scrubber drawn on the video itself, shown only in fullscreen where the
 * filmstrip editor (the normal way to seek) isn't on screen. It maps across the
 * active selection — the trimmed range is the clip — so 0% is the in-point and
 * 100% the out-point.
 */
function OverlaySeekBar({
  videoRef,
  start,
  end,
  marks,
  onSeek,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  start: number;
  end: number;
  /** Event positions (absolute clip seconds); shown as icon markers. */
  marks: EventMark[];
  onSeek: (t: number) => void;
}) {
  const current = useVideoTime(videoRef);
  const ref = React.useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = React.useState(false);
  const span = Math.max(0.0001, end - start);
  const pct = Math.min(100, Math.max(0, ((current - start) / span) * 100));
  const pctOf = (t: number) => ((t - start) / span) * 100;

  function seek(clientX: number) {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    onSeek(start + frac * span);
  }

  return (
    <div
      ref={ref}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        setDragging(true);
        seek(e.clientX);
      }}
      onPointerMove={(e) => {
        if (dragging) seek(e.clientX);
      }}
      onPointerUp={(e) => {
        setDragging(false);
        try {
          e.currentTarget.releasePointerCapture(e.pointerId);
        } catch {
          /* not captured */
        }
      }}
      role="slider"
      aria-label="Seek"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      className="group/seek relative flex h-4 cursor-pointer touch-none items-center select-none"
    >
      <div className="h-1 w-full overflow-hidden rounded-full bg-white/25">
        <div className="h-full rounded-full bg-primary" style={{ width: `${pct}%` }} />
      </div>

      {/* Event markers — click to jump to the moment. */}
      {marks.map((m, i) => {
        const p = pctOf(m.at);
        if (p < -0.5 || p > 100.5) return null;
        const { Icon, tint } = eventIconFor(m.label);
        return (
          <button
            type="button"
            key={`${m.label}-${i}`}
            title={`${m.label} · ${fmtClock(m.at)}`}
            aria-label={`Jump to ${m.label}`}
            onPointerDown={(e) => {
              e.stopPropagation();
              onSeek(m.at);
            }}
            className="absolute top-1/2 z-10 flex size-4 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-black/75 ring-1 ring-white/40 transition-transform hover:scale-125"
            style={{ left: `${Math.min(100, Math.max(0, p))}%` }}
          >
            <Icon weight="fill" className={cn("size-2.5", tint)} />
          </button>
        );
      })}

      <span
        className="pointer-events-none absolute size-3 -translate-x-1/2 rounded-full bg-white opacity-0 shadow transition-opacity group-hover/seek:opacity-100"
        style={{ left: `${pct}%` }}
      />
    </div>
  );
}

/**
 * The right-hand details sidebar (title, spec line, event badges, match context,
 * file actions, delete). It depends only on `clip` + the action callbacks — none
 * of the player/editor state — so it's memoized: playing, scrubbing, mute,
 * speed, and trim edits no longer re-render it.
 */
const DetailsPanel = React.memo(function DetailsPanel({
  clip,
  onClose,
  onRename,
  onDelete,
}: {
  clip: ClipRecord;
  onClose: () => void;
  onRename: (title: string) => void;
  onDelete: () => void;
}) {
  const trimmed = clip.event != null;
  return (
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

        {/* One compact spec line — date, size, length, resolution */}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
          <span>{fmtDate(clip.created_unix_ms)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">{fmtSize(clip.size_bytes)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">{fmtTime(clip.duration_secs)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">
            {clip.width}×{clip.height}
          </span>
        </div>

        <div className="flex flex-wrap gap-2">
          {trimmed ? (
            // One badge per event the clip's window covered (a merged window
            // can hold several, e.g. a spike-defuse and a kill).
            (clip.events.length ? clip.events : [clip.event ?? ""]).map((ev, i) => (
              <span
                key={`${ev}-${i}`}
                className="inline-flex items-center gap-1.5 rounded-md bg-warning/15 px-2.5 py-1 text-xs font-medium text-warning"
              >
                <Scissors weight="fill" className="size-3.5" />
                {ev}
              </span>
            ))
          ) : (
            <span className="inline-flex items-center gap-1.5 rounded-md bg-info/15 px-2.5 py-1 text-xs font-medium text-info">
              <Lightning weight="fill" className="size-3.5" />
              Auto Clip
            </span>
          )}
        </div>

        {/* Valorant match context — silent for clips cut outside a match */}
        <ClipGameContext clip={clip} />

        <div className="flex flex-col gap-2">
          <span className="text-[11px] font-semibold tracking-wide text-muted-foreground/70 uppercase">
            File
          </span>
          <button
            type="button"
            onClick={() => {
              void revealClip(clip.id).catch(() => {});
            }}
            className="flex items-center justify-center gap-2 rounded-lg border border-border/60 bg-card/40 px-4 py-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
          >
            <FolderOpen className="size-4" />
            Open in folder
          </button>
          <CopyPath path={clip.path} />
        </div>

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
  );
});

/**
 * Valorant match context for the open clip — agent, map, mode, result and
 * K/D/A. Renders nothing for clips cut outside a match (all fields null), so the
 * details panel stays clean for non-match clips. Mirrors the card badges'
 * artwork via the shared asset lookups.
 */
function ClipGameContext({ clip }: { clip: ClipRecord }) {
  const assets = useValorantAssets();
  const agent = assets.agentFor(clip);
  const agentName = agent?.name ?? clip.agent ?? null;
  const mapName = assets.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map);
  const hasResult = clip.won != null;
  const hasKda =
    clip.kills != null && clip.deaths != null && clip.assists != null;

  if (!agentName && !mapName && !clip.mode && !hasResult) return null;

  const sub = [mapName, clip.mode].filter(Boolean).join(" · ");

  return (
    <div className="flex flex-col gap-2">
      <span className="text-[11px] font-semibold tracking-wide text-muted-foreground/70 uppercase">
        Match
      </span>
      <div className="flex flex-col gap-3 rounded-lg border border-border/60 bg-card/40 p-3.5">
        <div className="flex items-center gap-3">
          {agent?.icon ? (
            <img
              src={agent.icon}
              alt=""
              className="size-10 shrink-0 rounded-md bg-black/30 object-cover"
            />
          ) : null}
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold text-foreground">
              {agentName ?? "Unknown agent"}
            </div>
            {sub ? (
              <div className="truncate text-xs text-muted-foreground">{sub}</div>
            ) : null}
          </div>
          {hasResult ? (
            <span
              className={cn(
                "rounded-md px-2 py-0.5 text-[11px] font-bold text-white",
                clip.won ? "bg-success/80" : "bg-destructive/80",
              )}
            >
              {clip.won ? "WIN" : "LOSS"}
            </span>
          ) : null}
        </div>

        {hasKda ? (
          <div className="flex items-center gap-2 border-t border-border/50 pt-3 text-xs text-muted-foreground">
            <span className="font-mono tabular-nums">
              <span className="font-semibold text-foreground">{clip.kills}</span>
              {" / "}
              <span className="font-semibold text-foreground">{clip.deaths}</span>
              {" / "}
              <span className="font-semibold text-foreground">{clip.assists}</span>
            </span>
            <span className="text-muted-foreground/70">KDA</span>
            {clip.headshot_pct != null ? (
              <span className="ml-auto">
                <span className="font-mono font-semibold tabular-nums text-foreground">
                  {Math.round(clip.headshot_pct)}%
                </span>{" "}
                <span className="text-muted-foreground/70">HS</span>
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
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
