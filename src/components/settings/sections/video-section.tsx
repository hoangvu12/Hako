import { SlidersHorizontal } from "@phosphor-icons/react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHero, Panel, Row, PresetCard } from "@/components/settings/primitives";
import {
  PRESETS,
  RESOLUTIONS,
  FPS_OPTIONS,
  BITRATE_OPTIONS,
  type SettingsSet,
} from "@/components/settings/config";
import type { GpuReport, Settings } from "@/lib/api";

export function VideoSection({
  draft,
  set,
  applyPreset,
  gpus,
}: {
  draft: Settings;
  set: SettingsSet;
  applyPreset: (p: (typeof PRESETS)[number]) => void;
  gpus: GpuReport | undefined;
}) {
  return (
    <>
      <SectionHero
        icon={SlidersHorizontal}
        title="Video"
        subtitle="Manage your recording resolution, frames per second, bitrate and more."
      />
      <Panel title="Recording Quality">
        <p className="pb-4 text-xs text-muted-foreground">
          Higher settings use more resources. If you have issues, try a lower one.
        </p>
        {/* Preset cards: Low / Standard / High / Custom. */}
        <div className="grid grid-cols-2 gap-3 pb-4">
          {PRESETS.map((p) => (
            <PresetCard
              key={p.key}
              title={p.label}
              blurb={p.blurb}
              line={p.line}
              selected={draft.quality_preset === p.key}
              onSelect={() => applyPreset(p)}
            />
          ))}
          <PresetCard
            title="Custom"
            blurb="Customize your own settings"
            selected={draft.quality_preset === "custom"}
            onSelect={() => set("quality_preset", "custom")}
          />
        </div>

        {/* Custom-only knobs — for a preset these are implied by the card. */}
        {draft.quality_preset === "custom" && (
          <>
            <Row
              label="Resolution"
              hint="Output size; capture is downscaled to fit (never upscaled)."
            >
              <Select value={draft.resolution} onValueChange={(v) => set("resolution", v)}>
                <SelectTrigger size="sm" className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {RESOLUTIONS.map((r) => (
                    <SelectItem key={r.value} value={r.value}>
                      {r.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Row>
            <Row label="FPS" hint="Frames per second to capture and encode.">
              <Select
                value={String(draft.target_fps)}
                onValueChange={(v) => set("target_fps", Number(v))}
              >
                <SelectTrigger size="sm" className="w-24">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {FPS_OPTIONS.map((f) => (
                    <SelectItem key={f} value={String(f)}>
                      {f}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Row>
            <Row label="Bitrate" hint="Encoding ceiling.">
              <Select
                value={String(draft.bitrate_mbps)}
                onValueChange={(v) => set("bitrate_mbps", Number(v))}
              >
                <SelectTrigger size="sm" className="w-24">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {BITRATE_OPTIONS.map((b) => (
                    <SelectItem key={b} value={String(b)}>
                      {b}M
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Row>
          </>
        )}

        {/* Video Encoder / Selected GPU / Codec — independent of preset. */}
        <Row label="Video Encoder" hint="Hardware (GPU) encoding. CPU encoding is coming soon.">
          <Select
            value={draft.video_encoder === "cpu" ? "cpu" : "gpu"}
            onValueChange={(v) => set("video_encoder", v)}
          >
            <SelectTrigger size="sm" className="w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="gpu">GPU</SelectItem>
              <SelectItem value="cpu" disabled>
                CPU (soon)
              </SelectItem>
            </SelectContent>
          </Select>
        </Row>
        <Row label="Selected GPU" hint="Which adapter captures and encodes.">
          <Select
            value={String(draft.gpu_adapter)}
            onValueChange={(v) => set("gpu_adapter", Number(v))}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="-1">Auto</SelectItem>
              {(gpus?.adapters ?? [])
                .filter((g) => !g.is_software)
                .map((g) => (
                  <SelectItem key={g.index} value={String(g.index)}>
                    {g.name}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Codec" hint="Video codec for saved clips.">
          <Select value={draft.codec} onValueChange={(v) => set("codec", v)}>
            <SelectTrigger size="sm" className="w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {["h264", "hevc", "av1"].map((c) => (
                <SelectItem key={c} value={c}>
                  {c.toUpperCase()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
      </Panel>
    </>
  );
}
