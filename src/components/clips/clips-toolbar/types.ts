import type { ResultFilter, SourceFilter } from "@/components/clips/use-clip-filters";

export type MultiKey = "agents" | "maps" | "modes" | "events";

/** Full-bleed row artwork. `gradient` is a CSS gradient drawn behind `image`;
 * `fit` controls how the image sits (maps splash = cover; agent texture = cover
 * over its signature gradient). */
export interface OptionArt {
  image?: string;
  gradient?: string;
  fit?: "cover";
}

export interface Option {
  value: string;
  label: string;
  /** Small thumbnail shown at the start of the row (agent portrait / map icon). */
  icon?: string;
  /** Optional full-bleed row background. */
  art?: OptionArt;
}

/**
 * Shared chip-trigger look. Every filter — multi or single select — uses the
 * same pill so the bar reads as one coherent group of controls instead of two
 * clashing idioms. Idle chips are quiet (field surface, muted label); an active
 * filter tints toward the brand so "a filter is on" scans at a glance.
 */
export const chipBase =
  "flex h-8 items-center gap-1.5 rounded-full border px-3 text-sm font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50";
export const chipIdle =
  "border-border/70 bg-field text-muted-foreground hover:border-border hover:text-foreground";
export const chipActive = "border-primary/40 bg-primary/10 text-foreground";

export const RESULT_OPTS: { value: ResultFilter; label: string }[] = [
  { value: "any", label: "Any result" },
  { value: "win", label: "Wins" },
  { value: "loss", label: "Losses" },
];
export const SOURCE_OPTS: { value: SourceFilter; label: string }[] = [
  { value: "any", label: "Any source" },
  { value: "auto", label: "Auto" },
  { value: "manual", label: "Manual" },
];
