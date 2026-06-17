import { useRecorderStatus } from "@/hooks/use-recorder";
import { useGpuInfo, useFfmpegInfo } from "@/hooks/use-gpu";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

/**
 * Live recorder / encoder / GPU readout. Lives in Settings as a "Status"
 * section (the standalone dashboard was removed in favour of Clips-as-home).
 */
export function RecordingStatus() {
  const { data, isLoading } = useRecorderStatus();
  const { data: gpu } = useGpuInfo();
  const { data: ffmpeg } = useFfmpegInfo();
  const nvencReady = ffmpeg?.encoders.find(
    (e) => e.name === "h264_nvenc"
  )?.available;

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Recorder</CardTitle>
            <CardDescription>RAM ring buffer state</CardDescription>
          </CardHeader>
          <CardContent className="flex items-center gap-2">
            {isLoading ? (
              <Badge variant="secondary">Loading…</Badge>
            ) : (
              <Badge variant={data?.capturing ? "default" : "secondary"}>
                {data?.capturing ? "Capturing" : "Idle"}
              </Badge>
            )}
            <span className="text-sm text-muted-foreground">
              {data?.buffer_seconds ?? "—"}s buffer
            </span>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Valorant</CardTitle>
            <CardDescription>Process / window detection</CardDescription>
          </CardHeader>
          <CardContent>
            <Badge variant={data?.valorant_detected ? "default" : "outline"}>
              {data?.valorant_detected ? "Detected" : "Not running"}
            </Badge>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Encoder</CardTitle>
            <CardDescription>Hardware backend</CardDescription>
          </CardHeader>
          <CardContent className="space-y-1.5">
            <div className="flex items-center gap-2">
              <Badge variant={gpu?.selected_encoder ? "default" : "outline"}>
                {gpu?.selected_encoder ?? data?.encoder ?? "Not selected"}
              </Badge>
              {nvencReady && (
                <Badge variant="secondary" className="text-success">
                  ready
                </Badge>
              )}
            </div>
            <p className="text-xs text-muted-foreground">
              {gpu?.device_ok && `D3D11 ${gpu.feature_level} · `}
              {ffmpeg
                ? `FFmpeg ${
                    ffmpeg.avcodec_version.split(".")[0] === "62"
                      ? "8.1"
                      : ffmpeg.avcodec_version
                  }`
                : "probing FFmpeg…"}
            </p>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">GPUs</CardTitle>
          <CardDescription>
            DXGI adapters — the discrete GPU is preferred to avoid cross-adapter
            copies.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-2">
          {gpu?.error && <p className="text-sm text-destructive">{gpu.error}</p>}
          {gpu?.adapters.map((a) => (
            <div
              key={a.index}
              className="flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-sm"
            >
              <div className="flex items-center gap-2">
                <span className="font-medium">{a.name}</span>
                {a.preferred && <Badge>Active</Badge>}
                {a.is_software && <Badge variant="outline">Software</Badge>}
              </div>
              <div className="flex items-center gap-3 text-muted-foreground">
                {a.dedicated_vram_mb > 0 && (
                  <span>{(a.dedicated_vram_mb / 1024).toFixed(1)} GB</span>
                )}
                <span>{a.encoder ?? "—"}</span>
              </div>
            </div>
          ))}
          {!gpu && (
            <p className="text-sm text-muted-foreground">Detecting GPUs…</p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
