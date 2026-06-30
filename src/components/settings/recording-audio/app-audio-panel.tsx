import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { GameController, Waveform } from "@phosphor-icons/react";

import {
  listActiveAudioSessions,
  GAME_SOURCE_ID,
  type AudioConfig,
  type AudioAppSel,
} from "@/lib/api";
import { Panel, SourceRow, VolumeSlider } from "./primitives";
import { MicRow } from "./mic-row";
import { SESSION_BLACKLIST, upsertApp } from "./helpers";

/**
 * "Specific apps" mode: a dedicated Game Audio row plus any other app currently
 * playing audio. Owns the live "apps playing audio" poll (every 3s) so that the
 * polling re-render is scoped to this subtree and only runs while this mode is
 * actually mounted.
 */
export function AppAudioPanel({
  audio,
  patch,
}: {
  audio: AudioConfig;
  patch: (p: Partial<AudioConfig>) => void;
}) {
  const { data: sessions } = useQuery({
    queryKey: ["audio-sessions"],
    queryFn: listActiveAudioSessions,
    retry: false,
    refetchInterval: 3000,
  });

  const game =
    audio.apps.find((a) => a.id === GAME_SOURCE_ID) ??
    ({ id: GAME_SOURCE_ID, name: "Game Audio", enabled: true, volume: 100 } as AudioAppSel);

  // Apps to show: saved sources (minus game) + live sessions not already saved.
  const appRows = useMemo(() => {
    const saved = audio.apps.filter((a) => a.id !== GAME_SOURCE_ID);
    const seen = new Set(saved.map((a) => a.id));
    // Single pass: filter + shape the live sessions in one reduce so we don't
    // walk the session list twice.
    const live = (sessions ?? []).reduce<AudioAppSel[]>((acc, s) => {
      if (
        !SESSION_BLACKLIST.has(s.process_name.toLowerCase()) &&
        !seen.has(s.process_name)
      ) {
        acc.push({
          id: s.process_name,
          name: s.display_name || s.process_name,
          enabled: false,
          volume: 100,
        });
      }
      return acc;
    }, []);
    return [...saved, ...live];
  }, [audio.apps, sessions]);

  // Real app icons keyed by process name (lowercased), from the live sessions.
  const iconByName = useMemo(() => {
    const m = new Map<string, string>();
    for (const s of sessions ?? []) {
      if (s.icon) m.set(s.process_name.toLowerCase(), s.icon);
    }
    return m;
  }, [sessions]);

  return (
    <Panel
      title="App audio"
      hint="Additional apps appear here when they start playing audio."
    >
      <SourceRow
        icon={GameController}
        label="Game Audio"
        checked={game.enabled}
        onCheckedChange={(v) =>
          patch({
            apps: upsertApp(audio.apps, GAME_SOURCE_ID, "Game Audio", {
              enabled: v,
            }),
          })
        }
      >
        <VolumeSlider
          value={game.volume}
          onCommit={(v) =>
            patch({
              apps: upsertApp(audio.apps, GAME_SOURCE_ID, "Game Audio", {
                volume: v,
              }),
            })
          }
        />
      </SourceRow>
      <MicRow audio={audio} patch={patch} />
      {appRows.map((a) => (
        <SourceRow
          key={a.id}
          icon={Waveform}
          iconUrl={iconByName.get(a.id.toLowerCase())}
          label={a.name}
          checked={a.enabled}
          onCheckedChange={(v) =>
            patch({
              apps: upsertApp(audio.apps, a.id, a.name, { enabled: v }),
            })
          }
        >
          <VolumeSlider
            value={a.volume}
            onCommit={(v) =>
              patch({
                apps: upsertApp(audio.apps, a.id, a.name, { volume: v }),
              })
            }
          />
        </SourceRow>
      ))}
      {appRows.length === 0 && (
        <p className="py-3 text-xs text-muted-foreground last:pb-0">
          No other apps are playing audio right now.
        </p>
      )}
    </Panel>
  );
}
