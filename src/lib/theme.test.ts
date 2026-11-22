import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getThemeMode,
  installSystemThemeListener,
  resolveTheme,
  resolvedTheme,
  setThemeMode,
} from "./theme";

function mockMatchMedia(initialDark: boolean) {
  let dark = initialDark;
  const listeners = new Set<() => void>();
  vi.stubGlobal("matchMedia", (query: string) => {
    expect(query).toBe("(prefers-color-scheme: dark)");
    return {
      get matches() {
        return dark;
      },
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: (_type: string, cb: EventListenerOrEventListenerObject) => {
        if (typeof cb === "function") listeners.add(cb as () => void);
      },
      removeEventListener: (_type: string, cb: EventListenerOrEventListenerObject) => {
        if (typeof cb === "function") listeners.delete(cb as () => void);
      },
      dispatchEvent: () => false,
    };
  });
  return {
    setDark(next: boolean) {
      dark = next;
      for (const cb of [...listeners]) cb();
    },
    listenerCount: () => listeners.size,
  };
}

afterEach(() => {
  setThemeMode("system");
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.themeMode;
  document.documentElement.style.colorScheme = "";
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("theme resolution", () => {
  it("resolves explicit light/dark choices and defers system to the OS preference", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("system", true)).toBe("dark");
  });

  it("updates root attributes and color-scheme on mode changes", () => {
    mockMatchMedia(false);
    setThemeMode("dark");
    expect(getThemeMode()).toBe("dark");
    expect(resolvedTheme()).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.themeMode).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");

    setThemeMode("light");
    expect(resolvedTheme()).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.themeMode).toBe("light");
    expect(document.documentElement.style.colorScheme).toBe("light");
  });

  it("tracks live OS theme changes only when set to system", () => {
    const mm = mockMatchMedia(false);
    const unlisten = installSystemThemeListener();
    setThemeMode("system");
    expect(document.documentElement.dataset.theme).toBe("light");

    mm.setDark(true);
    expect(document.documentElement.dataset.theme).toBe("dark");

    setThemeMode("light");
    mm.setDark(false);
    mm.setDark(true);
    expect(document.documentElement.dataset.theme).toBe("light");

    unlisten();
    expect(mm.listenerCount()).toBe(0);
  });
});
