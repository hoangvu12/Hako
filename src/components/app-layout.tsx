import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import {
  LayoutDashboard,
  Film,
  Crosshair,
  Settings as SettingsIcon,
  Circle,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { useRecorderEventBridge, useRecorderStatus } from "@/hooks/use-recorder";

const NAV = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard },
  { to: "/clips", label: "Clips", icon: Film },
  { to: "/valorant", label: "Valorant", icon: Crosshair },
  { to: "/settings", label: "Settings", icon: SettingsIcon },
] as const;

function StatusDot() {
  const { data } = useRecorderStatus();
  const capturing = data?.capturing ?? false;
  return (
    <span className="flex items-center gap-2 text-xs text-muted-foreground">
      <Circle
        className={cn(
          "size-2.5 fill-current",
          capturing ? "text-primary" : "text-muted-foreground/50"
        )}
      />
      {capturing ? "Recording" : "Idle"}
    </span>
  );
}

export function AppLayout() {
  // Wire Rust -> webview push updates into the query cache once, at the root.
  useRecorderEventBridge();

  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <aside className="flex w-56 shrink-0 flex-col border-r bg-card/40">
        <div className="flex h-14 items-center gap-2 px-4">
          <div className="flex size-7 items-center justify-center rounded-md bg-primary text-primary-foreground font-bold">
            H
          </div>
          <span className="font-semibold tracking-tight">Hako</span>
        </div>

        <nav className="flex flex-1 flex-col gap-1 px-2">
          {NAV.map(({ to, label, icon: Icon }) => {
            const active =
              to === "/" ? pathname === "/" : pathname.startsWith(to);
            return (
              <Link
                key={to}
                to={to}
                className={cn(
                  "flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors",
                  active
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                )}
              >
                <Icon className="size-4" />
                {label}
              </Link>
            );
          })}
        </nav>

        <div className="border-t px-4 py-3">
          <StatusDot />
        </div>
      </aside>

      <main className="flex-1 overflow-y-auto">
        <Outlet />
      </main>
    </div>
  );
}
