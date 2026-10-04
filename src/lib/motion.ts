// One place that decides whether decorative motion is reduced. The person's choice in Settings
// ("Follow Windows", "On", "Off") wins; "Follow Windows" defers to the operating system's own
// preference. Pure CSS follows the same decision through `data-motion` on the root element
// (`reduce`, `full` or `system`), and scripted animations ask `prefersReducedMotion()`.

import type { MotionMode } from "@/lib/settings/uiSettings";

let mode: MotionMode = "system";

const systemPrefersReduced = (): boolean =>
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Applies the chosen mode (also marks the page for the stylesheet). */
export function setMotionMode(next: MotionMode): void {
  mode = next;
  if (typeof document !== "undefined") {
    document.documentElement.dataset.motion =
      next === "on" ? "reduce" : next === "off" ? "full" : "system";
  }
}

export const getMotionMode = (): MotionMode => mode;

/** True when decorative, non-essential motion should be skipped or shortened. */
export function prefersReducedMotion(): boolean {
  if (mode === "on") return true;
  if (mode === "off") return false;
  return systemPrefersReduced();
}
