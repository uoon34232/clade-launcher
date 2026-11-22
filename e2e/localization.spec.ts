import type { Locator, Page } from "@playwright/test";
import { BASE_PHRASES } from "@/features/launcher/phrases";
import type { en } from "@/locales/en";
import { ALEX, DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import {
  accountTrigger,
  dock,
  headline,
  playButton,
  profileTrigger,
  settingsButton,
  text,
  textRu,
} from "./support/ui";

const isBasePhraseId = (id: string | null): id is string =>
  id !== null && BASE_PHRASES.some((p) => p.id === id);

const PLAYTIME_EN = text("hero.played", {
  time: text("playtime.hoursMinutes", { hours: 1, minutes: 50 }),
});
const PLAYTIME_RU = textRu("hero.played", {
  time: textRu("playtime.hoursMinutes", { hours: 1, minutes: 50 }),
});

async function assertNoBrowserStorage(page: Page): Promise<void> {
  const stored = await page.evaluate(async () => ({
    local: localStorage.length,
    session: sessionStorage.length,
    databases: (await indexedDB.databases()).length,
    cookies: document.cookie,
  }));
  expect(stored).toEqual({ local: 0, session: 0, databases: 0, cookies: "" });
}

async function assertDialogFitsViewport(dialog: Locator, width = 960, height = 600): Promise<void> {
  const fit = await dialog.evaluate(
    (el, [maxW, maxH]) => ({
      horizontal: el.scrollWidth <= el.clientWidth + 1,
      withinWidth: el.clientWidth <= maxW,
      withinHeight: el.clientHeight <= maxH,
    }),
    [width, height] as const,
  );
  expect(fit).toEqual({ horizontal: true, withinWidth: true, withinHeight: true });
  const box = await dialog.boundingBox();
  expect(box).not.toBeNull();
  if (box) {
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(width);
    expect(box.y + box.height).toBeLessThanOrEqual(height);
  }
}

test.describe("language", () => {
  test("English -> Русский -> English updates the whole interface live, without a reload", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    // A marker that a page reload would erase: proves the switch never reloads the page.
    await page.evaluate(() => {
      (window as unknown as { __noReload: boolean }).__noReload = true;
    });

    // ---- English
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    await expect(page.getByText(PLAYTIME_EN)).toBeVisible();
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect.poll(async () => (await backend.state()).ui.lastPhrase).not.toBeNull();
    const phraseId = (await backend.state()).ui.lastPhrase;
    expect(isBasePhraseId(phraseId)).toBe(true);
    const phraseKey = `phrase.${phraseId}` as keyof typeof en;
    await expect(headline(page)).toHaveText(text(phraseKey));

    await settingsButton(page).click();
    const settings = page.getByRole("dialog", { name: text("settings.title") });
    await expect(settings).toBeVisible();

    // ---- Русский
    await settings.getByRole("radio", { name: "Русский" }).click();
    await expect(page.getByRole("dialog", { name: textRu("settings.title") })).toBeVisible();
    await expect(page.getByRole("switch", { name: textRu("settings.sound") })).toBeVisible();
    await expect(page.getByRole("button", { name: textRu("common.done") })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    // the language was saved through the backend, and nowhere else
    await expect.poll(async () => (await backend.state()).ui.language).toBe("ru");
    expect((await backend.calls("set_language")).map((c) => c.args)).toEqual([{ language: "ru" }]);
    await assertNoBrowserStorage(page);

    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(headline(page)).toHaveText(textRu(phraseKey));
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(page.getByText(PLAYTIME_RU)).toBeVisible();
    await expect(playButton(page)).toHaveText(textRu("play.play"));
    await expect(profileTrigger(page)).toHaveAccessibleName(
      textRu("profile.trigger").replace("{label}", "Survival"),
    );

    // Create profile reads in Russian too
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: textRu("profile.create") }).click();
    await expect(page.getByRole("dialog", { name: textRu("create.title") })).toBeVisible();
    await page.getByRole("button", { name: textRu("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // ---- back to English
    await settingsButton(page).click();
    await page.getByRole("radio", { name: "English" }).click();
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await page.getByRole("button", { name: text("common.done") }).click();
    await expect(headline(page)).toHaveText(text(phraseKey));
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    await expect(page.getByText(PLAYTIME_EN)).toBeVisible();
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect.poll(async () => (await backend.state()).ui.language).toBe("en");
    await assertNoBrowserStorage(page);

    expect(
      await page.evaluate(() => (window as unknown as { __noReload?: boolean }).__noReload),
    ).toBe(true);
  });

  test("reverts the UI, radio selection, html[lang], and backend state when set_language fails", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");

    await settingsButton(page).click();
    const settings = page.getByRole("dialog", { name: text("settings.title") });
    await expect(settings).toBeVisible();
    await expect(settings.getByRole("radio", { name: "English" })).toBeChecked();

    await backend.failNext("set_language", "storage");
    await settings.getByRole("radio", { name: "Русский" }).click();

    await expect
      .poll(async () => (await backend.calls("set_language")).map((c) => c.args))
      .toEqual([{ language: "ru" }]);
    await expect(settings).toBeVisible();
    await expect(settings.getByRole("radio", { name: "English" })).toBeChecked();
    await expect(settings.getByRole("radio", { name: "Русский" })).not.toBeChecked();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    expect((await backend.state()).ui.language).toBe("en");

    await settings.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    await expect(playButton(page)).toHaveText(text("play.play"));
  });

  test("an open Create profile dialog stays open, with its typed text, when the language changes", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect.poll(async () => (await backend.state()).ui.lastPhrase).not.toBeNull();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(dialog).toBeVisible();
    const name = dialog.getByLabel(text("create.name"));
    await name.fill("Weekend");
    await expect(name).toBeFocused();

    // The backend announces a language change (as it does for any change made elsewhere).
    const liveUi = (await backend.state()).ui;
    expect(liveUi.lastPhrase).not.toBeNull();
    await backend.emit("ui-settings-changed", { ...liveUi, language: "ru" });

    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    const russian = page.getByRole("dialog", { name: textRu("create.title") });
    await expect(russian).toBeVisible();
    const nameRu = russian.getByLabel(textRu("create.name"));
    await expect(nameRu).toHaveValue("Weekend");
    await expect(nameRu).toBeFocused();
  });
});

test.describe("first run on a Russian Windows", () => {
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: null, systemLocale: "ru-RU" } },
  });

  test("follows the Windows language until a language is chosen", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(page.getByText(PLAYTIME_RU)).toBeVisible();
    await expect(playButton(page)).toHaveText(textRu("play.play"));
    await expect(dock(page)).toBeVisible();
  });
});

test.describe("systemLocale variants without an explicit choice", () => {
  test.describe("underscore Russian locale (ru_RU)", () => {
    test.use({
      backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: null, systemLocale: "ru_RU" } },
    });

    test("resolves to Russian", async ({ page, openApp }) => {
      await openApp();
      await expect(page.locator("html")).toHaveAttribute("lang", "ru");
      await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
      await expect(page.getByText(PLAYTIME_RU)).toBeVisible();
      await expect(playButton(page)).toHaveText(textRu("play.play"));
    });
  });

  test.describe("non-Russian locale (de-DE)", () => {
    test.use({
      backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: null, systemLocale: "de-DE" } },
    });

    test("falls back to English", async ({ page, openApp }) => {
      await openApp();
      await expect(page.locator("html")).toHaveAttribute("lang", "en");
      await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
      await expect(page.getByText(PLAYTIME_EN)).toBeVisible();
      await expect(playButton(page)).toHaveText(text("play.play"));
    });
  });
});

test.describe("an explicit English choice on a Russian Windows", () => {
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: "en", systemLocale: "ru-RU" } },
  });

  test("is never overridden by the system language", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
  });

  test("resetting settings clears the explicit choice and falls back to Russian", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");

    await settingsButton(page).click();
    const settings = page.getByRole("dialog", { name: text("settings.title") });
    await expect(settings).toBeVisible();

    await settings.getByRole("button", { name: text("settings.reset") }).click();
    await expect(settings.getByText(text("settings.resetConfirm"))).toBeVisible();
    await settings.getByRole("button", { name: text("settings.resetDo") }).click();

    await expect(page.getByRole("dialog", { name: textRu("settings.title") })).toBeVisible();
    await expect(page.getByRole("radio", { name: "Русский" })).toBeChecked();
    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    await expect.poll(async () => (await backend.state()).ui.language).toBeNull();
    expect(await backend.calls("reset_settings")).toHaveLength(1);

    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(page.getByText(PLAYTIME_RU)).toBeVisible();
    await expect(playButton(page)).toHaveText(textRu("play.play"));
  });
});

test.describe("Russian layout at the smallest 960x600 window", () => {
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: "ru" } },
  });

  test("fits Settings and Account dialogs cleanly without horizontal overflow", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    const pageFits = await page.evaluate(
      () => document.documentElement.scrollWidth <= document.documentElement.clientWidth,
    );
    expect(pageFits).toBe(true);

    await settingsButton(page).click();
    const settings = page.getByRole("dialog", { name: textRu("settings.title") });
    await expect(settings).toBeVisible();
    const settingsFit = await settings.evaluate((el) => ({
      vertical: el.scrollHeight <= el.clientHeight + 1,
      horizontal: el.scrollWidth <= el.clientWidth + 1,
    }));
    expect(settingsFit).toEqual({ vertical: true, horizontal: true });
    await settings.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: textRu("account.manage", { name: ALEX.username }) })
      .click();
    const account = page.getByRole("dialog", { name: ALEX.username });
    await expect(account).toBeVisible();
    // Wait for async `account_appearance` to settle before measuring layout dimensions.
    await expect(account.getByText(textRu("appearance.noCapes"))).toBeVisible();
    // AccountDialog uses Modal's `max-h-[calc(100vh-2rem)] overflow-y-auto`, so verify the
    // dialog stays inside the 960x600 viewport with zero horizontal overflow.
    await assertDialogFitsViewport(account);
    await account.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test.describe("with owned capes", () => {
    test.use({
      backendConfig: {
        ui: { ...DEFAULT_CONFIG.ui, language: "ru" },
        appearance: {
          [ALEX.id]: {
            skinModel: "classic",
            capes: [
              { id: "11111111-2222-4333-8444-555555555555", alias: "Migrator", active: true },
              { id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", alias: null, active: false },
            ],
          },
        },
      },
    });

    test("fits Account dialog with capes cleanly without horizontal overflow", async ({
      page,
      openApp,
    }) => {
      await page.setViewportSize({ width: 960, height: 600 });
      await openApp();

      await accountTrigger(page).click();
      await page
        .getByRole("menuitem", { name: textRu("account.manage", { name: ALEX.username }) })
        .click();
      const account = page.getByRole("dialog", { name: ALEX.username });
      await expect(account).toBeVisible();
      await expect(account.getByText(textRu("accountModal.capeAvailable"))).toBeVisible();
      await expect(account.getByRole("listitem")).toHaveCount(2);
      await expect(
        account.getByRole("button", { name: textRu("appearance.hideCape") }),
      ).toBeVisible();

      await assertDialogFitsViewport(account);
      const listHorizontalFit = await account.evaluate((el) => {
        const list = el.querySelector("ul");
        return list ? list.scrollWidth <= list.clientWidth + 1 : false;
      });
      expect(listHorizontalFit).toBe(true);

      await account.getByRole("button", { name: textRu("common.done") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });
  });
});
