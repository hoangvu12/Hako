import { useEffect, useState } from "react";
import { createLazyRoute, useSearch } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  Scissors,
  SlidersHorizontal,
  Crosshair,
  Monitor,
  HardDrives,
  Pulse,
  MagnifyingGlass,
  Warning,
  SpeakerHigh,
  Check,
  CaretDown,
  Trophy,
  Crown,
  Sword,
  Fire,
  Knife,
  Skull,
  Handshake,
  Bomb,
  Wrench,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Slider } from "@/components/ui/slider";
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
  getGpuInfo,
  type AudioConfig,
  type AutoCaptureMode,
  type EventToggles,
  type Settings,
} from "@/lib/api";

type SectionKey =
  | "clip"
  | "quality"
  | "audio"
  | "auto"
  | "capture"
  | "storage"
  | "status";

const SECTION_KEYS = new Set<SectionKey>([
  "clip",
  "quality",
  "audio",
  "auto",
  "capture",
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
      { key: "clip", label: "Clips", icon: Scissors },
      { key: "auto", label: "Auto-Capture", icon: Crosshair },
      { key: "quality", label: "Video", icon: SlidersHorizontal },
      { key: "audio", label: "Recording Audio", icon: SpeakerHigh },
      { key: "capture", label: "Capture", icon: Monitor },
    ],
  },
  {
    group: "System",
    items: [
      { key: "storage", label: "Storage", icon: HardDrives },
      { key: "status", label: "Status", icon: Pulse },
    ],
  },
];

const EVENT_LABELS: {
  key: keyof EventToggles;
  label: string;
  hint: string;
  icon: Icon;
}[] = [
  { key: "victory", label: "Victory", hint: "You won the match", icon: Trophy },
  { key: "clutch", label: "Clutch", hint: "1vX round win as the last alive", icon: Crown },
  { key: "kill", label: "Kill", hint: "Any elimination", icon: Sword },
  { key: "double_kill", label: "Double kill", hint: "Two in quick succession", icon: Sword },
  { key: "triple_kill", label: "Triple kill", hint: "3K", icon: Sword },
  { key: "quadra_kill", label: "Quadra kill", hint: "4K", icon: Sword },
  { key: "ace", label: "Ace", hint: "Full team wipe (5K)", icon: Fire },
  { key: "knife", label: "Knife kill", hint: "Melee elimination", icon: Knife },
  { key: "death", label: "Death", hint: "Your deaths", icon: Skull },
  { key: "assist", label: "Assist", hint: "Assisted eliminations", icon: Handshake },
  { key: "spike_detonated", label: "Spike detonated", hint: "A spike you planted exploded", icon: Bomb },
  { key: "spike_defused", label: "Spike defused", hint: "You defused the spike", icon: Wrench },
];

// Slider ranges for the per-event timing rows. Before can run long (the 45 s
// spike fuse); after is shorter.
const MAX_BEFORE_SECS = 60;
const MAX_AFTER_SECS = 30;

// Outplayed-style capture modes for the Auto Clipping section. Mirrors the Rust
// `AutoCaptureMode`; the cards write `auto_capture_mode`.
const CAPTURE_MODES: { key: AutoCaptureMode; label: string; blurb: string }[] = [
  { key: "manual", label: "Manual only", blurb: "Don't auto-capture; buffer + hotkey still work" },
  { key: "highlights", label: "Highlights", blurb: "Auto-clip the game events below" },
  { key: "full_match", label: "Full match", blurb: "Keep the entire match as one clip" },
  { key: "session", label: "Full session", blurb: "Record the whole time you're in-game" },
];

// Quality presets (Medal-style). A preset is just a named bundle of the
// concrete knobs; selecting one writes resolution/fps/bitrate, after which those
// fields are the source of truth. Codec/encoder/GPU are independent of the
// preset (separate dropdowns), so they're not touched here. Bitrates mirror
// Medal's ResolutionHandler table. "custom" is implicit (no card entry).
type PresetKey = "low" | "standard" | "high";
const PRESETS: {
  key: PresetKey;
  label: string;
  blurb: string;
  line: string;
  resolution: string;
  fps: number;
  bitrate: number;
}[] = [
  {
    key: "low",
    label: "Low Quality",
    blurb: "Lower-end PCs & faster uploads",
    line: "360p · 24 FPS",
    resolution: "360p",
    fps: 24,
    bitrate: 3,
  },
  {
    key: "standard",
    label: "Standard",
    blurb: "Performance & fast sharing",
    line: "720p · 60 FPS",
    resolution: "720p",
    fps: 60,
    bitrate: 10,
  },
  {
    key: "high",
    label: "High Quality",
    blurb: "Higher quality & slower uploads",
    line: "1080p · 60 FPS",
    resolution: "1080p",
    fps: 60,
    bitrate: 15,
  },
];

// Resolution targets for the Custom panel. "native" = no scaling (capture at the
// game's own size); the rest match the backend's `resolution_dims()` table.
const RESOLUTIONS: { value: string; label: string }[] = [
  { value: "native", label: "Native (no scaling)" },
  { value: "360p", label: "360p" },
  { value: "480p", label: "480p" },
  { value: "720p", label: "HD (720p)" },
  { value: "1080p", label: "Full HD (1080p)" },
  { value: "1440p", label: "QHD (1440p)" },
  { value: "2160p", label: "UHD 4K (2160p)" },
];

const FPS_OPTIONS = [24, 30, 60, 120, 144, 240];
const BITRATE_OPTIONS = [3, 5, 8, 10, 15, 20, 30, 50];

/** One selectable quality-preset card (Low / Standard / High / Custom). */
function PresetCard({
  title,
  blurb,
  line,
  selected,
  onSelect,
}: {
  title: string;
  blurb: string;
  line?: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "relative flex flex-col rounded-lg border p-3 text-left transition-colors",
        selected
          ? "border-primary bg-primary/10"
          : "border-border/70 bg-card/40 hover:border-border hover:bg-accent/40"
      )}
    >
      {selected && (
        <Check
          weight="bold"
          className="absolute top-2.5 right-2.5 size-4 text-primary-text"
        />
      )}
      <span className="text-sm font-semibold">{title}</span>
      <span className="mt-1 text-xs text-muted-foreground">{blurb}</span>
      {line && <span className="mt-2 text-xs font-medium">{line}</span>}
    </button>
  );
}

// The replay buffer keeps ~`buffer_seconds` of *compressed* video in RAM at the
// bitrate ceiling (mirrors the Rust `PacketRing`, which stores encoded packets
// sized by bitrate × time). Audio tracks add only a few MB, so they're ignored
// here. This is the app's dominant, directly-tunable RAM cost.
function estBufferBytes(bitrateMbps: number, bufferSeconds: number): number {
  return ((bitrateMbps * 1_000_000) / 8) * bufferSeconds;
}
function fmtBytes(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  return `${Math.round(bytes / (1 << 20))} MB`;
}
// Past this the buffer dominates a typical 8–16 GB machine; nudge the user.
const RAM_WARN_BYTES = 2 * (1 << 30);

/** RAM readout for the replay buffer, with an amber nudge once it gets large. */
function BufferRamHint({
  bitrateMbps,
  bufferSeconds,
}: {
  bitrateMbps: number;
  bufferSeconds: number;
}) {
  const bytes = estBufferBytes(bitrateMbps, bufferSeconds);
  const heavy = bytes >= RAM_WARN_BYTES;
  return (
    <p
      className={cn(
        "flex items-center gap-1.5 px-1 text-xs",
        heavy ? "text-warning" : "text-muted-foreground"
      )}
    >
      {heavy ? <Warning weight="fill" className="size-3.5 shrink-0" /> : null}
      Replay buffer holds ~{fmtBytes(bytes)} in RAM ({bufferSeconds}s × {bitrateMbps}{" "}
      Mbps)
      {heavy ? " — lower the bitrate or buffer length to use less." : "."}
    </p>
  );
}

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

// Lazy-loaded: only the component splits out — `validateSearch` stays eager in
// the route tree (router.tsx), which is required for type-safe search params.
export const Route = createLazyRoute("/settings")({
  component: SettingsPage,
});

function SettingsPage() {
  const { data } = useSettings();
  const update = useUpdateSettings();
  // GPU list for the "Selected GPU" dropdown. Cheap, cached; failure just leaves
  // the dropdown with the Auto option.
  const { data: gpus } = useQuery({
    queryKey: ["gpu-info"],
    queryFn: getGpuInfo,
    staleTime: 60_000,
    retry: false,
  });
  const search = useSearch({ from: "/settings" });
  const [draft, setDraft] = useState<Settings | null>(null);
  const [active, setActive] = useState<SectionKey>(
    isSectionKey(search.section) ? search.section : "clip"
  );
  const [navQuery, setNavQuery] = useState("");
  // Outplayed-style "Advanced options" disclosure for per-event timing.
  const [showTiming, setShowTiming] = useState(false);

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
  // Apply a preset: highlight its card and write its concrete knobs at once.
  const applyPreset = (p: (typeof PRESETS)[number]) =>
    persist({
      ...draft,
      quality_preset: p.key,
      resolution: p.resolution,
      target_fps: p.fps,
      bitrate_mbps: p.bitrate,
    });
  const toggleEvent = (key: keyof EventToggles) =>
    persist({ ...draft, events: { ...draft.events, [key]: !draft.events[key] } });
  // Per-event timing edits. `setTimingLocal` updates the draft live while
  // dragging (no save per pixel); `commitTiming` persists the final value on
  // release. The commit takes the explicit value (not a stale closure read) so a
  // single click — where onValueChange + onValueCommit fire in the same tick —
  // still saves the new value.
  const timingNext = (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ): Settings => ({
    ...draft,
    event_timings: {
      ...draft.event_timings,
      [key]: { ...draft.event_timings[key], [field]: value },
    },
  });
  const setTimingLocal = (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ) => setDraft(timingNext(key, field, value));
  const commitTiming = (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ) => persist(timingNext(key, field, value));

  const q = navQuery.trim().toLowerCase();
  // Single pass: filter each group's items and keep only non-empty groups in one
  // reduce, instead of mapping then filtering over the group list twice.
  const groups = NAV.reduce<typeof NAV>((acc, g) => {
    const items = g.items.filter((i) => i.label.toLowerCase().includes(q));
    if (items.length) acc.push({ ...g, items });
    return acc;
  }, []);

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
                subtitle="Set your save hotkey and the padding kept around each clip."
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
                title="Video"
                subtitle="Manage your recording resolution, frames per second, bitrate and more."
              />
              <Panel title="Recording Quality">
                <p className="pb-4 text-xs text-muted-foreground">
                  Higher settings use more resources. If you have issues, try a
                  lower one.
                </p>
                {/* Preset cards: Low / Standard / High / Custom. */}
                <div className="grid grid-cols-2 gap-3 pb-4">
                  {PRESETS.map((p) => (
                    <PresetCard
                      key={p.key}
                      title={p.label}
                      blurb={p.blurb}
                      line={p.line}
                      selected={draft.quality_preset === p.key}
                      onSelect={() => applyPreset(p)}
                    />
                  ))}
                  <PresetCard
                    title="Custom"
                    blurb="Customize your own settings"
                    selected={draft.quality_preset === "custom"}
                    onSelect={() => set("quality_preset", "custom")}
                  />
                </div>

                {/* Custom-only knobs — for a preset these are implied by the card. */}
                {draft.quality_preset === "custom" && (
                  <>
                    <Row label="Resolution" hint="Output size; capture is downscaled to fit (never upscaled).">
                      <Select
                        value={draft.resolution}
                        onValueChange={(v) => set("resolution", v)}
                      >
                        <SelectTrigger size="sm" className="w-44">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {RESOLUTIONS.map((r) => (
                            <SelectItem key={r.value} value={r.value}>
                              {r.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </Row>
                    <Row label="FPS" hint="Frames per second to capture and encode.">
                      <Select
                        value={String(draft.target_fps)}
                        onValueChange={(v) => set("target_fps", Number(v))}
                      >
                        <SelectTrigger size="sm" className="w-24">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {FPS_OPTIONS.map((f) => (
                            <SelectItem key={f} value={String(f)}>
                              {f}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </Row>
                    <Row label="Bitrate" hint="Encoding ceiling.">
                      <Select
                        value={String(draft.bitrate_mbps)}
                        onValueChange={(v) => set("bitrate_mbps", Number(v))}
                      >
                        <SelectTrigger size="sm" className="w-24">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {BITRATE_OPTIONS.map((b) => (
                            <SelectItem key={b} value={String(b)}>
                              {b}M
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </Row>
                  </>
                )}

                {/* Video Encoder / Selected GPU / Codec — independent of preset. */}
                <Row label="Video Encoder" hint="Hardware (GPU) encoding. CPU encoding is coming soon.">
                  <Select
                    value={draft.video_encoder === "cpu" ? "cpu" : "gpu"}
                    onValueChange={(v) => set("video_encoder", v)}
                  >
                    <SelectTrigger size="sm" className="w-28">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="gpu">GPU</SelectItem>
                      <SelectItem value="cpu" disabled>
                        CPU (soon)
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </Row>
                <Row label="Selected GPU" hint="Which adapter captures and encodes.">
                  <Select
                    value={String(draft.gpu_adapter)}
                    onValueChange={(v) => set("gpu_adapter", Number(v))}
                  >
                    <SelectTrigger size="sm" className="w-44">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="-1">Auto</SelectItem>
                      {(gpus?.adapters ?? [])
                        .filter((g) => !g.is_software)
                        .map((g) => (
                          <SelectItem key={g.index} value={String(g.index)}>
                            {g.name}
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
              </Panel>
            </>
          )}

          {active === "capture" && (
            <>
              <SectionHero
                icon={Monitor}
                title="Capture"
                subtitle="How frames are grabbed and how much gameplay is held ready to clip."
              />

              <Panel title="Replay buffer">
                <Row
                  label="Buffer length"
                  hint="Seconds of gameplay held ready to save as a clip."
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
                  label="Storage"
                  hint="RAM is fastest but uses memory; Disk spools the replay buffer to your drive to free RAM."
                >
                  <Select
                    value={draft.buffer_storage === "disk" ? "disk" : "ram"}
                    onValueChange={(v) => set("buffer_storage", v)}
                  >
                    <SelectTrigger size="sm" className="w-28">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="ram">RAM</SelectItem>
                      <SelectItem value="disk">Disk</SelectItem>
                    </SelectContent>
                  </Select>
                </Row>
              </Panel>

              {/* The bitrate ceiling (Video) × the buffer length above drives the
                  replay-buffer RAM. Only the RAM backend spends memory — the disk
                  backend spools to drive instead. */}
              {draft.buffer_storage !== "disk" && (
                <BufferRamHint
                  bitrateMbps={draft.bitrate_mbps}
                  bufferSeconds={draft.buffer_seconds}
                />
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
