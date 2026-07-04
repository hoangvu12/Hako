import { useEffect, useRef, useState } from "react";
import { CloudArrowUp, CloudCheck, X } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { useActiveUploads, useCancelUpload } from "@/hooks/use-cloud";
import { useClips } from "@/hooks/use-library";
import { fmtRate, pctOf } from "./cloud-format";
import { formatBytes } from "@/lib/format";

/** Linger after the last upload finishes so "complete" is actually seen. */
const COMPLETE_LINGER_MS = 4000;

/**
 * Background-first upload UX (Medal-style): a non-blocking bottom-right toast
 * that appears while clips are uploading and shows the active clip's progress,
 * throughput, and a "+N" badge for the rest of the queue. When the queue drains
 * it flashes a brief "Upload complete" before dismissing itself.
 */
export function UploadToast() {
  const active = useActiveUploads();
  const { data: clips } = useClips();
  const cancel = useCancelUpload();

  // Edge-detect the queue draining to 0 so we can flash a completion state.
  // Depends only on `active.length` (never on `done`): it always records the
  // previous count, so once it's seen the drain it won't re-fire. Letting `done`
  // drive this effect — together with the early return skipping the ref update —
  // is what made the "complete" toast re-flash forever every linger cycle.
  const [done, setDone] = useState(false);
  const prevActive = useRef(0);
  useEffect(() => {
    const prev = prevActive.current;
    prevActive.current = active.length;
    if (prev > 0 && active.length === 0) setDone(true); // queue just drained
    else if (active.length > 0) setDone(false); // new activity → drop the flash
  }, [active.length]);

  // Auto-dismiss the completion flash after the linger. Separate from the edge
  // detector so its `done` transitions can't retrigger the flash.
  useEffect(() => {
    if (!done) return;
    const t = window.setTimeout(() => setDone(false), COMPLETE_LINGER_MS);
    return () => window.clearTimeout(t);
  }, [done]);

  if (active.length === 0 && !done) return null;

  // The clip the toast headlines: the one actually streaming, else the next up.
  const current = active[0];
  const title = current
    ? clips?.find((c) => c.id === current.clipId)?.title || "Clip"
    : null;
  const queued = Math.max(0, active.length - 1);

  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-auto fixed right-4 bottom-4 z-50 w-72 rounded-xl border border-border/70 bg-card/95 p-3 shadow-lg backdrop-blur animate-in fade-in slide-in-from-bottom-2"
    >
      {current ? (
        <>
          <div className="flex items-center gap-2">
            <CloudArrowUp
              weight="fill"
              className="size-4 shrink-0 text-primary-text"
            />
            <span className="min-w-0 flex-1 truncate text-sm font-medium">
              {title}
            </span>
            {queued > 0 && (
              <span className="shrink-0 rounded-full bg-secondary px-1.5 py-0.5 text-[10px] font-semibold text-secondary-foreground">
                +{queued}
              </span>
            )}
            {(current.status === "uploading" || current.status === "queued") && (
              <button
                type="button"
                aria-label="Cancel upload"
                onClick={() => cancel.mutate(current.clipId)}
                className="-mr-0.5 flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground"
              >
                <X weight="bold" className="size-3.5" />
              </button>
            )}
          </div>

          <ProgressBar
            pct={pctOf(current.sent, current.total)}
            indeterminate={current.status === "queued"}
          />

          <div className="mt-1.5 flex items-center justify-between text-[11px] tabular-nums text-muted-foreground">
            <span>
              {current.status === "queued"
                ? "Queued…"
                : `${formatBytes(current.sent)} / ${formatBytes(current.total)}`}
            </span>
            <span>
              {current.status === "uploading"
                ? fmtRate(current.bytesPerSec)
                : ""}
            </span>
          </div>
        </>
      ) : (
        <div className="flex items-center gap-2">
          <CloudCheck weight="fill" className="size-4 shrink-0 text-success" />
          <span className="text-sm font-medium">Upload complete</span>
        </div>
      )}
    </div>
  );
}

function ProgressBar({
  pct,
  indeterminate,
}: {
  pct: number;
  indeterminate: boolean;
}) {
  return (
    <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-secondary">
      <div
        className={cn(
          "h-full rounded-full bg-primary transition-[width] duration-300",
          indeterminate && "w-1/3 animate-pulse",
        )}
        style={indeterminate ? undefined : { width: `${pct}%` }}
      />
    </div>
  );
}
