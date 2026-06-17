import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getSettings, updateSettings, type Settings } from "@/lib/api";

const SETTINGS_KEY = ["settings"];

export function useSettings() {
  return useQuery({ queryKey: SETTINGS_KEY, queryFn: getSettings, retry: false });
}

export function useUpdateSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (next: Settings) => updateSettings(next),
    // Instant-apply: reflect the new settings in the cache immediately, snapshot
    // the previous value, and roll back if the persist fails so the cache never
    // shows an un-saved state. The page reconciles its draft off this on error.
    onMutate: async (next) => {
      await qc.cancelQueries({ queryKey: SETTINGS_KEY });
      const prev = qc.getQueryData<Settings>(SETTINGS_KEY);
      qc.setQueryData<Settings>(SETTINGS_KEY, next);
      return { prev };
    },
    onError: (_e, _next, ctx) => {
      if (ctx?.prev) qc.setQueryData(SETTINGS_KEY, ctx.prev);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: SETTINGS_KEY }),
  });
}
