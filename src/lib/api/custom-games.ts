import { invoke } from "@tauri-apps/api/core";

/**
 * "Record any game" custom-list bindings (mirrors the Rust `CustomGame` +
 * custom-game commands in src-tauri/src/commands.rs). A custom game is added by
 * pointing the picker at its window; from then on the generic integration
 * auto-records it whenever it's running.
 */

/** Mirrors the Rust `CustomGame` (src-tauri/src/library/db.rs). */
export interface CustomGame {
  id: number;
  /** Exe file name, lowercase (the match key). */
  process_name: string;
  /** Real game title, shown in the UI + stored on clips. */
  display_name: string;
  window_class: string | null;
  caption: string | null;
  enabled: boolean;
  /** Unix millis when it was added. */
  added_at: number;
  /** The exe's icon as a PNG `data:` URL, captured at add time; null if none. */
  icon: string | null;
}

/** The user-added custom games, newest first. */
export async function listCustomGames(): Promise<CustomGame[]> {
  return invoke<CustomGame[]>("list_custom_games");
}

/**
 * Add the game owning window `hwnd` to the custom list (Request-a-Game): the
 * backend resolves its exe + title, stores them, and returns the row. Rejects
 * smart games and non-games (browsers/launchers).
 */
export async function addCustomGame(hwnd: number): Promise<CustomGame> {
  return invoke<CustomGame>("add_custom_game", { hwnd });
}

/** Remove a custom game (stops auto-recording it). */
export async function removeCustomGame(id: number): Promise<void> {
  await invoke("remove_custom_game", { id });
}

/** Enable/disable a custom game (kept in the list either way). */
export async function setCustomGameEnabled(id: number, enabled: boolean): Promise<void> {
  await invoke("set_custom_game_enabled", { id, enabled });
}
