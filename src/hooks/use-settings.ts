import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { appHydrated, Events, getSettings, updateSettings, type Settings } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

const SETTINGS_KEY = queryKeys.settings;

export function useSettings() {
  return useQuery({ queryKey: SETTINGS_KEY, queryFn: getSettings, retry: false });
}

/**
 * Refetch settings + clips once the backend hydrates its managed state from disk.
 *
 * At startup both are managed as placeholders (default settings, empty in-memory
 * library) so an IPC call that wins the race against `setup` can't panic on
 * unmanaged state. But a `get_settings`/`clips_list` that reads those placeholders
 * would otherwise stick — `onboarding_completed` reads false (wizard reappears)
 * and the clips list looks empty — because nothing remounts those query observers
 * to trigger a refetch (the settings observer lives in the always-mounted wizard).
 *
 * So when the `state-hydrated` event lands, refresh both queries. We register
 * the listener first and *then* read `appHydrated()`: that ordering closes the
 * race where hydration completed before we subscribed (the event was missed) —
 * the flag is already true, so we refetch anyway. Mount once at the app root.
 *
 * `refresh` *cancels* the queries before invalidating. Without that cancel a read
 * that's still resolving against the startup placeholder wins React Query's
 * request dedup: `invalidateQueries` attaches to the in-flight fetch, that fetch
 * resolves with the stale placeholder, and the invalidated flag clears — so the
 * wizard stays up (onboarding_completed reads false) and the clips list stays
 * empty. Cancelling discards that placeholder result, then invalidate forces a
 * fresh fetch that runs *after* hydration.
 */
export function useStateHydrationBridge() {
  const qc = useQueryClient();
  useEffect(() => {
    let cancelled = false;
    const refresh = async () => {
      await Promise.all([
        qc.cancelQueries({ queryKey: SETTINGS_KEY }),
        qc.cancelQueries({ queryKey: queryKeys.clips }),
      ]);
      if (cancelled) return;
      qc.invalidateQueries({ queryKey: SETTINGS_KEY });
      qc.invalidateQueries({ queryKey: queryKeys.clips });
    };
    const unlisten = listen(Events.StateHydrated, refresh).then((off) => {
      if (cancelled) {
        off();
        return off;
      }
      // Listener is live now — cover the case where hydration already finished
      // before we subscribed (the event fired and was missed).
      appHydrated()
        .then((ready) => {
          if (!cancelled && ready) refresh();
        })
        .catch(() => {});
      return off;
    });
    return () => {
      cancelled = true;
      unlisten.then((off) => off()).catch(() => {});
    };
  }, [qc]);
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
