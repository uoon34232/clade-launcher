import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, headline, playButton, profileTrigger, text, textRu } from "./support/ui";

const firstRun = { ui: { ...DEFAULT_CONFIG.ui, onboardingDone: false } };

const welcome = (page: Page) => page.getByRole("dialog", { name: text("onboarding.welcomeTitle") });

test.describe("first-run introduction", () => {
  test.use({ backendConfig: firstRun });

  test("appears on the first run with the welcome page", async ({ page, openApp }) => {
    await openApp();
    const dialog = welcome(page);
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(text("onboarding.tagline"))).toBeVisible();
    await expect(dialog.getByText(text("onboarding.welcomeText"))).toBeVisible();
    await expect(dialog.getByText(text("onboarding.step", { current: 1, total: 3 }))).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("onboarding.continue") })).toBeFocused();
    await expect(dialog.getByRole("button", { name: text("onboarding.skip") })).toBeVisible();
    // the first page has nothing to go back to
    await expect(
      dialog.getByRole("button", { name: text("onboarding.back"), exact: true }),
    ).toHaveCount(0);
  });

  test("Continue and Back move through the three pages", async ({ page, openApp }) => {
    await openApp();
    const next = () => page.getByRole("button", { name: text("onboarding.continue") });
    const back = () => page.getByRole("button", { name: text("onboarding.back"), exact: true });

    await next().click();
    const portable = page.getByRole("dialog", { name: text("onboarding.portableTitle") });
    await expect(portable).toBeVisible();
    await expect(portable.getByText(text("onboarding.portableText"))).toBeVisible();
    await expect(
      portable.getByText(text("onboarding.step", { current: 2, total: 3 })),
    ).toBeVisible();

    await back().click();
    await expect(welcome(page)).toBeVisible();

    await next().click();
    await expect(portable).toBeVisible();
    await next().click();
    const ready = page.getByRole("dialog", { name: text("onboarding.readyTitle") });
    await expect(ready).toBeVisible();
    await expect(ready.getByText(text("onboarding.step", { current: 3, total: 3 }))).toBeVisible();
    await expect(
      ready.getByRole("button", { name: text("onboarding.createProfile") }),
    ).toBeVisible();
    await expect(ready.getByRole("button", { name: text("onboarding.addAccount") })).toBeVisible();
    await expect(ready.getByRole("button", { name: text("onboarding.finish") })).toBeVisible();
    // Nothing to continue to, and no way to skip what is already the last page
    await expect(ready.getByRole("button", { name: text("onboarding.continue") })).toHaveCount(0);
    await expect(ready.getByRole("button", { name: text("onboarding.skip") })).toHaveCount(0);
  });

  test("Skip ends it for good and is saved through the backend", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.skip") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(headline(page)).toBeVisible();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    expect((await backend.state()).ui.onboardingDone).toBe(true);
  });

  test("Skip can be used from the second page too", async ({ page, backend, openApp }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.skip") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
  });

  test("Finish ends it for good and is saved through the backend", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.finish") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    expect((await backend.state()).ui.onboardingDone).toBe(true);
  });

  test("Escape counts as Skip, and a stray click outside does nothing", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.mouse.click(8, 8);
    await expect(welcome(page)).toBeVisible();
    expect(await backend.calls("complete_onboarding")).toHaveLength(0);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
  });

  test("once finished it does not come back when settings change", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.skip") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // the backend announces changes (here: a language change made elsewhere)
    await backend.emit("ui-settings-changed", { ...(await backend.state()).ui, language: "ru" });
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("uses no browser storage", async ({ page, openApp }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.skip") }).click();
    const stored = await page.evaluate(async () => ({
      local: localStorage.length,
      session: sessionStorage.length,
      databases: (await indexedDB.databases()).length,
    }));
    expect(stored).toEqual({ local: 0, session: 0, databases: 0 });
  });
});

test.describe("first-run introduction without an account or a profile", () => {
  test.use({
    backendConfig: { ...firstRun, profiles: [], selected: null, accounts: [], active: null },
  });

  test("can be finished without either, and starts nothing", async ({ page, backend, openApp }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.finish") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    const names = (await backend.calls()).map((c) => c.cmd);
    expect(names).not.toContain("create_profile");
    expect(names).not.toContain("begin_sign_in");
  });

  test("the last page can lead into Create profile", async ({ page, backend, openApp }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.createProfile") }).click();
    await expect(page.getByRole("dialog", { name: text("create.title") })).toBeVisible();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    // still nothing created: the person has not asked for it yet
    expect(await backend.calls("create_profile")).toHaveLength(0);
  });

  test("the last page can lead into adding an account, without faking a sign-in", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.addAccount") }).click();
    // the normal sign-in dialog (it asks to protect the accounts first)
    await expect(page.getByRole("dialog", { name: text("signin.createTitle") })).toBeVisible();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
    await page.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toBeVisible();
  });
});

test.describe("first-run introduction in Russian", () => {
  test.use({ backendConfig: { ui: { ...firstRun.ui, language: "ru" } } });

  test("is shown in Russian from the first page to the last", async ({ page, openApp }) => {
    await openApp();
    const welcomeRu = page.getByRole("dialog", { name: textRu("onboarding.welcomeTitle") });
    await expect(welcomeRu).toBeVisible();
    await expect(welcomeRu.getByText(textRu("onboarding.tagline"))).toBeVisible();
    await expect(welcomeRu.getByText(textRu("onboarding.welcomeText"))).toBeVisible();
    await page.getByRole("button", { name: textRu("onboarding.continue") }).click();
    await expect(
      page.getByRole("dialog", { name: textRu("onboarding.portableTitle") }),
    ).toBeVisible();
    await page.getByRole("button", { name: textRu("onboarding.continue") }).click();
    const ready = page.getByRole("dialog", { name: textRu("onboarding.readyTitle") });
    await expect(
      ready.getByRole("button", { name: textRu("onboarding.addAccount") }),
    ).toBeVisible();
    await expect(ready.getByRole("button", { name: textRu("onboarding.finish") })).toBeVisible();
  });
});

test.describe("first-run introduction follows the language rules", () => {
  test.use({
    backendConfig: { ui: { ...firstRun.ui, language: null, systemLocale: "ru-RU" } },
  });

  test("a Russian Windows with no saved choice shows it in Russian", async ({ page, openApp }) => {
    await openApp();
    await expect(
      page.getByRole("dialog", { name: textRu("onboarding.welcomeTitle") }),
    ).toBeVisible();
  });
});

test.describe("an introduction already seen", () => {
  test("is not shown again", async ({ page, backend, openApp }) => {
    await openApp();
    await expect(headline(page)).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("complete_onboarding")).toHaveLength(0);
  });
});
