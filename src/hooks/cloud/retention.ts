import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { cloudFreeUpSpace, cloudRetentionStats } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

// --- retention ("free up space") -------------------------------------------

/** Local-usage-vs-budget gauge. Cheap; refetched after a free-up-space pass. */
export function useRetentionStats() {
  return useQuery({
    queryKey: queryKeys.cloudRetention,
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
      qc.invalidateQueries({ queryKey: queryKeys.cloudRetention });
      qc.invalidateQueries({ queryKey: queryKeys.clips });
    },
  });
}
