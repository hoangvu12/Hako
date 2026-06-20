import { useState } from "react";
import {
  CheckCircle,
  Plus,
  Trash,
  XCircle,
  CircleNotch,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  useAddProvider,
  useCloudProviders,
  useConnectOAuth,
  useRemoveProvider,
  useTestProvider,
} from "@/hooks/use-cloud";
import type {
  OAuthProviderKind,
  ProviderConfig,
  ProviderKind,
  ProviderSecrets,
} from "@/lib/api";

type Kind = ProviderKind["kind"];

const KIND_LABELS: { value: Kind; label: string }[] = [
  { value: "r2", label: "Cloudflare R2" },
  { value: "s3", label: "Amazon S3 / compatible" },
  { value: "b2", label: "Backblaze B2" },
  { value: "gcs", label: "Google Cloud Storage" },
  { value: "gdrive", label: "Google Drive" },
  { value: "dropbox", label: "Dropbox" },
  { value: "onedrive", label: "OneDrive" },
];

const kindLabel = (k: Kind) =>
  KIND_LABELS.find((x) => x.value === k)?.label ?? k;

/** The consumer clouds added via OAuth (browser consent) rather than manual keys. */
const OAUTH_KINDS: readonly OAuthProviderKind[] = ["gdrive", "dropbox", "onedrive"];
const isOAuthKind = (k: Kind): k is OAuthProviderKind =>
  (OAUTH_KINDS as readonly string[]).includes(k);

/** Human summary of a provider's target (for the list row). */
function describe(kind: ProviderKind): string {
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

/** Provider list + an "add provider" form. Secrets are write-only: they go
 * straight to the OS keyring via `cloudAddProvider` and are never read back. */
export function CloudProviders() {
  const { data: providers } = useCloudProviders();
  const remove = useRemoveProvider();
  const test = useTestProvider();
  const [adding, setAdding] = useState(false);
  // Per-provider connectivity-test result, keyed by id.
  const [results, setResults] = useState<
    Record<string, { ok: boolean; message: string } | "pending">
  >({});

  const runTest = (id: string) => {
    setResults((r) => ({ ...r, [id]: "pending" }));
    test.mutate(id, {
      onSuccess: () =>
        setResults((r) => ({ ...r, [id]: { ok: true, message: "Connected" } })),
      onError: (e) =>
        setResults((r) => ({
          ...r,
          [id]: { ok: false, message: String(e) },
        })),
    });
  };

  return (
    <div className="flex flex-col gap-3">
      {providers && providers.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {providers.map((p) => {
            const result = results[p.id];
            return (
              <li
                key={p.id}
                className="flex items-center gap-3 rounded-lg border border-border/70 bg-card/40 px-3 py-2.5"
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{p.label}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    {describe(p.kind)}
                  </div>
                </div>
                {result && result !== "pending" ? (
                  <span
                    className={cn(
                      "flex items-center gap-1 text-xs",
                      result.ok ? "text-success" : "text-destructive",
                    )}
                    title={result.message}
                  >
                    {result.ok ? (
                      <CheckCircle weight="fill" className="size-4" />
                    ) : (
                      <XCircle weight="fill" className="size-4" />
                    )}
                  </span>
                ) : null}
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={result === "pending"}
                  onClick={() => runTest(p.id)}
                >
                  {result === "pending" ? (
                    <CircleNotch className="size-3.5 animate-spin" />
                  ) : null}
                  Test
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={`Remove ${p.label}`}
                  onClick={() => remove.mutate(p.id)}
                >
                  <Trash className="size-4" />
                </Button>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-xs text-muted-foreground">
          No providers yet. Add one to enable cloud uploads.
        </p>
      )}

      {adding ? (
        <AddProviderForm onDone={() => setAdding(false)} />
      ) : (
        <Button
          variant="secondary"
          size="sm"
          className="self-start"
          onClick={() => setAdding(true)}
        >
          <Plus className="size-4" />
          Add provider
        </Button>
      )}
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

/** Add-provider form. Renders only the fields the selected kind needs and builds
 * the tagged `ProviderKind` + `ProviderSecrets` on submit. */
function AddProviderForm({ onDone }: { onDone: () => void }) {
  const add = useAddProvider();
  const connect = useConnectOAuth();
  const [kind, setKind] = useState<Kind>("r2");
  const [label, setLabel] = useState("");
  // Superset of all kinds' fields; only the relevant ones are read on submit.
  const [accountId, setAccountId] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [region, setRegion] = useState("");
  const [bucket, setBucket] = useState("");
  const [bucketId, setBucketId] = useState("");
  const [prefix, setPrefix] = useState("");
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secretAccessKey, setSecretAccessKey] = useState("");
  const [gcsCredentialJson, setGcsCredentialJson] = useState("");
  // OAuth (gdrive/dropbox/onedrive): a destination folder, no manual secrets.
  const [folder, setFolder] = useState("/Hako");

  const oauth = isOAuthKind(kind);

  // Launch the browser-consent flow; on success the provider is added + listed.
  const connectOAuth = () => {
    if (!oauth) return;
    connect.mutate(
      { kind, folder: folder.trim() || "/Hako", label: label.trim() || undefined },
      { onSuccess: onDone },
    );
  };

  const buildKind = (): ProviderKind => {
    switch (kind) {
      case "r2":
        return { kind: "r2", account_id: accountId.trim(), bucket: bucket.trim(), prefix: prefix.trim() };
      case "s3":
        return {
          kind: "s3",
          endpoint: endpoint.trim(),
          region: region.trim(),
          bucket: bucket.trim(),
          prefix: prefix.trim(),
        };
      case "b2":
        return {
          kind: "b2",
          bucket: bucket.trim(),
          bucket_id: bucketId.trim(),
          prefix: prefix.trim(),
        };
      case "gcs":
        return { kind: "gcs", bucket: bucket.trim(), prefix: prefix.trim() };
      default:
        // OAuth kinds (gdrive/dropbox/onedrive) are built by the connect flow,
        // not this manual form — they never reach buildKind.
        throw new Error(`not a manual provider kind: ${kind}`);
    }
  };

  // Minimal validity gate: a bucket plus whatever credential the kind requires.
  const valid =
    bucket.trim().length > 0 &&
    (kind === "gcs"
      ? gcsCredentialJson.trim().length > 0
      : accessKeyId.trim().length > 0 && secretAccessKey.trim().length > 0) &&
    (kind !== "r2" || accountId.trim().length > 0) &&
    (kind !== "b2" || bucketId.trim().length > 0);

  const submit = () => {
    const config: ProviderConfig = {
      id: "", // backend assigns a stable id
      label: label.trim() || kindLabel(kind),
      kind: buildKind(),
    };
    const secrets: ProviderSecrets =
      kind === "gcs"
        ? { gcs_credential_json: gcsCredentialJson }
        : { access_key_id: accessKeyId, secret_access_key: secretAccessKey };
    add.mutate({ config, secrets }, { onSuccess: onDone });
  };

  const showKeyPair = kind !== "gcs";

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border/70 bg-card/40 p-3">
      <div className="grid grid-cols-2 gap-3">
        <Field label="Type">
          <Select value={kind} onValueChange={(v) => setKind(v as Kind)}>
            <SelectTrigger size="sm">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KIND_LABELS.map((k) => (
                <SelectItem key={k.value} value={k.value}>
                  {k.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Name">
          <Input
            value={label}
            placeholder={kindLabel(kind)}
            onChange={(e) => setLabel(e.target.value)}
          />
        </Field>
      </div>

      {oauth ? (
        <>
          <Field label="Folder">
            <Input
              value={folder}
              placeholder="/Hako"
              onChange={(e) => setFolder(e.target.value)}
            />
          </Field>
          <p className="text-xs text-muted-foreground">
            Connecting opens your browser to sign in to {kindLabel(kind)}. Hako
            stores only a refresh token in your OS keyring, never your password.
          </p>
          {connect.error ? (
            <p className="text-xs text-destructive">{String(connect.error)}</p>
          ) : null}
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={onDone}
              disabled={connect.isPending}
            >
              Cancel
            </Button>
            <Button size="sm" disabled={connect.isPending} onClick={connectOAuth}>
              {connect.isPending ? (
                <CircleNotch className="size-3.5 animate-spin" />
              ) : null}
              {connect.isPending ? "Waiting for browser…" : `Connect ${kindLabel(kind)}`}
            </Button>
          </div>
        </>
      ) : null}

      {!oauth && kind === "r2" ? (
        <Field label="Account ID">
          <Input
            value={accountId}
            placeholder="Cloudflare account id"
            onChange={(e) => setAccountId(e.target.value)}
          />
        </Field>
      ) : null}

      {!oauth && kind === "s3" ? (
        <div className="grid grid-cols-2 gap-3">
          <Field label="Endpoint">
            <Input
              value={endpoint}
              placeholder="https://s3.amazonaws.com"
              onChange={(e) => setEndpoint(e.target.value)}
            />
          </Field>
          <Field label="Region">
            <Input
              value={region}
              placeholder="us-east-1 (or blank)"
              onChange={(e) => setRegion(e.target.value)}
            />
          </Field>
        </div>
      ) : null}

      {!oauth && kind === "b2" ? (
        <Field label="Bucket ID">
          <Input
            value={bucketId}
            placeholder="B2 bucketId (not the bucket name)"
            onChange={(e) => setBucketId(e.target.value)}
          />
        </Field>
      ) : null}

      {!oauth ? (
        <>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Bucket">
          <Input
            value={bucket}
            placeholder="clips"
            onChange={(e) => setBucket(e.target.value)}
          />
        </Field>
        <Field label="Path prefix (optional)">
          <Input
            value={prefix}
            placeholder="hako"
            onChange={(e) => setPrefix(e.target.value)}
          />
        </Field>
      </div>

      {showKeyPair ? (
        <div className="grid grid-cols-2 gap-3">
          <Field label="Access key ID">
            <Input
              value={accessKeyId}
              autoComplete="off"
              onChange={(e) => setAccessKeyId(e.target.value)}
            />
          </Field>
          <Field label="Secret access key">
            <Input
              type="password"
              value={secretAccessKey}
              autoComplete="off"
              onChange={(e) => setSecretAccessKey(e.target.value)}
            />
          </Field>
        </div>
      ) : (
        <Field label="Service-account JSON">
          <textarea
            value={gcsCredentialJson}
            onChange={(e) => setGcsCredentialJson(e.target.value)}
            rows={4}
            placeholder='{ "type": "service_account", ... }'
            className="scrollbar-thin w-full rounded-md border border-input bg-field px-3 py-2 font-mono text-xs outline-none focus-visible:border-ring"
          />
        </Field>
      )}

      {add.error ? (
        <p className="text-xs text-destructive">{String(add.error)}</p>
      ) : null}

      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onDone}>
          Cancel
        </Button>
        <Button
          size="sm"
          disabled={!valid || add.isPending}
          onClick={submit}
        >
          {add.isPending ? <CircleNotch className="size-3.5 animate-spin" /> : null}
          Add provider
        </Button>
      </div>
        </>
      ) : null}
    </div>
  );
}
