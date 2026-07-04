import { useQuery } from "@tanstack/react-query";

import {
  cloudUploadStatus,
  type CloudUpload,
  type CloudUploadState,
} from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";
import { useClipProgress, useProgressMap, type LiveProgress } from "./upload-store";

export const TERMINAL: ReadonlySet<CloudUploadState> = new Set([
  "done",
  "error",
  "canceled",
]);

/** A clip's upload as the UI needs it: the persisted row merged with live bytes
 * and throughput while it's streaming. */
export interface UploadView {
  clipId: number;
  providerId: string;
  status: CloudUploadState;
  error: string | null;
  sent: number;
  total: number;
  bytesPerSec: number;
  remoteUrl: string | null;
  uploadedAt: number | null;
}

function toView(row: CloudUpload, live?: LiveProgress): UploadView {
  return {
    clipId: row.clip_id,
    providerId: row.provider_id,
    status: row.status,
    error: row.error,
    // Prefer live bytes while uploading; fall back to the row's persisted count.
    sent: live?.sent ?? row.bytes_sent,
    total: live?.total ?? row.size_bytes,
    bytesPerSec: live?.bytesPerSec ?? 0,
    remoteUrl: row.remote_url,
    uploadedAt: row.uploaded_at,
  };
}

/** All `cloud_uploads` rows (one per clip+provider), refetched on every status
 * transition by the bridge. */
function useUploadRows() {
  return useQuery({
    queryKey: queryKeys.cloudUploads,
    queryFn: () => cloudUploadStatus(),
    retry: false,
    staleTime: 5_000,
  });
}

/** The merged upload view for a single clip, or `undefined` if it was never
 * uploaded. Live progress refreshes while it streams. */
export function useClipUpload(clipId: number): UploadView | undefined {
  const { data: rows } = useUploadRows();
  const live = useClipProgress(clipId);
  const row = rows?.find((r) => r.clip_id === clipId);
  if (!row) return undefined;
  return toView(row, live);
}

/** A clip's presigned cloud URL, if it has a completed upload. Reads only the
 * persisted rows (not the live progress store), so callers don't re-render on
 * progress ticks — used for cloud-only (evicted) playback. */
export function useClipRemoteUrl(clipId: number): string | null {
  const { data: rows } = useUploadRows();
  return rows?.find((r) => r.clip_id === clipId)?.remote_url ?? null;
}

/** Clips currently queued or uploading (for the corner toast). Sorted with the
 * actively-uploading clip first. */
export function useActiveUploads(): UploadView[] {
  const { data: rows } = useUploadRows();
  const live = useProgressMap();
  if (!rows) return [];
  return rows
    .filter((r) => r.status === "queued" || r.status === "uploading")
    .map((r) => toView(r, live.get(r.clip_id)))
    .sort((a, b) => {
      if (a.status === b.status) return a.clipId - b.clipId;
      return a.status === "uploading" ? -1 : 1;
    });
}
