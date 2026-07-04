import type { Icon } from "@phosphor-icons/react";
import { Check } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";

/** Centered icon + title + subtitle header used at the top of every settings
 * section (and reused, with the same look, atop each onboarding step). */
export function SectionHero({
  icon: Icon,
  title,
  subtitle,
}: {
  icon: Icon;
  title: string;
  subtitle: string;
}) {
  return (
    <div className="flex flex-col items-center text-center">
      <div className="mb-3 flex size-14 items-center justify-center rounded-2xl bg-primary/10 text-primary-text">
        <Icon className="size-7" weight="duotone" />
      </div>
      <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
      <p className="mt-1 max-w-md text-sm text-muted-foreground">{subtitle}</p>
    </div>
  );
}

/** A bordered card grouping related rows, with an optional heading. Children are
 * divided by hairlines (use <Row> for each). */
export function Panel({ title, children }: { title?: string; children: React.ReactNode }) {
  return (
    <section className="rounded-xl border border-border/70 bg-card/40 p-5">
      {title && <h2 className="mb-2 text-sm font-semibold text-foreground">{title}</h2>}
      <div className="divide-y divide-border/60">{children}</div>
    </section>
  );
}

/** A labelled setting row: label (+ optional hint) on the left, the control on
 * the right. */
export function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 py-4 first:pt-0 last:pb-0">
      <div className="min-w-0">
        <div className="text-sm font-medium">{label}</div>
        {hint && <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/** One selectable card in a card-radio group (quality presets, capture modes). */
export function PresetCard({
  title,
  blurb,
  line,
  selected,
  onSelect,
}: {
  title: string;
  blurb: string;
  line?: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "relative flex flex-col rounded-lg border p-3 text-left transition-colors",
        selected
          ? "border-primary bg-primary/10"
          : "border-border/70 bg-card/40 hover:border-border hover:bg-accent/40",
      )}
    >
      {selected && (
        <Check weight="bold" className="absolute top-2.5 right-2.5 size-4 text-primary-text" />
      )}
      <span className="text-sm font-semibold">{title}</span>
      <span className="mt-1 text-xs text-muted-foreground">{blurb}</span>
      {line && <span className="mt-2 text-xs font-medium">{line}</span>}
    </button>
  );
}
