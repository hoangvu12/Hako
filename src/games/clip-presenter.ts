import { mapNameFromPath } from "@/hooks/use-valorant-assets";
import { friendlyLolMap } from "@/hooks/use-lol-assets";
import type { ClipRecord } from "@/lib/api";
import type { GameAssets } from "./use-game-assets";
import { clipGame, type GameId } from "./registry";

/**
 * Per-game clip presentation — the single place each game decides what its clips
 * surface in the UI. The clip card and the viewer's details panel both render
 * from a game's presenter, so adding a game is one `GAME_PRESENTERS` entry rather
 * than a new `if (clip.game === …)` branch scattered across components.
 *
 * Games differ in what they have: Valorant/League carry an agent/champion
 * portrait + map + K/D/A; Rematch is a soccer game with a stadium and goals but
 * no portrait or K/D/A. A presenter returns only what its game actually has, and
 * the renderers degrade gracefully around the gaps.
 */

/** One pill on the clip card's thumbnail overlay. */
export interface ClipBadge {
  /** Pill text. */
  label: string;
  /** Leading portrait (agent/champion), when the game has one and it resolved. */
  icon?: string;
  /**
   * Render the portrait-pill style — a leading round icon slot (kept even when
   * `icon` is missing, so the name still aligns) and a heavier weight. Plain text
   * pill when false/omitted.
   */
  portrait?: boolean;
}

/** The match-context block shown in the viewer's details panel. */
export interface ClipDetail {
  /** Large portrait (agent/champion), if the game has one. */
  icon?: string;
  /** Resolved headline (agent/champion/player), or null when unknown. */
  name: string | null;
  /** Shown in place of `name` when it's unknown (e.g. "Unknown agent"). */
  fallback: string;
  /** Secondary line under the headline ("Ascent · Competitive"). */
  sub: string;
  /** Whether the K/D/A + headshot row applies to this game. */
  showKda: boolean;
}

/** What every game must provide to render its clips. */
export interface GamePresenter {
  /** Pills on the clip card, in display order (empty ⇒ no overlay). */
  cardBadges(clip: ClipRecord, assets: GameAssets): ClipBadge[];
  /** Details-panel match context. */
  detail(clip: ClipRecord, assets: GameAssets): ClipDetail;
}

/** Drop empty pills so a missing field never renders a blank chip. */
function badges(...items: (ClipBadge | false | null | undefined)[]): ClipBadge[] {
  return items.filter((b): b is ClipBadge => Boolean(b && b.label));
}

const valorant: GamePresenter = {
  cardBadges(clip, a) {
    const agent = a.valorant.agentFor(clip);
    const name = agent?.name ?? clip.agent ?? "";
    const map = a.valorant.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map);
    return badges(
      { label: name, icon: agent?.icon, portrait: true },
      { label: map }
    );
  },
  detail(clip, a) {
    const agent = a.valorant.agentFor(clip);
    const map = a.valorant.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map);
    return {
      icon: agent?.icon,
      name: agent?.name ?? clip.agent ?? null,
      fallback: "Unknown agent",
      sub: [map, clip.mode].filter(Boolean).join(" · "),
      showKda: true,
    };
  },
};

const lol: GamePresenter = {
  cardBadges(clip, a) {
    const champ = a.lol.champFor(clip.agent);
    // League's map is fixed by the queue, so the mode ("ARAM") reads better than
    // the map; fall back to the (prettified) map when the mode is unknown.
    const mode = clip.mode || friendlyLolMap(clip.map);
    return badges(
      { label: clip.agent ?? "", icon: champ?.icon, portrait: true },
      { label: mode }
    );
  },
  detail(clip, a) {
    return {
      icon: a.lol.champFor(clip.agent)?.icon,
      name: clip.agent,
      fallback: "Unknown champion",
      sub: [friendlyLolMap(clip.map), clip.mode].filter(Boolean).join(" · "),
      showKda: true,
    };
  },
};

const rematch: GamePresenter = {
  // Rematch has no agent/champion portrait and no K/D/A — just the stadium (the
  // scorer is in the clip title). Deliberately lighter than the FPS/MOBA games.
  cardBadges(clip) {
    return badges({ label: clip.map ?? "" });
  },
  detail(clip) {
    return {
      icon: undefined,
      name: clip.agent ?? null,
      fallback: "Rematch",
      sub: [clip.map, clip.mode].filter(Boolean).join(" · "),
      showKda: false,
    };
  },
};

const cs2: GamePresenter = {
  // CS2 has a map + mode but no agent/champion portrait; K/D/A isn't tracked in
  // the clip context, so the card just shows the map (the multi-kill/headshot
  // labels are in the clip title).
  cardBadges(clip) {
    return badges({ label: clip.map ?? "" });
  },
  detail(clip) {
    return {
      icon: undefined,
      name: null,
      fallback: "Counter-Strike 2",
      sub: [clip.map, clip.mode].filter(Boolean).join(" · "),
      showKda: false,
    };
  },
};

const dota2: GamePresenter = {
  // Dota tags the hero in the agent column but ships no bundled portrait, so the
  // card shows the hero name as plain text (the multi-kill labels are in the
  // clip title). No map/mode or K/D/A surfaced.
  cardBadges(clip) {
    return badges({ label: clip.agent ?? "" });
  },
  detail(clip) {
    return {
      icon: undefined,
      name: clip.agent ?? null,
      fallback: "Dota 2",
      sub: "",
      showKda: false,
    };
  },
};

const warthunder: GamePresenter = {
  // War Thunder exposes no map/mode over its HUD API — only the local vehicle
  // class (stored in `mode`). No agent portrait or K/D/A; the Kill/Crash labels
  // live in the clip title.
  cardBadges(clip) {
    return badges({ label: clip.mode ?? "" });
  },
  detail(clip) {
    return {
      icon: undefined,
      name: null,
      fallback: "War Thunder",
      sub: clip.mode ?? "",
      showKda: false,
    };
  },
};

const pubg: GamePresenter = {
  // PUBG clips carry no match context — its replay sidecars give us only the
  // event moments (in the clip title), not a map or K/D/A. No portrait or badges.
  cardBadges() {
    return badges({ label: "" });
  },
  detail() {
    return {
      icon: undefined,
      name: null,
      fallback: "PUBG",
      sub: "",
      showKda: false,
    };
  },
};

const other: GamePresenter = {
  // Generic "record any game" clips carry no match context — just the real game
  // title (in `clip.game`, surfaced elsewhere). No portrait, badges, or K/D/A.
  cardBadges() {
    return badges({ label: "" });
  },
  detail(clip) {
    return {
      icon: undefined,
      name: null,
      fallback: clip.game ?? "Game",
      sub: "",
      showKda: false,
    };
  },
};

const GAME_PRESENTERS: Record<GameId, GamePresenter> = {
  valorant,
  lol,
  rematch,
  cs2,
  dota2,
  warthunder,
  pubg,
  other,
};

/** The presenter for a clip's source game (falls back to Valorant like the rest). */
export function clipPresenter(clip: ClipRecord): GamePresenter {
  return GAME_PRESENTERS[clipGame(clip.game)];
}
