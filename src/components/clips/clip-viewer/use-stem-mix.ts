import * as React from "react";

import { useClipAudioTracks } from "@/hooks/use-library";
import { useTrackMixer } from "@/hooks/use-track-mixer";
import type { AudioTrackInfo } from "@/lib/api";

import { DEFAULT_CTL, type TrackCtl } from "./constants";

/**
 * Multi-track audio state for the clip editor, extracted from `ViewerStage`.
 *
 * Stems are the audio tracks past the master (index 0). When a clip has stems
 * the editor offers per-track mute/solo/volume/denoise, previewed live through a
 * Web Audio gain graph (`useTrackMixer`) synced to the muted `<video>`, and
 * applied on export via a re-mux (browsers can't switch MP4 audio tracks live).
 *
 * Owns `audioEnabled` (the master "keep audio" switch) and the per-stem control
 * map; derives everything the stage + save dialog need from them. `muted`/
 * `volume` stay in the component (they also drive the `m` shortcut and the top
 * bar), and are passed in only to compute the preview's master monitor gain.
 */
export function useStemMix({
  clipId,
  fileSize,
  videoRef,
  muted,
  volume,
}: {
  clipId: number;
  fileSize: number;
  videoRef: React.RefObject<HTMLVideoElement | null>;
  muted: boolean;
  volume: number;
}) {
  const { data: audioTracks } = useClipAudioTracks(clipId);
  const stems = React.useMemo<AudioTrackInfo[]>(
    () => (audioTracks ?? []).filter((t) => t.index >= 1),
    [audioTracks],
  );
  const hasStems = stems.length > 0;

  const [audioEnabled, setAudioEnabled] = React.useState(true);
  const [trackCtl, setTrackCtl] = React.useState<Record<number, TrackCtl>>({});
  const ctlOf = React.useCallback(
    (idx: number): TrackCtl => trackCtl[idx] ?? DEFAULT_CTL,
    [trackCtl],
  );
  const patchTrack = React.useCallback(
    (idx: number, patch: Partial<TrackCtl>) =>
      setTrackCtl((prev) => ({
        ...prev,
        [idx]: { ...(prev[idx] ?? DEFAULT_CTL), ...patch },
      })),
    [],
  );
  // Stable handlers so the memoized <AudioSettingsPopover> doesn't re-render on
  // unrelated stage updates (trim drags, etc.) — only when the stem state it
  // actually reads changes.
  const toggleAudio = React.useCallback(() => setAudioEnabled((a) => !a), []);
  const onStemMute = React.useCallback(
    (idx: number) => patchTrack(idx, { muted: !ctlOf(idx).muted }),
    [patchTrack, ctlOf],
  );
  const onStemSolo = React.useCallback(
    (idx: number) => patchTrack(idx, { solo: !ctlOf(idx).solo }),
    [patchTrack, ctlOf],
  );
  const onStemVolume = React.useCallback(
    (idx: number, v: number) => patchTrack(idx, { volume: v }),
    [patchTrack],
  );
  const onStemDenoise = React.useCallback(
    (idx: number) => patchTrack(idx, { denoise: !ctlOf(idx).denoise }),
    [patchTrack, ctlOf],
  );

  const soloActive = stems.some((s) => ctlOf(s.index).solo);
  // A stem is audible when soloed (if any solo is active) or simply un-muted.
  const audibleStems = stems.filter((s) =>
    soloActive ? ctlOf(s.index).solo : !ctlOf(s.index).muted,
  );
  // The mix differs from the recorded master when a stem is muted/soloed, its
  // volume moved, or noise cancel is on — otherwise we keep the loss-less stream
  // copy. Uses `ctlOf` (not raw `trackCtl`) so the mic's default-on noise cancel
  // counts even before the user touches anything.
  const tracksEdited =
    hasStems &&
    stems.some((s) => {
      const c = ctlOf(s.index);
      return c.muted || c.solo || c.volume !== 100 || c.denoise;
    });
  // Stem indices to noise-cancel in the live preview — kept in lockstep with the
  // export's per-stem `denoise` flag so what you hear matches what you save.
  const denoiseStemIdx = React.useMemo(
    () => stems.filter((s) => ctlOf(s.index).denoise).map((s) => s.index),
    [stems, ctlOf],
  );

  // Live per-stem mixing: decode the stems and play them through a Web Audio
  // gain graph synced to the (muted) <video>, so mute/solo/volume are *heard*
  // during preview — not just applied on export. `active` is false (native
  // <video> audio kept) for no-stems clips or until/unless the decode succeeds.
  const stemGains = React.useMemo(() => {
    const m = new Map<number, number>();
    for (const s of stems) {
      const c = ctlOf(s.index);
      const audible = soloActive ? c.solo : !c.muted;
      m.set(s.index, audible ? c.volume / 100 : 0);
    }
    return m;
  }, [stems, ctlOf, soloActive]);
  // Top-bar mute/volume is the monitor level (preview-only; not in the export mix).
  const masterMonitorGain = muted ? 0 : volume;
  const {
    active: liveMix,
    decoding: mixDecoding,
    denoisingIdx,
  } = useTrackMixer({
    clipId,
    fileSize,
    stems,
    videoRef,
    stemGains,
    masterGain: masterMonitorGain,
    denoiseStemIdx,
  });

  return {
    audioEnabled,
    setAudioEnabled,
    toggleAudio,
    stems,
    hasStems,
    ctlOf,
    setTrackCtl,
    onStemMute,
    onStemSolo,
    onStemVolume,
    onStemDenoise,
    soloActive,
    audibleStems,
    tracksEdited,
    liveMix,
    mixDecoding,
    denoisingIdx,
  };
}
