import { useQuery } from "@tanstack/react-query";
import { Flask } from "@phosphor-icons/react";

import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  processLoopbackSupported,
  type AudioConfig,
} from "@/lib/api";
import { Panel } from "./recording-audio/primitives";
import { AllPcAudioPanel } from "./recording-audio/all-pc-audio-panel";
import { AppAudioPanel } from "./recording-audio/app-audio-panel";

export function RecordingAudio({
  audio,
  onChange,
}: {
  audio: AudioConfig;
  onChange: (next: AudioConfig) => void;
}) {
  const { data: supported } = useQuery({
    queryKey: ["process-loopback-supported"],
    queryFn: processLoopbackSupported,
    retry: false,
  });

  const patch = (p: Partial<AudioConfig>) => onChange({ ...audio, ...p });

  const supportsApps = supported ?? false;
  const modeUnsupported = audio.mode === "specific_apps" && !supportsApps;

  return (
    <div className="space-y-6">
      {/* Recording mode */}
      <Panel title="Recording mode">
        <div className="flex items-center justify-between gap-6 py-3 first:pt-0 last:pb-0">
          <div className="min-w-0">
            <div className="text-sm font-medium">Source</div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Capture all system audio, or split it per application.
            </p>
          </div>
          <Select
            value={audio.mode}
            onValueChange={(v) => patch({ mode: v })}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all_pc_audio">All PC audio</SelectItem>
              <SelectItem value="specific_apps" disabled={!supportsApps}>
                <span className="flex items-center gap-2">
                  Specific apps
                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold text-amber-500">
                    Experimental
                  </span>
                </span>
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        {!supportsApps && (
          <p className="flex items-start gap-2 py-3 text-xs text-muted-foreground last:pb-0">
            <Flask className="mt-0.5 size-3.5 shrink-0" weight="fill" />
            Per-app capture needs Windows 11 (build 20348+); it isn&apos;t available
            on this system, so recording stays on All PC audio.
          </p>
        )}
      </Panel>

      {modeUnsupported && (
        <div className="flex gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-500">
          <Flask className="size-4 shrink-0" weight="fill" />
          <span>
            Specific apps isn&apos;t supported on this PC, so capture will fall
            back to All PC audio. Switch the mode above to clear this.
          </span>
        </div>
      )}

      {audio.mode === "all_pc_audio" ? (
        <AllPcAudioPanel audio={audio} patch={patch} />
      ) : (
        <AppAudioPanel audio={audio} patch={patch} />
      )}

      {/* Separate audio tracks */}
      <Panel>
        <div className="flex items-center justify-between gap-6 py-1">
          <div className="min-w-0">
            <div className="text-sm font-medium">Separate audio tracks</div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Save each source as its own track so you can mute them separately in
              the editor. The clip still plays as the full mix.
            </p>
          </div>
          <Switch
            checked={audio.separate_tracks}
            onCheckedChange={(v) => patch({ separate_tracks: v })}
          />
        </div>
      </Panel>
    </div>
  );
}
