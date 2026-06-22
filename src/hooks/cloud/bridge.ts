import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";

import {
  Events,
  type CloudDownloadProgress,
  type CloudDownloadStatus as CloudDownloadStatusEvent,
  type CloudUpload,
  type CloudUploadProgress,
  type CloudUploadStatus as CloudUploadStatusEvent,
} from "@/lib/api";
import { CLIPS_KEY, UPLOADS_KEY } from "./keys";
import { emitProgress, progress } from "./upload-store";
import { downloads, emitDownloads } from "./download-store";
import { TERMINAL } from "./uploads";

/**
 * Wire the cloud-upload + cloud-download events into the query cache + the live
 * stores. Mount exactly once, at the app root (alongside the other event bridges).
 */
export function useCloudEventBridge() {
  const qc = useQueryClient();

  useEffect(() => {
    const unlistenProgress = listen<CloudUploadProgress>(
      Events.CloudUploadProgress,
      (e) => {
        const { clip_id, sent, total, bytes_per_sec } = e.payload;
        progress.set(clip_id, { sent, total, bytesPerSec: bytes_per_sec });
        emitProgress();
      },
    );

    const unlistenDownloadProgress = listen<CloudDownloadProgress>(
      Events.CloudDownloadProgress,
      (e) => {
        const { clip_id, received, total, bytes_per_sec } = e.payload;
        downloads.set(clip_id, { received, total, bytesPerSec: bytes_per_sec });
        emitDownloads();
      },
    );

    const unlistenDownloadStatus = listen<CloudDownloadStatusEvent>(
      Events.CloudDownloadStatus,
      (e) => {
        const { clip_id, status } = e.payload;
        if (status === "downloading") {
          if (!downloads.has(clip_id)) {
            downloads.set(clip_id, { received: 0, total: 0, bytesPerSec: 0 });
            emitDownloads();
          }
        } else {
          // done | error → drop the live entry; on done the clip is local again.
          if (downloads.delete(clip_id)) emitDownloads();
          if (status === "done") {
            void qc.invalidateQueries({ queryKey: CLIPS_KEY });
          }
        }
      },
    );

    const unlistenStatus = listen<CloudUploadStatusEvent>(
      Events.CloudUploadStatus,
      (e) => {
        const { clip_id, status } = e.payload;
        // Drop live progress once the upload leaves the in-flight states.
        if (TERMINAL.has(status) && progress.delete(clip_id)) emitProgress();
        // Optimistically reflect the new status, then refetch so terminal rows
        // pull their freshly-written `remote_url` / `uploaded_at` from the DB.
        qc.setQueryData<CloudUpload[]>(UPLOADS_KEY, (prev) =>
          patchStatus(prev, e.payload),
        );
        void qc.invalidateQueries({ queryKey: UPLOADS_KEY });
      },
    );

    return () => {
      unlistenProgress.then((off) => off()).catch(() => {});
      unlistenStatus.then((off) => off()).catch(() => {});
      unlistenDownloadProgress.then((off) => off()).catch(() => {});
      unlistenDownloadStatus.then((off) => off()).catch(() => {});
    };
  }, [qc]);
}

/** Upsert a status event into the cached rows so the UI flips immediately,
 * before the authoritative refetch lands. */
function patchStatus(
  prev: CloudUpload[] | undefined,
  ev: CloudUploadStatusEvent,
): CloudUpload[] {
  const rows = prev ? [...prev] : [];
  const i = rows.findIndex((r) => r.clip_id === ev.clip_id);
  if (i >= 0) {
    rows[i] = { ...rows[i], status: ev.status, error: ev.error ?? null };
  } else {
    rows.push({
      clip_id: ev.clip_id,
      provider_id: ev.provider_id,
      remote_path: null,
      remote_url: null,
      status: ev.status,
      bytes_sent: 0,
      size_bytes: 0,
      uploaded_at: null,
      error: ev.error ?? null,
      updated_at: 0,
    });
  }
  return rows;
}
