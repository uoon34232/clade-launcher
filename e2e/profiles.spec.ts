import type { Page } from "@playwright/test";
import { staleNote } from "@/features/launcher/profileUi";
import { CATALOG, DEFAULT_CONFIG, LOADERS } from "./support/fixtures";
import { expect, test } from "./support/test";
import { playButton, profileTrigger, text } from "./support/ui";

const rendered = (page: Page) =>
  page.evaluate(
    () =>
      new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  );

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
      text("profile.trigger", { label: "Creative" }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect
      .poll(async () => (await backend.calls("select_profile")).map((c) => c.args))
      .toEqual([{ id: "creative" }]);

    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Fresh" }),
    );
    await expect(playButton(page)).toHaveText(text("play.setup"));
    await expect.poll(async () => (await backend.state()).selected).toBe("fresh");
  });

  test("reverts an optimistic profile switch if select_profile fails", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const listBefore = (await backend.calls("list_profiles")).length;
    await backend.failNext("select_profile", "not_found");
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();

    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Survival" }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect
      .poll(async () => (await backend.calls("list_profiles")).length)
      .toBe(listBefore + 1);
    await expect.poll(async () => (await backend.state()).selected).toBe("survival");
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
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();
    await expect(menu.getByRole("menuitemradio", { name: /^Survival/ })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(menu.getByRole("menuitemradio", { name: /^Creative/ })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Creative" }),
    );
    await expect.poll(async () => (await backend.state()).selected).toBe("creative");
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
      text("profile.trigger", { label: "Weekend" }),
    );
    // a profile that has not been prepared yet
    await expect(playButton(page)).toHaveText(text("play.setup"));
    expect((await backend.calls("create_profile")).map((c) => c.args)).toEqual([
      { request: { name: "Weekend", minecraft: "1.20.1", profile: { type: "vanilla" } } },
    ]);
  });

  test("locks the dialog while creating, trims whitespace, and surfaces backend errors without closing", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("  Trimmed Profile  ");

    // Backend rejection keeps the dialog open and explains why in an alert.
    await backend.failNext("create_profile", "cannot_verify");
    await dialog.getByRole("button", { name: text("create.submit") }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.profile.cannot_verify"));
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeEnabled();

    // Holding create_profile locks Escape, disables Cancel, and switches submit to Creating…
    await backend.holdNext("create_profile");
    await dialog.getByRole("button", { name: text("create.submit") }).click();

    const creating = dialog.getByRole("button", { name: text("create.creating") });
    await expect(creating).toBeVisible();
    await expect(creating).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("common.cancel") })).toBeDisabled();

    await page.keyboard.press("Escape");
    await rendered(page);
    await expect(dialog).toHaveAttribute("data-open", "");
    await expect(dialog).not.toHaveAttribute("data-closed", "");

    await backend.release();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Trimmed Profile" }),
    );
    expect((await backend.calls("create_profile")).map((c) => c.args)).toEqual([
      {
        request: { name: "Trimmed Profile", minecraft: "1.21.4", profile: { type: "vanilla" } },
      },
      {
        request: { name: "Trimmed Profile", minecraft: "1.21.4", profile: { type: "vanilla" } },
      },
    ]);
  });

  test("in Simple mode hides version filters and loader version picker, creating Modded with recommended version", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await expect(dialog.getByRole("radiogroup", { name: text("create.filterLabel") })).toHaveCount(
      0,
    );
    await dialog.getByLabel(text("create.name")).fill("Simple Fabric");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await expect(
      dialog.getByRole("radiogroup", { name: text("create.loaderLabel") }),
    ).toBeVisible();
    await expect(dialog.getByRole("combobox", { name: text("create.loaderVersion") })).toHaveCount(
      0,
    );

    await dialog.getByRole("button", { name: text("create.submit") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const [call] = await backend.calls("create_profile");
    expect((call?.args as { request: unknown }).request).toEqual({
      name: "Simple Fabric",
      minecraft: "1.21.4",
      profile: {
        type: "modded",
        loader: "fabric",
        loader_version: { mode: "pinned", version: "0.16.9" },
      },
    });
  });

  test.describe("with Advanced mode version filters", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
      await expect(page.getByRole("listbox")).toHaveCount(0);

      await dialog.getByRole("radio", { name: text("create.filter.historical") }).click();
      await version.click();
      await expect(page.getByRole("option", { name: /^b1\.7\.3/ })).toBeVisible();
      await expect(page.getByRole("option", { name: /^a1\.2\.6/ })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("listbox")).toHaveCount(0);
    });

    test("creates a profile from a snapshot version", async ({ page, backend, openApp }) => {
      await openApp();
      const dialog = await openCreate(page);
      await dialog.getByLabel(text("create.name")).fill("Snapshot Run");
      await dialog.getByRole("radio", { name: text("create.filter.snapshots") }).click();

      const version = dialog.getByRole("combobox", { name: text("create.mcVersion") });
      await version.click();
      await page.getByRole("option", { name: /^24w50a/ }).click();
      await expect(page.getByRole("listbox")).toHaveCount(0);
      await expect(version).toContainText("24w50a");

      await dialog.getByRole("button", { name: text("create.submit") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect((await backend.calls("create_profile")).map((c) => c.args)).toEqual([
        { request: { name: "Snapshot Run", minecraft: "24w50a", profile: { type: "vanilla" } } },
      ]);
    });
  });

  test("asks for a name, and refuses a name that is already used or too long", async ({
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

    await name.fill("a".repeat(65));
    await expect(dialog.getByRole("alert")).toHaveText(text("validate.nameLong", { max: 64 }));
    await expect(name).toHaveAttribute("aria-invalid", "true");
    await expect(create).toBeDisabled();

    await name.fill("Survival 2");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await expect(create).toBeEnabled();
    expect(await backend.calls("create_profile")).toHaveLength(0);
  });

  test("Cancel and Escape close it without creating anything, and reopening starts fresh", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("Never created");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    const reopened = await openCreate(page);
    await expect(reopened.getByLabel(text("create.name"))).toHaveValue("");
    await expect(reopened.getByRole("radio", { name: text("profile.vanilla") })).toBeChecked();
    await expect(reopened.getByRole("alert")).toHaveCount(0);
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
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
      text("profile.trigger", { label: "With Forge" }),
    );
    const [call] = await backend.calls("create_profile");
    const request = (call?.args as { request: unknown }).request;
    expect(request).toEqual({
      name: "With Forge",
      minecraft: "1.21.4",
      profile: {
        type: "modded",
        loader: "forge",
        loader_version: { mode: "pinned", version: "54.0.1" },
      },
    });
  });

  test("allows picking a non-recommended loader version and pins it", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByLabel(text("create.name")).fill("Forge Latest");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await dialog
      .getByRole("radiogroup", { name: text("create.loaderLabel") })
      .getByRole("radio", { name: "Forge", exact: true })
      .click();

    const loaderVersion = dialog.getByRole("combobox", { name: text("create.loaderVersion") });
    await loaderVersion.click();
    await expect(page.getByRole("option", { name: /^54\.0\.1/ })).toContainText(
      text("create.channel.recommended"),
    );
    const latest = page.getByRole("option", { name: /^54\.0\.0/ });
    await expect(latest).toContainText(text("create.channel.latest"));
    await latest.click();
    await expect(page.getByRole("listbox")).toHaveCount(0);
    await expect(loaderVersion).toContainText("54.0.0");

    await dialog.getByRole("button", { name: text("create.submit") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    const [call] = await backend.calls("create_profile");
    expect((call?.args as { request: unknown }).request).toEqual({
      name: "Forge Latest",
      minecraft: "1.21.4",
      profile: {
        type: "modded",
        loader: "forge",
        loader_version: { mode: "pinned", version: "54.0.0" },
      },
    });
  });

  test("a loader with no build for the chosen Minecraft version cannot be picked and falls back", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    const loaders = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    const neoforge = loaders.getByRole("radio", { name: "NeoForge", exact: true });
    await neoforge.click();
    await expect(neoforge).toBeChecked();

    await dialog.getByRole("combobox", { name: text("create.mcVersion") }).click();
    await page.getByRole("option", { name: /^1\.20\.1/ }).click();
    await expect(neoforge).toBeDisabled();
    await expect(neoforge).not.toBeChecked();
    await expect(loaders.getByRole("radio", { name: "Fabric", exact: true })).toBeChecked();
    await expect(loaders.getByRole("radio", { name: "Fabric", exact: true })).toBeEnabled();
    await expect(loaders.getByRole("radio", { name: "Forge", exact: true })).toBeEnabled();
    await expect(loaders.getByRole("radio", { name: "Quilt", exact: true })).toBeEnabled();
    await expect(
      dialog.getByText(text("create.loaderNone", { loader: "NeoForge", minecraft: "1.20.1" }), {
        exact: false,
      }),
    ).toBeVisible();
    await expect(
      dialog.getByRole("combobox", { name: text("create.loaderVersion") }),
    ).toContainText(text("create.versionRecommended", { version: "0.15.11" }));
  });

  test("handles catalog loading, catalog retry, and loader check failure", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // 1. Loading indicator while get_version_catalog is in flight
    await backend.holdNext("get_version_catalog");
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(text("create.loading"))).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeDisabled();
    await backend.release();
    await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toContainText(
      "1.21.4",
    );
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // 2. Catalog unavailable -> inline error + Try again recovery
    await backend.failNext("get_version_catalog", "catalog_unavailable");
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(text("create.unavailable"))).toBeVisible();
    await expect(dialog.getByRole("alert")).toContainText(
      text("error.catalog.catalog_unavailable"),
    );
    await dialog.getByLabel(text("create.name")).fill("After Retry");
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeDisabled();

    await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
    await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toContainText(
      "1.21.4",
    );
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeEnabled();

    // 3. Loader options rejection -> loaderCheckFailed + all loaders disabled
    await backend.failNext("get_loader_options", "network_unavailable");
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();
    await expect(dialog.getByText(text("create.loaderCheckFailed"))).toBeVisible();
    const loaders = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    for (const loader of ["Fabric", "Forge", "NeoForge", "Quilt"] as const) {
      await expect(loaders.getByRole("radio", { name: loader, exact: true })).toBeDisabled();
    }
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeDisabled();
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

test.describe("create profile: stale and unavailable metadata", () => {
  test.use({
    backendConfig: {
      ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" },
      catalog: {
        ...CATALOG,
        entries: CATALOG.entries.filter((e) => e.kind === "release"),
        origin: "cache_stale",
        stale: true,
      },
      loaders: {
        ...LOADERS,
        "1.21.4": [
          {
            loader: "fabric",
            status: "ok",
            versions: [{ version: "0.16.9", channel: "recommended" }],
            recommended: "0.16.9",
            stale: true,
            error: null,
          },
          {
            loader: "forge",
            status: "unavailable",
            versions: [],
            recommended: null,
            stale: false,
            error: "offline",
          },
          LOADERS["1.21.4"][2],
          LOADERS["1.21.4"][3],
        ],
      },
    },
  });

  test("shows stale catalog note, empty filter message, stale loader note, and unavailable loader hint", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openCreate(page);

    await expect(dialog.getByText(staleNote(CATALOG.fetchedAt))).toBeVisible();
    await dialog.getByLabel(text("create.name")).fill("Offline Check");
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeEnabled();

    await dialog.getByRole("radio", { name: text("create.filter.snapshots") }).click();
    await expect(dialog.getByText(text("create.noVersions"))).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("create.submit") })).toBeDisabled();

    await dialog.getByRole("radio", { name: text("create.filter.releases") }).click();
    await dialog.getByRole("radio", { name: text("profile.modded") }).click();

    await expect(dialog.getByText(text("create.staleLoaders"))).toBeVisible();
    const loaders = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    await expect(loaders.getByRole("radio", { name: "Forge", exact: true })).toBeDisabled();
    await expect(
      dialog.getByText(text("create.loaderUnchecked", { loader: "Forge" }), { exact: false }),
    ).toBeVisible();
  });
});
