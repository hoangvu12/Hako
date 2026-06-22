import { Surface } from "./shared";

/** Welcome — a single clip card, mid-recording. */
export function WelcomePreview() {
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
