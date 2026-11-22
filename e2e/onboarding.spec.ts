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
    await expect(next()).toBeFocused();

    await next().click();
    await expect(portable).toBeVisible();
    await next().click();
    const ready = page.getByRole("dialog", { name: text("onboarding.readyTitle") });
    await expect(ready).toBeVisible();
    await expect(ready.getByText(text("onboarding.readyText"))).toBeVisible();
    await expect(ready.getByText(text("onboarding.step", { current: 3, total: 3 }))).toBeVisible();
    await expect(
      ready.getByRole("button", { name: text("onboarding.createProfile") }),
    ).toBeVisible();
    await expect(ready.getByRole("button", { name: text("onboarding.addAccount") })).toBeVisible();
    const finish = ready.getByRole("button", { name: text("onboarding.finish") });
    await expect(finish).toBeVisible();
    await expect(finish).toBeFocused();
    // Nothing to continue to, and no way to skip what is already the last page
    await expect(ready.getByRole("button", { name: text("onboarding.continue") })).toHaveCount(0);
    await expect(ready.getByRole("button", { name: text("onboarding.skip") })).toHaveCount(0);

    // Back from step 3 returns to step 2, and advancing again focuses Finish
    await back().click();
    await expect(portable).toBeVisible();
    await expect(
      portable.getByText(text("onboarding.step", { current: 2, total: 3 })),
    ).toBeVisible();
    await next().click();
    await expect(ready).toBeVisible();
    await expect(finish).toBeFocused();
  });

  test("traps keyboard focus inside the introduction modal", async ({ page, openApp }) => {
    await openApp();
    const dialog = welcome(page);
    const skipBtn = dialog.getByRole("button", { name: text("onboarding.skip") });
    const continueBtn = dialog.getByRole("button", { name: text("onboarding.continue") });
    await expect(continueBtn).toBeFocused();

    await page.keyboard.press("Tab");
    await expect(skipBtn).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(continueBtn).toBeFocused();

    await page.keyboard.press("Shift+Tab");
    await expect(skipBtn).toBeFocused();
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
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    // the backend announces changes (here: a language change made elsewhere)
    await backend.emit("ui-settings-changed", { ...(await backend.state()).ui, language: "ru" });
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("updates copy in place when language changes while open", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await expect(
      page.getByRole("dialog", { name: text("onboarding.portableTitle") }),
    ).toBeVisible();

    await backend.emit("ui-settings-changed", { ...(await backend.state()).ui, language: "ru" });

    const portableRu = page.getByRole("dialog", { name: textRu("onboarding.portableTitle") });
    await expect(portableRu).toBeVisible();
    await expect(portableRu.getByText(textRu("onboarding.portableText"))).toBeVisible();
    await expect(
      portableRu.getByText(textRu("onboarding.step", { current: 2, total: 3 })),
    ).toBeVisible();
    await expect(
      portableRu.getByRole("button", { name: textRu("onboarding.continue") }),
    ).toBeVisible();
  });

  test("still closes and opens follow-up dialogs when complete_onboarding fails", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await backend.failNext("complete_onboarding", "storage");
    await page.getByRole("button", { name: text("onboarding.createProfile") }).click();

    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    const createDialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(createDialog).toBeVisible();
    expect((await backend.state()).ui.onboardingDone).toBe(false);

    await createDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();
    await expect(page.getByRole("dialog", { name: text("signin.createTitle") })).toBeVisible();
  });

  test("uses no browser storage", async ({ page, backend, openApp }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.skip") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
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
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
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
    const createDialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(createDialog).toBeVisible();
    await expect(page.getByRole("dialog", { name: text("onboarding.readyTitle") })).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    await expect(createDialog.getByLabel(text("create.name"))).toBeFocused();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    // still nothing created: the person has not asked for it yet
    expect(await backend.calls("create_profile")).toHaveLength(0);

    await createDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
    await expect(playButton(page)).toBeVisible();
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
    const signInDialog = page.getByRole("dialog", { name: text("signin.createTitle") });
    await expect(signInDialog).toBeVisible();
    await expect(page.getByRole("dialog", { name: text("onboarding.readyTitle") })).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    await expect(signInDialog.getByLabel(text("common.passphrase"), { exact: true })).toBeFocused();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
    await signInDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toBeVisible();
  });
});

test.describe("first-run introduction with an existing locked vault", () => {
  test.use({
    backendConfig: {
      ...firstRun,
      profiles: [],
      selected: null,
      accounts: [],
      active: null,
      vault: { exists: true, unlocked: false },
    },
  });

  test("the last page can lead into unlocking the existing vault", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.continue") }).click();
    await page.getByRole("button", { name: text("onboarding.addAccount") }).click();

    const unlockDialog = page.getByRole("dialog", { name: text("signin.unlockTitle") });
    await expect(unlockDialog).toBeVisible();
    await expect(page.getByRole("dialog", { name: text("onboarding.readyTitle") })).toHaveCount(0);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    await expect(unlockDialog.getByLabel(text("common.passphrase"), { exact: true })).toBeFocused();
    await expect.poll(async () => (await backend.calls("complete_onboarding")).length).toBe(1);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);

    await unlockDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
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
    await expect(
      welcomeRu.getByText(textRu("onboarding.step", { current: 1, total: 3 })),
    ).toBeVisible();
    await page.getByRole("button", { name: textRu("onboarding.continue") }).click();

    const portableRu = page.getByRole("dialog", { name: textRu("onboarding.portableTitle") });
    await expect(portableRu).toBeVisible();
    await expect(portableRu.getByText(textRu("onboarding.portableText"))).toBeVisible();
    await expect(
      portableRu.getByText(textRu("onboarding.step", { current: 2, total: 3 })),
    ).toBeVisible();
    await page.getByRole("button", { name: textRu("onboarding.continue") }).click();

    const ready = page.getByRole("dialog", { name: textRu("onboarding.readyTitle") });
    await expect(ready).toBeVisible();
    await expect(ready.getByText(textRu("onboarding.readyText"))).toBeVisible();
    await expect(
      ready.getByText(textRu("onboarding.step", { current: 3, total: 3 })),
    ).toBeVisible();
    await expect(
      ready.getByRole("button", { name: textRu("onboarding.createProfile") }),
    ).toBeVisible();
    await expect(
      ready.getByRole("button", { name: textRu("onboarding.addAccount") }),
    ).toBeVisible();
    await expect(ready.getByRole("button", { name: textRu("onboarding.finish") })).toBeVisible();
  });
});

test.describe("first-run introduction follows the language rules", () => {
  test.describe("with no saved choice on Russian Windows", () => {
    test.use({
      backendConfig: { ui: { ...firstRun.ui, language: null, systemLocale: "ru-RU" } },
    });

    test("a Russian Windows with no saved choice shows it in Russian", async ({
      page,
      openApp,
    }) => {
      await openApp();
      await expect(
        page.getByRole("dialog", { name: textRu("onboarding.welcomeTitle") }),
      ).toBeVisible();
    });
  });

  test.describe("with an explicit English choice on Russian Windows", () => {
    test.use({
      backendConfig: { ui: { ...firstRun.ui, language: "en", systemLocale: "ru-RU" } },
    });

    test("explicit English overrides Russian Windows during first run", async ({
      page,
      openApp,
    }) => {
      await openApp();
      await expect(welcome(page)).toBeVisible();
      await expect(
        page.getByRole("dialog", { name: textRu("onboarding.welcomeTitle") }),
      ).toHaveCount(0);
    });
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
