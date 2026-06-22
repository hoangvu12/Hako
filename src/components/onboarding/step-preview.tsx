import type { Settings } from "@/lib/api";
import { WelcomePreview } from "./step-preview/welcome-preview";
import { StoragePreview } from "./step-preview/storage-preview";
import { VideoPreview } from "./step-preview/video-preview";
import { AudioPreview } from "./step-preview/audio-preview";
import { ClipsPreview } from "./step-preview/clips-preview";
import { AutoPreview } from "./step-preview/auto-preview";
import { DonePreview } from "./step-preview/done-preview";

/**
 * The right-hand "visualization" panel of the onboarding wizard. Every step maps
 * to a live mock built from the real app's UI vocabulary, reacting to the form
 * on the left — no headers/descriptions here (those live with the form).
 *
 * Each preview is its own component (under `step-preview/`), so switching steps
 * mounts only the active one and the others never render.
 */
export function StepPreview({ step, draft }: { step: string; draft: Settings }) {
  switch (step) {
    case "storage":
      return <StoragePreview draft={draft} />;
    case "video":
      return <VideoPreview draft={draft} />;
    case "audio":
      return <AudioPreview draft={draft} />;
    case "clips":
      return <ClipsPreview draft={draft} />;
    case "auto":
      return <AutoPreview draft={draft} />;
    case "done":
      return <DonePreview draft={draft} />;
    default:
      return <WelcomePreview />;
  }
}
