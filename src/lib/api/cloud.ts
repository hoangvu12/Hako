import { invoke } from "@tauri-apps/api/core";

import type { ClipRecord } from "./clips";

// ---------------------------------------------------------------------------
// Cloud upload (src-tauri/src/cloud)
// ---------------------------------------------------------------------------

/**
 * Non-secret provider config (mirrors Rust `ProviderKind`, serde-tagged on
 * `kind` with snake_case variants). Secrets (keys / service-account JSON) are
 * passed separately to `cloudAddProvider` and stored in the OS keyring — never
 * here, never in `cloud_providers.json`.
 */
export type ProviderKind =
  | { kind: "s3"; endpoint: string; region: string; bucket: string; prefix: string }
  | { kind: "r2"; account_id: string; bucket: string; prefix: string }
  | { kind: "b2"; bucket: string; bucket_id: string; prefix: string }
  | { kind: "gcs"; bucket: string; prefix: string }
  // Phase 2/3 consumer OAuth clouds. Folder-rooted (the upload engine writes
  // under `folder`); auth is an OAuth refresh token in the keyring, obtained via
  // the `cloudConnect*` flow — never entered by hand.
  | { kind: "gdrive"; folder: string }
  | { kind: "dropbox"; folder: string }
  | { kind: "onedrive"; folder: string };

/** A configured cloud target (mirrors Rust `ProviderConfig`). */
export interface ProviderConfig {
  /** Stable id; used as `provider_id` in `cloud_uploads` + the keyring key. */
  id: string;
  /** User-facing name, e.g. "My R2 clips". */
  label: string;
  kind: ProviderKind;
}

/** Secrets for a provider (mirrors Rust `Secrets`). Only the fields a given
 * `kind` needs are read: S3/R2/B2 use the key pair; GCS uses the JSON. */
export interface ProviderSecrets {
  access_key_id?: string;
  secret_access_key?: string;
  /** GCS service-account JSON (the whole file contents). */
  gcs_credential_json?: string;
  // Consumer OAuth clouds (gdrive/dropbox/onedrive). Written by the
  // `cloudConnect*` flow, not the add-provider form; OpenDAL refreshes the
  // access token from these. Listed for type-parity with the Rust `Secrets`.
  refresh_token?: string;
  client_id?: string;
  client_secret?: string;
}

/** Cloud-upload status values (mirrors Rust `cloud_status`). */
export type CloudUploadState =
  | "queued"
  | "uploading"
  | "done"
  | "error"
  | "canceled";

/** One `cloud_uploads` row (mirrors Rust `CloudUpload`). */
export interface CloudUpload {
  clip_id: number;
  provider_id: string;
  remote_path: string | null;
  remote_url: string | null;
  status: CloudUploadState;
  bytes_sent: number;
  size_bytes: number;
  /** Unix ms; set ONLY on success — the local-eviction gate. */
  uploaded_at: number | null;
  error: string | null;
  updated_at: number;
}

/** Payload of the `cloud-upload-progress` event. */
export interface CloudUploadProgress {
  clip_id: number;
  provider_id: string;
  /** Bytes streamed so far. */
  sent: number;
  /** Total file size in bytes. */
  total: number;
  /** Throughput estimate for the toast (e.g. "12.4 MB/s"). */
  bytes_per_sec: number;
}

/** Payload of the `cloud-upload-status` event. */
export interface CloudUploadStatus {
  clip_id: number;
  provider_id: string;
  status: CloudUploadState;
  error?: string | null;
}

/** Cloud-download lifecycle for an evicted clip being re-fetched to edit. */
export type CloudDownloadState = "downloading" | "done" | "error";

/** Payload of the `cloud-download-progress` event. */
export interface CloudDownloadProgress {
  clip_id: number;
  /** Bytes received so far. */
  received: number;
  /** Total object size in bytes. */
  total: number;
  /** Throughput estimate (bytes/sec). */
  bytes_per_sec: number;
}

/** Payload of the `cloud-download-status` event. */
export interface CloudDownloadStatus {
  clip_id: number;
  status: CloudDownloadState;
  error?: string | null;
}

/** Configured cloud providers (no secrets). */
export async function cloudListProviders(): Promise<ProviderConfig[]> {
  return invoke<ProviderConfig[]>("cloud_list_providers");
}

/** Add a provider: persists the config to `cloud_providers.json` and its
 * secrets to the OS keyring. Returns the stored config (with its assigned id). */
export async function cloudAddProvider(
  config: ProviderConfig,
  secrets: ProviderSecrets,
): Promise<ProviderConfig> {
  return invoke<ProviderConfig>("cloud_add_provider", { config, secrets });
}

/** Remove a provider (config + keyring secrets). */
export async function cloudRemoveProvider(id: string): Promise<void> {
  await invoke("cloud_remove_provider", { id });
}

/** Test connectivity/credentials for a configured provider (`op.check()`). */
export async function cloudTestProvider(id: string): Promise<void> {
  await invoke("cloud_test_provider", { id });
}

/** Which consumer cloud to connect via OAuth (mirrors the Rust connect commands). */
export type OAuthProviderKind = "gdrive" | "dropbox" | "onedrive";

/**
 * Connect a consumer cloud (Google Drive / Dropbox / OneDrive) via OAuth. Opens
 * the system browser for consent; on success the refresh token is stored in the
 * OS keyring and the new provider config is returned (it then behaves like any
 * other provider). `folder` defaults to `/Hako`. The promise resolves only after
 * the user finishes the browser consent (or rejects on cancel/timeout/error).
 */
export async function cloudConnectOAuth(
  kind: OAuthProviderKind,
  folder?: string,
  label?: string,
): Promise<ProviderConfig> {
  const command =
    kind === "gdrive"
      ? "cloud_connect_gdrive"
      : kind === "dropbox"
        ? "cloud_connect_dropbox"
        : "cloud_connect_onedrive";
  return invoke<ProviderConfig>(command, {
    folder: folder ?? null,
    label: label ?? null,
  });
}

/** Enqueue a clip for upload. `providerId` defaults to `cloud_default_provider`. */
export async function cloudUploadClip(
  clipId: number,
  providerId?: string,
): Promise<void> {
  await invoke("cloud_upload_clip", { clipId, providerId: providerId ?? null });
}

/** Cancel an in-flight or queued upload for a clip. */
export async function cloudCancelUpload(clipId: number): Promise<void> {
  await invoke("cloud_cancel_upload", { clipId });
}

/** Cloud-upload rows for one clip, or all rows when `clipId` is omitted. */
export async function cloudUploadStatus(clipId?: number): Promise<CloudUpload[]> {
  return invoke<CloudUpload[]>("cloud_upload_status", { clipId: clipId ?? null });
}

/** Re-download an evicted clip's file from the cloud so it can be edited locally.
 * Resolves with the refreshed record (`evicted = false`); progress streams over
 * the `cloud-download-progress` event. No-op for a clip that's already local. */
export async function cloudDownloadClip(clipId: number): Promise<ClipRecord> {
  return invoke<ClipRecord>("cloud_download_clip", { clipId });
}

/** Retention gauge / outcome (mirrors Rust `EvictStats`). */
export interface EvictStats {
  /** Local bytes still on disk (non-evicted clips). */
  local_bytes: number;
  /** Clips with local files still on disk. */
  local_count: number;
  /** Configured budget in bytes (`cloud_retention_gb` × 1 GiB). */
  budget_bytes: number;
  /** Bytes reclaimed by a pass (0 for a stats-only probe). */
  freed_bytes: number;
  /** Clips evicted by a pass (0 for a stats-only probe). */
  evicted_count: number;
}

/** Current retention gauge (local usage vs. budget); changes nothing. */
export async function cloudRetentionStats(): Promise<EvictStats> {
  return invoke<EvictStats>("cloud_retention_stats");
}

/** Run a retention pass now ("Free up space"): evict oldest uploaded clips until
 * under budget. Returns what was reclaimed. */
export async function cloudFreeUpSpace(): Promise<EvictStats> {
  return invoke<EvictStats>("cloud_free_up_space");
}
