import {
  Crosshair,
  Sword,
  SoccerBall,
  GameController,
  type Icon,
} from "@phosphor-icons/react";

/**
 * Frontend game registry — the single source of truth for which games Hako's UI
 * knows about. Mirrors the Rust `GameId` enum + `registry()`
 * (src-tauri/src/games/mod.rs): adding a game is one `GAMES` entry here (+ its
 * logo asset, event labels, and per-game settings slice).
 *
 * Everything game-aware in the UI — the clips Game filter, the per-clip artwork
 * resolver (`useGameAssets`), and the Auto-Capture cards — iterates this list
 * rather than hard-coding "valorant"/"lol".
 */

/** Stable lowercase id, matching the clip DB's `game` column + settings keys. */
export type GameId = "valorant" | "lol" | "rematch";

export interface GameMeta {
  id: GameId;
  /** Human label for UI copy. */
  label: string;
  /** Bundled brand logo (public/games/<id>.svg). */
  logo: string;
  /** Phosphor glyph fallback, shown if the logo image fails to load. */
  Icon: Icon;
  /** Brand accent (hex), for subtle card/chip tinting. */
  accent: string;
}

/** Registry order = display order across the app. */
export const GAMES: GameMeta[] = [
  {
    id: "valorant",
    label: "Valorant",
    logo: "/games/valorant.svg",
    Icon: Crosshair,
    accent: "#FF4655",
  },
  {
    id: "lol",
    label: "League of Legends",
    logo: "/games/lol.svg",
    Icon: Sword,
    accent: "#C89B3C",
  },
  {
    id: "rematch",
    label: "Rematch",
    logo: "/games/rematch.svg",
    Icon: SoccerBall,
    accent: "#4F9D5B",
  },
];

const BY_ID = new Map(GAMES.map((g) => [g.id, g]));

/** Generic fallback for an unknown / not-yet-registered game. */
const FALLBACK: GameMeta = {
  id: "valorant",
  label: "Unknown",
  logo: "/games/_default.svg",
  Icon: GameController,
  accent: "#8b8b8b",
};

/** Registry entry for an id (fallback for ids not in the registry). */
export function gameMeta(id: GameId): GameMeta {
  return BY_ID.get(id) ?? FALLBACK;
}

/**
 * The `GameId` a clip belongs to. Clips predating multi-game support stored
 * `null` and are treated as Valorant; unknown ids also fall back to Valorant so
 * the UI never drops a clip.
 */
export function clipGame(game: string | null | undefined): GameId {
  if (game === "lol") return "lol";
  if (game === "rematch") return "rematch";
  return "valorant";
}
