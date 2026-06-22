import { Check } from "@phosphor-icons/react";

import { Panel, Row } from "@/components/settings/primitives";
import type { Settings } from "@/lib/api";
import { CAPTURE_MODES } from "../config";

export function DoneStep({ draft }: { draft: Settings }) {
  return (
    <>
      {/* Bespoke (not SectionHero) so the check can pop in as a small reward on
          completion. */}
      <div className="flex flex-col items-center text-center">
        <div className="mb-3 flex size-16 items-center justify-center rounded-2xl bg-primary/15 text-primary-text duration-500 animate-in zoom-in-50">
          <Check
            weight="bold"
            className="size-8 delay-150 duration-500 animate-in zoom-in-0 fill-mode-both"
          />
        </div>
        <h1 className="text-xl font-semibold tracking-tight duration-500 animate-in fade-in slide-in-from-bottom-2">
          You're all set
        </h1>
        <p className="mt-1 max-w-md text-sm text-muted-foreground duration-700 animate-in fade-in">
          Hako is ready. Launch Valorant and your moments will be captured
          automatically.
        </p>
      </div>
      <Panel title="Your setup">
        <Row label="Clip folder">
          <span className="text-sm text-muted-foreground">
            {draft.storage_dir || "Videos/Hako"}
          </span>
        </Row>
        <Row label="Quality">
          <span className="text-sm text-muted-foreground capitalize">
            {draft.quality_preset}
          </span>
        </Row>
        <Row label="Save-clip hotkey">
          <span className="text-sm text-muted-foreground">{draft.save_hotkey}</span>
        </Row>
        <Row label="Auto-capture">
          <span className="text-sm text-muted-foreground">
            {CAPTURE_MODES.find((m) => m.key === draft.auto_capture_mode)?.label ??
              draft.auto_capture_mode}
          </span>
        </Row>
      </Panel>
    </>
  );
}
