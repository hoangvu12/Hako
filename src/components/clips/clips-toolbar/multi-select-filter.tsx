import { CaretDown, Check } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { OptionRow } from "./option-row";
import { chipActive, chipBase, chipIdle, type Option } from "./types";

/** A multi-select facet chip: shows the active count, opens a checklist. */
export function MultiSelectFilter({
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
      <PopoverContent align="start" className={cn("p-1.5", hasArt ? "w-64" : "w-56")}>
        <ScrollArea viewportClassName="max-h-80">
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
                          "absolute inset-0 size-full object-cover",
                          fit,
                          // Over a gradient the image is the agent-select
                          // texture: keep it faint so the colors carry.
                          gradient && "opacity-40 mix-blend-overlay",
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
                          : "border-white/60 bg-black/30",
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
