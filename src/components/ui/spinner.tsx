import { CircleNotch, type IconWeight } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";

/**
 * A loading spinner. `animate-spin` lives on the wrapper <div>, not the <svg>:
 * CSS transforms on SVG elements aren't hardware-accelerated in many engines, so
 * keeping the rotation on the wrapper puts it on the GPU compositor for a smoother
 * spin. Size it via `className` (e.g. `size-4`) exactly like the raw icon.
 */
export function Spinner({
  className,
  weight = "regular",
}: {
  className?: string;
  weight?: IconWeight;
}) {
  return (
    <div className={cn("inline-flex animate-spin", className)}>
      <CircleNotch weight={weight} className="size-full" />
    </div>
  );
}
