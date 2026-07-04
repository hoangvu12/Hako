import { Monitor, Warning } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import { estBufferBytes, fmtBytesCoarse, RAM_WARN_BYTES } from "@/components/settings/format";
import type { SettingsSet } from "@/components/settings/config";
import type { Settings } from "@/lib/api";

/** RAM readout for the replay buffer, with an amber nudge once it gets large. */
function BufferRamHint({
  bitrateMbps,
  bufferSeconds,
}: {
  bitrateMbps: number;
  bufferSeconds: number;
}) {
  const bytes = estBufferBytes(bitrateMbps, bufferSeconds);
  const heavy = bytes >= RAM_WARN_BYTES;
  return (
    <p
      className={cn(
        "flex items-center gap-1.5 px-1 text-xs",
        heavy ? "text-warning" : "text-muted-foreground"
      )}
    >
      {heavy ? <Warning weight="fill" className="size-3.5 shrink-0" /> : null}
      Replay buffer holds ~{fmtBytesCoarse(bytes)} in RAM ({bufferSeconds}s × {bitrateMbps}{" "}
      Mbps)
      {heavy ? ". Lower the bitrate or buffer length to use less." : "."}
    </p>
  );
}

export function CaptureSection({
  draft,
  set,
}: {
  draft: Settings;
  set: SettingsSet;
}) {
  return (
    <>
      <SectionHero
        icon={Monitor}
        title="Capture"
        subtitle="How frames are grabbed and how much gameplay is held ready to clip."
      />

      <Panel title="Replay buffer">
        <Row
          label="Buffer length"
          hint="Seconds of gameplay held ready to save as a clip."
        >
          <Select
            value={String(draft.buffer_seconds)}
            onValueChange={(v) => set("buffer_seconds", Number(v))}
          >
            <SelectTrigger size="sm" className="w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {[30, 60, 90, 120, 180].map((s) => (
                <SelectItem key={s} value={String(s)}>
                  {s}s
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Storage"
          hint="RAM is fastest but uses memory; Disk spools the replay buffer to your drive to free RAM."
        >
          <Select
            value={draft.buffer_storage === "disk" ? "disk" : "ram"}
            onValueChange={(v) => set("buffer_storage", v)}
          >
            <SelectTrigger size="sm" className="w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="ram">RAM</SelectItem>
              <SelectItem value="disk">Disk</SelectItem>
            </SelectContent>
          </Select>
        </Row>
      </Panel>

      {/* The bitrate ceiling (Video) × the buffer length above drives the
          replay-buffer RAM. Only the RAM backend spends memory — the disk
          backend spools to drive instead. */}
      {draft.buffer_storage !== "disk" && (
        <BufferRamHint
          bitrateMbps={draft.bitrate_mbps}
          bufferSeconds={draft.buffer_seconds}
        />
      )}

      <Panel title="Performance">
        <Row
          label="Pause background work while gaming"
          hint="Defers cloud uploads, automatic cleanup, and clip filmstrips while Hako is recording or a supported game is open."
        >
          <Switch
            checked={draft.pause_background_while_gaming}
            onCheckedChange={(v) => set("pause_background_while_gaming", v)}
          />
        </Row>
      </Panel>


      <Panel title="Mouse cursor">
        <Row
          label="Record mouse cursor"
          hint="Draw your mouse pointer into clips. Games use a hardware cursor that isn't in the captured frame, so it's added on top — turn off for a clean, pointer-free recording."
        >
          <Switch
            checked={draft.record_cursor}
            onCheckedChange={(v) => set("record_cursor", v)}
          />
        </Row>
      </Panel>

      <Panel title="Tabbed out">
        <Row
          label='Show "tabbed out" card'
          hint="When you alt-tab and the game stops drawing, stamp a card over the frozen frame so anyone watching the clip knows you stepped away — instead of a silently held frame."
        >
          <Switch
            checked={draft.freeze_overlay}
            onCheckedChange={(v) => set("freeze_overlay", v)}
          />
        </Row>
      </Panel>
    </>
  );
}
