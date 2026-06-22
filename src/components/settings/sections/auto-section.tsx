import { useState } from "react";
import {
  Crosshair,
  GameController,
  CaretDown,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Switch } from "@/components/ui/switch";
import { Slider } from "@/components/ui/slider";
import { SectionHero, Panel, Row, PresetCard } from "@/components/settings/primitives";
import {
  CAPTURE_MODES,
  GAME_MODE_LABELS,
  EVENT_LABELS,
  MAX_BEFORE_SECS,
  MAX_AFTER_SECS,
  type SettingsSet,
} from "@/components/settings/config";
import { useValorantAssets } from "@/hooks/use-valorant-assets";
import type { EventToggles, GameModeToggles, Settings } from "@/lib/api";

/** A per-event clip-window editor laid out like Outplayed's "Events timing":
 *  the before value, a slider that fills inward from the left, the event icon,
 *  a slider that fills outward to the right, and the after value. Dragging
 *  updates the draft live (`onChange`); the release commits it (`onCommit`). */
function TimingRow({
  icon: Icon,
  label,
  before,
  after,
  onChange,
  onCommit,
}: {
  icon: Icon;
  label: string;
  before: number;
  after: number;
  onChange: (field: "before" | "after", value: number) => void;
  onCommit: (field: "before" | "after", value: number) => void;
}) {
  return (
    <div className="flex items-center gap-3 py-2.5">
      <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
        {before}s
      </span>
      {/* Before: inverted so the fill grows from the centre icon leftwards. */}
      <Slider
        inverted
        aria-label={`${label} seconds before`}
        min={0}
        max={MAX_BEFORE_SECS}
        step={1}
        value={[before]}
        onValueChange={(v) => onChange("before", v[0] ?? 0)}
        onValueCommit={(v) => onCommit("before", v[0] ?? 0)}
        className="flex-1"
      />
      <div className="flex w-24 shrink-0 flex-col items-center gap-1">
        <div className="flex size-9 items-center justify-center rounded-md border border-border/70 bg-secondary text-foreground">
          <Icon className="size-4" weight="fill" />
        </div>
        <span className="text-center text-[11px] leading-tight font-medium text-muted-foreground">
          {label}
        </span>
      </div>
      {/* After: normal direction, fill grows from the centre icon rightwards. */}
      <Slider
        aria-label={`${label} seconds after`}
        min={0}
        max={MAX_AFTER_SECS}
        step={1}
        value={[after]}
        onValueChange={(v) => onChange("after", v[0] ?? 0)}
        onValueCommit={(v) => onCommit("after", v[0] ?? 0)}
        className="flex-1"
      />
      <span className="w-9 shrink-0 text-xs tabular-nums text-muted-foreground">
        {after}s
      </span>
    </div>
  );
}

export function AutoSection({
  draft,
  set,
  toggleEvent,
  toggleGameMode,
  setTimingLocal,
  commitTiming,
}: {
  draft: Settings;
  set: SettingsSet;
  toggleEvent: (key: keyof EventToggles) => void;
  toggleGameMode: (key: keyof GameModeToggles) => void;
  setTimingLocal: (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number,
  ) => void;
  commitTiming: (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number,
  ) => void;
}) {
  // Valorant gamemode artwork for the per-mode toggle rows (icons load lazily;
  // rows fall back to a label-only look until the asset query resolves).
  const assets = useValorantAssets();
  // Outplayed-style "Advanced options" disclosure for per-event timing.
  const [showTiming, setShowTiming] = useState(false);

  return (
    <>
      <SectionHero
        icon={Crosshair}
        title="Auto-Capture"
        subtitle="Choose which Valorant moments are clipped automatically."
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

      {/* Per-game-mode gate. Applies to the per-match recording modes
          (Highlights / Full match); Manual and Full session ignore it. */}
      {(draft.auto_capture_mode === "highlights" ||
        draft.auto_capture_mode === "full_match") && (
        <Panel title="Game modes">
          <p className="-mt-1 pb-3 text-xs text-muted-foreground">
            Only auto-capture matches in the modes you turn on. Manual
            saves and Full session recording aren't affected.
          </p>
          {GAME_MODE_LABELS.map((gm) => {
            const icon = gm.art
              ? assets.modeFor(gm.art)?.icon
              : undefined;
            return (
              <div
                key={gm.key}
                className="flex items-center justify-between gap-6 py-4 last:pb-0"
              >
                <div className="flex min-w-0 items-center gap-3">
                  {icon ? (
                    <img
                      src={icon}
                      alt=""
                      className="size-7 shrink-0 rounded object-contain"
                    />
                  ) : (
                    <span className="flex size-7 shrink-0 items-center justify-center rounded bg-secondary/60 text-muted-foreground">
                      <GameController className="size-4" />
                    </span>
                  )}
                  <div className="min-w-0">
                    <div className="text-sm font-medium">{gm.label}</div>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      {gm.hint}
                    </p>
                  </div>
                </div>
                <Switch
                  checked={draft.auto_clip_modes[gm.key]}
                  onCheckedChange={() => toggleGameMode(gm.key)}
                />
              </div>
            );
          })}
        </Panel>
      )}

      {/* Events + timing only matter when auto-clipping highlights. */}
      {draft.auto_capture_mode === "highlights" && (
        <>
          <Panel title="Auto captured events">
            {EVENT_LABELS.map((ev) => (
              <Row key={ev.key} label={ev.label} hint={ev.hint}>
                <Switch
                  checked={draft.events[ev.key]}
                  onCheckedChange={() => toggleEvent(ev.key)}
                />
              </Row>
            ))}
          </Panel>

          {/* Advanced options: per-event clip windows (Outplayed's
              "Events timing"). Only the enabled events are shown. */}
          <Panel>
            <button
              type="button"
              onClick={() => setShowTiming((v) => !v)}
              className="flex w-full items-center gap-2 text-sm font-semibold text-foreground"
            >
              <CaretDown
                weight="bold"
                className={cn(
                  "size-4 transition-transform",
                  showTiming ? "rotate-0" : "-rotate-90"
                )}
              />
              Advanced options
              <span className="ml-auto text-xs font-normal text-muted-foreground">
                Events timing
              </span>
            </button>
            {showTiming && (
              <div className="pt-3">
                {EVENT_LABELS.filter((ev) => draft.events[ev.key]).map(
                  (ev) => (
                    <TimingRow
                      key={ev.key}
                      icon={ev.icon}
                      label={ev.label}
                      before={draft.event_timings[ev.key].before}
                      after={draft.event_timings[ev.key].after}
                      onChange={(field, value) =>
                        setTimingLocal(ev.key, field, value)
                      }
                      onCommit={(field, value) =>
                        commitTiming(ev.key, field, value)
                      }
                    />
                  )
                )}
                {EVENT_LABELS.every((ev) => !draft.events[ev.key]) && (
                  <p className="py-3 text-xs text-muted-foreground">
                    Enable an event above to set its clip timing.
                  </p>
                )}
                <p className="pt-2 text-xs text-muted-foreground">
                  Seconds kept before and after each moment. The save-clip
                  hotkey uses its own padding (Clip Settings).
                </p>
              </div>
            )}
          </Panel>
        </>
      )}
    </>
  );
}
