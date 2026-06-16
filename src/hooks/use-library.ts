import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  clipsList,
  deleteClip,
  Events,
  renameClip,
  saveClip,
  type ClipRecord,
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
