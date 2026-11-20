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

  test("restores a removed profile (including name collision), from empty state, and in Russian at 960x600", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();

    // Remove Survival, then open Removed profiles (1)… and restore it back.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: text("removeProfile.confirm") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.removedMenu", { count: 1 }) }).click();
    const removedDialog = page.getByRole("dialog", { name: text("removedProfiles.title") });
    await expect(removedDialog).toContainText(SURVIVAL.name);
    const fits = await removedDialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1);
    expect(fits).toBe(true);

    await removedDialog.getByRole("button", { name: text("removedProfiles.openFolder") }).click();
    expect(await backend.calls("open_removed_profiles_folder")).toHaveLength(1);

    await removedDialog.getByRole("button", { name: text("removedProfiles.restore") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: SURVIVAL.name }),
    );

    // Remove Survival again, create a new profile with the same name, then restore -> "Survival (2)".
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: text("removeProfile.confirm") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const create = page.getByRole("dialog", { name: text("create.title") });
    await create.getByLabel(text("create.name")).fill(SURVIVAL.name);
    await create.getByRole("button", { name: text("create.submit") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.removedMenu", { count: 1 }) }).click();
    await page
      .getByRole("dialog", { name: text("removedProfiles.title") })
      .getByRole("button", { name: text("removedProfiles.restore") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Survival (2)" }),
    );

    // Switch to Russian, remove every profile, and open Removed profiles from the empty-state notice.
    await settingsButton(page).click();
    await page
      .getByRole("dialog", { name: text("settings.title") })
      .getByRole("radio", { name: "Русский" })
      .click();
    await page.getByRole("button", { name: textRu("common.done") }).click();

    for (let i = 0; i < 4; i++) {
      await profileTrigger(page).click();
      await page.getByRole("menuitem", { name: textRu("profile.remove") }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: textRu("removeProfile.confirm") })
        .click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    }

    await expect(page.getByText("В папке Clade сохранены 4 убранных профиля.")).toBeVisible();
    await page.getByRole("button", { name: textRu("removedProfiles.openAction") }).click();
    const ruDialog = page.getByRole("dialog", { name: textRu("removedProfiles.title") });
    await expect(ruDialog).toBeVisible();
    const fitsRu = await ruDialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1);
    expect(fitsRu).toBe(true);
    await ruDialog
      .getByRole("button", { name: textRu("removedProfiles.restore") })
      .first()
      .click();
    await expect(ruDialog).toBeVisible(); // 3 removed profiles still remain
    await ruDialog.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});
