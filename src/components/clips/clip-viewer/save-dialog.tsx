import { Copy, FloppyDisk } from "@phosphor-icons/react";

import { Spinner } from "@/components/ui/spinner";

import type { TrimMode } from "@/lib/api";
import { fmtClock } from "./format";

export function SaveDialog({
  title,
  selDuration,
  audioSummary,
  pending,
  error,
  onCancel,
  onChoose,
}: {
  title: string;
  selDuration: number;
  audioSummary: string;
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onChoose: (mode: TrimMode) => void;
}) {
  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center">
      <button
        type="button"
        aria-label="Cancel"
        onClick={pending ? undefined : onCancel}
        className="absolute inset-0 cursor-default bg-black/60 backdrop-blur-sm"
      />
      <div className="relative z-10 w-[380px] rounded-2xl border border-border bg-popover p-6 shadow-2xl">
        <h3 className="text-base font-semibold">Save trim</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          {fmtClock(selDuration)} selected · {audioSummary}. Choose how to save "
          {title || "Untitled"}".
        </p>

        {error ? (
          <p className="mt-3 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </p>
        ) : null}

        <div className="mt-5 flex flex-col gap-2">
          <button
            type="button"
            disabled={pending}
            onClick={() => onChoose("copy")}
            className="flex items-center gap-3 rounded-lg border border-border bg-card/50 px-4 py-3 text-left transition-colors hover:bg-card disabled:opacity-50"
          >
            <Copy weight="bold" className="size-5 shrink-0 text-primary-text" />
            <span>
              <span className="block text-sm font-medium">Save as a copy</span>
              <span className="block text-xs text-muted-foreground">
                Keep the original, add a new trimmed clip
              </span>
            </span>
          </button>
          <button
            type="button"
            disabled={pending}
            onClick={() => onChoose("overwrite")}
            className="flex items-center gap-3 rounded-lg border border-border bg-card/50 px-4 py-3 text-left transition-colors hover:bg-card disabled:opacity-50"
          >
            <FloppyDisk weight="bold" className="size-5 shrink-0 text-warning" />
            <span>
              <span className="block text-sm font-medium">Overwrite original</span>
              <span className="block text-xs text-muted-foreground">
                Replace the clip. This can't be undone.
              </span>
            </span>
          </button>
        </div>

        <div className="mt-4 flex items-center justify-between">
          <button
            type="button"
            disabled={pending}
            onClick={onCancel}
            className="text-sm text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
          >
            Cancel
          </button>
          {pending ? (
            <span className="flex items-center gap-2 text-sm text-muted-foreground">
              <Spinner weight="bold" className="size-4" />
              Saving…
            </span>
          ) : null}
        </div>
      </div>
    </div>
  );
}
