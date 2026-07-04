import { useEffect, useRef, useState } from "react";
import { createLazyRoute, useSearch } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MagnifyingGlass, CircleNotch } from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import {
  countClipsIn,
  getGpuInfo,
  migrateClipsTo,
  type EventTiming,
  type EventToggles,
  type GameModeToggles,
  type AutoCaptureMode,
  type SmartGameKey,
  type Settings,
} from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";
import {
  NAV,
  PRESETS,
  isSectionKey,
  type SectionKey,
} from "@/components/settings/config";
import { ClipSection } from "@/components/settings/sections/clip-section";
import { VideoSection } from "@/components/settings/sections/video-section";
import { CaptureSection } from "@/components/settings/sections/capture-section";
import { AudioSection } from "@/components/settings/sections/audio-section";
import { AutoSection } from "@/components/settings/sections/auto-section";
import { StorageSection } from "@/components/settings/sections/storage-section";
import { CloudSection } from "@/components/settings/sections/cloud-section";
import { OverlaySection } from "@/components/settings/sections/overlay-section";
import { StatusSection } from "@/components/settings/sections/status-section";

// Lazy-loaded: only the component splits out — `validateSearch` stays eager in
// the route tree (router.tsx), which is required for type-safe search params.
export const Route = createLazyRoute("/settings")({
  component: SettingsPage,
});

function SettingsPage() {
  const { data } = useSettings();
  // `mutate`/`error` are pulled out individually: `mutate` is referentially
  // stable (react-query), so the mutators below stay stable too.
  const { mutate: saveSettings, error: saveError } = useUpdateSettings();
  const qc = useQueryClient();
  // GPU list for the "Selected GPU" dropdown. Cheap, cached; failure just leaves
  // the dropdown with the Auto option.
  const { data: gpus } = useQuery({
    queryKey: queryKeys.gpuInfo,
    queryFn: getGpuInfo,
    staleTime: 60_000,
    retry: false,
  });
  const search = useSearch({ from: "/settings" });
  const [draft, setDraft] = useState<Settings | null>(null);
  // Latest draft mirrored into a ref so the mutators can read it without closing
  // over `draft` — that keeps their identity stable across renders, which lets
  // the React Compiler memoize each section so only the control you touched
  // re-renders instead of the whole page.
  const draftRef = useRef<Settings | null>(null);
  useEffect(() => {
    draftRef.current = draft;
  }, [draft]);
  const [active, setActive] = useState<SectionKey>(
    isSectionKey(search.section) ? search.section : "clip"
  );
  const [navQuery, setNavQuery] = useState("");
  // "Move existing clips to the new folder?" prompt, shown after the clip folder
  // changes while clips still live in the old one. `null` = no prompt open.
  const [movePrompt, setMovePrompt] = useState<{
    from: string | null;
    to: string | null;
    count: number;
  } | null>(null);
  // The actual move (off the UI thread on the Rust side). `isPending` drives the
  // dialog's spinner; clips refetch on success so cards point at the new paths.
  const moveClips = useMutation({
    mutationFn: ({ from, to }: { from: string | null; to: string | null }) =>
      migrateClipsTo(from, to),
    onSettled: () => {
      qc.invalidateQueries({ queryKey: queryKeys.clips });
      qc.invalidateQueries({ queryKey: queryKeys.cloudRetention });
    },
  });

  // Initialise the draft once; instant-apply edits keep it in sync afterwards.
  useEffect(() => {
    if (data && !draft) setDraft(data);
  }, [data, draft]);

  // A failed save rolls the cache back to the last persisted settings; mirror
  // that into the draft so the UI stops showing the value that didn't save.
  // Keyed on the error edge (not `data`) so it can't clobber in-flight edits.
  useEffect(() => {
    if (!saveError) return;
    const persisted = qc.getQueryData<Settings>(queryKeys.settings);
    if (persisted) setDraft(persisted);
  }, [saveError, qc]);

  // Deep-link: jump to a section when navigated with `?section=` (e.g. from the
  // recorder popover) — including while the page is already mounted.
  useEffect(() => {
    if (isSectionKey(search.section)) setActive(search.section);
  }, [search.section]);

  if (!draft) {
    return (
      <div className="p-8 text-sm text-muted-foreground">Loading settings…</div>
    );
  }

  // All mutators read the live draft from `draftRef` rather than the `draft`
  // closure, so their identity is stable render-to-render. Each guards on a
  // non-null draft (we're past the loading return, so it's always set in
  // practice) to satisfy the type and the impossible early-call case.
  const persist = (next: Settings) => {
    setDraft(next);
    saveSettings(next);
  };
  // Instant-apply for toggles/selects.
  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const d = draftRef.current;
    if (d) persist({ ...d, [key]: value });
  };
  // Local edit (number/text) — committed on blur to avoid a save per keystroke.
  const setLocal = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const d = draftRef.current;
    if (d) setDraft({ ...d, [key]: value });
  };
  const commit = () => {
    const d = draftRef.current;
    if (d) saveSettings(d);
  };
  // Persist a new clip folder, then — if existing clips still live in the old
  // one — offer to move them. The old value is read from the persisted cache
  // (not the draft, which live-updates as the user types) so it's the real
  // previous folder. New clips already save to the new folder regardless.
  const changeStorageDir = (next: string | null) => {
    const value = next?.trim() ? next : null;
    const prev =
      qc.getQueryData<Settings>(queryKeys.settings)?.storage_dir?.trim() || null;
    set("storage_dir", value);
    if (value === prev) return; // no real change (e.g. blur without an edit)
    void countClipsIn(prev)
      .then((count) => {
        if (count > 0) setMovePrompt({ from: prev, to: value, count });
      })
      .catch(() => {});
  };
  // Commit the typed clip-folder on blur — reads the live draft so a fast
  // type-then-blur still sees the final value.
  const onCommitStorage = () =>
    changeStorageDir(draftRef.current?.storage_dir ?? null);
  // Apply a preset: highlight its card and write its concrete knobs at once.
  const applyPreset = (p: (typeof PRESETS)[number]) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        quality_preset: p.key,
        resolution: p.resolution,
        target_fps: p.fps,
        bitrate_mbps: p.bitrate,
      });
  };
  const toggleEvent = (key: keyof EventToggles) => {
    const d = draftRef.current;
    if (d) persist({ ...d, events: { ...d.events, [key]: !d.events[key] } });
  };
  const toggleGameMode = (key: keyof GameModeToggles) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        auto_clip_modes: {
          ...d.auto_clip_modes,
          [key]: !d.auto_clip_modes[key],
        },
      });
  };
  // Per-event timing edits. `setTimingLocal` updates the draft live while
  // dragging (no save per pixel); `commitTiming` persists the final value on
  // release. The commit takes the explicit value (not a stale closure read) so a
  // single click — where onValueChange + onValueCommit fire in the same tick —
  // still saves the new value.
  const timingNext = (
    d: Settings,
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ): Settings => ({
    ...d,
    event_timings: {
      ...d.event_timings,
      [key]: { ...d.event_timings[key], [field]: value },
    },
  });
  const setTimingLocal = (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ) => {
    const d = draftRef.current;
    if (d) setDraft(timingNext(d, key, field, value));
  };
  const commitTiming = (
    key: keyof EventToggles,
    field: "before" | "after",
    value: number
  ) => {
    const d = draftRef.current;
    if (d) persist(timingNext(d, key, field, value));
  };

  // --- Smart per-game auto-capture handlers, keyed by game id ----------------
  // Every smart game's settings slice (`games.lol`, `games.cs2`, …) shares the
  // same shape, so one generic set of handlers keyed by `SmartGameKey` drives
  // all of them — adding a game needs no new handler here, just a registry
  // entry + a model builder in AutoSection. The event key/field are `string`
  // because the generic auto-capture card is game-agnostic; each game's own
  // toggle/timing types stay enforced where the card reads them back out.
  //
  // The casts are confined to this seam: the dynamic `[eventKey]` write can't be
  // expressed against the per-game union without naming each game, which is the
  // duplication we're removing. Reads elsewhere remain fully typed.
  type GameSlice = Settings["games"][SmartGameKey];
  const patchGame = (d: Settings, key: SmartGameKey, slice: GameSlice): Settings => ({
    ...d,
    games: { ...d.games, [key]: slice } as Settings["games"],
  });
  const setGameMode = (key: SmartGameKey, mode: AutoCaptureMode) => {
    const d = draftRef.current;
    if (d) persist(patchGame(d, key, { ...d.games[key], auto_capture_mode: mode }));
  };
  const setGameDisabled = (key: SmartGameKey, disabled: boolean) => {
    const d = draftRef.current;
    if (d) persist(patchGame(d, key, { ...d.games[key], disabled }));
  };
  const toggleGameEvent = (key: SmartGameKey, eventKey: string) => {
    const d = draftRef.current;
    if (!d) return;
    const slice = d.games[key];
    const events = slice.events as unknown as Record<string, boolean>;
    persist(
      patchGame(d, key, {
        ...slice,
        events: { ...events, [eventKey]: !events[eventKey] },
      } as unknown as GameSlice)
    );
  };
  // Live drag applies locally (`setGameTimingLocal`, no save per pixel); release
  // persists (`commitGameTiming`). Both take the explicit value so a single
  // click — where onValueChange + onValueCommit fire in one tick — still saves.
  const gameTimingSlice = (
    slice: GameSlice,
    eventKey: string,
    field: "before" | "after",
    value: number
  ): GameSlice => {
    const timings = slice.event_timings as unknown as Record<string, EventTiming>;
    return {
      ...slice,
      event_timings: {
        ...timings,
        [eventKey]: { ...timings[eventKey], [field]: value },
      },
    } as unknown as GameSlice;
  };
  const setGameTimingLocal = (
    key: SmartGameKey,
    eventKey: string,
    field: "before" | "after",
    value: number
  ) => {
    const d = draftRef.current;
    if (d) setDraft(patchGame(d, key, gameTimingSlice(d.games[key], eventKey, field, value)));
  };
  const commitGameTiming = (
    key: SmartGameKey,
    eventKey: string,
    field: "before" | "after",
    value: number
  ) => {
    const d = draftRef.current;
    if (d) persist(patchGame(d, key, gameTimingSlice(d.games[key], eventKey, field, value)));
  };

  // War Thunder's nickname is the one game-specific field (its combat log is
  // free text). Free-text ⇒ apply locally per keystroke, persist on blur.
  const warThunderNicknameNext = (d: Settings, nickname: string): Settings => ({
    ...d,
    games: { ...d.games, warthunder: { ...d.games.warthunder, nickname } },
  });
  const setWarThunderNickname = (nickname: string) => {
    const d = draftRef.current;
    if (d) setDraft(warThunderNicknameNext(d, nickname));
  };
  const commitWarThunderNickname = (nickname: string) => {
    const d = draftRef.current;
    if (d) persist(warThunderNicknameNext(d, nickname));
  };

  // --- Other Games (generic "record any game") handlers, on games.other ------
  const setOtherMode = (mode: AutoCaptureMode) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        games: { ...d.games, other: { ...d.games.other, auto_capture_mode: mode } },
      });
  };
  const setOtherDisabled = (disabled: boolean) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        games: { ...d.games, other: { ...d.games.other, disabled } },
      });
  };
  const setOtherDetect = (
    key: "detect_steam" | "detect_curated",
    value: boolean
  ) => {
    const d = draftRef.current;
    if (d)
      persist({
        ...d,
        games: { ...d.games, other: { ...d.games.other, [key]: value } },
      });
  };

  const q = navQuery.trim().toLowerCase();
  // Single pass: filter each group's items and keep only non-empty groups in one
  // reduce, instead of mapping then filtering over the group list twice.
  const groups = NAV.reduce<typeof NAV>((acc, g) => {
    const items = g.items.filter((i) => i.label.toLowerCase().includes(q));
    if (items.length) acc.push({ ...g, items });
    return acc;
  }, []);

  return (
    <div className="flex h-full">
      {/* Settings nav */}
      <aside className="flex w-[300px] shrink-0 flex-col border-r border-panel-border bg-panel">
        <div className="p-3">
          <div className="relative">
            <MagnifyingGlass className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={navQuery}
              onChange={(e) => setNavQuery(e.target.value)}
              placeholder="Search settings"
              className="h-9 bg-field pl-9"
            />
          </div>
        </div>
        <nav className="scrollbar-thin flex-1 overflow-y-auto px-2 pb-4">
          {groups.map((g) => (
            <div key={g.group} className="mb-4">
              <div className="px-3 pb-1.5 text-[11px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
                {g.group}
              </div>
              {g.items.map((it) => {
                const on = active === it.key;
                return (
                  <button
                    key={it.key}
                    type="button"
                    onClick={() => setActive(it.key)}
                    className={cn(
                      "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-[15px] transition-colors",
                      on
                        ? "bg-white/10 font-semibold text-foreground"
                        : "font-medium text-foreground/90 hover:bg-accent/60 hover:text-foreground"
                    )}
                  >
                    <it.icon className="size-[18px]" weight={on ? "fill" : "regular"} />
                    {it.label}
                  </button>
                );
              })}
            </div>
          ))}
          {groups.length === 0 && (
            <p className="px-3 text-sm text-muted-foreground">No matches.</p>
          )}
        </nav>
      </aside>

      {/* Content */}
      <div className="scrollbar-thin min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl space-y-6 px-8 py-10">
          {active === "clip" && (
            <ClipSection draft={draft} set={set} setLocal={setLocal} commit={commit} />
          )}

          {active === "quality" && (
            <VideoSection draft={draft} set={set} applyPreset={applyPreset} gpus={gpus} />
          )}

          {active === "capture" && <CaptureSection draft={draft} set={set} />}

          {active === "audio" && <AudioSection draft={draft} set={set} />}

          {active === "auto" && (
            <AutoSection
              draft={draft}
              set={set}
              toggleEvent={toggleEvent}
              toggleGameMode={toggleGameMode}
              setTimingLocal={setTimingLocal}
              commitTiming={commitTiming}
              setGameMode={setGameMode}
              setGameDisabled={setGameDisabled}
              toggleGameEvent={toggleGameEvent}
              setGameTimingLocal={setGameTimingLocal}
              commitGameTiming={commitGameTiming}
              setWarThunderNickname={setWarThunderNickname}
              commitWarThunderNickname={commitWarThunderNickname}
              setOtherMode={setOtherMode}
              setOtherDisabled={setOtherDisabled}
              setOtherDetect={setOtherDetect}
            />
          )}

          {active === "storage" && (
            <StorageSection
              draft={draft}
              setLocal={setLocal}
              changeStorageDir={changeStorageDir}
              onCommitStorage={onCommitStorage}
            />
          )}

          {active === "cloud" && (
            <CloudSection draft={draft} set={set} setLocal={setLocal} commit={commit} />
          )}

          {active === "overlay" && <OverlaySection draft={draft} set={set} />}

          {active === "status" && <StatusSection />}

          {saveError ? (
            <p className="text-sm text-destructive">{String(saveError)}</p>
          ) : null}
        </div>
      </div>

      {/* Opt-in "move existing clips to the new folder?" prompt. The folder change
          is already saved; this only relocates the files on disk. */}
      <AlertDialog
        open={movePrompt !== null}
        onOpenChange={(open) => {
          // Block dismissal while a move is running; otherwise close = "keep".
          if (!open && !moveClips.isPending) setMovePrompt(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Move {movePrompt?.count}{" "}
              {movePrompt?.count === 1 ? "clip" : "clips"} to the new folder?
            </AlertDialogTitle>
            <AlertDialogDescription>
              New clips already save to the new folder. Your{" "}
              {movePrompt?.count === 1 ? "existing clip" : "existing clips"} can
              be moved there too, or left in place — either way they stay in your
              library.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={moveClips.isPending}>
              Keep in place
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={moveClips.isPending}
              onClick={(e) => {
                // Keep the dialog open (with the spinner) until the move finishes.
                e.preventDefault();
                if (!movePrompt) return;
                moveClips.mutate(
                  { from: movePrompt.from, to: movePrompt.to },
                  { onSettled: () => setMovePrompt(null) }
                );
              }}
            >
              {moveClips.isPending ? (
                <CircleNotch className="size-4 animate-spin" />
              ) : null}
              {moveClips.isPending ? "Moving…" : "Move clips"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
