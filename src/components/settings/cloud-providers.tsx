import { useState } from "react";
import { Plus } from "@phosphor-icons/react";

import { Button } from "@/components/ui/button";
import { useCloudProviders, useRemoveProvider } from "@/hooks/use-cloud";
import { ProviderRow } from "./cloud-providers/provider-row";
import { AddProviderForm } from "./cloud-providers/add-provider-form";

/** Provider list + an "add provider" form. Secrets are write-only: they go
 * straight to the OS keyring via `cloudAddProvider` and are never read back. */
export function CloudProviders() {
  const { data: providers } = useCloudProviders();
  const remove = useRemoveProvider();
  const [adding, setAdding] = useState(false);

  return (
    <div className="flex flex-col gap-3">
      {providers && providers.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {providers.map((p) => (
            <ProviderRow key={p.id} provider={p} onRemove={remove.mutate} />
          ))}
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
