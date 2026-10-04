import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { settingsButton, text, textRu } from "./support/ui";

const openSettings = async (page: Page) => {
  await settingsButton(page).click();
  const dialog = page.getByRole("dialog", { name: text("settings.title") });
  await expect(dialog).toBeVisible();
  return dialog;
};

test.describe("settings sections", () => {
  test("shows Interface, Minecraft, Behavior and Support, and nothing else", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const headings = await dialog.getByRole("heading", { level: 3 }).allTextContents();
    expect(headings).toEqual([
      text("settings.interface"),
      text("settings.minecraft"),
      text("settings.behavior"),
      text("settings.support"),
    ]);
  });

  test("keeps the whole screen in view at the smallest window, without scrolling", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    const dialog = await openSettings(page);
    const fits = await dialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1);
    expect(fits).toBe(true);
  });

  test("reads in Russian", async ({ page, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    await page.getByRole("radio", { name: "Русский" }).click();
    const dialog = page.getByRole("dialog", { name: textRu("settings.title") });
    const headings = await dialog.getByRole("heading", { level: 3 }).allTextContents();
    expect(headings).toEqual([
      textRu("settings.interface"),
      textRu("settings.minecraft"),
      textRu("settings.behavior"),
      textRu("settings.support"),
    ]);
    await expect(dialog.getByRole("radiogroup", { name: textRu("settings.motion") })).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: textRu("settings.exportDiagnostics") }),
    ).toBeVisible();
    await expect(dialog.getByRole("button", { name: textRu("settings.reset") })).toBeVisible();
  });
});

test.describe("reduced motion", () => {
  test("offers Follow Windows, On and Off, and applies the choice at once", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const group = dialog.getByRole("radiogroup", { name: text("settings.motion") });
    await expect(group.getByRole("radio", { name: text("settings.motion.system") })).toBeChecked();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "system");

    await group.getByRole("radio", { name: text("settings.motion.on") }).click();
    await expect(group.getByRole("radio", { name: text("settings.motion.on") })).toBeChecked();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "reduce");
    expect((await backend.calls("set_reduced_motion")).map((c) => c.args)).toEqual([
      { mode: "on" },
    ]);

    await group.getByRole("radio", { name: text("settings.motion.off") }).click();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "full");
    await group.getByRole("radio", { name: text("settings.motion.system") }).click();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "system");
    expect((await backend.state()).ui.reducedMotion).toBe("system");
  });

  test("a saved choice is applied from the first paint", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "system");
  });
});

test.describe("reduced motion saved as On", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" } } });

  test("is applied when the launcher starts", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "reduce");
    const dialog = await openSettings(page);
    await expect(dialog.getByRole("radio", { name: text("settings.motion.on") })).toBeChecked();
  });
});

test.describe("memory", () => {
  test("is Automatic by default and says what that means", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    const group = dialog.getByRole("radiogroup", { name: text("settings.memory") });
    await expect(group.getByRole("radio", { name: text("settings.memory.auto") })).toBeChecked();
    await expect(
      dialog.getByText(text("settings.memory.autoNote", { value: "4 GB" })),
    ).toBeVisible();
    await expect(dialog.getByRole("slider", { name: text("settings.memory.slider") })).toHaveCount(
      0,
    );
  });

  test("Custom shows a RAM slider limited to what the computer allows, and saves the value", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const group = dialog.getByRole("radiogroup", { name: text("settings.memory") });
    await group.getByRole("radio", { name: text("settings.memory.custom") }).click();

    const slider = dialog.getByRole("slider", { name: text("settings.memory.slider") });
    await expect(slider).toBeVisible();
    // the synthetic computer: 1 GB to 12 GB, starting from the automatic value
    await expect(slider).toHaveAttribute("min", "1024");
    await expect(slider).toHaveAttribute("max", "12288");
    await expect(slider).toHaveAttribute("aria-valuetext", "4 GB");
    expect((await backend.calls("set_memory")).map((c) => c.args)).toEqual([{ mb: 4096 }]);

    await slider.focus();
    await page.keyboard.press("ArrowRight");
    await expect(slider).toHaveAttribute("aria-valuetext", "4.3 GB");
    await expect.poll(async () => (await backend.state()).ui.memoryMb).toBe(4352);

    await page.keyboard.press("End");
    await expect(slider).toHaveAttribute("aria-valuetext", "12 GB");
    await expect.poll(async () => (await backend.state()).ui.memoryMb).toBe(12288);
    await page.keyboard.press("Home");
    await expect.poll(async () => (await backend.state()).ui.memoryMb).toBe(1024);
  });

  test("Automatic again goes back to the automatic policy", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    const group = dialog.getByRole("radiogroup", { name: text("settings.memory") });
    await group.getByRole("radio", { name: text("settings.memory.custom") }).click();
    await expect(
      dialog.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toBeVisible();
    await group.getByRole("radio", { name: text("settings.memory.auto") }).click();
    await expect(dialog.getByRole("slider", { name: text("settings.memory.slider") })).toHaveCount(
      0,
    );
    await expect.poll(async () => (await backend.state()).ui.memoryMb).toBeNull();
  });

  test("a value the backend refuses is explained, in words", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await backend.failNext("set_memory", "invalid_memory", { min: 1024, max: 12288 });
    await dialog
      .getByRole("radiogroup", { name: text("settings.memory") })
      .getByRole("radio", { name: text("settings.memory.custom") })
      .click();
    await expect(dialog.getByRole("alert")).toHaveText(
      text("error.settings.invalid_memory", { min: "1 GB", max: "12 GB" }),
    );
  });
});

test.describe("memory saved as Custom", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, memoryMb: 6144 } } });

  test("opens on Custom with the saved amount", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByRole("radio", { name: text("settings.memory.custom") })).toBeChecked();
    await expect(
      dialog.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toHaveAttribute("aria-valuetext", "6 GB");
  });
});

test.describe("behavior around the game", () => {
  test("keeps Clade open and shows it again by default, and saves each choice", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const onLaunch = dialog.getByRole("radiogroup", { name: text("settings.onLaunch") });
    const onExit = dialog.getByRole("radiogroup", { name: text("settings.onExit") });
    await expect(
      onLaunch.getByRole("radio", { name: text("settings.onLaunch.keep") }),
    ).toBeChecked();
    await expect(onExit.getByRole("radio", { name: text("settings.onExit.show") })).toBeChecked();
    await expect(dialog.getByText(text("settings.onExit.hiddenNote"))).toHaveCount(0);

    await onLaunch.getByRole("radio", { name: text("settings.onLaunch.hide") }).click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("hide");
    await onExit.getByRole("radio", { name: text("settings.onExit.hidden") }).click();
    await expect.poll(async () => (await backend.state()).ui.onExit).toBe("stay_hidden");
    // being honest about what staying hidden means
    await expect(dialog.getByText(text("settings.onExit.hiddenNote"))).toBeVisible();

    await onLaunch.getByRole("radio", { name: text("settings.onLaunch.keep") }).click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("keep_open");
    expect((await backend.calls("set_behavior")).map((c) => c.args)).toEqual([
      { onLaunch: "hide" },
      { onExit: "stay_hidden" },
      { onLaunch: "keep_open" },
    ]);
  });
});

test.describe("support actions", () => {
  test("Open Clade folder and Open logs ask the backend, which opens its own folders", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    await dialog.getByRole("button", { name: text("settings.openFolder") }).click();
    await dialog.getByRole("button", { name: text("common.openLogs") }).click();
    await expect
      .poll(async () => (await backend.calls()).map((c) => c.cmd).filter((c) => /^open_/.test(c)))
      .toEqual(["open_clade_folder", "open_logs_folder"]);
    // no folder or path is ever sent from the page
    expect((await backend.calls("open_clade_folder"))[0]?.args).toEqual({});
    await expect(dialog.getByRole("alert")).toHaveCount(0);
  });

  test("a folder that cannot be opened is reported in plain words", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    await backend.failNext("open_logs_folder", "open_failed");
    await dialog.getByRole("button", { name: text("common.openLogs") }).click();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.settings.open_failed"));
  });

  test("Export diagnostics reports the file it made", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByText(text("settings.diagnosticsNote"))).toBeVisible();
    await dialog.getByRole("button", { name: text("settings.exportDiagnostics") }).click();
    await expect(dialog.getByRole("status")).toHaveText(
      text("settings.diagnosticsDone", { name: "Clade-Diagnostics-20261004-120000.zip" }),
    );
    expect(await backend.calls("export_diagnostics")).toHaveLength(1);
    await expect(
      dialog.getByRole("button", { name: text("settings.exportDiagnostics") }),
    ).toBeEnabled();
  });

  test("a failed export says so, and can be tried again", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await backend.failNext("export_diagnostics", "diagnostics_failed");
    await dialog.getByRole("button", { name: text("settings.exportDiagnostics") }).click();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.settings.diagnostics_failed"));
    await dialog.getByRole("button", { name: text("settings.exportDiagnostics") }).click();
    await expect(dialog.getByRole("status")).toContainText("Clade-Diagnostics-");
    await expect(dialog.getByRole("alert")).toHaveCount(0);
  });
});

test.describe("reset settings", () => {
  test("asks first, and Keep settings changes nothing", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await dialog.getByRole("button", { name: text("settings.reset") }).click();
    await expect(dialog.getByText(text("settings.resetConfirm"))).toBeVisible();
    // while asking, the dialog cannot be dismissed by accident
    await expect(dialog.getByRole("button", { name: text("common.done") })).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await page.mouse.click(8, 8);
    await expect(dialog).toBeVisible();

    await dialog.getByRole("button", { name: text("settings.resetKeep") }).click();
    await expect(dialog.getByText(text("settings.resetConfirm"))).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: text("common.done") })).toBeEnabled();
    expect(await backend.calls("reset_settings")).toHaveLength(0);
  });

  test("Reset puts the preferences back, and touches no profile, account or playtime", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    // change a few things first
    await dialog.getByRole("switch", { name: text("settings.sound") }).click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.motion") })
      .getByRole("radio", { name: text("settings.motion.on") })
      .click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.memory") })
      .getByRole("radio", { name: text("settings.memory.custom") })
      .click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.onLaunch") })
      .getByRole("radio", { name: text("settings.onLaunch.hide") })
      .click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("hide");

    await dialog.getByRole("button", { name: text("settings.reset") }).click();
    await dialog.getByRole("button", { name: text("settings.resetDo") }).click();
    await expect(dialog.getByText(text("settings.resetConfirm"))).toHaveCount(0);

    await expect(dialog.getByRole("switch", { name: text("settings.sound") })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: text("settings.motion.system") })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: text("settings.memory.auto") })).toBeChecked();
    await expect(dialog.getByRole("radio", { name: text("settings.onLaunch.keep") })).toBeChecked();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "system");
    const ui = (await backend.state()).ui;
    expect([ui.soundEnabled, ui.reducedMotion, ui.memoryMb, ui.onLaunch]).toEqual([
      true,
      "system",
      null,
      "keep_open",
    ]);
    // the introduction stays seen, and the change touched nothing but the settings
    expect(ui.onboardingDone).toBe(true);
    expect(await backend.calls("reset_settings")).toHaveLength(1);
    const touched = (await backend.calls())
      .map((c) => c.cmd)
      .filter((c) =>
        /^(create_|select_|remove_|prepare_|launch_profile|begin_sign_in|unlock_vault)/.test(c),
      );
    expect(touched).toEqual([]);
    const state = await backend.state();
    expect(state.profiles).toHaveLength(3);
    expect(state.accounts).toHaveLength(2);
  });
});

test.describe("footer", () => {
  test("shows the product, the year and the version the backend reports", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByText("Clade", { exact: true })).toBeVisible();
    await expect(dialog.getByText("© 2026", { exact: true })).toBeVisible();
    await expect(dialog.getByText("v0.1.0", { exact: true })).toBeVisible();
  });
});

test.describe("footer with another version", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, version: "9.8.7" } } });

  test("takes the version from the backend, not from the page", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByText("v9.8.7", { exact: true })).toBeVisible();
    await expect(dialog.getByText("v0.1.0", { exact: true })).toHaveCount(0);
  });
});

test.describe("settings persistence", () => {
  test("every change goes to the backend and none to browser storage", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    await dialog.getByRole("switch", { name: text("settings.sound") }).click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.motion") })
      .getByRole("radio", { name: text("settings.motion.off") })
      .click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.onExit") })
      .getByRole("radio", { name: text("settings.onExit.hidden") })
      .click();
    await dialog
      .getByRole("radiogroup", { name: text("settings.memory") })
      .getByRole("radio", { name: text("settings.memory.custom") })
      .click();
    await expect.poll(async () => (await backend.state()).ui.memoryMb).toBe(4096);
    const commands = (await backend.calls()).map((c) => c.cmd);
    for (const sent of ["set_sound_settings", "set_reduced_motion", "set_behavior", "set_memory"]) {
      expect(commands, sent).toContain(sent);
    }
    const stored = await page.evaluate(async () => ({
      local: localStorage.length,
      session: sessionStorage.length,
      databases: (await indexedDB.databases()).length,
      cookies: document.cookie,
    }));
    expect(stored).toEqual({ local: 0, session: 0, databases: 0, cookies: "" });
  });
});
