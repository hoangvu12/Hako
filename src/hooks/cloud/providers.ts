import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  cloudAddProvider,
  cloudConnectOAuth,
  cloudListProviders,
  cloudRemoveProvider,
  cloudTestProvider,
  type OAuthProviderKind,
  type ProviderConfig,
  type ProviderSecrets,
} from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

// --- providers -------------------------------------------------------------

/** Configured cloud providers (no secrets). */
export function useCloudProviders() {
  return useQuery({
    queryKey: queryKeys.cloudProviders,
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
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.cloudProviders }),
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
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.cloudProviders }),
  });
}

/** Remove a provider (config + keyring secrets). */
export function useRemoveProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => cloudRemoveProvider(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.cloudProviders }),
  });
}

/** Test connectivity/credentials (`op.check()`). Resolves on success, throws the
 * friendly error string on failure — the form surfaces it inline. */
export function useTestProvider() {
  return useMutation({ mutationFn: (id: string) => cloudTestProvider(id) });
}
