import * as React from "react";

/**
 * Generates a strip of evenly-spaced thumbnail frames from a video by decoding it
 * offscreen and drawing seeked frames to a canvas — the "filmstrip" under the
 * scrubber (Medal-style). Pure client-side, no backend.
 *
 * Canvas `toDataURL` throws a SecurityError if the source frame taints the canvas
 * (some asset-protocol configs do), so the whole thing degrades to `unsupported`
 * and the caller falls back to a plain track.
 */
export type FilmstripState =
  | { status: "loading" }
  | { status: "ready"; frames: string[] }
  | { status: "unsupported" };

export function useFilmstrip(
  src: string,
  durationHint: number,
  count = 16,
): FilmstripState {
  const [state, setState] = React.useState<FilmstripState>({
    status: "loading",
  });

  React.useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });

    const video = document.createElement("video");
    video.src = src;
    video.muted = true;
    video.preload = "auto";
    video.crossOrigin = "anonymous";
    (video as HTMLVideoElement).playsInline = true;

    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");

    async function run() {
      try {
        if (!ctx) throw new Error("no 2d context");
        await waitFor(video, "loadeddata", 10_000, true);
        if (cancelled) return;

        const duration =
          Number.isFinite(video.duration) && video.duration > 0
            ? video.duration
            : durationHint;
        const w = 160;
        const h =
          video.videoWidth > 0 && video.videoHeight > 0
            ? Math.round((video.videoHeight / video.videoWidth) * w)
            : 90;
        canvas.width = w;
        canvas.height = h;

        const frames: string[] = [];
        for (let i = 0; i < count; i++) {
          if (cancelled) return;
          const t = (duration * (i + 0.5)) / count;
          video.currentTime = Math.min(Math.max(t, 0), Math.max(0, duration - 0.05));
          // Resolve on seek; never hang if the browser skips the event.
          await waitFor(video, "seeked", 2500, false);
          if (cancelled) return;
          ctx.drawImage(video, 0, 0, w, h);
          frames.push(canvas.toDataURL("image/jpeg", 0.6));
        }
        if (!cancelled) setState({ status: "ready", frames });
      } catch {
        if (!cancelled) setState({ status: "unsupported" });
      }
    }
    void run();

    return () => {
      cancelled = true;
      video.removeAttribute("src");
      video.load();
    };
  }, [src, durationHint, count]);

  return state;
}

/**
 * Resolve when `event` fires (or after `timeout`). When `rejectOnTimeout` we
 * surface a failure instead of silently continuing — used for the initial decode.
 */
function waitFor(
  el: HTMLMediaElement,
  event: string,
  timeout: number,
  rejectOnTimeout: boolean,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const done = (fn: () => void) => {
      clearTimeout(timer);
      el.removeEventListener(event, ok);
      el.removeEventListener("error", err);
      fn();
    };
    const ok = () => done(resolve);
    const err = () => done(() => reject(new Error(`${event} error`)));
    const timer = setTimeout(
      () => done(rejectOnTimeout ? () => reject(new Error("timeout")) : resolve),
      timeout,
    );
    el.addEventListener(event, ok, { once: true });
    el.addEventListener("error", err, { once: true });
  });
}
