import * as React from "react";
import {
  Scissors,
  CaretDown,
  ArrowsDownUp,
  MagnifyingGlass,
  Check,
  X,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverClose,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { ValorantAssets } from "@/hooks/use-valorant-assets";
import {
  SORTS,
  type ResultFilter,
  type SourceFilter,
  type ClipFilters,
  type Facets,
} from "@/components/clips/use-clip-filters";

type MultiKey = "agents" | "maps" | "modes" | "events";

/** Full-bleed row artwork. `gradient` is a CSS gradient drawn behind `image`;
 * `fit` controls how the image sits (maps splash = cover; agent texture = cover
 * over its signature gradient). */
interface OptionArt {
  image?: string;
  gradient?: string;
  fit?: "cover";
}

interface Option {
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
const chipBase =
  "flex h-8 items-center gap-1.5 rounded-full border px-3 text-sm font-medium outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50";
const chipIdle =
  "border-border/70 bg-field text-muted-foreground hover:border-border hover:text-foreground";
const chipActive = "border-primary/40 bg-primary/10 text-foreground";

/** One row inside a filter popover: a select indicator, optional artwork, label. */
function OptionRow({
  on,
  shape,
  icon,
  label,
}: {
  on: boolean;
  shape: "check" | "radio";
  icon?: string;
  label: string;
}) {
  return (
    <>
      <span
        className={cn(
          "flex size-4 shrink-0 items-center justify-center border",
          shape === "check" ? "rounded" : "rounded-full",
          on ? "border-primary bg-primary text-primary-foreground" : "border-border"
        )}
      >
        {on ? (
          shape === "check" ? (
            <Check weight="bold" className="size-3" />
          ) : (
            <span className="size-1.5 rounded-full bg-primary-foreground" />
          )
        ) : null}
      </span>
      {icon ? (
        <img
          src={icon}
          alt=""
          className="size-5 shrink-0 rounded object-cover"
        />
      ) : null}
      <span className="truncate">{label}</span>
    </>
  );
}

/** A multi-select facet chip: shows the active count, opens a checklist. */
function MultiSelectFilter({
  label,
  options,
  selected,
  onToggle,
}: {
  label: string;
  options: Option[];
  selected: string[];
  onToggle: (value: string) => void;
}) {
  if (options.length === 0) return null;
  const count = selected.length;
  const hasArt = options.some((o) => o.art);
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button type="button" className={cn(chipBase, count > 0 ? chipActive : chipIdle)}>
          {label}
          {count > 0 ? (
            <span className="flex size-4 items-center justify-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground">
              {count}
            </span>
          ) : null}
          <CaretDown className="size-3 opacity-60" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className={cn("p-1.5", hasArt ? "w-64" : "w-56")}
      >
        <ScrollArea className="max-h-80">
          <div className="flex flex-col gap-0.5">
            {options.map((o) => {
              const on = selected.includes(o.value);
              // Art rows (maps, agents): the artwork fills the row behind a
              // left-anchored scrim (solid popover → transparent) so the
              // checkbox + name stay legible while the art reads at a glance.
              // Maps use their splash; agents use their signature gradient with
              // the faint agent-select texture over it + a round portrait.
              if (o.art) {
                const { image, gradient, fit } = o.art;
                return (
                  <button
                    key={o.value}
                    type="button"
                    onClick={() => onToggle(o.value)}
                    className="group/opt relative flex h-12 items-center gap-2.5 overflow-hidden rounded-lg px-2.5 text-left"
                  >
                    {gradient ? (
                      <span
                        aria-hidden
                        className="absolute inset-0"
                        style={{ backgroundImage: gradient }}
                      />
                    ) : null}
                    {image ? (
                      <img
                        src={image}
                        alt=""
                        className={cn(
                          "absolute inset-0 size-full object-cover transition-transform duration-500 ease-out group-hover/opt:scale-105",
                          fit,
                          // Over a gradient the image is the agent-select
                          // texture: keep it faint so the colors carry.
                          gradient && "opacity-40 mix-blend-overlay"
                        )}
                      />
                    ) : null}
                    <span className="absolute inset-0 bg-gradient-to-r from-popover via-popover/60 to-popover/5" />
                    {on ? (
                      <span className="pointer-events-none absolute inset-0 rounded-lg ring-2 ring-inset ring-primary" />
                    ) : null}
                    <span
                      className={cn(
                        "relative flex size-4 shrink-0 items-center justify-center rounded border",
                        on
                          ? "border-primary bg-primary text-primary-foreground"
                          : "border-white/60 bg-black/30"
                      )}
                    >
                      {on ? <Check weight="bold" className="size-3" /> : null}
                    </span>
                    {gradient && o.icon ? (
                      <img
                        src={o.icon}
                        alt=""
                        className="relative size-7 shrink-0 rounded-full object-cover ring-1 ring-white/25"
                      />
                    ) : null}
                    <span className="relative truncate text-sm font-semibold text-white [text-shadow:0_1px_3px_rgb(0_0_0/0.85)]">
                      {o.label}
                    </span>
                  </button>
                );
              }
              return (
                <button
                  key={o.value}
                  type="button"
                  onClick={() => onToggle(o.value)}
                  className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent"
                >
                  <OptionRow on={on} shape="check" icon={o.icon} label={o.label} />
                </button>
              );
            })}
          </div>
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}

/**
 * A single-select chip (Result / Source). `defaultValue` is the neutral "no
 * filter" state; while it's chosen the chip stays quiet and shows the bare
 * label. Pick a real value and the chip tints + swaps its label for that value
 * (e.g. "Wins", "Auto"), so it stays compact and matches the multi-select chips.
 * Same idiom as the facet chips, just one choice at a time.
 */
function SingleSelectFilter<T extends string>({
  label,
  value,
  defaultValue,
  options,
  onChange,
}: {
  label: string;
  value: T;
  defaultValue: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  const active = value !== defaultValue;
  const current = options.find((o) => o.value === value);
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button type="button" className={cn(chipBase, active ? chipActive : chipIdle)}>
          {active ? current?.label ?? label : label}
          <CaretDown className="size-3 opacity-60" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-44 p-1.5">
        <div className="flex flex-col">
          {options.map((o) => (
            <PopoverClose key={o.value} asChild>
              <button
                type="button"
                onClick={() => onChange(o.value)}
                className="flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent"
              >
                <OptionRow on={value === o.value} shape="radio" label={o.label} />
              </button>
            </PopoverClose>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

// Memoized: the clips grid re-renders `ClipsPage` on every scroll tick (the
// virtualizer's own state). Without this boundary, each of those re-rendered the
// whole toolbar and every filter Popover/Popper subtree — scroll state the
// toolbar doesn't care about. All props from `useClipFilters` are already stable
// (useMemo/useCallback), so this holds as long as the parent passes a stable
// `onSave` too.
export const ClipsToolbar = React.memo(function ClipsToolbar({
  clipSeconds,
  onSave,
  saving,
  total,
  filters,
  facets,
  activeCount,
  update,
  toggle,
  reset,
  assets,
}: {
  clipSeconds: number;
  onSave: () => void;
  saving: boolean;
  total: number;
  filters: ClipFilters;
  facets: Facets;
  activeCount: number;
  update: (patch: Partial<ClipFilters>) => void;
  toggle: (key: MultiKey, value: string) => void;
  reset: () => void;
  assets: ValorantAssets;
}) {
  const agentOptions: Option[] = facets.agents.map((name) => {
    const a = assets.agentByName(name);
    return {
      value: name,
      label: name,
      icon: a?.icon,
      // Signature gradient base + faint agent-select texture over it.
      art: a?.gradient
        ? { gradient: a.gradient, image: a.background || undefined, fit: "cover" }
        : undefined,
    };
  });
  const mapOptions: Option[] = facets.maps.map((m) => {
    const meta = assets.mapFor(m.path);
    return {
      value: m.path,
      label: meta?.name || m.name,
      icon: meta?.listIcon,
      // Full-bleed map splash (unchanged from before).
      art: meta?.splash ? { image: meta.splash, fit: "cover" } : undefined,
    };
  });
  const modeOptions: Option[] = facets.modes.map((m) => ({ value: m, label: m }));
  const eventOptions: Option[] = facets.events.map((e) => ({ value: e, label: e }));

  const resultOpts: { value: ResultFilter; label: string }[] = [
    { value: "any", label: "Any result" },
    { value: "win", label: "Wins" },
    { value: "loss", label: "Losses" },
  ];
  const sourceOpts: { value: SourceFilter; label: string }[] = [
    { value: "any", label: "Any source" },
    { value: "auto", label: "Auto" },
    { value: "manual", label: "Manual" },
  ];

  const hasFacets =
    agentOptions.length > 0 ||
    mapOptions.length > 0 ||
    modeOptions.length > 0 ||
    eventOptions.length > 0;
  const showResult = facets.hasResult;
  const showSource = facets.hasAuto && facets.hasManual;
  const hasSingle = showResult || showSource;
  const hasFilters = hasFacets || hasSingle;

  const sortLabel =
    SORTS.find((s) => s.key === filters.sort)?.label.replace(" first", "") ??
    "Newest";

  return (
    <div className="shrink-0 border-b border-panel-border bg-panel">
      {/* One row, three zones with breathing room between them: the Save action,
          the unified filter chips, and the view controls (count · sort · search)
          pushed right. Wraps gracefully when the panel is narrow. */}
      <div className="flex flex-wrap items-center gap-x-2 gap-y-2.5 px-6 py-2.5">
        <Button size="sm" onClick={onSave} disabled={saving}>
          <Scissors weight="bold" />
          {saving ? "Saving…" : `Save last ${clipSeconds}s`}
        </Button>

        {hasFilters ? (
          <span className="mx-1.5 h-5 w-px bg-border/60" aria-hidden />
        ) : null}

        {/* Filter group: tight internal spacing marks it as one cluster. */}
        {hasFilters ? (
          <div className="flex flex-wrap items-center gap-1.5">
            <MultiSelectFilter
              label="Agent"
              options={agentOptions}
              selected={filters.agents}
              onToggle={(v) => toggle("agents", v)}
            />
            <MultiSelectFilter
              label="Map"
              options={mapOptions}
              selected={filters.maps}
              onToggle={(v) => toggle("maps", v)}
            />
            <MultiSelectFilter
              label="Mode"
              options={modeOptions}
              selected={filters.modes}
              onToggle={(v) => toggle("modes", v)}
            />
            <MultiSelectFilter
              label="Event"
              options={eventOptions}
              selected={filters.events}
              onToggle={(v) => toggle("events", v)}
            />

            {/* A subtle beat separates the multi-facets from the single toggles. */}
            {hasFacets && hasSingle ? (
              <span className="mx-0.5 h-4 w-px bg-border/50" aria-hidden />
            ) : null}

            {showResult ? (
              <SingleSelectFilter
                label="Result"
                value={filters.result}
                defaultValue="any"
                options={resultOpts}
                onChange={(v) => update({ result: v })}
              />
            ) : null}
            {showSource ? (
              <SingleSelectFilter
                label="Source"
                value={filters.source}
                defaultValue="any"
                options={sourceOpts}
                onChange={(v) => update({ source: v })}
              />
            ) : null}
          </div>
        ) : null}

        {activeCount > 0 ? (
          <button
            type="button"
            onClick={reset}
            className="flex h-8 items-center gap-1 rounded-full px-2.5 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
          >
            <X className="size-3.5" />
            Clear {activeCount}
          </button>
        ) : null}

        {/* View zone: count · sort · search, pushed to the right edge. */}
        <div className="ml-auto flex items-center gap-3">
          <span className="whitespace-nowrap text-sm font-medium text-muted-foreground">
            {total} {total === 1 ? "clip" : "clips"}
          </span>

          <DropdownMenu>
            <DropdownMenuTrigger className="flex h-8 items-center gap-1.5 rounded-full border border-border/70 bg-field px-3 text-sm font-medium text-muted-foreground outline-none transition-colors hover:border-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50">
              <ArrowsDownUp className="size-4" />
              {sortLabel}
              <CaretDown className="size-3 opacity-60" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {SORTS.map((s) => (
                <DropdownMenuItem
                  key={s.key}
                  onSelect={() => update({ sort: s.key })}
                  className={
                    filters.sort === s.key
                      ? "text-foreground"
                      : "text-muted-foreground"
                  }
                >
                  {s.label}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>

          <div className="relative">
            <MagnifyingGlass className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={filters.search}
              onChange={(e) => update({ search: e.target.value })}
              placeholder="Search"
              className="h-8 w-44 rounded-full border-border/70 bg-field pl-9 placeholder:text-muted-foreground/70"
            />
          </div>
        </div>
      </div>
    </div>
  );
});
