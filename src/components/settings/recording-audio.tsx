import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Microphone,
  SpeakerHigh,
  GameController,
  Waveform,
  Flask,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Switch } from "@/components/ui/switch";
import { Slider } from "@/components/ui/slider";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  listAudioInputs,
  listAudioOutputs,
  listActiveAudioSessions,
  processLoopbackSupported,
  AUTO_DEVICE,
  GAME_SOURCE_ID,
  type AudioConfig,
  type AudioAppSel,
} from "@/lib/api";

/**
 * Processes never offered as a generic app source: system mixers + Hako, plus
 * the Valorant game process itself — it's already represented by the dedicated
 * "Game Audio" row, so listing it here would duplicate the game.
 */
const SESSION_BLACKLIST = new Set([
  "svchost.exe",
  "audiodg.exe",
  "hako.exe",
  "valorant-win64-shipping.exe",
]);

function Panel({
  title,
  hint,
  children,
}: {
  title?: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-xl border border-border/70 bg-card/40 p-5">
      {title && (
        <h2 className="text-sm font-semibold text-foreground">{title}</h2>
      )}
      {hint && <p className="mt-0.5 mb-1 text-xs text-muted-foreground">{hint}</p>}
      <div className="mt-2 divide-y divide-border/60">{children}</div>
    </section>
  );
}

/**
 * A 0–100 volume slider with a live readout. Keeps a local value while dragging
 * (smooth), persisting only on release (`onValueCommit`) so we don't write
 * settings to disk on every pixel.
 */
function VolumeSlider({
  value,
  onCommit,
  disabled,
}: {
  value: number;
  onCommit: (v: number) => void;
  disabled?: boolean;
}) {
  // `local` is a transient drag override (null = not dragging): we read `value`
  // directly during render and only diverge while the user drags, releasing back
  // to the prop on commit. Nothing copies the prop into state and there's no
  // re-sync effect, so the readout never shows a stale value when the parent's
  // `value` changes.
  const [local, setLocal] = useState<number | null>(null);
  const shown = local ?? value;
  return (
    <div className="flex w-44 items-center gap-3">
      <Slider
        value={[shown]}
        min={0}
        max={100}
        step={1}
        disabled={disabled}
        onValueChange={([v]) => setLocal(v)}
        onValueCommit={([v]) => {
          setLocal(null);
          onCommit(v);
        }}
        aria-label="Volume"
      />
      <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
        {shown}%
      </span>
    </div>
  );
}

/** A source row: enable checkbox + icon + label, with a control on the right. */
function SourceRow({
  icon: Icon,
  iconUrl,
  label,
  hint,
  checked,
  onCheckedChange,
  disabled,
  children,
}: {
  icon: typeof SpeakerHigh;
  /** Real app icon (PNG data URL). When set, shown instead of `icon`. */
  iconUrl?: string | null;
  label: string;
  hint?: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
  disabled?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
      <Checkbox
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onCheckedChange(v === true)}
      />
      {iconUrl ? (
        <img
          src={iconUrl}
          alt=""
          className={cn(
            "size-4 shrink-0 rounded-[3px] object-contain",
            !checked && "opacity-60"
          )}
        />
      ) : (
        <Icon
          className={cn(
            "size-4 shrink-0",
            checked ? "text-primary-text" : "text-muted-foreground"
          )}
          weight="fill"
        />
      )}
      <div className="min-w-0 flex-1">
        <div
          className={cn(
            "truncate text-sm font-medium",
            !checked && "text-foreground/70"
          )}
        >
          {label}
        </div>
        {hint && (
          <p className="truncate text-xs text-muted-foreground">{hint}</p>
        )}
      </div>
      <div className={cn("shrink-0", !checked && "pointer-events-none opacity-40")}>
        {children}
      </div>
    </div>
  );
}

/** Upsert an app source by id, merging `patch` (creating it enabled if absent). */
function upsertApp(
  apps: AudioAppSel[],
  id: string,
  name: string,
  patch: Partial<AudioAppSel>
): AudioAppSel[] {
  const idx = apps.findIndex((a) => a.id === id);
  if (idx >= 0) {
    const next = [...apps];
    next[idx] = { ...next[idx], ...patch };
    return next;
  }
  return [...apps, { id, name, enabled: true, volume: 100, ...patch }];
}

export function RecordingAudio({
  audio,
  onChange,
}: {
  audio: AudioConfig;
  onChange: (next: AudioConfig) => void;
}) {
  const { data: outputs } = useQuery({
    queryKey: ["audio-outputs"],
    queryFn: listAudioOutputs,
    retry: false,
  });
  const { data: micDevices } = useQuery({
    queryKey: ["audio-inputs"],
    queryFn: listAudioInputs,
    retry: false,
  });
  const { data: supported } = useQuery({
    queryKey: ["process-loopback-supported"],
    queryFn: processLoopbackSupported,
    retry: false,
  });
  // Live "apps playing audio" list — only worth polling while in specific_apps.
  const { data: sessions } = useQuery({
    queryKey: ["audio-sessions"],
    queryFn: listActiveAudioSessions,
    retry: false,
    refetchInterval: audio.mode === "specific_apps" ? 3000 : false,
    enabled: audio.mode === "specific_apps",
  });

  const patch = (p: Partial<AudioConfig>) => onChange({ ...audio, ...p });

  // --- All PC audio: device multi-select --------------------------------------
  // Always lead with the synthetic "Default Output Device" (auto), then live
  // render endpoints. A device is captured when its config entry is enabled.
  const devices = useMemo(() => {
    const live = (outputs ?? []).filter((d) => d.id !== AUTO_DEVICE);
    return [{ id: AUTO_DEVICE, name: "Default Output Device" }, ...live];
  }, [outputs]);
  const deviceEnabled = (id: string) =>
    audio.pc_audio.find((d) => d.id === id)?.enabled ?? false;
  const pcEnabled = audio.pc_audio.some((d) => d.enabled);

  const setDevice = (id: string, name: string, on: boolean) => {
    const idx = audio.pc_audio.findIndex((d) => d.id === id);
    let pc_audio = audio.pc_audio;
    if (idx >= 0) {
      pc_audio = [...pc_audio];
      pc_audio[idx] = { ...pc_audio[idx], enabled: on };
    } else if (on) {
      pc_audio = [...pc_audio, { id, name, enabled: true, volume: 100 }];
    }
    patch({ pc_audio });
  };
  const setPcEnabled = (on: boolean) => {
    if (!on) {
      patch({ pc_audio: audio.pc_audio.map((d) => ({ ...d, enabled: false })) });
      return;
    }
    // Re-enable the default output (add it if the list was emptied).
    if (audio.pc_audio.some((d) => d.id === AUTO_DEVICE)) {
      patch({
        pc_audio: audio.pc_audio.map((d) =>
          d.id === AUTO_DEVICE ? { ...d, enabled: true } : d
        ),
      });
    } else {
      patch({
        pc_audio: [
          { id: AUTO_DEVICE, name: "Default Output Device", enabled: true, volume: 100 },
          ...audio.pc_audio,
        ],
      });
    }
  };

  // --- Specific apps -----------------------------------------------------------
  const game =
    audio.apps.find((a) => a.id === GAME_SOURCE_ID) ??
    ({ id: GAME_SOURCE_ID, name: "Game Audio", enabled: true, volume: 100 } as AudioAppSel);
  // Apps to show: saved sources (minus game) + live sessions not already saved.
  const appRows = useMemo(() => {
    const saved = audio.apps.filter((a) => a.id !== GAME_SOURCE_ID);
    const seen = new Set(saved.map((a) => a.id));
    // Single pass: filter + shape the live sessions in one reduce so we don't
    // walk the session list twice.
    const live = (sessions ?? []).reduce<AudioAppSel[]>((acc, s) => {
      if (
        !SESSION_BLACKLIST.has(s.process_name.toLowerCase()) &&
        !seen.has(s.process_name)
      ) {
        acc.push({
          id: s.process_name,
          name: s.display_name || s.process_name,
          enabled: false,
          volume: 100,
        });
      }
      return acc;
    }, []);
    return [...saved, ...live];
  }, [audio.apps, sessions]);

  // Real app icons keyed by process name (lowercased), from the live sessions.
  const iconByName = useMemo(() => {
    const m = new Map<string, string>();
    for (const s of sessions ?? []) {
      if (s.icon) m.set(s.process_name.toLowerCase(), s.icon);
    }
    return m;
  }, [sessions]);

  const supportsApps = supported ?? false;
  const modeUnsupported = audio.mode === "specific_apps" && !supportsApps;

  // --- Microphone (shared by both modes) --------------------------------------
  const micRow = (
    <SourceRow
      icon={Microphone}
      label="Microphone"
      hint={audio.mic_mono ? "Mono" : undefined}
      checked={audio.mic_enabled}
      onCheckedChange={(v) => patch({ mic_enabled: v })}
    >
      <div className="flex items-center gap-2">
        <Select
          value={audio.mic_source}
          onValueChange={(v) => patch({ mic_source: v })}
        >
          <SelectTrigger size="sm" className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="max-w-[280px]">
            <SelectItem value={AUTO_DEVICE}>Auto</SelectItem>
            {micDevices?.map((d) => (
              <SelectItem key={d.id} value={d.id}>
                {d.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <VolumeSlider
          value={audio.mic_volume}
          onCommit={(v) => patch({ mic_volume: v })}
        />
      </div>
    </SourceRow>
  );

  return (
    <div className="space-y-6">
      {/* Recording mode */}
      <Panel title="Recording mode">
        <div className="flex items-center justify-between gap-6 py-3 first:pt-0 last:pb-0">
          <div className="min-w-0">
            <div className="text-sm font-medium">Source</div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Capture all system audio, or split it per application.
            </p>
          </div>
          <Select
            value={audio.mode}
            onValueChange={(v) => patch({ mode: v })}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all_pc_audio">All PC audio</SelectItem>
              <SelectItem value="specific_apps" disabled={!supportsApps}>
                <span className="flex items-center gap-2">
                  Specific apps
                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold text-amber-500">
                    Experimental
                  </span>
                </span>
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        {!supportsApps && (
          <p className="flex items-start gap-2 py-3 text-xs text-muted-foreground last:pb-0">
            <Flask className="mt-0.5 size-3.5 shrink-0" weight="fill" />
            Per-app capture needs Windows 11 (build 20348+); it isn&apos;t available
            on this system, so recording stays on All PC audio.
          </p>
        )}
      </Panel>

      {modeUnsupported && (
        <div className="flex gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-500">
          <Flask className="size-4 shrink-0" weight="fill" />
          <span>
            Specific apps isn&apos;t supported on this PC, so capture will fall
            back to All PC audio. Switch the mode above to clear this.
          </span>
        </div>
      )}

      {audio.mode === "all_pc_audio" ? (
        <Panel title="PC audio" hint="System sound captured as loopback.">
          <SourceRow
            icon={SpeakerHigh}
            label="PC Audio"
            checked={pcEnabled}
            onCheckedChange={setPcEnabled}
          >
            <VolumeSlider
              value={audio.master_volume}
              onCommit={(v) => patch({ master_volume: v })}
            />
          </SourceRow>
          {/* Output device multi-select. */}
          <div className="py-3 last:pb-0">
            <div className="mb-2 pl-7 text-xs font-medium text-muted-foreground">
              Output devices
            </div>
            <div className="space-y-1">
              {devices.map((d) => (
                <label
                  key={d.id}
                  className={cn(
                    "flex cursor-pointer items-center gap-3 rounded-md px-7 py-1.5 hover:bg-accent/40",
                    !pcEnabled && "pointer-events-none opacity-40"
                  )}
                >
                  <Checkbox
                    checked={deviceEnabled(d.id)}
                    disabled={!pcEnabled}
                    onCheckedChange={(v) => setDevice(d.id, d.name, v === true)}
                  />
                  <span className="truncate text-sm">{d.name}</span>
                </label>
              ))}
            </div>
          </div>
          {micRow}
        </Panel>
      ) : (
        <Panel
          title="App audio"
          hint="Additional apps appear here when they start playing audio."
        >
          <SourceRow
            icon={GameController}
            label="Game Audio"
            hint="Valorant"
            checked={game.enabled}
            onCheckedChange={(v) =>
              patch({
                apps: upsertApp(audio.apps, GAME_SOURCE_ID, "Game Audio", {
                  enabled: v,
                }),
              })
            }
          >
            <VolumeSlider
              value={game.volume}
              onCommit={(v) =>
                patch({
                  apps: upsertApp(audio.apps, GAME_SOURCE_ID, "Game Audio", {
                    volume: v,
                  }),
                })
              }
            />
          </SourceRow>
          {micRow}
          {appRows.map((a) => (
            <SourceRow
              key={a.id}
              icon={Waveform}
              iconUrl={iconByName.get(a.id.toLowerCase())}
              label={a.name}
              checked={a.enabled}
              onCheckedChange={(v) =>
                patch({
                  apps: upsertApp(audio.apps, a.id, a.name, { enabled: v }),
                })
              }
            >
              <VolumeSlider
                value={a.volume}
                onCommit={(v) =>
                  patch({
                    apps: upsertApp(audio.apps, a.id, a.name, { volume: v }),
                  })
                }
              />
            </SourceRow>
          ))}
          {appRows.length === 0 && (
            <p className="py-3 text-xs text-muted-foreground last:pb-0">
              No other apps are playing audio right now.
            </p>
          )}
        </Panel>
      )}

      {/* Separate audio tracks */}
      <Panel>
        <div className="flex items-center justify-between gap-6 py-1">
          <div className="min-w-0">
            <div className="text-sm font-medium">Separate audio tracks</div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Save each source as its own track so you can mute them separately in
              the editor. The clip still plays as the full mix.
            </p>
          </div>
          <Switch
            checked={audio.separate_tracks}
            onCheckedChange={(v) => patch({ separate_tracks: v })}
          />
        </div>
      </Panel>
    </div>
  );
}
