import { useEffect } from "react";
import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  clipAudioTracks,
  clipsList,
  deleteClip,
  Events,
  remuxWithTracks,
  renameClip,
  saveClip,
  trimClip,
  type ClipRecord,
  type TrimMode,
  type TrackVolume,
} from "@/lib/api";

const CLIPS_KEY = ["clips"];

/**
 * Insert a new clip (newest-first) or replace an existing one, keyed by id.
 * This is the single write path for "a clip appeared / changed", shared by the
 * `clip-created` event bridge and every mutation that returns a canonical record
 * — so the event and the mutation result reconcile to the same row instead of
 * racing or duplicating. No-ops until the list query has been seeded (the
 * initial fetch will already include the row).
 */
function upsertClip(qc: QueryClient, clip: ClipRecord) {
  qc.setQueryData<ClipRecord[]>(CLIPS_KEY, (prev) => {
    if (!prev) return prev;
    const idx = prev.findIndex((c) => c.id === clip.id);
    if (idx === -1) return [clip, ...prev]; // brand new → newest-first
    const next = prev.slice();
    next[idx] = clip; // overwrite (e.g. in-place trim/remux) keeps its position
    return next;
  });
}

/**
 * Bridge Rust -> webview `clip-created` events into the query cache. Mount once
 * near the app root (like `useRecorderEventBridge`). Covers every "new clip"
 * path — manual save (incl. the F9 hotkey), Valorant auto-clips, and trim/remux
 * in copy mode — by prepending the event's payload directly. No refetch needed.
 */
export function useClipEventBridge() {
  const qc = useQueryClient();

  useEffect(() => {
    const unlisten = listen<ClipRecord>(Events.ClipCreated, (e) => {
      upsertClip(qc, e.payload);
    });
    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, [qc]);
}

/** The clip library, newest first. Live updates arrive via `useClipEventBridge`. */
export function useClips() {
  return useQuery({ queryKey: CLIPS_KEY, queryFn: clipsList, retry: false });
}

/**
 * Save the last N seconds to a clip. The new row arrives via the `clip-created`
 * bridge; the returned record is also upserted here so the cache is correct even
 * if a caller mounts outside the bridge. Both paths dedupe by id.
 */
export function useSaveClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (seconds?: number) => saveClip(seconds),
    onSuccess: (record) => upsertClip(qc, record),
  });
}

export function useDeleteClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => deleteClip(id),
    // Optimistically drop the card; restore it if the delete fails.
    onMutate: async (id) => {
      await qc.cancelQueries({ queryKey: CLIPS_KEY });
      const prev = qc.getQueryData<ClipRecord[]>(CLIPS_KEY);
      qc.setQueryData<ClipRecord[]>(CLIPS_KEY, (c) =>
        c?.filter((x) => x.id !== id)
      );
      return { prev };
    },
    onError: (_e, _id, ctx) => {
      if (ctx?.prev) qc.setQueryData(CLIPS_KEY, ctx.prev);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
  });
}

export function useRenameClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, title }: { id: number; title: string }) =>
      renameClip(id, title),
    // Optimistically patch the title in place; roll back on failure.
    onMutate: async ({ id, title }) => {
      await qc.cancelQueries({ queryKey: CLIPS_KEY });
      const prev = qc.getQueryData<ClipRecord[]>(CLIPS_KEY);
      qc.setQueryData<ClipRecord[]>(CLIPS_KEY, (c) =>
        c?.map((x) => (x.id === id ? { ...x, title } : x))
      );
      return { prev };
    },
    onError: (_e, _vars, ctx) => {
      if (ctx?.prev) qc.setQueryData(CLIPS_KEY, ctx.prev);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
  });
}

export function useTrimClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      id: number;
      start: number;
      end: number;
      dropAudio: boolean;
      mode: TrimMode;
    }) => trimClip(args),
    // Copy mode also emits `clip-created` (bridge prepends); overwrite mode has
    // no event, so upserting the returned record is what refreshes its row.
    onSuccess: (record) => upsertClip(qc, record),
  });
}

/** A clip's audio tracks (count + names) — drives the editor's per-track UI. */
export function useClipAudioTracks(id: number | undefined) {
  return useQuery({
    queryKey: ["clip-audio-tracks", id],
    queryFn: () => clipAudioTracks(id as number),
    enabled: id != null,
    staleTime: Infinity,
    retry: false,
  });
}

/** Export with a per-track audio mix applied (see `remuxWithTracks`). */
export function useRemuxClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (args: {
      id: number;
      start: number;
      end: number;
      tracks: TrackVolume[];
      mode: TrimMode;
    }) => remuxWithTracks(args),
    onSuccess: (record) => upsertClip(qc, record),
  });
}
