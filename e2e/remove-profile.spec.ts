import { CREATIVE, DEFAULT_CONFIG, FRESH, SURVIVAL } from "./support/fixtures";
import { expect, test } from "./support/test";
import { playButton, profileTrigger, settingsButton, text, textRu } from "./support/ui";

test.describe("Remove from Clade", () => {
  test("asks first, keeps the files (says so), enforces focus/Escape/backdrop, and removes only the selected profile", async ({
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
    await expect(dialog.getByRole("button", { name: text("common.cancel") })).toBeFocused();

    // Backdrop click does not close a confirmation dialog (`closeOnBackdrop={false}`).
    await page.locator('[data-slot="dialog-overlay"]').click({ position: { x: 8, y: 8 } });
    await expect(dialog).toBeVisible();

    // Cancel changes nothing at all.
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("remove_profile")).toHaveLength(0);

    // Escape closes the dialog without calling `remove_profile`.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
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
    for (const profile of [SURVIVAL, CREATIVE, FRESH]) {
      await profileTrigger(page).click();
      await page.getByRole("menuitem", { name: text("profile.remove") }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog).toContainText(text("removeProfile.title", { profile: profile.name }));
      await dialog.getByRole("button", { name: text("removeProfile.confirm") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    }
    await expect
      .poll(async () => (await backend.calls("remove_profile")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id }, { id: CREATIVE.id }, { id: FRESH.id }]);
    await expect(page.getByRole("button", { name: text("profile.createFirst") })).toBeVisible();
    await expect(playButton(page)).toBeVisible();
    await expect(page.getByText(text("removedProfiles.emptyNotice", { count: 3 }))).toBeVisible();
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

  test("shows localized error alerts and keeps dialogs open for remove, restore, and open-folder failures", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // RemoveProfileDialog error codes: stays open, renders alert, supports Cancel and retry.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    const removeDialog = page.getByRole("dialog");
    await backend.failNext("remove_profile", "remove_failed");
    await removeDialog.getByRole("button", { name: text("removeProfile.confirm") }).click();
    await expect(removeDialog.getByRole("alert")).toHaveText(text("error.profile.remove_failed"));
    await expect(removeDialog).toBeVisible();
    await removeDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    for (const code of ["remove_refused", "already_running", "busy", "not_found"] as const) {
      await backend.failNext("remove_profile", code);
      await removeDialog.getByRole("button", { name: text("removeProfile.confirm") }).click();
      await expect(removeDialog.getByRole("alert")).toHaveText(text(`error.profile.${code}`));
      await expect(removeDialog).toBeVisible();
    }
    // Retry succeeds and clears the dialog.
    await removeDialog.getByRole("button", { name: text("removeProfile.confirm") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // Remove Creative too so two profiles sit in Removed profiles.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: text("removeProfile.confirm") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.removedMenu", { count: 2 }) }).click();
    const removedDialog = page.getByRole("dialog", { name: text("removedProfiles.title") });

    // Open-folder failure renders alert, keeps dialog open, and clears on retry.
    await backend.failNext("open_removed_profiles_folder", "open_failed");
    await removedDialog.getByRole("button", { name: text("removedProfiles.openFolder") }).click();
    await expect(removedDialog.getByRole("alert")).toHaveText(text("error.profile.open_failed"));
    await expect(removedDialog).toBeVisible();
    await removedDialog.getByRole("button", { name: text("removedProfiles.openFolder") }).click();
    await expect(removedDialog.getByRole("alert")).toHaveCount(0);

    // Restore failures keep dialog open and list intact; retry restores cleanly.
    for (const code of ["restore_failed", "not_found"] as const) {
      await backend.failNext("restore_profile", code);
      await removedDialog
        .getByRole("button", { name: text("removedProfiles.restore") })
        .first()
        .click();
      await expect(removedDialog.getByRole("alert")).toHaveText(text(`error.profile.${code}`));
      await expect(removedDialog).toBeVisible();
      await expect(removedDialog.getByRole("listitem")).toHaveCount(2);
    }

    await removedDialog
      .getByRole("button", { name: text("removedProfiles.restore") })
      .first()
      .click();
    await expect(removedDialog.getByRole("alert")).toHaveCount(0);
    await expect(removedDialog.getByRole("listitem")).toHaveCount(1);
    await removedDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("locks remove and restore dialogs while the backend is busy", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // RemoveProfileDialog busy/locked state.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    const removeDialog = page.getByRole("dialog");
    await backend.holdNext("remove_profile");
    await removeDialog.getByRole("button", { name: text("removeProfile.confirm") }).click();

    await expect(removeDialog).toBeVisible();
    await expect(removeDialog.getByRole("button", { name: text("common.cancel") })).toBeDisabled();
    await expect(
      removeDialog.getByRole("button", { name: text("removeProfile.working") }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(removeDialog).toBeVisible();

    await backend.release();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: CREATIVE.name }),
    );

    // Remove Creative so two rows appear in RemovedProfilesDialog.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: text("removeProfile.confirm") })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.removedMenu", { count: 2 }) }).click();
    const removedDialog = page.getByRole("dialog", { name: text("removedProfiles.title") });

    // RemovedProfilesDialog busy/locked state.
    await backend.holdNext("restore_profile");
    await removedDialog
      .getByRole("button", { name: text("removedProfiles.restore") })
      .first()
      .click();

    await expect(removedDialog).toBeVisible();
    await expect(
      removedDialog.getByRole("button", { name: text("removedProfiles.restoring") }),
    ).toBeDisabled();
    await expect(
      removedDialog.getByRole("button", { name: text("removedProfiles.restore") }),
    ).toBeDisabled();
    await expect(
      removedDialog.getByRole("button", { name: text("removedProfiles.openFolder") }),
    ).toBeDisabled();
    await expect(removedDialog.getByRole("button", { name: text("common.done") })).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(removedDialog).toBeVisible();

    await backend.release();
    await expect(removedDialog).toBeVisible();
    await expect(removedDialog.getByRole("listitem")).toHaveCount(1);
    await expect(
      removedDialog.getByRole("button", { name: text("removedProfiles.restore") }),
    ).toBeEnabled();
    await expect(
      removedDialog.getByRole("button", { name: text("removedProfiles.openFolder") }),
    ).toBeEnabled();
    await expect(removedDialog.getByRole("button", { name: text("common.done") })).toBeEnabled();
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
    // Removed profiles menu item disappears once no removed profiles remain.
    await profileTrigger(page).click();
    await expect(page.getByRole("menuitem")).toHaveText([
      text("profile.create"),
      text("profile.settings"),
      text("profile.openFolder"),
      text("profile.verify"),
      text("profile.remove"),
    ]);
    await page.keyboard.press("Escape");

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

    await expect(page.getByText(textRu("removedProfiles.emptyNotice", { count: 4 }))).toBeVisible();
    await page.getByRole("button", { name: textRu("removedProfiles.openAction") }).click();
    const ruDialog = page.getByRole("dialog", { name: textRu("removedProfiles.title") });
    await expect(ruDialog).toBeVisible();
    await expect(ruDialog.getByRole("listitem")).toHaveCount(4);
    const fitsRu = await ruDialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1);
    expect(fitsRu).toBe(true);
    await ruDialog
      .getByRole("button", { name: textRu("removedProfiles.restore") })
      .first()
      .click();
    await expect(ruDialog).toBeVisible(); // 3 removed profiles still remain
    await expect(ruDialog.getByRole("listitem")).toHaveCount(3);
    await ruDialog.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("status")).toHaveCount(0);
    await expect(page.getByRole("button", { name: textRu("profile.createFirst") })).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      textRu("profile.trigger", { label: SURVIVAL.name }),
    );
  });

  test.describe("when the selected profile is running", () => {
    test.use({ backendConfig: { runs: { survival: "playing" } } });

    test("disables Remove from Clade in the dock menu", async ({ page, openApp }) => {
      await openApp();
      await profileTrigger(page).click();
      await expect(page.getByRole("menuitem", { name: text("profile.remove") })).toBeDisabled();
    });
  });

  test.describe("emptyNotice singular pluralization", () => {
    test.use({ backendConfig: { profiles: [SURVIVAL], selected: SURVIVAL.id } });

    test("renders singular removedProfiles.emptyNotice in English and Russian", async ({
      page,
      openApp,
    }) => {
      await openApp();
      await profileTrigger(page).click();
      await page.getByRole("menuitem", { name: text("profile.remove") }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: text("removeProfile.confirm") })
        .click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.getByText(text("removedProfiles.emptyNotice", { count: 1 }))).toBeVisible();

      await settingsButton(page).click();
      await page
        .getByRole("dialog", { name: text("settings.title") })
        .getByRole("radio", { name: "Русский" })
        .click();
      await page.getByRole("button", { name: textRu("common.done") }).click();
      await expect(
        page.getByText(textRu("removedProfiles.emptyNotice", { count: 1 })),
      ).toBeVisible();
    });
  });

  test.describe("emptyNotice Russian many pluralization", () => {
    test.use({
      backendConfig: {
        profiles: [],
        selected: null,
        removed: [1, 2, 3, 4, 5].map((n) => ({
          ...SURVIVAL,
          id: `removed-${n}`,
          name: `Removed ${n}`,
          folder: `removed-${n}`,
        })),
        ui: { ...DEFAULT_CONFIG.ui, language: "ru" },
      },
    });

    test("renders many form for 5 removed profiles in Russian", async ({ page, openApp }) => {
      await openApp();
      await expect(
        page.getByText(textRu("removedProfiles.emptyNotice", { count: 5 })),
      ).toBeVisible();
    });
  });
});
