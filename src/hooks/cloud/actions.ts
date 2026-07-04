import { useMutation, useQueryClient } from "@tanstack/react-query";

import { cloudCancelUpload, cloudDownloadClip, cloudUploadClip } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

// --- upload actions --------------------------------------------------------

/** Enqueue a clip for upload (defaults to the configured default provider). */
export function useUploadClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ clipId, providerId }: { clipId: number; providerId?: string }) =>
      cloudUploadClip(clipId, providerId),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.cloudUploads }),
  });
}

/** Cancel a clip's queued or in-flight upload. */
export function useCancelUpload() {
  return useMutation({ mutationFn: (clipId: number) => cloudCancelUpload(clipId) });
}

/** Re-download an evicted clip's file so it can be edited locally ("download to
 * edit"). On success the clip is no longer evicted; refetch clips + the gauge.
 * Live byte progress rides the download store (see `useClipDownload`). */
export function useDownloadClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (clipId: number) => cloudDownloadClip(clipId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.clips });
      qc.invalidateQueries({ queryKey: queryKeys.cloudRetention });
    },
  });
}
