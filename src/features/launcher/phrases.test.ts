import { describe, expect, it } from "vitest";
import { setLocale, type Locale } from "@/lib/i18n";
import {
  BASE_PHRASES,
  candidates,
  phraseText,
  pickPhrase,
  type Phrase,
  type PhraseRule,
} from "./phrases";

const IDS = [
  "ready-when-you-are",
  "your-world-is-waiting",
  "welcome-back",
  "pick-up-where-you-left-off",
  "where-to-today",
  "one-more-world",
  "take-your-time",
  "choose-your-world",
];

describe("phrase pool", () => {
  it("contains the curated phrases with unique, stable ids", () => {
    expect(BASE_PHRASES.map((x) => x.id)).toEqual(IDS);
    expect(new Set(BASE_PHRASES.map((x) => x.id)).size).toBe(BASE_PHRASES.length);
  });

  it("has the English wording", () => {
    expect(BASE_PHRASES.map(phraseText)).toEqual([
      "Ready when you are.",
      "Your world is waiting.",
      "Welcome back.",
      "Pick up where you left off.",
      "Where to today?",
      "One more world?",
      "Take your time.",
      "Choose your world.",
    ]);
  });

  it("has a curated Russian wording for every id", () => {
    setLocale("ru");
    const texts = BASE_PHRASES.map(phraseText);
    expect(texts).toContain("Начнём, когда захочешь.");
    expect(new Set(texts).size).toBe(BASE_PHRASES.length);
    for (const text of texts) expect(text).toMatch(/[А-Яа-яЁё]/); // never an English fallback
  });

  it.each<[Locale, RegExp]>([
    ["en", /^[A-Z][^!]*[.?]$/],
    ["ru", /^[А-ЯЁ][^!]*[.?]$/],
  ])(
    "keeps the tone calm in %s: short, sentence case, no exclamation marks or emoji",
    (l, shape) => {
      setLocale(l);
      for (const ph of BASE_PHRASES) {
        const text = phraseText(ph);
        expect(text.length).toBeLessThanOrEqual(30); // must fit on one line at the smallest window
        expect(text).toMatch(shape);
        expect(text).not.toMatch(/\p{Extended_Pictographic}/u);
      }
    },
  );

  it("is the same set of ids in both languages, so the remembered id is language-neutral", () => {
    for (const l of ["en", "ru"] as const) {
      setLocale(l);
      expect(BASE_PHRASES.map((ph) => ph.id)).toEqual(IDS);
    }
  });
});

describe("pickPhrase", () => {
  it("never repeats the previous phrase, whatever the random numbers are", () => {
    for (const prev of BASE_PHRASES) {
      for (let i = 0; i <= 100; i++) {
        const picked = pickPhrase(BASE_PHRASES, prev.id, () => i / 100);
        expect(picked.id).not.toBe(prev.id);
      }
    }
  });

  it("can reach every other phrase and handles rng edge values", () => {
    const reached = new Set<string>();
    for (let i = 0; i < 1000; i++) reached.add(pickPhrase(BASE_PHRASES, "welcome-back").id);
    expect(reached.size).toBe(BASE_PHRASES.length - 1);
    expect(() => pickPhrase(BASE_PHRASES, undefined, () => 0.999999999)).not.toThrow();
    expect(() => pickPhrase(BASE_PHRASES, undefined, () => 0)).not.toThrow();
  });

  it("copes with a pool of one and rejects an empty pool", () => {
    const only: Phrase[] = [{ id: "a", key: "phrase.a" }];
    expect(pickPhrase(only, "a").id).toBe("a");
    expect(() => pickPhrase([])).toThrow();
  });
});

describe("contextual rules (extension point, none shipped yet)", () => {
  const rule: PhraseRule = {
    id: "long-break",
    applies: (ctx) => (ctx.secondsSinceLastSession ?? 0) > 7 * 86400,
    phrases: (ctx) => [
      {
        id: "long-break-1",
        key: "phrase.long-break-1",
        params: { name: ctx.lastInstanceName ?? "" },
      },
    ],
  };

  it("ships with no contextual rules, so the pool is exactly the base pool", () => {
    expect(
      candidates({
        lastInstanceName: "Fabric 1.21.4",
        totalPlaytimeSeconds: 99,
        secondsSinceLastSession: 1e9,
      }),
    ).toEqual(BASE_PHRASES);
  });

  it("adds a rule's phrases only while its condition applies", () => {
    expect(candidates({ secondsSinceLastSession: 60 }, [rule]).length).toBe(BASE_PHRASES.length);
    const withRule = candidates(
      { secondsSinceLastSession: 8 * 86400, lastInstanceName: "Vanilla" },
      [rule],
    );
    expect(withRule.length).toBe(BASE_PHRASES.length + 1);
    expect(withRule.at(-1)?.id).toBe("long-break-1");
  });

  it("ignores duplicate ids", () => {
    const dup: PhraseRule = { id: "d", applies: () => true, phrases: () => [BASE_PHRASES[0]] };
    expect(candidates({}, [dup]).length).toBe(BASE_PHRASES.length);
  });
});
