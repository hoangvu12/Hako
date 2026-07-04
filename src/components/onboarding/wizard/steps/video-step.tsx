import { SlidersHorizontal } from "@phosphor-icons/react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHero, Panel, Row, PresetCard } from "@/components/settings/primitives";
import type { Settings } from "@/lib/api";
import { PRESETS, RESOLUTIONS, FPS_OPTIONS, BITRATE_OPTIONS, type WizardSet } from "../config";

export function VideoStep({
  draft,
  set,
  applyPreset,
}: {
  draft: Settings;
  set: WizardSet;
  applyPreset: (p: (typeof PRESETS)[number]) => void;
}) {
  return (
    <>
      <SectionHero
        icon={SlidersHorizontal}
        title="Recording quality"
        subtitle="Higher settings look better but use more resources. You can fine-tune this later."
      />
      <Panel title="Quality preset">
        <div className="grid grid-cols-2 gap-3 pt-1">
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
            blurb="Choose your own resolution, FPS and bitrate"
            selected={draft.quality_preset === "custom"}
            onSelect={() => set("quality_preset", "custom")}
          />
        </div>
      </Panel>

      {draft.quality_preset === "custom" && (
        <Panel title="Custom">
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
        </Panel>
      )}
    </>
  );
}
