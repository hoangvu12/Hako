import { Sparkle, Crosshair, Scissors, CloudArrowUp } from "@phosphor-icons/react";

import { SectionHero, Panel, Row } from "@/components/settings/primitives";

export function WelcomeStep() {
  return (
    <>
      <SectionHero
        icon={Sparkle}
        title="Never miss a play"
        subtitle="Hako quietly records in the background and clips your best Valorant moments — clutches, aces, multikills — so you can relive and share them."
      />
      <Panel title="In about a minute, you'll be able to">
        {[
          {
            icon: Crosshair,
            label: "Capture highlights automatically",
            hint: "Your best rounds, saved without lifting a finger.",
          },
          {
            icon: Scissors,
            label: "Save any moment with a hotkey",
            hint: "Pull the last few seconds whenever something pops off.",
          },
          {
            icon: CloudArrowUp,
            label: "Keep and share your clips",
            hint: "Stored your way, ready to upload.",
          },
        ].map((it) => (
          <Row key={it.label} label={it.label} hint={it.hint}>
            <it.icon className="size-5 text-primary-text" weight="duotone" />
          </Row>
        ))}
      </Panel>
      <p className="px-1 text-center text-xs text-muted-foreground">
        Takes about a minute. You can skip anything and change it all later in Settings.
      </p>
    </>
  );
}
