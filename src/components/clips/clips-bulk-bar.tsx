import * as React from "react";
import { X, Trash, CloudArrowUp, ListChecks } from "@phosphor-icons/react";

import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";

/**
 * Replaces the filters toolbar while a bulk selection is active. Same container
 * chrome (height, border, surface) as `ClipsToolbar` so swapping one for the
 * other doesn't shift the grid below.
 *
 * Memoized for the same reason the toolbar is: `ClipsPage` re-renders on every
 * virtualizer scroll tick. All props here are primitives or stable callbacks
 * (see the page's `useCallback`s), so an in-progress scroll never re-renders the
 * bar.
 */
export const ClipsBulkBar = React.memo(function ClipsBulkBar({
  selectedCount,
  allSelected,
  onSelectAll,
  onClear,
  onDelete,
  onUpload,
  uploading,
}: {
  selectedCount: number;
  allSelected: boolean;
  onSelectAll: () => void;
  onClear: () => void;
  onDelete: () => void;
  onUpload: () => void;
  uploading: boolean;
}) {
  return (
    <div className="shrink-0 border-b border-panel-border bg-panel">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-2.5 px-6 py-2.5">
        <button
          type="button"
          onClick={onClear}
          aria-label="Clear selection"
          className="flex size-8 items-center justify-center rounded-full text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <X className="size-4" weight="bold" />
        </button>

        <span className="whitespace-nowrap text-sm font-semibold text-foreground">
          {selectedCount} selected
        </span>

        <span className="mx-1.5 h-5 w-px bg-border/60" aria-hidden />

        <button
          type="button"
          onClick={allSelected ? onClear : onSelectAll}
          className="flex h-8 items-center gap-1.5 rounded-full border border-border/70 bg-field px-3 text-sm font-medium text-muted-foreground outline-none transition-colors hover:border-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <ListChecks className="size-4" />
          {allSelected ? "Select none" : "Select all"}
        </button>

        {/* Actions, pushed to the right edge — matches the toolbar's view zone. */}
        <div className="ml-auto flex items-center gap-2">
          <Button size="sm" variant="secondary" onClick={onUpload} disabled={uploading}>
            <CloudArrowUp weight="bold" />
            Upload to cloud
          </Button>
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button size="sm" variant="destructive">
                <Trash weight="bold" />
                Delete
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>
                  Delete {selectedCount} clip{selectedCount > 1 ? "s" : ""}?
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {selectedCount > 1 ? "These clips" : "This clip"} will be
                  permanently removed from your library. This can't be undone.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={onDelete}>
                  Delete
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      </div>
    </div>
  );
});
