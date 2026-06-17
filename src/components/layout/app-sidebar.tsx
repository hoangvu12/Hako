import { Link, useRouterState } from "@tanstack/react-router";
import {
  MonitorPlay,
  Crosshair,
  Gear,
  type Icon,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

type RailEntry = {
  icon: Icon;
  label: string;
  to: string;
  exact?: boolean;
};

const TOP: RailEntry[] = [
  { icon: MonitorPlay, label: "Clips", to: "/clips" },
  { icon: Crosshair, label: "Valorant", to: "/valorant" },
];

const BOTTOM: RailEntry[] = [
  { icon: Gear, label: "Settings", to: "/settings" },
];

function RailItem({ entry }: { entry: RailEntry }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const Icon = entry.icon;

  const active =
    entry.to != null &&
    (entry.exact ? pathname === entry.to : pathname.startsWith(entry.to));

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Link
          to={entry.to}
          aria-label={entry.label}
          className={cn(
            "group relative mx-2 flex items-center justify-center rounded-xl py-2.5 transition-colors",
            active
              ? "bg-primary/10 text-primary"
              : "text-sidebar-foreground hover:bg-sidebar-accent/60 hover:text-foreground"
          )}
        >
          <Icon
            className="size-5 transition-colors"
            weight={active ? "fill" : "regular"}
          />
          {active && (
            <span className="absolute top-1/2 left-0 h-5 w-1 -translate-y-1/2 rounded-r-full bg-primary" />
          )}
        </Link>
      </TooltipTrigger>
      <TooltipContent side="right">{entry.label}</TooltipContent>
    </Tooltip>
  );
}

/** Bottom storage gauge — a small ring driven by current library usage. */
function StorageGauge({ usedMb }: { usedMb: number }) {
  // No hard cap configured; the ring is a gentle "fullness" hint only.
  const pct = Math.min(usedMb / 1024, 1) * 100;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={`${usedMb} MB used`}
          className="mx-auto flex flex-col items-center gap-1 py-1"
        >
          <span className="relative size-5">
            <svg viewBox="0 0 36 36" className="size-full -rotate-90">
              <path
                className="text-secondary"
                stroke="currentColor"
                strokeWidth={3}
                fill="none"
                d="M18 2.0845a15.9155 15.9155 0 0 1 0 31.831a15.9155 15.9155 0 0 1 0 -31.831"
              />
              <path
                className="text-muted-foreground"
                stroke="currentColor"
                strokeWidth={3}
                strokeLinecap="round"
                strokeDasharray={`${pct}, 100`}
                fill="none"
                d="M18 2.0845a15.9155 15.9155 0 0 1 0 31.831a15.9155 15.9155 0 0 1 0 -31.831"
              />
            </svg>
          </span>
        </button>
      </TooltipTrigger>
      <TooltipContent side="right">
        {usedMb} MB · No limit
      </TooltipContent>
    </Tooltip>
  );
}

export function AppSidebar({ usedMb = 0 }: { usedMb?: number }) {
  return (
    <aside className="relative z-20 flex w-[60px] shrink-0 flex-col justify-between border-r border-sidebar-border bg-sidebar py-4">
      <div className="flex w-full flex-col items-stretch gap-1">
        <Link
          to="/clips"
          aria-label="Hako home"
          className="mx-auto mb-3 flex size-8 items-center justify-center"
        >
          <img
            src="/logo.png"
            alt="Hako"
            draggable={false}
            className="size-7 rounded-lg"
          />
        </Link>
        {TOP.map((entry) => (
          <RailItem key={entry.label} entry={entry} />
        ))}
      </div>

      <div className="flex w-full flex-col items-stretch gap-1">
        <StorageGauge usedMb={usedMb} />
        {BOTTOM.map((entry) => (
          <RailItem key={entry.label} entry={entry} />
        ))}
      </div>
    </aside>
  );
}
