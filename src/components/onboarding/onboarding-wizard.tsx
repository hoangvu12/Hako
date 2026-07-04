import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Check } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { StepPreview } from "@/components/onboarding/step-preview";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import type { EventToggles, Settings } from "@/lib/api";
import { PRESETS, STEPS } from "@/components/onboarding/wizard/config";
import { WelcomeStep } from "@/components/onboarding/wizard/steps/welcome-step";
import { StorageStep } from "@/components/onboarding/wizard/steps/storage-step";
import { VideoStep } from "@/components/onboarding/wizard/steps/video-step";
import { AudioStep } from "@/components/onboarding/wizard/steps/audio-step";
import { ClipsStep } from "@/components/onboarding/wizard/steps/clips-step";
import { AutoStep } from "@/components/onboarding/wizard/steps/auto-step";
import { DoneStep } from "@/components/onboarding/wizard/steps/done-step";

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
  // Seed the editable draft once settings load. Render-phase init (not an effect)
  // so it can't cascade an extra commit; the `!draft` guard makes it fire once.
  if (data && !draft) setDraft(data);

  const [stepIndex, setStepIndex] = useState(0);

  // Gate: nothing until settings load, and never once onboarding is done.
  if (!data || data.onboarding_completed || !draft) return null;

  // --- Draft mutators, mirroring the settings page (instant-apply for
  // toggles/selects, deferred commit for free-text/number inputs). The mutators
  // read the live draft via `draftRef` rather than closing over `draft`, so
  // their identity stays stable and the compiler can skip re-rendering steps
  // whose own props haven't changed. ----------------------------------------
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
            {step.key === "welcome" && <WelcomeStep />}
            {step.key === "storage" && (
              <StorageStep draft={draft} set={set} setLocal={setLocal} commit={commit} />
            )}
            {step.key === "video" && (
              <VideoStep draft={draft} set={set} applyPreset={applyPreset} />
            )}
            {step.key === "audio" && <AudioStep draft={draft} set={set} />}
            {step.key === "clips" && <ClipsStep draft={draft} set={set} />}
            {step.key === "auto" && <AutoStep draft={draft} set={set} toggleEvent={toggleEvent} />}
            {step.key === "done" && <DoneStep draft={draft} />}
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
