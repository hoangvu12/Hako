import * as React from "react";

import { cn } from "@/lib/utils";
import { FILMSTRIP_TILES, FILMSTRIP_VISIBLE } from "./constants";

/**
 * The scrubber's frame strip. Renders `FILMSTRIP_TILES` slots, each showing one
 * tile of the Rust-generated sprite sheet via `background-position` — so there's
 * no second `<video>` decoding in the webview (which used to contend with
 * playback for the hardware decoder). Memoized: playhead ticks don't touch it.
 * Falls back to a repeated poster for clips saved before filmstrips existed.
 */
export const FilmstripStrip = React.memo(function FilmstripStrip({
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

export function TrimHandle({
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
