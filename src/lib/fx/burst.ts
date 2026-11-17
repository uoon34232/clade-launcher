// A tiny, one-shot particle burst. Not a particle system: each click spawns a handful of
// elements, each animates once, and each is removed the moment its animation ends.
//
// Style: 6-12 very small dots or short strokes in the existing palette (muted clay, graphite,
// warm ivory/sand), scattering a short, slightly random distance and fading quickly.

const COLORS = ["#b5604a", "#9c5039", "#2b2622", "#b5604a", "#e4d5be", "#f6f0e6"] as const;
const MIN_COUNT = 6;
const MAX_COUNT = 12;
const MIN_LIFE_MS = 350;
const MAX_LIFE_MS = 650;
/** Safety cap across overlapping bursts, so mashing the headline can never flood the DOM. */
export const MAX_ACTIVE_PARTICLES = 24;
const LAYER_ID = "fx-layer";

let active = 0;
let layer: HTMLDivElement | null = null;

export const activeParticleCount = () => active;

export interface BurstOptions {
  /** Overrides the random particle count (6-12). */
  count?: number;
  /** Injectable for deterministic tests. */
  random?: () => number;
  /** With reduced motion the burst is skipped entirely. */
  reducedMotion?: boolean;
}

function getLayer(): HTMLDivElement {
  if (layer && layer.isConnected) return layer;
  active = 0;
  const el = document.createElement("div");
  el.id = LAYER_ID;
  el.setAttribute("aria-hidden", "true");
  Object.assign(el.style, {
    position: "fixed",
    inset: "0",
    pointerEvents: "none",
    overflow: "hidden",
    zIndex: "60",
  } satisfies Partial<CSSStyleDeclaration>);
  document.body.appendChild(el);
  layer = el;
  return el;
}

interface ParticleEntry {
  node: HTMLSpanElement;
  released: boolean;
}

/** Emits a burst at viewport coordinates. Returns how many particles were created. */
export function emitBurst(x: number, y: number, opts: BurstOptions = {}): number {
  if (typeof document === "undefined" || opts.reducedMotion) return 0;
  if (layer && !layer.isConnected) {
    layer = null;
    active = 0;
  }
  const rnd = opts.random ?? Math.random;
  const wanted = opts.count ?? MIN_COUNT + Math.floor(rnd() * (MAX_COUNT - MIN_COUNT + 1));
  const count = Math.max(0, Math.min(wanted, MAX_ACTIVE_PARTICLES - active));
  if (count === 0) return 0;

  const host = getLayer();
  const canAnimate = typeof host.animate === "function";
  const baseAngle = rnd() * Math.PI * 2;
  const entries: ParticleEntry[] = [];
  let maxLife = 0;
  let remaining = count;
  let batchTimer: number | undefined;

  const releaseEntry = (entry: ParticleEntry) => {
    if (entry.released) return;
    entry.released = true;
    entry.node.remove();
    active = Math.max(0, active - 1);
    remaining--;
    if (remaining <= 0 && batchTimer !== undefined) {
      window.clearTimeout(batchTimer);
      batchTimer = undefined;
    }
    if (active === 0 && layer) {
      layer.remove();
      layer = null;
    }
  };

  for (let i = 0; i < count; i++) {
    // Evenly spread directions with jitter, so the burst reads as a scatter, not a ring.
    const angle = baseAngle + (i / count) * Math.PI * 2 + (rnd() - 0.5) * 0.9;
    const distance = 16 + rnd() * 30;
    const dx = Math.cos(angle) * distance;
    const dy = Math.sin(angle) * distance + 5; // a hint of settling
    const life = MIN_LIFE_MS + rnd() * (MAX_LIFE_MS - MIN_LIFE_MS);
    if (life > maxLife) maxLife = life;
    const color = COLORS[Math.floor(rnd() * COLORS.length)];
    const stroke = rnd() < 0.3;
    const size = stroke ? 1.5 : 2 + rnd() * 2;
    const length = stroke ? 5 + rnd() * 3 : size;

    const node = document.createElement("span");
    Object.assign(node.style, {
      position: "absolute",
      left: `${x}px`,
      top: `${y}px`,
      width: `${length}px`,
      height: `${size}px`,
      borderRadius: "9999px",
      background: color,
      opacity: "0",
    } satisfies Partial<CSSStyleDeclaration>);
    host.appendChild(node);
    const entry: ParticleEntry = { node, released: false };
    entries.push(entry);
    active++;

    if (!canAnimate) continue;
    const rotation = stroke ? `rotate(${(angle * 180) / Math.PI}deg)` : "";
    const anim = node.animate(
      [
        {
          transform: `translate(-50%, -50%) translate(0px, 0px) ${rotation} scale(1)`,
          opacity: 0.95,
        },
        {
          transform: `translate(-50%, -50%) translate(${dx}px, ${dy}px) ${rotation} scale(0.35)`,
          opacity: 0,
        },
      ],
      { duration: life, easing: "cubic-bezier(0.16, 0.8, 0.3, 1)", fill: "forwards" },
    );
    const done = () => releaseEntry(entry);
    anim.onfinish = done;
    anim.oncancel = done;
  }

  // Single batch failsafe timer for the entire burst instead of N timers per particle.
  batchTimer = window.setTimeout(
    () => {
      batchTimer = undefined;
      for (const entry of entries) releaseEntry(entry);
    },
    Math.ceil(maxLife) + 120,
  );
  return count;
}
