// One place that resolves and applies the colour theme. The person's choice in Settings
// ("Follow Windows", "Light", "Dark") wins; "Follow Windows" defers to the operating system's own
// preference (`prefers-color-scheme: dark`). It marks `data-theme` (`"light"` or `"dark"`) and
// `data-theme-mode` (`"system"`, `"light"` or `"dark"`) on the root element so the stylesheet and
// any theme-aware script see the resolved palette immediately.

import type { ThemeMode } from "@/lib/settings/uiSettings";

export type ResolvedTheme = "light" | "dark";

let mode: ThemeMode = "system";

const systemPrefersDark = (): boolean =>
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(prefers-color-scheme: dark)").matches;

export const resolveTheme = (themeMode: ThemeMode, systemDark: boolean): ResolvedTheme =>
  themeMode === "dark" ? "dark" : themeMode === "light" ? "light" : systemDark ? "dark" : "light";

export function resolvedTheme(): ResolvedTheme {
  return resolveTheme(mode, systemPrefersDark());
}

function applyResolvedTheme(): void {
  if (typeof document === "undefined") return;
  const resolved = resolvedTheme();
  const root = document.documentElement;
  root.dataset.themeMode = mode;
  root.dataset.theme = resolved;
  root.style.colorScheme = resolved;
}

/** Applies the chosen mode (and marks the page for the stylesheet). */
export function setThemeMode(next: ThemeMode): void {
  mode = next;
  applyResolvedTheme();
}

export const getThemeMode = (): ThemeMode => mode;

/** Keeps `"system"` synchronized when Windows switches between Light and Dark mode live. */
export function installSystemThemeListener(): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return () => {};
  }
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const onChange = () => {
    if (mode === "system") applyResolvedTheme();
  };
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}
