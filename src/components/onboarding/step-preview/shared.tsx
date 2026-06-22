import { cn } from "@/lib/utils";

// The right-hand panel is purely VISUAL and mirrors the REAL app UI as closely
// as possible — clip cards (clip-card.tsx) and toasts (upload-toast.tsx) — so it
// reads as Hako, not a generic mock. No header/description here (those live with
// the form on the left), and no invented gradient "scenes": thumbnails are the
// app's flat `bg-muted`, optionally backed by a real screenshot dropped into
// `public/onboarding/` (falls back to muted if the file is absent).

/** Optional real screenshots; if a file is missing the <img> hides → bg-muted. */
export type Sample = { title: string; img: string; dur: string; meta: string; won: boolean };
export const SAMPLE_CLIPS: Sample[] = [
  { title: "Ace on Haven", img: "/onboarding/clip-1.jpg", dur: "0:18", meta: "Ace", won: true },
  { title: "1v3 Clutch", img: "/onboarding/clip-2.jpg", dur: "0:24", meta: "Clutch", won: true },
  { title: "Triple kill", img: "/onboarding/clip-3.jpg", dur: "0:12", meta: "Triple kill", won: false },
  { title: "Spike defuse", img: "/onboarding/clip-4.jpg", dur: "0:09", meta: "Defuse", won: true },
];

export const hideOnError = (e: React.SyntheticEvent<HTMLImageElement>) => {
  e.currentTarget.style.display = "none";
};

/** Card shell mirroring the real clip card / toast container. */
export function Surface({ children, className }: { children: React.ReactNode; className?: string }) {
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
export function Thumb({
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

export function WinPill({ won }: { won: boolean }) {
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

export function DurationBadge({ children }: { children: React.ReactNode }) {
  return (
    <span className="pointer-events-none absolute right-2 bottom-2 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white">
      {children}
    </span>
  );
}

export function Dot() {
  return <span className="size-[3px] shrink-0 rounded-full bg-secondary" />;
}

/** A faithful, compact clip card (matches clip-card.tsx's structure). */
export function ClipCardMini({ clip, badges }: { clip: Sample; badges?: React.ReactNode }) {
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

/** Small animated equalizer to signal a live audio source. */
export function Equalizer({ active }: { active: boolean }) {
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
export function DiscordIcon({ className }: { className?: string }) {
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
