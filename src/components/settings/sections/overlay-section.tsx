import { Bell } from "@phosphor-icons/react";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import { OVERLAY_POSITIONS, type SettingsSet } from "@/components/settings/config";
import { overlayTest, type Settings } from "@/lib/api";

export function OverlaySection({ draft, set }: { draft: Settings; set: SettingsSet }) {
  return (
    <>
      <SectionHero
        icon={Bell}
        title="Overlay"
        subtitle="In-game toasts for capture state, saved clips, and low disk space."
      />
      <Panel title="In-game overlay">
        <Row
          label="Show overlay"
          hint="Pop toasts over the game while recording. Off hides them entirely."
        >
          <Switch
            checked={draft.overlay_enabled}
            onCheckedChange={(v) => set("overlay_enabled", v)}
          />
        </Row>
        <Row
          label="Position"
          hint="Which corner the toasts stack in. Applies the next time the overlay shows."
        >
          <Select
            value={draft.overlay_position}
            onValueChange={(v) => set("overlay_position", v as Settings["overlay_position"])}
            disabled={!draft.overlay_enabled}
          >
            <SelectTrigger size="sm" className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {OVERLAY_POSITIONS.map((p) => (
                <SelectItem key={p.value} value={p.value}>
                  {p.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row
          label="Test overlay"
          hint="Fire a sample toast over Valorant, or your primary screen if it isn't running."
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              void overlayTest();
            }}
          >
            Test overlay
          </Button>
        </Row>
      </Panel>

      <Panel title="Notifications">
        <Row
          label="Recording started & stopped"
          hint="'Now recording' when capture starts, 'Recording stopped' when it ends."
        >
          <Switch
            checked={draft.overlay_on_capture_state}
            disabled={!draft.overlay_enabled}
            onCheckedChange={(v) => set("overlay_on_capture_state", v)}
          />
        </Row>
        <Row label="Clip saved" hint="When you save a clip with the hotkey or the save button.">
          <Switch
            checked={draft.overlay_on_clip_saved}
            disabled={!draft.overlay_enabled}
            onCheckedChange={(v) => set("overlay_on_clip_saved", v)}
          />
        </Row>
        <Row label="Storage almost full" hint="When the clips drive drops below 5 GB free.">
          <Switch
            checked={draft.overlay_on_disk_low}
            disabled={!draft.overlay_enabled}
            onCheckedChange={(v) => set("overlay_on_disk_low", v)}
          />
        </Row>
      </Panel>

      <p className="px-1 text-xs text-muted-foreground">
        Overlays show in Borderless and most Fullscreen modes. If they don't appear in exclusive
        fullscreen, switch Valorant to Borderless.
      </p>
    </>
  );
}
