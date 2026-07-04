import { SpeakerHigh } from "@phosphor-icons/react";

import { SectionHero } from "@/components/settings/primitives";
import { RecordingAudio } from "@/components/settings/recording-audio";
import type { SettingsSet } from "@/components/settings/config";
import { effectiveAudioConfig, type AudioConfig, type Settings } from "@/lib/api";

export function AudioSection({ draft, set }: { draft: Settings; set: SettingsSet }) {
  return (
    <>
      <SectionHero
        icon={SpeakerHigh}
        title="Recording Audio"
        subtitle="Choose which sources are recorded, set their volumes, and split them onto separate tracks."
      />
      <RecordingAudio
        audio={effectiveAudioConfig(draft)}
        onChange={(audio: AudioConfig) => set("audio", audio)}
      />
    </>
  );
}
