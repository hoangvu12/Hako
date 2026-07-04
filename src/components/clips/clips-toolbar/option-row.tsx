import { Check } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";

/**
 * One row inside a filter popover: a select indicator, optional artwork, label.
 *
 * Two indicator idioms, matching the rest of the app:
 *  - `check` (multi-select): a square checkbox that fills when on.
 *  - `mark`  (single-select): a bare checkmark on the chosen row, like the app's
 *    `Select` dropdowns — no round radio, so every filter reads the same way.
 */
export function OptionRow({
  on,
  shape,
  icon,
  label,
}: {
  on: boolean;
  shape: "check" | "mark";
  icon?: string;
  label: string;
}) {
  return (
    <>
      {shape === "check" ? (
        <span
          className={cn(
            "flex size-4 shrink-0 items-center justify-center rounded border",
            // Off state: a clearly-visible outline + faint fill so the empty box
            // reads as a control. A bare 10%-white border (border-border) all but
            // disappears on the dark popover.
            on
              ? "border-primary bg-primary text-primary-foreground"
              : "border-muted-foreground/40 bg-white/[0.03]",
          )}
        >
          {on ? <Check weight="bold" className="size-3" /> : null}
        </span>
      ) : (
        <span className="flex size-4 shrink-0 items-center justify-center text-primary">
          {on ? <Check weight="bold" className="size-3.5" /> : null}
        </span>
      )}
      {icon ? <img src={icon} alt="" className="size-5 shrink-0 rounded object-cover" /> : null}
      <span className="truncate">{label}</span>
    </>
  );
}
