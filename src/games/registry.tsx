import {
  Crosshair,
  Sword,
  SoccerBall,
  GameController,
  Target,
  Lightning,
  Airplane,
  Skull,
  type Icon,
} from "@phosphor-icons/react";

/**
 * Frontend game registry — the single source of truth for which games Hako's UI
 * knows about. Mirrors the Rust `GameId` enum + `registry()`
 * (src-tauri/src/games/mod.rs): adding a game is one `GAMES` entry here, plus its
 * logo asset, event labels, per-game settings slice, artwork hook
 * (`use-game-assets.ts`), and clip presenter (`clip-presenter.ts`).
 *
 * Everything game-aware in the UI — the clips Game filter, the per-clip artwork
 * hook (`useGameAssets`), the clip presenters that turn a clip into card pills /
 * detail fields, and the Auto-Capture cards — keys off this list rather than
 * hard-coding "valorant"/"lol".
 */

/** Stable lowercase id, matching the clip DB's `game` column + settings keys.
 * `other` is the generic "record any game" bucket (its settings + arbiter key);
 * individual generic clips store the *real* game title, not `other`. */
export type GameId =
  "valorant" | "lol" | "rematch" | "cs2" | "dota2" | "warthunder" | "pubg" | "other";

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
  /**
   * The game's own executable name(s), lowercased. Mirrors the backend
   * detection process names (src-tauri/src/{valorant,games/lol,games/rematch}).
   * The audio panel uses these to keep a game's own process out of the
   * "additional apps" list — it's already the dedicated "Game Audio" source.
   */
  processNames: string[];
}

/** Registry order = display order across the app. */
export const GAMES: GameMeta[] = [
  {
    id: "valorant",
    label: "Valorant",
    logo: "/games/valorant.svg",
    Icon: Crosshair,
    accent: "#FF4655",
    processNames: ["valorant-win64-shipping.exe"],
  },
  {
    id: "lol",
    label: "League of Legends",
    logo: "/games/lol.svg",
    Icon: Sword,
    accent: "#C89B3C",
    processNames: ["league of legends.exe"],
  },
  {
    id: "rematch",
    label: "Rematch",
    logo: "/games/rematch.svg",
    Icon: SoccerBall,
    accent: "#4F9D5B",
    processNames: ["runtimeclient-win64-shipping.exe", "runtimeclient-wingdk-shipping.exe"],
  },
  {
    id: "cs2",
    label: "Counter-Strike 2",
    logo: "/games/cs2.svg",
    Icon: Target,
    accent: "#E9A13B",
    processNames: ["cs2.exe"],
  },
  {
    id: "dota2",
    label: "Dota 2",
    logo: "/games/dota2.svg",
    Icon: Lightning,
    accent: "#C23C2A",
    processNames: ["dota2.exe"],
  },
  {
    id: "warthunder",
    label: "War Thunder",
    logo: "/games/warthunder.svg",
    Icon: Airplane,
    accent: "#6E8B3D",
    processNames: ["aces.exe"],
  },
  {
    id: "pubg",
    label: "PUBG",
    logo: "/games/pubg.svg",
    Icon: Skull,
    accent: "#CA9A3C",
    processNames: ["tslgame.exe"],
  },
  // The generic "record any game" bucket — kept LAST (mirrors the backend
  // registry order). Detected games are added via the picker / auto-scan; this
  // entry styles the Auto-Capture "Other Games" card, not per-game detection.
  {
    id: "other",
    label: "Other Games",
    logo: "/games/_default.svg",
    Icon: GameController,
    accent: "#8b8b8b",
    processNames: [],
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
  processNames: [],
};

/** Registry entry for an id (fallback for ids not in the registry). */
export function gameMeta(id: GameId): GameMeta {
  return BY_ID.get(id) ?? FALLBACK;
}

/**
 * The `GameId` bucket a clip belongs to. Clips predating multi-game support
 * stored `null` and are treated as Valorant (the only game then; the backfill
 * also labels them "valorant"). Generic "record any game" clips store their
 * **real title** (e.g. "Elden Ring"), which is none of the smart ids → they
 * bucket under `"other"` so they filter + present under "Other Games" instead of
 * masquerading as Valorant.
 */
export function clipGame(game: string | null | undefined): GameId {
  if (!game) return "valorant"; // legacy null (backfilled to Valorant)
  // Any id the registry knows maps to itself; a real game title (generic clips
  // store the actual title, e.g. "Elden Ring") isn't a registered id → "other".
  return BY_ID.has(game as GameId) ? (game as GameId) : "other";
}
