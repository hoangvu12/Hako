import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useValorantStatus } from "@/hooks/use-valorant";

function mapName(raw: string): string {
  if (!raw) return "—";
  // "/Game/Maps/Ascent/Ascent" → "Ascent"
  const parts = raw.split("/").filter(Boolean);
  return parts[parts.length - 1] || raw;
}

export default function ValorantPage() {
  const { data } = useValorantStatus();

  const connected = data?.connected ?? false;
  const running = data?.running ?? false;
  const state = data?.loop_state ?? null;

  const badge = !running
    ? { label: "Game offline", variant: "outline" as const }
    : !connected
      ? { label: "Riot API unreachable", variant: "outline" as const }
      : { label: state ?? "Connected", variant: "default" as const };

  return (
    <div className="space-y-6 p-8">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Valorant</h1>
        <p className="text-sm text-muted-foreground">
          Live match state from the Riot local presence API (polled every 2s).
        </p>
      </header>

      <Card>
        <CardHeader className="flex-row items-center justify-between">
          <div className="space-y-1">
            <CardTitle className="text-base">Match state</CardTitle>
            <CardDescription>
              Process + lockfile + `/chat/v4/presences`
            </CardDescription>
          </div>
          <Badge variant={badge.variant}>{badge.label}</Badge>
        </CardHeader>
        <CardContent className="space-y-2 text-sm text-muted-foreground">
          <div>
            sessionLoopState: <code>{state ?? "—"}</code> · map:{" "}
            <code>{mapName(data?.map ?? "")}</code> · score:{" "}
            <code>
              {data?.score_ally ?? 0} – {data?.score_enemy ?? 0}
            </code>
          </div>
          {data?.error ? (
            <div className="text-destructive">⚠ {data.error}</div>
          ) : null}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Event feed</CardTitle>
          <CardDescription>
            Kills/aces reconciled from match-details after the match ends (auto-cut
            clips). Full match recording + reconciliation wiring is the remaining
            work — derivation, reconciliation, and the event model are done and
            tested.
          </CardDescription>
        </CardHeader>
        <CardContent className="text-sm text-muted-foreground">
          No events yet.
        </CardContent>
      </Card>
    </div>
  );
}
