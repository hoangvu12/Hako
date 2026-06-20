import { Outlet } from "@tanstack/react-router";

import { AppSidebar } from "@/components/layout/app-sidebar";
import { WindowTitlebar } from "@/components/layout/window-titlebar";
import { UploadToast } from "@/components/clips/upload-toast";
import { OnboardingWizard } from "@/components/onboarding/onboarding-wizard";
import { useRecorderEventBridge } from "@/hooks/use-recorder";
import { useClipEventBridge } from "@/hooks/use-library";
import { useCloudEventBridge } from "@/hooks/use-cloud";

export function AppLayout() {
  // Wire Rust -> webview push updates into the query cache once, at the root.
  useRecorderEventBridge();
  useClipEventBridge();
  useCloudEventBridge();

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <AppSidebar />
      <main className="flex min-w-0 flex-1 flex-col">
        <WindowTitlebar />
        <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
          <Outlet />
        </div>
      </main>
      {/* Background-first upload UX: a corner toast tracking active uploads. */}
      <UploadToast />
      {/* First-run setup wizard. Self-gates on `settings.onboarding_completed`. */}
      <OnboardingWizard />
    </div>
  );
}
