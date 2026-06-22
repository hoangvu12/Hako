import { CaretDown } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  Popover,
  PopoverClose,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { OptionRow } from "./option-row";
import { chipActive, chipBase, chipIdle } from "./types";

/**
 * A single-select chip (Result / Source). `defaultValue` is the neutral "no
 * filter" state; while it's chosen the chip stays quiet and shows the bare
 * label. Pick a real value and the chip tints + swaps its label for that value
 * (e.g. "Wins", "Auto"), so it stays compact and matches the multi-select chips.
 * Same idiom as the facet chips, just one choice at a time.
 */
export function SingleSelectFilter<T extends string>({
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
                <OptionRow on={value === o.value} shape="mark" label={o.label} />
              </button>
            </PopoverClose>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
