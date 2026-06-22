import { HardDrives, FolderOpen } from "@phosphor-icons/react";
import { open } from "@tauri-apps/plugin-dialog";

import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import type { SettingsSet } from "@/components/settings/config";
import type { Settings } from "@/lib/api";

export function StorageSection({
  draft,
  setLocal,
  changeStorageDir,
  onCommitStorage,
}: {
  draft: Settings;
  setLocal: SettingsSet;
  changeStorageDir: (next: string | null) => void;
  /** Commit the typed folder on blur (reads the live draft in the parent). */
  onCommitStorage: () => void;
}) {
  return (
    <>
      <SectionHero
        icon={HardDrives}
        title="Storage"
        subtitle="Where clips are written on disk."
      />
      <Panel title="Library">
        <Row
          label="Clip folder"
          hint="Browse to a folder, or paste a path. Leave blank to use the default (Videos/Hako)."
        >
          <div className="flex items-center gap-2">
            <Input
              className="w-64"
              value={draft.storage_dir ?? ""}
              placeholder="Videos/Hako"
              onChange={(e) =>
                setLocal("storage_dir", e.target.value || null)
              }
              onBlur={onCommitStorage}
            />
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                void (async () => {
                  const picked = await open({
                    directory: true,
                    defaultPath: draft.storage_dir ?? undefined,
                  });
                  if (typeof picked === "string") {
                    changeStorageDir(picked);
                  }
                })();
              }}
            >
              <FolderOpen />
              Browse
            </Button>
          </div>
        </Row>
      </Panel>
    </>
  );
}
