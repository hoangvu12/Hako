import { useEffect, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  Events,
  captureStatus,
  listWindows,
  startCapture,
  stopCapture,
  type CaptureStats,
} from "@/lib/api";

/** Capturable windows (refetch on demand via the returned `refetch`). */
export function useWindows() {
  return useQuery({
    queryKey: ["windows"],
    queryFn: listWindows,
    retry: false,
    staleTime: 10_000,
  });
}

/** Subscribe to live `capture-stats` events. Null until capture starts. */
export function useCaptureStats() {
  const [stats, setStats] = useState<CaptureStats | null>(null);

  useEffect(() => {
    const unlisten = listen<CaptureStats>(Events.CaptureStats, (e) => {
      setStats(e.payload);
    });
    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, []);

  return { stats, reset: () => setStats(null) };
}

export function useStartCapture() {
  return useMutation({
    mutationFn: ({
      hwnd,
      fps,
      adapterIndex,
    }: {
      hwnd: number;
      fps: number;
      adapterIndex?: number;
    }) => startCapture(hwnd, fps, adapterIndex),
  });
}

export function useStopCapture() {
  return useMutation({ mutationFn: stopCapture });
}

/**
 * Backend capture-running flag. The recorder runs in the Rust core regardless of
 * which page is mounted, so the UI reads this to stay in sync across navigation
 * (instead of stopping capture when a component unmounts).
 */
export function useCaptureStatus() {
  return useQuery({
    queryKey: ["capture-status"],
    queryFn: captureStatus,
    staleTime: 1_000,
    refetchInterval: 5_000,
  });
}
