import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { cloudFreeUpSpace, cloudRetentionStats } from "@/lib/api";
import { CLIPS_KEY, RETENTION_KEY } from "./keys";

// --- retention ("free up space") -------------------------------------------

/** Local-usage-vs-budget gauge. Cheap; refetched after a free-up-space pass. */
export function useRetentionStats() {
  return useQuery({
    queryKey: RETENTION_KEY,
    queryFn: cloudRetentionStats,
    retry: false,
  });
}

/** Run a retention pass now. Refreshes the gauge and the clip list (evicted
 * clips become cloud-only). */
export function useFreeUpSpace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => cloudFreeUpSpace(),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: RETENTION_KEY });
      qc.invalidateQueries({ queryKey: CLIPS_KEY });
    },
  });
}
