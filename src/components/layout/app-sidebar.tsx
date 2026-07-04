import { Link, useRouterState } from "@tanstack/react-router";
import { MonitorPlay, Gear, type Icon } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

type RailEntry = {
  icon: Icon;
  label: string;
  to: string;
  exact?: boolean;
};

const TOP: RailEntry[] = [{ icon: MonitorPlay, label: "Clips", to: "/clips" }];

const BOTTOM: RailEntry[] = [{ icon: Gear, label: "Settings", to: "/settings" }];

function RailItem({ entry }: { entry: RailEntry }) {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const Icon = entry.icon;

  const active =
    entry.to != null && (entry.exact ? pathname === entry.to : pathname.startsWith(entry.to));

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Link
          to={entry.to}
          aria-label={entry.label}
          className={cn(
            "group relative mx-2 flex items-center justify-center rounded-xl py-2.5 transition-colors",
            active
              ? "bg-white/10 text-foreground"
              : "text-sidebar-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
          )}
        >
          <Icon className="size-6 transition-colors" weight={active ? "fill" : "regular"} />
        </Link>
      </TooltipTrigger>
      <TooltipContent side="right">{entry.label}</TooltipContent>
    </Tooltip>
  );
}

export function AppSidebar() {
  return (
    <aside className="relative z-20 flex w-[72px] shrink-0 flex-col justify-between border-r border-sidebar-border bg-sidebar py-4">
      <div className="flex w-full flex-col items-stretch gap-1">
        <Link
          to="/clips"
          aria-label="Hako home"
          className="mx-auto mb-3 flex size-9 items-center justify-center"
        >
          <img src="/logo.png" alt="Hako" draggable={false} className="size-8 rounded-lg" />
        </Link>
        {TOP.map((entry) => (
          <RailItem key={entry.label} entry={entry} />
        ))}
      </div>

      <div className="flex w-full flex-col items-stretch gap-1">
        {BOTTOM.map((entry) => (
          <RailItem key={entry.label} entry={entry} />
        ))}
      </div>
    </aside>
  );
}
