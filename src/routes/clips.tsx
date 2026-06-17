import * as React from "react";
import {
  Scissors,
  CaretDown,
  ArrowsDownUp,
  MagnifyingGlass,
} from "@phosphor-icons/react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ClipCard } from "@/components/clips/clip-card";
import {
  useClips,
  useDeleteClip,
  useRenameClip,
  useSaveClip,
} from "@/hooks/use-library";
import type { ClipRecord } from "@/lib/api";

type SortKey = "newest" | "oldest" | "largest";
const SORTS: { key: SortKey; label: string }[] = [
  { key: "newest", label: "Newest first" },
  { key: "oldest", label: "Oldest first" },
  { key: "largest", label: "Largest first" },
];

function sortClips(clips: ClipRecord[], key: SortKey): ClipRecord[] {
  const copy = [...clips];
  switch (key) {
    case "oldest":
      return copy.sort((a, b) => a.created_unix_ms - b.created_unix_ms);
    case "largest":
      return copy.sort((a, b) => b.size_bytes - a.size_bytes);
    default:
      return copy.sort((a, b) => b.created_unix_ms - a.created_unix_ms);
  }
}

export default function ClipsPage() {
  const { data: clips, isLoading } = useClips();
  const save = useSaveClip();
  const del = useDeleteClip();
  const rename = useRenameClip();

  const [query, setQuery] = React.useState("");
  const [sort, setSort] = React.useState<SortKey>("newest");

  const visible = React.useMemo(() => {
    const list = (clips ?? []).filter((c) =>
      c.title.toLowerCase().includes(query.trim().toLowerCase())
    );
    return sortClips(list, sort);
  }, [clips, query, sort]);

  function handleRename(clip: ClipRecord) {
    const next = window.prompt("Rename clip", clip.title);
    if (next && next !== clip.title) rename.mutate({ id: clip.id, title: next });
  }

  return (
    <div className="flex h-full flex-col">
      {/* Toolbar */}
      <div className="flex h-14 shrink-0 items-center justify-between border-b border-panel-border bg-panel px-6">
        <Button
          size="sm"
          onClick={() => save.mutate(30)}
          disabled={save.isPending}
        >
          <Scissors weight="bold" />
          {save.isPending ? "Saving…" : "Save last 30s"}
        </Button>

        <div className="flex items-center gap-4">
          <span className="text-sm font-medium text-muted-foreground">
            {visible.length} {visible.length === 1 ? "Clip" : "Clips"}
          </span>

          <DropdownMenu>
            <DropdownMenuTrigger className="flex items-center gap-1 text-sm text-muted-foreground outline-none transition-colors hover:text-foreground">
              <ArrowsDownUp className="size-4" />
              <CaretDown className="size-3" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {SORTS.map((s) => (
                <DropdownMenuItem
                  key={s.key}
                  onSelect={() => setSort(s.key)}
                  className={
                    sort === s.key ? "text-foreground" : "text-muted-foreground"
                  }
                >
                  {s.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>

          <div className="relative">
            <MagnifyingGlass className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search"
              className="h-8 w-48 rounded-full border-border/80 bg-field pl-9 placeholder:text-muted-foreground/70"
            />
          </div>
        </div>
      </div>

      {save.error ? (
        <p className="shrink-0 bg-panel px-6 pb-2 text-sm text-destructive">
          {String(save.error)}
        </p>
      ) : null}

      {/* Grid */}
      <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto p-6">
        {isLoading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : (clips?.length ?? 0) === 0 ? (
          <div className="rounded-xl border border-dashed border-border/60 p-10 text-center text-sm text-muted-foreground">
            No clips yet. Press <kbd>F9</kbd> in-game or hit “Save last 30s” to
            capture a highlight.
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5">
            {visible.map((clip) => (
              <ClipCard
                key={clip.id}
                clip={clip}
                onDelete={() => del.mutate(clip.id)}
                onRename={() => handleRename(clip)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
