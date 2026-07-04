import { Scissors } from "@phosphor-icons/react";

import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { HotkeyRecorder } from "@/components/ui/hotkey-recorder";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import { CLIP_LENGTHS, type SettingsSet } from "@/components/settings/config";
import type { Settings } from "@/lib/api";

export function ClipSection({
  draft,
  set,
  setLocal,
  commit,
}: {
  draft: Settings;
  set: SettingsSet;
  setLocal: SettingsSet;
  commit: () => void;
}) {
  return (
    <>
      <SectionHero
        icon={Scissors}
        title="Clip Settings"
        subtitle="Set your save hotkey and the padding kept around each clip."
      />
      <Panel title="Clipping">
        <Row
          label="Save-clip hotkey"
          hint="Click and press the keys. Saves the last buffered seconds."
        >
          <HotkeyRecorder
            aria-label="Save-clip hotkey"
            value={draft.save_hotkey}
            onChange={(accel) => accel && set("save_hotkey", accel)}
            allowClear={false}
          />
        </Row>
        <Row label="Clip length" hint="Seconds the hotkey captures (capped at the buffer length).">
          <Select
            value={String(draft.clip_seconds)}
            onValueChange={(v) => set("clip_seconds", Number(v))}
          >
            <SelectTrigger size="sm" className="w-24">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {CLIP_LENGTHS.filter((s) => s <= draft.buffer_seconds).map((s) => (
                <SelectItem key={s} value={String(s)}>
                  {s}s
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Long-recording hotkey"
          hint="Start/stop a manual recording of any length (coming soon)."
        >
          <HotkeyRecorder
            aria-label="Long-recording hotkey"
            value={draft.long_recording_hotkey}
            onChange={(accel) => accel && set("long_recording_hotkey", accel)}
            allowClear={false}
          />
        </Row>
        <Row label="Pad before" hint="Extra seconds kept before the moment.">
          <div className="flex items-center gap-2">
            <Input
              type="number"
              className="h-9 w-20 text-right"
              value={draft.pad_before_secs}
              onChange={(e) => setLocal("pad_before_secs", Number(e.target.value))}
              onBlur={commit}
            />
            <span className="text-sm text-muted-foreground">s</span>
          </div>
        </Row>
        <Row label="Pad after" hint="Extra seconds kept after the moment.">
          <div className="flex items-center gap-2">
            <Input
              type="number"
              className="h-9 w-20 text-right"
              value={draft.pad_after_secs}
              onChange={(e) => setLocal("pad_after_secs", Number(e.target.value))}
              onBlur={commit}
            />
            <span className="text-sm text-muted-foreground">s</span>
          </div>
        </Row>
      </Panel>
    </>
  );
}
