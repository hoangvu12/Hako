import {
  Trophy,
  Crown,
  Sword,
  Fire,
  Knife,
  Skull,
  Handshake,
  Bomb,
  Wrench,
  type Icon,
} from "@phosphor-icons/react";

import type { AutoCaptureMode, EventToggles, Settings } from "@/lib/api";

// A single typed mutator shared by every step component: instant-apply a settings
// field to the live draft (mirrors the settings page). Kept local so the wizard
// stays self-contained.
export type WizardSet = <K extends keyof Settings>(key: K, value: Settings[K]) => void;

// --- Curated option sets for the wizard. These mirror the equivalents in
// `routes/settings.tsx`; kept local so the wizard is self-contained and editing
// the full settings page can't accidentally reshape the first-run flow. ------

export type PresetKey = "low" | "standard" | "high";
export const PRESETS: {
  key: PresetKey;
  label: string;
  blurb: string;
  line: string;
  resolution: string;
  fps: number;
  bitrate: number;
}[] = [
  { key: "low", label: "Low Quality", blurb: "Lower-end PCs & faster uploads", line: "360p · 24 FPS", resolution: "360p", fps: 24, bitrate: 3 },
  { key: "standard", label: "Standard", blurb: "Performance & fast sharing", line: "720p · 60 FPS", resolution: "720p", fps: 60, bitrate: 10 },
  { key: "high", label: "High Quality", blurb: "Higher quality & slower uploads", line: "1080p · 60 FPS", resolution: "1080p", fps: 60, bitrate: 15 },
];

export const CLIP_LENGTHS = [10, 15, 30, 60, 90, 120, 180];

// Custom-quality knobs (mirror routes/settings.tsx), shown when the preset is
// "Custom".
export const RESOLUTIONS: { value: string; label: string }[] = [
  { value: "native", label: "Native (no scaling)" },
  { value: "360p", label: "360p" },
  { value: "480p", label: "480p" },
  { value: "720p", label: "HD (720p)" },
  { value: "1080p", label: "Full HD (1080p)" },
  { value: "1440p", label: "QHD (1440p)" },
  { value: "2160p", label: "UHD 4K (2160p)" },
];
export const FPS_OPTIONS = [24, 30, 60, 120, 144, 240];
export const BITRATE_OPTIONS = [3, 5, 8, 10, 15, 20, 30, 50];

export const CAPTURE_MODES: { key: AutoCaptureMode; label: string; blurb: string }[] = [
  { key: "manual", label: "Manual only", blurb: "Don't auto-capture; buffer + hotkey still work" },
  { key: "highlights", label: "Highlights", blurb: "Auto-clip the game events below" },
  { key: "full_match", label: "Full match", blurb: "Keep the entire match as one clip" },
  { key: "session", label: "Full session", blurb: "Record the whole time you're in-game" },
];

export const EVENT_LABELS: { key: keyof EventToggles; label: string; hint: string; icon: Icon }[] = [
  { key: "victory", label: "Victory", hint: "You won the match", icon: Trophy },
  { key: "clutch", label: "Clutch", hint: "1vX round win as the last alive", icon: Crown },
  { key: "kill", label: "Kill", hint: "Any elimination", icon: Sword },
  { key: "double_kill", label: "Double kill", hint: "Two in quick succession", icon: Sword },
  { key: "triple_kill", label: "Triple kill", hint: "3K", icon: Sword },
  { key: "quadra_kill", label: "Quadra kill", hint: "4K", icon: Sword },
  { key: "ace", label: "Ace", hint: "Full team wipe (5K)", icon: Fire },
  { key: "knife", label: "Knife kill", hint: "Melee elimination", icon: Knife },
  { key: "death", label: "Death", hint: "Your deaths", icon: Skull },
  { key: "assist", label: "Assist", hint: "Assisted eliminations", icon: Handshake },
  { key: "spike_detonated", label: "Spike detonated", hint: "A spike you planted exploded", icon: Bomb },
  { key: "spike_defused", label: "Spike defused", hint: "You defused the spike", icon: Wrench },
];

export type StepKey =
  | "welcome"
  | "storage"
  | "video"
  | "audio"
  | "clips"
  | "auto"
  | "done";

// Ordered so the essentials needed for a first clip (storage → quality → audio →
// hotkey) come first; auto-capture is `optional` and reachable via a "Finish
// now" exit, per the "work backward to the first win" onboarding rule.
export const STEPS: { key: StepKey; nav: string; optional?: boolean }[] = [
  { key: "welcome", nav: "Welcome" },
  { key: "storage", nav: "Storage" },
  { key: "video", nav: "Video" },
  { key: "audio", nav: "Audio" },
  { key: "clips", nav: "Clips" },
  { key: "auto", nav: "Auto-capture", optional: true },
  { key: "done", nav: "Done" },
];
