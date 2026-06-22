import { Pulse } from "@phosphor-icons/react";

import { SectionHero } from "@/components/settings/primitives";
import { RecordingStatus } from "@/components/settings/recording-status";

export function StatusSection() {
  return (
    <>
      <SectionHero
        icon={Pulse}
        title="Status"
        subtitle="Live recorder, encoder, and GPU detection."
      />
      <RecordingStatus />
    </>
  );
}
