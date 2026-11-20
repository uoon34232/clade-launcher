import { renderMessage } from "@/lib/i18n/core";
import { en, ru } from "@/locales";
import { CREATIVE, DEFAULT_CONFIG, FRESH, SURVIVAL } from "./support/fixtures";
import { expect, test, type Page } from "./support/test";
import { dock, playButton, profileTrigger, settingsButton, text, textRu } from "./support/ui";

async function openProfileSettings(page: Page) {
  await profileTrigger(page).click();
  await page.getByRole("menuitem", { name: text("profile.settings") }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  return dialog;
}

test.describe("profile settings", () => {
  test("in Simple mode shows details and files only, hides advanced sections, and fits 960x600 in English and Russian", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();

    const fits = async () =>
      page.getByRole("dialog").evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      }));

    const dialog = await openProfileSettings(page);
    expect(await dialog.getByRole("heading", { level: 3 }).allTextContents()).toEqual([
      text("profileModal.details"),
      text("profileModal.files"),
    ]);
    await expect(dialog.getByRole("heading", { name: text("settings.memory") })).toHaveCount(0);
    await expect(
      dialog.getByRole("heading", { name: text("profileModal.resolution") }),
    ).toHaveCount(0);
    await expect(dialog.getByRole("heading", { name: text("profileModal.jvmArgs") })).toHaveCount(
      0,
    );
    expect(await fits()).toEqual({ vertical: true, horizontal: true });

    await dialog.getByRole("button", { name: text("profileModal.changeVersion") }).click();
    await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toBeVisible();
    await expect(dialog.getByRole("radiogroup", { name: text("create.filterLabel") })).toHaveCount(
      0,
    );
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await settingsButton(page).click();
    await page
      .getByRole("dialog", { name: text("settings.title") })
      .getByRole("radio", { name: "Русский" })
      .click();
    await page.getByRole("button", { name: textRu("common.done") }).click();

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: textRu("profile.settings") }).click();
    const ruDialog = page.getByRole("dialog");
    await expect(ruDialog).toBeVisible();
    expect(await ruDialog.getByRole("heading", { level: 3 }).allTextContents()).toEqual([
      textRu("profileModal.details"),
      textRu("profileModal.files"),
    ]);
    expect(await fits()).toEqual({ vertical: true, horizontal: true });
  });

  test("shows profile status, playtime and files note, and renames (including case-only and duplicate refusal)", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(page.getByRole("status")).toHaveCount(0);

    const dialog = await openProfileSettings(page);
    await expect(dialog.getByRole("heading", { name: SURVIVAL.name })).toBeVisible();
    await expect(dialog.getByText("Vanilla 1.21.4")).toBeVisible();
    await expect(dialog.getByText(text("profileModal.statusReady"))).toBeVisible();
    await expect(dialog.getByText("1h 30m")).toBeVisible();
    await expect(dialog.getByText(text("profileModal.filesNoteVanilla"))).toBeVisible();

    const name = dialog.getByLabel(text("create.name"));
    const save = dialog.getByRole("button", { name: text("profileModal.saveName") });
    await expect(name).toHaveValue(SURVIVAL.name);
    await expect(save).toBeDisabled();

    // Duplicate of another profile is refused inline before calling the backend
    await name.fill(CREATIVE.name);
    await expect(dialog.getByRole("alert")).toHaveText(text("validate.nameDuplicate"));
    await expect(save).toBeDisabled();
    expect(await backend.calls("rename_profile")).toHaveLength(0);

    // Case-only rename of own profile is allowed
    await name.fill("survival");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
    await expect(save).toBeEnabled();
    await save.click();
    await expect(dialog.getByRole("heading", { name: "survival" })).toBeVisible();
    await expect
      .poll(async () => (await backend.calls("rename_profile")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id, name: "survival" }]);
    expect((await backend.state()).profiles[0]?.name).toBe("survival");

    // Full rename updates header and dock trigger immediately
    await name.fill("Weekend World");
    await save.click();
    await expect(dialog.getByRole("heading", { name: "Weekend World" })).toBeVisible();

    // Backend duplicate_name refusal is worded cleanly
    await backend.failNext("rename_profile", "duplicate_name", { name: CREATIVE.name });
    await name.fill("Taken Name");
    await save.click();
    await expect(dialog.getByRole("alert")).toHaveText(
      text("error.profile.duplicate_name", { name: CREATIVE.name }),
    );

    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Weekend World" }),
    );

    // Modded profile shows loader detail and modded files note; pending profile shows Needs setup
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(`^${CREATIVE.name}`) }).click();
    const moddedDialog = await openProfileSettings(page);
    await expect(moddedDialog.getByText("Fabric 1.21.4 · 0.16.9")).toBeVisible();
    await expect(moddedDialog.getByText(text("profileModal.filesNoteModded"))).toBeVisible();
    await moddedDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(`^${FRESH.name}`) }).click();
    const pendingDialog = await openProfileSettings(page);
    await expect(pendingDialog.getByText(text("profileModal.statusPending"))).toBeVisible();
    await pendingDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("opens the profile folder from menu and dialog, and hands off Verify files and Remove from Clade…", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // Direct menu item
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.openFolder") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("open_profile_folder")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id }]);

    // Inside ProfileDialog + open_failed
    const dialog = await openProfileSettings(page);
    await dialog.getByRole("button", { name: text("profile.openFolder") }).click();
    await expect.poll(async () => (await backend.calls("open_profile_folder")).length).toBe(2);

    await backend.failNext("open_profile_folder", "open_failed");
    await dialog.getByRole("button", { name: text("profile.openFolder") }).click();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.profile.open_failed"));

    // Verify files handoff
    await dialog.getByRole("button", { name: text("profile.verify") }).click();
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id, verify: true }]);
    await backend.finishPrepare();
    const readyDialog = page.getByRole("dialog", { name: text("prepare.ready") });
    await expect(readyDialog).toBeVisible();
    await readyDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // Remove from Clade… handoff
    const reopened = await openProfileSettings(page);
    await reopened.getByRole("button", { name: text("profile.remove") }).click();
    const removeDialog = page.getByRole("dialog", {
      name: text("removeProfile.title", { profile: SURVIVAL.name }),
    });
    await expect(removeDialog).toBeVisible();
    await removeDialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("duplicates a profile and changes its Minecraft version without touching the original", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openProfileSettings(page);

    // Duplicate "Survival" -> "Survival (2)" and switch straight onto the new copy
    await dialog.getByRole("button", { name: text("profileModal.duplicate") }).click();
    await expect
      .poll(async () => (await backend.calls("duplicate_profile")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id }]);
    await expect(dialog.getByRole("heading", { name: "Survival (2)" })).toBeVisible();
    await expect(dialog.getByLabel(text("create.name"))).toHaveValue("Survival (2)");

    // Change the copy's Minecraft version to 1.21.1 -> flips copy to Needs setup + dock Set up
    await dialog.getByRole("button", { name: text("profileModal.changeVersion") }).click();
    await dialog.getByRole("combobox", { name: text("create.mcVersion") }).click();
    await page.getByRole("option", { name: /^1\.21\.1/ }).click();
    await dialog.getByRole("button", { name: text("profileModal.saveVersion") }).click();

    await expect(
      dialog.getByText(text("profileModal.statusPending"), { exact: true }),
    ).toBeVisible();
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Survival (2)" }),
    );
    await expect(playButton(page)).toHaveText(text("play.setup"));

    // Original "Survival" stays Ready on 1.21.4
    const snap = await backend.state();
    const orig = snap.profiles.find((p) => p.id === SURVIVAL.id);
    const copy = snap.profiles.find((p) => p.name === "Survival (2)");
    expect(orig?.minecraft).toBe("1.21.4");
    expect(orig?.ready).toBe(true);
    expect(copy?.minecraft).toBe("1.21.1");
    expect(copy?.ready).toBe(false);
  });
});

test.describe("profile settings: Advanced mode", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

  test("switches between Launcher default and Custom memory with 256 MiB keyboard steps and words invalid_memory", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openProfileSettings(page);

    const defaultRadio = dialog.getByRole("radio", { name: text("profileModal.memoryDefault") });
    const customRadio = dialog.getByRole("radio", { name: text("settings.memory.custom") });
    await expect(defaultRadio).toBeChecked();
    await expect(
      dialog.getByText(text("profileModal.memoryDefaultNote", { value: "4 GB" })),
    ).toBeVisible();

    await customRadio.click();
    await expect(customRadio).toBeChecked();
    const slider = dialog.getByRole("slider", { name: text("settings.memory.slider") });
    await expect(slider).toHaveValue("4096");
    await expect(slider).toHaveAttribute("aria-valuetext", "4 GB");
    await expect.poll(async () => (await backend.state()).profiles[0]?.memoryMb).toBe(4096);

    await slider.focus();
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await expect(slider).toHaveValue("4608");
    await expect(slider).toHaveAttribute("aria-valuetext", "4.5 GB");
    await expect.poll(async () => (await backend.state()).profiles[0]?.memoryMb).toBe(4608);

    await page.keyboard.press("End");
    await expect(slider).toHaveValue("12288");
    await expect(slider).toHaveAttribute("aria-valuetext", "12 GB");
    await expect.poll(async () => (await backend.state()).profiles[0]?.memoryMb).toBe(12288);

    await page.keyboard.press("Home");
    await expect(slider).toHaveValue("1024");
    await expect(slider).toHaveAttribute("aria-valuetext", "1 GB");
    await expect.poll(async () => (await backend.state()).profiles[0]?.memoryMb).toBe(1024);

    // Backend invalid_memory refusal formats {min} and {max}
    await backend.failNext("set_profile_memory", "invalid_memory", { min: 1024, max: 12288 });
    await page.keyboard.press("ArrowRight");
    await expect(dialog.getByRole("alert")).toHaveText(
      text("error.profile.invalid_memory", { min: "1 GB", max: "12 GB" }),
    );

    // Back to Launcher default
    await defaultRadio.click();
    await expect(defaultRadio).toBeChecked();
    await expect(
      dialog.getByText(text("profileModal.memoryDefaultNote", { value: "4 GB" })),
    ).toBeVisible();
    await expect.poll(async () => (await backend.state()).profiles[0]?.memoryMb).toBeNull();
  });

  test("configures Window size and JVM arguments and explains invalid values", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openProfileSettings(page);

    const resGroup = dialog.getByRole("radiogroup", { name: text("profileModal.resolution") });
    await expect(
      resGroup.getByRole("radio", { name: text("profileModal.resolutionDefault") }),
    ).toBeChecked();
    await expect(dialog.getByText(text("profileModal.resolutionDefaultNote"))).toBeVisible();

    await resGroup.getByRole("radio", { name: text("profileModal.resolutionFixed") }).click();
    await expect
      .poll(async () => (await backend.state()).profiles[0]?.resolution)
      .toEqual({ width: 1280, height: 720 });

    const width = dialog.getByLabel(text("profileModal.resolutionWidth"));
    const height = dialog.getByLabel(text("profileModal.resolutionHeight"));
    const applySize = dialog.getByRole("button", { name: text("profileModal.applyResolution") });
    await expect(width).toHaveValue("1280");
    await expect(height).toHaveValue("720");
    await expect(applySize).toBeDisabled();

    // Out-of-range dimensions are caught cleanly without calling the backend
    await width.fill("100");
    await applySize.click();
    await expect(dialog.getByRole("alert")).toHaveText(
      text("error.profile.invalid_resolution", {
        minWidth: 320,
        maxWidth: 7680,
        minHeight: 240,
        maxHeight: 4320,
      }),
    );
    expect(await backend.calls("set_profile_resolution")).toHaveLength(1);

    await width.fill("1920");
    await height.fill("1080");
    await applySize.click();
    await expect
      .poll(async () => (await backend.state()).profiles[0]?.resolution)
      .toEqual({ width: 1920, height: 1080 });

    await resGroup.getByRole("radio", { name: text("profileModal.resolutionDefault") }).click();
    await expect.poll(async () => (await backend.state()).profiles[0]?.resolution).toBeNull();

    // JVM arguments
    const jvmInput = dialog.getByLabel(text("profileModal.jvmArgs"));
    const applyFlags = dialog.getByRole("button", { name: text("profileModal.applyJvmArgs") });
    await expect(applyFlags).toBeDisabled();

    await jvmInput.fill("-XX:+UseStringDeduplication -Dfml.readTimeout=120");
    await expect(applyFlags).toBeEnabled();
    await applyFlags.click();
    await expect
      .poll(async () => (await backend.state()).profiles[0]?.jvmArgs)
      .toEqual(["-XX:+UseStringDeduplication", "-Dfml.readTimeout=120"]);
    await expect(applyFlags).toBeDisabled();

    // Forbidden flag refused by backend
    await jvmInput.fill("-Xmx4G");
    await applyFlags.click();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.profile.invalid_jvm_args"));

    // Clearing flags
    await jvmInput.fill("");
    await applyFlags.click();
    await expect.poll(async () => (await backend.state()).profiles[0]?.jvmArgs).toEqual([]);
  });

  test("locks edits, verification and removal while the game runs, while Open profile folder and Done still work", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // Set custom memory and fixed window size first so all controls are visible during the lock check
    const initial = await openProfileSettings(page);
    await initial.getByRole("radio", { name: text("settings.memory.custom") }).click();
    await expect(
      initial.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toBeVisible();
    await initial.getByRole("radio", { name: text("profileModal.resolutionFixed") }).click();
    await expect(initial.getByLabel(text("profileModal.resolutionWidth"))).toBeVisible();
    await initial.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await playButton(page).click();
    await backend.playing(SURVIVAL.id);
    await expect(playButton(page)).toHaveText(text("play.playing"));

    const dialog = await openProfileSettings(page);
    await expect(dialog.getByLabel(text("create.name"))).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("profileModal.saveName") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("radio", { name: text("profileModal.memoryDefault") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("radio", { name: text("settings.memory.custom") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("radio", { name: text("profileModal.resolutionDefault") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("radio", { name: text("profileModal.resolutionFixed") }),
    ).toBeDisabled();
    await expect(dialog.getByLabel(text("profileModal.resolutionWidth"))).toBeDisabled();
    await expect(dialog.getByLabel(text("profileModal.resolutionHeight"))).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("profileModal.applyResolution") }),
    ).toBeDisabled();
    await expect(dialog.getByLabel(text("profileModal.jvmArgs"))).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("profileModal.applyJvmArgs") }),
    ).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("profile.verify") })).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("profile.remove") })).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("profileModal.changeVersion") }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("profileModal.duplicate") }),
    ).toBeDisabled();

    const openFolder = dialog.getByRole("button", { name: text("profile.openFolder") });
    await expect(openFolder).toBeEnabled();
    await openFolder.click();
    await expect
      .poll(async () => (await backend.calls("open_profile_folder")).map((c) => c.args))
      .toEqual([{ id: SURVIVAL.id }]);

    const done = dialog.getByRole("button", { name: text("common.done") });
    await expect(done).toBeEnabled();
    await done.click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("fits inside the minimum 960x600 window in both English and Russian (Custom memory and Fixed window size)", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();

    const fits = async () =>
      page.getByRole("dialog").evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      }));

    // English: defaults and expanded Custom memory + Fixed window size
    const enDialog = await openProfileSettings(page);
    expect(await fits()).toEqual({ vertical: true, horizontal: true });

    await enDialog.getByRole("radio", { name: text("settings.memory.custom") }).click();
    await enDialog.getByRole("radio", { name: text("profileModal.resolutionFixed") }).click();
    await expect(
      enDialog.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toBeVisible();
    await expect(enDialog.getByLabel(text("profileModal.resolutionWidth"))).toBeVisible();
    expect(await fits()).toEqual({ vertical: true, horizontal: true });

    await enDialog.getByRole("radio", { name: text("profileModal.memoryDefault") }).click();
    await enDialog.getByRole("radio", { name: text("profileModal.resolutionDefault") }).click();
    await enDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // Switch to Russian
    await settingsButton(page).click();
    await page
      .getByRole("dialog", { name: text("settings.title") })
      .getByRole("radio", { name: "Русский" })
      .click();
    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    // Russian: defaults and expanded Custom memory + Fixed window size
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: textRu("profile.settings") }).click();
    const ruDialog = page.getByRole("dialog");
    await expect(ruDialog).toBeVisible();
    await expect(ruDialog.getByText(textRu("profileModal.details"), { exact: true })).toBeVisible();
    await expect(
      ruDialog.getByText(textRu("profileModal.statusReady"), { exact: true }),
    ).toBeVisible();
    await expect(
      ruDialog.getByText(textRu("profileModal.memoryDefaultNote", { value: "4 ГБ" })),
    ).toBeVisible();
    expect(await fits()).toEqual({ vertical: true, horizontal: true });

    await ruDialog.getByRole("radio", { name: textRu("settings.memory.custom") }).click();
    await ruDialog.getByRole("radio", { name: textRu("profileModal.resolutionFixed") }).click();
    await expect(
      ruDialog.getByRole("slider", { name: textRu("settings.memory.slider") }),
    ).toBeVisible();
    await expect(ruDialog.getByLabel(textRu("profileModal.resolutionWidth"))).toBeVisible();
    expect(await fits()).toEqual({ vertical: true, horizontal: true });
  });
});

test.describe("unreadable profile folders recovery banner", () => {
  test.use({
    backendConfig: {
      unreadable: ["broken-world", "corrupt-pack"],
      ui: { ...DEFAULT_CONFIG.ui },
    },
  });

  test("shows a calm status strip above the dock, opens the profiles folder, localizes to Russian, and fits 960x600", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();

    const status = page.getByRole("status");
    await expect(status).toBeVisible();
    await expect(status).toContainText(
      renderMessage("en", en["profile.unreadableNotice"], {
        count: 2,
        folders: "broken-world, corrupt-pack",
      }),
    );

    const statusBox = await status.boundingBox();
    const dockBox = await dock(page).boundingBox();
    expect(statusBox).not.toBeNull();
    expect(dockBox).not.toBeNull();
    if (statusBox && dockBox) {
      expect(statusBox.y + statusBox.height).toBeLessThanOrEqual(dockBox.y);
    }

    const pageFitsEn = await page.evaluate(() => ({
      horizontal: document.documentElement.scrollWidth <= document.documentElement.clientWidth,
      vertical: document.documentElement.scrollHeight <= document.documentElement.clientHeight,
    }));
    expect(pageFitsEn).toEqual({ horizontal: true, vertical: true });

    await status.getByRole("button", { name: text("profile.openInstancesFolder") }).click();
    await expect.poll(async () => (await backend.calls("open_instances_folder")).length).toBe(1);

    // Russian localization + 960x600 fit
    await backend.emit("ui-settings-changed", { ...DEFAULT_CONFIG.ui, language: "ru" });
    await expect(status).toContainText(
      renderMessage("ru", ru["profile.unreadableNotice"], {
        count: 2,
        folders: "broken-world, corrupt-pack",
      }),
    );
    await expect(
      status.getByRole("button", { name: textRu("profile.openInstancesFolder") }),
    ).toBeVisible();

    const pageFitsRu = await page.evaluate(() => ({
      horizontal: document.documentElement.scrollWidth <= document.documentElement.clientWidth,
      vertical: document.documentElement.scrollHeight <= document.documentElement.clientHeight,
    }));
    expect(pageFitsRu).toEqual({ horizontal: true, vertical: true });
  });
});
