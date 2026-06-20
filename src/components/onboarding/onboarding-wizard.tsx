import { useEffect, useRef, useState } from "react";
import {
  Sparkle,
  HardDrives,
  SlidersHorizontal,
  SpeakerHigh,
  Scissors,
  Crosshair,
  CloudArrowUp,
  FolderOpen,
  ArrowLeft,
  ArrowRight,
  Check,
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
import { open } from "@tauri-apps/plugin-dialog";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { HotkeyRecorder } from "@/components/ui/hotkey-recorder";
import { SectionHero, Panel, Row, PresetCard } from "@/components/settings/primitives";
import { StepPreview } from "@/components/onboarding/step-preview";
import { RecordingAudio } from "@/components/settings/recording-audio";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import {
  effectiveAudioConfig,
  type AudioConfig,
  type AutoCaptureMode,
  type EventToggles,
  type Settings,
} from "@/lib/api";

// --- Curated option sets for the wizard. These mirror the equivalents in
// `routes/settings.tsx`; kept local so the wizard is self-contained and editing
// the full settings page can't accidentally reshape the first-run flow. ------

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
  { key: "low", label: "Low Quality", blurb: "Lower-end PCs & faster uploads", line: "360p · 24 FPS", resolution: "360p", fps: 24, bitrate: 3 },
  { key: "standard", label: "Standard", blurb: "Performance & fast sharing", line: "720p · 60 FPS", resolution: "720p", fps: 60, bitrate: 10 },
  { key: "high", label: "High Quality", blurb: "Higher quality & slower uploads", line: "1080p · 60 FPS", resolution: "1080p", fps: 60, bitrate: 15 },
];

const CLIP_LENGTHS = [10, 15, 30, 60, 90, 120, 180];

// Custom-quality knobs (mirror routes/settings.tsx), shown when the preset is
// "Custom".
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

const CAPTURE_MODES: { key: AutoCaptureMode; label: string; blurb: string }[] = [
  { key: "manual", label: "Manual only", blurb: "Don't auto-capture; buffer + hotkey still work" },
  { key: "highlights", label: "Highlights", blurb: "Auto-clip the game events below" },
  { key: "full_match", label: "Full match", blurb: "Keep the entire match as one clip" },
  { key: "session", label: "Full session", blurb: "Record the whole time you're in-game" },
];

const EVENT_LABELS: { key: keyof EventToggles; label: string; hint: string; icon: Icon }[] = [
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

type StepKey =
  | "welcome"
  | "storage"
  | "video"
  | "audio"
  | "clips"
  | "auto"
  | "done";

// Ordered so the essentials needed for a first clip (storage → quality → audio →
// hotkey) come first; auto-capture is `optional` and reachable via a "Finish
// now" exit, per the "work backward to the first win" onboarding rule.
const STEPS: { key: StepKey; nav: string; optional?: boolean }[] = [
  { key: "welcome", nav: "Welcome" },
  { key: "storage", nav: "Storage" },
  { key: "video", nav: "Video" },
  { key: "audio", nav: "Audio" },
  { key: "clips", nav: "Clips" },
  { key: "auto", nav: "Auto-capture", optional: true },
  { key: "done", nav: "Done" },
];

/**
 * First-run setup wizard. A full-screen overlay shown while
 * `settings.onboarding_completed` is false; it walks the user through every
 * settings section, writing each choice straight to `settings.json` via the
 * same data flow as the settings page. Finishing (or skipping) flips the flag.
 *
 * Mounted unconditionally at the app root — it gates itself on the settings
 * query so hooks stay stable and the host layout needn't know about it.
 */
export function OnboardingWizard() {
  const { data } = useSettings();
  const { mutate: saveSettings } = useUpdateSettings();

  const [draft, setDraft] = useState<Settings | null>(null);
  const draftRef = useRef<Settings | null>(null);
  useEffect(() => {
    draftRef.current = draft;
  }, [draft]);
  useEffect(() => {
    if (data && !draft) setDraft(data);
  }, [data, draft]);

  const [stepIndex, setStepIndex] = useState(0);

  // Gate: nothing until settings load, and never once onboarding is done.
  if (!data || data.onboarding_completed || !draft) return null;

  // --- Draft mutators, mirroring the settings page (instant-apply for
  // toggles/selects, deferred commit for free-text/number inputs). ----------
  const persist = (next: Settings) => {
    setDraft(next);
    saveSettings(next);
  };
  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const d = draftRef.current;
    if (d) persist({ ...d, [key]: value });
  };
  const setLocal = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const d = draftRef.current;
    if (d) setDraft({ ...d, [key]: value });
  };
  const commit = () => {
    const d = draftRef.current;
    if (d) saveSettings(d);
  };
  const applyPreset = (p: (typeof PRESETS)[number]) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        quality_preset: p.key,
        resolution: p.resolution,
        target_fps: p.fps,
        bitrate_mbps: p.bitrate,
      });
  };
  const toggleEvent = (key: keyof EventToggles) => {
    const d = draftRef.current;
    if (d) persist({ ...d, events: { ...d.events, [key]: !d.events[key] } });
  };

  // Finishing and skipping are the same write — flip the flag on the live draft.
  const complete = () => {
    const d = draftRef.current;
    if (d) persist({ ...d, onboarding_completed: true });
  };

  const step = STEPS[stepIndex];
  const isFirst = stepIndex === 0;
  const isLast = stepIndex === STEPS.length - 1;
  const next = () => (isLast ? complete() : setStepIndex((i) => i + 1));
  const back = () => setStepIndex((i) => Math.max(0, i - 1));
  const pct = Math.round((stepIndex / (STEPS.length - 1)) * 100);

  // From the last essential step onward (but not the Done recap), surface a
  // prominent "Finish now" so users who don't want the optional extras can leave
  // with everything important already configured.
  const firstOptionalIndex = STEPS.findIndex((s) => s.optional);
  const showFinishNow = stepIndex >= firstOptionalIndex - 1 && !isLast;

  return (
    <div className="fixed inset-0 z-50 flex bg-background">
      {/* Left pane: the form. Full width on narrow screens; a column beside the
          preview on wide ones. */}
      <div className="flex h-full w-full flex-col lg:w-[46%] lg:min-w-[440px] lg:border-r lg:border-border/60">
        {/* Progress header — pinned to the top of the pane. */}
        <div className="border-b border-border/60 px-6 py-4">
          <div className="mx-auto w-full max-w-xl">
          <div className="mb-2 flex items-center justify-between text-xs text-muted-foreground">
            <span className="font-medium">
              Step {stepIndex + 1} of {STEPS.length}
              {step.optional && (
                <span className="ml-2 rounded-full bg-secondary px-2 py-0.5 text-[10px] tracking-wide uppercase">
                  Optional
                </span>
              )}
            </span>
            {!isLast && (
              <button
                type="button"
                onClick={complete}
                className="font-medium text-muted-foreground transition-colors hover:text-foreground"
              >
                Skip setup
              </button>
            )}
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-secondary">
            <div
              className="h-full rounded-full bg-primary transition-[width] duration-300"
              style={{ width: `${pct}%` }}
            />
          </div>
        </div>
      </div>

        {/* Step body — scrolls between the header and footer bars. */}
        <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-xl space-y-6 px-6 py-10">
          {step.key === "welcome" && (
            <>
              <SectionHero
                icon={Sparkle}
                title="Never miss a play"
                subtitle="Hako quietly records in the background and clips your best Valorant moments — clutches, aces, multikills — so you can relive and share them."
              />
              <Panel title="In about a minute, you'll be able to">
                {[
                  { icon: Crosshair, label: "Capture highlights automatically", hint: "Your best rounds, saved without lifting a finger." },
                  { icon: Scissors, label: "Save any moment with a hotkey", hint: "Pull the last few seconds whenever something pops off." },
                  { icon: CloudArrowUp, label: "Keep and share your clips", hint: "Stored your way, ready to upload." },
                ].map((it) => (
                  <Row key={it.label} label={it.label} hint={it.hint}>
                    <it.icon className="size-5 text-primary-text" weight="duotone" />
                  </Row>
                ))}
              </Panel>
              <p className="px-1 text-center text-xs text-muted-foreground">
                Takes about a minute. You can skip anything and change it all later
                in Settings.
              </p>
            </>
          )}

          {step.key === "storage" && (
            <>
              <SectionHero
                icon={HardDrives}
                title="Where to save clips"
                subtitle="Pick a folder on a drive with some free space. Leave it blank to use the default."
              />
              <Panel title="Clip folder">
                <Row
                  label="Folder"
                  hint="Browse to a folder, or paste a path. Default: Videos/Hako."
                >
                  <div className="flex items-center gap-2">
                    <Input
                      className="w-56"
                      value={draft.storage_dir ?? ""}
                      placeholder="Videos/Hako"
                      onChange={(e) => setLocal("storage_dir", e.target.value || null)}
                      onBlur={commit}
                    />
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => {
                        void (async () => {
                          const picked = await open({
                            directory: true,
                            defaultPath: draft.storage_dir ?? undefined,
                          });
                          if (typeof picked === "string") set("storage_dir", picked);
                        })();
                      }}
                    >
                      <FolderOpen />
                      Browse
                    </Button>
                  </div>
                </Row>
              </Panel>
            </>
          )}

          {step.key === "video" && (
            <>
              <SectionHero
                icon={SlidersHorizontal}
                title="Recording quality"
                subtitle="Higher settings look better but use more resources. You can fine-tune this later."
              />
              <Panel title="Quality preset">
                <div className="grid grid-cols-2 gap-3 pt-1">
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
                    blurb="Choose your own resolution, FPS and bitrate"
                    selected={draft.quality_preset === "custom"}
                    onSelect={() => set("quality_preset", "custom")}
                  />
                </div>
              </Panel>

              {draft.quality_preset === "custom" && (
                <Panel title="Custom">
                  <Row
                    label="Resolution"
                    hint="Output size; capture is downscaled to fit (never upscaled)."
                  >
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
                </Panel>
              )}
            </>
          )}

          {step.key === "audio" && (
            <>
              <SectionHero
                icon={SpeakerHigh}
                title="Recording audio"
                subtitle="Choose which sources are recorded and set their volumes."
              />
              <RecordingAudio
                audio={effectiveAudioConfig(draft)}
                onChange={(audio: AudioConfig) => set("audio", audio)}
              />
            </>
          )}

          {step.key === "clips" && (
            <>
              <SectionHero
                icon={Scissors}
                title="Save-clip hotkey"
                subtitle="Press this in-game to instantly save the last few seconds."
              />
              <Panel title="Clipping">
                <Row
                  label="Save-clip hotkey"
                  hint="Click and press the keys you want."
                >
                  <HotkeyRecorder
                    aria-label="Save-clip hotkey"
                    value={draft.save_hotkey}
                    onChange={(accel) => accel && set("save_hotkey", accel)}
                    allowClear={false}
                  />
                </Row>
                <Row
                  label="Clip length"
                  hint="Seconds the hotkey captures (capped at the buffer length)."
                >
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
          )}

          {step.key === "auto" && (
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
                      <Switch
                        checked={draft.events[ev.key]}
                        onCheckedChange={() => toggleEvent(ev.key)}
                      />
                    </Row>
                  ))}
                </Panel>
              )}
            </>
          )}

          {step.key === "done" && (
            <>
              {/* Bespoke (not SectionHero) so the check can pop in as a small
                  reward on completion. */}
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
          )}
        </div>
      </div>

        {/* Footer nav — pinned to the bottom of the pane. */}
        <div className="border-t border-border/60 px-6 py-4">
          <div className="mx-auto flex w-full max-w-xl items-center justify-between">
          <Button
            variant="ghost"
            size="sm"
            onClick={back}
            disabled={isFirst}
            className={cn(isFirst && "invisible")}
          >
            <ArrowLeft />
            Back
          </Button>
          <div className="flex items-center gap-2">
            {showFinishNow && (
              <Button variant="secondary" size="sm" onClick={complete}>
                Finish now
              </Button>
            )}
            <Button size="sm" onClick={next}>
              {isLast ? (
                <>
                  <Check />
                  Finish
                </>
              ) : (
                <>
                  {step.key === "welcome" ? "Get started" : "Next"}
                  <ArrowRight />
                </>
              )}
            </Button>
          </div>
          </div>
        </div>
      </div>

      {/* Right pane: the live "visualization" — reacts to the form on the left.
          Hidden on narrow widths where the form takes the full screen. */}
      <div
        className="relative hidden flex-1 items-center justify-center overflow-hidden bg-cover bg-center lg:flex"
        style={{ backgroundImage: "url(/onboarding/hero-gradient.png)" }}
      >
        <div
          key={step.key}
          className="relative z-10 flex w-full max-w-xl justify-center px-8 duration-500 animate-in fade-in slide-in-from-bottom-3"
        >
          <StepPreview step={step.key} draft={draft} />
        </div>
      </div>
    </div>
  );
}
