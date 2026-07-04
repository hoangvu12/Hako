import { invoke } from "@tauri-apps/api/core";

/**
 * Mirrors the Rust `ClipRecord` (src-tauri/src/library/db.rs). Also the payload
 * of the `clip-created` event.
 */
/** One event's position inside a clip (mirrors Rust `EventMark`). */
export interface EventMark {
  /** EventKind label, e.g. "Kill", "Ace", "Spike Defused". */
  label: string;
  /** Seconds from the clip's start where the event happened. */
  at: number;
}

export interface ClipRecord {
  id: number;
  path: string;
  title: string;
  /** Headline event (the dominant one when a clip's window merged several). */
  event: string | null;
  /** Every event captured in the clip's window, in time order. Falls back to
   * `[event]` for clips saved before multi-event tracking existed. */
  events: string[];
  /** Per-event positions within the clip (label + offset seconds), for the
   * editor's seek-bar markers. Empty for manual saves and for clips cut before
   * positions were persisted. */
  event_marks: EventMark[];
  duration_secs: number;
  width: number;
  height: number;
  size_bytes: number;
  thumb_path: string | null;
  /** Sprite-sheet filmstrip (one JPEG, N tiles) for the editor scrubber. */
  filmstrip_path: string | null;
  created_unix_ms: number;

  // --- Valorant game context (all nullable) -----------------------------
  // Filled for clips cut from a match: auto-clips carry everything; manual F9
  // saves carry agent/map/mode (win + K/D/A are unknowable mid-match). All null
  // for clips saved outside a match and for clips predating this metadata.
  /** Agent display name (e.g. "Jett"). */
  agent: string | null;
  /** Agent UUID (`characterId`) — pairs with `agent` for artwork lookup. */
  agent_id: string | null;
  /** Map asset path (e.g. "/Game/Maps/Ascent/Ascent"); prettify for display. */
  map: string | null;
  /** Game-mode display name (e.g. "Competitive", "Standard"). */
  mode: string | null;
  /** Match result when known (auto-clips): true = win, false = loss. */
  won: boolean | null;
  /** Match K/D/A totals (auto-clips only). */
  kills: number | null;
  deaths: number | null;
  assists: number | null;
  /** Headshot % over recorded damage, 0–100 (auto-clips only). */
  headshot_pct: number | null;
  /** Source game: "valorant" | "lol". Null on clips predating multi-game
   * support (treated as Valorant). For League, `agent` holds the champion name,
   * `map`/`mode` the map + queue, and `headshot_pct` is unused. */
  game: string | null;

  /** True once cloud retention deleted the local files. The clip is now
   * cloud-only — `path`/`thumb_path` no longer point at real files, so playback
   * falls back to the provider's presigned `remote_url`. */
  evicted: boolean;
}

/**
 * Save the last `seconds` (default 30) of buffered gameplay to an MP4 via
 * stream-copy, record it in the library, and return the new clip. Also fires the
 * `clip-created` event. The global hotkey **F9** triggers the same save for 30s.
 */
export async function saveClip(seconds?: number): Promise<ClipRecord> {
  return invoke<ClipRecord>("save_clip", { seconds });
}

/** All clips in the library, newest first. */
export async function clipsList(): Promise<ClipRecord[]> {
  return invoke<ClipRecord[]>("clips_list");
}

/** Delete a clip (row + file + thumbnail). */
export async function deleteClip(id: number): Promise<void> {
  await invoke("delete_clip", { id });
}

/** Rename a clip's title. */
export async function renameClip(id: number, title: string): Promise<void> {
  await invoke("rename_clip", { id, title });
}

/** Reveal a clip's file in the OS file manager (Explorer), selecting it. */
export async function revealClip(id: number): Promise<void> {
  await invoke("reveal_clip", { id });
}

/** Where a trim writes its result. */
export type TrimMode = "overwrite" | "copy";

/**
 * Loss-lessly trim a clip to `[start, end)` seconds (stream copy, optionally
 * dropping audio). `"copy"` creates a new library clip; `"overwrite"` replaces
 * the original file in place. Returns the resulting record.
 */
export async function trimClip(args: {
  id: number;
  start: number;
  end: number;
  dropAudio: boolean;
  mode: TrimMode;
}): Promise<ClipRecord> {
  return invoke<ClipRecord>("trim_clip", {
    id: args.id,
    start: args.start,
    end: args.end,
    dropAudio: args.dropAudio,
    mode: args.mode,
  });
}

/**
 * One of a clip's audio tracks (mirrors Rust `library::remux::AudioTrackInfo`).
 * `index` is the 0-based position among the file's audio streams — track 0 is
 * the master "All Audio" mix; 1..N are the stems ("Microphone", per-app, …).
 */
export interface AudioTrackInfo {
  index: number;
  name: string;
}

/** The audio tracks in a clip (count + names) for the editor's per-track UI. */
export async function clipAudioTracks(id: number): Promise<AudioTrackInfo[]> {
  return invoke<AudioTrackInfo[]>("clip_audio_tracks", { id });
}

/**
 * Read a byte range `[start, end)` of a clip file as an `ArrayBuffer`. Backs the
 * editor's live per-stem mixer: mediabunny decodes the stems in the webview via
 * a `CustomSource` that pulls bytes over IPC, because it can't `fetch()` the
 * `hakoclip://` streaming scheme (WebView2 blocks cross-scheme fetch by CORS;
 * the `<video>` element is exempt). `end` is clamped to the file size in Rust.
 */
export async function readClipRange(id: number, start: number, end: number): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("read_clip_range", { id, start, end });
}

/**
 * A stem selected for export (mirrors Rust `TrackVolume`): its 0–100 volume and
 * whether to apply offline noise suppression (the mic stem's "noise cancel").
 */
export interface TrackVolume {
  index: number;
  volume: number;
  /** Run RNNoise noise suppression on this stem when re-mixing the export. */
  denoise?: boolean;
}

/**
 * Export a clip to `[start, end)` with its audio being the chosen `tracks`
 * (stems) mixed at their volumes — the editor's per-track mute/solo/volume,
 * applied on export. Empty `tracks` ⇒ video-only; one stem at 100% ⇒ a
 * loss-less stream copy; otherwise the stems are decoded, mixed, and re-encoded
 * to one master track. `"copy"` adds a new clip; `"overwrite"` replaces it.
 */
export async function remuxWithTracks(args: {
  id: number;
  start: number;
  end: number;
  tracks: TrackVolume[];
  mode: TrimMode;
}): Promise<ClipRecord> {
  return invoke<ClipRecord>("remux_with_tracks", {
    id: args.id,
    start: args.start,
    end: args.end,
    tracks: args.tracks,
    mode: args.mode,
  });
}
