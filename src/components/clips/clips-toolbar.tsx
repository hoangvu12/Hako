import * as React from "react";
import { Scissors, CaretDown, ArrowsDownUp, MagnifyingGlass, X } from "@phosphor-icons/react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useGameAssetsContext } from "@/games/use-game-assets";
import { SORTS, type ClipFilters, type Facets } from "@/components/clips/use-clip-filters";
import { MultiSelectFilter } from "./clips-toolbar/multi-select-filter";
import { SingleSelectFilter } from "./clips-toolbar/single-select-filter";
import { buildFacetOptions } from "./clips-toolbar/options";
import { RESULT_OPTS, SOURCE_OPTS, type MultiKey } from "./clips-toolbar/types";

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
}) {
  const assets = useGameAssetsContext();
  const { gameOptions, agentOptions, mapOptions, modeOptions, eventOptions } = buildFacetOptions(
    facets,
    assets.valorant,
  );

  // The Game facet only earns a chip once the library spans more than one game.
  const showGame = gameOptions.length > 1;
  const hasFacets =
    showGame ||
    agentOptions.length > 0 ||
    mapOptions.length > 0 ||
    modeOptions.length > 0 ||
    eventOptions.length > 0;
  const showResult = facets.hasResult;
  const showSource = facets.hasAuto && facets.hasManual;
  const hasSingle = showResult || showSource;
  const hasFilters = hasFacets || hasSingle;

  const sortLabel =
    SORTS.find((s) => s.key === filters.sort)?.label.replace(" first", "") ?? "Newest";

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

        {hasFilters ? <span className="mx-1.5 h-5 w-px bg-border/60" aria-hidden /> : null}

        {/* Filter group: tight internal spacing marks it as one cluster. */}
        {hasFilters ? (
          <div className="flex flex-wrap items-center gap-1.5">
            {showGame ? (
              <MultiSelectFilter
                label="Game"
                options={gameOptions}
                selected={filters.games}
                onToggle={(v) => toggle("games", v)}
              />
            ) : null}
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
                options={RESULT_OPTS}
                onChange={(v) => update({ result: v })}
              />
            ) : null}
            {showSource ? (
              <SingleSelectFilter
                label="Source"
                value={filters.source}
                defaultValue="any"
                options={SOURCE_OPTS}
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
                  className={filters.sort === s.key ? "text-foreground" : "text-muted-foreground"}
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
