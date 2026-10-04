// The one place the UI gets its text from.
//
// A tiny store holds the current language so every component (and every helper that formats a
// number or a date) agrees on it, and so switching language re-renders whatever is on screen
// without remounting anything. The language itself is launcher state and lives in the Rust
// settings service (`data/settings.json`); see `lib/settings/uiSettings.ts`. English is the
// canonical fallback: a key missing from the current language shows its English text.

import { useCallback, useSyncExternalStore } from "react";
import { CATALOGS, type MessageKey } from "@/locales";
import { FALLBACK_LOCALE, isLocale, translate, type Locale, type Params } from "./core";

export { LANGUAGE_NAMES, LOCALES, INTL_TAG, resolveLocale, isLocale } from "./core";
export type { Locale, Params } from "./core";
export type { MessageKey } from "@/locales";

let current: Locale = FALLBACK_LOCALE;
const listeners = new Set<() => void>();

export const getLocale = (): Locale => current;

/** Switches the language for the whole UI at once. Unknown codes are ignored. */
export function setLocale(next: Locale): void {
  if (!isLocale(next) || next === current) return;
  current = next;
  if (typeof document !== "undefined") document.documentElement.lang = next;
  for (const l of [...listeners]) l();
}

export function subscribeLocale(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Text for a compile-time-checked key, in the current language. */
export function t(key: MessageKey, params?: Params): string {
  return translate(CATALOGS, current, key, params);
}

/** Text for a key assembled at run time (`phrase.<id>`, `error.<domain>.<code>`). */
export function tDynamic(key: string, params?: Params): string {
  return translate(CATALOGS, current, key, params);
}

/** Whether the English catalog (the canonical one) has this key. */
export const hasMessage = (key: string): boolean => key in CATALOGS[FALLBACK_LOCALE];

/** The current language; a component using it re-renders when the language changes. */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribeLocale, getLocale, getLocale);
}

/**
 * `t` bound to the current language. Calling this hook is what makes a component update when the
 * language changes; the returned function changes identity with the language.
 */
export function useT(): (key: MessageKey, params?: Params) => string {
  const locale = useLocale();
  return useCallback((key, params) => translate(CATALOGS, locale, key, params), [locale]);
}
