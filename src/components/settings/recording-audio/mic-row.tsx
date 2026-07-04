import { useQuery } from "@tanstack/react-query";
import { Microphone } from "@phosphor-icons/react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { listAudioInputs, AUTO_DEVICE, type AudioConfig } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";
import { SourceRow, VolumeSlider } from "./primitives";

/**
 * The microphone source row, shared by both recording modes. Owns its own input-
 * device query (cached, no polling) so neither mode panel needs to thread the
 * device list through.
 */
export function MicRow({
  audio,
  patch,
}: {
  audio: AudioConfig;
  patch: (p: Partial<AudioConfig>) => void;
}) {
  const { data: micDevices } = useQuery({
    queryKey: queryKeys.audioInputs,
    queryFn: listAudioInputs,
    retry: false,
  });

  return (
    <SourceRow
      icon={Microphone}
      label="Microphone"
      hint={audio.mic_mono ? "Mono" : undefined}
      checked={audio.mic_enabled}
      onCheckedChange={(v) => patch({ mic_enabled: v })}
    >
      <div className="flex items-center gap-2">
        <Select value={audio.mic_source} onValueChange={(v) => patch({ mic_source: v })}>
          <SelectTrigger size="sm" className="w-32">
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="max-w-[280px]">
            <SelectItem value={AUTO_DEVICE}>Auto</SelectItem>
            {micDevices?.map((d) => (
              <SelectItem key={d.id} value={d.id}>
                {d.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <VolumeSlider value={audio.mic_volume} onCommit={(v) => patch({ mic_volume: v })} />
      </div>
    </SourceRow>
  );
}
