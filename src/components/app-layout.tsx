import { Outlet } from "@tanstack/react-router";

import { AppSidebar } from "@/components/layout/app-sidebar";
import { WindowTitlebar } from "@/components/layout/window-titlebar";
import { UploadToast } from "@/components/clips/upload-toast";
import { useRecorderEventBridge } from "@/hooks/use-recorder";
import { useClipEventBridge, useClips } from "@/hooks/use-library";
import { useCloudEventBridge } from "@/hooks/use-cloud";

export function AppLayout() {
  // Wire Rust -> webview push updates into the query cache once, at the root.
  useRecorderEventBridge();
  useClipEventBridge();
  useCloudEventBridge();

  const { data: clips } = useClips();
  const usedMb = Math.round(
    (clips?.reduce((sum, c) => sum + c.size_bytes, 0) ?? 0) / (1 << 20)
  );

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <AppSidebar usedMb={usedMb} />
      <main className="flex min-w-0 flex-1 flex-col">
        <WindowTitlebar />
        <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
          <Outlet />
        </div>
      </main>
      {/* Background-first upload UX: a corner toast tracking active uploads. */}
      <UploadToast />
    </div>
  );
}
