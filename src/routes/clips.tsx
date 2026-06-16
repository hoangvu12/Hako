import { convertFileSrc } from "@tauri-apps/api/core";
import { Link } from "@tanstack/react-router";
import { Play } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useClips, useDeleteClip, useSaveClip } from "@/hooks/use-library";
import type { ClipRecord } from "@/lib/api";

function fmtDuration(secs: number): string {
  const s = Math.round(secs);
  const m = Math.floor(s / 60);
  return m > 0 ? `${m}:${String(s % 60).padStart(2, "0")}` : `${s}s`;
}

function fmtSize(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

function ClipCard({ clip, onDelete }: { clip: ClipRecord; onDelete: () => void }) {
  return (
    <Card className="overflow-hidden">
      <Link
        to="/clips/$clipId"
        params={{ clipId: String(clip.id) }}
        className="group relative block aspect-video bg-muted"
      >
        {clip.thumb_path ? (
          <img
            src={convertFileSrc(clip.thumb_path)}
            alt={clip.title}
            className="h-full w-full object-cover"
            onError={(e) => {
              (e.currentTarget as HTMLImageElement).style.display = "none";
            }}
          />
        ) : null}
        {/* Hover play affordance */}
        <span className="absolute inset-0 flex items-center justify-center bg-black/0 transition-colors group-hover:bg-black/40">
          <Play className="size-10 text-white opacity-0 transition-opacity group-hover:opacity-100" />
        </span>
        {clip.event ? (
          <Badge className="absolute left-2 top-2">{clip.event}</Badge>
        ) : (
          <Badge variant="outline" className="absolute left-2 top-2">
            Manual
          </Badge>
        )}
        <span className="absolute bottom-2 right-2 rounded bg-black/70 px-1.5 py-0.5 text-xs text-white">
          {fmtDuration(clip.duration_secs)}
        </span>
      </Link>
      <CardContent className="space-y-1 p-3">
        <div className="truncate text-sm font-medium" title={clip.title}>
          {clip.title}
        </div>
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>
            {clip.width}×{clip.height} · {fmtSize(clip.size_bytes)}
          </span>
          <Button variant="ghost" size="sm" onClick={onDelete}>
            Delete
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

export default function ClipsPage() {
  const { data: clips, isLoading } = useClips();
  const save = useSaveClip();
  const del = useDeleteClip();

  return (
    <div className="space-y-6 p-8">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Clips</h1>
          <p className="text-sm text-muted-foreground">
            Saved highlights. Press <kbd>F9</kbd> in-game or use the button to
            save the last 30s.
          </p>
        </div>
        <Button onClick={() => save.mutate(30)} disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save last 30s"}
        </Button>
      </header>

      {save.error ? (
        <p className="text-sm text-destructive">{String(save.error)}</p>
      ) : null}

      {isLoading ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : !clips || clips.length === 0 ? (
        <Card>
          <CardContent className="p-8 text-center text-sm text-muted-foreground">
            No clips yet. Start a capture, then press <kbd>F9</kbd> (or “Save last
            30s”) to save a highlight.
          </CardContent>
        </Card>
      ) : (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {clips.map((clip) => (
            <ClipCard key={clip.id} clip={clip} onDelete={() => del.mutate(clip.id)} />
          ))}
        </div>
      )}
    </div>
  );
}
