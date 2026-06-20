# Medal Design System (extracted)

Reverse-engineered from `resources/app/renderer.min.css` of the installed Medal
desktop client (v2625.252.1). For competitive reference only.

**TL;DR:** Medal is **shadcn/ui's default `zinc` dark theme + Tailwind v4 default
scales + Inter (variable) + a named brand color layer**. There is no bespoke type
scale or spacing system. The "polish" comes from the font, fully-opaque near-white
text, translucent borders, and disciplined use of one lime accent.

---

## 1. Foundations

### Fonts
```
--font-sans:    "Inter", ui-sans-serif, system-ui, "Segoe UI", Roboto, ...
--font-display: "Funnel Display", "Inter", system-ui, Georgia, ...
                Poppins (400/500/600) — used for specific accent/badge text
```
- **Inter** loaded as a true variable font: `@font-face { font-weight: 1 999; src: Inter.ttf format("truetype-variations") }` — so every weight, including 600, is real.
- **No `-webkit-font-smoothing` override anywhere.** Uses Chromium's default (heavier on dark).

### Type scale (stock Tailwind v4 — identical to hako)
| Token | Size | Line-height |
|---|---|---|
| text-xs | 0.75rem / 12px | 1/0.75 |
| text-sm | 0.875rem / 14px | 1.25/0.875 |
| text-base | 1rem / 16px | 1.5/1 |
| text-lg | 1.125rem / 18px | 1.75/1.125 |
| text-xl | 1.25rem / 20px | 1.75/1.25 |
| text-2xl | 1.5rem / 24px | 2/1.5 |
| text-3xl | 1.875rem / 30px | 2.25/1.875 |
| text-4xl | 2.25rem / 36px | 2.5/2.25 |
| text-5xl…8xl | 3 / 3.75 / 4.5 / 6rem | 1 |

### Font weights (stock Tailwind)
`thin 100 · light 300 · normal/regular 400 · medium 500 · semibold 600 · bold 700`
Inter being variable means 600 is genuinely available (Satoshi has no 600).

### Leading / tracking (stock Tailwind)
`leading: tight 1.25 · snug 1.375 · normal 1.5 · relaxed 1.625`
`tracking: wide .025em · wider .05em · widest .1em`

### Radius (stock Tailwind)
`xs .125 · sm .25 · md .375 · lg .5 · xl .75 · 2xl 1 · 3xl 1.5rem`

### Spacing
`--spacing: 0.25rem` base (stock Tailwind 4-unit scale).

---

## 2. Semantic surface tokens (shadcn `zinc` dark, OKLCH)

| Token | OKLCH | ≈ hex | Role |
|---|---|---|---|
| `--background` | `14.1% .005 285.8` | #09090b | app canvas |
| `--card` / `--popover` / `--sidebar` | `21% .006 285.9` | ~#1a1a1d | raised surface |
| `--foreground` | `98.5% 0 0` | **#fafafa** | primary text (near-pure white) |
| `--card-foreground` | `98.5% 0 0` | #fafafa | text on cards |
| `--muted` / `--accent` / `--secondary` | `27.4% .006 286` | ~#27272a | subtle fills |
| `--muted-foreground` | `70.5% .015 286` | ~#a1a1aa | secondary text |
| `--primary` | `92% .004 286.3` | ~#e8e8ea | (light, shadcn convention) |
| `--primary-foreground` | `21% .006 285.9` | ~#1a1a1d | text on primary |
| `--border` | `100% 0 0 / .1` | white @ 10% | **translucent**, not solid |
| `--input` | `100% 0 0 / .15` | white @ 15% | translucent field stroke |
| `--ring` | `55.2% .016 285.9` | ~#71717a | focus ring |
| `--destructive` | `70.4% .191 22.2` | ~#f87171 | destructive |
| `--sidebar-primary` | `48.8% .243 264.4` | ~#3b5bdb | active accent (blue) |

**Two details hako doesn't do:**
1. Foreground is **98.5%** (pure white). hako's is `#ededf0` ≈ 93% — measurably dimmer.
2. Borders are **translucent white** (`white/.1`), not a solid zinc hex. Softer, self-adapting.

---

## 3. Brand color layer (named scales, full 50→950)

Tailwind-style 11-step ramps. Hue tells the story:

| Family | Hue | Identity | 500 value |
|---|---|---|---|
| **brand-primary** | ~128 | **lime / chartreuse** (Medal's signature green) | `82.31% .22 128.77` |
| **brand-secondary** | ~31 | orange / red-orange | `64.01% .24 30.7` |
| **brand-tertiary** | ~256 | blue | `62.73% .2 255.69` |
| **brand-quaternary** | ~286 | purple / violet | (400: `54.13% .22 289.66`) |
| **danger** | ~22 | red | `64.18% .19 21.72` |
| **success** | ~161 | emerald | `76.29% .17 161.04` |
| **warning** | ~58 | amber | `73.63% .18 57.69` |

Accent aliases map onto the scales:
```
--color-accent-primary:   brand-primary-400   (lime — the "Learn More" link, logo accent)
--color-accent-secondary: brand-secondary-400 (orange)
--color-accent-tertiary / -info: brand-tertiary (blue — "Create Link")
--color-accent-quaternary: brand-quaternary-400 (purple)
--color-accent-success:   success-500
--color-accent-warning:   warning-500
--color-accent-danger:    danger-500
```

### brand-primary (lime) full ramp
```
50  98.94% .03 115.87   500 82.31% .22 128.77
100 97.64% .07 118.24   600 69.55% .19 129.7
200 95.73% .14 120.61   700 56.77% .15 129.95
300 93.59% .2  123.89   800 47.96% .12 129.18
400 90.73% .21 125.71   900 42.56% .1  129.2
                        950 28.9%  .08 129.92
```
(secondary/tertiary/danger/success/warning ramps follow the same 50→950 structure;
see `renderer.min.css` if exact mid-steps are needed.)

---

## 4. What this means for hako

| Aspect | Medal | hako | Adopt? |
|---|---|---|---|
| Type scale | Tailwind default | Tailwind default | already matched |
| Font | Inter (variable, tall x-height) | Satoshi (geometric, shorter x-height) | brand call — keep Satoshi |
| Font smoothing | browser default (heavier) | was `antialiased` → **fixed** | done |
| Foreground text | **98.5%** white | 93% (`#ededf0`) | consider bumping to ~96–98% |
| Borders | translucent `white/.1` | solid `#27272a` | consider — softer, adaptive |
| Accent discipline | one lime, rest muted | red + many saturated chips | tighten |
| Brand palette | lime / orange / blue / purple full ramps | single brand red | n/a — different identity |

**Highest-leverage borrow:** raise `--foreground` toward 96–98% and switch borders to
translucent white. Those two are most of why Medal reads "cleaner/stronger" beyond the font.
