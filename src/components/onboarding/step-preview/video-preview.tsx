import { Play } from "@phosphor-icons/react";

import type { Settings } from "@/lib/api";
import { Surface, Thumb, WinPill, DurationBadge, Dot, SAMPLE_CLIPS } from "./shared";

// Roughly how the chosen quality would actually look: lower resolution → softer,
// low bitrate → a little flatter. Native / 1080p+ stay crisp.
const RES_BLUR: Record<string, number> = {
  "360p": 3,
  "480p": 1.8,
  "720p": 0.8,
  "1080p": 0,
  "1440p": 0,
  "2160p": 0,
};

/** Video — a clip card whose thumbnail visibly degrades to match the quality. */
export function VideoPreview({ draft }: { draft: Settings }) {
  const resLabel = draft.resolution === "native" ? "Native" : draft.resolution;
  const clip = SAMPLE_CLIPS[0];
  const blur = RES_BLUR[draft.resolution] ?? 0; // native + unknown → crisp
  const lowBitrate = draft.bitrate_mbps <= 5;
  const imgStyle: React.CSSProperties =
    blur || lowBitrate
      ? {
          filter: [
            blur ? `blur(${blur}px)` : "",
            lowBitrate ? "contrast(0.92) saturate(0.85)" : "",
          ]
            .filter(Boolean)
            .join(" "),
          // Scale up slightly so the blur doesn't reveal the muted edges.
          transform: blur ? "scale(1.06)" : undefined,
        }
      : {};
  return (
    <Surface>
      <Thumb src={clip.img} imgStyle={imgStyle}>
        <span className="absolute top-2 left-2 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
          {resLabel} · {draft.target_fps}fps
        </span>
        <WinPill won={clip.won} />
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <span className="flex size-11 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
            <Play weight="fill" className="size-5 text-white" />
          </span>
        </div>
        <DurationBadge>{clip.dur}</DurationBadge>
      </Thumb>
      <div className="flex items-center justify-between gap-2 p-3">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold">{clip.title}</h3>
          <div className="mt-0.5 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
            <span>{clip.meta}</span>
            <Dot />
            <span>{draft.bitrate_mbps} Mbps {draft.codec.toUpperCase()}</span>
          </div>
        </div>
      </div>
    </Surface>
  );
}
