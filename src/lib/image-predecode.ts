// Off-DOM thumbnail pre-decoder for the clips grid.
//
// The browser decodes a thumbnail the moment its <img> scrolls into view; during
// a fast flick that decode lands on the frame's critical path and drops FPS
// (the compositor waits on raster/GPU upload). By decoding the thumbnails for
// rows just *beyond* the rendered window ahead of time — on throwaway Image
// objects, in a small concurrency-limited queue — the bitmap is already in
// Chromium's decode cache when the real <img> mounts, so it paints from a warm
// tile instead of blocking. This is the same "decode ahead of the viewport"
// technique Immich / Google Photos use for their timeline grids.
//
// We force this off-DOM because the grid cards use `content-visibility: auto`,
// which otherwise *defers* decode for off-screen (overscan) rows until they're
// nearly visible — exactly when we don't want the work to happen.

const decoded = new Set<string>();
const inflight = new Set<string>();
const queue: string[] = [];
let active = 0;

// Keep the parallel decode count small: too many contend with each other (and
// with the main thread); too few can't keep up with a fast scroll. 3 is the
// sweet spot the research converges on.
const MAX_CONCURRENT = 3;

function pump(): void {
  while (active < MAX_CONCURRENT && queue.length > 0) {
    const src = queue.shift();
    if (src === undefined) break;
    active++;
    const img = new Image();
    img.decoding = "async";
    img.src = src;
    // decode() resolves once the bitmap is ready to paint; the decoded data
    // stays in the browser's image cache keyed by URL, so this throwaway element
    // can be GC'd — the real <img> that mounts later reuses the cached decode.
    img
      .decode()
      .then(() => {
        decoded.add(src);
      })
      .catch(() => {
        // Missing/corrupt thumb — don't retry, just stop tracking it.
      })
      .finally(() => {
        inflight.delete(src);
        active--;
        pump();
      });
  }
}

/** Schedule `src` for off-screen decode. No-op if already decoded or queued. */
export function predecodeImage(src: string): void {
  if (!src || decoded.has(src) || inflight.has(src)) return;
  inflight.add(src);
  queue.push(src);
  pump();
}
