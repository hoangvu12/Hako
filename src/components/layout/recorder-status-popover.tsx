import { useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowRight,
  GameController,
  GearSix,
  SpeakerHigh,
  Sparkle,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  Popover,
  PopoverClose,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { effectiveAudioConfig } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";
import { useRecorderStatus } from "@/hooks/use-recorder";
import { useSettings } from "@/hooks/use-settings";

/** A labelled row inside the popover: icon + label on the left, control right. */
function Row({
  icon: Icon,
  label,
  children,
}: {
  icon: typeof SpeakerHigh;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 px-4 py-2">
      <div className="flex flex-1 items-center gap-2.5 text-sm font-medium whitespace-nowrap text-foreground">
        <Icon className="size-4 text-muted-foreground" weight="fill" />
        {label}
      </div>
      <div className="flex w-40 shrink-0 justify-end">{children}</div>
    </div>
  );
}

/** A deep-link button styled like the popover's elevated controls. */
function LinkButton({
  children,
  onClick,
}: {
  children: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <PopoverClose asChild>
      <button
        type="button"
        onClick={onClick}
        className="flex h-8 w-full items-center justify-between gap-2 rounded-md border border-white/10 bg-secondary px-2.5 text-sm text-foreground shadow-xs transition-colors hover:bg-[#323236]"
      >
        <span className="truncate whitespace-nowrap">{children}</span>
        <GearSix className="size-4 shrink-0 text-muted-foreground" />
      </button>
    </PopoverClose>
  );
}

/**
 * Medal-style recorder popover anchored to the "game status" pill in the
 * titlebar. Filtered to what Hako actually supports: live detection status and
 * compact Quality + Audio summaries that deep-link to their Settings sections.
 * Recording Audio is configured entirely in Settings (the source of truth), so
 * this surface is read-only.
 */
export function RecorderStatusPopover() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { data: status } = useRecorderStatus();
  const { data: settings } = useSettings();

  // The window being present (`detected`) is NOT the same as a capture actually
  // running (`capturing`): the hook can fail to inject (anti-cheat, minimized,
  // missing binaries) while the game window still exists. Gate the "Now Clipping"
  // indicator on `capturing` so it can't claim to be recording when it isn't —
  // otherwise the buffer is empty and "Save last 30s" fails with "no capture".
  const detected = status?.detected_game != null;
  const capturing = status?.capturing ?? false;
  // A capture can be running but frozen — the game is minimized (or otherwise not
  // presenting), so the hook re-copies one stale frame. `capturing_live` is false
  // then; surface an honest "paused" state rather than a green "Now Clipping" that
  // would imply live footage is being buffered.
  const live = status?.capturing_live ?? false;
  const frozen = capturing && !live;
  // The actual detected game, so the indicator names it instead of always
  // claiming "Valorant" (the label is game-agnostic now).
  const gameName = status?.detected_game ?? "Game";

  const fps = settings?.target_fps ?? 60;
  const codec = (settings?.codec ?? "h264").toUpperCase();
  const audio = settings
    ? effectiveAudioConfig(settings)
    : { mode: "all_pc_audio", mic_enabled: false };
  const modeLabel =
    audio.mode === "specific_apps" ? "Specific apps" : "All PC audio";
  const audioSummary = `${modeLabel} · Mic ${audio.mic_enabled ? "on" : "off"}`;

  const recheck = () => {
    qc.invalidateQueries({ queryKey: queryKeys.recorderStatus });
    qc.invalidateQueries({ queryKey: queryKeys.valorantStatus });
  };

  const goToSettings = (section?: string) =>
    navigate({ to: "/settings", search: section ? { section } : {} });

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(
            "flex h-8 items-center gap-2.5 rounded-lg border border-border bg-secondary/50 px-3 text-sm font-medium transition-colors hover:bg-secondary",
            capturing || detected ? "text-foreground" : "text-foreground/90"
          )}
        >
          {capturing && !frozen ? (
            <>
              <span className="relative flex size-2">
                <span className="absolute inline-flex size-full animate-ping rounded-full bg-success/70" />
                <span className="relative inline-flex size-2 rounded-full bg-success" />
              </span>
              Now Clipping {gameName}
            </>
          ) : frozen ? (
            <>
              <span className="relative inline-flex size-2 rounded-full bg-amber-400" />
              Paused, Game Minimized
            </>
          ) : detected ? (
            <>
              <span className="relative inline-flex size-2 rounded-full bg-amber-400" />
              {gameName} Detected
            </>
          ) : (
            <>
              <GameController className="size-4" weight="regular" />
              Waiting For Game
            </>
          )}
        </button>
      </PopoverTrigger>

      <PopoverContent className="w-[368px]">
        {/* Detection status card */}
        <div className="p-3">
          <div className="rounded-lg bg-secondary/40 px-4 py-5 text-center">
            {capturing && !frozen ? (
              <>
                <div className="mb-1 flex items-center justify-center gap-2">
                  <span className="relative flex size-2.5">
                    <span className="absolute inline-flex size-full animate-ping rounded-full bg-success/70" />
                    <span className="relative inline-flex size-2.5 rounded-full bg-success" />
                  </span>
                </div>
                <div className="text-sm font-semibold text-foreground">
                  Now clipping {gameName}
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  {status?.message ?? "Gameplay is being buffered."}
                </p>
              </>
            ) : frozen ? (
              <>
                <div className="mb-1 flex items-center justify-center gap-2">
                  <span className="relative inline-flex size-2.5 rounded-full bg-amber-400" />
                </div>
                <div className="text-sm font-semibold text-foreground">
                  Paused, game minimized
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  The game stopped presenting frames, so clipping is paused to
                  avoid recording a frozen screen. It resumes when you return.
                </p>
              </>
            ) : detected ? (
              <>
                <div className="mb-1 flex items-center justify-center gap-2">
                  <span className="relative inline-flex size-2.5 rounded-full bg-amber-400" />
                </div>
                <div className="text-sm font-semibold text-foreground">
                  {gameName} detected, not recording yet
                </div>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  Capture hasn&apos;t started, so there&apos;s nothing to save.
                </p>
                <button
                  type="button"
                  onClick={recheck}
                  className="mt-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                >
                  Retry detection
                </button>
              </>
            ) : (
              <>
                <GameController
                  className="mx-auto mb-2 size-7 text-muted-foreground"
                  weight="fill"
                />
                <div className="text-sm font-semibold text-foreground">
                  Waiting for game to be detected
                </div>
                <button
                  type="button"
                  onClick={recheck}
                  className="mt-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
                >
                  Click here if we haven&apos;t detected your game yet
                </button>
              </>
            )}
          </div>
        </div>

        <div className="border-t border-panel-border py-1">
          <Row icon={Sparkle} label="Quality">
            <LinkButton onClick={() => goToSettings("quality")}>
              <span className="tabular-nums">
                {fps} FPS · {codec}
              </span>
            </LinkButton>
          </Row>

          <Row icon={SpeakerHigh} label="Audio">
            <LinkButton onClick={() => goToSettings("audio")}>
              {audioSummary}
            </LinkButton>
          </Row>
        </div>

        <PopoverClose asChild>
          <button
            type="button"
            onClick={() => goToSettings("audio")}
            className="flex w-full items-center justify-between border-t border-panel-border px-4 py-3 text-sm font-medium text-foreground transition-colors hover:bg-secondary/60"
          >
            Recording settings
            <ArrowRight className="size-4 text-muted-foreground" />
          </button>
        </PopoverClose>
      </PopoverContent>
    </Popover>
  );
}
