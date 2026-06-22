import * as React from "react";
import {
  SpeakerSimpleHigh,
  SpeakerSimpleX,
  CircleNotch,
  Faders,
  Sparkle,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Slider } from "@/components/ui/slider";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import type { AudioTrackInfo } from "@/lib/api";
import type { TrackCtl } from "./constants";

/**
 * The toolbar's "Audio" control: a popover holding the master include-audio
 * toggle plus, for multi-track clips, the per-stem mute/solo/volume mixer. The
 * recorded clip carries a master mix (track 0, what the player uses) plus raw
 * stems; these controls choose how the stems are re-mixed into the export. Solo
 * overrides mute: if any stem is soloed, only soloed stems are audible.
 */
export const AudioSettingsPopover = React.memo(function AudioSettingsPopover({
  audioEnabled,
  onToggleAudio,
  hasStems,
  stems,
  decoding,
  denoisingIdx,
  ctlOf,
  soloActive,
  onMute,
  onSolo,
  onVolume,
  onDenoise,
}: {
  audioEnabled: boolean;
  onToggleAudio: () => void;
  hasStems: boolean;
  stems: AudioTrackInfo[];
  /** Decoding the stems for the live preview mix; controls aren't audible yet. */
  decoding: boolean;
  /** Stem indices whose noise-cancel preview is still being computed (spinner). */
  denoisingIdx: number[];
  ctlOf: (idx: number) => TrackCtl;
  soloActive: boolean;
  onMute: (idx: number) => void;
  onSolo: (idx: number) => void;
  onVolume: (idx: number, v: number) => void;
  onDenoise: (idx: number) => void;
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          title="Audio settings"
          className={cn(
            "flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-sm font-medium transition-colors",
            audioEnabled
              ? "border-border/70 bg-card/50 text-foreground hover:bg-card"
              : "border-border/50 bg-transparent text-muted-foreground hover:text-foreground",
          )}
        >
          {audioEnabled ? (
            <SpeakerSimpleHigh weight="fill" className="size-4" />
          ) : (
            <SpeakerSimpleX weight="fill" className="size-4" />
          )}
          Audio
          <Faders weight="bold" className="size-4 opacity-70" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[24rem] p-0">
        {/* Master toggle: include this clip's audio in the export */}
        <button
          type="button"
          role="switch"
          aria-checked={audioEnabled}
          onClick={onToggleAudio}
          className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left transition-colors hover:bg-white/5"
        >
          <span className="min-w-0">
            <span className="block text-sm font-medium text-foreground">
              Include audio
            </span>
            <span className="block text-xs text-muted-foreground">
              {audioEnabled ? "Saved with sound" : "Saved without sound"}
            </span>
          </span>
          <span
            className={cn(
              "relative h-5 w-9 shrink-0 rounded-full transition-colors",
              audioEnabled ? "bg-primary" : "bg-muted-foreground/30",
            )}
          >
            <span
              className={cn(
                "absolute top-0.5 left-0.5 size-4 rounded-full bg-white transition-transform",
                audioEnabled && "translate-x-4",
              )}
            />
          </span>
        </button>

        {/* Per-stem mixer (multi-track clips only, when audio is kept) */}
        {hasStems && audioEnabled ? (
          <div className="border-t border-panel-border px-4 py-3">
            <div className="mb-2.5 flex items-center gap-2 text-xs font-medium text-muted-foreground">
              Tracks
              {decoding ? (
                <span className="flex items-center gap-1.5 font-normal text-muted-foreground/70">
                  <CircleNotch weight="bold" className="size-3 animate-spin" />
                  Decoding…
                </span>
              ) : denoisingIdx.length ? (
                // Text, not just the spinner: the OS "reduce motion" setting
                // freezes every CSS animation, so a lone spinner reads as idle —
                // the label is what tells the user noise cancel is working.
                <span className="flex items-center gap-1.5 font-normal text-info/80">
                  <CircleNotch weight="bold" className="size-3 animate-spin" />
                  Cancelling noise…
                </span>
              ) : null}
            </div>
            <div
              className={cn(
                "flex flex-col gap-2.5",
                // While decoding, the per-stem controls can't be *heard* yet
                // (native master audio plays meanwhile) — blur + dim them so it
                // reads as "preparing," not "broken." Snap it on/off: animating
                // the `filter` repaints every frame and stutters.
                decoding && "pointer-events-none select-none opacity-50 blur-[2px]",
              )}
              aria-busy={decoding}
            >
              {stems.map((s) => {
                const c = ctlOf(s.index);
                const audible = soloActive ? c.solo : !c.muted;
                const denoising = denoisingIdx.includes(s.index);
                return (
                  <div key={s.index} className="flex items-center gap-2">
                    <span
                      className={cn(
                        "w-20 shrink-0 truncate text-[13px]",
                        audible ? "text-foreground" : "text-muted-foreground/60",
                      )}
                      title={s.name}
                    >
                      {s.name}
                    </span>
                    <button
                      type="button"
                      onClick={() => onMute(s.index)}
                      aria-label={c.muted ? "Unmute track" : "Mute track"}
                      title={c.muted ? "Unmute" : "Mute"}
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border transition-colors",
                        c.muted
                          ? "border-destructive/40 bg-destructive/10 text-destructive"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {c.muted ? (
                        <SpeakerSimpleX weight="fill" className="size-3.5" />
                      ) : (
                        <SpeakerSimpleHigh weight="fill" className="size-3.5" />
                      )}
                    </button>
                    <button
                      type="button"
                      onClick={() => onSolo(s.index)}
                      aria-label={c.solo ? "Unsolo track" : "Solo track"}
                      title="Solo"
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border text-xs font-bold transition-colors",
                        c.solo
                          ? "border-primary/50 bg-primary/15 text-primary-text"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      S
                    </button>
                    <button
                      type="button"
                      onClick={() => onDenoise(s.index)}
                      aria-label={c.denoise ? "Disable noise cancel" : "Enable noise cancel"}
                      aria-pressed={c.denoise}
                      aria-busy={denoising}
                      title={
                        denoising
                          ? "Preparing noise cancel…"
                          : c.denoise
                            ? "Noise cancel on (removes background noise on export)"
                            : "Noise cancel off"
                      }
                      className={cn(
                        "flex size-7 shrink-0 items-center justify-center rounded-md border transition-colors",
                        c.denoise
                          ? "border-info/50 bg-info/15 text-info"
                          : "border-border/70 bg-card/50 text-muted-foreground hover:text-foreground",
                      )}
                    >
                      {denoising ? (
                        <CircleNotch weight="bold" className="size-3.5 animate-spin" />
                      ) : (
                        <Sparkle
                          weight={c.denoise ? "fill" : "regular"}
                          className="size-3.5"
                        />
                      )}
                    </button>
                    <Slider
                      min={0}
                      // 200% = +6 dB boost. gain (volume/100) is unclamped in
                      // both preview (Web Audio GainNode) and export
                      // (`remux_with_tracks`), so a stem can be amplified, not
                      // just attenuated; >100% may clip if the stem is already hot.
                      max={200}
                      value={[c.volume]}
                      onValueChange={([v]) => onVolume(s.index, v)}
                      disabled={!audible}
                      aria-label={`${s.name} volume`}
                      className="min-w-0 flex-1"
                    />
                    <span className="w-9 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
                      {c.volume}%
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        ) : null}
      </PopoverContent>
    </Popover>
  );
});
