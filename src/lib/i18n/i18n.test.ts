/// <reference types="node" />
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { formatBytes, formatPlaytime, itemsText, staleNote } from "@/features/launcher/profileUi";
import { CATALOGS, en, ru } from "@/locales";
import {
  fill,
  placeholders,
  renderMessage,
  resolveLocale,
  translate,
  type Catalog,
  type Locale,
} from "./core";
import { describeCode, describeError } from "./errors";
import { BackendError } from "@/lib/ipc";
import { getLocale, hasMessage, setLocale, subscribeLocale, t, tDynamic } from "./index";

afterEach(() => setLocale("en"));

const enKeys = Object.keys(en).sort();
const ruKeys = Object.keys(ru).sort();

describe("catalog parity (English is canonical, Russian is complete)", () => {
  it("has exactly the same keys in both languages", () => {
    expect(
      ruKeys.filter((k) => !(k in en)),
      "keys only in Russian",
    ).toEqual([]);
    expect(
      enKeys.filter((k) => !(k in ru)),
      "keys missing from Russian",
    ).toEqual([]);
    expect(ruKeys).toEqual(enKeys);
  });

  it("has no duplicate keys (a duplicate would silently shadow the first)", () => {
    // Object literals collapse duplicates, so scan the source text instead.
    for (const file of ["en", "ru"]) {
      const text = readFileSync(`src/locales/${file}.ts`, "utf8");
      const keys = [...text.matchAll(/^ {2}"([^"]+)":/gm)].map((m) => m[1]);
      const dupes = keys.filter((k, i) => keys.indexOf(k) !== i);
      expect(dupes, `${file}.ts`).toEqual([]);
      expect(keys.length).toBe(enKeys.length);
    }
  });

  it("uses the same named parameters in both languages for every key", () => {
    for (const key of enKeys) {
      const a = [...placeholders(en[key as keyof typeof en])].sort();
      const b = [...placeholders(ru[key as keyof typeof ru])].sort();
      expect(b, key).toEqual(a);
    }
  });

  it("leaves no Russian entry empty or still in English (except where English is also empty or a product name)", () => {
    const sameAllowed = new Set([
      "accountModal.uuid", // a technical label
      "create.channel.stable", // intentionally blank
    ]);
    for (const key of enKeys) {
      const ruMsg = ru[key as keyof typeof ru];
      const enMsg = en[key as keyof typeof en];
      if (sameAllowed.has(key)) continue;
      const ruText = typeof ruMsg === "string" ? ruMsg : ruMsg.other;
      const enText = typeof enMsg === "string" ? enMsg : enMsg.other;
      expect(ruText.length, key).toBeGreaterThan(0);
      expect(/[А-Яа-яЁё]/.test(ruText) || ruText === enText, `${key}: ${ruText}`).toBe(true);
      if (ruText === enText) expect(/^[^A-Za-z]*$|^\{[a-z]+\}/.test(ruText), key).toBe(true);
    }
  });

  it("keeps product names as they are, never transliterated", () => {
    for (const [key, msg] of Object.entries(ru)) {
      const text = typeof msg === "string" ? msg : JSON.stringify(msg);
      expect(text, key).not.toMatch(/Клейд|Клэйд|Фабрик|Фордж|Неофордж|Квилт|Майнкрафт/);
    }
  });
});

describe("the translator", () => {
  it("fills named parameters from complete templates", () => {
    expect(fill("Hello {name}, {n} left", { name: "Alex", n: 3 })).toBe("Hello Alex, 3 left");
    expect(fill("Keep {unknown}", {})).toBe("Keep {unknown}");
    expect(t("account.trigger", { name: "Alex" })).toBe("Account: Alex. Switch account");
    setLocale("ru");
    expect(t("account.trigger", { name: "Alex" })).toBe("Аккаунт: Alex. Сменить аккаунт");
  });

  it("picks Russian plural forms (one / few / many) for counted nouns", () => {
    const files = (total: number) => renderMessage("ru", ru["prepare.files"], { done: 1, total });
    expect(files(1)).toBe("1 из 1 файла");
    expect(files(21)).toBe("1 из 21 файла");
    expect(files(2)).toBe("1 из 2 файлов");
    expect(files(5)).toBe("1 из 5 файлов");
    expect(files(640)).toBe("1 из 640 файлов");
    expect(files(11)).toBe("1 из 11 файлов");
    expect(renderMessage("en", en["prepare.files"], { done: 412, total: 640 })).toBe(
      "412 of 640 files",
    );
    const chars = (count: number) => renderMessage("ru", ru["signin.passphraseNote"], { count });
    expect(chars(8)).toBe("Не менее 8 символов. Подойдёт несколько слов.");
    expect(chars(21)).toContain("21 символа");
    expect(t("profile.unreadableNotice", { count: 1, folders: "old" })).toBe(
      "1 profile folder couldn’t be read (old). Its files were left untouched.",
    );
    expect(t("profile.unreadableNotice", { count: 2, folders: "old, broken" })).toBe(
      "2 profile folders couldn’t be read (old, broken). Their files were left untouched.",
    );
    setLocale("ru");
    expect(t("profile.unreadableNotice", { count: 1, folders: "old" })).toBe(
      "Не удалось прочитать 1 папку профиля (old). Файлы не затронуты.",
    );
    expect(t("profile.unreadableNotice", { count: 2, folders: "a, b" })).toBe(
      "Не удалось прочитать 2 папки профилей (a, b). Файлы не затронуты.",
    );
    expect(t("profile.unreadableNotice", { count: 5, folders: "a, b" })).toBe(
      "Не удалось прочитать 5 папок профилей (a, b). Файлы не затронуты.",
    );
    expect(t("profile.unreadableNotice", { count: 21, folders: "a, b" })).toBe(
      "Не удалось прочитать 21 папку профиля (a, b). Файлы не затронуты.",
    );
  });

  it("falls back to English for a key missing in the current language, never crashing", () => {
    const partial: Record<Locale, Catalog> = {
      en: { "only.en": "English text", both: "Both EN" },
      ru: { both: "Оба RU" },
    };
    expect(translate(partial, "ru", "only.en")).toBe("English text");
    expect(translate(partial, "ru", "both")).toBe("Оба RU");
    expect(translate(partial, "en", "both")).toBe("Both EN");
    // Missing everywhere: the key itself is shown (visible in development), no exception.
    expect(translate(partial, "ru", "nowhere")).toBe("nowhere");
    expect(CATALOGS.en).toBe(en);
    expect(hasMessage("hero.subtitle")).toBe(true);
    expect(hasMessage("hero.nope")).toBe(false);
    expect(tDynamic("hero.nope")).toBe("hero.nope");
  });

  it("switches the whole UI language at once and tells subscribers", () => {
    let calls = 0;
    const off = subscribeLocale(() => calls++);
    setLocale("ru");
    expect(getLocale()).toBe("ru");
    expect(document.documentElement.lang).toBe("ru");
    setLocale("ru"); // no change: no notification
    setLocale("en");
    expect(calls).toBe(2);
    expect(document.documentElement.lang).toBe("en");
    // @ts-expect-error unknown languages are refused
    setLocale("de");
    expect(getLocale()).toBe("en");
    off();
  });
});

describe("first-run language", () => {
  it("follows the Windows language when nothing was ever chosen", () => {
    expect(resolveLocale(null, "ru-RU")).toBe("ru");
    expect(resolveLocale(undefined, "ru")).toBe("ru");
    expect(resolveLocale(null, "RU-ru")).toBe("ru");
    expect(resolveLocale(null, "en-US")).toBe("en");
    expect(resolveLocale(null, "de-DE")).toBe("en");
    expect(resolveLocale(null, "uk-UA")).toBe("en"); // only "ru…" is Russian
    expect(resolveLocale(null, "rue")).toBe("en"); // a different language that merely starts with r-u
    expect(resolveLocale(null, null)).toBe("en");
    expect(resolveLocale(null, "")).toBe("en");
  });

  it("an explicit choice always wins over the system language", () => {
    expect(resolveLocale("en", "ru-RU")).toBe("en");
    expect(resolveLocale("ru", "en-US")).toBe("ru");
    expect(resolveLocale("de", "ru-RU")).toBe("ru"); // not a shipped language: treated as no choice
  });
});

describe("locale-aware formatting (raw numbers in, text out)", () => {
  it("formats playtime in English", () => {
    expect(formatPlaytime(0)).toBe("0m");
    expect(formatPlaytime(30)).toBe("<1m");
    expect(formatPlaytime(42 * 60)).toBe("42m");
    expect(formatPlaytime(27 * 3600 + 42 * 60)).toBe("27h 42m");
    expect(t("hero.played", { time: formatPlaytime(27 * 3600 + 42 * 60) })).toBe("Played 27h 42m");
  });

  it("formats playtime in Russian: minutes only, hours and minutes, large totals", () => {
    setLocale("ru");
    expect(formatPlaytime(0)).toBe("0 мин");
    expect(formatPlaytime(30)).toBe("<1 мин");
    expect(formatPlaytime(42 * 60)).toBe("42 мин");
    expect(formatPlaytime(3600)).toBe("1 ч 0 мин");
    expect(formatPlaytime(27 * 3600 + 42 * 60)).toBe("27 ч 42 мин");
    expect(formatPlaytime(1234 * 3600 + 5 * 60)).toBe("1234 ч 5 мин");
    expect(t("hero.played", { time: formatPlaytime(27 * 3600 + 42 * 60) })).toBe(
      "В игре 27 ч 42 мин",
    );
  });

  it("formats sizes with the language's units and decimal mark", () => {
    expect(formatBytes(184 * 1024 * 1024)).toBe("184 MB");
    expect(formatBytes(1.4 * 1024 * 1024 * 1024)).toBe("1.4 GB");
    setLocale("ru");
    expect(formatBytes(184 * 1024 * 1024)).toBe("184 МБ");
    expect(formatBytes(1.4 * 1024 * 1024 * 1024)).toBe("1,4 ГБ");
    expect(formatBytes(512)).toBe("1 КБ");
    expect(itemsText(412, 640)).toBe("412 из 640 файлов");
    expect(itemsText(null, 640)).toBeNull();
  });

  it("words the saved-list notice with a localized date", () => {
    const now = Date.UTC(2026, 9, 4);
    expect(staleNote(Date.UTC(2026, 9, 3) / 1000, now)).toMatch(
      /^Offline: using the version list from 3 Oct/,
    );
    setLocale("ru");
    const ruNote = staleNote(Date.UTC(2026, 9, 3) / 1000, now);
    expect(ruNote).toMatch(/^Нет сети: используется список версий от 3 окт/);
    expect(staleNote(0)).toBe("Нет сети: используется сохранённый список версий.");
    // A short month that ends in a dot ("сент.") must not produce a double full stop.
    const sep = staleNote(Date.UTC(2026, 8, 26) / 1000, now);
    expect(sep).toMatch(/от 26 сент\.$/);
    expect(sep).not.toMatch(/\.\.$/);
  });
});

describe("structured backend errors are worded in the current language", () => {
  const disk = new BackendError("English detail for logs", "disk_space", {
    params: { requiredBytes: 3 * 1024 ** 3, availableBytes: 200 * 1024 ** 2 },
  });

  it("builds the disk-space message from byte counts, in both languages", () => {
    expect(describeError("prepare", disk)).toBe(
      "There isn't enough free space on the drive that holds Clade: about 3.0 GB is needed and 200 MB is available. Free some space and try again.",
    );
    setLocale("ru");
    expect(describeError("prepare", disk)).toBe(
      "На диске, где находится Clade, не хватает места: нужно около 3,0 ГБ, доступно 200 МБ. Освободи место и повтори.",
    );
  });

  it("never shows the backend's English detail message", () => {
    setLocale("ru");
    const text = describeError("prepare", disk);
    expect(text).not.toContain("English detail");
    expect(describeError("launch", new Error("Something raw and technical"))).toBe(
      "Не удалось запустить Minecraft.",
    );
  });

  it("words each domain's own codes, with parameters", () => {
    expect(
      describeError(
        "profile",
        new BackendError("x", "duplicate_name", { params: { name: "Survival" } }),
      ),
    ).toBe("A profile named “Survival” already exists.");
    expect(describeCode("profile", "invalid_memory", { min: 1024, max: 12288 })).toBe(
      "That amount of memory isn't safe for this computer. Choose between 1 GB and 12 GB.",
    );
    expect(
      describeCode("profile", "invalid_resolution", {
        minWidth: 320,
        maxWidth: 7680,
        minHeight: 240,
        maxHeight: 4320,
      }),
    ).toBe("Choose a window size between 320×240 and 7680×4320.");
    expect(describeCode("profile", "invalid_jvm_args")).toMatch(/safe JVM flags/);
    expect(describeCode("profile", "open_failed")).toBe("Windows couldn't open that folder.");
    setLocale("ru");
    expect(
      describeError(
        "profile",
        new BackendError("x", "no_compatible_loader", {
          params: { loader: "Fabric", minecraft: "1.20.1" },
        }),
      ),
    ).toBe("Для Minecraft 1.20.1 нет версии Fabric.");
    expect(describeCode("profile", "invalid_memory", { min: 1024, max: 12288 })).toBe(
      "Такой объём памяти небезопасен для этого компьютера. Выбери от 1 ГБ до 12 ГБ.",
    );
    expect(
      describeCode("profile", "invalid_resolution", {
        minWidth: 320,
        maxWidth: 7680,
        minHeight: 240,
        maxHeight: 4320,
      }),
    ).toBe("Выбери размер окна от 320×240 до 7680×4320.");
    expect(describeCode("profile", "invalid_jvm_args")).toMatch(/безопасные флаги JVM/);
    expect(describeCode("profile", "open_failed")).toBe("Windows не смог открыть эту папку.");
    expect(describeCode("launch", "needs_sign_in")).toBe(
      "Этому аккаунту нужно войти снова, чтобы играть.",
    );
  });

  it("tells the two Java-profile situations apart and treats a code as domain-specific", () => {
    expect(describeCode("signIn", "no_java_profile", { ownsMinecraft: true })).toMatch(
      /owns Minecraft but has no Java profile/,
    );
    expect(describeCode("signIn", "no_java_profile", { ownsMinecraft: false })).toMatch(
      /does not own Minecraft: Java Edition/,
    );
    // "offline" means different things when launching, preparing and signing in.
    const texts = new Set(
      (["launch", "prepare", "signIn"] as const).map((d) => describeCode(d, "offline")),
    );
    expect(texts.size).toBe(3);
  });

  it("falls back to the domain's general message, then a generic one, for unknown codes", () => {
    expect(describeCode("signIn", "something_new")).toBe("Sign-in failed.");
    expect(describeCode("prepare", null)).toBe("Setup failed.");
    expect(describeCode("catalog", "storage")).toMatch(/couldn't read or write/);
    setLocale("ru");
    expect(describeCode("vault", "never_heard_of_it")).toBe("Не получилось.");
  });

  it("words catalog, sign-in, language and folder-path codes in both languages", () => {
    expect(describeCode("catalog", "catalog_unavailable")).toBe(
      "Could not load the version list. Check your connection and try again.",
    );
    expect(describeCode("signIn", "sign_in_unavailable")).toBe(
      "Microsoft sign-in isn't available in this build yet.",
    );
    expect(describeCode("settings", "unsupported_language")).toBe("That language isn't supported.");
    expect(describeCode("launch", "unsupported_folder_path")).toBe(
      "Clade can't run from this folder path. Move the Clade folder to a local drive path without special device prefixes and try again.",
    );
    expect(describeCode("settings", "unsupported_folder_path")).toBe(
      "Clade can't run from this folder path. Move the Clade folder to a local drive path without special device prefixes and try again.",
    );

    setLocale("ru");
    expect(describeCode("catalog", "catalog_unavailable")).toBe(
      "Не удалось загрузить список версий. Проверь подключение и повтори.",
    );
    expect(describeCode("signIn", "sign_in_unavailable")).toBe(
      "Вход через Microsoft пока недоступен в этой сборке.",
    );
    expect(describeCode("settings", "unsupported_language")).toBe("Этот язык не поддерживается.");
    expect(describeCode("launch", "unsupported_folder_path")).toBe(
      "Clade не может работать из этой папки. Перемести папку Clade на обычный локальный диск и попробуй снова.",
    );
    expect(describeCode("settings", "unsupported_folder_path")).toBe(
      "Clade не может работать из этой папки. Перемести папку Clade на обычный локальный диск и попробуй снова.",
    );
  });

  it("has wording for every code the Rust backend can send", () => {
    const dir = join(process.cwd(), "src-tauri", "src");
    const sources = readdirSync(dir, { recursive: true })
      .map(String)
      .filter((f) => f.endsWith(".rs") && !f.includes("tests"))
      .map((f) => readFileSync(join(dir, f), "utf8"));
    const text = sources.join("\n");
    const wanted = (domain: string, codes: string[]) =>
      codes.filter((c) => !hasMessage(`error.${domain}.${c}`) && !hasMessage(`error.common.${c}`));

    const launch = [...text.matchAll(/\bfail\(\s*"([a-z_]+)"/g)].map((m) => m[1]);
    expect(wanted("launch", [...new Set(launch)])).toEqual([]);

    const authBlock = text.slice(text.indexOf("pub fn code(&self)"), text.indexOf("fn network("));
    const auth = [...authBlock.matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]);
    expect(auth.length).toBeGreaterThan(10);
    expect(
      wanted(
        "signIn",
        auth.filter((c) => c !== "cancelled"),
      ),
    ).toEqual([]);

    const prepBlock = text.slice(
      text.indexOf("impl PrepareError"),
      text.indexOf("impl From<DownloadError>"),
    );
    const prep = [...prepBlock.matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]);
    expect(prep).toContain("disk_space");
    expect(
      wanted(
        "prepare",
        prep.filter((c) => c !== "cancelled"),
      ),
    ).toEqual([]);

    const coded = [...text.matchAll(/CodedError::new\(\s*"([a-z_]+)"/g)].map((m) => m[1]);
    const known = new Set([
      ...[
        "duplicate_name",
        "unknown_minecraft",
        "cannot_verify",
        "cannot_verify_loader",
        "no_compatible_loader",
        "loader_version_not_offered",
        "invalid_name",
        "catalog_unavailable",
        "network_unavailable",
        "not_found",
        "storage",
        "open_failed",
        "unsupported_language",
        "unsupported_folder_path",
        "unknown_account",
        "sign_in_unavailable",
        "unsupported_value",
        "invalid_memory",
        "invalid_resolution",
        "invalid_jvm_args",
        "diagnostics_failed",
        // removing or duplicating a profile
        "already_running",
        "busy",
        "remove_failed",
        "remove_refused",
        "duplicate_failed",
      ],
      ...[
        "passphrase_mismatch",
        "passphrase_too_short",
        "passphrase_too_long",
        "vault_exists",
        "vault_unreadable",
        "vault_missing",
      ],
    ]);
    for (const c of new Set(coded)) expect(known.has(c), `unknown code ${c}`).toBe(true);
    const profileCodes = [
      "duplicate_name",
      "unknown_minecraft",
      "cannot_verify",
      "cannot_verify_loader",
      "no_compatible_loader",
      "loader_version_not_offered",
      "invalid_name",
      "invalid_memory",
      "invalid_resolution",
      "invalid_jvm_args",
      "open_failed",
      "already_running",
      "busy",
      "remove_failed",
      "remove_refused",
      "not_found",
      "duplicate_failed",
    ];
    expect(wanted("profile", profileCodes)).toEqual([]);
    // "Start fresh" (vault_recovery.rs) and the unlock errors
    expect(
      wanted("vault", [
        "passphrase_mismatch",
        "passphrase_too_short",
        "passphrase_too_long",
        "vault_unreadable",
        "vault_missing",
        "sign_in_in_progress",
        "appearance_busy",
        "game_starting",
        "vault_unlocked",
        "reset_failed",
        "accounts_not_updated",
        "storage",
      ]),
    ).toEqual([]);
    expect(
      wanted("settings", [
        "unsupported_value",
        "unsupported_language",
        "unsupported_folder_path",
        "invalid_memory",
        "diagnostics_failed",
        "open_failed",
        "storage",
      ]),
    ).toEqual([]);
    expect(wanted("catalog", ["catalog_unavailable"])).toEqual([]);
    expect(wanted("signIn", ["sign_in_unavailable"])).toEqual([]);
    expect(
      wanted("vault", [
        "passphrase_mismatch",
        "passphrase_too_short",
        "passphrase_too_long",
        "vault_exists",
        "vault_unreadable",
      ]),
    ).toEqual([]);

    const reasons = [...text.matchAll(/\("[^"]+", "([a-z_]+)", "/g)].map((m) => m[1]);
    expect(reasons.length).toBeGreaterThan(8);
    for (const r of new Set([...reasons, "early_exit", "unexpected", "stopped"])) {
      expect(hasMessage(`exit.${r}`), `exit.${r}`).toBe(true);
    }
  });
});
