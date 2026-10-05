// The translucent window material ("glass"), as far as the interface is concerned.
//
// Rust decides (`glass.rs`): the person's "Transparency effects" choice, the Windows preference and
// High Contrast are resolved natively, the window material is applied, and the result is sent here.
// This module only reflects that result on the root element so the stylesheet can switch its tokens:
//   data-glass="on" | "off"          the material is active / the classic opaque look
//   data-glass-live="true" | "false" Windows is really drawing the material / shows a solid fallback
// With "off" nothing glass-related stays on the page: the attributes are removed, not just changed.
//
// Transparency and reduced motion are independent: nothing here reads the motion setting.
//
// Outside the launcher window (plain browser, unit tests) there is no native material, so the page
// stays opaque.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/settings/uiSettings";

export interface GlassStatus {
  /** The window material is on. */
  active: boolean;
  /** Windows is really drawing it (false when it shows its solid fallback instead). */
  live: boolean;
}

export const OPAQUE: Readonly<GlassStatus> = { active: false, live: false };

export const GLASS_EVENT = "glass-status-changed";

/** Validates what the backend sent; anything unexpected is the opaque look. */
export function parseGlassStatus(raw: unknown): GlassStatus {
  if (typeof raw !== "object" || raw === null) return { ...OPAQUE };
  const o = raw as Record<string, unknown>;
  const active = o.active === true;
  return { active, live: active && o.live === true };
}

/** Marks the page for the stylesheet. An inactive status leaves no glass attribute behind. */
export function applyGlass(status: GlassStatus): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  if (status.active) {
    root.dataset.glass = "on";
    root.dataset.glassLive = String(status.live);
  } else {
    root.dataset.glass = "off";
    delete root.dataset.glassLive;
  }
}

/** Reads the current state once at startup. Never rejects: any failure is the opaque look. */
export async function loadGlassStatus(): Promise<GlassStatus> {
  if (!inTauri()) return { ...OPAQUE };
  try {
    return parseGlassStatus(await invoke("get_glass_status"));
  } catch (e) {
    console.warn("Clade glass: could not read the window material state", e);
    return { ...OPAQUE };
  }
}

/** Follows later changes (a setting, the Windows preference, High Contrast). */
export async function onGlassStatusChanged(
  callback: (s: GlassStatus) => void,
): Promise<() => void> {
  if (!inTauri()) return () => {};
  try {
    return await listen<unknown>(GLASS_EVENT, (e) => callback(parseGlassStatus(e.payload)));
  } catch (e) {
    console.warn("Clade glass: could not subscribe", e);
    return () => {};
  }
}
