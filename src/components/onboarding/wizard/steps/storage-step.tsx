import { HardDrives, FolderOpen } from "@phosphor-icons/react";
import { open } from "@tauri-apps/plugin-dialog";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SectionHero, Panel, Row } from "@/components/settings/primitives";
import type { Settings } from "@/lib/api";
import type { WizardSet } from "../config";

export function StorageStep({
  draft,
  set,
  setLocal,
  commit,
}: {
  draft: Settings;
  set: WizardSet;
  setLocal: WizardSet;
  commit: () => void;
}) {
  return (
    <>
      <SectionHero
        icon={HardDrives}
        title="Where to save clips"
        subtitle="Pick a folder on a drive with some free space. Leave it blank to use the default."
      />
      <Panel title="Clip folder">
        <Row label="Folder" hint="Browse to a folder, or paste a path. Default: Videos/Hako.">
          <div className="flex items-center gap-2">
            <Input
              className="w-56"
              value={draft.storage_dir ?? ""}
              placeholder="Videos/Hako"
              onChange={(e) => setLocal("storage_dir", e.target.value || null)}
              onBlur={commit}
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
                  if (typeof picked === "string") set("storage_dir", picked);
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
