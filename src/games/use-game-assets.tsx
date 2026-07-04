import * as React from "react";

import { useValorantAssets } from "@/hooks/use-valorant-assets";
import { useLolAssets } from "@/hooks/use-lol-assets";

/**
 * The per-game artwork providers, called once and bundled so every per-clip
 * surface shares one set of (cached) asset lookups. Both hooks run
 * unconditionally to satisfy the rules of hooks; a clip's source game decides
 * which provider to read.
 *
 * This hook only *fetches* art. Turning a clip into card pills / detail fields
 * lives in the per-game presenters (`clip-presenter.ts`), which take this bundle
 * — so adding a game is one asset hook here plus one presenter entry there.
 */
export function useGameAssets() {
  const valorant = useValorantAssets();
  const lol = useLolAssets();

  return React.useMemo(() => ({ valorant, lol }), [valorant, lol]);
}

/** Merged game-asset bundle shared across the clips grid + toolbar. */
export type GameAssets = ReturnType<typeof useGameAssets>;

const GameAssetsContext = React.createContext<GameAssets | null>(null);

/**
 * Provide the game-asset bundle to a whole subtree so the clips grid, its cards'
 * badges, and the toolbar's game filter read it from context instead of the
 * bundle being threaded prop-by-prop through the (memoized, virtualized) row →
 * card → badges chain. The bundle is fetched once here and is session-stable, so
 * context is a clean fit — consumers only re-render when the artwork actually
 * loads. The clip viewer route is a separate subtree and reads `useGameAssets`
 * directly; both share the same underlying (React Query-cached) fetches.
 */
export function GameAssetsProvider({ children }: { children: React.ReactNode }) {
  const assets = useGameAssets();
  return (
    <GameAssetsContext.Provider value={assets}>
      {children}
    </GameAssetsContext.Provider>
  );
}

/** Read the provided game-asset bundle. Must be under a `GameAssetsProvider`. */
export function useGameAssetsContext(): GameAssets {
  const ctx = React.useContext(GameAssetsContext);
  if (!ctx) {
    throw new Error("useGameAssetsContext must be used within a GameAssetsProvider");
  }
  return ctx;
}
