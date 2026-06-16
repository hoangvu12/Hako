import { Link, useParams } from "@tanstack/react-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ArrowLeft } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useClips } from "@/hooks/use-library";

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

export default function ClipDetailPage() {
  const { clipId } = useParams({ from: "/clips/$clipId" });
  const { data: clips, isLoading } = useClips();
  const clip = clips?.find((c) => String(c.id) === clipId);

  return (
    <div className="space-y-6 p-8">
      <Button asChild variant="ghost" size="sm">
        <Link to="/clips">
          <ArrowLeft className="size-4" /> Back to clips
        </Link>
      </Button>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">
            {clip?.title ?? `Clip ${clipId}`}
          </CardTitle>
          <CardDescription>
            {clip
              ? `${clip.width}×${clip.height} · ${fmtDuration(
                  clip.duration_secs,
                )} · ${fmtSize(clip.size_bytes)}`
              : "Loading…"}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {isLoading ? (
            <div className="flex aspect-video items-center justify-center rounded-lg border border-dashed text-sm text-muted-foreground">
              Loading…
            </div>
          ) : !clip ? (
            <div className="flex aspect-video items-center justify-center rounded-lg border border-dashed text-sm text-muted-foreground">
              Clip not found — it may have been deleted.
            </div>
          ) : (
            <>
              {/* key forces the <video> to reload when navigating between clips */}
              <video
                key={clip.id}
                src={convertFileSrc(clip.path)}
                controls
                autoPlay
                className="aspect-video w-full rounded-lg bg-black"
              />
              {clip.event ? <Badge>{clip.event}</Badge> : null}
            </>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
