import type { Locator, Page } from "@playwright/test";
import { DEFAULT_CONFIG, FRESH } from "./support/fixtures";
import { expect, test } from "./support/test";
import {
  accountTrigger,
  headline,
  playButton,
  profileTrigger,
  settingsButton,
  text,
} from "./support/ui";

/**
 * Verifies that `Tab` and `Shift+Tab` visit every expected focusable control in order and wrap
 * cleanly between the first and last controls without escaping the container.
 */
async function expectTabOrder(
  page: Page,
  controls: readonly Locator[],
  startIndex = 0,
): Promise<void> {
  expect(controls.length).toBeGreaterThan(0);
  await expect(controls[startIndex]).toBeFocused();

  for (let step = 1; step <= controls.length; step++) {
    await page.keyboard.press("Tab");
    await expect(controls[(startIndex + step) % controls.length]).toBeFocused();
  }

  for (let step = 1; step <= controls.length; step++) {
    await page.keyboard.press("Shift+Tab");
    const index = (startIndex - step + controls.length * 2) % controls.length;
    await expect(controls[index]).toBeFocused();
  }
}

async function getOutlineStyle(
  locator: Locator,
): Promise<{ style: string; width: string; color: string }> {
  return locator.evaluate((el) => {
    const cs = getComputedStyle(el);
    return {
      style: cs.outlineStyle,
      width: cs.outlineWidth,
      color: cs.outlineColor,
    };
  });
}

async function expectFocusRingVisible(locator: Locator): Promise<void> {
  await expect
    .poll(async () => {
      const { style, width, color } = await getOutlineStyle(locator);
      return (
        style !== "none" &&
        width !== "0px" &&
        color !== "rgba(0, 0, 0, 0)" &&
        color !== "transparent"
      );
    })
    .toBe(true);
}

async function expectFocusRingCleared(locator: Locator): Promise<void> {
  await expect
    .poll(async () => {
      const { style, color } = await getOutlineStyle(locator);
      return style === "none" || color === "rgba(0, 0, 0, 0)" || color === "transparent";
    })
    .toBe(true);
}

test.describe("modal basics", () => {
  test("Settings (Simple): focus moves in, wraps forwards and backwards, toggles switch, and returns on close", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const trigger = settingsButton(page);
    await trigger.focus();
    await page.keyboard.press("Enter");

    const dialog = page.getByRole("dialog", { name: text("settings.title") });
    await expect(dialog).toBeVisible();

    const simpleRadio = dialog
      .getByRole("radiogroup", { name: text("settings.mode") })
      .getByRole("radio", { name: text("settings.mode.simple") });
    const englishRadio = dialog
      .getByRole("radiogroup", { name: text("settings.language") })
      .getByRole("radio", { name: "English" });
    const themeSystemRadio = dialog
      .getByRole("radiogroup", { name: text("settings.theme") })
      .getByRole("radio", { name: text("settings.theme.system") });
    const soundSwitch = dialog.getByRole("switch", { name: text("settings.sound") });
    const volumeSlider = dialog.getByRole("slider", { name: text("settings.volume") });
    const openFolderBtn = dialog.getByRole("button", { name: text("settings.openFolder") });
    const resetBtn = dialog.getByRole("button", { name: text("settings.reset") });
    const catBtn = dialog.getByRole("button", { name: text("settings.cat") });
    const checkUpdatesBtn = dialog.getByRole("button", { name: text("settings.checkUpdates") });
    const doneBtn = dialog.getByRole("button", { name: text("common.done") });

    await expectTabOrder(page, [
      simpleRadio,
      englishRadio,
      themeSystemRadio,
      soundSwitch,
      volumeSlider,
      openFolderBtn,
      resetBtn,
      catBtn,
      checkUpdatesBtn,
      doneBtn,
    ]);

    // Sound switch can be toggled with Space and Enter from the keyboard.
    await soundSwitch.focus();
    await expect(soundSwitch).toBeChecked();
    await page.keyboard.press("Space");
    await expect(soundSwitch).not.toBeChecked();
    await expect(volumeSlider).toBeDisabled();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(false);

    await page.keyboard.press("Enter");
    await expect(soundSwitch).toBeChecked();
    await expect(volumeSlider).toBeEnabled();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(true);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(trigger).toBeFocused();
  });

  test("Settings (Advanced) traps focus across all sections and locks Escape during Reset confirmation", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const trigger = settingsButton(page);
    await trigger.focus();
    await page.keyboard.press("Enter");

    const dialog = page.getByRole("dialog", { name: text("settings.title") });
    const modeGroup = dialog.getByRole("radiogroup", { name: text("settings.mode") });
    const simpleRadio = modeGroup.getByRole("radio", { name: text("settings.mode.simple") });
    const advancedRadio = modeGroup.getByRole("radio", { name: text("settings.mode.advanced") });
    await expect(simpleRadio).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(advancedRadio).toBeChecked();
    await expect(advancedRadio).toBeFocused();

    await expectTabOrder(page, [
      advancedRadio,
      dialog
        .getByRole("radiogroup", { name: text("settings.language") })
        .getByRole("radio", { name: "English" }),
      dialog
        .getByRole("radiogroup", { name: text("settings.theme") })
        .getByRole("radio", { name: text("settings.theme.system") }),
      dialog.getByRole("switch", { name: text("settings.sound") }),
      dialog.getByRole("slider", { name: text("settings.volume") }),
      dialog
        .getByRole("radiogroup", { name: text("settings.motion") })
        .getByRole("radio", { name: text("settings.motion.system") }),
      dialog
        .getByRole("radiogroup", { name: text("settings.transparency") })
        .getByRole("radio", { name: text("settings.transparency.system") }),
      dialog
        .getByRole("radiogroup", { name: text("settings.memory") })
        .getByRole("radio", { name: text("settings.memory.auto") }),
      dialog
        .getByRole("radiogroup", { name: text("settings.onLaunch") })
        .getByRole("radio", { name: text("settings.onLaunch.embed") }),
      dialog
        .getByRole("radiogroup", { name: text("settings.onExit") })
        .getByRole("radio", { name: text("settings.onExit.show") }),
      dialog.getByRole("button", { name: text("settings.openFolder") }),
      dialog.getByRole("button", { name: text("common.openLogs") }),
      dialog.getByRole("button", { name: text("settings.exportDiagnostics") }),
      dialog.getByRole("button", { name: text("settings.reset") }),
      dialog.getByRole("button", { name: text("settings.cat") }),
      dialog.getByRole("button", { name: text("settings.checkUpdates") }),
      dialog.getByRole("button", { name: text("common.done") }),
    ]);

    const resetBtn = dialog.getByRole("button", { name: text("settings.reset") });
    await resetBtn.focus();
    await page.keyboard.press("Enter");

    const keepBtn = dialog.getByRole("button", { name: text("settings.resetKeep") });
    const confirmResetBtn = dialog.getByRole("button", { name: text("settings.resetDo") });
    const catBtn = dialog.getByRole("button", { name: text("settings.cat") });
    await expect(keepBtn).toBeFocused();

    // Locked during confirmation: Escape must not dismiss the dialog.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await expect(keepBtn).toBeVisible();

    await expectTabOrder(page, [keepBtn, confirmResetBtn, catBtn]);

    await page.keyboard.press("Enter");
    await expect(resetBtn).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(trigger).toBeFocused();
  });

  test("Create profile focuses Name, skips inert modded controls when Vanilla, visits them when Modded, and restores focus to trigger", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();

    const dialog = page.getByRole("dialog", { name: text("create.title") });
    const nameInput = dialog.getByLabel(text("create.name"));
    await expect(nameInput).toBeFocused();

    const versionCombo = dialog.getByRole("combobox", { name: text("create.mcVersion") });
    await expect(versionCombo).toBeVisible();
    await nameInput.fill("Alder");
    await nameInput.focus();

    const typeGroup = dialog.getByRole("radiogroup", { name: text("create.typeLabel") });
    const vanillaRadio = typeGroup.getByRole("radio", { name: text("profile.vanilla") });
    const moddedRadio = typeGroup.getByRole("radio", { name: text("profile.modded") });
    const cancelBtn = dialog.getByRole("button", { name: text("common.cancel") });
    const submitBtn = dialog.getByRole("button", { name: text("create.submit") });

    // Vanilla selected: collapsed modded loader section is `inert` and skipped on Tab / Shift+Tab.
    await expectTabOrder(page, [nameInput, versionCombo, vanillaRadio, cancelBtn, submitBtn]);

    // Modded selected: loader radiogroup becomes reachable between type and footer buttons.
    await vanillaRadio.focus();
    await page.keyboard.press("ArrowRight");
    await expect(moddedRadio).toBeChecked();
    const fabricRadio = dialog
      .getByRole("radiogroup", { name: text("create.loaderLabel") })
      .getByRole("radio", { name: "Fabric" });
    await expect(fabricRadio).toBeEnabled();

    await nameInput.focus();
    await expectTabOrder(page, [
      nameInput,
      versionCombo,
      moddedRadio,
      fabricRadio,
      cancelBtn,
      submitBtn,
    ]);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toBeFocused();
  });
});

test.describe("keyboard operation", () => {
  test("full-page tab order traverses WindowChrome, Hero, and Dock, and Play supports Enter and Space with busy ARIA state", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    const expectedOrder = [
      page.getByRole("button", { name: text("window.wordmark") }),
      page.getByRole("button", { name: text("window.minimize") }),
      page.getByRole("button", { name: text("window.close") }),
      headline(page).getByRole("button"),
      accountTrigger(page),
      profileTrigger(page),
      playButton(page),
      settingsButton(page),
    ];

    for (const control of expectedOrder) {
      await page.keyboard.press("Tab");
      await expect(control).toBeFocused();
    }

    // Navigate back to PlayButton via Shift+Tab (no manual .focus()).
    await page.keyboard.press("Shift+Tab");
    await expect(playButton(page)).toBeFocused();

    await page.keyboard.press("Enter");
    await expect(playButton(page)).toHaveText(text("play.launching"));
    await expect(playButton(page)).toHaveAttribute("aria-busy", "true");
    await expect(playButton(page)).toBeDisabled();
    expect(await backend.calls("launch_profile")).toHaveLength(1);

    await backend.stopped("survival", null);
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toHaveAttribute("aria-busy", "false");
    await expect(playButton(page)).toBeEnabled();

    // Reach PlayButton via Shift+Tab from Settings again and activate with Space.
    await settingsButton(page).focus();
    await page.keyboard.press("Shift+Tab");
    await expect(playButton(page)).toBeFocused();

    await page.keyboard.press("Space");
    await expect(playButton(page)).toHaveText(text("play.launching"));
    await expect(playButton(page)).toHaveAttribute("aria-busy", "true");
    await expect(playButton(page)).toBeDisabled();
    expect(await backend.calls("launch_profile")).toHaveLength(2);
  });

  test("the focus ring appears for the keyboard, clears on clicking the focused element, and ignores modifier/shortcut keys", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const html = page.locator("html");
    await expect(html).toHaveAttribute("data-input", "pointer");

    for (const key of ["Shift", "Control", "Alt", "Meta", "Control+a"]) {
      await page.keyboard.press(key);
      await expect(html).toHaveAttribute("data-input", "pointer");
    }

    const wordmark = page.getByRole("button", { name: text("window.wordmark") });
    await page.keyboard.press("Tab");
    await expect(wordmark).toBeFocused();
    await expect(html).toHaveAttribute("data-input", "keyboard");
    await expectFocusRingVisible(wordmark);

    await wordmark.click();
    await expect(html).toHaveAttribute("data-input", "pointer");
    await expectFocusRingCleared(wordmark);

    const heroBtn = headline(page).getByRole("button");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    await expect(heroBtn).toBeFocused();
    await expect(html).toHaveAttribute("data-input", "keyboard");
    await expectFocusRingVisible(heroBtn);

    await heroBtn.click();
    await expect(html).toHaveAttribute("data-input", "pointer");
    await expectFocusRingCleared(heroBtn);
  });

  test("segmented choices implement roving tabIndex, all directional keys, wrap-around, disabled skipping, and Select combobox keyboard control", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });

    const typeGroup = dialog.getByRole("radiogroup", { name: text("create.typeLabel") });
    const vanilla = typeGroup.getByRole("radio", { name: text("profile.vanilla") });
    const modded = typeGroup.getByRole("radio", { name: text("profile.modded") });

    await expect(vanilla).toHaveAttribute("tabindex", "0");
    await expect(modded).toHaveAttribute("tabindex", "-1");
    await vanilla.focus();
    await page.keyboard.press("ArrowRight");
    await expect(modded).toBeChecked();
    await expect(modded).toBeFocused();
    await expect(modded).toHaveAttribute("tabindex", "0");
    await expect(vanilla).toHaveAttribute("tabindex", "-1");

    await page.keyboard.press("ArrowLeft");
    await expect(vanilla).toBeChecked();
    await expect(vanilla).toBeFocused();
    await expect(vanilla).toHaveAttribute("tabindex", "0");
    await expect(modded).toHaveAttribute("tabindex", "-1");

    // Operate the Minecraft version Select combobox solely via keyboard to pick 1.20.1.
    const versionCombo = dialog.getByRole("combobox", { name: text("create.mcVersion") });
    await versionCombo.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("listbox")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("listbox")).toHaveCount(0);
    await expect(dialog).toBeVisible();
    await expect(versionCombo).toBeFocused();

    await page.keyboard.press("Enter");
    await expect(page.getByRole("listbox")).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(versionCombo).toContainText("1.20.1");
    await expect(versionCombo).toBeFocused();

    // Switch to Modded so the 4-loader Segmented group (with disabled NeoForge on 1.20.1) is active.
    await vanilla.focus();
    await page.keyboard.press("ArrowRight");

    const loaderGroup = dialog.getByRole("radiogroup", { name: text("create.loaderLabel") });
    const fabric = loaderGroup.getByRole("radio", { name: "Fabric", exact: true });
    const forge = loaderGroup.getByRole("radio", { name: "Forge", exact: true });
    const neoforge = loaderGroup.getByRole("radio", { name: "NeoForge", exact: true });
    const quilt = loaderGroup.getByRole("radio", { name: "Quilt", exact: true });

    await expect(fabric).toBeEnabled();
    await expect(neoforge).toBeDisabled();
    await expect(fabric).toBeChecked();
    await expect(fabric).toHaveAttribute("tabindex", "0");
    await expect(forge).toHaveAttribute("tabindex", "-1");
    await expect(neoforge).toHaveAttribute("tabindex", "-1");
    await expect(quilt).toHaveAttribute("tabindex", "-1");

    await fabric.focus();
    await page.keyboard.press("ArrowRight");
    await expect(forge).toBeChecked();
    await expect(forge).toBeFocused();
    await expect(forge).toHaveAttribute("tabindex", "0");
    await expect(fabric).toHaveAttribute("tabindex", "-1");

    // ArrowDown skips disabled NeoForge and lands on Quilt.
    await page.keyboard.press("ArrowDown");
    await expect(quilt).toBeChecked();
    await expect(quilt).toBeFocused();

    // ArrowRight wraps forward from Quilt to Fabric.
    await page.keyboard.press("ArrowRight");
    await expect(fabric).toBeChecked();
    await expect(fabric).toBeFocused();

    // ArrowUp wraps backward from Fabric to Quilt.
    await page.keyboard.press("ArrowUp");
    await expect(quilt).toBeChecked();
    await expect(quilt).toBeFocused();

    // ArrowLeft skips disabled NeoForge backward to Forge.
    await page.keyboard.press("ArrowLeft");
    await expect(forge).toBeChecked();
    await expect(forge).toBeFocused();

    // End / Home jump to the last / first enabled options.
    await page.keyboard.press("End");
    await expect(quilt).toBeChecked();
    await expect(quilt).toBeFocused();

    await page.keyboard.press("Home");
    await expect(fabric).toBeChecked();
    await expect(fabric).toBeFocused();
  });

  test("the account menu navigates its grid items, switches accounts, and opens Manage from the keyboard", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();

    await page.keyboard.press("Enter");
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio", { name: /^FixtureAlex/ })).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(
      menu.getByRole("menuitem", { name: text("account.manage", { name: "FixtureAlex" }) }),
    ).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(menu.getByRole("menuitemradio", { name: /^FixtureAlex/ })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(
      menu.getByRole("menuitem", { name: text("account.manage", { name: "FixtureAlex" }) }),
    ).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(menu.getByRole("menuitemradio", { name: /^FixtureBlake/ })).toBeFocused();
    await page.keyboard.press("Enter");

    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(accountTrigger(page)).toContainText("FixtureBlake");
    await expect(accountTrigger(page)).toBeFocused();
    await expect.poll(async () => (await backend.state()).active).toBe("acc-blake");

    await page.keyboard.press("Enter");
    const reopenMenu = page.getByRole("menu");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    const manageBlake = reopenMenu.getByRole("menuitem", {
      name: text("account.manage", { name: "FixtureBlake" }),
    });
    await expect(manageBlake).toBeFocused();
    await page.keyboard.press("Enter");

    const accountDialog = page.getByRole("dialog", { name: "FixtureBlake" });
    await expect(accountDialog).toBeVisible();
    await expect(accountDialog.getByRole("button", { name: text("common.done") })).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();
  });
});

test.describe("remaining modal keyboard flows", () => {
  test("AccountDialog traps focus, locks Escape on removal confirmation, and restores focus", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).focus();
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");

    const dialog = page.getByRole("dialog", { name: "FixtureAlex" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText(text("appearance.noCapes"))).toBeVisible();

    const classicRadio = dialog
      .getByRole("radiogroup", { name: text("accountModal.skinModel") })
      .getByRole("radio", { name: text("accountModal.skinClassic") });
    const choosePngBtn = dialog.getByRole("button", { name: text("accountModal.choosePng") });
    const restoreBtn = dialog.getByRole("button", { name: text("accountModal.restoreDefault") });
    const removeTrigger = dialog.getByRole("button", {
      name: text("accountModal.signOutRemove"),
    });
    const doneBtn = dialog.getByRole("button", { name: text("common.done") });

    await expectTabOrder(page, [classicRadio, choosePngBtn, restoreBtn, removeTrigger, doneBtn], 4);

    await removeTrigger.focus();
    await page.keyboard.press("Enter");
    const keepBtn = dialog.getByRole("button", { name: text("accountModal.keep") });
    const confirmRemoveBtn = dialog.getByRole("button", { name: text("accountModal.remove") });
    await expect(keepBtn).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await expectTabOrder(page, [keepBtn, confirmRemoveBtn]);

    await page.keyboard.press("Enter");
    await expect(removeTrigger).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();
  });

  test("ProfileDialog traps focus, restores focus from VersionEditor Cancel, and returns to trigger", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.settings") }).click();

    const dialog = page.getByRole("dialog", { name: "Survival" });
    await expect(dialog).toBeVisible();

    const nameInput = dialog.getByLabel(text("create.name"));
    const changeVersionBtn = dialog.getByRole("button", {
      name: text("profileModal.changeVersion"),
    });
    const openFolderBtn = dialog.getByRole("button", { name: text("profile.openFolder") });
    const verifyBtn = dialog.getByRole("button", { name: text("profile.verify") });
    const removeBtn = dialog.getByRole("button", { name: text("profile.remove") });
    const duplicateBtn = dialog.getByRole("button", { name: text("profileModal.duplicate") });
    const doneBtn = dialog.getByRole("button", { name: text("common.done") });

    await expectTabOrder(page, [
      nameInput,
      changeVersionBtn,
      openFolderBtn,
      verifyBtn,
      removeBtn,
      duplicateBtn,
      doneBtn,
    ]);

    await changeVersionBtn.focus();
    await page.keyboard.press("Enter");
    const versionCombo = dialog.getByRole("combobox");
    await expect(versionCombo).toBeVisible();
    await versionCombo.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("listbox")).toBeVisible();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(versionCombo).toContainText("1.21.1");

    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(changeVersionBtn).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toBeFocused();
  });

  test("RemoveProfileDialog traps focus between Cancel and Confirm and returns to trigger", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.remove") }).click();

    const dialog = page.getByRole("dialog", {
      name: text("removeProfile.title", { profile: "Survival" }),
    });
    await expect(dialog).toBeVisible();

    const cancelBtn = dialog.getByRole("button", { name: text("common.cancel") });
    const confirmBtn = dialog.getByRole("button", { name: text("removeProfile.confirm") });
    await expectTabOrder(page, [cancelBtn, confirmBtn]);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toBeFocused();
  });

  test("LaunchProblemDialog traps focus on ended crash and locks Escape during vault recovery", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const unlockDialog = page.getByRole("dialog", { name: text("problem.unlockTitle") });
    await expect(unlockDialog).toBeVisible();

    const passInput = unlockDialog.getByLabel(text("common.passphrase"));
    const forgotLink = unlockDialog.getByRole("button", { name: text("recovery.link") });
    const cancelBtn = unlockDialog.getByRole("button", { name: text("common.cancel") });
    await expectTabOrder(page, [passInput, forgotLink, cancelBtn]);

    await forgotLink.focus();
    await page.keyboard.press("Enter");
    const recoveryDialog = page.getByRole("dialog");
    const keepTryingBtn = recoveryDialog.getByRole("button", { name: text("recovery.keep") });
    const startFreshBtn = recoveryDialog.getByRole("button", { name: text("recovery.confirm") });
    await expect(keepTryingBtn).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(recoveryDialog).toBeVisible();
    await expectTabOrder(page, [keepTryingBtn, startFreshBtn]);

    await page.keyboard.press("Enter");
    await expect(forgotLink).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await backend.stopped("survival", { kind: "crashed", reason: "java_crash", code: 1 });
    const endedDialog = page.getByRole("dialog", { name: text("problem.endedTitle") });
    await expect(endedDialog).toBeVisible();
    const openLogsBtn = endedDialog.getByRole("button", { name: text("common.openLogs") });
    const closeBtn = endedDialog.getByRole("button", { name: text("common.close") });
    await expectTabOrder(page, [openLogsBtn, closeBtn], 1);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});

test.describe("framed WindowChrome tab order", () => {
  test.use({
    backendConfig: {
      embed: { active: true, instanceId: "survival", maximized: false },
    },
  });

  test("includes Maximize in WindowChrome tab order while a game is framed", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const expected = [
      page.getByRole("button", { name: text("window.wordmark") }),
      page.getByRole("button", { name: text("window.minimize") }),
      page.getByRole("button", { name: text("window.maximize") }),
      page.getByRole("button", { name: text("window.close") }),
    ];
    for (const btn of expected) {
      await page.keyboard.press("Tab");
      await expect(btn).toBeFocused();
    }
  });
});

test.describe("OnboardingDialog keyboard flow", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, onboardingDone: false } } });

  test("traps focus across all onboarding steps and skips on Escape", async ({ page, openApp }) => {
    await openApp();
    const welcomeDialog = page.getByRole("dialog", { name: text("onboarding.welcomeTitle") });
    await expect(welcomeDialog).toBeVisible();
    await expectTabOrder(
      page,
      [
        welcomeDialog.getByRole("button", { name: text("onboarding.skip") }),
        welcomeDialog.getByRole("button", { name: text("onboarding.continue") }),
      ],
      1,
    );

    await page.keyboard.press("Enter");
    const portableDialog = page.getByRole("dialog", { name: text("onboarding.portableTitle") });
    await expect(portableDialog).toBeVisible();
    await expectTabOrder(
      page,
      [
        portableDialog.getByRole("button", { name: text("onboarding.back"), exact: true }),
        portableDialog.getByRole("button", { name: text("onboarding.skip") }),
        portableDialog.getByRole("button", { name: text("onboarding.continue") }),
      ],
      2,
    );

    await page.keyboard.press("Enter");
    const readyDialog = page.getByRole("dialog", { name: text("onboarding.readyTitle") });
    await expect(readyDialog).toBeVisible();
    await expectTabOrder(
      page,
      [
        readyDialog.getByRole("button", { name: text("onboarding.createProfile") }),
        readyDialog.getByRole("button", { name: text("onboarding.addAccount") }),
        readyDialog.getByRole("button", { name: text("onboarding.back"), exact: true }),
        readyDialog.getByRole("button", { name: text("onboarding.finish") }),
      ],
      3,
    );

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});

test.describe("RemovedProfilesDialog keyboard flow", () => {
  test.use({
    backendConfig: {
      removed: [{ ...FRESH, folder: "old-fresh", removedAt: 1700000000 }],
    },
  });

  test("traps focus inside RemovedProfilesDialog and returns to profile trigger on Escape", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.removedMenu", { count: 1 }) }).click();

    const dialog = page.getByRole("dialog", { name: text("removedProfiles.title") });
    await expect(dialog).toBeVisible();

    const restoreBtn = dialog.getByRole("button", { name: text("removedProfiles.restore") });
    const openFolderBtn = dialog.getByRole("button", { name: text("removedProfiles.openFolder") });
    const doneBtn = dialog.getByRole("button", { name: text("common.done") });
    await expectTabOrder(page, [restoreBtn, openFolderBtn, doneBtn]);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(profileTrigger(page)).toBeFocused();
  });
});

test.describe("PrepareDialog keyboard flow", () => {
  test.use({ backendConfig: { selected: "fresh" } });

  test("traps focus during setup, moves to Done on completion, and restores focus to PlayButton", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).focus();
    await page.keyboard.press("Enter");

    const workingDialog = page.getByRole("dialog", { name: "Fresh" });
    await expect(workingDialog).toBeVisible();
    const cancelBtn = workingDialog.getByRole("button", { name: text("common.cancel") });
    await expectTabOrder(page, [cancelBtn]);

    await backend.finishPrepare();
    const readyDialog = page.getByRole("dialog", { name: text("prepare.ready") });
    await expect(readyDialog).toBeVisible();
    const doneBtn = readyDialog.getByRole("button", { name: text("common.done") });
    await expectTabOrder(page, [doneBtn]);

    await page.keyboard.press("Enter");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toBeFocused();
  });
});

test.describe("SignInDialog locked recovery flow", () => {
  test.use({ backendConfig: { vault: { exists: true, unlocked: false } } });

  test("traps focus in unlock phase, locks Escape during Start fresh?, and restores focus", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.unlockTitle") });
    await expect(dialog).toBeVisible();
    const passInput = dialog.getByLabel(text("common.passphrase"));
    const forgotLink = dialog.getByRole("button", { name: text("recovery.link") });
    const cancelBtn = dialog.getByRole("button", { name: text("common.cancel") });
    await expectTabOrder(page, [passInput, forgotLink, cancelBtn]);

    await forgotLink.focus();
    await page.keyboard.press("Enter");
    const recoveryDialog = page.getByRole("dialog");
    const keepTryingBtn = recoveryDialog.getByRole("button", { name: text("recovery.keep") });
    const startFreshBtn = recoveryDialog.getByRole("button", { name: text("recovery.confirm") });
    await expect(keepTryingBtn).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(recoveryDialog).toBeVisible();
    await expectTabOrder(page, [keepTryingBtn, startFreshBtn]);

    await page.keyboard.press("Enter");
    await expect(forgotLink).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();
  });
});

test.describe("visual accessibility and reduced-motion regressions", () => {
  test.describe("reduced motion keeps the Settings gear stationary", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" } } });

    test("does not rotate the settings gear icon on hover when reduced motion is enabled", async ({
      page,
      openApp,
    }) => {
      await openApp();
      const btn = settingsButton(page);
      await btn.hover();
      const transform = await btn
        .locator(".settings-gear")
        .evaluate((el) => getComputedStyle(el).transform);
      expect(transform === "none" || transform === "matrix(1, 0, 0, 1, 0, 0)").toBe(true);
    });
  });

  test.describe("dark theme contrast, SelectTrigger surface, and native range track", () => {
    test.use({
      backendConfig: {
        ui: { ...DEFAULT_CONFIG.ui, theme: "dark", uiMode: "advanced" },
      },
    });

    test("keeps Dark primary tokens >= 4.5:1, matches SelectTrigger to FIELD, and styles range track", async ({
      page,
      openApp,
    }) => {
      await openApp();
      const ratios = await page.evaluate(() => {
        const parseHex = (h: string) => {
          const s = h.trim().replace("#", "");
          return [0, 2, 4].map((i) => parseInt(s.slice(i, i + 2), 16));
        };
        const lum = ([r, g, b]: number[]) =>
          [r, g, b]
            .map((v) => {
              const x = v / 255;
              return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
            })
            .reduce((acc, c, i) => acc + c * [0.2126, 0.7152, 0.0722][i], 0);
        const cr = (a: string, b: string) => {
          const l1 = lum(parseHex(a));
          const l2 = lum(parseHex(b));
          return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
        };
        const cs = getComputedStyle(document.documentElement);
        return {
          hover: cr("#fbf6ee", cs.getPropertyValue("--clade-clay-hover")),
          active: cr("#fbf6ee", cs.getPropertyValue("--clade-clay-active")),
        };
      });
      expect(ratios.hover).toBeGreaterThanOrEqual(4.5);
      expect(ratios.active).toBeGreaterThanOrEqual(4.5);

      await profileTrigger(page).click();
      await page.getByRole("menuitem", { name: text("profile.create") }).click();
      const createDialog = page.getByRole("dialog", { name: text("create.title") });
      await expect(createDialog).toBeVisible();
      const colors = await createDialog.evaluate((el) => {
        const input = el.querySelector("#profile-name");
        const trigger = el.querySelector('[data-slot="select-trigger"]');
        return {
          inputBg: input ? getComputedStyle(input).backgroundColor : "",
          triggerBg: trigger ? getComputedStyle(trigger).backgroundColor : "",
        };
      });
      expect(colors.triggerBg).toBe(colors.inputBg);
      await page.keyboard.press("Escape");

      await settingsButton(page).click();
      const settingsDialog = page.getByRole("dialog", { name: text("settings.title") });
      await expect(settingsDialog).toBeVisible();
      const rangeAppearance = await settingsDialog
        .locator("#ui-volume")
        .evaluate((el) => getComputedStyle(el).appearance);
      expect(rangeAppearance).toBe("none");
    });
  });

  test.describe("forced-colors high contrast menu items", () => {
    test.use({ backendConfig: { runs: { survival: "playing" } } });

    test("forced-colors keeps disabled items opaque and overrides highlighted child text color", async ({
      page,
      openApp,
    }) => {
      await page.emulateMedia({ forcedColors: "active" });
      await openApp();

      await profileTrigger(page).click();
      const menu = page.getByRole("menu");
      await expect(menu).toBeVisible();
      const disabledItem = menu.getByRole("menuitem", { name: text("profile.remove") });
      await expect(disabledItem).toBeDisabled();
      const disabledOpacity = await disabledItem.evaluate((el) => getComputedStyle(el).opacity);
      expect(disabledOpacity).toBe("1");

      const radioItem = menu.getByRole("menuitemradio", { name: /Survival/ });
      await radioItem.focus();
      await expect(radioItem).toBeFocused();
      await expect
        .poll(() =>
          radioItem.evaluate((el) => {
            const parentColor = getComputedStyle(el).color;
            const spans = Array.from(el.querySelectorAll("span"));
            return (
              spans.length > 0 &&
              parentColor !== "rgb(246, 240, 230)" &&
              spans.every((s) => getComputedStyle(s).color === parentColor)
            );
          }),
        )
        .toBe(true);
    });
  });

  test.describe("light theme segmented hover response", () => {
    test.use({
      backendConfig: {
        ui: { ...DEFAULT_CONFIG.ui, theme: "light", uiMode: "advanced" },
      },
    });

    test("unselected Segmented option gains a visible background tint on hover in Light theme", async ({
      page,
      openApp,
    }) => {
      await openApp();
      await settingsButton(page).click();
      const dialog = page.getByRole("dialog", { name: text("settings.title") });
      await expect(dialog).toBeVisible();

      const onExitGroup = dialog.getByRole("radiogroup", {
        name: text("settings.onExit"),
      });
      const stayHidden = onExitGroup.getByRole("radio", {
        name: text("settings.onExit.hidden"),
      });
      await expect(stayHidden).toHaveAttribute("aria-checked", "false");

      const rest = await stayHidden.evaluate((el) => ({
        bg: getComputedStyle(el).backgroundColor,
        color: getComputedStyle(el).color,
      }));
      expect(rest.bg).toBe("rgba(0, 0, 0, 0)");

      await stayHidden.hover();
      await expect
        .poll(() =>
          stayHidden.evaluate((el) => ({
            bg: getComputedStyle(el).backgroundColor,
            color: getComputedStyle(el).color,
          })),
        )
        .not.toEqual(rest);
      const hovered = await stayHidden.evaluate((el) => ({
        bg: getComputedStyle(el).backgroundColor,
        color: getComputedStyle(el).color,
      }));
      expect(hovered.bg).not.toBe("rgba(0, 0, 0, 0)");
      expect(hovered.color).not.toBe(rest.color);
    });
  });
});
