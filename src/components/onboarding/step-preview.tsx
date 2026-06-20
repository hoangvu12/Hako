import {
  Check,
  Play,
  Microphone,
  SpeakerHigh,
  GameController,
  FolderOpen,
  Crosshair,
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
import {
  effectiveAudioConfig,
  type EventToggles,
  type Settings,
} from "@/lib/api";

// The right-hand panel is purely VISUAL and mirrors the REAL app UI as closely
// as possible — clip cards (clip-card.tsx) and toasts (upload-toast.tsx) — so it
// reads as Hako, not a generic mock. No header/description here (those live with
// the form on the left), and no invented gradient "scenes": thumbnails are the
// app's flat `bg-muted`, optionally backed by a real screenshot dropped into
// `public/onboarding/` (falls back to muted if the file is absent).

/** Optional real screenshots; if a file is missing the <img> hides → bg-muted. */
type Sample = { title: string; img: string; dur: string; meta: string; won: boolean };
const SAMPLE_CLIPS: Sample[] = [
  { title: "Ace on Haven", img: "/onboarding/clip-1.jpg", dur: "0:18", meta: "Ace", won: true },
  { title: "1v3 Clutch", img: "/onboarding/clip-2.jpg", dur: "0:24", meta: "Clutch", won: true },
  { title: "Triple kill", img: "/onboarding/clip-3.jpg", dur: "0:12", meta: "Triple kill", won: false },
  { title: "Spike defuse", img: "/onboarding/clip-4.jpg", dur: "0:09", meta: "Defuse", won: true },
];

const hideOnError = (e: React.SyntheticEvent<HTMLImageElement>) => {
  e.currentTarget.style.display = "none";
};

/** Card shell mirroring the real clip card / toast container. */
function Surface({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "w-full overflow-hidden rounded-xl border border-border/60 bg-card shadow-lg",
        className
      )}
    >
      {children}
    </div>
  );
}

/** The thumbnail surface from clip-card.tsx: flat `bg-muted`, optional real
 * screenshot, and the same top scrim so overlaid badges stay legible. */
function Thumb({
  src,
  className,
  imgStyle,
  children,
}: {
  src?: string;
  className?: string;
  imgStyle?: React.CSSProperties;
  children?: React.ReactNode;
}) {
  return (
    <div className={cn("relative aspect-video overflow-hidden bg-muted", className)}>
      {src && (
        <img
          src={src}
          alt=""
          draggable={false}
          className="size-full object-cover"
          style={imgStyle}
          onError={hideOnError}
        />
      )}
      {/* Same scrim the real card uses (clip-card.tsx). */}
      <span className="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/50 to-transparent opacity-60" />
      {children}
    </div>
  );
}

function WinPill({ won }: { won: boolean }) {
  return (
    <span
      className={cn(
        "pointer-events-none absolute top-2 right-2 rounded-full px-2 py-0.5 text-[10px] font-bold text-white",
        won ? "bg-success/90" : "bg-destructive/90"
      )}
    >
      {won ? "WIN" : "LOSS"}
    </span>
  );
}

function DurationBadge({ children }: { children: React.ReactNode }) {
  return (
    <span className="pointer-events-none absolute right-2 bottom-2 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
      {children}
    </span>
  );
}

function Dot() {
  return <span className="size-[3px] shrink-0 rounded-full bg-secondary" />;
}

/** A faithful, compact clip card (matches clip-card.tsx's structure). */
function ClipCardMini({ clip, badges }: { clip: Sample; badges?: React.ReactNode }) {
  return (
    <div className="group flex flex-col overflow-hidden rounded-xl border border-border/60 bg-card shadow-sm">
      <Thumb src={clip.img}>
        <WinPill won={clip.won} />
        {badges}
        <DurationBadge>{clip.dur}</DurationBadge>
      </Thumb>
      <div className="flex flex-col gap-1 p-2.5">
        <h3 className="truncate text-xs font-semibold text-card-foreground">{clip.title}</h3>
        <div className="flex items-center gap-1.5 text-[10px] font-medium text-muted-foreground">
          <span className="truncate text-foreground/80">{clip.meta}</span>
          <Dot />
          <span className="shrink-0">2m ago</span>
        </div>
      </div>
    </div>
  );
}

/** Welcome — a single clip card, mid-recording. */
function WelcomePreview() {
  return (
    <Surface>
      <div className="relative aspect-video overflow-hidden bg-muted">
        <video
          src="/onboarding/welcome.mp4"
          autoPlay
          loop
          muted
          playsInline
          className="size-full object-cover"
        />
        {/* Same scrim the real card uses (clip-card.tsx). */}
        <span className="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/50 to-transparent opacity-60" />
        <span className="absolute top-2 left-2 flex items-center gap-1.5 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
          <span className="hako-rec-pulse size-1.5 rounded-full bg-red-500" />
          REC
        </span>
      </div>
      <div className="flex items-center gap-2 p-3">
        <span className="hako-rec-pulse size-1.5 rounded-full bg-red-500" />
        <span className="text-sm font-medium">Recording your match…</span>
      </div>
    </Surface>
  );
}

/** Storage — a library grid headed by the live folder path. */
function StoragePreview({ draft }: { draft: Settings }) {
  return (
    <Surface>
      <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2 text-xs text-muted-foreground">
        <FolderOpen weight="fill" className="size-4 shrink-0" />
        <span className="truncate">{draft.storage_dir || "Videos/Hako"}</span>
      </div>
      <div className="grid grid-cols-2 gap-2.5 p-3">
        {SAMPLE_CLIPS.map((c) => (
          <ClipCardMini key={c.title} clip={c} />
        ))}
      </div>
    </Surface>
  );
}

// Roughly how the chosen quality would actually look: lower resolution → softer,
// low bitrate → a little flatter. Native / 1080p+ stay crisp.
const RES_BLUR: Record<string, number> = {
  "360p": 3,
  "480p": 1.8,
  "720p": 0.8,
  "1080p": 0,
  "1440p": 0,
  "2160p": 0,
};

/** Video — a clip card whose thumbnail visibly degrades to match the quality. */
function VideoPreview({ draft }: { draft: Settings }) {
  const resLabel = draft.resolution === "native" ? "Native" : draft.resolution;
  const clip = SAMPLE_CLIPS[0];
  const blur = RES_BLUR[draft.resolution] ?? 0; // native + unknown → crisp
  const lowBitrate = draft.bitrate_mbps <= 5;
  const imgStyle: React.CSSProperties =
    blur || lowBitrate
      ? {
          filter: [
            blur ? `blur(${blur}px)` : "",
            lowBitrate ? "contrast(0.92) saturate(0.85)" : "",
          ]
            .filter(Boolean)
            .join(" "),
          // Scale up slightly so the blur doesn't reveal the muted edges.
          transform: blur ? "scale(1.06)" : undefined,
        }
      : {};
  return (
    <Surface>
      <Thumb src={clip.img} imgStyle={imgStyle}>
        <span className="absolute top-2 left-2 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
          {resLabel} · {draft.target_fps}fps
        </span>
        <WinPill won={clip.won} />
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <span className="flex size-11 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
            <Play weight="fill" className="size-5 text-white" />
          </span>
        </div>
        <DurationBadge>{clip.dur}</DurationBadge>
      </Thumb>
      <div className="flex items-center justify-between gap-2 p-3">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold">{clip.title}</h3>
          <div className="mt-0.5 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
            <span>{clip.meta}</span>
            <Dot />
            <span>{draft.bitrate_mbps} Mbps {draft.codec.toUpperCase()}</span>
          </div>
        </div>
      </div>
    </Surface>
  );
}

/** Small animated equalizer to signal a live audio source. */
function Equalizer({ active }: { active: boolean }) {
  // Each bar gets its own height, speed and offset so they never move in sync —
  // the staggered, mismatched durations read as random "live audio" motion.
  const bars = [
    { h: 70, dur: 620, d: 0 },
    { h: 100, dur: 820, d: 110 },
    { h: 45, dur: 520, d: 60 },
    { h: 90, dur: 720, d: 200 },
    { h: 60, dur: 580, d: 90 },
  ];
  return (
    <div className="flex h-5 items-end gap-0.5">
      {bars.map((b, i) => (
        <span
          key={i}
          className={cn(
            "w-1 origin-bottom rounded-full",
            active ? "bg-primary hako-eq-bar" : "bg-muted-foreground/30"
          )}
          style={{
            height: `${b.h}%`,
            animation: active
              ? `hako-eq ${b.dur}ms ease-in-out ${b.d}ms infinite`
              : undefined,
          }}
        />
      ))}
    </div>
  );
}

/** Discord's brand glyph (simple-icons), tinted via `currentColor`. */
function DiscordIcon({ className }: { className?: string }) {
  return (
    <svg
      role="img"
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
      className={className}
    >
      <path d="M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z" />
    </svg>
  );
}

/** Audio — a live mixer of the enabled sources and their volumes. */
function AudioPreview({ draft }: { draft: Settings }) {
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

/** Clips — the real "Clip saved" toast (upload-toast.tsx style) + a keycap. */
function ClipsPreview({ draft }: { draft: Settings }) {
  const clip = SAMPLE_CLIPS[0];
  const dur = `${Math.floor(draft.clip_seconds / 60)}:${String(
    draft.clip_seconds % 60
  ).padStart(2, "0")}`;
  return (
    <div className="flex w-full flex-col items-center gap-6">
      {/* The keycap presses + glows on a loop… */}
      <div className="hako-key flex min-w-16 items-center justify-center rounded-xl border-2 border-border bg-secondary px-6 py-3 text-2xl font-bold tracking-tight">
        {draft.save_hotkey}
      </div>
      <p className="-mt-3 text-xs text-muted-foreground">
        Press to save the last {draft.clip_seconds}s
      </p>

      {/* …and a clip "pops" into the library on each press. */}
      <div className="hako-clip-pop w-full max-w-[260px]">
        <div className="overflow-hidden rounded-xl border border-border/60 bg-card shadow-lg">
          <Thumb src={clip.img}>
            <span className="absolute top-2 left-2 flex items-center gap-1.5 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
              <Check weight="bold" className="size-3 text-emerald-400" />
              Clip saved
            </span>
            <DurationBadge>{dur}</DurationBadge>
          </Thumb>
          <div className="flex items-center gap-2 p-2.5">
            <Check weight="fill" className="size-3.5 shrink-0 text-success" />
            <span className="text-xs font-medium">Saved to your library</span>
          </div>
        </div>
      </div>
    </div>
  );
}

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
function AutoPreview({ draft }: { draft: Settings }) {
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
            <p className="text-[10px] font-medium text-success">
              Saved to your library
            </p>
          </div>
          <Check
            weight="fill"
            className="hako-auto-check ml-auto size-4 shrink-0 text-success"
          />
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

/** Done — a mini library dashboard, ready to go. */
function DonePreview({ draft }: { draft: Settings }) {
  return (
    <Surface>
      <div className="flex items-center justify-between border-b border-border/60 px-3 py-2">
        <span className="text-sm font-semibold">Your clips</span>
        <span className="rounded-full border border-border/60 bg-secondary px-2 py-0.5 text-[10px] font-medium">
          Ready
        </span>
      </div>
      <div className="grid grid-cols-2 gap-2.5 p-3">
        {SAMPLE_CLIPS.slice(0, 2).map((c) => (
          <ClipCardMini key={c.title} clip={c} />
        ))}
      </div>
      <div className="flex items-center gap-2 border-t border-border/60 px-3 py-2 text-xs text-muted-foreground">
        <span className="size-1.5 animate-pulse rounded-full bg-red-500" />
        Recording armed · {draft.save_hotkey} to clip
      </div>
    </Surface>
  );
}

/**
 * The right-hand "visualization" panel of the onboarding wizard. Every step maps
 * to a live mock built from the real app's UI vocabulary, reacting to the form
 * on the left — no headers/descriptions here (those live with the form).
 */
export function StepPreview({ step, draft }: { step: string; draft: Settings }) {
  switch (step) {
    case "storage":
      return <StoragePreview draft={draft} />;
    case "video":
      return <VideoPreview draft={draft} />;
    case "audio":
      return <AudioPreview draft={draft} />;
    case "clips":
      return <ClipsPreview draft={draft} />;
    case "auto":
      return <AutoPreview draft={draft} />;
    case "done":
      return <DonePreview draft={draft} />;
    default:
      return <WelcomePreview />;
  }
}
