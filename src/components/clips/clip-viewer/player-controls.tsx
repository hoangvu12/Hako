import * as React from "react";
import {
  CaretLeft,
  CaretRight,
  Play,
  Pause,
  Check,
  Gauge,
  GearSix,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import type { EventMark } from "@/lib/api";
import { SPEED_OPTIONS } from "./constants";
import { eventIconFor, fmtClock } from "./format";
import { formatTime } from "@/lib/format";
import { useVideoTime } from "./use-video-time";

/** Live `0:03 / 0:12` readout — isolated so it, not the player, ticks. */
export function TimeReadout({
  videoRef,
  duration,
}: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  duration: number;
}) {
  const current = useVideoTime(videoRef);
  return (
    <span className="font-mono text-xs tabular-nums text-white/85">
      {formatTime(current)} / {formatTime(duration)}
    </span>
  );
}

/** The draggable filmstrip playhead — positioned from the live playback time. */
export function Playhead({
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
      aria-valuetext={formatTime(current)}
      className="pointer-events-auto absolute -top-2 -bottom-2 z-40 flex w-5 -translate-x-1/2 cursor-ew-resize justify-center touch-none"
      style={{ left: `${progress}%` }}
    >
      <span className="h-full w-0.5 bg-primary shadow-[0_0_8px] shadow-primary/50" />
      <span className="absolute -top-1 left-1/2 h-3.5 w-3 -translate-x-1/2 rounded-sm bg-primary shadow" />
    </div>
  );
}

export function NavArrow({ side, onClick }: { side: "left" | "right"; onClick: () => void }) {
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

export function CtrlButton({
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
export function PlayPauseButton({
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
export function SettingsButton({
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

/**
 * Slim scrubber drawn on the video itself, shown only in fullscreen where the
 * filmstrip editor (the normal way to seek) isn't on screen. It maps across the
 * active selection — the trimmed range is the clip — so 0% is the in-point and
 * 100% the out-point.
 */
export function OverlaySeekBar({
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
