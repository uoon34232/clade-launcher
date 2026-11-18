import { CREATIVE, FRESH, SURVIVAL } from "./support/fixtures";
import { expect, test } from "./support/test";
import { playButton, profileTrigger, settingsButton, text, textRu } from "./support/ui";

test.describe("Remove from Clade", () => {
  test("asks first, keeps the files (says so), removes only the selected profile and selects another", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    // pinned and visually separated, with no buttons on the profile rows
    await expect(page.getByRole("menuitem")).toHaveText([
      text("profile.create"),
      text("profile.settings"),
      text("profile.openFolder"),
      text("profile.verify"),
      text("profile.remove"),
    ]);
    await expect(page.locator('[data-slot="dropdown-menu-content"] button')).toHaveCount(0);
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText(text("removeProfile.title", { profile: SURVIVAL.name }));
    await expect(dialog).toContainText(text("removeProfile.body"));
    // Cancel changes nothing at all
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("remove_profile")).toHaveLength(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: text("removeProfile.confirm") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("remove_profile")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id }]);

    // immediately gone, another profile selected, no reload; the others are untouched
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: CREATIVE.name }),
    );
    await profileTrigger(page).click();
    await expect(page.getByRole("menuitemradio")).toHaveText([
      new RegExp(`^${CREATIVE.name}`),
      new RegExp(`^${FRESH.name}`),
    ]);
  });

  test("removing the last profile returns to the empty state", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    for (const name of [SURVIVAL.name, CREATIVE.name, FRESH.name]) {
      await profileTrigger(page).click();
      await page.getByRole("menuitem", { name: text("profile.remove") }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: text("removeProfile.confirm") })
        .click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(name).toBeTruthy();
    }
    expect(await backend.calls("remove_profile")).toHaveLength(3);
    await expect(page.getByRole("button", { name: text("profile.createFirst") })).toBeVisible();
    await expect(playButton(page)).toBeVisible();
  });

  test("is worded in Russian", async ({ page, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    await page
      .getByRole("dialog", { name: text("settings.title") })
      .getByRole("radio", { name: "Русский" })
      .click();
    await page.getByRole("button", { name: textRu("common.done") }).click();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: textRu("profile.remove") }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText(`Убрать «${SURVIVAL.name}» из Clade?`);
    await expect(dialog).toContainText(textRu("removeProfile.body"));
  });
});
