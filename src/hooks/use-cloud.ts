import { useEffect, useSyncExternalStore } from "react";
import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";

import {
  Events,
  cloudAddProvider,
  cloudCancelUpload,
  cloudConnectOAuth,
  cloudDownloadClip,
  cloudFreeUpSpace,
  cloudListProviders,
  cloudRemoveProvider,
  cloudRetentionStats,
  cloudTestProvider,
  cloudUploadClip,
  cloudUploadStatus,
  type OAuthProviderKind,
  type CloudDownloadProgress,
  type CloudDownloadStatus as CloudDownloadStatusEvent,
  type CloudUpload,
  type CloudUploadProgress,
  type CloudUploadState,
  type CloudUploadStatus as CloudUploadStatusEvent,
  type ProviderConfig,
  type ProviderSecrets,
} from "@/lib/api";

const PROVIDERS_KEY = ["cloud-providers"];
const UPLOADS_KEY = ["cloud-uploads"];
const RETENTION_KEY = ["cloud-retention"];
const CLIPS_KEY = ["clips"];

// --- providers -------------------------------------------------------------

/** Configured cloud providers (no secrets). */
export function useCloudProviders() {
  return useQuery({
    queryKey: PROVIDERS_KEY,
    queryFn: cloudListProviders,
    retry: false,
  });
}

/** Add (or replace, by id) a provider; secrets go to the OS keyring. */
export function useAddProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      config,
      secrets,
    }: {
      config: ProviderConfig;
      secrets: ProviderSecrets;
    }) => cloudAddProvider(config, secrets),
    onSuccess: () => qc.invalidateQueries({ queryKey: PROVIDERS_KEY }),
  });
}

/** Connect a consumer cloud (Google Drive / Dropbox / OneDrive) via OAuth. The
 * browser opens for consent; on success the provider is added (refresh token in
 * the keyring) and the provider list is refreshed. */
export function useConnectOAuth() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      kind,
      folder,
      label,
    }: {
      kind: OAuthProviderKind;
      folder?: string;
      label?: string;
    }) => cloudConnectOAuth(kind, folder, label),
    onSuccess: () => qc.invalidateQueries({ queryKey: PROVIDERS_KEY }),
  });
}

/** Remove a provider (config + keyring secrets). */
export function useRemoveProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => cloudRemoveProvider(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: PROVIDERS_KEY }),
  });
}

/** Test connectivity/credentials (`op.check()`). Resolves on success, throws the
 * friendly error string on failure — the form surfaces it inline. */
export function useTestProvider() {
  return useMutation({ mutationFn: (id: string) => cloudTestProvider(id) });
}

// --- upload actions --------------------------------------------------------

/** Enqueue a clip for upload (defaults to the configured default provider). */
export function useUploadClip() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ clipId, providerId }: { clipId: number; providerId?: string }) =>
      cloudUploadClip(clipId, providerId),
    onSuccess: () => qc.invalidateQueries({ queryKey: UPLOADS_KEY }),
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
      qc.invalidateQueries({ queryKey: CLIPS_KEY });
      qc.invalidateQueries({ queryKey: RETENTION_KEY });
    },
  });
}

// --- retention ("free up space") -------------------------------------------

/** Local-usage-vs-budget gauge. Cheap; refetched after a free-up-space pass. */
export function useRetentionStats() {
  return useQuery({
    queryKey: RETENTION_KEY,
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
      qc.invalidateQueries({ queryKey: RETENTION_KEY });
      qc.invalidateQueries({ queryKey: CLIPS_KEY });
    },
  });
}

// --- live upload state -----------------------------------------------------
//
// Persisted rows come from `cloud_uploads` via React Query; high-frequency byte
// progress (≈4×/s for the one in-flight clip) rides a tiny external store so it
// never churns the query cache. `useCloudEventBridge` (mounted once at the app
// root) feeds both from the two Tauri events. Components read merged per-clip
// views via `useClipUpload` / `useActiveUploads`.

interface LiveProgress {
  sent: number;
  total: number;
  bytesPerSec: number;
}

const progress = new Map<number, LiveProgress>();
const progressListeners = new Set<() => void>();
// Re-created on every mutation so `useSyncExternalStore` sees a new reference.
let progressSnapshot = new Map<number, LiveProgress>();

function emitProgress() {
  progressSnapshot = new Map(progress);
  for (const l of progressListeners) l();
}

function subscribeProgress(cb: () => void) {
  progressListeners.add(cb);
  return () => progressListeners.delete(cb);
}

function useProgressMap() {
  return useSyncExternalStore(
    subscribeProgress,
    () => progressSnapshot,
    () => progressSnapshot,
  );
}

/**
 * One clip's live progress, subscribed individually. Every progress tick (~4×/s)
 * notifies all subscribers, but `getSnapshot` returns this clip's entry — a
 * reference that only changes when *this* clip updates (we replace the object on
 * each `progress.set`). So `useSyncExternalStore` bails out of re-rendering every
 * badge except the one clip actually streaming. Reading the whole map instead
 * (see `useProgressMap`) would re-render all visible badges on every tick.
 */
function useClipProgress(clipId: number): LiveProgress | undefined {
  return useSyncExternalStore(
    subscribeProgress,
    () => progressSnapshot.get(clipId),
    () => progressSnapshot.get(clipId),
  );
}

const TERMINAL: ReadonlySet<CloudUploadState> = new Set([
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
    queryKey: UPLOADS_KEY,
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

// --- live download state ---------------------------------------------------
//
// "Download to edit" re-fetches an evicted clip. Presence in this store == the
// clip is actively downloading; entries appear on the `downloading` status, get
// byte updates from the progress event, and are dropped on done/error. Kept off
// the query cache for the same reason upload progress is (high-frequency ticks).

interface LiveDownload {
  received: number;
  total: number;
  bytesPerSec: number;
}

const downloads = new Map<number, LiveDownload>();
const downloadListeners = new Set<() => void>();
let downloadSnapshot = new Map<number, LiveDownload>();

function emitDownloads() {
  downloadSnapshot = new Map(downloads);
  for (const l of downloadListeners) l();
}

function subscribeDownloads(cb: () => void) {
  downloadListeners.add(cb);
  return () => downloadListeners.delete(cb);
}

function useDownloadMap() {
  return useSyncExternalStore(
    subscribeDownloads,
    () => downloadSnapshot,
    () => downloadSnapshot,
  );
}

/** A clip's live download view for the editor's "download to edit" affordance.
 * `downloading` is true from the start of the fetch until it completes/fails. */
export interface DownloadView {
  downloading: boolean;
  received: number;
  total: number;
  pct: number;
}

export function useClipDownload(clipId: number): DownloadView {
  const map = useDownloadMap();
  const d = map.get(clipId);
  if (!d) return { downloading: false, received: 0, total: 0, pct: 0 };
  const pct = d.total > 0 ? Math.min(100, (d.received / d.total) * 100) : 0;
  return { downloading: true, received: d.received, total: d.total, pct };
}

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
