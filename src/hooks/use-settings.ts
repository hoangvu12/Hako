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
    onSuccess: () => qc.invalidateQueries({ queryKey: SETTINGS_KEY }),
  });
}
