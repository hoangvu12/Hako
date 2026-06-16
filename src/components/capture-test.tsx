import { useEffect, useState } from "react";

import {
  useWindows,
  useCaptureStats,
  useCaptureStatus,
  useStartCapture,
  useStopCapture,
} from "@/hooks/use-capture";
import { useGpuInfo } from "@/hooks/use-gpu";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

const FPS_OPTIONS = [30, 60, 120, 144, 240];

/**
 * Dev tool: pick a window, start WGC capture, watch live throughput.
 * Capture a moving window (video, scrolling) — a static window only emits its
 * first frame (WGC fires on content change).
 */
export function CaptureTest() {
  const { data: windows, refetch, isFetching } = useWindows();
  const { data: gpu } = useGpuInfo();
  const { stats, reset } = useCaptureStats();
  const status = useCaptureStatus();
  const start = useStartCapture();
  const stop = useStopCapture();

  const adapters = gpu?.adapters.filter((a) => !a.is_software) ?? [];

  const [hwnd, setHwnd] = useState<number | null>(null);
  const [fps, setFps] = useState(60);
  const [adapter, setAdapter] = useState<number | null>(null);
  const [running, setRunning] = useState(false);

  // Default the selection to the first window once loaded.
  useEffect(() => {
    if (hwnd === null && windows && windows.length > 0) {
      const firstNonHako = windows.find((w) => w.title !== "Hako") ?? windows[0];
      setHwnd(firstNonHako.hwnd);
    }
  }, [windows, hwnd]);

  // Default capture GPU to the display-owning adapter (avoids cross-adapter copies).
  useEffect(() => {
    if (adapter === null && adapters.length > 0) {
      const display = adapters.find((a) => a.drives_display) ?? adapters[0];
      setAdapter(display.index);
    }
  }, [adapters, adapter]);

  // The recorder lives in the Rust core (background threads) — it keeps running
  // when this card unmounts (e.g. navigating to Clips). So we DON'T stop on
  // unmount; instead we sync the running flag from the backend, which also
  // restores the correct state when navigating back to the Dashboard.
  useEffect(() => {
    if (status.data !== undefined) setRunning(status.data);
  }, [status.data]);

  const onStart = async () => {
    if (hwnd === null) return;
    reset();
    await start.mutateAsync({ hwnd, fps, adapterIndex: adapter ?? undefined });
    await status.refetch();
  };

  const onStop = async () => {
    await stop.mutateAsync();
    await status.refetch();
  };

  const error = start.error
    ? String((start.error as Error).message ?? start.error)
    : null;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Capture test</CardTitle>
        <CardDescription>
          WGC window capture on the shared D3D11 device. Pick a moving window to
          see real FPS.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <select
            value={hwnd ?? ""}
            disabled={running}
            onChange={(e) => setHwnd(Number(e.target.value))}
            className="h-9 min-w-56 flex-1 rounded-md border bg-background px-3 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50"
          >
            {windows?.map((w) => (
              <option key={w.hwnd} value={w.hwnd}>
                {w.title}
              </option>
            ))}
          </select>

          <select
            value={fps}
            disabled={running}
            onChange={(e) => setFps(Number(e.target.value))}
            className="h-9 rounded-md border bg-background px-3 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50"
          >
            {FPS_OPTIONS.map((f) => (
              <option key={f} value={f}>
                {f} fps
              </option>
            ))}
          </select>

          <select
            value={adapter ?? ""}
            disabled={running}
            onChange={(e) => setAdapter(Number(e.target.value))}
            title="Capture GPU"
            className="h-9 rounded-md border bg-background px-3 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50"
          >
            {adapters.map((a) => (
              <option key={a.index} value={a.index}>
                {a.vendor_label}
                {a.drives_display ? " (display)" : ""}
              </option>
            ))}
          </select>

          <Button
            variant="outline"
            size="sm"
            disabled={running || isFetching}
            onClick={() => refetch()}
          >
            Reload
          </Button>

          {running ? (
            <Button variant="destructive" size="sm" onClick={onStop}>
              Stop
            </Button>
          ) : (
            <Button size="sm" disabled={hwnd === null || start.isPending} onClick={onStart}>
              Start
            </Button>
          )}
        </div>

        {error && <p className="text-sm text-destructive">{error}</p>}

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <Stat label="Capture FPS" value={running ? stats?.fps?.toFixed(1) ?? "—" : "—"} />
          <Stat
            label="Resolution"
            value={stats?.width ? `${stats.width}×${stats.height}` : "—"}
          />
          <Stat label="Frames" value={stats ? String(stats.frames) : "—"} />
          <Stat label="Arrived" value={stats ? String(stats.arrived) : "—"} />
          <Stat
            label="Encode FPS"
            value={running ? stats?.encoded_fps?.toFixed(1) ?? "—" : "—"}
          />
          <Stat
            label="Bitrate"
            value={
              running && stats?.encoded_kbps
                ? `${(stats.encoded_kbps / 1000).toFixed(1)} Mbps`
                : "—"
            }
          />
          <Stat
            label="Encoded"
            value={stats ? String(stats.encoded_frames) : "—"}
          />
        </div>

        <div className="flex items-center gap-2">
          <Badge variant={running ? "default" : "outline"}>
            {running ? "Capturing + encoding" : "Stopped"}
          </Badge>
          {running && (
            <span className="text-xs text-muted-foreground">
              target {stats?.target_fps ?? fps} fps · h264_qsv · zero-copy GPU
            </span>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function Stat({ label, value }: { label: string; value?: string }) {
  return (
    <div className="rounded-md border px-3 py-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="text-lg font-semibold tabular-nums">{value ?? "—"}</div>
    </div>
  );
}
