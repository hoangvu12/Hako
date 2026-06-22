import { FolderOpen } from "@phosphor-icons/react";

import type { Settings } from "@/lib/api";
import { Surface, ClipCardMini, SAMPLE_CLIPS } from "./shared";

/** Storage — a library grid headed by the live folder path. */
export function StoragePreview({ draft }: { draft: Settings }) {
  return (
    <Surface>
      <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2 text-xs text-muted-foreground">
        <FolderOpen weight="fill" className="size-4 shrink-0" />
        <span className="truncate">{draft.storage_dir || "Videos/Hako"}</span>
      </div>
      <div className="grid grid-cols-2 gap-2.5 p-3">
        {SAMPLE_CLIPS.map((c) => (
          <ClipCardMini key={c.title} clip={c} />
        ))}
      </div>
    </Surface>
  );
}
