import {
  Crosshair,
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

import { type EventToggles, type Settings } from "@/lib/api";
import { Surface, Thumb, DurationBadge, SAMPLE_CLIPS } from "./shared";

const EVENT_CHIPS: { key: keyof EventToggles; label: string; icon: Icon }[] = [
  { key: "victory", label: "Victory", icon: Trophy },
  { key: "clutch", label: "Clutch", icon: Crown },
  { key: "kill", label: "Kill", icon: Sword },
  { key: "double_kill", label: "2K", icon: Sword },
  { key: "triple_kill", label: "3K", icon: Sword },
  { key: "quadra_kill", label: "4K", icon: Sword },
  { key: "ace", label: "Ace", icon: Fire },
  { key: "knife", label: "Knife", icon: Knife },
  { key: "death", label: "Death", icon: Skull },
  { key: "assist", label: "Assist", icon: Handshake },
  { key: "spike_detonated", label: "Spike", icon: Bomb },
  { key: "spike_defused", label: "Defuse", icon: Wrench },
];

const MODE_BLURBS: Record<string, string> = {
  manual: "Manual only — buffer + hotkey",
  full_match: "The whole match, saved as one clip",
  session: "Recording the entire session",
};

/**
 * Auto-capture — instead of a flat chip grid (which reads as "what's armed" but
 * never shows the automation), this plays the actual story on a loop: a game
 * event fires, then a clip auto-drops into the library. The hero event is pulled
 * from whatever the user has enabled, so it still reacts to the form.
 */
export function AutoPreview({ draft }: { draft: Settings }) {
  if (draft.auto_capture_mode !== "highlights") {
    return (
      <Surface className="flex flex-col items-center gap-3 p-7 text-center">
        <div className="flex size-12 items-center justify-center rounded-2xl bg-primary/15 text-primary-text">
          <Crosshair weight="duotone" className="size-6" />
        </div>
        <p className="text-sm font-medium">
          {MODE_BLURBS[draft.auto_capture_mode] ?? "Manual only"}
        </p>
      </Surface>
    );
  }

  const armed = EVENT_CHIPS.filter((e) => draft.events[e.key]);
  const hero = armed[0] ?? EVENT_CHIPS[0];

  return (
    <Surface className="p-4">
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Crosshair weight="duotone" className="size-4 text-primary-text" />
          <span className="text-sm font-semibold">Auto-capture</span>
        </div>
        <span className="flex items-center gap-1.5 rounded-full border border-border/60 bg-secondary/60 px-2 py-0.5 text-[10px] font-medium text-muted-foreground">
          <span className="hako-rec-pulse size-1.5 rounded-full bg-red-500" />
          Watching your game
        </span>
      </div>

      {/* The automation, looping: event detected → clip saved, hands-free. */}
      <div className="space-y-2.5">
        <div className="hako-auto-fire flex items-center gap-2 rounded-lg border border-primary/40 bg-primary/15 px-3 py-2 text-primary-text">
          <hero.icon weight="fill" className="size-4 shrink-0" />
          <span className="text-sm font-semibold">{hero.label}</span>
          <span className="ml-auto text-[10px] font-medium tracking-wide uppercase opacity-70">
            detected
          </span>
        </div>

        <div className="hako-auto-drop flex items-center gap-2.5 rounded-lg border border-border/60 bg-card p-2 shadow-sm">
          <Thumb src={SAMPLE_CLIPS[2].img} className="w-20 shrink-0 rounded-md">
            <DurationBadge>{SAMPLE_CLIPS[2].dur}</DurationBadge>
          </Thumb>
          <div className="min-w-0">
            <p className="truncate text-xs font-semibold">{hero.label} · auto</p>
            <p className="text-[10px] font-medium text-success">Saved to your library</p>
          </div>
          <Check weight="fill" className="hako-auto-check ml-auto size-4 shrink-0 text-success" />
        </div>
      </div>

      {armed.length > 1 && (
        <div className="mt-3 border-t border-border/60 pt-3">
          <p className="mb-2 text-[10px] font-medium tracking-wide text-muted-foreground uppercase">
            Also watching
          </p>
          <div className="flex flex-wrap gap-1.5">
            {armed.slice(1).map((e) => (
              <span
                key={e.key}
                className="flex items-center gap-1 rounded-full border border-border/60 bg-secondary/40 px-2 py-0.5 text-[10px] font-medium text-muted-foreground"
              >
                <e.icon weight="fill" className="size-2.5" />
                {e.label}
              </span>
            ))}
          </div>
        </div>
      )}
    </Surface>
  );
}
