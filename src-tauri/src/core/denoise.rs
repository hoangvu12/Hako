//! Offline microphone noise suppression (RNNoise, via the pure-Rust
//! `nnnoiseless`).
//!
//! This is the editor's "noise cancel" — applied to a clip's **mic stem** when
//! it's re-mixed on export (`library::remux`), never in the capture hot path.
//! It runs the same model (RNNoise) as the editor's live WASM preview
//! (`src/lib/denoise-preview.ts`), so **what you hear in the editor is what gets
//! saved** — true preview↔export parity.
//!
//! `nnnoiseless` is a pure-Rust port: no FFI, no model file to ship, no
//! JIT/codegen runtime. (We first integrated DeepFilterNet 3 via `deep_filter` +
//! `tract` for its higher quality, but tract failed to build the model graph at
//! runtime — "running pass codegen" — under debug builds, i.e. `tauri dev`, so
//! the feature silently no-op'd. RNNoise here just works in dev and release.)
//!
//! The export mixer hands us **interleaved 48 kHz stereo f32** (a decoded stem,
//! see `remux::StemDecoder`). RNNoise is a mono 48 kHz model, and mic capture is
//! mono in all but pathological setups, so we downmix → enhance → fan the
//! cleaned mono back out to both channels.

#![allow(dead_code)]

use nnnoiseless::DenoiseState;

/// RNNoise operates on int16-scaled samples, not [-1, 1] floats.
const INT16_SCALE: f32 = 32_768.0;

/// Denoise an **interleaved 48 kHz stereo** f32 buffer in place (mic noise
/// suppression). Downmixes to mono, runs RNNoise, and fans the cleaned mono back
/// to both channels.
pub fn denoise_interleaved_stereo_48k(samples: &mut [f32]) {
    let frames = samples.len() / 2;
    if frames == 0 {
        return;
    }
    // Downmix L/R → mono (mic is mono in practice; this is the standard speech
    // path and matches the single-channel RNNoise model).
    let mut mono = Vec::with_capacity(frames);
    for i in 0..frames {
        mono.push(0.5 * (samples[i * 2] + samples[i * 2 + 1]));
    }

    denoise_mono_48k(&mut mono);

    // Fan the cleaned mono back out to both channels.
    for i in 0..frames {
        let v = mono[i];
        samples[i * 2] = v;
        samples[i * 2 + 1] = v;
    }
}

/// Enhance a mono 48 kHz f32 buffer in place. RNNoise is causal and frame-aligned
/// (no latency to compensate), so the output stays in sync with the original.
fn denoise_mono_48k(mono: &mut [f32]) {
    // RNNoise processes a fixed number of samples per call (480 @ 48 kHz = 10 ms).
    const N: usize = DenoiseState::FRAME_SIZE;
    let mut denoise = DenoiseState::new();
    let mut in_buf = [0f32; N];
    let mut out_buf = [0f32; N];

    let len = mono.len();
    let mut off = 0;
    while off < len {
        let n = (len - off).min(N);
        // Fill a full frame (zero-pad the tail) and scale to the int16 range
        // RNNoise expects.
        for i in 0..N {
            in_buf[i] = if i < n {
                mono[off + i] * INT16_SCALE
            } else {
                0.0
            };
        }
        denoise.process_frame(&mut out_buf, &in_buf);
        for i in 0..n {
            mono[off + i] = out_buf[i] / INT16_SCALE;
        }
        off += N;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real model on non-silent audio and asserts it actually changes
    /// the signal (a no-op would mean the denoiser silently failed — which is how
    /// the earlier DeepFilterNet/tract codegen failure slipped past a test that
    /// only fed silence).
    #[test]
    fn denoises_white_noise() {
        // 1 s of mono white noise at 48 kHz (cheap LCG, no rand dep).
        let n = 48_000usize;
        let mut seed = 0x1234_5678u32;
        let mut mono = vec![0f32; n];
        for s in mono.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0; // ~[-1, 1)
        }
        let before = mono.clone();
        denoise_mono_48k(&mut mono);
        assert!(
            before.iter().zip(&mono).any(|(a, b)| (a - b).abs() > 1e-6),
            "denoise was a no-op — model didn't actually process",
        );
    }
}
