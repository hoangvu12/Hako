import { useState } from "react";
import {
  Crosshair,
  GameController,
  CaretDown,
  Check,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Switch } from "@/components/ui/switch";
import { Slider } from "@/components/ui/slider";
import { SectionHero, Panel, PresetCard } from "@/components/settings/primitives";
import {
  CAPTURE_MODES,
  GAME_MODE_LABELS,
  EVENT_LABELS,
  LOL_EVENT_LABELS,
  REMATCH_EVENT_LABELS,
  MAX_BEFORE_SECS,
  MAX_AFTER_SECS,
  type SettingsSet,
} from "@/components/settings/config";
import { useValorantAssets } from "@/hooks/use-valorant-assets";
import { GAMES, gameMeta, type GameId, type GameMeta } from "@/games/registry";
import type {
  AutoCaptureMode,
  EventTiming,
  EventToggles,
  GameModeToggles,
  LolEventToggles,
  RematchEventToggles,
  Settings,
} from "@/lib/api";

/* -------------------------------------------------------------------------- */
/* Per-game auto descriptor — the modular seam                                */
/* -------------------------------------------------------------------------- */

/** One auto-clip event row (label list entry), game-agnostic. */
interface EventDef {
  key: string;
  label: string;
  hint: string;
  icon: Icon;
}

/** Valorant-only per-game-mode gating. */
interface GameModeModel {
  labels: typeof GAME_MODE_LABELS;
  enabled: (key: keyof GameModeToggles) => boolean;
  toggle: (key: keyof GameModeToggles) => void;
  iconFor: (art?: string) => string | undefined;
}

/**
 * Everything the generic `GameAutoCard` needs to render + edit one game's
 * auto-capture config, regardless of where that config lives in `Settings`
 * (Valorant in flat fields, League under `games.lol`). Each game builds one of
 * these from the registry meta + the handlers `AutoSection` already receives, so
 * the card UI stays game-agnostic. Adding a game = one more descriptor builder.
 */
interface GameAutoModel {
  meta: GameMeta;
  mode: AutoCaptureMode;
  setMode: (mode: AutoCaptureMode) => void;
  /** Master "capture this game at all" flag. When true, Hako never attaches to
   * the game — no buffer, no auto-record (independent of `mode`). */
  disabled: boolean;
  setDisabled: (disabled: boolean) => void;
  events: EventDef[];
  enabled: (key: string) => boolean;
  toggleEvent: (key: string) => void;
  timing: (key: string) => EventTiming;
  setTimingLocal: (key: string, field: "before" | "after", value: number) => void;
  commitTiming: (key: string, field: "before" | "after", value: number) => void;
  /** Present only for games with per-mode gating (Valorant). */
  gameModes?: GameModeModel;
}

/* -------------------------------------------------------------------------- */
/* Shared pieces                                                              */
/* -------------------------------------------------------------------------- */

/** Game logo image with a Phosphor glyph fallback if the asset fails to load. */
function GameLogo({ meta, className }: { meta: GameMeta; className?: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return <meta.Icon className={className} weight="fill" />;
  return (
    <img
      src={meta.logo}
      alt=""
      draggable={false}
      onError={() => setFailed(true)}
      className={className}
    />
  );
}

/** A per-event clip-window editor laid out like Outplayed's "Events timing". */
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

/** Mode-card grid shared by both games. */
function ModeCards({
  value,
  onSelect,
}: {
  value: AutoCaptureMode;
  onSelect: (mode: AutoCaptureMode) => void;
}) {
  return (
    <Panel title="Mode">
      <div className="grid grid-cols-2 gap-3 pt-1">
        {CAPTURE_MODES.map((m) => (
          <PresetCard
            key={m.key}
            title={m.label}
            blurb={m.blurb}
            selected={value === m.key}
            onSelect={() => onSelect(m.key)}
          />
        ))}
      </div>
    </Panel>
  );
}

/** A compact Medal-style event toggle: checkbox + icon + label in a grid cell. */
function EventCheck({
  icon: Icon,
  label,
  hint,
  on,
  onToggle,
}: {
  icon: Icon;
  label: string;
  hint: string;
  on: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      title={hint}
      className={cn(
        "flex items-center gap-2.5 rounded-lg border px-3 py-2.5 text-left transition-colors",
        on
          ? "border-primary/40 bg-primary/10"
          : "border-border/60 bg-card/30 hover:border-border hover:bg-accent/40"
      )}
    >
      <span
        className={cn(
          "flex size-4 shrink-0 items-center justify-center rounded border",
          on
            ? "border-primary bg-primary text-primary-foreground"
            : "border-muted-foreground/40 bg-white/[0.03]"
        )}
      >
        {on ? <Check weight="bold" className="size-3" /> : null}
      </span>
      <Icon className="size-4 shrink-0 text-muted-foreground" weight="fill" />
      <span className="min-w-0 truncate text-sm font-medium">{label}</span>
    </button>
  );
}

/* -------------------------------------------------------------------------- */
/* Per-game card                                                             */
/* -------------------------------------------------------------------------- */

/**
 * One game's auto-capture card (Medal "Auto Clipping Games" layout): logo + name
 * + a master ON/OFF switch in the header, with the full config (mode, per-mode
 * gating, events, advanced timing) in the expandable body. The master switch is
 * the per-game `disabled` flag: OFF ⇒ Hako ignores the game entirely (no buffer,
 * no auto-record); ON ⇒ Hako manages it per the selected mode. "Manual only"
 * (buffer + save-hotkey, no auto-clips) is just the first mode card — distinct
 * from the master switch being off.
 */
function GameAutoCard({ model }: { model: GameAutoModel }) {
  const { meta } = model;
  const enabled = !model.disabled;
  const [open, setOpen] = useState(enabled);
  const [showTiming, setShowTiming] = useState(false);

  const toggleEnabled = () => {
    const next = !enabled;
    model.setDisabled(!next);
    if (next) setOpen(true);
  };

  const showGameModes =
    model.gameModes && (model.mode === "highlights" || model.mode === "full_match");
  const showEvents = model.mode === "highlights";

  return (
    <section
      className={cn(
        "overflow-hidden rounded-xl border transition-colors",
        enabled ? "border-border/70 bg-card/40" : "border-border/50 bg-card/20"
      )}
    >
      {/* Header — always visible. */}
      <div className="flex items-center gap-3 p-4">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-3 text-left"
        >
          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-secondary/60">
            <GameLogo meta={meta} className="size-6 object-contain" />
          </span>
          <span className="min-w-0">
            <span className="block truncate text-sm font-semibold">{meta.label}</span>
            <span className="block truncate text-xs text-muted-foreground">
              {enabled
                ? CAPTURE_MODES.find((m) => m.key === model.mode)?.label ?? "On"
                : "Off — not captured"}
            </span>
          </span>
          {enabled && (
            <CaretDown
              weight="bold"
              className={cn(
                "ml-1 size-4 text-muted-foreground transition-transform",
                open ? "rotate-0" : "-rotate-90"
              )}
            />
          )}
        </button>
        <Switch checked={enabled} onCheckedChange={toggleEnabled} />
      </div>

      {/* Body — config, shown when expanded and the game is enabled. */}
      {open && enabled && (
        <div className="flex flex-col gap-4 border-t border-border/60 p-4">
          <ModeCards value={model.mode} onSelect={model.setMode} />

          {showGameModes && model.gameModes && (
            <Panel title="Game modes">
              <p className="-mt-1 pb-3 text-xs text-muted-foreground">
                Only auto-capture matches in the modes you turn on. Manual saves and
                Full session recording aren't affected.
              </p>
              {model.gameModes.labels.map((gm) => {
                const icon = model.gameModes!.iconFor(gm.art);
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
                        <p className="mt-0.5 text-xs text-muted-foreground">{gm.hint}</p>
                      </div>
                    </div>
                    <Switch
                      checked={model.gameModes!.enabled(gm.key)}
                      onCheckedChange={() => model.gameModes!.toggle(gm.key)}
                    />
                  </div>
                );
              })}
            </Panel>
          )}

          {showEvents && (
            <>
              <Panel title="Auto captured events">
                <div className="grid grid-cols-2 gap-2 pt-1">
                  {model.events.map((ev) => (
                    <EventCheck
                      key={ev.key}
                      icon={ev.icon}
                      label={ev.label}
                      hint={ev.hint}
                      on={model.enabled(ev.key)}
                      onToggle={() => model.toggleEvent(ev.key)}
                    />
                  ))}
                </div>
              </Panel>

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
                    {model.events
                      .filter((ev) => model.enabled(ev.key))
                      .map((ev) => {
                        const t = model.timing(ev.key);
                        return (
                          <TimingRow
                            key={ev.key}
                            icon={ev.icon}
                            label={ev.label}
                            before={t.before}
                            after={t.after}
                            onChange={(field, value) =>
                              model.setTimingLocal(ev.key, field, value)
                            }
                            onCommit={(field, value) =>
                              model.commitTiming(ev.key, field, value)
                            }
                          />
                        );
                      })}
                    {model.events.every((ev) => !model.enabled(ev.key)) && (
                      <p className="py-3 text-xs text-muted-foreground">
                        Enable an event above to set its clip timing.
                      </p>
                    )}
                  </div>
                )}
              </Panel>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/* -------------------------------------------------------------------------- */
/* Section                                                                   */
/* -------------------------------------------------------------------------- */

export function AutoSection({
  draft,
  set,
  toggleEvent,
  toggleGameMode,
  setTimingLocal,
  commitTiming,
  setLolMode,
  setLolDisabled,
  toggleLolEvent,
  setLolTimingLocal,
  commitLolTiming,
  setRematchMode,
  setRematchDisabled,
  toggleRematchEvent,
  setRematchTimingLocal,
  commitRematchTiming,
}: {
  draft: Settings;
  set: SettingsSet;
  toggleEvent: (key: keyof EventToggles) => void;
  toggleGameMode: (key: keyof GameModeToggles) => void;
  setTimingLocal: (key: keyof EventToggles, field: "before" | "after", value: number) => void;
  commitTiming: (key: keyof EventToggles, field: "before" | "after", value: number) => void;
  setLolMode: (mode: AutoCaptureMode) => void;
  setLolDisabled: (disabled: boolean) => void;
  toggleLolEvent: (key: keyof LolEventToggles) => void;
  setLolTimingLocal: (key: keyof LolEventToggles, field: "before" | "after", value: number) => void;
  commitLolTiming: (key: keyof LolEventToggles, field: "before" | "after", value: number) => void;
  setRematchMode: (mode: AutoCaptureMode) => void;
  setRematchDisabled: (disabled: boolean) => void;
  toggleRematchEvent: (key: keyof RematchEventToggles) => void;
  setRematchTimingLocal: (
    key: keyof RematchEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  commitRematchTiming: (
    key: keyof RematchEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
}) {
  const assets = useValorantAssets();

  // Build one descriptor per game from the registry meta + the handlers passed
  // in, then render the registry in order. The only game-specific code lives in
  // these builders; the card UI above is fully generic.
  const lol = draft.games.lol;
  const rematch = draft.games.rematch;
  const models: Record<GameId, GameAutoModel> = {
    valorant: {
      meta: gameMeta("valorant"),
      mode: draft.auto_capture_mode,
      setMode: (m) => set("auto_capture_mode", m),
      disabled: draft.auto_capture_disabled,
      setDisabled: (v) => set("auto_capture_disabled", v),
      events: EVENT_LABELS,
      enabled: (k) => draft.events[k as keyof EventToggles],
      toggleEvent: (k) => toggleEvent(k as keyof EventToggles),
      timing: (k) => draft.event_timings[k as keyof EventToggles],
      setTimingLocal: (k, f, v) => setTimingLocal(k as keyof EventToggles, f, v),
      commitTiming: (k, f, v) => commitTiming(k as keyof EventToggles, f, v),
      gameModes: {
        labels: GAME_MODE_LABELS,
        enabled: (k) => draft.auto_clip_modes[k],
        toggle: toggleGameMode,
        iconFor: (art) => (art ? assets.modeFor(art)?.icon : undefined),
      },
    },
    lol: {
      meta: gameMeta("lol"),
      mode: lol.auto_capture_mode,
      setMode: setLolMode,
      disabled: lol.disabled,
      setDisabled: setLolDisabled,
      events: LOL_EVENT_LABELS,
      enabled: (k) => lol.events[k as keyof LolEventToggles],
      toggleEvent: (k) => toggleLolEvent(k as keyof LolEventToggles),
      timing: (k) => lol.event_timings[k as keyof LolEventToggles],
      setTimingLocal: (k, f, v) => setLolTimingLocal(k as keyof LolEventToggles, f, v),
      commitTiming: (k, f, v) => commitLolTiming(k as keyof LolEventToggles, f, v),
    },
    rematch: {
      meta: gameMeta("rematch"),
      mode: rematch.auto_capture_mode,
      setMode: setRematchMode,
      disabled: rematch.disabled,
      setDisabled: setRematchDisabled,
      events: REMATCH_EVENT_LABELS,
      enabled: (k) => rematch.events[k as keyof RematchEventToggles],
      toggleEvent: (k) => toggleRematchEvent(k as keyof RematchEventToggles),
      timing: (k) => rematch.event_timings[k as keyof RematchEventToggles],
      setTimingLocal: (k, f, v) => setRematchTimingLocal(k as keyof RematchEventToggles, f, v),
      commitTiming: (k, f, v) => commitRematchTiming(k as keyof RematchEventToggles, f, v),
    },
  };

  return (
    <>
      <SectionHero
        icon={Crosshair}
        title="Auto-Capture"
        subtitle="Choose which moments are clipped automatically, per game."
      />

      <div className="flex flex-col gap-3">
        {GAMES.map((g) => (
          <GameAutoCard key={g.id} model={models[g.id]} />
        ))}
      </div>
    </>
  );
}
