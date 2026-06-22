import {
  ALL_FORMATS,
  AudioBufferSink,
  CustomSource,
  Input,
  type InputAudioTrack,
} from "mediabunny";

import { readClipRange } from "@/lib/api";

/**
 * Open a clip's container for stem decoding. mediabunny reads bytes over IPC
 * (`readClipRange`) because it can't `fetch()` the `hakoclip://` scheme — WebView2
 * blocks cross-scheme fetch by CORS.
 */
export function createStemInput(clipId: number, fileSize: number): Input {
  return new Input({
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
}

/**
 * Decode one stem track fully into a single `AudioBuffer` on `ctx`, preserving
 * gaps (each chunk is placed at its own timestamp). Returns null for an empty
 * track.
 */
export async function decodeStem(
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
