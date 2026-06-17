import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from "@tanstack/react-router";

import { AppLayout } from "@/components/app-layout";
import ClipsPage from "@/routes/clips";
import ClipDetailPage from "@/routes/clip-detail";
import SettingsPage from "@/routes/settings";
import ValorantPage from "@/routes/valorant";

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
  component: ClipDetailPage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const valorantRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/valorant",
  component: ValorantPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  clipsRoute,
  clipDetailRoute,
  settingsRoute,
  valorantRoute,
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
