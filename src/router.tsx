import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from "@tanstack/react-router";

import { AppLayout } from "@/components/app-layout";
// `clips` is the landing route, so it stays eager (in the boot bundle). The
// heavier routes below are lazy — their component code is split into separate
// chunks fetched on navigation, and preloaded on link intent (see router opts).
import ClipsPage from "@/routes/clips";

const rootRoute = createRootRoute({ component: AppLayout });

// Clips is the home of the app — "/" just lands there.
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/clips" });
  },
});

const clipsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/clips",
  component: ClipsPage,
});

const clipDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/clips/$clipId",
}).lazy(() => import("@/routes/clip-detail").then((d) => d.Route));

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  // `?section=` deep-links a settings nav section (e.g. the recorder popover's
  // "Audio" summary jumps straight to Recording Audio). `validateSearch` is a
  // critical (non-lazy) option, so it stays here; only the component is split.
  validateSearch: (search: Record<string, unknown>): { section?: string } =>
    typeof search.section === "string" ? { section: search.section } : {},
}).lazy(() => import("@/routes/settings").then((d) => d.Route));

const routeTree = rootRoute.addChildren([
  indexRoute,
  clipsRoute,
  clipDetailRoute,
  settingsRoute,
]);

export const router = createRouter({
  routeTree,
  defaultPreload: "intent",
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
