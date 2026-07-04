import { CloudArrowUp, CloudCheck, WarningCircle } from "@phosphor-icons/react";

import { Spinner } from "@/components/ui/spinner";

import { cn } from "@/lib/utils";
import { useClipUpload } from "@/hooks/use-cloud";

/**
 * Cloud-upload status as a single icon chip, sized to sit inline in the clip
 * card's bottom-left badge row (next to the game pills) rather than overlapping
 * them. The icon + tint carry the state — queued/uploading (neutral), done
 * (green), failed (red) — so it stays compact; the full status text lives in the
 * viewer. Renders nothing when the clip was never uploaded (or was canceled).
 */
export function ClipUploadBadge({ clipId }: { clipId: number }) {
  const upload = useClipUpload(clipId);
  if (!upload || upload.status === "canceled") return null;

  const chip = "flex size-[18px] items-center justify-center rounded-full text-white";

  switch (upload.status) {
    case "queued":
      return (
        <span className={cn(chip, "bg-black/70")} title="Upload queued">
          <CloudArrowUp weight="fill" className="size-3" />
        </span>
      );
    case "uploading":
      return (
        <span className={cn(chip, "bg-black/70")} title="Uploading…">
          <Spinner weight="bold" className="size-3" />
        </span>
      );
    case "done":
      return (
        <span className={cn(chip, "bg-success/90")} title="Uploaded">
          <CloudCheck weight="fill" className="size-3" />
        </span>
      );
    case "error":
      return (
        <span className={cn(chip, "bg-destructive/90")} title="Upload failed">
          <WarningCircle weight="fill" className="size-3" />
        </span>
      );
    default:
      return null;
  }
}
