import type { Page } from "@playwright/test";
import { expect, test } from "./support/test";
import { playButton, profileTrigger, text } from "./support/ui";

const openCreate = async (page: Page) => {
  await profileTrigger(page).click();
  await page.getByRole("menuitem", { name: text("profile.create") }).click();
  const dialog = page.getByRole("dialog", { name: text("create.title") });
  await expect(dialog).toBeVisible();
  // the version list has been fetched
  await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toContainText(
    "1.21.4",
  );
  return dialog;
};

test.describe("profile selector", () => {
  test("lists the profiles, marks the selected one, and ends with Create and Verify", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio")).toHaveCount(3);
    await expect(menu.getByRole("menuitemradio", { name: /^Survival/ })).toBeChecked();
    await expect(
      menu.getByRole("menuitemradio", { name: /^Creative Fabric 1\.21\.4/ }),
    ).not.toBeChecked();
    await expect(menu.getByRole("menuitem", { name: text("profile.create") })).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: text("profile.verify") })).toBeVisible();
  });

  test("choosing a profile selects it and updates the dock; an unprepared one offers Set up", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Creative/ }).click();
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Creative · Fabric 1.21.4" }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect
      .poll(async () => (await backend.calls("select_profile")).map((c) => c.args))
      .toEqual([{ id: "creative" }]);

    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();
    await expect(playButton(page)).toHaveText(text("play.setup"));
    expect((await backend.state()).selected).toBe("fresh");
  });

  test("Escape closes the menu and focus returns to the control", async ({ page, openApp }) => {
    await openApp();
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(profileTrigger(page)).toBeFocused();
  });

  test("can be operated from the keyboard", async ({ page, backend, openApp }) => {
    await openApp();
    await profileTrigger(page).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await backend.state()).selected).not.toBe("survival");
  });
});

test.describe("create profile: Vanilla", () => {
  test("needs a name, then creates a Vanilla profile for the chosen version", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    const create = dialog.getByRole("button", { name: text("create.submit") });

    await expect(dialog.getByRole("radio", { name: text("profile.vanilla") })).toBeChecked();
    await expect(dialog.getByText(text("create.vanillaNote"))).toBeVisible();
    await expect(create).toBeDisabled();

    await dialog.getByLabel(text("create.name")).fill("Weekend");
    await expect(create).toBeEnabled();

    await dialog.getByRole("combobox", { name: text("create.mcVersion") }).click();
    await page.getByRole("option", { name: /^1\.20\.1/ }).click();
    await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toContainText(
      "1.20.1",
    );

    await create.click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Weekend · Vanilla 1.20.1" }),
    );
    // a profile that has not been prepared yet
    await expect(playButton(page)).toHaveText(text("play.setup"));
    expect((await backend.calls("create_profile")).map((c) => c.args)).toEqual([
      { request: { name: "Weekend", minecraft: "1.20.1", profile: { type: "vanilla" } } },
    ]);
  });

  test("lists releases, snapshots and historical versions separately", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    const version = dialog.getByRole("combobox", { name: text("create.mcVersion") });

    await version.click();
    await expect(page.getByRole("option")).toHaveCount(4);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("listbox")).toHaveCount(0);

    await dialog.getByRole("radio", { name: text("create.filter.snapshots") }).click();
    await version.click();
    await expect(page.getByRole("option", { name: /^24w50a/ })).toBeVisible();
    await page.keyboard.press("Escape");

    await dialog.getByRole("radio", { name: text("create.filter.historical") }).click();
    await version.click();
    await expect(page.getByRole("option", { name: /^b1\.7\.3/ })).toBeVisible();
    await expect(page.getByRole("option", { name: /^a1\.2\.6/ })).toBeVisible();
  });

  test("asks for a name, and refuses a name that is already used", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    const name = dialog.getByLabel(text("create.name"));
    const create = dialog.getByRole("button", { name: text("create.submit") });

    // leaving the empty field shows the reason
    await name.focus();
    await name.blur();
    await expect(dialog.getByRole("alert")).toHaveText(text("validate.nameEmpty"));
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(create).toBeDisabled();

    await name.fill("survival");
    await expect(dialog.getByRole("alert")).toHaveText(text("validate.nameDuplicate"));
    await expect(create).toBeDisabled();

    await name.fill("Survival 2");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await expect(create).toBeEnabled();
    expect(await backend.calls("create_profile")).toHaveLength(0);
  });

  test("Cancel and Escape close it without creating anything", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("Never created");
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await openCreate(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("create_profile")).toHaveLength(0);
  });

  test("a stray click on the backdrop does not discard what was typed", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("Typed text");
    await page.mouse.click(8, 8);
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel(text("create.name"))).toHaveValue("Typed text");
  });
});

test.describe("create profile: Modded", () => {
  test("offers Fabric, Forge, NeoForge and Quilt with their recommended versions", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await expect(dialog.getByText(text("create.moddedNote"))).toBeVisible();

    const loaders = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    const loaderVersion = dialog.getByRole("combobox", { name: text("create.loaderVersion") });
    for (const [name, version] of [
      ["Fabric", "0.16.9"],
      ["Forge", "54.0.1"],
      ["NeoForge", "21.4.10"],
      ["Quilt", "0.27.1"],
    ] as const) {
      await loaders.getByRole("radio", { name, exact: true }).click();
      await expect(loaders.getByRole("radio", { name, exact: true })).toBeChecked();
      await expect(loaderVersion).toContainText(text("create.versionRecommended", { version }));
    }
  });

  test("creates a modded profile with the chosen loader", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("With Forge");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await dialog
      .getByRole("radiogroup", { name: text("create.loaderLabel") })
      .getByRole("radio", { name: "Forge", exact: true })
      .click();
    await dialog.getByRole("button", { name: text("create.submit") }).click();

    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "With Forge · Forge 1.21.4" }),
    );
    const [call] = await backend.calls("create_profile");
    const request = (
      call?.args as {
        request: { name: string; minecraft: string; profile: { type: string; loader?: string } };
      }
    ).request;
    expect(request).toMatchObject({
      name: "With Forge",
      minecraft: "1.21.4",
      profile: { type: "modded", loader: "forge" },
    });
  });

  test("a loader with no build for the chosen Minecraft version cannot be picked", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    const loaders = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    await expect(loaders.getByRole("radio", { name: "NeoForge", exact: true })).toBeEnabled();

    await dialog.getByRole("combobox", { name: text("create.mcVersion") }).click();
    await page.getByRole("option", { name: /^1\.20\.1/ }).click();
    await expect(loaders.getByRole("radio", { name: "NeoForge", exact: true })).toBeDisabled();
    await expect(loaders.getByRole("radio", { name: "Fabric", exact: true })).toBeEnabled();
    await expect(loaders.getByRole("radio", { name: "Forge", exact: true })).toBeEnabled();
    await expect(loaders.getByRole("radio", { name: "Quilt", exact: true })).toBeEnabled();
  });

  test("switching back to Vanilla creates a plain profile", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("Changed my mind");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await dialog.getByRole("radio", { name: text("profile.vanilla") }).click();
    await dialog.getByRole("button", { name: text("create.submit") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const [call] = await backend.calls("create_profile");
    expect((call?.args as { request: { profile: { type: string } } }).request.profile).toEqual({
      type: "vanilla",
    });
  });
});
