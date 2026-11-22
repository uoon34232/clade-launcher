import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
const listen =
  vi.fn<(name: string, cb: (e: { payload: unknown }) => void) => Promise<() => void>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (n: string, cb: (e: { payload: unknown }) => void) => listen(n, cb),
}));

import {
  DEFAULT_UI_SETTINGS,
  SETTINGS_CHANGED_EVENT,
  loadUiSettings,
  onUiSettingsChanged,
  parseUiSettings,
  completeOnboarding,
  getUiSettings,
  resetUiSettings,
  saveBehavior,
  saveLanguage,
  saveLastPhrase,
  saveMemory,
  saveReducedMotion,
  saveSoundSettings,
  saveTheme,
  saveTransparency,
  saveUiMode,
  subscribeUiSettings,
} from "./uiSettings";

const asTauri = () => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
};
const asBrowser = () => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
};

beforeEach(async () => {
  invoke.mockReset();
  listen.mockReset();
  invoke.mockResolvedValue(undefined);
  vi.spyOn(console, "warn").mockImplementation(() => {});
  asTauri();
  invoke.mockResolvedValueOnce(DEFAULT_UI_SETTINGS);
  await loadUiSettings();
  asBrowser();
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
});
afterEach(() => {
  asBrowser();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("parseUiSettings", () => {
  it("accepts the backend's camelCase payload", () => {
    expect(
      parseUiSettings({ soundEnabled: false, soundVolume: 0.25, lastPhrase: "welcome-back" }),
    ).toEqual({
      ...DEFAULT_UI_SETTINGS,
      soundEnabled: false,
      soundVolume: 0.25,
      lastPhrase: "welcome-back",
    });
  });

  it("reads the Settings fields and repairs bad ones to safe values", () => {
    expect(
      parseUiSettings({
        reducedMotion: "on",
        onboardingDone: false,
        memoryMb: 6144,
        memory: { min: 1024, max: 12288, auto: 4096, total: 16384 },
        onLaunch: "keep_open",
        onExit: "stay_hidden",
        version: "0.1.0",
      }),
    ).toMatchObject({
      reducedMotion: "on",
      onboardingDone: false,
      memoryMb: 6144,
      memory: { min: 1024, max: 12288, auto: 4096, total: 16384 },
      onLaunch: "keep_open",
      onExit: "stay_hidden",
      version: "0.1.0",
    });
    const bad = parseUiSettings({
      reducedMotion: "sometimes",
      onboardingDone: "yes",
      memoryMb: -4,
      memory: { min: 5000, max: 10, auto: 99999, total: "lots" },
      onLaunch: "explode",
      onExit: 7,
      version: "../../x",
    });
    expect(bad.reducedMotion).toBe("system");
    expect(bad.onboardingDone).toBe(true); // an unreadable answer never traps anyone in the introduction
    expect(bad.memoryMb).toBeNull();
    expect(bad.memory.max).toBeGreaterThanOrEqual(bad.memory.min);
    expect(bad.memory.auto).toBeLessThanOrEqual(bad.memory.max);
    expect(bad.memory.total).toBeNull();
    expect([bad.onLaunch, bad.onExit]).toEqual(["embed", "show"]);
    expect(bad.version).toBe("");
    expect(parseUiSettings({ onLaunch: "embed" }).onLaunch).toBe("embed");
    expect(parseUiSettings({ onLaunch: "hide" }).onLaunch).toBe("hide");
  });

  it("accepts only the two shipped languages and a well-formed Windows locale name", () => {
    expect(parseUiSettings({ language: "ru", systemLocale: "ru-RU" })).toMatchObject({
      language: "ru",
      systemLocale: "ru-RU",
    });
    expect(parseUiSettings({ language: "en" }).language).toBe("en");
    for (const bad of ["de", "RU", "", 5, null, "ru-RU"]) {
      expect(parseUiSettings({ language: bad }).language).toBeNull();
    }
    expect(parseUiSettings({ systemLocale: "../../x" }).systemLocale).toBeNull();
    expect(parseUiSettings({ systemLocale: 7 }).systemLocale).toBeNull();
  });

  it("repairs anything malformed instead of trusting it", () => {
    expect(parseUiSettings(null)).toEqual(DEFAULT_UI_SETTINGS);
    expect(parseUiSettings("nope")).toEqual(DEFAULT_UI_SETTINGS);
    expect(parseUiSettings({ soundEnabled: "yes", soundVolume: "loud", lastPhrase: 5 })).toEqual(
      DEFAULT_UI_SETTINGS,
    );
    expect(parseUiSettings({ soundVolume: 9 }).soundVolume).toBe(1);
    expect(parseUiSettings({ soundVolume: -1 }).soundVolume).toBe(0);
    expect(parseUiSettings({ soundVolume: Number.NaN }).soundVolume).toBe(0.5);
    expect(parseUiSettings({ lastPhrase: "../../evil" }).lastPhrase).toBeNull();
    expect(parseUiSettings({ lastPhrase: "Has Spaces" }).lastPhrase).toBeNull();
  });

  it("rejects non-finite and non-positive numbers in memory and sound settings", () => {
    for (const bad of [Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, Number.NaN, 0]) {
      const parsed = parseUiSettings({
        soundVolume: bad,
        memoryMb: bad,
        memory: { min: bad, max: bad, auto: bad, total: bad },
      });
      expect(parsed.memoryMb).toBeNull();
      expect(parsed.memory).toEqual(DEFAULT_UI_SETTINGS.memory);
      expect(parsed.soundVolume).toBe(bad === 0 ? 0 : DEFAULT_UI_SETTINGS.soundVolume);
    }
  });
});

describe("inside the launcher (Tauri)", () => {
  beforeEach(asTauri);

  it("loads the saved settings from the Rust settings service", async () => {
    invoke.mockResolvedValueOnce({
      soundEnabled: false,
      soundVolume: 0.7,
      lastPhrase: "take-your-time",
      language: "ru",
      systemLocale: "en-US",
    });
    expect(await loadUiSettings()).toEqual({
      ...DEFAULT_UI_SETTINGS,
      soundEnabled: false,
      soundVolume: 0.7,
      lastPhrase: "take-your-time",
      language: "ru",
      systemLocale: "en-US",
    });
    expect(invoke).toHaveBeenCalledWith("get_ui_settings", undefined);
  });

  it("falls back to defaults, without hanging, if the backend fails or never answers", async () => {
    invoke.mockRejectedValueOnce(new Error("boom"));
    expect(await loadUiSettings()).toEqual(DEFAULT_UI_SETTINGS);

    vi.useFakeTimers();
    invoke.mockReturnValueOnce(new Promise(() => {}));
    const pending = loadUiSettings();
    await vi.advanceTimersByTimeAsync(2000);
    expect(await pending).toEqual(DEFAULT_UI_SETTINGS);
  });

  it("saves sound changes and the last headline through the backend commands", () => {
    saveSoundSettings({ enabled: false, volume: 0.3 });
    expect(invoke).toHaveBeenCalledWith("set_sound_settings", { enabled: false, volume: 0.3 });
    saveLastPhrase("one-more-world");
    expect(invoke).toHaveBeenCalledWith("set_last_phrase", { id: "one-more-world" });
  });

  it("saves an explicit language through the backend and reports whether it stuck", async () => {
    expect(await saveLanguage("ru")).toBe(true);
    expect(invoke).toHaveBeenCalledWith("set_language", { language: "ru" });
    invoke.mockRejectedValueOnce(new Error("read-only stick"));
    expect(await saveLanguage("en")).toBe(false);
  });

  it("sends each Settings change through its own command and reports a refusal", async () => {
    expect(await saveUiMode("advanced")).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_ui_mode", { mode: "advanced" });
    expect(await saveReducedMotion("on")).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_reduced_motion", { mode: "on" });
    expect(await saveMemory(4096)).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_memory", { mb: 4096 });
    expect(await saveMemory(null)).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_memory", { mb: null });
    expect(await saveBehavior({ onLaunch: "hide" })).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_behavior", { onLaunch: "hide", onExit: undefined });
    expect(await saveBehavior({ onExit: "stay_hidden" })).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_behavior", {
      onLaunch: undefined,
      onExit: "stay_hidden",
    });
    expect(await completeOnboarding()).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("complete_onboarding", {});
    expect(await resetUiSettings()).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("reset_settings", {});

    const refused = { code: "invalid_memory", params: { min: 1024, max: 4096 } };
    invoke.mockRejectedValueOnce(refused);
    expect(await saveMemory(99999)).toEqual({ ok: false, error: refused });

    const ioError = new Error("read-only stick");
    for (const call of [
      () => saveUiMode("advanced"),
      () => saveReducedMotion("on"),
      () => saveTransparency("off"),
      () => saveBehavior({ onLaunch: "hide" }),
      () => completeOnboarding(),
      () => resetUiSettings(),
    ]) {
      invoke.mockRejectedValueOnce(ioError);
      expect(await call()).toEqual({ ok: false, error: ioError });
    }
  });

  it("never throws when a save fails", async () => {
    invoke.mockRejectedValue(new Error("read-only stick"));
    expect(() => saveSoundSettings({ volume: 0.1 })).not.toThrow();
    expect(() => saveLastPhrase("welcome-back")).not.toThrow();
    await Promise.resolve();
  });

  it("subscribes to backend change events, syncs the live store, and validates payloads", async () => {
    const unlisten = vi.fn();
    let handler: (e: { payload: unknown }) => void = () => {};
    listen.mockImplementation((_n, cb) => {
      handler = cb;
      return Promise.resolve(unlisten);
    });
    const seen: unknown[] = [];
    const storeNotifications = vi.fn();
    const unsub = subscribeUiSettings(storeNotifications);
    const off = await onUiSettingsChanged((s) => seen.push(s));
    expect(listen.mock.calls[0][0]).toBe(SETTINGS_CHANGED_EVENT);
    handler({ payload: { soundEnabled: false, soundVolume: 4, lastPhrase: null } });
    const expected = { ...DEFAULT_UI_SETTINGS, soundEnabled: false, soundVolume: 1 };
    expect(seen).toEqual([expected]);
    expect(getUiSettings()).toEqual(expected);
    expect(storeNotifications).toHaveBeenCalledTimes(1);
    unsub();
    off();
    expect(unlisten).toHaveBeenCalled();
  });

  it("returns a no-op unsubscriber when listening for change events fails", async () => {
    listen.mockRejectedValueOnce(new Error("event bridge unavailable"));
    const off = await onUiSettingsChanged(() => {});
    expect(off()).toBeUndefined();
    expect(console.warn).toHaveBeenCalled();
  });
});

describe("outside the launcher (plain browser, tests)", () => {
  it("keeps values in memory only: no backend calls and no WebView storage", async () => {
    const get = vi.spyOn(Storage.prototype, "getItem");
    const set = vi.spyOn(Storage.prototype, "setItem");
    saveSoundSettings({ enabled: false, volume: 0.9 });
    saveLastPhrase("choose-your-world");
    expect(await saveLanguage("ru")).toBe(true);
    expect(await loadUiSettings()).toMatchObject({
      soundEnabled: false,
      soundVolume: 0.9,
      lastPhrase: "choose-your-world",
      language: "ru",
    });
    expect(invoke).not.toHaveBeenCalled();
    expect(get).not.toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
    const off = await onUiSettingsChanged(() => {});
    expect(off()).toBeUndefined();
  });

  it("uses navigator.language as systemLocale until one is already set", async () => {
    const langSpy = vi.spyOn(navigator, "language", "get");
    langSpy.mockReturnValueOnce("../../bad");
    expect((await loadUiSettings()).systemLocale).toBeNull();

    langSpy.mockReturnValueOnce("ru-RU");
    expect((await loadUiSettings()).systemLocale).toBe("ru-RU");

    langSpy.mockReturnValueOnce("en-US");
    expect((await loadUiSettings()).systemLocale).toBe("ru-RU");
  });

  it("clamps sound volume and sanitizes lastPhrase in memory", () => {
    saveSoundSettings({ enabled: false, volume: 4 });
    expect(getUiSettings()).toMatchObject({ soundEnabled: false, soundVolume: 1 });

    saveSoundSettings({ volume: -2 });
    expect(getUiSettings()).toMatchObject({ soundEnabled: false, soundVolume: 0 });

    saveSoundSettings({ volume: Number.NaN });
    expect(getUiSettings().soundVolume).toBe(DEFAULT_UI_SETTINGS.soundVolume);

    saveLastPhrase("Has Spaces");
    expect(getUiSettings().lastPhrase).toBeNull();

    saveLastPhrase("welcome-back");
    expect(getUiSettings().lastPhrase).toBe("welcome-back");
  });

  it("updates only the supplied behavior field in memory", async () => {
    expect(await saveBehavior({ onExit: "stay_hidden" })).toEqual({ ok: true });
    expect(getUiSettings()).toMatchObject({ onLaunch: "embed", onExit: "stay_hidden" });

    expect(await saveBehavior({ onLaunch: "hide" })).toEqual({ ok: true });
    expect(getUiSettings()).toMatchObject({ onLaunch: "hide", onExit: "stay_hidden" });
  });
});

describe("the transparency preference", () => {
  it("defaults to following Windows, also for an older backend that does not send it", () => {
    expect(DEFAULT_UI_SETTINGS.transparency).toBe("system");
    expect(parseUiSettings({}).transparency).toBe("system");
    expect(parseUiSettings({ reducedMotion: "on" }).transparency).toBe("system");
  });

  it("accepts the three modes and repairs anything else to system", () => {
    for (const m of ["system", "on", "off"])
      expect(parseUiSettings({ transparency: m }).transparency).toBe(m);
    for (const bad of ["glass", "", 1, true, null, {}])
      expect(parseUiSettings({ transparency: bad }).transparency).toBe("system");
  });

  it("is independent of reduced motion", () => {
    expect(parseUiSettings({ reducedMotion: "on" }).transparency).toBe("system");
    expect(parseUiSettings({ transparency: "on" }).reducedMotion).toBe("system");
  });

  it("is saved through its own backend command and restored by a reset", async () => {
    asTauri();
    expect(await saveTransparency("off")).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_transparency", { mode: "off" });
    asBrowser();
    await saveTransparency("on");
    expect(getUiSettings().transparency).toBe("on");
    await saveReducedMotion("on");
    expect(getUiSettings().transparency).toBe("on");
    await resetUiSettings();
    expect(getUiSettings().transparency).toBe("system");
  });
});

describe("the theme preference", () => {
  it("defaults to following Windows, also for an older backend that does not send it", () => {
    expect(DEFAULT_UI_SETTINGS.theme).toBe("system");
    expect(parseUiSettings({}).theme).toBe("system");
    expect(parseUiSettings({ reducedMotion: "on" }).theme).toBe("system");
  });

  it("accepts the three modes and repairs anything else to system", () => {
    for (const m of ["system", "light", "dark"])
      expect(parseUiSettings({ theme: m }).theme).toBe(m);
    for (const bad of ["midnight", "sepia", "", 1, true, null, {}])
      expect(parseUiSettings({ theme: bad }).theme).toBe("system");
  });

  it("is saved through set_theme and restored by a reset", async () => {
    asTauri();
    expect(await saveTheme("dark")).toEqual({ ok: true });
    expect(invoke).toHaveBeenCalledWith("set_theme", { mode: "dark" });
    asBrowser();
    await saveTheme("light");
    expect(getUiSettings().theme).toBe("light");
    await resetUiSettings();
    expect(getUiSettings().theme).toBe("system");
  });
});

describe("the live settings copy", () => {
  it("keeps session-only values and tells subscribers, with no backend and no browser storage", async () => {
    const seen: number[] = [];
    const off = subscribeUiSettings(() => seen.push(getUiSettings().memoryMb ?? -1));
    expect(await saveUiMode("advanced")).toEqual({ ok: true });
    expect(await saveMemory(8192)).toEqual({ ok: true });
    expect(await saveReducedMotion("off")).toEqual({ ok: true });
    expect(await saveBehavior({ onLaunch: "keep_open", onExit: "stay_hidden" })).toEqual({
      ok: true,
    });
    expect(await completeOnboarding()).toEqual({ ok: true });
    expect(getUiSettings()).toEqual({
      ...DEFAULT_UI_SETTINGS,
      uiMode: "advanced",
      memoryMb: 8192,
      reducedMotion: "off",
      onLaunch: "keep_open",
      onExit: "stay_hidden",
      onboardingDone: true,
    });
    expect(seen.length).toBeGreaterThanOrEqual(4);
    // a reset puts the preferences back, but never the introduction
    expect(await resetUiSettings()).toEqual({ ok: true });
    expect(getUiSettings()).toEqual({
      ...DEFAULT_UI_SETTINGS,
      uiMode: "simple",
      memoryMb: null,
      reducedMotion: "system",
      onLaunch: "embed",
      onExit: "show",
      onboardingDone: true,
    });
    const countBeforeOff = seen.length;
    off();
    await saveUiMode("advanced");
    expect(seen).toHaveLength(countBeforeOff);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("defaults uiMode to simple and repairs unknown values", () => {
    expect(DEFAULT_UI_SETTINGS.uiMode).toBe("simple");
    expect(parseUiSettings({ uiMode: "advanced" }).uiMode).toBe("advanced");
    for (const bad of ["expert", "", 1, null, {}]) {
      expect(parseUiSettings({ uiMode: bad }).uiMode).toBe("simple");
    }
  });
});
