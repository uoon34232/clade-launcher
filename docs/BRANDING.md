# Clade Brand Direction

This document records the current visual direction for Clade Launcher so future assets remain consistent with the product UI.

## Name

Primary product name:

**Clade**

Formal name for releases, documentation, operating-system metadata, and package names:

**Clade Launcher**

## Tagline

**Simple. Portable. Yours.**

The tagline should remain secondary to the product name. It works best on GitHub, release pages, and promotional material rather than inside the launcher chrome.

## Visual character

Clade should feel:

- calm;
- editorial rather than "gamer";
- warm rather than neon;
- precise;
- lightweight;
- modern without heavy glassmorphism.

Avoid:

- RGB/neon gaming aesthetics;
- cyan/purple glow-heavy gradients;
- generic AI-dashboard styling;
- Minecraft-block imitation as the primary brand mark;
- glossy 3D logos;
- excessive blur;
- busy background art behind functional UI.

## Palette

Clade ships with dual warm editorial themes sharing a restrained clay accent and zero pure `#000`, neon gradients, or heavy glassmorphism:

- **Light (Warm Paper)** — warm ivory / cream surfaces (`#F3EDE3` / `#F6F0E6`), graphite text (`#2B2622`) and muted ink (`#52493F`), terracotta clay accents (`#A3523D`), and soft warm ambient gradients.
- **Dark (Graphite Paper)** — soft warm graphite paper surfaces (`#1C1917` / `#24201C`), warm cream paper ink (`#EFE8DD`) and parchment muted ink (`#B2A696`), terracotta clay (`#AB563F`) / soft ember accents (`#E2917A`), and low-saturation warm umber fields.

Exact production colors should be taken from the launcher design tokens rather than duplicated independently in GitHub assets.

## Typography

Current UI direction:

- **English / Latin** (default):
  - **Fraunces** (`@fontsource-variable/fraunces` / `"Fraunces Variable"`) for editorial display/headline moments;
  - **Instrument Sans** (`@fontsource-variable/instrument-sans` / `"Instrument Sans Variable"`) for controls, labels, navigation, and body text.
- **Russian / Cyrillic** (`html[lang="ru"]`):
  - **Literata** (`@fontsource-variable/literata` / `"Literata Variable"`) for editorial display/heading moments;
  - **Onest** (`@fontsource-variable/onest` / `"Onest Variable"`) for controls, labels, navigation, and body text.

GitHub imagery should follow the same relationship where licensing and rendering context permit it.

## Logo concept

The preferred primary mark is an abstract **C**.

It should:

- work at 16×16 through large banner size;
- remain recognizable in a single color;
- use simple geometry;
- feel related to Clade's restrained interface;
- avoid looking like a Minecraft block, pickaxe, grass cube, or creeper face;
- avoid depending on gradients for recognition.

A useful direction is a compact C-shaped form constructed from softly offset geometric segments or stepped planes, with enough negative space to remain legible at small sizes.

The small cat motif discussed for the launcher UI is better treated as a subtle Easter egg or secondary illustration, not the primary application logo.

## Application icon

The approved Clade mark is a white, custom-cut **C** on a warm clay rounded-square tile. The source files are `docs/media/logo-mark-final.svg` and `.png`.

The Windows icon (`src-tauri/icons/icon.ico`) carries the 16, 20, 24, 32, 40, 48, 64, 128 and 256 px sizes, each rendered from the vector so the C stays crisp. The other files in `src-tauri/icons/` are generated from the same mark.

The icon should keep:

- one strong silhouette;
- minimal internal detail;
- good contrast against both light and dark Windows surfaces;
- no text.

## GitHub banner

The approved banner and social preview are `docs/media/banner-final.*` and `docs/media/social-preview.png`. The guidance below describes their composition.

Recommended composition:

- wide, simple layout;
- Clade mark on the left;
- **Clade Launcher** beside or below it;
- tagline: **Simple. Portable. Yours.**;
- warm ivory background or very subtle warm gradient;
- graphite typography;
- one restrained clay accent.

Do not use screenshots as the entire banner background. Product screenshots should appear separately in the README where they remain readable.

## README screenshots

When the launcher UI is stable, capture at least:

1. home screen;
2. Create Profile modal;
3. account menu / account management;
4. preparation or verification flow;
5. English and Russian interface comparison if useful.

Use real Clade UI only. Do not use mock screenshots that imply functionality not present in the build.

Before publishing screenshots, verify that they contain no:

- Microsoft email addresses;
- real UUIDs that the owner does not want public;
- access or refresh tokens;
- local personal filesystem paths;
- private server addresses;
- vault or debug information.

## Voice

Public copy should be concise and factual.

Prefer:

- "Fully portable"
- "Vanilla-first"
- "Isolated profiles"
- "Launcher-managed Java"
- "Official Microsoft sign-in"

Avoid exaggerated claims such as:

- "the ultimate launcher";
- "revolutionary";
- "fastest launcher";
- "100% secure";
- "zero traces on Windows".

Clade's branding should be supported by the product's behavior rather than marketing superlatives.
