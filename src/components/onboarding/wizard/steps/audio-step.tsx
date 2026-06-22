import { SpeakerHigh } from "@phosphor-icons/react";

import { SectionHero } from "@/components/settings/primitives";
import { RecordingAudio } from "@/components/settings/recording-audio";
import { effectiveAudioConfig, type AudioConfig, type Settings } from "@/lib/api";
import type { WizardSet } from "../config";

export function AudioStep({ draft, set }: { draft: Settings; set: WizardSet }) {
  return (
    <>
      <SectionHero
        icon={SpeakerHigh}
        title="Recording audio"
        subtitle="Choose which sources are recorded and set their volumes."
      />
      <RecordingAudio
        audio={effectiveAudioConfig(draft)}
        onChange={(audio: AudioConfig) => set("audio", audio)}
      />
    </>
  );
}
