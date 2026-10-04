// Frontend side of the launcher's persistent UI state (sound switch, UI volume, last headline).
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
}

export const DEFAULT_UI_SETTINGS: Readonly<UiSettings> = {
  soundEnabled: true,
  soundVolume: 0.5,
  lastPhrase: null,
  language: null,
  systemLocale: null,
};

/** Name of the event the backend emits whenever a setting changes. */
export const SETTINGS_CHANGED_EVENT = "ui-settings-changed";

/** Starting the UI must never hang on the backend: after this, defaults are used. */
const LOAD_TIMEOUT_MS = 1500;

export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let memory: UiSettings = { ...DEFAULT_UI_SETTINGS };

/** Validates data coming from the backend instead of trusting its shape. */
export function parseUiSettings(raw: unknown): UiSettings {
  const o = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const volume =
    typeof o.soundVolume === "number" && Number.isFinite(o.soundVolume)
      ? Math.min(1, Math.max(0, o.soundVolume))
      : DEFAULT_UI_SETTINGS.soundVolume;
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
  };
}

function warn(what: string, e: unknown) {
  console.warn(`Clade settings: ${what} failed`, e);
}

/** Reads the saved settings once at startup. Never rejects and never takes longer than the timeout. */
export async function loadUiSettings(): Promise<UiSettings> {
  if (!inTauri())
    return {
      ...memory,
      // No launcher backend: the browser's language stands in for the Windows one.
      systemLocale:
        memory.systemLocale ?? parseUiSettings({ systemLocale: browserLanguage() }).systemLocale,
    };
  try {
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("timed out")), LOAD_TIMEOUT_MS),
    );
    return parseUiSettings(await Promise.race([invoke("get_ui_settings"), timeout]));
  } catch (e) {
    warn("loading", e);
    return { ...DEFAULT_UI_SETTINGS };
  }
}

/** Saves a sound-preference change (fire and forget; the UI never waits on the disk). */
export function saveSoundSettings(patch: { enabled?: boolean; volume?: number }): void {
  if (!inTauri()) {
    memory = {
      ...memory,
      soundEnabled: patch.enabled ?? memory.soundEnabled,
      soundVolume:
        patch.volume === undefined
          ? memory.soundVolume
          : parseUiSettings({ soundVolume: patch.volume }).soundVolume,
    };
    return;
  }
  invoke("set_sound_settings", { enabled: patch.enabled, volume: patch.volume }).catch(
    (e: unknown) => warn("saving sound settings", e),
  );
}

function browserLanguage(): string | null {
  return typeof navigator !== "undefined" && typeof navigator.language === "string"
    ? navigator.language
    : null;
}

/**
 * Saves an explicit language choice (`data/settings.json`, `ui.language`). Resolves to false when
 * it could not be saved, so the caller can undo the switch and memory never disagrees with disk.
 */
export async function saveLanguage(language: Locale): Promise<boolean> {
  if (!inTauri()) {
    memory = { ...memory, language };
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

/** Records the headline that was just shown. */
export function saveLastPhrase(id: string): void {
  if (!inTauri()) {
    memory = { ...memory, lastPhrase: parseUiSettings({ lastPhrase: id }).lastPhrase };
    return;
  }
  invoke("set_last_phrase", { id }).catch((e: unknown) => warn("saving the last headline", e));
}

/** Subscribes to backend-originated changes. Resolves to an unsubscribe function. */
export async function onUiSettingsChanged(callback: (s: UiSettings) => void): Promise<() => void> {
  if (!inTauri()) return () => {};
  try {
    return await listen<unknown>(SETTINGS_CHANGED_EVENT, (e) =>
      callback(parseUiSettings(e.payload)),
    );
  } catch (e) {
    warn("subscribing", e);
    return () => {};
  }
}
