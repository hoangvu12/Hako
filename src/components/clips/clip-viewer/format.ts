import {
  Skull,
  Knife,
  Bomb,
  Trophy,
  Fire,
  Handshake,
  ShieldCheck,
  type Icon as PhosphorIcon,
} from "@phosphor-icons/react";

import { formatTime } from "@/lib/format";

/** A playhead timestamp with tenths, e.g. "1:07.4" — for the trim selection. */
export function fmtClock(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const whole = Math.floor(secs);
  const tenth = Math.floor((secs - whole) * 10);
  return `${formatTime(whole)}.${tenth}`;
}

export function fmtDate(unixMs: number): string {
  return new Date(unixMs).toLocaleDateString(undefined, {
    year: "numeric",
    month: "long",
    day: "numeric",
  });
}

/** Icon + tint for a seek-bar event marker, keyed off the EventKind label. */
export function eventIconFor(label: string): { Icon: PhosphorIcon; tint: string } {
  const l = label.toLowerCase();
  if (l.includes("victory")) return { Icon: Trophy, tint: "text-warning" };
  if (l.includes("clutch")) return { Icon: Fire, tint: "text-warning" };
  if (l.includes("knife")) return { Icon: Knife, tint: "text-white" };
  if (l.includes("defus")) return { Icon: ShieldCheck, tint: "text-info" };
  if (l.includes("spike") || l.includes("detonat"))
    return { Icon: Bomb, tint: "text-destructive" };
  if (l.includes("death")) return { Icon: Skull, tint: "text-destructive" };
  if (l.includes("assist")) return { Icon: Handshake, tint: "text-info" };
  // Kills (single + multi-kill + ace) and anything unrecognised.
  return { Icon: Skull, tint: "text-white" };
}

/** Pick a "nice" ruler step (≈8 ticks) for a given duration. */
export function rulerStep(duration: number): number {
  const target = duration / 8;
  const steps = [1, 2, 5, 8, 10, 15, 20, 30, 60, 120, 300];
  return steps.find((s) => s >= target) ?? Math.ceil(target);
}
