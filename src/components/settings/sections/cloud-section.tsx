import { CloudArrowUp } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import { CloudProviders } from "@/components/settings/cloud-providers";
import { fmtBytesCoarse } from "@/components/settings/format";
import type { SettingsSet } from "@/components/settings/config";
import {
  useCloudProviders,
  useFreeUpSpace,
  useRetentionStats,
} from "@/hooks/use-cloud";
import type { Settings } from "@/lib/api";

/** The Cloud Upload settings section: auto-upload toggle + default provider,
 * provider management, and the local-cache retention controls. Split into its
 * own component so it can call `useCloudProviders` without conditionally hooking
 * the page. */
export function CloudSection({
  draft,
  set,
  setLocal,
  commit,
}: {
  draft: Settings;
  set: SettingsSet;
  setLocal: SettingsSet;
  commit: () => void;
}) {
  const { data: providers } = useCloudProviders();
  const { data: stats } = useRetentionStats();
  const freeUp = useFreeUpSpace();
  const hasProviders = !!providers?.length;

  // Local-cache gauge: bytes on disk vs. the configured budget.
  const usedPct =
    stats && stats.budget_bytes > 0
      ? Math.min(100, Math.round((stats.local_bytes / stats.budget_bytes) * 100))
      : 0;

  return (
    <>
      <SectionHero
        icon={CloudArrowUp}
        title="Cloud Upload"
        subtitle="Back up clips to your own object storage (R2, S3, B2, GCS)."
      />

      <Panel title="Auto-upload">
        <Row
          label="Upload clips automatically"
          hint="Send every newly saved clip to your default provider in the background."
        >
          <Switch
            checked={draft.cloud_auto_upload}
            onCheckedChange={(v) => set("cloud_auto_upload", v)}
          />
        </Row>
        <Row
          label="Default provider"
          hint={
            hasProviders
              ? "Where clips upload (auto-upload and the 'Upload to cloud' action)."
              : "Add a provider below first."
          }
        >
          <Select
            value={draft.cloud_default_provider ?? "none"}
            onValueChange={(v) =>
              set("cloud_default_provider", v === "none" ? null : v)
            }
            disabled={!hasProviders}
          >
            <SelectTrigger size="sm" className="w-56">
              <SelectValue placeholder="Select a provider" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">None</SelectItem>
              {providers?.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {p.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
      </Panel>

      <Panel title="Providers">
        <div className="py-1">
          <CloudProviders />
        </div>
      </Panel>

      <Panel title="Local storage">
        <Row
          label="Free up space after upload"
          hint="Once uploaded, delete local copies of the oldest clips when the library grows past the budget below."
        >
          <Switch
            checked={draft.cloud_free_up_space_enabled}
            onCheckedChange={(v) => set("cloud_free_up_space_enabled", v)}
          />
        </Row>
        <Row
          label="Keep up to"
          hint="Local-cache budget in GB before the oldest uploaded clips are evicted."
        >
          <div className="flex items-center gap-2">
            <Input
              className="w-20"
              type="number"
              min={1}
              inputMode="numeric"
              value={String(draft.cloud_retention_gb)}
              disabled={!draft.cloud_free_up_space_enabled}
              onChange={(e) => {
                const n = parseInt(e.target.value, 10);
                setLocal("cloud_retention_gb", Number.isFinite(n) && n > 0 ? n : 1);
              }}
              onBlur={commit}
            />
            <span className="text-xs text-muted-foreground">GB</span>
          </div>
        </Row>
        <Row
          label="Send deleted files to Recycle Bin"
          hint="Evicted clips go to the Recycle Bin instead of being permanently deleted."
        >
          <Switch
            checked={draft.cloud_delete_to_recycle_bin}
            disabled={!draft.cloud_free_up_space_enabled}
            onCheckedChange={(v) => set("cloud_delete_to_recycle_bin", v)}
          />
        </Row>

        {/* Usage gauge + manual trigger. Eviction only ever touches clips that
            are already uploaded, so this is safe to run regardless of the auto
            toggle. */}
        <div className="flex flex-col gap-2 py-4 last:pb-0">
          <div className="flex items-center justify-between text-xs">
            <span className="font-medium">Local clips</span>
            <span className="tabular-nums text-muted-foreground">
              {stats
                ? `${fmtBytesCoarse(stats.local_bytes)} of ${fmtBytesCoarse(stats.budget_bytes)} · ${stats.local_count} clip${stats.local_count === 1 ? "" : "s"}`
                : "—"}
            </span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-secondary">
            <div
              className={cn(
                "h-full rounded-full transition-[width]",
                usedPct >= 100 ? "bg-destructive" : "bg-primary",
              )}
              style={{ width: `${usedPct}%` }}
            />
          </div>
          <div className="flex items-center justify-between">
            <p className="text-xs text-muted-foreground">
              {freeUp.data
                ? freeUp.data.evicted_count > 0
                  ? `Freed ${fmtBytesCoarse(freeUp.data.freed_bytes)} from ${freeUp.data.evicted_count} clip${freeUp.data.evicted_count === 1 ? "" : "s"}.`
                  : "Already under budget, nothing to evict."
                : "Deletes local copies of the oldest uploaded clips."}
            </p>
            <Button
              variant="secondary"
              size="sm"
              disabled={freeUp.isPending}
              onClick={() => freeUp.mutate()}
            >
              Free up space now
            </Button>
          </div>
        </div>
      </Panel>
    </>
  );
}
