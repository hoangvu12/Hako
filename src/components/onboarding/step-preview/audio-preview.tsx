import { Microphone, SpeakerHigh, GameController } from "@phosphor-icons/react";

import { effectiveAudioConfig, type Settings } from "@/lib/api";
import { Surface, Equalizer, DiscordIcon } from "./shared";

/** Audio — a live mixer of the enabled sources and their volumes. */
export function AudioPreview({ draft }: { draft: Settings }) {
  const cfg = effectiveAudioConfig(draft);
  const sources: { name: string; volume: number; icon: (cls: string) => React.ReactNode }[] = [];
  if (cfg.mode === "specific_apps") {
    for (const a of cfg.apps.filter((a) => a.enabled)) {
      const isGame = a.name === "Game Audio" || a.id === "game";
      const isDiscord = /discord/i.test(a.name) || /discord/i.test(a.id);
      const icon = isGame
        ? (cls: string) => <GameController weight="fill" className={cls} />
        : isDiscord
          ? (cls: string) => <DiscordIcon className={cls} />
          : (cls: string) => <SpeakerHigh weight="fill" className={cls} />;
      sources.push({
        name: isGame ? "Game" : a.name.replace(/\.exe$/i, ""),
        volume: a.volume,
        icon,
      });
    }
  } else {
    for (const p of cfg.pc_audio.filter((p) => p.enabled)) {
      sources.push({
        name: p.name || "PC Audio",
        volume: p.volume,
        icon: (cls) => <SpeakerHigh weight="fill" className={cls} />,
      });
    }
  }
  if (cfg.mic_enabled)
    sources.push({
      name: "Microphone",
      volume: cfg.mic_volume,
      icon: (cls) => <Microphone weight="fill" className={cls} />,
    });

  return (
    <Surface className="p-5">
      <div className="mb-4 flex items-center gap-2">
        <SpeakerHigh weight="duotone" className="size-4 text-primary-text" />
        <span className="text-sm font-semibold">Audio mix</span>
      </div>
      <div className="space-y-5">
        {sources.length === 0 && (
          <p className="py-4 text-center text-xs text-muted-foreground">
            No sources enabled — your clips will be silent.
          </p>
        )}
        {sources.map((s, i) => (
          <div key={`${s.name}-${i}`} className="flex items-center gap-3">
            <span className="flex size-4 shrink-0 items-center justify-center text-muted-foreground">
              {s.icon("size-4")}
            </span>
            <span className="w-20 shrink-0 truncate text-xs font-medium">{s.name}</span>
            <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-secondary">
              <div
                className="h-full rounded-full bg-primary transition-[width] duration-300"
                style={{ width: `${s.volume}%` }}
              />
            </div>
            <Equalizer active={s.volume > 0} />
          </div>
        ))}
      </div>
    </Surface>
  );
}
