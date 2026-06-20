import { useNavigate } from "@tanstack/react-router";
import { ArrowRight, Info } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  Popover,
  PopoverClose,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { HotkeyRecorder, KeyCombo } from "@/components/ui/hotkey-recorder";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import type { Settings } from "@/lib/api";

/** Candidate clip lengths for the CLIPS duration dropdown; filtered to what the
 *  replay buffer can actually hold (`buffer_seconds`). */
const CLIP_LENGTHS = [10, 15, 30, 60, 90, 120, 180];

/** The interactive titlebar pill that opens a hotkey popover. */
function PillTrigger({
  children,
  active,
}: {
  children: React.ReactNode;
  active?: boolean;
}) {
  return (
    <PopoverTrigger asChild>
      <button
        type="button"
        className={cn(
          "flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs font-medium transition-colors",
          active
            ? "border-border bg-secondary text-foreground"
            : "border-border bg-secondary/50 text-foreground hover:bg-secondary"
        )}
      >
        {children}
      </button>
    </PopoverTrigger>
  );
}

/** Popover header: section label on the left, "Manage Hotkeys →" on the right. */
function PopoverHeader({
  label,
  onManage,
}: {
  label: string;
  onManage: () => void;
}) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-xs font-semibold tracking-wider text-muted-foreground/80 uppercase">
        {label}
      </span>
      <PopoverClose asChild>
        <button
          type="button"
          onClick={onManage}
          className="flex items-center gap-1 text-xs font-medium text-foreground/90 transition-colors hover:text-foreground"
        >
          Manage Hotkeys
          <ArrowRight className="size-3.5" />
        </button>
      </PopoverClose>
    </div>
  );
}

/** Info line under the header (icon + description). */
function PopoverNote({ children }: { children: React.ReactNode }) {
  return (
    <p className="flex items-start gap-2 text-xs leading-relaxed text-muted-foreground">
      <Info className="mt-0.5 size-4 shrink-0" weight="fill" />
      <span>{children}</span>
    </p>
  );
}

/** Shared settings read + optimistic write (`["settings"]` query). */
function useSettingsPatch() {
  const { data: settings } = useSettings();
  const update = useUpdateSettings();
  const patch = (p: Partial<Settings>) => {
    if (settings) update.mutate({ ...settings, ...p });
  };
  return { settings, patch };
}

/**
 * CLIPS popover — the save-clip hotkey + how many seconds it captures. Anchored
 * to the "Clip" titlebar pill. Both controls persist instantly through the
 * settings mutation; editing the hotkey re-registers the OS shortcut in the core.
 */
export function ClipHotkeyPopover() {
  const navigate = useNavigate();
  const { settings, patch } = useSettingsPatch();
  const hotkey = settings?.save_hotkey ?? "F9";
  const clipSeconds = settings?.clip_seconds ?? 30;
  const bufferSeconds = settings?.buffer_seconds ?? 120;
  const lengths = CLIP_LENGTHS.filter((s) => s <= bufferSeconds);

  return (
    <Popover>
      <PillTrigger>
        <KeyCombo accel={hotkey} />
        <span>Clip {clipSeconds}s</span>
      </PillTrigger>
      <PopoverContent align="start" className="w-[380px] p-4">
        <PopoverHeader
          label="Clips"
          onManage={() => navigate({ to: "/settings", search: { section: "clip" } })}
        />
        <div className="mt-3">
          <PopoverNote>
            Press your clip hotkey to instantly save the last few seconds of
            gameplay. The replay buffer uses no disk space until you clip.
          </PopoverNote>
        </div>
        <div className="mt-4 flex items-stretch gap-2">
          <HotkeyRecorder
            size="lg"
            aria-label="Save-clip hotkey"
            value={hotkey}
            onChange={(accel) => accel && patch({ save_hotkey: accel })}
            allowClear={false}
            className="flex-1"
          />
          <Select
            value={String(clipSeconds)}
            onValueChange={(v) => patch({ clip_seconds: Number(v) })}
          >
            <SelectTrigger className="h-14! w-24 shrink-0 bg-secondary text-base font-semibold">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {lengths.map((s) => (
                <SelectItem key={s} value={String(s)}>
                  {s}s
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </PopoverContent>
    </Popover>
  );
}

/**
 * RECORDING popover — the long-recording start/stop hotkey. Anchored to the
 * "Long Recording" titlebar pill. The hotkey persists, but the manual
 * long-recording capture feature itself isn't wired yet (display-only).
 */
export function RecordingHotkeyPopover() {
  const navigate = useNavigate();
  const { settings, patch } = useSettingsPatch();
  const hotkey = settings?.long_recording_hotkey ?? "Alt+F7";

  return (
    <Popover>
      <PillTrigger>
        <KeyCombo accel={hotkey} />
        <span>Long Recording</span>
      </PillTrigger>
      <PopoverContent align="start" className="w-[380px] p-4">
        <PopoverHeader
          label="Recording"
          onManage={() => navigate({ to: "/settings", search: { section: "clip" } })}
        />
        <div className="mt-3">
          <PopoverNote>
            Begin and end a recording of any length. Manual long recording is
            coming soon. The hotkey is saved and ready.
          </PopoverNote>
        </div>
        <div className="mt-4">
          <HotkeyRecorder
            size="lg"
            aria-label="Long-recording hotkey"
            value={hotkey}
            onChange={(accel) => accel && patch({ long_recording_hotkey: accel })}
            allowClear={false}
          />
        </div>
        <PopoverClose asChild>
          <button
            type="button"
            onClick={() => navigate({ to: "/settings", search: { section: "storage" } })}
            className="mt-3 flex w-full items-center justify-between rounded-md px-1 py-2 text-sm font-medium text-foreground transition-colors hover:text-foreground/80"
          >
            Storage limit and settings
            <ArrowRight className="size-4 text-muted-foreground" />
          </button>
        </PopoverClose>
      </PopoverContent>
    </Popover>
  );
}
