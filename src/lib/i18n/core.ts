// The translation engine: pure functions, no React, no I/O.
//
// Messages are complete templates with named parameters (`{name}`); a sentence is never built by
// concatenating translated fragments, because word order and endings differ between languages.
// Counted nouns use plural forms chosen by `Intl.PluralRules` (Russian has one/few/many).

export type Locale = "en" | "ru";

/** Exactly the languages Clade ships. English is the canonical fallback. */
export const LOCALES: readonly Locale[] = ["en", "ru"];
export const FALLBACK_LOCALE: Locale = "en";

/** What the person sees in the language picker: each language in its own words. */
export const LANGUAGE_NAMES: Readonly<Record<Locale, string>> = {
  en: "English",
  ru: "Русский",
};

export type Params = Readonly<Record<string, string | number>>;

/** Plural forms selected by the numeric parameter named by `by` (default `count`). */
export interface PluralForms {
  by?: string;
  one?: string;
  few?: string;
  many?: string;
  other: string;
}

export type Message = string | PluralForms;
export type Catalog = Readonly<Record<string, Message>>;

export const isLocale = (v: unknown): v is Locale =>
  typeof v === "string" && (LOCALES as readonly string[]).includes(v);

/**
 * The language to use: an explicit choice always wins; otherwise the Windows language decides
 * (Russian if it starts with `ru`, English for everything else).
 */
export function resolveLocale(explicit: unknown, systemLocale: string | null | undefined): Locale {
  if (isLocale(explicit)) return explicit;
  return typeof systemLocale === "string" && /^ru([-_]|$)/i.test(systemLocale.trim()) ? "ru" : "en";
}

/** The BCP 47 tag used for number and date formatting. */
export const INTL_TAG: Readonly<Record<Locale, string>> = { en: "en-GB", ru: "ru-RU" };

const PLACEHOLDER = /\{([A-Za-z][A-Za-z0-9]*)\}/g;

/** Every placeholder name in a message (all plural forms included). Used by the parity tests. */
export function placeholders(message: Message): Set<string> {
  const texts = typeof message === "string" ? [message] : formsOf(message);
  const names = new Set<string>();
  for (const t of texts) for (const m of t.matchAll(PLACEHOLDER)) names.add(m[1]);
  if (typeof message !== "string") names.add(message.by ?? "count");
  return names;
}

function formsOf(m: PluralForms): string[] {
  return [m.one, m.few, m.many, m.other].filter((x): x is string => typeof x === "string");
}

function pickForm(locale: Locale, m: PluralForms, params: Params): string {
  const n = Number(params[m.by ?? "count"]);
  const category = Number.isFinite(n) ? new Intl.PluralRules(locale).select(n) : "other";
  switch (category) {
    case "one":
      return m.one ?? m.other;
    case "few":
      return m.few ?? m.other;
    case "many":
      return m.many ?? m.other;
    default:
      return m.other;
  }
}

export function fill(template: string, params: Params): string {
  return template.replace(PLACEHOLDER, (whole, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}

export function renderMessage(locale: Locale, message: Message, params: Params = {}): string {
  return fill(typeof message === "string" ? message : pickForm(locale, message, params), params);
}

/**
 * Looks a key up in `locale`'s catalog and falls back to English when it is missing, so an
 * incomplete translation shows English text instead of failing. A key missing everywhere is shown
 * as the key itself (visible in development, never a crash).
 */
export function translate(
  catalogs: Readonly<Record<Locale, Catalog>>,
  locale: Locale,
  key: string,
  params?: Params,
): string {
  const own = catalogs[locale][key];
  if (own !== undefined) return renderMessage(locale, own, params);
  const fallback = catalogs[FALLBACK_LOCALE][key];
  if (fallback !== undefined) return renderMessage(FALLBACK_LOCALE, fallback, params);
  return key;
}
