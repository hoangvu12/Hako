import { Scissors } from "@phosphor-icons/react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { HotkeyRecorder } from "@/components/ui/hotkey-recorder";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import type { Settings } from "@/lib/api";
import { CLIP_LENGTHS, type WizardSet } from "../config";

export function ClipsStep({ draft, set }: { draft: Settings; set: WizardSet }) {
  return (
    <>
      <SectionHero
        icon={Scissors}
        title="Save-clip hotkey"
        subtitle="Press this in-game to instantly save the last few seconds."
      />
      <Panel title="Clipping">
        <Row label="Save-clip hotkey" hint="Click and press the keys you want.">
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
      </Panel>
    </>
  );
}
