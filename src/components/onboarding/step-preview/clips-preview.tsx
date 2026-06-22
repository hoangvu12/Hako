import { Check } from "@phosphor-icons/react";

import type { Settings } from "@/lib/api";
import { Thumb, DurationBadge, SAMPLE_CLIPS } from "./shared";

/** Clips — the real "Clip saved" toast (upload-toast.tsx style) + a keycap. */
export function ClipsPreview({ draft }: { draft: Settings }) {
  const clip = SAMPLE_CLIPS[0];
  const dur = `${Math.floor(draft.clip_seconds / 60)}:${String(
    draft.clip_seconds % 60
  ).padStart(2, "0")}`;
  return (
    <div className="flex w-full flex-col items-center gap-6">
      {/* The keycap presses + glows on a loop… */}
      <div className="hako-key flex min-w-16 items-center justify-center rounded-xl border-2 border-border bg-secondary px-6 py-3 text-2xl font-bold tracking-tight">
        {draft.save_hotkey}
      </div>
      <p className="-mt-3 text-xs text-muted-foreground">
        Press to save the last {draft.clip_seconds}s
      </p>

      {/* …and a clip "pops" into the library on each press. */}
      <div className="hako-clip-pop w-full max-w-[260px]">
        <div className="overflow-hidden rounded-xl border border-border/60 bg-card shadow-lg">
          <Thumb src={clip.img}>
            <span className="absolute top-2 left-2 flex items-center gap-1.5 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
              <Check weight="bold" className="size-3 text-emerald-400" />
              Clip saved
            </span>
            <DurationBadge>{dur}</DurationBadge>
          </Thumb>
          <div className="flex items-center gap-2 p-2.5">
            <Check weight="fill" className="size-3.5 shrink-0 text-success" />
            <span className="text-xs font-medium">Saved to your library</span>
          </div>
        </div>
      </div>
    </div>
  );
}
