import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { dock, playButton, profileTrigger, settingsButton, text, textRu } from "./support/ui";

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
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    await expect(playButton(page)).toHaveText(text("play.play"));

    await settingsButton(page).click();
    const settings = page.getByRole("dialog", { name: text("settings.title") });
    await expect(settings).toBeVisible();

    // ---- Русский
    await settings.getByRole("radio", { name: "Русский" }).click();
    await expect(page.getByRole("dialog", { name: textRu("settings.title") })).toBeVisible();
    await expect(page.getByRole("switch", { name: textRu("settings.sound") })).toBeVisible();
    await expect(page.getByRole("button", { name: textRu("common.done") })).toBeVisible();
    // the language was saved through the backend, and nowhere else
    await expect.poll(async () => (await backend.state()).ui.language).toBe("ru");
    expect((await backend.calls("set_language")).map((c) => c.args)).toEqual([{ language: "ru" }]);

    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(playButton(page)).toHaveText(textRu("play.play"));
    await expect(profileTrigger(page)).toHaveAccessibleName(
      textRu("profile.trigger").replace(
        "{label}",
        `Survival · ${textRu("profile.vanilla")} 1.21.4`,
      ),
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
    await page.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect.poll(async () => (await backend.state()).ui.language).toBe("en");

    expect(
      await page.evaluate(() => (window as unknown as { __noReload?: boolean }).__noReload),
    ).toBe(true);
  });

  test("an open Create profile dialog stays open, with its typed text, when the language changes", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(dialog).toBeVisible();
    const name = dialog.getByLabel(text("create.name"));
    await name.fill("Weekend");
    await expect(name).toBeFocused();

    // The backend announces a language change (as it does for any change made elsewhere).
    await backend.emit("ui-settings-changed", { ...DEFAULT_CONFIG.ui, language: "ru" });

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
    await expect(page.getByText(textRu("hero.subtitle"))).toBeVisible();
    await expect(playButton(page)).toHaveText(textRu("play.play"));
    await expect(dock(page)).toBeVisible();
  });
});

test.describe("an explicit English choice on a Russian Windows", () => {
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, language: "en", systemLocale: "ru-RU" } },
  });

  test("is never overridden by the system language", async ({ page, openApp }) => {
    await openApp();
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
  });
});
