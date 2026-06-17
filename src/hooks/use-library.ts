import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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

/** The clip library, newest first. Invalidates live on `clip-created`. */
export function useClips() {
  const qc = useQueryClient();

  useEffect(() => {
    const unlisten = listen<ClipRecord>(Events.ClipCreated, () => {
      qc.invalidateQueries({ queryKey: CLIPS_KEY });
    });
    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, [qc]);

  return useQuery({ queryKey: CLIPS_KEY, queryFn: clipsList, retry: false });
}

export function useSaveClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (seconds?: number) => saveClip(seconds),
    onSuccess: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
  });
}

export function useDeleteClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: number) => deleteClip(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
  });
}

export function useRenameClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, title }: { id: number; title: string }) => renameClip(id, title),
    onSuccess: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
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
    onSuccess: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
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
    onSuccess: () => qc.invalidateQueries({ queryKey: CLIPS_KEY }),
  });
}
