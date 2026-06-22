import * as React from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Link } from "@tanstack/react-router";
import {
  Play,
  Pause,
  SpeakerSimpleHigh,
  SpeakerSimpleX,
  CornersOut,
  CornersIn,
  CloudArrowUp,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { useClipRemoteUrl } from "@/hooks/use-cloud";
import type { ClipRecord } from "@/lib/api";
import { fmtDuration, fmtTime } from "./format";

// Dwell time before a hovered card mounts + autoplays its preview video.
// Scrolling sweeps the cursor across many cards; without a dwell gate each
// mouseenter would spin up a <video> decoder, and a burst of those tanks FPS. A
// quick scroll-over never lingers this long, so it triggers nothing.
const HOVER_PLAY_DELAY_MS = 180;

/**
 * Thumbnail that autoplays (muted) on hover, with mute + fullscreen affordances.
 * In fullscreen we render our own clean controls bar instead of the native one.
 */
export function ClipPreview({ clip }: { clip: ClipRecord }) {
  const containerRef = React.useRef<HTMLDivElement>(null);
  const videoRef = React.useRef<HTMLVideoElement>(null);
  const barRef = React.useRef<HTMLDivElement>(null);
  const hoverTimer = React.useRef<number | null>(null);

  const [active, setActive] = React.useState(false); // hovered → video mounted
  // Cursor over the card → mount the hover controls. Kept separate from `active`
  // (which gates the heavier <video>): the controls appear instantly, while the
  // video still waits out the dwell. Mounting them only on hover keeps their
  // Phosphor icon trees off every card's scroll-mount path.
  const [hovered, setHovered] = React.useState(false);
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
    setHovered(true);
    // Arm the dwell timer; the video only mounts (and autoplays) if the cursor
    // is still here after HOVER_PLAY_DELAY_MS — so scrolling past does nothing.
    if (hoverTimer.current != null) window.clearTimeout(hoverTimer.current);
    hoverTimer.current = window.setTimeout(() => {
      hoverTimer.current = null;
      setActive(true);
    }, HOVER_PLAY_DELAY_MS);
  }
  function handleLeave() {
    setHovered(false);
    if (hoverTimer.current != null) {
      window.clearTimeout(hoverTimer.current);
      hoverTimer.current = null;
    }
    if (fullscreen) return;
    setActive(false);
    setPlaying(false);
  }

  // Don't leave a pending dwell timer behind when the card unmounts (e.g. the
  // virtualizer recycles it as you scroll).
  React.useEffect(() => {
    return () => {
      if (hoverTimer.current != null) window.clearTimeout(hoverTimer.current);
    };
  }, []);

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

  // Cloud-only (evicted) clips have no local file: play from the presigned URL
  // and skip the (now-deleted) local thumbnail.
  const cloudUrl = useClipRemoteUrl(clip.id);
  const videoSrc = clip.evicted ? cloudUrl : convertFileSrc(clip.path);

  const progress = duration > 0 ? (current / duration) * 100 : 0;
  const showVideo = (active || fullscreen) && !!videoSrc;
  // Evicted (cloud-only) clips keep their thumbnail on disk — retention deletes
  // only the video — so still render the poster; only fall back to the cloud
  // placeholder when there's genuinely no thumbnail (e.g. an old eviction).
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
          decoding="async"
          draggable={false}
          className="size-full object-cover opacity-90 outline outline-1 -outline-offset-1 outline-white/10 transition-opacity duration-300 group-hover:opacity-100"
          onError={(e) => {
            (e.currentTarget as HTMLImageElement).style.display = "none";
          }}
        />
      ) : null}

      {/* Cloud-only placeholder, only when there's no thumbnail to show and the
          video isn't mounted (hover plays it from the cloud). */}
      {clip.evicted && !showVideo && !poster ? (
        <div className="flex size-full items-center justify-center bg-muted text-muted-foreground">
          <CloudArrowUp weight="duotone" className="size-8 opacity-60" />
        </div>
      ) : null}

      {showVideo ? (
        <video
          ref={videoRef}
          src={videoSrc ?? undefined}
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

      {/* Hover play hint (only before the video starts) — mounted only while
          hovered so its icon stays off the scroll-mount path. */}
      {hovered && !fullscreen && !playing ? (
        <span className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center opacity-0 transition-opacity group-hover/media:opacity-100">
          <span className="flex size-11 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
            <Play weight="fill" className="size-5 text-white" />
          </span>
        </span>
      ) : null}

      {/* Mute / fullscreen affordances (hover, hidden during fullscreen bar) —
          mounted only while hovered to keep their icons off the scroll path. */}
      {hovered && !fullscreen ? (
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
        <span className="pointer-events-none absolute right-2 bottom-2 z-10 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white transition-opacity group-hover/media:opacity-0">
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
