import * as React from "react";

/**
 * Subscribe to a <video>'s playback position. Returns the current time, updated
 * from the element's own `timeupdate` (during playback) and `seeking`/`seeked`
 * (during scrubbing). Keeping this in small leaf components — instead of one
 * `current` state on `ViewerStage` — means the heavy editor (filmstrip, ruler,
 * details panel) no longer re-renders ~10×/sec while a clip plays; only the
 * playhead, the time readout, and the overlay seek bar do.
 */
export function useVideoTime(videoRef: React.RefObject<HTMLVideoElement | null>): number {
  const [time, setTime] = React.useState(0);
  React.useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const sync = () => setTime(v.currentTime);
    sync();
    v.addEventListener("timeupdate", sync);
    v.addEventListener("seeking", sync);
    v.addEventListener("seeked", sync);
    return () => {
      v.removeEventListener("timeupdate", sync);
      v.removeEventListener("seeking", sync);
      v.removeEventListener("seeked", sync);
    };
  }, [videoRef]);
  return time;
}
