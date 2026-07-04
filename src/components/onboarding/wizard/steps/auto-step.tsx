import { Crosshair } from "@phosphor-icons/react";

import { Switch } from "@/components/ui/switch";
import { SectionHero, Panel, Row, PresetCard } from "@/components/settings/primitives";
import type { EventToggles, Settings } from "@/lib/api";
import { CAPTURE_MODES, EVENT_LABELS, type WizardSet } from "../config";

export function AutoStep({
  draft,
  set,
  toggleEvent,
}: {
  draft: Settings;
  set: WizardSet;
  toggleEvent: (key: keyof EventToggles) => void;
}) {
  return (
    <>
      <SectionHero
        icon={Crosshair}
        title="Auto-capture"
        subtitle="Let Hako clip your best Valorant moments automatically."
      />
      <Panel title="Mode">
        <div className="grid grid-cols-2 gap-3 pt-1">
          {CAPTURE_MODES.map((m) => (
            <PresetCard
              key={m.key}
              title={m.label}
              blurb={m.blurb}
              selected={draft.auto_capture_mode === m.key}
              onSelect={() => set("auto_capture_mode", m.key)}
            />
          ))}
        </div>
      </Panel>
      {draft.auto_capture_mode === "highlights" && (
        <Panel title="Auto-captured events">
          {EVENT_LABELS.map((ev) => (
            <Row key={ev.key} label={ev.label} hint={ev.hint}>
              <Switch checked={draft.events[ev.key]} onCheckedChange={() => toggleEvent(ev.key)} />
            </Row>
          ))}
        </Panel>
      )}
    </>
  );
}
