import {
  CloudArrowUp,
  CloudCheck,
  CircleNotch,
  WarningCircle,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { useClipUpload } from "@/hooks/use-cloud";
import { pctOf } from "./cloud-format";

/**
 * Compact cloud-upload status pill overlaid on a clip thumbnail (bottom-left).
 * Renders nothing when the clip was never uploaded (or its upload was canceled),
 * and fades out on hover so it never fights the preview's controls. Mirrors
 * Medal's inline per-clip status.
 */
export function ClipUploadBadge({ clipId }: { clipId: number }) {
  const upload = useClipUpload(clipId);
  if (!upload || upload.status === "canceled") return null;

  const base =
    "pointer-events-none absolute bottom-2 left-2 z-10 flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-semibold text-white transition-opacity group-hover/media:opacity-0";

  switch (upload.status) {
    case "queued":
      return (
        <span className={cn(base, "bg-black/80")}>
          <CloudArrowUp weight="fill" className="size-3" />
          Queued
        </span>
      );
    case "uploading":
      return (
        <span className={cn(base, "bg-black/80")}>
          <CircleNotch weight="bold" className="size-3 animate-spin" />
          {pctOf(upload.sent, upload.total)}%
        </span>
      );
    case "done":
      return (
        <span className={cn(base, "bg-success/90")}>
          <CloudCheck weight="fill" className="size-3" />
          Uploaded
        </span>
      );
    case "error":
      return (
        <span className={cn(base, "bg-destructive/90")}>
          <WarningCircle weight="fill" className="size-3" />
          Failed
        </span>
      );
    default:
      return null;
  }
}
