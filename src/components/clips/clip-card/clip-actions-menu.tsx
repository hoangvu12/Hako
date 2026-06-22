import * as React from "react";
import {
  DotsThreeVertical,
  PencilSimple,
  Trash,
  CloudArrowUp,
  ArrowsClockwise,
  Prohibit,
  LinkSimple,
  FolderOpen,
} from "@phosphor-icons/react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { revealClip, type ClipRecord } from "@/lib/api";
import {
  useCancelUpload,
  useClipUpload,
  useUploadClip,
} from "@/hooks/use-cloud";

// Shared trigger styling for the "⋯" actions affordance, whether it's the cheap
// placeholder button or the real Radix trigger.
const ACTIONS_TRIGGER_CLASS =
  "-mr-1 flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-[color,opacity] outline-none hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100";

/**
 * The per-card "⋯" actions menu, lazily upgraded. Until the user first opens it,
 * this is a plain <button> — so a card mounting during scroll pays for one icon,
 * not the full Radix DropdownMenu (Root + Popper + Portal + their effects, which
 * run even while closed). On first click we mount the real menu and open it; it
 * then behaves normally. Profiling showed the per-card Radix tree was the single
 * largest scroll-mount cost.
 */
export function ClipActionsMenu({
  clip,
  onRename,
  onDelete,
}: {
  clip: ClipRecord;
  onRename: (clip: ClipRecord) => void;
  onDelete: (clip: ClipRecord) => void;
}) {
  const [mounted, setMounted] = React.useState(false);
  const [open, setOpen] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);

  if (!mounted) {
    return (
      <button
        type="button"
        aria-label="Clip actions"
        className={ACTIONS_TRIGGER_CLASS}
        onClick={() => {
          setMounted(true);
          setOpen(true);
        }}
      >
        <DotsThreeVertical weight="bold" className="size-4" />
      </button>
    );
  }

  return (
    <>
      <DropdownMenu open={open} onOpenChange={setOpen}>
        <DropdownMenuTrigger aria-label="Clip actions" className={ACTIONS_TRIGGER_CLASS}>
          <DotsThreeVertical weight="bold" className="size-4" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <CloudUploadItems clip={clip} />
          <DropdownMenuItem
            onSelect={() => {
              void revealClip(clip.id).catch(() => {});
            }}
          >
            <FolderOpen />
            Open in folder
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => onRename(clip)}>
            <PencilSimple />
            Rename
          </DropdownMenuItem>
          {/* Open the confirm dialog rather than deleting outright — the menu
              closes and the alert dialog takes focus. */}
          <DropdownMenuItem
            variant="destructive"
            onSelect={() => setConfirmDelete(true)}
          >
            <Trash />
            Delete
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete clip?</AlertDialogTitle>
            <AlertDialogDescription>
              “{clip.title || "Untitled"}” will be permanently removed from your
              library. This can't be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => onDelete(clip)}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/**
 * Cloud-upload entries in the clip actions menu, adapting to the clip's current
 * upload state: enqueue/retry when idle or failed, cancel while in-flight, copy
 * the shared link once it's done. Only mounted when the menu is open (lazy), so
 * its `useClipUpload` subscription costs nothing during scroll.
 */
function CloudUploadItems({ clip }: { clip: ClipRecord }) {
  const upload = useClipUpload(clip.id);
  const startUpload = useUploadClip();
  const cancelUpload = useCancelUpload();

  const status = upload?.status;
  const inFlight = status === "queued" || status === "uploading";
  const enqueue = () => startUpload.mutate({ clipId: clip.id });

  return (
    <>
      {inFlight ? (
        <DropdownMenuItem onSelect={() => cancelUpload.mutate(clip.id)}>
          <Prohibit />
          Cancel upload
        </DropdownMenuItem>
      ) : status === "error" ? (
        <DropdownMenuItem onSelect={enqueue}>
          <ArrowsClockwise />
          Retry upload
        </DropdownMenuItem>
      ) : status === "done" ? (
        <>
          <DropdownMenuItem onSelect={enqueue}>
            <CloudArrowUp />
            Upload again
          </DropdownMenuItem>
          {upload?.remoteUrl ? (
            <DropdownMenuItem
              onSelect={() => {
                void navigator.clipboard
                  .writeText(upload.remoteUrl as string)
                  .catch(() => {});
              }}
            >
              <LinkSimple />
              Copy cloud link
            </DropdownMenuItem>
          ) : null}
        </>
      ) : (
        <DropdownMenuItem onSelect={enqueue}>
          <CloudArrowUp />
          Upload to cloud
        </DropdownMenuItem>
      )}
      <DropdownMenuSeparator />
    </>
  );
}
