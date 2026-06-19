import type { DenoiseState, Rnnoise } from "@shiguredo/rnnoise-wasm";

/**
 * Live mic-noise preview for the clip editor (RNNoise via WebAssembly).
 *
 * This is the *preview* half of the editor's "noise cancel". The export runs the
 * same RNNoise model in Rust (`core::denoise`, via `nnnoiseless`); here we run
 * RNNoise in the browser over the already-decoded stem `AudioBuffer`, so the user
 * *hears* "this mic gets cleaned up" while scrubbing — and it matches what gets
 * saved (same model both sides).
 *
 * The wasm (~4.8 MB, base64-inlined) is **dynamically imported** the first time
 * denoise is actually switched on, so it never weighs down the editor's initial
 * load. All failures degrade to the raw buffer — preview noise cancel must never
 * cost the user their monitor audio.
 */

/** RNNoise is trained for 48 kHz only; Hako's stems are always 48 kHz. */
const RNNOISE_RATE = 48_000;
/** RNNoise expects 16-bit-PCM-scaled samples, not [-1, 1] floats. */
const INT16_SCALE = 32_768;
/**
 * Max wall-clock time to spend processing before yielding to the event loop.
 * RNNoise runs on the main thread (the wasm has no worker build here), so
 * denoising a whole stem in one synchronous pass froze the UI for the length of
 * the clip. Yielding on a *time budget* (not a fixed frame count) keeps every
 * work slice well under one display frame regardless of clip length / CPU speed,
 * so the editor stays at full fps and its "denoising…" spinner keeps spinning.
 * `DenoiseState` is recurrent, but it lives across the yields — only the event
 * loop is released. ~5 ms leaves headroom in a 16 ms frame for paint + input.
 */
const SLICE_BUDGET_MS = 5;

/**
 * Yield to the event loop (lets paint + input run), then resume. Uses a
 * `MessageChannel` macrotask rather than `setTimeout(0)` — the latter is clamped
 * to ~4 ms per call, which would dominate the runtime when yielding this often.
 * A single reused channel avoids per-yield allocation.
 */
function makeYieldToMain(): () => Promise<void> {
  const ch = new MessageChannel();
  let resolve: (() => void) | null = null;
  ch.port1.onmessage = () => {
    const r = resolve;
    resolve = null;
    r?.();
  };
  return () =>
    new Promise<void>((res) => {
      resolve = res;
      ch.port2.postMessage(null);
    });
}

let rnnoisePromise: Promise<Rnnoise> | null = null;

/** Lazy-load (and cache) the RNNoise wasm module on first use. */
function loadRnnoise(): Promise<Rnnoise> {
  return (rnnoisePromise ??= import("@shiguredo/rnnoise-wasm").then((m) =>
    m.Rnnoise.load(),
  ));
}

/**
 * Return a noise-suppressed copy of `buffer` for live preview. Downmixes to
 * mono, runs RNNoise at 48 kHz, and fans the cleaned mono back across the
 * original channel count (matching the export, which also fans mono to stereo).
 *
 * Returns the **original** buffer untouched if RNNoise can't load, the rate
 * isn't 48 kHz, or processing throws — a best-effort monitor, never silence.
 */
export async function denoiseAudioBufferForPreview(
  ctx: BaseAudioContext,
  buffer: AudioBuffer,
): Promise<AudioBuffer> {
  // RNNoise is 48 kHz-only; resampling here would add cost and artifacts for a
  // case Hako never hits (stems are captured/encoded at 48 kHz). Skip cleanly.
  if (buffer.sampleRate !== RNNOISE_RATE) return buffer;

  let rnnoise: Rnnoise;
  try {
    rnnoise = await loadRnnoise();
  } catch (e) {
    console.warn("[denoise-preview] RNNoise load failed; previewing raw mic", e);
    return buffer;
  }

  let state: DenoiseState | null = null;
  try {
    state = rnnoise.createDenoiseState();

    const len = buffer.length;
    const chCount = buffer.numberOfChannels;

    // Downmix to mono (mic is mono in practice; matches the DF3 export path).
    const mono = new Float32Array(len);
    for (let ch = 0; ch < chCount; ch++) {
      const data = buffer.getChannelData(ch);
      for (let i = 0; i < len; i++) mono[i] += data[i];
    }
    if (chCount > 1) for (let i = 0; i < len; i++) mono[i] /= chCount;

    // Process sequentially in fixed frames — RNNoise is recurrent, so one
    // `DenoiseState` must see the whole stem in order. `processFrame` works in
    // place and assumes int16-scaled input, so we scale in and back out.
    const frameSize = rnnoise.frameSize; // 480 @ 48 kHz (10 ms)
    const frame = new Float32Array(frameSize);
    const out = new Float32Array(len);
    const yieldToMain = makeYieldToMain();
    let sliceStart = performance.now();
    for (let off = 0; off < len; off += frameSize) {
      const n = Math.min(frameSize, len - off);
      for (let i = 0; i < frameSize; i++) {
        frame[i] = i < n ? mono[off + i] * INT16_SCALE : 0; // zero-pad the tail
      }
      state.processFrame(frame);
      for (let i = 0; i < n; i++) out[off + i] = frame[i] / INT16_SCALE;
      // Hand the main thread back once this slice has run long enough, so a
      // multi-second stem never blocks paint/input for more than ~one frame at a
      // time (the "few fps" stutter this fixes). Time-based, not frame-count
      // based, so it self-tunes to the clip length and the machine's speed.
      if (performance.now() - sliceStart >= SLICE_BUDGET_MS) {
        await yieldToMain();
        sliceStart = performance.now();
      }
    }

    // Fan the cleaned mono back to every channel of a fresh buffer.
    const result = ctx.createBuffer(chCount, len, buffer.sampleRate);
    for (let ch = 0; ch < chCount; ch++) result.getChannelData(ch).set(out);
    return result;
  } catch (e) {
    console.warn("[denoise-preview] RNNoise processing failed; previewing raw mic", e);
    return buffer;
  } finally {
    state?.destroy();
  }
}
