import { useQuery } from "@tanstack/react-query";
import { valorantStatus } from "@/lib/api";

/**
 * Live Valorant status, polled every 2s (matches the presence cadence).
 * Best-effort: `connected:false` / `error` set when Riot isn't running.
 */
export function useValorantStatus() {
  return useQuery({
    queryKey: ["valorant-status"],
    queryFn: valorantStatus,
    retry: false,
    refetchInterval: 2000,
  });
}
