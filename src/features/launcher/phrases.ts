// The home screen's headline messages.
//
// Tone: short, calm, understated, editorial. No jokes, gamer slang, exclamation marks or
// motivational quotes. Everything is local, with no network and no remote copy.
//
// A phrase is an id plus a translation key (`phrase.<id>` in `src/locales`), never finished text:
// the id is what is remembered (`ui.last_phrase`) and compared, so it means the same thing in
// every language and switching language never corrupts it. The wording is curated separately for
// each language, not translated word for word.
//
// Contextual messages (last played instance, total playtime, time since the last session) are
// not implemented yet, but the structure is ready: add a `PhraseRule` to `CONTEXT_RULES` and it
// joins the pool whenever its context applies. Nothing else needs to change.

import { tDynamic, type Params } from "@/lib/i18n";

export interface Phrase {
  /** Stable id, used to avoid repeating a phrase and to remember the last one shown. */
  readonly id: string;
  /** Translation key of the wording, and any named parameters it needs. */
  readonly key: string;
  readonly params?: Params;
}

/** The phrase's wording in the current language. */
export const phraseText = (ph: Phrase): string => tDynamic(ph.key, ph.params);

/** Facts a contextual rule may use. All optional: a rule must cope with missing data. */
export interface PhraseContext {
  /** Display name of the instance played last. */
  readonly lastInstanceName?: string;
  /** Lifetime playtime across all instances, in seconds. */
  readonly totalPlaytimeSeconds?: number;
  /** Seconds since the last session ended. */
  readonly secondsSinceLastSession?: number;
}

/** A conditional source of phrases, e.g. "Welcome back to {instance}." after a long break. */
export interface PhraseRule {
  readonly id: string;
  applies(ctx: PhraseContext): boolean;
  phrases(ctx: PhraseContext): readonly Phrase[];
}

const p = (id: string): Phrase => ({ id, key: `phrase.${id}` });

/** The curated, always-available pool. Each wording must be short enough to sit on one line. */
export const BASE_PHRASES: readonly Phrase[] = [
  p("ready-when-you-are"),
  p("your-world-is-waiting"),
  p("welcome-back"),
  p("pick-up-where-you-left-off"),
  p("where-to-today"),
  p("one-more-world"),
  p("take-your-time"),
  p("choose-your-world"),
];

/** No contextual rules yet. Planned: last played instance, total playtime, time since last session. */
export const CONTEXT_RULES: readonly PhraseRule[] = [];

/** Everything that may be shown right now: the base pool plus any rule that applies. */
export function candidates(
  ctx: PhraseContext = {},
  rules: readonly PhraseRule[] = CONTEXT_RULES,
  base: readonly Phrase[] = BASE_PHRASES,
): readonly Phrase[] {
  const extra = rules.filter((r) => r.applies(ctx)).flatMap((r) => r.phrases(ctx));
  const seen = new Set<string>();
  return [...base, ...extra].filter((ph) => (seen.has(ph.id) ? false : (seen.add(ph.id), true)));
}

/**
 * Picks a phrase uniformly at random, never the one with id `previousId` (unless it is the only
 * choice). `rng` returns a number in [0, 1) and is injectable for deterministic tests.
 */
export function pickPhrase(
  pool: readonly Phrase[],
  previousId?: string,
  rng: () => number = Math.random,
): Phrase {
  if (pool.length === 0) throw new Error("phrase pool is empty");
  const options = pool.length > 1 ? pool.filter((ph) => ph.id !== previousId) : pool;
  const index = Math.min(options.length - 1, Math.floor(rng() * options.length));
  return options[index];
}
