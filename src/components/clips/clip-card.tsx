import * as React from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Link } from "@tanstack/react-router";
import {
  Play,
  Pause,
  DotsThree,
  PencilSimple,
  Trash,
  SpeakerSimpleHigh,
  SpeakerSimpleX,
  CornersOut,
  CornersIn,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ClipRecord } from "@/lib/api";
import {
  mapNameFromPath,
  type ValorantAssets,
} from "@/hooks/use-valorant-assets";

function fmtDuration(secs: number): string {
  const s = Math.round(secs);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function fmtTime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

function timeAgo(unixMs: number): string {
  const diff = Date.now() - unixMs;
  const mins = Math.round(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours} hour${hours > 1 ? "s" : ""} ago`;
  const days = Math.round(hours / 24);
  return `${days} day${days > 1 ? "s" : ""} ago`;
}

function Dot() {
  return <span className="size-[3px] shrink-0 rounded-full bg-secondary" />;
}

/**
 * Thumbnail that autoplays (muted) on hover, with mute + fullscreen affordances.
 * In fullscreen we render our own clean controls bar instead of the native one.
 */
function ClipPreview({ clip }: { clip: ClipRecord }) {
  const containerRef = React.useRef<HTMLDivElement>(null);
  const videoRef = React.useRef<HTMLVideoElement>(null);
  const barRef = React.useRef<HTMLDivElement>(null);

  const [active, setActive] = React.useState(false); // hovered → video mounted
  const [muted, setMuted] = React.useState(true);
  const [playing, setPlaying] = React.useState(false);
  const [fullscreen, setFullscreen] = React.useState(false);
  const [current, setCurrent] = React.useState(0);
  // The clip's stored duration is the render-time fallback; once the <video>
  // reports its real duration we keep that (genuinely new data), rather than
  // copying the prop into state where it would go stale if `clip` changed.
  const [videoDuration, setVideoDuration] = React.useState<number | null>(null);
  const duration = videoDuration ?? clip.duration_secs;
  const [scrubbing, setScrubbing] = React.useState(false);

  // Keep the actual element's `muted` in sync with state (React doesn't reflect it as an attribute reliably).
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted;
  }, [muted, active]);

  // Track fullscreen state across Esc / native exit.
  React.useEffect(() => {
    function onChange() {
      const fs = document.fullscreenElement === containerRef.current;
      setFullscreen(fs);
      const v = videoRef.current;
      if (!v) return;
      if (fs) {
        void v.play().catch(() => {});
        return;
      }
      // Exited fullscreen: keep playing only if the cursor is still over the card.
      const hovered = containerRef.current?.matches(":hover") ?? false;
      setActive(hovered);
      if (!hovered) {
        v.pause();
        v.currentTime = 0;
      }
    }
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  function handleEnter() {
    setActive(true);
    // play() runs once the element mounts (see autoPlay-on-mount below)
  }
  function handleLeave() {
    if (fullscreen) return;
    setActive(false);
    setPlaying(false);
  }

  function togglePlay(e?: React.MouseEvent) {
    e?.preventDefault();
    e?.stopPropagation();
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play().catch(() => {});
    else v.pause();
  }

  function toggleMute(e: React.MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMuted((m) => !m);
  }

  async function toggleFullscreen(e: React.MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await containerRef.current?.requestFullscreen();
    } catch {
      /* fullscreen unavailable */
    }
  }

  function seekFromEvent(clientX: number) {
    const bar = barRef.current;
    const v = videoRef.current;
    if (!bar || !v || !Number.isFinite(duration) || duration <= 0) return;
    const rect = bar.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    v.currentTime = frac * duration;
    setCurrent(v.currentTime);
  }

  const progress = duration > 0 ? (current / duration) * 100 : 0;
  const showVideo = active || fullscreen;
  const poster = clip.thumb_path ? convertFileSrc(clip.thumb_path) : undefined;

  return (
    <div
      ref={containerRef}
      onMouseEnter={handleEnter}
      onMouseLeave={handleLeave}
      className={cn(
        "group/media relative block aspect-video overflow-hidden bg-muted",
        fullscreen && "flex aspect-auto h-full items-center justify-center bg-black",
      )}
    >
      {/* Poster image (shown until hover mounts the video) */}
      {poster && !showVideo ? (
        <img
          src={poster}
          alt={clip.title}
          className="size-full object-cover opacity-90 transition-all duration-300 group-hover:scale-[1.02] group-hover:opacity-100"
          onError={(e) => {
            (e.currentTarget as HTMLImageElement).style.display = "none";
          }}
        />
      ) : null}

      {showVideo ? (
        <video
          ref={videoRef}
          src={convertFileSrc(clip.path)}
          poster={poster}
          loop={!fullscreen}
          playsInline
          autoPlay
          preload="metadata"
          className={cn(
            "size-full bg-black",
            fullscreen ? "object-contain" : "object-cover",
          )}
          onLoadedMetadata={(e) => {
            const d = e.currentTarget.duration;
            if (Number.isFinite(d) && d > 0) setVideoDuration(d);
          }}
          onTimeUpdate={(e) => {
            if (!scrubbing) setCurrent(e.currentTarget.currentTime);
          }}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onClick={fullscreen ? togglePlay : undefined}
        />
      ) : null}

      <span className="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/50 to-transparent opacity-60" />

      {/* Click target → open detail (disabled while fullscreen) */}
      {!fullscreen ? (
        <Link
          to="/clips/$clipId"
          params={{ clipId: String(clip.id) }}
          aria-label={`Open ${clip.title || "clip"}`}
          className="absolute inset-0 z-10"
        />
      ) : null}

      {/* Hover play hint (only before the video starts) */}
      {!fullscreen && !playing ? (
        <span className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center opacity-0 transition-opacity group-hover/media:opacity-100">
          <span className="flex size-11 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
            <Play weight="fill" className="size-5 text-white" />
          </span>
        </span>
      ) : null}

      {/* Mute / fullscreen affordances (hover, hidden during fullscreen bar) */}
      {!fullscreen ? (
        <div className="absolute inset-x-2 bottom-2 z-20 flex items-end justify-between opacity-0 transition-opacity group-hover/media:opacity-100">
          <button
            type="button"
            onClick={toggleMute}
            aria-label={muted ? "Unmute" : "Mute"}
            className="flex size-9 items-center justify-center rounded-full bg-black/55 text-white backdrop-blur-sm transition-colors hover:bg-black/75"
          >
            {muted ? (
              <SpeakerSimpleX weight="fill" className="size-4" />
            ) : (
              <SpeakerSimpleHigh weight="fill" className="size-4" />
            )}
          </button>
          <button
            type="button"
            onClick={toggleFullscreen}
            aria-label="Fullscreen"
            className="flex size-9 items-center justify-center rounded-full bg-black/55 text-white backdrop-blur-sm transition-colors hover:bg-black/75"
          >
            <CornersOut weight="bold" className="size-4" />
          </button>
        </div>
      ) : null}

      {/* Duration badge (hidden in fullscreen — the bar shows time) */}
      {!fullscreen ? (
        <span className="pointer-events-none absolute right-2 bottom-2 z-10 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white backdrop-blur-sm transition-opacity group-hover/media:opacity-0">
          {fmtDuration(clip.duration_secs)}
        </span>
      ) : null}

      {/* Clean fullscreen controls bar */}
      {fullscreen ? (
        <div className="absolute inset-x-0 bottom-0 z-30 flex flex-col gap-3 bg-gradient-to-t from-black/80 via-black/40 to-transparent px-6 pt-12 pb-5 text-white">
          {/* Seek bar */}
          <div
            ref={barRef}
            onPointerDown={(e) => {
              (e.target as HTMLElement).setPointerCapture(e.pointerId);
              setScrubbing(true);
              seekFromEvent(e.clientX);
            }}
            onPointerMove={(e) => {
              if (scrubbing) seekFromEvent(e.clientX);
            }}
            onPointerUp={() => setScrubbing(false)}
            className="group/bar relative h-4 cursor-pointer touch-none"
          >
            <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 overflow-hidden rounded-full bg-white/25">
              <div
                className="h-full rounded-full bg-primary"
                style={{ width: `${progress}%` }}
              />
            </div>
            <div
              className="absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-primary opacity-0 shadow transition-opacity group-hover/bar:opacity-100"
              style={{ left: `${progress}%` }}
            />
          </div>

          {/* Buttons row */}
          <div className="flex items-center gap-4">
            <button
              type="button"
              onClick={togglePlay}
              aria-label={playing ? "Pause" : "Play"}
              className="text-white transition-opacity hover:opacity-80"
            >
              {playing ? (
                <Pause weight="fill" className="size-6" />
              ) : (
                <Play weight="fill" className="size-6" />
              )}
            </button>
            <button
              type="button"
              onClick={toggleMute}
              aria-label={muted ? "Unmute" : "Mute"}
              className="text-white transition-opacity hover:opacity-80"
            >
              {muted ? (
                <SpeakerSimpleX weight="fill" className="size-5" />
              ) : (
                <SpeakerSimpleHigh weight="fill" className="size-5" />
              )}
            </button>
            <span className="font-mono text-xs tabular-nums text-white/80">
              {fmtTime(current)} / {fmtTime(duration)}
            </span>
            <span className="flex-1" />
            <button
              type="button"
              onClick={toggleFullscreen}
              aria-label="Exit fullscreen"
              className="text-white transition-opacity hover:opacity-80"
            >
              <CornersIn weight="bold" className="size-5" />
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

/**
 * Game-context overlay on the thumbnail: agent portrait + name, map, and a W/L
 * pill — whatever the clip carries. Pointer-events-none so it never blocks the
 * card's click target; degrades to text (or nothing) when artwork/fields are
 * absent (old clips, manual saves outside a match).
 */
function ClipBadges({
  clip,
  assets,
}: {
  clip: ClipRecord;
  assets?: ValorantAssets;
}) {
  const agent = assets?.agentFor(clip);
  const mapName = assets?.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map);
  const agentName = agent?.name ?? clip.agent ?? null;
  const hasResult = clip.won != null;

  if (!agentName && !mapName && !hasResult) return null;

  return (
    <div className="pointer-events-none absolute inset-x-2 top-2 z-20 flex items-start justify-between gap-2">
      <div className="flex max-w-[80%] flex-wrap items-center gap-1.5">
        {agentName ? (
          <span className="flex items-center gap-1 rounded-full bg-black/55 py-0.5 pr-2 pl-0.5 text-[10px] font-semibold text-white backdrop-blur-sm">
            {agent?.icon ? (
              <img
                src={agent.icon}
                alt=""
                className="size-4 rounded-full object-cover"
              />
            ) : (
              <span className="size-4" />
            )}
            {agentName}
          </span>
        ) : null}
        {mapName ? (
          <span className="rounded-full bg-black/55 px-2 py-0.5 text-[10px] font-medium text-white backdrop-blur-sm">
            {mapName}
          </span>
        ) : null}
      </div>
      {hasResult ? (
        <span
          className={cn(
            "rounded-full px-2 py-0.5 text-[10px] font-bold text-white backdrop-blur-sm",
            clip.won ? "bg-success/80" : "bg-destructive/80"
          )}
        >
          {clip.won ? "WIN" : "LOSS"}
        </span>
      ) : null}
    </div>
  );
}

export function ClipCard({
  clip,
  onDelete,
  onRename,
  assets,
}: {
  clip: ClipRecord;
  onDelete: () => void;
  onRename: () => void;
  assets?: ValorantAssets;
}) {
  // Every event the clip's window covered, falling back to the headline tag for
  // clips saved before multi-event tracking (mirrors the detail panel).
  const eventLabels = clip.events.length
    ? clip.events
    : clip.event
      ? [clip.event]
      : [];

  return (
    <div className="group flex flex-col overflow-hidden rounded-xl border border-border/60 bg-card shadow-sm transition-colors hover:border-border">
      {/* Thumbnail / hover-preview, with the game-context overlay on top */}
      <div className="relative">
        <ClipPreview clip={clip} />
        <ClipBadges clip={clip} assets={assets} />
      </div>

      {/* Meta */}
      <div className="flex flex-1 flex-col gap-1.5 p-3.5">
        <div className="flex items-center justify-between gap-2">
          <h3
            className="truncate text-sm font-medium text-card-foreground"
            title={clip.title}
          >
            {clip.title || "Untitled"}
          </h3>

          <DropdownMenu>
            <DropdownMenuTrigger
              aria-label="Clip actions"
              className="-mr-1 flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-[color,opacity] outline-none hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
            >
              <DotsThree weight="bold" className="size-4" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={onRename}>
                <PencilSimple />
                Rename
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={onDelete}>
                <Trash />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* One quiet metadata line: the event(s) lead (slightly emphasized,
            truncating when many), then when / how big. No chips or per-item
            icons — the thumbnail badges already carry the visual weight. */}
        <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
          {eventLabels.length ? (
            <>
              <span
                className="min-w-0 truncate text-foreground/80"
                title={eventLabels.join(", ")}
              >
                {eventLabels.join(", ")}
              </span>
              <Dot />
            </>
          ) : null}
          <span className="shrink-0">{timeAgo(clip.created_unix_ms)}</span>
          <Dot />
          <span className="shrink-0">{fmtSize(clip.size_bytes)}</span>
        </div>
      </div>
    </div>
  );
}
