import {
  Scissors,
  SlidersHorizontal,
  Crosshair,
  Monitor,
  HardDrives,
  SpeakerHigh,
  Trophy,
  Crown,
  Sword,
  Fire,
  Knife,
  Skull,
  Handshake,
  Bomb,
  Wrench,
  Bell,
  CloudArrowUp,
  Pulse,
  Drop,
  Star,
  Eye,
  Buildings,
  Cube,
  Shield,
  type Icon,
} from "@phosphor-icons/react";

import type {
  AutoCaptureMode,
  EventToggles,
  GameModeToggles,
  LolEventToggles,
  Settings,
} from "@/lib/api";

export type SectionKey =
  | "clip"
  | "quality"
  | "audio"
  | "auto"
  | "capture"
  | "storage"
  | "cloud"
  | "overlay"
  | "status";

const SECTION_KEYS = new Set<SectionKey>([
  "clip",
  "quality",
  "audio",
  "auto",
  "capture",
  "storage",
  "cloud",
  "overlay",
  "status",
]);
export const isSectionKey = (v: unknown): v is SectionKey =>
  typeof v === "string" && SECTION_KEYS.has(v as SectionKey);

/** Shared signature for the page's instant-apply / local-edit settings mutators. */
export type SettingsSet = <K extends keyof Settings>(
  key: K,
  value: Settings[K],
) => void;

export const NAV: {
  group: string;
  items: { key: SectionKey; label: string; icon: Icon }[];
}[] = [
  {
    group: "Recording",
    items: [
      { key: "clip", label: "Clips", icon: Scissors },
      { key: "auto", label: "Auto-Capture", icon: Crosshair },
      { key: "quality", label: "Video", icon: SlidersHorizontal },
      { key: "audio", label: "Recording Audio", icon: SpeakerHigh },
      { key: "capture", label: "Capture", icon: Monitor },
    ],
  },
  {
    group: "System",
    items: [
      { key: "storage", label: "Storage", icon: HardDrives },
      { key: "cloud", label: "Cloud Upload", icon: CloudArrowUp },
      { key: "overlay", label: "Overlay", icon: Bell },
      { key: "status", label: "Status", icon: Pulse },
    ],
  },
];

export const EVENT_LABELS: {
  key: keyof EventToggles;
  label: string;
  hint: string;
  icon: Icon;
}[] = [
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

// League of Legends auto-clip events (mirrors Rust `LolEventToggles`). Champion
// combat + objectives + the match result.
export const LOL_EVENT_LABELS: {
  key: keyof LolEventToggles;
  label: string;
  hint: string;
  icon: Icon;
}[] = [
  { key: "victory", label: "Victory", hint: "You won the match", icon: Trophy },
  { key: "pentakill", label: "Pentakill", hint: "Five-kill streak", icon: Star },
  { key: "ace", label: "Ace", hint: "Your team aced the enemy", icon: Crown },
  { key: "first_blood", label: "First blood", hint: "First takedown of the game", icon: Drop },
  { key: "quadra_kill", label: "Quadra kill", hint: "4-kill streak", icon: Sword },
  { key: "triple_kill", label: "Triple kill", hint: "3-kill streak", icon: Sword },
  { key: "double_kill", label: "Double kill", hint: "2-kill streak", icon: Sword },
  { key: "kill", label: "Kill", hint: "Any takedown", icon: Sword },
  { key: "death", label: "Death", hint: "Your deaths", icon: Skull },
  { key: "assist", label: "Assist", hint: "Assisted takedowns", icon: Handshake },
  { key: "baron", label: "Baron Nashor", hint: "You secured Baron", icon: Shield },
  { key: "dragon", label: "Dragon", hint: "You secured a dragon", icon: Fire },
  { key: "herald", label: "Rift Herald", hint: "You secured the Herald", icon: Eye },
  { key: "turret", label: "Turret", hint: "You destroyed a turret", icon: Buildings },
  { key: "inhibitor", label: "Inhibitor", hint: "You destroyed an inhibitor", icon: Cube },
];

// Slider ranges for the per-event timing rows. Before can run long (the 45 s
// spike fuse); after is shorter.
export const MAX_BEFORE_SECS = 60;
export const MAX_AFTER_SECS = 30;

// Outplayed-style capture modes for the Auto Clipping section. Mirrors the Rust
// `AutoCaptureMode`; the cards write `auto_capture_mode`.
export const CAPTURE_MODES: { key: AutoCaptureMode; label: string; blurb: string }[] = [
  { key: "manual", label: "Manual only", blurb: "Don't auto-capture; buffer + hotkey still work" },
  { key: "highlights", label: "Highlights", blurb: "Auto-clip the game events below" },
  { key: "full_match", label: "Full match", blurb: "Keep the entire match as one clip" },
  { key: "session", label: "Full session", blurb: "Record the whole time you're in-game" },
];

// Per-game-mode auto-clip toggles. `key` is the live presence queueId (mirrors
// Rust `GameModeToggles`); `art` is the valorant-api gameMode display name used
// to fetch the mode's icon — the four bomb-based queues share "Standard" artwork.
// `other` is the rotating/seasonal/custom catch-all and carries no artwork.
export const GAME_MODE_LABELS: {
  key: keyof GameModeToggles;
  label: string;
  hint: string;
  art?: string;
}[] = [
  { key: "competitive", label: "Competitive", hint: "Ranked", art: "Standard" },
  { key: "unrated", label: "Unrated", hint: "Standard 5v5", art: "Standard" },
  { key: "swiftplay", label: "Swiftplay", hint: "Shortened matches", art: "Swiftplay" },
  { key: "premier", label: "Premier", hint: "Competitive teams", art: "Standard" },
  { key: "spikerush", label: "Spike Rush", hint: "Fast best-of-7", art: "Spike Rush" },
  { key: "deathmatch", label: "Deathmatch", hint: "Free-for-all", art: "Deathmatch" },
  { key: "hurm", label: "Team Deathmatch", hint: "First to 100 kills", art: "Team Deathmatch" },
  { key: "ggteam", label: "Escalation", hint: "Team gun game", art: "Escalation" },
  { key: "onefa", label: "Replication", hint: "One-agent teams", art: "Replication" },
  { key: "snowball", label: "Snowball Fight", hint: "Seasonal", art: "Snowball Fight" },
  { key: "newmap", label: "New Map", hint: "Featured map queue", art: "Standard" },
  { key: "other", label: "Other modes", hint: "Rotating, seasonal & custom games" },
];

// Quality presets (Medal-style). A preset is just a named bundle of the
// concrete knobs; selecting one writes resolution/fps/bitrate, after which those
// fields are the source of truth. Codec/encoder/GPU are independent of the
// preset (separate dropdowns), so they're not touched here. Bitrates mirror
// Medal's ResolutionHandler table. "custom" is implicit (no card entry).
type PresetKey = "low" | "standard" | "high";
export const PRESETS: {
  key: PresetKey;
  label: string;
  blurb: string;
  line: string;
  resolution: string;
  fps: number;
  bitrate: number;
}[] = [
  {
    key: "low",
    label: "Low Quality",
    blurb: "Lower-end PCs & faster uploads",
    line: "360p · 24 FPS",
    resolution: "360p",
    fps: 24,
    bitrate: 3,
  },
  {
    key: "standard",
    label: "Standard",
    blurb: "Performance & fast sharing",
    line: "720p · 60 FPS",
    resolution: "720p",
    fps: 60,
    bitrate: 10,
  },
  {
    key: "high",
    label: "High Quality",
    blurb: "Higher quality & slower uploads",
    line: "1080p · 60 FPS",
    resolution: "1080p",
    fps: 60,
    bitrate: 15,
  },
];

// Resolution targets for the Custom panel. "native" = no scaling (capture at the
// game's own size); the rest match the backend's `resolution_dims()` table.
export const RESOLUTIONS: { value: string; label: string }[] = [
  { value: "native", label: "Native (no scaling)" },
  { value: "360p", label: "360p" },
  { value: "480p", label: "480p" },
  { value: "720p", label: "HD (720p)" },
  { value: "1080p", label: "Full HD (1080p)" },
  { value: "1440p", label: "QHD (1440p)" },
  { value: "2160p", label: "UHD 4K (2160p)" },
];

// Corner placements for the in-game overlay toast stack (mirrors Rust
// `overlay_position`).
export const OVERLAY_POSITIONS: { value: Settings["overlay_position"]; label: string }[] = [
  { value: "top_left", label: "Top left" },
  { value: "top_right", label: "Top right" },
  { value: "bottom_left", label: "Bottom left" },
  { value: "bottom_right", label: "Bottom right" },
];

export const FPS_OPTIONS = [24, 30, 60, 120, 144, 240];
export const BITRATE_OPTIONS = [3, 5, 8, 10, 15, 20, 30, 50];
// Candidate save-clip lengths; the UI filters these to the buffer depth.
export const CLIP_LENGTHS = [10, 15, 30, 60, 90, 120, 180];
