import * as React from "react";
import { Input } from "mediabunny";

import { denoiseAudioBufferForPreview } from "@/lib/denoise-preview";
import { DRIFT_MAX, GAIN_RAMP, START_LOOKAHEAD } from "./track-mixer/constants";
import { createStemInput, decodeStem } from "./track-mixer/decode";
import type { Anchor, Graph, UseTrackMixerArgs } from "./track-mixer/types";

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
 *
 * Stem decoding (`createStemInput` / `decodeStem`), the time/graph types
 * (`Anchor`, `Graph`, `UseTrackMixerArgs`), and the drift/ramp constants live in
 * sibling `track-mixer/` modules; this file owns the React state, refs, and
 * effects that wire them to the live `<video>`.
 */
export function useTrackMixer({
  clipId,
  fileSize,
  stems,
  videoRef,
  stemGains,
  masterGain,
  denoiseStemIdx,
}: UseTrackMixerArgs): {
  active: boolean;
  decoding: boolean;
  denoisingIdx: number[];
} {
  const [active, setActive] = React.useState(false);
  // True while a stems clip's decode is in flight (before live mixing engages),
  // so the UI can show a "preparing" state instead of silently-inert controls.
  // Distinct from `!active`, which also covers the no-stems / decode-failed
  // fallback where the native <video> audio is the (final) answer, not a wait.
  const [decoding, setDecoding] = React.useState(false);
  // Stem indices whose RNNoise preview buffer is currently being computed (wasm
  // load + the per-frame pass). Drives the per-stem "denoising…" spinner so the
  // first noise-cancel toggle reads as working, not frozen.
  const [denoisingIdx, setDenoisingIdx] = React.useState<number[]>([]);

  const graphRef = React.useRef<Graph | null>(null);
  const sourcesRef = React.useRef<AudioBufferSourceNode[]>([]);
  const anchorRef = React.useRef<Anchor | null>(null);
  const rafRef = React.useRef<number | null>(null);
  // Denoise (RNNoise) preview: the set of stem indices currently noise-cancelled,
  // plus a cache of each stem's denoised buffer (computed lazily, reused across
  // toggles). `resync` reads these to pick the raw vs cleaned buffer per stem, so
  // toggling denoise just re-points the sources — no re-decode, no re-IPC.
  const denoiseSetRef = React.useRef<Set<number>>(new Set());
  const denoisedRef = React.useRef<Map<number, AudioBuffer>>(new Map());
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
  // Stable key for the denoise selection, so the denoise effect fires only when
  // the set of cancelled stems changes (order-independent).
  const denoiseKey = [...denoiseStemIdx].sort((a, b) => a - b).join(",");

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
      for (const [idx, rawBuffer] of graph.buffers) {
        const gain = graph.gains.get(idx);
        // Use the cleaned buffer when this stem is noise-cancelled and its
        // denoised version is ready; otherwise fall back to the raw decode.
        const buffer =
          (denoiseSetRef.current.has(idx) && denoisedRef.current.get(idx)) || rawBuffer;
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
      // This effect is inherently side-effectful (async decode + audio graph),
      // so the no-stems reset can't move to render phase.
      /* eslint-disable react-hooks/set-state-in-effect */
      setActive(false);
      setDecoding(false);
      /* eslint-enable react-hooks/set-state-in-effect */
      return;
    }
    let cancelled = false;
    let input: Input | null = null;
    let ctx: AudioContext | null = null;
    setDecoding(true);

    (async () => {
      try {
        // `end` is exclusive; the backend clamps it to the file size.
        input = createStemInput(clipId, fileSize);
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

    // Capture the (stable, never-reassigned) denoise cache now so the cleanup
    // doesn't read `denoisedRef.current` after it may have changed.
    const denoised = denoisedRef.current;
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
      // Denoised buffers belong to the closing context — drop them so the next
      // clip recomputes against its own graph.
      denoised.clear();
      (built?.ctx ?? ctx)?.close().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clipId, fileSize, stemKey, hasStems]);

  // --- Denoise (RNNoise) preview: compute cleaned buffers + re-point sources. --
  // Lazily denoises each newly-selected stem (cached for reuse), tracks the live
  // selection in a ref `resync` reads, then re-anchors so the change is heard.
  // Decode is untouched: toggling denoise never re-reads or re-decodes the stem.
  React.useEffect(() => {
    // Indices come from the stable key, not the prop array — so a volume drag
    // (new array, same selection) doesn't re-run this and restart the sources.
    const indices = denoiseKey ? denoiseKey.split(",").map(Number) : [];
    denoiseSetRef.current = new Set(indices);
    const graph = graphRef.current;
    if (!graph || !active) {
      setDenoisingIdx([]);
      return;
    }
    let cancelled = false;

    // Stems that still need their cleaned buffer computed — the ones to spin on.
    const todo = indices.filter((idx) => !denoisedRef.current.has(idx) && graph.buffers.has(idx));
    setDenoisingIdx(todo);

    (async () => {
      for (const idx of indices) {
        if (denoisedRef.current.has(idx)) continue; // already cleaned
        const raw = graph.buffers.get(idx);
        if (!raw) continue;
        const cleaned = await denoiseAudioBufferForPreview(graph.ctx, raw);
        if (cancelled || graphRef.current !== graph) return; // clip changed
        denoisedRef.current.set(idx, cleaned);
        setDenoisingIdx((cur) => cur.filter((i) => i !== idx));
      }
      // Re-point the running sources at the right buffers (denoise on → cleaned,
      // off → raw). A no-op while paused; the next play resyncs from scratch.
      const v = videoRef.current;
      if (!cancelled && v && !v.paused) resync(v.currentTime);
    })();

    return () => {
      cancelled = true;
    };
  }, [denoiseKey, active, videoRef, resync]);

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
      ctx
        .resume()
        .then(() => resync(v.currentTime))
        .catch(() => {});
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

  return { active, decoding, denoisingIdx };
}
