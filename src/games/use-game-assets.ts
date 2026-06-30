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

/** Merged game-asset bundle threaded through the clips grid + toolbar. */
export type GameAssets = ReturnType<typeof useGameAssets>;
