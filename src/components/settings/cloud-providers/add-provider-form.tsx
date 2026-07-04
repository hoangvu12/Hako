import { useState } from "react";
import { Spinner } from "@/components/ui/spinner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useAddProvider, useConnectOAuth } from "@/hooks/use-cloud";
import type { ProviderConfig, ProviderKind, ProviderSecrets } from "@/lib/api";
import { Field } from "./field";
import { KIND_LABELS, kindLabel, isOAuthKind, type Kind } from "./config";

/** Add-provider form. Renders only the fields the selected kind needs and builds
 * the tagged `ProviderKind` + `ProviderSecrets` on submit. */
export function AddProviderForm({ onDone }: { onDone: () => void }) {
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
        return {
          kind: "r2",
          account_id: accountId.trim(),
          bucket: bucket.trim(),
          prefix: prefix.trim(),
        };
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
            <Input value={folder} placeholder="/Hako" onChange={(e) => setFolder(e.target.value)} />
          </Field>
          <p className="text-xs text-muted-foreground">
            Connecting opens your browser to sign in to {kindLabel(kind)}. Hako stores only a
            refresh token in your OS keyring, never your password.
          </p>
          {connect.error ? (
            <p className="text-xs text-destructive">{String(connect.error)}</p>
          ) : null}
          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={onDone} disabled={connect.isPending}>
              Cancel
            </Button>
            <Button size="sm" disabled={connect.isPending} onClick={connectOAuth}>
              {connect.isPending ? <Spinner className="size-3.5" /> : null}
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

          {add.error ? <p className="text-xs text-destructive">{String(add.error)}</p> : null}

          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" size="sm" onClick={onDone}>
              Cancel
            </Button>
            <Button size="sm" disabled={!valid || add.isPending} onClick={submit}>
              {add.isPending ? <Spinner className="size-3.5" /> : null}
              Add provider
            </Button>
          </div>
        </>
      ) : null}
    </div>
  );
}
