import * as React from "react";
import { createLazyRoute } from "@tanstack/react-router";
import { Crosshair } from "@phosphor-icons/react";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { ClipCard } from "@/components/clips/clip-card";
import {
  useMatchState,
  useMatchSummary,
  useValorantStatus,
} from "@/hooks/use-valorant";
import { useClips, useDeleteClip, useRenameClip } from "@/hooks/use-library";
import type { ClipRecord } from "@/lib/api";

function mapName(raw: string): string {
  if (!raw) return "—";
  // "/Game/Maps/Ascent/Ascent" → "Ascent"
  const parts = raw.split("/").filter(Boolean);
  return parts[parts.length - 1] || raw;
}

function fmtDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "—";
  const total = Math.round(ms / 1000);
  const m = Math.floor(total / 60);
  return `${m}:${String(total % 60).padStart(2, "0")}`;
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="space-y-0.5">
      <div className="text-[11px] tracking-wide text-muted-foreground uppercase">
        {label}
      </div>
      <div className="font-medium text-foreground tabular-nums">{value}</div>
    </div>
  );
}

// Lazy-loaded: deferred out of the boot bundle, fetched on navigation.
export const Route = createLazyRoute("/valorant")({
  component: ValorantPage,
});

function ValorantPage() {
  const { data: status } = useValorantStatus();
  const match = useMatchState();
  const summary = useMatchSummary();

  const { data: clips } = useClips();
  const del = useDeleteClip();
  const rename = useRenameClip();

  // The live event is the freshest source; fall back to the poll on first load.
  const connected = status?.connected ?? false;
  const running = status?.running ?? false;
  const recording = match?.recording ?? false;
  const inMatch = match?.in_match ?? false;
  const state = match?.loop_state ?? status?.loop_state ?? null;
  const map = match?.map ?? status?.map ?? "";
  const scoreAlly = match?.score_ally ?? status?.score_ally ?? 0;
  const scoreEnemy = match?.score_enemy ?? status?.score_enemy ?? 0;

  const badge = !running
    ? { label: "Game offline", variant: "outline" as const }
    : !connected
      ? { label: "Riot API unreachable", variant: "outline" as const }
      : inMatch
        ? { label: "In match", variant: "default" as const }
        : { label: state ?? "Connected", variant: "secondary" as const };

  // Auto-clips: library entries tagged with an event (Ace, Triple Kill, …).
  const autoClips = React.useMemo(
    () =>
      (clips ?? [])
        .filter((c: ClipRecord) => c.event != null)
        .sort((a, b) => b.created_unix_ms - a.created_unix_ms),
    [clips]
  );

  function handleRename(clip: ClipRecord) {
    const next = window.prompt("Rename clip", clip.title);
    if (next && next !== clip.title) rename.mutate({ id: clip.id, title: next });
  }

  return (
    <div className="scrollbar-thin h-full overflow-y-auto">
      <div className="space-y-6 p-8">
        <header>
          <h1 className="text-2xl font-semibold tracking-tight">Valorant</h1>
          <p className="text-sm text-muted-foreground">
            Auto-clips multikills, aces, and knife kills — recorded for the whole
            match, cut from match-details when the match ends.
          </p>
        </header>

        <Card>
          <CardHeader className="flex-row items-center justify-between">
            <div className="space-y-1">
              <CardTitle className="text-base">Match state</CardTitle>
              <CardDescription>
                Live from the Riot local presence API (polled every 2s)
              </CardDescription>
            </div>
            <div className="flex items-center gap-2">
              {recording ? (
                <Badge
                  variant="destructive"
                  className="gap-1.5"
                  aria-label="Recording the match"
                >
                  <span className="size-2 animate-pulse rounded-full bg-current" />
                  Recording
                </Badge>
              ) : null}
              <Badge variant={badge.variant}>{badge.label}</Badge>
            </div>
          </CardHeader>
          <CardContent className="space-y-2 text-sm text-muted-foreground">
            <div>
              sessionLoopState: <code>{state ?? "—"}</code> · map:{" "}
              <code>{mapName(map)}</code> · score:{" "}
              <code>
                {scoreAlly} – {scoreEnemy}
              </code>
            </div>
            {recording ? (
              <div className="text-foreground">
                Recording the full match — clips are cut automatically when it
                ends.
              </div>
            ) : inMatch ? (
              <div>
                In a match. Start a capture from the recorder to auto-record and
                clip it.
              </div>
            ) : null}
            {status?.error ? (
              <div className="text-destructive">⚠ {status.error}</div>
            ) : null}
          </CardContent>
        </Card>

        {summary ? (
          <Card className={summary.won ? "border-success/40" : "border-destructive/40"}>
            <CardHeader className="flex-row items-center justify-between">
              <div className="space-y-1">
                <CardTitle className="text-base">{summary.title}</CardTitle>
                <CardDescription>Last match result</CardDescription>
              </div>
              <Badge variant={summary.won ? "default" : "destructive"}>
                {summary.won ? "Victory" : "Defeat"}
              </Badge>
            </CardHeader>
            <CardContent>
              <div className="flex flex-wrap gap-x-6 gap-y-2 text-sm">
                <Stat label="K / D / A" value={`${summary.kills} / ${summary.deaths} / ${summary.assists}`} />
                <Stat label="Headshot %" value={`${summary.headshot_pct.toFixed(1)}%`} />
                <Stat label="Agent" value={summary.agent || "Unknown"} />
                <Stat label="Map" value={mapName(summary.map)} />
                <Stat label="Mode" value={summary.mode || "—"} />
                <Stat label="Duration" value={fmtDuration(summary.duration_ms)} />
              </div>
            </CardContent>
          </Card>
        ) : null}

        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-base font-semibold">Auto-clips</h2>
              <p className="text-sm text-muted-foreground">
                Multikills, aces, and knife kills cut from your matches.
              </p>
            </div>
            {autoClips.length > 0 ? (
              <span className="text-sm font-medium text-muted-foreground">
                {autoClips.length}{" "}
                {autoClips.length === 1 ? "clip" : "clips"}
              </span>
            ) : null}
          </div>

          {autoClips.length === 0 ? (
            <div className="flex flex-col items-center gap-3 rounded-xl border border-dashed border-border/60 p-10 text-center">
              <div className="flex size-12 items-center justify-center rounded-2xl bg-primary/10 text-primary-text">
                <Crosshair className="size-6" weight="duotone" />
              </div>
              <p className="text-sm text-muted-foreground">
                No auto-clips yet. Play a match with a capture running — Hako cuts
                a clip for each multikill, ace, and knife kill once the match
                ends.
              </p>
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5">
              {autoClips.map((clip) => (
                <ClipCard
                  key={clip.id}
                  clip={clip}
                  onDelete={() => del.mutate(clip.id)}
                  onRename={() => handleRename(clip)}
                />
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
