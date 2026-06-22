import type { ValorantAssets } from "@/hooks/use-valorant-assets";
import type { Facets } from "@/components/clips/use-clip-filters";
import type { Option } from "./types";

/**
 * Turn the raw facet values into renderable filter `Option`s, attaching the
 * matching Valorant artwork (agent portraits + signature gradients, map splashes,
 * mode list art) so the popovers read as Hako rather than plain text rows.
 */
export function buildFacetOptions(facets: Facets, assets: ValorantAssets) {
  const agentOptions: Option[] = facets.agents.map((name) => {
    const a = assets.agentByName(name);
    return {
      value: name,
      label: name,
      icon: a?.icon,
      // Signature gradient base + faint agent-select texture over it.
      art: a?.gradient
        ? { gradient: a.gradient, image: a.background || undefined, fit: "cover" }
        : undefined,
    };
  });
  const mapOptions: Option[] = facets.maps.map((m) => {
    const meta = assets.mapFor(m.path);
    return {
      value: m.path,
      label: meta?.name || m.name,
      icon: meta?.listIcon,
      // Full-bleed map splash (unchanged from before).
      art: meta?.splash ? { image: meta.splash, fit: "cover" } : undefined,
    };
  });
  const modeOptions: Option[] = facets.modes.map((m) => {
    const meta = assets.modeFor(m);
    return {
      value: m,
      label: m,
      icon: meta?.icon,
      // Full-bleed tall list art behind the row, mirroring the Map filter's splash.
      art: meta?.tall ? { image: meta.tall, fit: "cover" } : undefined,
    };
  });
  const eventOptions: Option[] = facets.events.map((e) => ({ value: e, label: e }));

  return { agentOptions, mapOptions, modeOptions, eventOptions };
}
