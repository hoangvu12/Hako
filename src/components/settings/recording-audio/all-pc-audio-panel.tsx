import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { SpeakerHigh } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Checkbox } from "@/components/ui/checkbox";
import { listAudioOutputs, AUTO_DEVICE, type AudioConfig } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";
import { Panel, SourceRow, VolumeSlider } from "./primitives";
import { MicRow } from "./mic-row";

/**
 * "All PC audio" mode: capture system sound as loopback, with a multi-select of
 * output devices. Owns its own output-device query so the per-app mode never
 * loads it.
 */
export function AllPcAudioPanel({
  audio,
  patch,
}: {
  audio: AudioConfig;
  patch: (p: Partial<AudioConfig>) => void;
}) {
  const { data: outputs } = useQuery({
    queryKey: queryKeys.audioOutputs,
    queryFn: listAudioOutputs,
    retry: false,
  });

  // Always lead with the synthetic "Default Output Device" (auto), then live
  // render endpoints. A device is captured when its config entry is enabled.
  const devices = useMemo(() => {
    const live = (outputs ?? []).filter((d) => d.id !== AUTO_DEVICE);
    return [{ id: AUTO_DEVICE, name: "Default Output Device" }, ...live];
  }, [outputs]);
  const deviceEnabled = (id: string) =>
    audio.pc_audio.find((d) => d.id === id)?.enabled ?? false;
  const pcEnabled = audio.pc_audio.some((d) => d.enabled);

  const setDevice = (id: string, name: string, on: boolean) => {
    const idx = audio.pc_audio.findIndex((d) => d.id === id);
    let pc_audio = audio.pc_audio;
    if (idx >= 0) {
      pc_audio = [...pc_audio];
      pc_audio[idx] = { ...pc_audio[idx], enabled: on };
    } else if (on) {
      pc_audio = [...pc_audio, { id, name, enabled: true, volume: 100 }];
    }
    patch({ pc_audio });
  };
  const setPcEnabled = (on: boolean) => {
    if (!on) {
      patch({ pc_audio: audio.pc_audio.map((d) => ({ ...d, enabled: false })) });
      return;
    }
    // Re-enable the default output (add it if the list was emptied).
    if (audio.pc_audio.some((d) => d.id === AUTO_DEVICE)) {
      patch({
        pc_audio: audio.pc_audio.map((d) =>
          d.id === AUTO_DEVICE ? { ...d, enabled: true } : d
        ),
      });
    } else {
      patch({
        pc_audio: [
          { id: AUTO_DEVICE, name: "Default Output Device", enabled: true, volume: 100 },
          ...audio.pc_audio,
        ],
      });
    }
  };

  return (
    <Panel title="PC audio" hint="System sound captured as loopback.">
      <SourceRow
        icon={SpeakerHigh}
        label="PC Audio"
        checked={pcEnabled}
        onCheckedChange={setPcEnabled}
      >
        <VolumeSlider
          value={audio.master_volume}
          onCommit={(v) => patch({ master_volume: v })}
        />
      </SourceRow>
      {/* Output device multi-select. */}
      <div className="py-3 last:pb-0">
        <div className="mb-2 pl-7 text-xs font-medium text-muted-foreground">
          Output devices
        </div>
        <div className="space-y-1">
          {devices.map((d) => (
            <label
              key={d.id}
              className={cn(
                "flex cursor-pointer items-center gap-3 rounded-md px-7 py-1.5 hover:bg-accent/40",
                !pcEnabled && "pointer-events-none opacity-40"
              )}
            >
              <Checkbox
                checked={deviceEnabled(d.id)}
                disabled={!pcEnabled}
                onCheckedChange={(v) => setDevice(d.id, d.name, v === true)}
              />
              <span className="truncate text-sm">{d.name}</span>
            </label>
          ))}
        </div>
      </div>
      <MicRow audio={audio} patch={patch} />
    </Panel>
  );
}
