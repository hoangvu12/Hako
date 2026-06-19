import * as React from "react";
import {
  ALL_FORMATS,
  AudioBufferSink,
  CustomSource,
  Input,
  type InputAudioTrack,
} from "mediabunny";

import { readClipRange, type AudioTrackInfo } from "@/lib/api";

/**
 * Live per-stem audio mixing for the clip editor.
 *
 * A multi-track clip's `<video>` element can only play audio track 0 (the master
 * mix), so the editor's per-stem mute/solo/volume couldn't be *heard* during
 * preview — only applied on export. This hook closes that gap: it decodes each
 * stem track (via mediabunny) into a Web Audio `AudioBuffer`, plays them through
 * a per-stem `GainNode` graph, and keeps that graph locked to the (muted)
 * `<video>`'s clock. The stem gains map 1:1 onto the export re-mix, so what you
 * hear matches what you save.
 *
 * The video stays the master clock; this hook only follows it. We re-anchor the
 * audio on every play/seek/loop/rate change and correct slow drift each frame.
 * Decoding runs off the main path: until it finishes (or if it fails, or there
 * are no stems) `active` is false and the caller leaves the native `<video>`
 * audio playing — a seamless fallback that also covers single-track clips.
 */

/** Lead time when (re)starting buffer sources, so `start()` isn't in the past. */
const START_LOOKAHEAD = 0.03;
/** Resync once |audio − video| exceeds this (seconds). Above human-perceptible. */
const DRIFT_MAX = 0.05;
/** Gain ramp to dodge zipper clicks on mute/volume changes (seconds). */
const GAIN_RAMP = 0.012;

export interface UseTrackMixerArgs {
  /** Clip id — stem bytes are pulled over IPC (mediabunny can't fetch the
   *  `hakoclip://` scheme; see `readClipRange`). */
  clipId: number;
  /** Clip file size in bytes — the `CustomSource`'s `getSize`. */
  fileSize: number;
  /** Audio stems (index ≥ 1); empty ⇒ mixer disabled, native audio kept. */
  stems: AudioTrackInfo[];
  videoRef: React.RefObject<HTMLVideoElement | null>;
  /** Per-stem linear gain (0..1) keyed by stem index — solo/mute already resolved. */
  stemGains: Map<number, number>;
  /** Master monitor gain (0..1) from the top-bar mute/volume. */
  masterGain: number;
}

/** One playing buffer-source set's time anchor, for drift math. */
interface Anchor {
  /** `AudioContext.currentTime` at which the sources begin. */
  ctxTime: number;
  /** Media time (video clock, seconds) the sources begin at. */
  mediaTime: number;
  /** Playback rate captured at (re)start. */
  rate: number;
}

interface Graph {
  ctx: AudioContext;
  master: GainNode;
  /** Per-stem gain node, keyed by stem index. */
  gains: Map<number, GainNode>;
  /** Per-stem decoded buffer, keyed by stem index (empty stems omitted). */
  buffers: Map<number, AudioBuffer>;
}

/**
 * Decode one stem track fully into a single `AudioBuffer` on `ctx`, preserving
 * gaps (each chunk is placed at its own timestamp). Returns null for an empty
 * track.
 */
async function decodeStem(
  ctx: AudioContext,
  track: InputAudioTrack,
): Promise<AudioBuffer | null> {
  const sink = new AudioBufferSink(track);
  const chunks: { buffer: AudioBuffer; timestamp: number }[] = [];
  let channels = 0;
  let sampleRate = 0;
  let endTime = 0;
  for await (const { buffer, timestamp } of sink.buffers()) {
    channels = Math.max(channels, buffer.numberOfChannels);
    sampleRate = sampleRate || buffer.sampleRate;
    endTime = Math.max(endTime, timestamp + buffer.duration);
    chunks.push({ buffer, timestamp });
  }
  if (!chunks.length || !sampleRate) return null;

  const length = Math.max(1, Math.ceil(endTime * sampleRate));
  const out = ctx.createBuffer(channels, length, sampleRate);
  for (const { buffer, timestamp } of chunks) {
    const at = Math.round(timestamp * sampleRate);
    if (at >= length) continue;
    const copy = Math.min(buffer.length, length - at);
    for (let ch = 0; ch < channels; ch++) {
      // Stems are uniformly mono or stereo; if a chunk is narrower, reuse its
      // last channel rather than leaving a silent gap.
      const srcCh = Math.min(ch, buffer.numberOfChannels - 1);
      const data = buffer.getChannelData(srcCh);
      out.getChannelData(ch).set(data.subarray(0, copy), at);
    }
  }
  return out;
}

export function useTrackMixer({
  clipId,
  fileSize,
  stems,
  videoRef,
  stemGains,
  masterGain,
}: UseTrackMixerArgs): { active: boolean; decoding: boolean } {
  const [active, setActive] = React.useState(false);
  // True while a stems clip's decode is in flight (before live mixing engages),
  // so the UI can show a "preparing" state instead of silently-inert controls.
  // Distinct from `!active`, which also covers the no-stems / decode-failed
  // fallback where the native <video> audio is the (final) answer, not a wait.
  const [decoding, setDecoding] = React.useState(false);

  const graphRef = React.useRef<Graph | null>(null);
  const sourcesRef = React.useRef<AudioBufferSourceNode[]>([]);
  const anchorRef = React.useRef<Anchor | null>(null);
  const rafRef = React.useRef<number | null>(null);
  // Latest gain targets, read when a decode finishes (async, long after commit)
  // so a stem the user already muted starts at the right level. Synced in an
  // effect, not during render: writing a ref mid-render is a Rules-of-React
  // violation that React Compiler miscompiles (it broke live mixing). A commit's
  // worth of lag is irrelevant here — decode completion is seconds away.
  const stemGainsRef = React.useRef(stemGains);
  const masterGainRef = React.useRef(masterGain);
  React.useEffect(() => {
    stemGainsRef.current = stemGains;
    masterGainRef.current = masterGain;
  });

  const hasStems = stems.length > 0;
  // Stable key so the decode effect re-runs only when the clip or its stem set
  // actually changes (not on every gain tweak).
  const stemKey = stems.map((s) => s.index).join(",");

  // --- Stop / (re)start the per-stem buffer sources, anchored to the video. ---
  const stopSources = React.useCallback(() => {
    for (const s of sourcesRef.current) {
      try {
        s.onended = null;
        s.stop();
        s.disconnect();
      } catch {
        /* already stopped */
      }
    }
    sourcesRef.current = [];
  }, []);

  const resync = React.useCallback(
    (mediaTime: number) => {
      const graph = graphRef.current;
      const v = videoRef.current;
      stopSources();
      if (!graph || !v || v.paused || graph.ctx.state !== "running") {
        anchorRef.current = null;
        return;
      }
      const rate = v.playbackRate || 1;
      const when = graph.ctx.currentTime + START_LOOKAHEAD;
      // Where the video will be once the sources actually fire, so audio lines
      // up instead of starting a lookahead behind.
      const startMedia = mediaTime + START_LOOKAHEAD * rate;
      const started: AudioBufferSourceNode[] = [];
      for (const [idx, buffer] of graph.buffers) {
        const gain = graph.gains.get(idx);
        if (!gain || startMedia >= buffer.duration) continue;
        const node = graph.ctx.createBufferSource();
        node.buffer = buffer;
        node.playbackRate.value = rate;
        node.connect(gain);
        node.start(when, Math.max(0, startMedia));
        started.push(node);
      }
      sourcesRef.current = started;
      anchorRef.current = { ctxTime: when, mediaTime: startMedia, rate };
    },
    [videoRef, stopSources],
  );

  // --- Decode stems, build the graph, engage live mixing. Re-runs per clip. ---
  React.useEffect(() => {
    if (!hasStems) {
      setActive(false);
      setDecoding(false);
      return;
    }
    let cancelled = false;
    let input: Input | null = null;
    let ctx: AudioContext | null = null;
    setDecoding(true);

    (async () => {
      try {
        input = new Input({
          // mediabunny can't `fetch()` the `hakoclip://` scheme (WebView2 blocks
          // cross-scheme fetch by CORS), so stem bytes come over IPC. `end` is
          // exclusive; the backend clamps it to the file size.
          source: new CustomSource({
            read: (start, end) =>
              readClipRange(clipId, start, end).then((b) => new Uint8Array(b)),
            getSize: () => fileSize,
            // Audio is finely interleaved with 20 Mbps video, so decoding a stem
            // touches byte ranges spanning the *whole* file. mediabunny's default
            // ("none") issues one IPC round-trip per granular read — thousands of
            // tiny `read_clip_range` invokes (~30s). "fileSystem" (fixed 64 KiB
            // windows) barely helped because each window still holds ~1 audio
            // frame. "network" instead grows the read-ahead exponentially up to
            // 8 MiB for the sequential forward scan a full-track decode is,
            // collapsing thousands of reads into a few dozen big ones. Padded
            // ranges are clamped to the file size by the orchestrator, so they
            // never exceed EOF (the backend would otherwise return a short
            // buffer). The big cache lets the parallel per-stem scans (below)
            // share fetched windows instead of each re-reading the whole file.
            prefetchProfile: "network",
            maxCacheSize: 64 * 2 ** 20,
          }),
          formats: ALL_FORMATS,
        });
        const audioTracks = await input.getAudioTracks();
        if (cancelled) return;

        ctx = new AudioContext();
        const master = ctx.createGain();
        master.gain.value = masterGainRef.current;
        master.connect(ctx.destination);

        const gains = new Map<number, GainNode>();
        const buffers = new Map<number, AudioBuffer>();
        // Decode every stem concurrently. They share one `input`/source, so the
        // scans advance through the file together and hit the orchestrator cache
        // for each other's windows — the file is read ~once, not once per stem
        // (sequential decode rescanned the whole file N times). `decodeStem` only
        // touches its own track + the shared (thread-safe) source.
        const decoded = await Promise.all(
          stems.map(async (s) => {
            // Audio ordinal (0 = master, 1..N stems) indexes the track list 1:1
            // with the backend's `audio_stream_indices` ordering.
            const track = audioTracks[s.index];
            if (!track) return null;
            const buffer = await decodeStem(ctx!, track);
            return buffer ? { index: s.index, buffer } : null;
          }),
        );
        if (cancelled) return;
        for (const d of decoded) {
          if (!d) continue;
          const gain = ctx.createGain();
          gain.gain.value = stemGainsRef.current.get(d.index) ?? 1;
          gain.connect(master);
          gains.set(d.index, gain);
          buffers.set(d.index, d.buffer);
        }
        if (!buffers.size) return;

        graphRef.current = { ctx, master, gains, buffers };
        const v = videoRef.current;
        // Web Audio now owns playback — silence the element's master track.
        if (v) v.muted = true;
        await ctx.resume().catch(() => {});
        setActive(true);
        if (v && !v.paused) resync(v.currentTime);
      } catch (err) {
        // Decode/read failed → leave `active` false: the caller keeps native
        // <video> audio (track 0), the same fallback as a no-stems clip.
        console.warn("[track-mixer] live decode failed; keeping native audio", err);
        if (ctx) ctx.close().catch(() => {});
        ctx = null;
      } finally {
        // Settled (engaged, fell back, or early-returned). A cancelled run is
        // mid-cleanup — leave the flag to the cleanup / next run to avoid a flash.
        if (!cancelled) setDecoding(false);
      }
    })();

    return () => {
      cancelled = true;
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
      stopSources();
      anchorRef.current = null;
      // Close whichever context exists — the built graph's, or a bare one from a
      // decode that was cancelled before the graph was wired up.
      const built = graphRef.current;
      graphRef.current = null;
      setActive(false);
      input?.dispose();
      (built?.ctx ?? ctx)?.close().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId, fileSize, stemKey, hasStems]);

  // --- Live gain updates (cheap; no resync needed — gains are AudioParams). ---
  React.useEffect(() => {
    const graph = graphRef.current;
    if (!graph || !active) return;
    const t = graph.ctx.currentTime;
    for (const [idx, node] of graph.gains) {
      node.gain.setTargetAtTime(stemGains.get(idx) ?? 0, t, GAIN_RAMP);
    }
    graph.master.gain.setTargetAtTime(masterGain, t, GAIN_RAMP);
  }, [stemGains, masterGain, active]);

  // --- Follow the video: (re)anchor on play/seek/rate, suspend on pause. ------
  React.useEffect(() => {
    const v = videoRef.current;
    if (!v || !active) return;

    const onPlay = () => {
      const ctx = graphRef.current?.ctx;
      if (!ctx) return;
      ctx.resume().then(() => resync(v.currentTime)).catch(() => {});
    };
    const onPause = () => {
      stopSources();
      anchorRef.current = null;
    };
    // `seeking` fires on every currentTime write — user scrubs, I/O keys, and the
    // editor's loop-back at trimEnd all land here, so audio re-anchors in lockstep.
    const onSeek = () => {
      if (!v.paused) resync(v.currentTime);
    };
    const onRateChange = () => {
      if (!v.paused) resync(v.currentTime);
    };

    v.addEventListener("play", onPlay);
    v.addEventListener("pause", onPause);
    v.addEventListener("seeking", onSeek);
    v.addEventListener("ratechange", onRateChange);

    // Per-frame drift correction (cheap; self-guards while paused/suspended).
    const tick = () => {
      const graph = graphRef.current;
      const a = anchorRef.current;
      if (graph && a && !v.paused && graph.ctx.state === "running") {
        const ctxNow = graph.ctx.currentTime;
        if (ctxNow >= a.ctxTime) {
          const expected = a.mediaTime + (ctxNow - a.ctxTime) * a.rate;
          if (Math.abs(v.currentTime - expected) > DRIFT_MAX) resync(v.currentTime);
        }
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);

    // Engage immediately if the clip is already rolling (autoplay).
    if (!v.paused) resync(v.currentTime);

    return () => {
      v.removeEventListener("play", onPlay);
      v.removeEventListener("pause", onPause);
      v.removeEventListener("seeking", onSeek);
      v.removeEventListener("ratechange", onRateChange);
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
      stopSources();
    };
  }, [active, videoRef, resync, stopSources]);

  return { active, decoding };
}
