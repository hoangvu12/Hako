import * as React from "react";

// Grid metrics (kept in sync with the row layout). `GAP` mirrors the `gap-3`;
// `MIN_CARD` is the smallest comfortable card width before we drop a column.
// `EST_HEADER_ROW` seeds header scroll math.
export const GAP = 12;
export const MIN_CARD = 340;
export const EST_CLIP_ROW = 280; // fallback until the container width is known
export const EST_HEADER_ROW = 44;
// Fixed chrome below each card's aspect-video thumbnail: border (2) + meta block
// (~76: p-3.5 padding + single-line title row + meta line) + the row's bottom
// padding (pb-3 = 12). This is constant regardless of width — the title is
// truncated and the meta is one line — so a clip row's height is fully
// determined by the card width. That lets us size rows mathematically and skip
// per-row measurement (no ResizeObserver layout reads on the scroll path).
export const CARD_CHROME = 90;
// How many rows beyond the rendered (visible + overscan) window to pre-decode
// thumbnails for, in each scroll direction. Wide enough to stay ahead of a fast
// flick without flooding the decode queue.
export const DECODE_AHEAD_ROWS = 6;

/**
 * Responsive column count *and* content width of the scroll container (so the
 * grid tracks the panel, not the window). One ResizeObserver feeds both; the
 * width drives deterministic fixed row heights in the page.
 */
export function useGridMetrics(ref: React.RefObject<HTMLElement | null>): {
  columns: number;
  width: number;
} {
  const [metrics, setMetrics] = React.useState({ columns: 1, width: 0 });
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const compute = (w: number) => Math.max(1, Math.floor((w + GAP) / (MIN_CARD + GAP)));
    const apply = (w: number) =>
      setMetrics((m) => {
        const columns = compute(w);
        return m.columns === columns && m.width === w ? m : { columns, width: w };
      });
    // Measure synchronously here, before paint, so the first frame already has
    // the right column count. The ResizeObserver's initial callback is delivered
    // *after* paint, so relying on it alone flashes a single column on mount
    // (e.g. returning from the clip detail). `clientWidth` minus horizontal
    // padding matches the content-box width the observer reports below.
    const style = getComputedStyle(el);
    const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    apply(el.clientWidth - padX);
    const ro = new ResizeObserver((entries) => {
      apply(entries[0]?.contentRect.width ?? 0);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return metrics;
}
