import { useState } from "react";
import { SpeakerHigh } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Slider } from "@/components/ui/slider";
import { Checkbox } from "@/components/ui/checkbox";

export function Panel({
  title,
  hint,
  children,
}: {
  title?: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-xl border border-border/70 bg-card/40 p-5">
      {title && <h2 className="text-sm font-semibold text-foreground">{title}</h2>}
      {hint && <p className="mt-0.5 mb-1 text-xs text-muted-foreground">{hint}</p>}
      <div className="mt-2 divide-y divide-border/60">{children}</div>
    </section>
  );
}

/**
 * A 0–100 volume slider with a live readout. Keeps a local value while dragging
 * (smooth), persisting only on release (`onValueCommit`) so we don't write
 * settings to disk on every pixel.
 */
export function VolumeSlider({
  value,
  onCommit,
  disabled,
}: {
  value: number;
  onCommit: (v: number) => void;
  disabled?: boolean;
}) {
  // `local` is a transient drag override (null = not dragging): we read `value`
  // directly during render and only diverge while the user drags, releasing back
  // to the prop on commit. Nothing copies the prop into state and there's no
  // re-sync effect, so the readout never shows a stale value when the parent's
  // `value` changes.
  const [local, setLocal] = useState<number | null>(null);
  const shown = local ?? value;
  return (
    <div className="flex w-44 items-center gap-3">
      <Slider
        value={[shown]}
        min={0}
        max={100}
        step={1}
        disabled={disabled}
        onValueChange={([v]) => setLocal(v)}
        onValueCommit={([v]) => {
          setLocal(null);
          onCommit(v);
        }}
        aria-label="Volume"
      />
      <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
        {shown}%
      </span>
    </div>
  );
}

/** A source row: enable checkbox + icon + label, with a control on the right. */
export function SourceRow({
  icon: Icon,
  iconUrl,
  label,
  hint,
  checked,
  onCheckedChange,
  disabled,
  children,
}: {
  icon: typeof SpeakerHigh;
  /** Real app icon (PNG data URL). When set, shown instead of `icon`. */
  iconUrl?: string | null;
  label: string;
  hint?: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
  disabled?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
      <Checkbox
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onCheckedChange(v === true)}
      />
      {iconUrl ? (
        <img
          src={iconUrl}
          alt=""
          className={cn("size-4 shrink-0 rounded-[3px] object-contain", !checked && "opacity-60")}
        />
      ) : (
        <Icon
          className={cn("size-4 shrink-0", checked ? "text-primary-text" : "text-muted-foreground")}
          weight="fill"
        />
      )}
      <div className="min-w-0 flex-1">
        <div className={cn("truncate text-sm font-medium", !checked && "text-foreground/70")}>
          {label}
        </div>
        {hint && <p className="truncate text-xs text-muted-foreground">{hint}</p>}
      </div>
      <div className={cn("shrink-0", !checked && "pointer-events-none opacity-40")}>{children}</div>
    </div>
  );
}
