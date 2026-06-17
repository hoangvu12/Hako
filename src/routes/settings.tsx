import { useEffect, useState } from "react";
import { useSearch } from "@tanstack/react-router";
import {
  Scissors,
  SlidersHorizontal,
  Crosshair,
  HardDrives,
  Pulse,
  MagnifyingGlass,
  Warning,
  SpeakerHigh,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { RecordingStatus } from "@/components/settings/recording-status";
import { RecordingAudio } from "@/components/settings/recording-audio";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import {
  effectiveAudioConfig,
  type AudioConfig,
  type EventToggles,
  type Settings,
} from "@/lib/api";

type SectionKey = "clip" | "quality" | "audio" | "auto" | "storage" | "status";

const SECTION_KEYS = new Set<SectionKey>([
  "clip",
  "quality",
  "audio",
  "auto",
  "storage",
  "status",
]);
const isSectionKey = (v: unknown): v is SectionKey =>
  typeof v === "string" && SECTION_KEYS.has(v as SectionKey);

const NAV: {
  group: string;
  items: { key: SectionKey; label: string; icon: Icon }[];
}[] = [
  {
    group: "Recording",
    items: [
      { key: "clip", label: "Clip Settings", icon: Scissors },
      { key: "quality", label: "Quality", icon: SlidersHorizontal },
      { key: "audio", label: "Recording Audio", icon: SpeakerHigh },
      { key: "auto", label: "Auto Clipping", icon: Crosshair },
    ],
  },
  {
    group: "Storage",
    items: [{ key: "storage", label: "Storage", icon: HardDrives }],
  },
  {
    group: "System",
    items: [{ key: "status", label: "Status", icon: Pulse }],
  },
];

const EVENT_LABELS: { key: keyof EventToggles; label: string; hint: string }[] =
  [
    { key: "kill", label: "Kill", hint: "Any elimination" },
    { key: "double_kill", label: "Double kill", hint: "Two in quick succession" },
    { key: "triple_kill", label: "Triple kill", hint: "3K" },
    { key: "quadra_kill", label: "Quadra kill", hint: "4K" },
    { key: "ace", label: "Ace", hint: "Full team wipe (5K)" },
    { key: "knife", label: "Knife kill", hint: "Melee elimination" },
    { key: "death", label: "Death", hint: "Your deaths" },
    { key: "assist", label: "Assist", hint: "Assisted eliminations" },
  ];

function SectionHero({
  icon: Icon,
  title,
  subtitle,
}: {
  icon: Icon;
  title: string;
  subtitle: string;
}) {
  return (
    <div className="flex flex-col items-center text-center">
      <div className="mb-3 flex size-14 items-center justify-center rounded-2xl bg-primary/10 text-primary-text">
        <Icon className="size-7" weight="duotone" />
      </div>
      <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
      <p className="mt-1 max-w-md text-sm text-muted-foreground">{subtitle}</p>
    </div>
  );
}

function Panel({
  title,
  children,
}: {
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-xl border border-border/70 bg-card/40 p-5">
      {title && (
        <h2 className="mb-2 text-sm font-semibold text-foreground">{title}</h2>
      )}
      <div className="divide-y divide-border/60">{children}</div>
    </section>
  );
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 py-4 first:pt-0 last:pb-0">
      <div className="min-w-0">
        <div className="text-sm font-medium">{label}</div>
        {hint && <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

export default function SettingsPage() {
  const { data } = useSettings();
  const update = useUpdateSettings();
  const search = useSearch({ from: "/settings" });
  const [draft, setDraft] = useState<Settings | null>(null);
  const [active, setActive] = useState<SectionKey>(
    isSectionKey(search.section) ? search.section : "clip"
  );
  const [navQuery, setNavQuery] = useState("");

  // Initialise the draft once; instant-apply edits keep it in sync afterwards.
  useEffect(() => {
    if (data && !draft) setDraft(data);
  }, [data, draft]);

  // Deep-link: jump to a section when navigated with `?section=` (e.g. from the
  // recorder popover) — including while the page is already mounted.
  useEffect(() => {
    if (isSectionKey(search.section)) setActive(search.section);
  }, [search.section]);

  if (!draft) {
    return (
      <div className="p-8 text-sm text-muted-foreground">Loading settings…</div>
    );
  }

  const persist = (next: Settings) => {
    setDraft(next);
    update.mutate(next);
  };
  // Instant-apply for toggles/selects.
  const set = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    persist({ ...draft, [key]: value });
  // Local edit (number/text) — committed on blur to avoid a save per keystroke.
  const setLocal = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setDraft({ ...draft, [key]: value });
  const commit = () => update.mutate(draft);
  const toggleEvent = (key: keyof EventToggles) =>
    persist({ ...draft, events: { ...draft.events, [key]: !draft.events[key] } });

  const q = navQuery.trim().toLowerCase();
  const groups = NAV.map((g) => ({
    ...g,
    items: g.items.filter((i) => i.label.toLowerCase().includes(q)),
  })).filter((g) => g.items.length);

  return (
    <div className="flex h-full">
      {/* Settings nav */}
      <aside className="flex w-[260px] shrink-0 flex-col border-r border-panel-border bg-panel">
        <div className="p-3">
          <div className="relative">
            <MagnifyingGlass className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={navQuery}
              onChange={(e) => setNavQuery(e.target.value)}
              placeholder="Search settings"
              className="h-9 bg-field pl-9"
            />
          </div>
        </div>
        <nav className="scrollbar-thin flex-1 overflow-y-auto px-2 pb-4">
          {groups.map((g) => (
            <div key={g.group} className="mb-4">
              <div className="px-3 pb-1.5 text-[11px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
                {g.group}
              </div>
              {g.items.map((it) => {
                const on = active === it.key;
                return (
                  <button
                    key={it.key}
                    type="button"
                    onClick={() => setActive(it.key)}
                    className={cn(
                      "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-sm transition-colors",
                      on
                        ? "bg-primary/10 font-medium text-primary-text"
                        : "text-foreground/80 hover:bg-accent/60 hover:text-foreground"
                    )}
                  >
                    <it.icon className="size-4" weight={on ? "fill" : "regular"} />
                    {it.label}
                  </button>
                );
              })}
            </div>
          ))}
          {groups.length === 0 && (
            <p className="px-3 text-sm text-muted-foreground">No matches.</p>
          )}
        </nav>
      </aside>

      {/* Content */}
      <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-2xl space-y-6 px-8 py-10">
          {active === "clip" && (
            <>
              <SectionHero
                icon={Scissors}
                title="Clip Settings"
                subtitle="Set your save hotkey, replay buffer, and clip padding."
              />
              <Panel title="Clipping">
                <Row
                  label="Save-clip hotkey"
                  hint="Saves the last buffered seconds to a clip."
                >
                  <Input
                    className="h-9 w-36 bg-secondary text-center font-medium"
                    value={draft.save_hotkey}
                    onChange={(e) => setLocal("save_hotkey", e.target.value)}
                    onBlur={commit}
                  />
                </Row>
                <Row
                  label="Replay buffer"
                  hint="Seconds of gameplay kept in RAM, ready to save."
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
                  label="Pad before"
                  hint="Extra seconds kept before the moment."
                >
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      className="h-9 w-20 text-right"
                      value={draft.pad_before_secs}
                      onChange={(e) =>
                        setLocal("pad_before_secs", Number(e.target.value))
                      }
                      onBlur={commit}
                    />
                    <span className="text-sm text-muted-foreground">s</span>
                  </div>
                </Row>
                <Row
                  label="Pad after"
                  hint="Extra seconds kept after the moment."
                >
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      className="h-9 w-20 text-right"
                      value={draft.pad_after_secs}
                      onChange={(e) =>
                        setLocal("pad_after_secs", Number(e.target.value))
                      }
                      onBlur={commit}
                    />
                    <span className="text-sm text-muted-foreground">s</span>
                  </div>
                </Row>
              </Panel>
            </>
          )}

          {active === "quality" && (
            <>
              <SectionHero
                icon={SlidersHorizontal}
                title="Quality"
                subtitle="Capture stays at native resolution — tune frame rate, codec, and bitrate."
              />
              <Panel title="Encoder">
                <Row
                  label="Target FPS"
                  hint="Frames per second to capture and encode."
                >
                  <Select
                    value={String(draft.target_fps)}
                    onValueChange={(v) => set("target_fps", Number(v))}
                  >
                    <SelectTrigger size="sm" className="w-24">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {[30, 60, 120, 144, 240].map((f) => (
                        <SelectItem key={f} value={String(f)}>
                          {f}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Row>
                <Row label="Codec" hint="Video codec for saved clips.">
                  <Select
                    value={draft.codec}
                    onValueChange={(v) => set("codec", v)}
                  >
                    <SelectTrigger size="sm" className="w-28">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {["h264", "hevc", "av1"].map((c) => (
                        <SelectItem key={c} value={c}>
                          {c.toUpperCase()}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Row>
                <Row label="Bitrate" hint="Encoding ceiling.">
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      className="h-9 w-20 text-right"
                      value={draft.bitrate_mbps}
                      onChange={(e) =>
                        setLocal("bitrate_mbps", Number(e.target.value))
                      }
                      onBlur={commit}
                    />
                    <span className="text-sm text-muted-foreground">Mbps</span>
                  </div>
                </Row>
              </Panel>

              <Panel title="Capture mode">
                <Row
                  label="Backend"
                  hint="WGC is Vanguard-safe; game-hook beats the FPS cap but carries anti-cheat risk."
                >
                  <Select
                    value={draft.capture_mode === "hook" ? "hook" : "wgc"}
                    onValueChange={(v) => set("capture_mode", v)}
                  >
                    <SelectTrigger size="sm" className="w-36">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="wgc">WGC (safe)</SelectItem>
                      <SelectItem value="hook">Game hook</SelectItem>
                    </SelectContent>
                  </Select>
                </Row>
              </Panel>

              {draft.capture_mode === "hook" && (
                <div className="flex gap-2 rounded-lg border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive">
                  <Warning className="size-4 shrink-0" weight="fill" />
                  <span>
                    Game-hook injects into the game to capture above the ~60&nbsp;FPS
                    desktop-composition cap. Anti-cheats (e.g. Valorant's Vanguard)
                    may flag the injector and put your account at risk. WGC stays
                    the safe default.
                  </span>
                </div>
              )}
            </>
          )}

          {active === "audio" && (
            <>
              <SectionHero
                icon={SpeakerHigh}
                title="Recording Audio"
                subtitle="Choose which sources are recorded, set their volumes, and split them onto separate tracks."
              />
              <RecordingAudio
                audio={effectiveAudioConfig(draft)}
                onChange={(audio: AudioConfig) => set("audio", audio)}
              />
            </>
          )}

          {active === "auto" && (
            <>
              <SectionHero
                icon={Crosshair}
                title="Auto Clipping"
                subtitle="Choose which Valorant moments are clipped automatically."
              />
              <Panel title="Events">
                {EVENT_LABELS.map((ev) => (
                  <Row key={ev.key} label={ev.label} hint={ev.hint}>
                    <Switch
                      checked={draft.events[ev.key]}
                      onCheckedChange={() => toggleEvent(ev.key)}
                    />
                  </Row>
                ))}
              </Panel>
            </>
          )}

          {active === "storage" && (
            <>
              <SectionHero
                icon={HardDrives}
                title="Storage"
                subtitle="Where clips are written on disk."
              />
              <Panel title="Library">
                <Row
                  label="Clip folder"
                  hint="Leave blank to use the default (Videos/Hako)."
                >
                  <Input
                    className="w-64"
                    value={draft.storage_dir ?? ""}
                    placeholder="Videos/Hako"
                    onChange={(e) =>
                      setLocal("storage_dir", e.target.value || null)
                    }
                    onBlur={commit}
                  />
                </Row>
              </Panel>
            </>
          )}

          {active === "status" && (
            <>
              <SectionHero
                icon={Pulse}
                title="Status"
                subtitle="Live recorder, encoder, and GPU detection."
              />
              <RecordingStatus />
            </>
          )}

          {update.error ? (
            <p className="text-sm text-destructive">{String(update.error)}</p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
