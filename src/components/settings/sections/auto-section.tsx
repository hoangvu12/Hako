import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Crosshair,
  GameController,
  CaretDown,
  Check,
  Trash,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Switch } from "@/components/ui/switch";
import { Slider } from "@/components/ui/slider";
import { SectionHero, Panel, PresetCard } from "@/components/settings/primitives";
import { RequestGameDialog } from "@/components/settings/sections/request-game-dialog";
import {
  CAPTURE_MODES,
  GAME_MODE_LABELS,
  EVENT_LABELS,
  LOL_EVENT_LABELS,
  REMATCH_EVENT_LABELS,
  CS2_EVENT_LABELS,
  DOTA2_EVENT_LABELS,
  WARTHUNDER_EVENT_LABELS,
  PUBG_EVENT_LABELS,
  MAX_BEFORE_SECS,
  MAX_AFTER_SECS,
  type SettingsSet,
} from "@/components/settings/config";
import { useValorantAssets } from "@/hooks/use-valorant-assets";
import { GAMES, gameMeta, type GameId, type GameMeta } from "@/games/registry";
import {
  listCustomGames,
  removeCustomGame,
  setCustomGameEnabled,
  type CustomGame,
} from "@/lib/api";
import type {
  AutoCaptureMode,
  Cs2EventToggles,
  Dota2EventToggles,
  WarThunderEventToggles,
  PubgEventToggles,
  EventTiming,
  EventToggles,
  GameModeToggles,
  LolEventToggles,
  OtherGamesSettings,
  RematchEventToggles,
  Settings,
} from "@/lib/api";

/** The generic bucket has no event feed, so Highlights is omitted from its modes. */
const OTHER_CAPTURE_MODES = CAPTURE_MODES.filter((m) => m.key !== "highlights");

/** The smart games (everything but the generic "other" bucket), in registry order. */
const SMART_GAMES = GAMES.filter((g) => g.id !== "other");
type SmartGameId = Exclude<GameId, "other">;

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
  /** Present only for games that need a free-text field to attribute events to
   * the local player (War Thunder's in-game nickname). */
  nickname?: NicknameModel;
}

/** A required free-text identity field (War Thunder's nickname). */
interface NicknameModel {
  label: string;
  hint: string;
  placeholder: string;
  value: string;
  /** Local edit (per keystroke). */
  onChange: (value: string) => void;
  /** Persist (on blur). */
  onCommit: (value: string) => void;
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

/** Mode-card grid shared by every game (generic passes a filtered mode list). */
function ModeCards({
  value,
  onSelect,
  modes = CAPTURE_MODES,
}: {
  value: AutoCaptureMode;
  onSelect: (mode: AutoCaptureMode) => void;
  modes?: typeof CAPTURE_MODES;
}) {
  return (
    <Panel title="Mode">
      <div className="grid grid-cols-2 gap-3 pt-1">
        {modes.map((m) => (
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

/** A game's required identity field (War Thunder's nickname). Edits apply
 * locally per keystroke and persist on blur — the same instant-apply/commit split
 * the timing sliders use. */
function NicknameField({ nickname }: { nickname: NicknameModel }) {
  return (
    <Panel title={nickname.label}>
      <p className="-mt-1 pb-2 text-xs text-muted-foreground">{nickname.hint}</p>
      <input
        type="text"
        value={nickname.value}
        placeholder={nickname.placeholder}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        onChange={(e) => nickname.onChange(e.target.value)}
        onBlur={(e) => nickname.onCommit(e.target.value)}
        className="w-full rounded-lg border border-border/70 bg-secondary/40 px-3 py-2 text-sm text-foreground outline-none transition-colors placeholder:text-muted-foreground focus:border-primary/50"
      />
    </Panel>
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

          {model.nickname && <NicknameField nickname={model.nickname} />}

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
/* Other Games (generic "record any game") card                              */
/* -------------------------------------------------------------------------- */

/** A custom game's captured exe icon, with the generic glyph as a fallback (no
 * icon captured, or the data URL fails to decode). */
function CustomGameIcon({ icon }: { icon: string | null }) {
  const [failed, setFailed] = useState(false);
  const box =
    "flex size-8 shrink-0 items-center justify-center rounded bg-secondary/60 text-muted-foreground";
  if (!icon || failed) {
    return (
      <span className={box}>
        <GameController className="size-4" />
      </span>
    );
  }
  return (
    <span className={box}>
      <img
        src={icon}
        alt=""
        draggable={false}
        onError={() => setFailed(true)}
        className="size-5 object-contain"
      />
    </span>
  );
}

/** A labeled on/off row (used for the generic detection toggles). */
function ToggleRow({
  label,
  hint,
  checked,
  onCheckedChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-6 py-3 first:pt-1 last:pb-0">
      <div className="min-w-0">
        <div className="text-sm font-medium">{label}</div>
        <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>
      </div>
      <Switch checked={checked} onCheckedChange={onCheckedChange} />
    </div>
  );
}

/**
 * The generic "record any game" card. Unlike the smart-game cards it has no
 * events — just a capture mode (Manual / Full session / Full match), the
 * auto-detection toggles, and the managed list of user-added games with a
 * "+ Add a game" picker (Medal's Request-a-Game). Detected games record
 * generically and are tagged with their real title.
 */
function OtherGamesCard({
  other,
  setMode,
  setDisabled,
  setDetect,
}: {
  other: OtherGamesSettings;
  setMode: (mode: AutoCaptureMode) => void;
  setDisabled: (disabled: boolean) => void;
  setDetect: (key: "detect_steam" | "detect_curated", value: boolean) => void;
}) {
  const meta = gameMeta("other");
  const enabled = !other.disabled;
  const [open, setOpen] = useState(enabled);
  const qc = useQueryClient();

  const { data: games = [] } = useQuery({
    queryKey: ["custom-games"],
    queryFn: listCustomGames,
  });
  const refresh = () => qc.invalidateQueries({ queryKey: ["custom-games"] });
  const toggle = useMutation({
    mutationFn: ({ id, on }: { id: number; on: boolean }) =>
      setCustomGameEnabled(id, on),
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: (id: number) => removeCustomGame(id),
    onSettled: refresh,
  });

  const toggleEnabled = () => {
    const next = !enabled;
    setDisabled(!next);
    if (next) setOpen(true);
  };

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
                ? OTHER_CAPTURE_MODES.find((m) => m.key === other.auto_capture_mode)
                    ?.label ?? "On"
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

      {/* Body — config, shown when expanded and enabled. */}
      {open && enabled && (
        <div className="flex flex-col gap-4 border-t border-border/60 p-4">
          <ModeCards
            value={other.auto_capture_mode}
            onSelect={setMode}
            modes={OTHER_CAPTURE_MODES}
          />

          <Panel title="Auto-detect games">
            <ToggleRow
              label="Detect Steam games automatically"
              hint="Recognize any game launched from your Steam library"
              checked={other.detect_steam}
              onCheckedChange={(v) => setDetect("detect_steam", v)}
            />
            <ToggleRow
              label="Detect known games"
              hint="Recognize popular non-Steam games (Fortnite, Apex, Roblox…)"
              checked={other.detect_curated}
              onCheckedChange={(v) => setDetect("detect_curated", v)}
            />
          </Panel>

          <Panel title="Your games">
            <p className="-mt-1 pb-2 text-xs text-muted-foreground">
              Add any game not detected automatically — point Hako at its window
              once and it auto-records from then on.
            </p>
            <div className="flex flex-col gap-1.5">
              {games.map((g: CustomGame) => (
                <div
                  key={g.id}
                  className="flex items-center gap-3 rounded-lg border border-border/60 bg-card/30 px-3 py-2"
                >
                  <CustomGameIcon icon={g.icon} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">
                      {g.display_name}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {g.process_name}
                    </span>
                  </span>
                  <Switch
                    checked={g.enabled}
                    onCheckedChange={(on) => toggle.mutate({ id: g.id, on })}
                  />
                  <button
                    type="button"
                    aria-label={`Remove ${g.display_name}`}
                    onClick={() => remove.mutate(g.id)}
                    className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                  >
                    <Trash className="size-4" />
                  </button>
                </div>
              ))}
            </div>
            <div className="pt-3">
              <RequestGameDialog onAdded={refresh} />
            </div>
          </Panel>
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
  setCs2Mode,
  setCs2Disabled,
  toggleCs2Event,
  setCs2TimingLocal,
  commitCs2Timing,
  setDota2Mode,
  setDota2Disabled,
  toggleDota2Event,
  setDota2TimingLocal,
  commitDota2Timing,
  setWarThunderMode,
  setWarThunderDisabled,
  setWarThunderNickname,
  commitWarThunderNickname,
  toggleWarThunderEvent,
  setWarThunderTimingLocal,
  commitWarThunderTiming,
  setPubgMode,
  setPubgDisabled,
  togglePubgEvent,
  setPubgTimingLocal,
  commitPubgTiming,
  setOtherMode,
  setOtherDisabled,
  setOtherDetect,
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
  setCs2Mode: (mode: AutoCaptureMode) => void;
  setCs2Disabled: (disabled: boolean) => void;
  toggleCs2Event: (key: keyof Cs2EventToggles) => void;
  setCs2TimingLocal: (
    key: keyof Cs2EventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  commitCs2Timing: (
    key: keyof Cs2EventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  setDota2Mode: (mode: AutoCaptureMode) => void;
  setDota2Disabled: (disabled: boolean) => void;
  toggleDota2Event: (key: keyof Dota2EventToggles) => void;
  setDota2TimingLocal: (
    key: keyof Dota2EventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  commitDota2Timing: (
    key: keyof Dota2EventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  setWarThunderMode: (mode: AutoCaptureMode) => void;
  setWarThunderDisabled: (disabled: boolean) => void;
  setWarThunderNickname: (nickname: string) => void;
  commitWarThunderNickname: (nickname: string) => void;
  toggleWarThunderEvent: (key: keyof WarThunderEventToggles) => void;
  setWarThunderTimingLocal: (
    key: keyof WarThunderEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  commitWarThunderTiming: (
    key: keyof WarThunderEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  setPubgMode: (mode: AutoCaptureMode) => void;
  setPubgDisabled: (disabled: boolean) => void;
  togglePubgEvent: (key: keyof PubgEventToggles) => void;
  setPubgTimingLocal: (
    key: keyof PubgEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  commitPubgTiming: (
    key: keyof PubgEventToggles,
    field: "before" | "after",
    value: number
  ) => void;
  setOtherMode: (mode: AutoCaptureMode) => void;
  setOtherDisabled: (disabled: boolean) => void;
  setOtherDetect: (key: "detect_steam" | "detect_curated", value: boolean) => void;
}) {
  const assets = useValorantAssets();

  // Build one descriptor per game from the registry meta + the handlers passed
  // in, then render the registry in order. The only game-specific code lives in
  // these builders; the card UI above is fully generic.
  const lol = draft.games.lol;
  const rematch = draft.games.rematch;
  const cs2 = draft.games.cs2;
  const dota2 = draft.games.dota2;
  const warthunder = draft.games.warthunder;
  const pubg = draft.games.pubg;
  const models: Record<SmartGameId, GameAutoModel> = {
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
    cs2: {
      meta: gameMeta("cs2"),
      mode: cs2.auto_capture_mode,
      setMode: setCs2Mode,
      disabled: cs2.disabled,
      setDisabled: setCs2Disabled,
      events: CS2_EVENT_LABELS,
      enabled: (k) => cs2.events[k as keyof Cs2EventToggles],
      toggleEvent: (k) => toggleCs2Event(k as keyof Cs2EventToggles),
      timing: (k) => cs2.event_timings[k as keyof Cs2EventToggles],
      setTimingLocal: (k, f, v) => setCs2TimingLocal(k as keyof Cs2EventToggles, f, v),
      commitTiming: (k, f, v) => commitCs2Timing(k as keyof Cs2EventToggles, f, v),
    },
    dota2: {
      meta: gameMeta("dota2"),
      mode: dota2.auto_capture_mode,
      setMode: setDota2Mode,
      disabled: dota2.disabled,
      setDisabled: setDota2Disabled,
      events: DOTA2_EVENT_LABELS,
      enabled: (k) => dota2.events[k as keyof Dota2EventToggles],
      toggleEvent: (k) => toggleDota2Event(k as keyof Dota2EventToggles),
      timing: (k) => dota2.event_timings[k as keyof Dota2EventToggles],
      setTimingLocal: (k, f, v) => setDota2TimingLocal(k as keyof Dota2EventToggles, f, v),
      commitTiming: (k, f, v) => commitDota2Timing(k as keyof Dota2EventToggles, f, v),
    },
    warthunder: {
      meta: gameMeta("warthunder"),
      mode: warthunder.auto_capture_mode,
      setMode: setWarThunderMode,
      disabled: warthunder.disabled,
      setDisabled: setWarThunderDisabled,
      events: WARTHUNDER_EVENT_LABELS,
      enabled: (k) => warthunder.events[k as keyof WarThunderEventToggles],
      toggleEvent: (k) => toggleWarThunderEvent(k as keyof WarThunderEventToggles),
      timing: (k) => warthunder.event_timings[k as keyof WarThunderEventToggles],
      setTimingLocal: (k, f, v) =>
        setWarThunderTimingLocal(k as keyof WarThunderEventToggles, f, v),
      commitTiming: (k, f, v) =>
        commitWarThunderTiming(k as keyof WarThunderEventToggles, f, v),
      nickname: {
        label: "In-game nickname",
        hint: "War Thunder's combat log is free text, so Hako matches this name to know which kills and deaths are yours. Enter it exactly as it appears in-game.",
        placeholder: "Your War Thunder nickname",
        value: warthunder.nickname,
        onChange: setWarThunderNickname,
        onCommit: commitWarThunderNickname,
      },
    },
    pubg: {
      meta: gameMeta("pubg"),
      mode: pubg.auto_capture_mode,
      setMode: setPubgMode,
      disabled: pubg.disabled,
      setDisabled: setPubgDisabled,
      events: PUBG_EVENT_LABELS,
      enabled: (k) => pubg.events[k as keyof PubgEventToggles],
      toggleEvent: (k) => togglePubgEvent(k as keyof PubgEventToggles),
      timing: (k) => pubg.event_timings[k as keyof PubgEventToggles],
      setTimingLocal: (k, f, v) => setPubgTimingLocal(k as keyof PubgEventToggles, f, v),
      commitTiming: (k, f, v) => commitPubgTiming(k as keyof PubgEventToggles, f, v),
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
        {SMART_GAMES.map((g) => (
          <GameAutoCard key={g.id} model={models[g.id as SmartGameId]} />
        ))}
        <OtherGamesCard
          other={draft.games.other}
          setMode={setOtherMode}
          setDisabled={setOtherDisabled}
          setDetect={setOtherDetect}
        />
      </div>
    </>
  );
}
