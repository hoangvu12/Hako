import { useState } from "react";
import { useQuery, useMutation } from "@tanstack/react-query";
import { Plus, MagnifyingGlass, Warning, CheckCircle } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  addCustomGame,
  listWindows,
  type CustomGame,
  type WindowTarget,
} from "@/lib/api";

/**
 * "Request a Game" picker (Medal's RAG): list the open windows Hako doesn't
 * already know, let the user point at the game's window, and add it to the custom
 * list. Once added, the generic integration auto-records it whenever it runs.
 */
export function RequestGameDialog({
  onAdded,
}: {
  /** Called with the new row after a successful add (to refresh the list). */
  onAdded: (game: CustomGame) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  // Only enumerate windows while the dialog is open; refetch each open so the
  // list is current (the user just launched the game they want to add).
  const { data: windows = [], isFetching } = useQuery({
    queryKey: ["windows"],
    queryFn: listWindows,
    enabled: open,
    staleTime: 0,
  });

  // Keep the dialog open after a successful add so the user gets clear
  // confirmation (there's no toast system) and can add several games in one
  // sitting. `add.data` holds the last-added row for the success banner; the
  // list behind refreshes via `onAdded`.
  const add = useMutation({
    mutationFn: (hwnd: number) => addCustomGame(hwnd),
    onSuccess: (game) => {
      onAdded(game);
      setQuery("");
    },
  });

  const q = query.trim().toLowerCase();
  const filtered = q
    ? windows.filter((w) => w.title.toLowerCase().includes(q))
    : windows;

  return (
    <>
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          add.reset();
          setOpen(true);
        }}
        className="gap-1.5"
      >
        <Plus weight="bold" className="size-4" />
        Add a game
      </Button>

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent className="max-w-md">
          <AlertDialogHeader className="sm:items-start sm:text-left">
            <AlertDialogTitle>Add a game</AlertDialogTitle>
            <AlertDialogDescription>
              Pick the game's window. Hako will record it now and auto-record it
              whenever it's running.
            </AlertDialogDescription>
          </AlertDialogHeader>

          <div className="relative">
            <MagnifyingGlass className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search open windows"
              className="h-9 bg-field pl-9"
            />
          </div>

          {add.isError && (
            <p className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
              <Warning weight="fill" className="mt-px size-4 shrink-0" />
              <span>{String(add.error)}</span>
            </p>
          )}

          {add.isSuccess && add.data && (
            <p className="flex items-start gap-2 rounded-md border border-success/40 bg-success/10 px-3 py-2 text-xs text-success">
              <CheckCircle weight="fill" className="mt-px size-4 shrink-0" />
              <span>
                Added <span className="font-medium">{add.data.display_name}</span> — it'll
                auto-record whenever it's running.
              </span>
            </p>
          )}

          <div className="scrollbar-thin max-h-72 overflow-y-auto rounded-lg border border-border/60">
            {filtered.length === 0 ? (
              <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                {isFetching ? "Loading open windows…" : "No windows match."}
              </p>
            ) : (
              filtered.map((w: WindowTarget) => (
                <button
                  key={w.hwnd}
                  type="button"
                  disabled={add.isPending}
                  onClick={() => add.mutate(w.hwnd)}
                  className={cn(
                    "flex w-full items-center gap-2 border-b border-border/40 px-3 py-2.5 text-left text-sm transition-colors last:border-b-0",
                    "hover:bg-accent/50 disabled:cursor-not-allowed disabled:opacity-50"
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{w.title}</span>
                  {add.isPending && add.variables === w.hwnd && (
                    <span className="text-xs text-muted-foreground">Adding…</span>
                  )}
                </button>
              ))
            )}
          </div>

          <AlertDialogFooter>
            <AlertDialogCancel disabled={add.isPending}>
              {add.isSuccess ? "Done" : "Cancel"}
            </AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
