import type { ClipRecord } from "@/lib/api";

/** A flattened virtual row: either a date header or a row of up to `columns`
 * clips. Flattening lets one virtualizer drive the whole album view. */
export type VirtualRow =
  | { type: "header"; key: string; label: string; count: number }
  | { type: "clips"; key: string; clips: ClipRecord[] };

export function flattenSections(
  sections: { key: string; label: string; clips: ClipRecord[] }[],
  columns: number,
): VirtualRow[] {
  const rows: VirtualRow[] = [];
  for (const sec of sections) {
    rows.push({
      type: "header",
      key: `h:${sec.key}`,
      label: sec.label,
      count: sec.clips.length,
    });
    for (let i = 0; i < sec.clips.length; i += columns) {
      rows.push({
        type: "clips",
        key: `${sec.key}:${i}`,
        clips: sec.clips.slice(i, i + columns),
      });
    }
  }
  return rows;
}
