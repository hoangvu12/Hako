/** Lead time when (re)starting buffer sources, so `start()` isn't in the past. */
export const START_LOOKAHEAD = 0.03;
/** Resync once |audio − video| exceeds this (seconds). Above human-perceptible. */
export const DRIFT_MAX = 0.05;
/** Gain ramp to dodge zipper clicks on mute/volume changes (seconds). */
export const GAIN_RAMP = 0.012;
