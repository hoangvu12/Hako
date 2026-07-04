import { Check } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { toggleClipSelected } from "@/components/clips/use-clip-selection";

/**
 * Top-left selection checkbox. Hidden until the card is hovered, the clip is
 * selected, or the grid is in selection mode (the ancestor scroll container
 * carries `data-selecting`, flipped via CSS only — so entering/leaving selection
 * mode costs zero card re-renders). Sits above the full-card `<Link>` overlay
 * with its own stop-propagation handler, so ticking it never navigates.
 *
 * A plain button (not the Radix `Checkbox`) keeps the per-card scroll-mount cost
 * to a single icon, matching the lazy actions menu's rationale above.
 */
export function SelectCheckbox({ id, selected }: { id: number; selected: boolean }) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={selected}
      aria-label={selected ? "Deselect clip" : "Select clip"}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        toggleClipSelected(id);
      }}
      className={cn(
        "absolute top-2 left-2 z-30 flex size-6 items-center justify-center rounded-full border-2 outline-none transition-[opacity,background-color,border-color] [filter:drop-shadow(0_1px_2px_rgb(0_0_0/0.55))] focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60",
        selected
          ? "border-primary bg-primary text-primary-foreground opacity-100"
          : "border-white bg-transparent text-transparent opacity-0 group-hover:opacity-100 group-data-[selecting]/grid:opacity-100",
      )}
    >
      <Check weight="bold" className="size-4" />
    </button>
  );
}
