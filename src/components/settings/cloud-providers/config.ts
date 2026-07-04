import type { OAuthProviderKind, ProviderKind } from "@/lib/api";

export type Kind = ProviderKind["kind"];

export const KIND_LABELS: { value: Kind; label: string }[] = [
  { value: "r2", label: "Cloudflare R2" },
  { value: "s3", label: "Amazon S3 / compatible" },
  { value: "b2", label: "Backblaze B2" },
  { value: "gcs", label: "Google Cloud Storage" },
  { value: "gdrive", label: "Google Drive" },
  { value: "dropbox", label: "Dropbox" },
  { value: "onedrive", label: "OneDrive" },
];

export const kindLabel = (k: Kind) => KIND_LABELS.find((x) => x.value === k)?.label ?? k;

/** The consumer clouds added via OAuth (browser consent) rather than manual keys. */
export const OAUTH_KINDS: readonly OAuthProviderKind[] = ["gdrive", "dropbox", "onedrive"];
export const isOAuthKind = (k: Kind): k is OAuthProviderKind =>
  (OAUTH_KINDS as readonly string[]).includes(k);

/** Human summary of a provider's target (for the list row). */
export function describe(kind: ProviderKind): string {
  switch (kind.kind) {
    case "r2":
      return `R2 · ${kind.bucket}`;
    case "s3":
      return `S3 · ${kind.bucket}${kind.region ? ` (${kind.region})` : ""}`;
    case "b2":
      return `B2 · ${kind.bucket}`;
    case "gcs":
      return `GCS · ${kind.bucket}`;
    case "gdrive":
      return `Google Drive · ${kind.folder}`;
    case "dropbox":
      return `Dropbox · ${kind.folder}`;
    case "onedrive":
      return `OneDrive · ${kind.folder}`;
  }
}
