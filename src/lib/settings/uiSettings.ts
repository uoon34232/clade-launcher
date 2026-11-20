// Frontend side of the launcher's persistent UI state (sound, language, motion, game memory, window
// behavior, the first-run introduction, the last headline).
//
// The authority is the Rust settings service (`data/settings.json` in the portable launcher
// folder). This module only talks to it over Tauri commands and events. It deliberately does NOT
// use localStorage, IndexedDB, cookies or any other WebView storage: the WebView profile
// (`data/webview2`) is disposable browser-engine cache that may be deleted at any time.
// A unit test scans the source to keep it that way.
//
// Outside Tauri (plain-browser development, unit tests) there is no launcher backend, so values
// live in memory for the session only and are never written anywhere.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isLocale, type Locale } from "@/lib/i18n/core";

export type UiMode = "simple" | "advanced";
export type MotionMode = "system" | "on" | "off";
/** Whether the translucent window material follows Windows, is always tried, or is never used. */
export type TransparencyMode = "system" | "on" | "off";
export type OnLaunch = "embed" | "keep_open" | "hide";
export type OnExit = "show" | "stay_hidden";

/** What this computer allows for the game's memory, in MiB, and what "Automatic" means here. */
export interface MemoryInfo {
  min: number;
  max: number;
  auto: number;
  total: number | null;
}

export interface UiSettings {
  soundEnabled: boolean;
  /** 0 to 1. */
  soundVolume: number;
  /** Id of the headline shown last, so the next start shows a different one. */
  lastPhrase: string | null;
  /** The language the person chose, or `null` if they never did (then the Windows language decides). */
  language: Locale | null;
  /** The Windows display language (a locale name such as `ru-RU`), for the first-run default. */
  systemLocale: string | null;
  /** Whether the interface hides advanced tuning options (`simple`) or exposes them (`advanced`). */
  uiMode: UiMode;
  /** Whether decorative motion follows Windows, is always reduced, or is never reduced. */
  reducedMotion: MotionMode;
  /** The "Transparency effects" choice: follow Windows, on, or off. Separate from reduced motion. */
  transparency: TransparencyMode;
  /** True once the first-run introduction was finished or skipped. */
  onboardingDone: boolean;
  /** The global game memory in MiB, or `null` for the automatic policy. */
  memoryMb: number | null;
  memory: MemoryInfo;
  onLaunch: OnLaunch;
  onExit: OnExit;
  /** The application version, from the backend (Cargo.toml). Empty outside the launcher. */
  version: string;
}

export const DEFAULT_UI_SETTINGS: Readonly<UiSettings> = {
  soundEnabled: true,
  soundVolume: 0.5,
  lastPhrase: null,
  language: null,
  systemLocale: null,
  uiMode: "simple",
  reducedMotion: "system",
  transparency: "system",
  // A backend that cannot be read must never trap anyone in the introduction, so the fallback is
  // "already seen". The real backend reports the true value.
  onboardingDone: true,
  memoryMb: null,
  memory: { min: 1024, max: 16384, auto: 4096, total: null },
  onLaunch: "embed",
  onExit: "show",
  version: "",
};

/** Name of the event the backend emits whenever a setting changes. */
export const SETTINGS_CHANGED_EVENT = "ui-settings-changed";

/** Starting the UI must never hang on the backend: after this, defaults are used. */
const LOAD_TIMEOUT_MS = 1500;

export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ------------------------------------------------------------------------------ the live copy

let current: UiSettings = { ...DEFAULT_UI_SETTINGS };
const listeners = new Set<() => void>();

function setCurrent(next: UiSettings) {
  current = next;
  for (const l of [...listeners]) l();
}

/** The settings as the UI currently knows them (what the backend last said, or the session's own). */
export const getUiSettings = (): UiSettings => current;

/** Calls `listener` whenever the settings change, from this window or from the backend. */
export function subscribeUiSettings(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// ------------------------------------------------------------------------------------ parsing

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const oneOf = <T extends string>(v: unknown, allowed: readonly T[], fallback: T): T =>
  typeof v === "string" && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
const whole = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.round(v) : fallback;

/** Validates data coming from the backend instead of trusting its shape. */
export function parseUiSettings(raw: unknown): UiSettings {
  const o = isObj(raw) ? raw : {};
  const volume =
    typeof o.soundVolume === "number" && Number.isFinite(o.soundVolume)
      ? Math.min(1, Math.max(0, o.soundVolume))
      : DEFAULT_UI_SETTINGS.soundVolume;
  const m = isObj(o.memory) ? o.memory : {};
  const d = DEFAULT_UI_SETTINGS.memory;
  const min = whole(m.min, d.min);
  const max = Math.max(min, whole(m.max, d.max));
  return {
    soundEnabled:
      typeof o.soundEnabled === "boolean" ? o.soundEnabled : DEFAULT_UI_SETTINGS.soundEnabled,
    soundVolume: volume,
    lastPhrase:
      typeof o.lastPhrase === "string" && /^[a-z0-9-]{1,64}$/.test(o.lastPhrase)
        ? o.lastPhrase
        : null,
    language: isLocale(o.language) ? o.language : null,
    systemLocale:
      typeof o.systemLocale === "string" &&
      /^[A-Za-z]{2,3}([-_][A-Za-z0-9]{2,8}){0,3}$/.test(o.systemLocale)
        ? o.systemLocale
        : null,
    uiMode: oneOf(o.uiMode, ["simple", "advanced"], "simple"),
    reducedMotion: oneOf(o.reducedMotion, ["system", "on", "off"], "system"),
    transparency: oneOf(o.transparency, ["system", "on", "off"], "system"),
    onboardingDone:
      typeof o.onboardingDone === "boolean" ? o.onboardingDone : DEFAULT_UI_SETTINGS.onboardingDone,
    memoryMb: typeof o.memoryMb === "number" && o.memoryMb > 0 ? Math.round(o.memoryMb) : null,
    memory: {
      min,
      max,
      auto: Math.min(max, Math.max(min, whole(m.auto, d.auto))),
      total: typeof m.total === "number" && m.total > 0 ? Math.round(m.total) : null,
    },
    onLaunch: oneOf(o.onLaunch, ["embed", "keep_open", "hide"], "embed"),
    onExit: oneOf(o.onExit, ["show", "stay_hidden"], "show"),
    version:
      typeof o.version === "string" && /^[0-9A-Za-z.+-]{1,32}$/.test(o.version) ? o.version : "",
  };
}

function warn(what: string, e: unknown) {
  console.warn(`Clade settings: ${what} failed`, e);
}

function browserLanguage(): string | null {
  return typeof navigator !== "undefined" && typeof navigator.language === "string"
    ? navigator.language
    : null;
}

// ---------------------------------------------------------------------------------- loading

/** Reads the saved settings once at startup. Never rejects and never takes longer than the timeout. */
export async function loadUiSettings(): Promise<UiSettings> {
  if (!inTauri()) {
    // No launcher backend: the browser's language stands in for the Windows one.
    const systemLocale =
      current.systemLocale ?? parseUiSettings({ systemLocale: browserLanguage() }).systemLocale;
    setCurrent({ ...current, systemLocale });
    return current;
  }
  try {
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("timed out")), LOAD_TIMEOUT_MS),
    );
    setCurrent(parseUiSettings(await Promise.race([invoke("get_ui_settings"), timeout])));
    return current;
  } catch (e) {
    warn("loading", e);
    setCurrent({ ...DEFAULT_UI_SETTINGS });
    return current;
  }
}

// ----------------------------------------------------------------------- saving (fire and forget)

/** Saves a sound-preference change (fire and forget; the UI never waits on the disk). */
export function saveSoundSettings(patch: { enabled?: boolean; volume?: number }): void {
  if (!inTauri()) {
    setCurrent({
      ...current,
      soundEnabled: patch.enabled ?? current.soundEnabled,
      soundVolume:
        patch.volume === undefined
          ? current.soundVolume
          : parseUiSettings({ soundVolume: patch.volume }).soundVolume,
    });
    return;
  }
  invoke("set_sound_settings", { enabled: patch.enabled, volume: patch.volume }).catch(
    (e: unknown) => warn("saving sound settings", e),
  );
}

/** Records the headline that was just shown. */
export function saveLastPhrase(id: string): void {
  if (!inTauri()) {
    setCurrent({ ...current, lastPhrase: parseUiSettings({ lastPhrase: id }).lastPhrase });
    return;
  }
  invoke("set_last_phrase", { id }).catch((e: unknown) => warn("saving the last headline", e));
}

/**
 * Saves an explicit language choice (`data/settings.json`, `ui.language`). Resolves to false when
 * it could not be saved, so the caller can undo the switch and memory never disagrees with disk.
 */
export async function saveLanguage(language: Locale): Promise<boolean> {
  if (!inTauri()) {
    setCurrent({ ...current, language });
    return true;
  }
  try {
    await invoke("set_language", { language });
    return true;
  } catch (e) {
    warn("saving the language", e);
    return false;
  }
}

// ------------------------------------------------------------------ saving (reports the outcome)

/** The outcome of a change that can be refused: on failure `error` is the backend's coded error. */
export type SaveResult = { ok: true } | { ok: false; error: unknown };

async function change(
  command: string,
  args: Record<string, unknown>,
  local: Partial<UiSettings>,
): Promise<SaveResult> {
  if (!inTauri()) {
    setCurrent({ ...current, ...local });
    return { ok: true };
  }
  try {
    await invoke(command, args);
    return { ok: true };
  } catch (error) {
    warn(command, error);
    return { ok: false, error };
  }
}

export const saveUiMode = (mode: UiMode) => change("set_ui_mode", { mode }, { uiMode: mode });

export const saveReducedMotion = (mode: MotionMode) =>
  change("set_reduced_motion", { mode }, { reducedMotion: mode });

export const saveTransparency = (mode: TransparencyMode) =>
  change("set_transparency", { mode }, { transparency: mode });

/** `null` selects the automatic memory policy. */
export const saveMemory = (mb: number | null) => change("set_memory", { mb }, { memoryMb: mb });

export const saveBehavior = (patch: { onLaunch?: OnLaunch; onExit?: OnExit }) =>
  change(
    "set_behavior",
    { onLaunch: patch.onLaunch, onExit: patch.onExit },
    {
      ...(patch.onLaunch ? { onLaunch: patch.onLaunch } : {}),
      ...(patch.onExit ? { onExit: patch.onExit } : {}),
    },
  );

/** Marks the first-run introduction as finished or skipped, permanently. */
export const completeOnboarding = () => change("complete_onboarding", {}, { onboardingDone: true });

/** Puts the user-configurable preferences back to their defaults. */
export const resetUiSettings = () =>
  change(
    "reset_settings",
    {},
    {
      soundEnabled: DEFAULT_UI_SETTINGS.soundEnabled,
      soundVolume: DEFAULT_UI_SETTINGS.soundVolume,
      language: null,
      uiMode: "simple",
      reducedMotion: "system",
      transparency: "system",
      memoryMb: null,
      onLaunch: "embed",
      onExit: "show",
    },
  );

// ------------------------------------------------------------------------------- subscribing

/** Subscribes to backend-originated changes. Resolves to an unsubscribe function. */
export async function onUiSettingsChanged(callback: (s: UiSettings) => void): Promise<() => void> {
  if (!inTauri()) return () => {};
  try {
    return await listen<unknown>(SETTINGS_CHANGED_EVENT, (e) => {
      const next = parseUiSettings(e.payload);
      setCurrent(next);
      callback(next);
    });
  } catch (e) {
    warn("subscribing", e);
    return () => {};
  }
}
