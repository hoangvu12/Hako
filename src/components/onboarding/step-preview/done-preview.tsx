import type { Settings } from "@/lib/api";
import { Surface, ClipCardMini, SAMPLE_CLIPS } from "./shared";

/** Done — a mini library dashboard, ready to go. */
export function DonePreview({ draft }: { draft: Settings }) {
  return (
    <Surface>
      <div className="flex items-center justify-between border-b border-border/60 px-3 py-2">
        <span className="text-sm font-semibold">Your clips</span>
        <span className="rounded-full border border-border/60 bg-secondary px-2 py-0.5 text-[10px] font-medium">
          Ready
        </span>
      </div>
      <div className="grid grid-cols-2 gap-2.5 p-3">
        {SAMPLE_CLIPS.slice(0, 2).map((c) => (
          <ClipCardMini key={c.title} clip={c} />
        ))}
      </div>
      <div className="flex items-center gap-2 border-t border-border/60 px-3 py-2 text-xs text-muted-foreground">
        <span className="size-1.5 animate-pulse rounded-full bg-red-500" />
        Recording armed · {draft.save_hotkey} to clip
      </div>
    </Surface>
  );
}
