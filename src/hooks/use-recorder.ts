import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { Events, getRecorderStatus, type RecorderStatus } from "@/lib/api";

const QUERY_KEY = ["recorder-status"] as const;

/** Query the current recorder status (invoke round-trip). */
export function useRecorderStatus() {
  return useQuery({
    queryKey: QUERY_KEY,
    queryFn: getRecorderStatus,
    // The webview can render before Tauri is ready; don't hammer on failure.
    retry: false,
    staleTime: Infinity,
  });
}

/**
 * Bridge Rust -> webview events into TanStack Query cache.
 * Mount once near the app root. Demonstrates the push-update path.
 */
export function useRecorderEventBridge() {
  const qc = useQueryClient();

  useEffect(() => {
    const unlisten = listen<RecorderStatus>(Events.RecorderStatus, (event) => {
      qc.setQueryData(QUERY_KEY, event.payload);
    });

    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, [qc]);
}
