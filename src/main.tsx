import React from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { TooltipProvider } from "@/components/ui/tooltip";
import { router } from "./router";
import "./styles.css";

const queryClient = new QueryClient();

function mount() {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider delayDuration={200}>
          <RouterProvider router={router} />
        </TooltipProvider>
      </QueryClientProvider>
    </React.StrictMode>
  );
}

// Dev-only React render profiling (live overlay). Loaded via a dynamic import
// gated on `import.meta.env.DEV`: Vite folds that to `false` in production, so
// this whole branch — and the `react-scan` import with it — is eliminated and
// never enters the shipped bundle. In dev we load it before mounting so it
// instruments the very first render.
if (import.meta.env.DEV) {
  import("./perf/react-scan").then(({ initReactScan }) => {
    initReactScan();
    mount();
  });
} else {
  mount();
}
