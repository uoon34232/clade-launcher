import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG, SURVIVAL } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, settingsButton, text, textRu } from "./support/ui";

const openSettings = async (page: Page) => {
  await settingsButton(page).click();
  const dialog = page.getByRole("dialog", { name: text("settings.title") });
  await expect(dialog).toBeVisible();
  return dialog;
};

test.describe("simple and advanced mode", () => {
  test("starts in Simple mode, switches live to Advanced and back, and fits 960x600 in English and Russian", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    const dialog = await openSettings(page);

    const modeGroup = dialog.getByRole("radiogroup", { name: text("settings.mode") });
    await expect(
      modeGroup.getByRole("radio", { name: text("settings.mode.simple") }),
    ).toBeChecked();
    expect(await dialog.getByRole("heading", { level: 3 }).allTextContents()).toEqual([
      text("settings.interface"),
      text("settings.support"),
    ]);
    await expect(dialog.getByRole("radiogroup", { name: text("settings.motion") })).toHaveCount(0);
    await expect(
      dialog.getByRole("button", { name: text("settings.exportDiagnostics") }),
    ).toHaveCount(0);
    expect(
      await dialog.evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      })),
    ).toEqual({ vertical: true, horizontal: true });

    // Switch live to Advanced
    await modeGroup.getByRole("radio", { name: text("settings.mode.advanced") }).click();
    await expect(
      modeGroup.getByRole("radio", { name: text("settings.mode.advanced") }),
    ).toBeChecked();
    await expect.poll(async () => (await backend.state()).ui.uiMode).toBe("advanced");
    expect((await backend.calls("set_ui_mode")).map((c) => c.args)).toEqual([{ mode: "advanced" }]);
    expect(await dialog.getByRole("heading", { level: 3 }).allTextContents()).toEqual([
      text("settings.interface"),
      text("settings.minecraft"),
      text("settings.behavior"),
      text("settings.support"),
    ]);
    await expect(dialog.getByRole("radiogroup", { name: text("settings.motion") })).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: text("settings.exportDiagnostics") }),
    ).toBeVisible();
    expect(
      await dialog.evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      })),
    ).toEqual({ vertical: true, horizontal: true });

    // Russian in Advanced mode also fits 960x600
    await dialog.getByRole("radio", { name: "Русский" }).click();
    const ruDialog = page.getByRole("dialog", { name: textRu("settings.title") });
    await expect(ruDialog.getByRole("radiogroup", { name: textRu("settings.mode") })).toBeVisible();
    expect(
      await ruDialog.evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      })),
    ).toEqual({ vertical: true, horizontal: true });

    // Switch back to Simple in Russian
    await ruDialog
      .getByRole("radiogroup", { name: textRu("settings.mode") })
      .getByRole("radio", { name: textRu("settings.mode.simple") })
      .click();
    await expect.poll(async () => (await backend.state()).ui.uiMode).toBe("simple");
    expect(await ruDialog.getByRole("heading", { level: 3 }).allTextContents()).toEqual([
      textRu("settings.interface"),
      textRu("settings.support"),
    ]);
  });
});

test.describe("settings sections", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
    await expect(dialog.getByRole("radiogroup", { name: textRu("settings.mode") })).toBeVisible();
    await expect(dialog.getByRole("radiogroup", { name: textRu("settings.motion") })).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: textRu("settings.exportDiagnostics") }),
    ).toBeVisible();
    await expect(dialog.getByRole("button", { name: textRu("settings.reset") })).toBeVisible();
  });
});

test.describe("reduced motion", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced", reducedMotion: "on" } },
  });

  test("is applied when the launcher starts", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("data-motion", "reduce");
    const dialog = await openSettings(page);
    await expect(
      dialog
        .getByRole("radiogroup", { name: text("settings.motion") })
        .getByRole("radio", { name: text("settings.motion.on") }),
    ).toBeChecked();
  });
});

test.describe("memory", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
  test.use({
    backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced", memoryMb: 6144 } },
  });

  test("opens on Custom with the saved amount", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByRole("radio", { name: text("settings.memory.custom") })).toBeChecked();
    await expect(
      dialog.getByRole("slider", { name: text("settings.memory.slider") }),
    ).toHaveAttribute("aria-valuetext", "6 GB");
  });
});

test.describe("memory saved above what a smaller computer allows", () => {
  test.use({
    backendConfig: {
      ui: {
        ...DEFAULT_CONFIG.ui,
        uiMode: "advanced",
        memoryMb: 16384,
        memory: { min: 1024, max: 6144, auto: 4096, total: 8192 },
      },
    },
  });

  test("clamps the displayed slider value and label to this computer's safe range", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const slider = dialog.getByRole("slider", { name: text("settings.memory.slider") });
    await expect(slider).toHaveAttribute("aria-valuetext", "6 GB");
    await expect(dialog.getByText("6 GB", { exact: true })).toBeVisible();
  });
});

test.describe("behavior around the game", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

  test("opens Minecraft inside Clade and shows Clade again on exit by default, and saves each choice", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openSettings(page);
    const onLaunch = dialog.getByRole("radiogroup", { name: text("settings.onLaunch") });
    const onExit = dialog.getByRole("radiogroup", { name: text("settings.onExit") });
    await expect(
      onLaunch.getByRole("radio", { name: text("settings.onLaunch.embed") }),
    ).toBeChecked();
    await expect(onExit.getByRole("radio", { name: text("settings.onExit.show") })).toBeChecked();
    await expect(dialog.getByText(text("settings.onExit.hiddenNote"))).toHaveCount(0);

    await onLaunch.getByRole("radio", { name: text("settings.onLaunch.keep") }).click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("keep_open");
    await onExit.getByRole("radio", { name: text("settings.onExit.hidden") }).click();
    await expect.poll(async () => (await backend.state()).ui.onExit).toBe("stay_hidden");
    // being honest about what staying hidden means
    await expect(dialog.getByText(text("settings.onExit.hiddenNote"))).toBeVisible();

    await onLaunch.getByRole("radio", { name: text("settings.onLaunch.hide") }).click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("hide");
    await onLaunch.getByRole("radio", { name: text("settings.onLaunch.embed") }).click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("embed");
    expect((await backend.calls("set_behavior")).map((c) => c.args)).toEqual([
      { onLaunch: "keep_open" },
      { onExit: "stay_hidden" },
      { onLaunch: "hide" },
      { onLaunch: "embed" },
    ]);
  });
});

test.describe("support actions", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

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
    await expect(dialog.getByRole("button", { name: text("settings.reset") })).toBeFocused();
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
      .getByRole("radio", { name: text("settings.onLaunch.keep") })
      .click();
    await expect.poll(async () => (await backend.state()).ui.onLaunch).toBe("keep_open");

    await dialog.getByRole("button", { name: text("settings.reset") }).click();
    await dialog.getByRole("button", { name: text("settings.resetDo") }).click();
    await expect(dialog.getByText(text("settings.resetConfirm"))).toHaveCount(0);

    await expect(
      dialog
        .getByRole("radiogroup", { name: text("settings.mode") })
        .getByRole("radio", { name: text("settings.mode.simple") }),
    ).toBeChecked();
    await expect(dialog.getByRole("switch", { name: text("settings.sound") })).toBeChecked();
    await expect(dialog.getByRole("heading", { name: text("settings.minecraft") })).toHaveCount(0);
    await expect(dialog.getByRole("heading", { name: text("settings.behavior") })).toHaveCount(0);
    await expect(page.locator("html")).toHaveAttribute("data-motion", "system");
    await expect(page.locator("html")).toHaveAttribute("data-glass", "on");
    const ui = (await backend.state()).ui;
    expect([
      ui.uiMode,
      ui.soundEnabled,
      ui.reducedMotion,
      ui.transparency,
      ui.memoryMb,
      ui.onLaunch,
    ]).toEqual(["simple", true, "system", "system", null, "embed"]);
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
    await dialog
      .getByRole("radiogroup", { name: text("settings.mode") })
      .getByRole("radio", { name: text("settings.mode.advanced") })
      .click();
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
    for (const sent of [
      "set_ui_mode",
      "set_sound_settings",
      "set_reduced_motion",
      "set_behavior",
      "set_memory",
    ]) {
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

test.describe("transparency effects", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" } } });

  const html = (page: Page) => page.locator("html");
  const group = (dialog: Awaited<ReturnType<typeof openSettings>>) =>
    dialog.getByRole("radiogroup", { name: text("settings.transparency") });

  test("offers Follow Windows, On and Off, follows Windows by default and applies each choice live", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    // Windows has transparency on in the synthetic machine, so "Follow Windows" means glass
    await expect(html(page)).toHaveAttribute("data-glass", "on");
    await expect(html(page)).toHaveAttribute("data-glass-live", "true");
    const dialog = await openSettings(page);
    const g = group(dialog);
    await expect(
      g.getByRole("radio", { name: text("settings.transparency.system") }),
    ).toBeChecked();

    await g.getByRole("radio", { name: text("settings.transparency.off") }).click();
    await expect(g.getByRole("radio", { name: text("settings.transparency.off") })).toBeChecked();
    // Off is a true opaque fallback: nothing glass-related is left on the page
    await expect(html(page)).toHaveAttribute("data-glass", "off");
    await expect(html(page)).not.toHaveAttribute("data-glass-live", /.*/);
    const off = await page.evaluate(() => {
      const dock = getComputedStyle(document.querySelector("nav")!);
      return {
        dockFilter: dock.backdropFilter,
        pageBase: getComputedStyle(document.querySelector(".page-base")!).backgroundColor,
      };
    });
    expect(off.dockFilter).toBe("none");
    expect(off.pageBase).toBe("rgb(243, 237, 227)"); // fully opaque, the classic look
    // (the open card eases between materials for a moment, so wait for it to settle)
    await expect
      .poll(() =>
        page.evaluate(() => {
          const m = getComputedStyle(document.querySelector('[data-slot="dialog-content"]')!);
          return [m.backdropFilter, m.backgroundColor];
        }),
      )
      .toEqual(["none", "rgb(246, 240, 230)"]);

    await g.getByRole("radio", { name: text("settings.transparency.on") }).click();
    await expect(html(page)).toHaveAttribute("data-glass", "on");
    const on = await page.evaluate(() => ({
      dockFilter: getComputedStyle(document.querySelector("nav")!).backdropFilter,
      pageBase: getComputedStyle(document.querySelector(".page-base")!).backgroundColor,
    }));
    expect(on.dockFilter).toBe("none"); // the blur is the window material's, never a CSS filter (it costs GPU)
    expect(on.pageBase).toMatch(/^rgba\(243, 237, 227, 0\.\d+\)$/);

    await g.getByRole("radio", { name: text("settings.transparency.system") }).click();
    await expect(html(page)).toHaveAttribute("data-glass", "on");
    expect((await backend.calls("set_transparency")).map((c) => c.args)).toEqual([
      { mode: "off" },
      { mode: "on" },
      { mode: "system" },
    ]);
    expect((await backend.state()).ui.transparency).toBe("system");
  });

  test("is named in Russian too", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await dialog.getByRole("radio", { name: "Русский" }).click();
    const ru = page.getByRole("dialog");
    await expect(
      ru.getByRole("radiogroup", { name: textRu("settings.transparency") }),
    ).toBeVisible();
    for (const k of ["system", "on", "off"] as const)
      await expect(
        ru.getByRole("radio", { name: textRu(`settings.transparency.${k}`) }).last(),
      ).toBeVisible();
  });

  test("does not depend on reduced motion in either direction", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    const motion = dialog.getByRole("radiogroup", { name: text("settings.motion") });
    await motion.getByRole("radio", { name: text("settings.motion.on") }).click();
    await expect(html(page)).toHaveAttribute("data-motion", "reduce");
    await expect(html(page)).toHaveAttribute("data-glass", "on"); // reduced motion keeps the material
    await group(dialog)
      .getByRole("radio", { name: text("settings.transparency.off") })
      .click();
    await expect(html(page)).toHaveAttribute("data-glass", "off");
    await expect(html(page)).toHaveAttribute("data-motion", "reduce"); // and Off keeps the motion choice
  });

  test("Reset settings puts it back to Follow Windows", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await group(dialog)
      .getByRole("radio", { name: text("settings.transparency.off") })
      .click();
    await expect(html(page)).toHaveAttribute("data-glass", "off");
    await dialog.getByRole("button", { name: text("settings.reset") }).click();
    await page.getByRole("button", { name: text("settings.resetDo") }).click();
    await expect(html(page)).toHaveAttribute("data-glass", "on");
    expect((await backend.state()).ui.transparency).toBe("system");
  });
});

test.describe("transparency effects when Windows has them off", () => {
  test.use({
    backendConfig: {
      ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" },
      windows: { transparencyEffects: false, highContrast: false },
    },
  });

  test("Follow Windows is opaque, and On asks for glass but says Windows is not drawing it", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("data-glass", "off");
    const dialog = await openSettings(page);
    await dialog
      .getByRole("radiogroup", { name: text("settings.transparency") })
      .getByRole("radio", { name: text("settings.transparency.on") })
      .click();
    await expect(page.locator("html")).toHaveAttribute("data-glass", "on");
    await expect(page.locator("html")).toHaveAttribute("data-glass-live", "false");
  });
});

test.describe("transparency effects with Windows High Contrast", () => {
  test.use({
    backendConfig: {
      ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" },
      windows: { transparencyEffects: true, highContrast: true },
    },
  });

  test("stay opaque even when On is chosen", async ({ page, openApp }) => {
    await openApp();
    await expect(page.locator("html")).toHaveAttribute("data-glass", "off");
    const dialog = await openSettings(page);
    await dialog
      .getByRole("radiogroup", { name: text("settings.transparency") })
      .getByRole("radio", { name: text("settings.transparency.on") })
      .click();
    await expect(page.locator("html")).toHaveAttribute("data-glass", "off");
  });
});

test.describe("the account menu material", () => {
  const menuStyle = (page: Page) =>
    page.evaluate(() => {
      const m = getComputedStyle(document.querySelector('[data-slot="dropdown-menu-content"]')!);
      return { bg: m.backgroundColor, filter: m.backdropFilter };
    });

  test("is translucent smoked glass with glass on, with no CSS blur, and fully opaque graphite when off", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    const on = await menuStyle(page);
    expect(on.bg).toMatch(/^rgba\(43, 38, 34, 0\.\d+\)$/);
    expect(on.filter).toBe("none");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    // Off: the same menu is the classic opaque graphite
    await page.evaluate(() => {
      document.documentElement.dataset.glass = "off";
      delete document.documentElement.dataset.glassLive;
    });
    await accountTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect.poll(() => menuStyle(page)).toEqual({ bg: "rgb(47, 42, 38)", filter: "none" });
  });

  test("the profile menu uses the same material", async ({ page, openApp }) => {
    await openApp();
    const trigger = page.getByRole("navigation", { name: "Clade" }).getByRole("button").nth(1);
    await trigger.click();
    await expect(page.getByRole("menu")).toBeVisible();
    const on = await menuStyle(page);
    expect(on.bg).toMatch(/^rgba\(43, 38, 34, 0\.\d+\)$/);
    expect(on.filter).toBe("none");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await page.evaluate(() => {
      document.documentElement.dataset.glass = "off";
      delete document.documentElement.dataset.glassLive;
    });
    await trigger.click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect.poll(() => menuStyle(page)).toEqual({ bg: "rgb(47, 42, 38)", filter: "none" });
  });
});

test.describe("dock menus keep their rectangle", () => {
  const box = async (page: Page) => {
    const b = await page.locator('[data-slot="dropdown-menu-content"]').boundingBox();
    if (!b) throw new Error("the menu is not open");
    return b;
  };
  const near = (a: number, b: number) => Math.abs(a - b) <= 0.5;

  for (const [name, index] of [
    ["Account", 0],
    ["Profile", 1],
  ] as const) {
    test(`the ${name} menu does not move or resize while the pointer moves over it`, async ({
      page,
      openApp,
    }) => {
      await openApp();
      // the dock rises into place when the window opens; wait for it, or the menu would follow it
      await page.waitForTimeout(1800);
      const trigger = page
        .getByRole("navigation", { name: "Clade" })
        .getByRole("button")
        .nth(index);
      await trigger.hover();
      await trigger.click();
      await expect(page.getByRole("menu")).toBeVisible();
      // let the opening animation finish, then take the rectangle the menu opened with
      await page.waitForTimeout(500);
      const opened = await box(page);

      const items = page.locator('[data-slot="dropdown-menu-content"] [role^="menuitem"]');
      const targets = [items.first(), items.last(), items.first(), items.last()];
      for (const t of targets) {
        await t.hover();
        await page.waitForTimeout(250);
        const now = await box(page);
        for (const k of ["x", "y", "width", "height"] as const)
          expect(near(now[k], opened[k]), name + " " + k + ": " + opened[k] + " -> " + now[k]).toBe(
            true,
          );
      }
      // and over the empty padding of the menu itself
      await page.mouse.move(opened.x + 3, opened.y + 3);
      await page.waitForTimeout(250);
      const edge = await box(page);
      for (const k of ["x", "y", "width", "height"] as const)
        expect(near(edge[k], opened[k]), name + " " + k + " at the edge").toBe(true);
    });
  }
});

test.describe("the profile menu with many profiles", () => {
  const many = Array.from({ length: 8 }, (_, i) => ({
    ...SURVIVAL,
    id: "world-" + String(i),
    name: "World " + String(i),
  }));
  test.use({ backendConfig: { profiles: many, selected: "world-0" } });

  test("scrolls only the list, and keeps Create profile, Verify files and Remove fully visible", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    await page.waitForTimeout(1800); // the dock rises into place first
    await page.getByRole("navigation", { name: "Clade" }).getByRole("button").nth(1).click();
    const menu = page.locator('[data-slot="dropdown-menu-content"]');
    await expect(menu).toBeVisible();
    await page.waitForTimeout(400);
    const m = await menu.boundingBox();
    if (!m) throw new Error("no menu");
    expect(m.y).toBeGreaterThanOrEqual(0);
    expect(m.y + m.height).toBeLessThanOrEqual(600);

    // the list is what scrolls
    const list = menu.locator(".menu-scroll");
    expect(await list.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
    expect(await menu.evaluate((el) => el.scrollHeight <= el.clientHeight + 1)).toBe(true);

    // both actions are completely inside the menu, whatever the list is doing
    for (const name of [text("profile.create"), text("profile.verify"), text("profile.remove")]) {
      const item = menu.getByRole("menuitem", { name });
      await expect(item).toBeVisible();
      const b = await item.boundingBox();
      if (!b) throw new Error("no item");
      expect(b.y).toBeGreaterThanOrEqual(m.y - 0.5);
      expect(b.y + b.height).toBeLessThanOrEqual(m.y + m.height + 0.5);
    }
    // every profile can be reached by scrolling the list
    await list.evaluate((el) => (el.scrollTop = el.scrollHeight));
    const last = menu.getByRole("menuitemradio", { name: /World 7/ });
    const lb = await last.boundingBox();
    const lm = await list.boundingBox();
    if (!lb || !lm) throw new Error("no boxes");
    expect(lb.y + lb.height).toBeLessThanOrEqual(lm.y + lm.height + 0.5);
  });
});

test.describe("clearing archived vaults", () => {
  test("hides the button when no archived vaults exist", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openSettings(page);
    await expect(dialog.getByRole("button", { name: /Clear archived vaults/ })).toHaveCount(0);
  });

  test.describe("with archived vaults", () => {
    test.use({
      backendConfig: {
        ...DEFAULT_CONFIG,
        retiredVaults: 2,
        ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" },
      },
    });

    test("fits 960x600 in English and Russian (Simple and Advanced), asks first, handles failure and clears archives", async ({
      page,
      backend,
      openApp,
    }) => {
      await page.setViewportSize({ width: 960, height: 600 });
      await openApp();
      const dialog = await openSettings(page);

      const clearBtn = dialog.getByRole("button", { name: "Clear archived vaults (2)" });
      await expect(clearBtn).toBeVisible();
      expect(
        await dialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });

      // Confirmation locks dismissal, fits 960x600, and Keep archives returns focus.
      await clearBtn.click();
      await expect(dialog.getByText(/Delete 2 archived vaults\?/)).toBeVisible();
      await expect(dialog.getByRole("button", { name: text("common.done") })).toBeDisabled();
      expect(
        await dialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });
      await page.keyboard.press("Escape");
      await expect(dialog).toBeVisible();
      await page.mouse.click(8, 8);
      await expect(dialog).toBeVisible();

      await dialog.getByRole("button", { name: text("settings.clearRetiredVaultsKeep") }).click();
      await expect(clearBtn).toBeFocused();
      await expect(dialog.getByRole("button", { name: text("common.done") })).toBeEnabled();
      expect(await backend.calls("clear_retired_vaults")).toHaveLength(0);

      // Check Simple mode and Russian layout fit (including open confirmation) as well.
      await dialog
        .getByRole("radiogroup", { name: text("settings.mode") })
        .getByRole("radio", { name: text("settings.mode.simple") })
        .click();
      await expect(clearBtn).toBeVisible();
      await dialog
        .getByRole("radiogroup", { name: text("settings.mode") })
        .getByRole("radio", { name: text("settings.mode.advanced") })
        .click();
      await dialog.getByRole("radio", { name: "Русский" }).click();
      const ruDialog = page.getByRole("dialog", { name: textRu("settings.title") });
      const ruClearBtn = ruDialog.getByRole("button", { name: "Удалить архивные хранилища (2)" });
      await expect(ruClearBtn).toBeVisible();
      expect(
        await ruDialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });
      await ruClearBtn.click();
      expect(
        await ruDialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });
      await ruDialog
        .getByRole("button", { name: textRu("settings.clearRetiredVaultsKeep") })
        .click();

      // Switch back to English and verify failure -> retry -> permanent deletion.
      await ruDialog.getByRole("radio", { name: "English" }).click();
      await backend.failNext("clear_retired_vaults", "retired_vaults_clear_failed");
      await clearBtn.click();
      await dialog.getByRole("button", { name: text("settings.clearRetiredVaultsDo") }).click();
      await expect(dialog.getByRole("alert")).toHaveText(
        text("error.settings.retired_vaults_clear_failed"),
      );
      await expect(clearBtn).toBeVisible();

      await clearBtn.click();
      await dialog.getByRole("button", { name: text("settings.clearRetiredVaultsDo") }).click();
      await expect(dialog.getByRole("status")).toHaveText("Deleted 2 archived vaults.");
      await expect(dialog.getByRole("alert")).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: /Clear archived vaults/ })).toHaveCount(0);
      expect((await backend.state()).retiredVaults).toBe(0);
    });
  });
});

test.describe("manual update check", () => {
  test("never checks automatically, reports Up to date on click, and fits 960x600", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    const dialog = await openSettings(page);

    expect(await backend.calls("check_for_updates")).toHaveLength(0);
    await dialog.getByRole("button", { name: text("settings.checkUpdates") }).click();
    await expect(dialog.getByRole("status")).toHaveText(text("settings.updatesUpToDate"));
    await expect(
      dialog.getByRole("button", { name: text("settings.checkUpdatesAgain") }),
    ).toBeVisible();
    expect(await backend.calls("check_for_updates")).toHaveLength(1);
    expect(
      await dialog.evaluate((el) => ({
        vertical: el.scrollHeight <= el.clientHeight + 1,
        horizontal: el.scrollWidth <= el.clientWidth + 1,
      })),
    ).toEqual({ vertical: true, horizontal: true });
  });

  test.describe("when a newer release exists", () => {
    test.use({
      backendConfig: {
        ...DEFAULT_CONFIG,
        latestRelease: "v0.2.0",
        ui: { ...DEFAULT_CONFIG.ui, uiMode: "advanced" },
      },
    });

    test("handles offline errors, shows available version in EN and RU at 960x600, and opens the release", async ({
      page,
      backend,
      openApp,
    }) => {
      await page.setViewportSize({ width: 960, height: 600 });
      await openApp();
      const dialog = await openSettings(page);

      // Offline error fits 960x600 in EN and RU, then retry recovers cleanly.
      await backend.failNext("check_for_updates", "offline");
      await dialog.getByRole("button", { name: text("settings.checkUpdates") }).click();
      await expect(dialog.getByRole("alert")).toHaveText(text("error.settings.offline"));
      expect(
        await dialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });

      await dialog.getByRole("radio", { name: "Русский" }).click();
      const ruDialog = page.getByRole("dialog", { name: textRu("settings.title") });
      await expect(ruDialog.getByRole("alert")).toHaveText(textRu("error.settings.offline"));
      expect(
        await ruDialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });

      await ruDialog.getByRole("button", { name: textRu("settings.checkUpdatesAgain") }).click();
      await expect(ruDialog.getByRole("status")).toHaveText(
        textRu("settings.updateAvailable", { version: "v0.2.0" }),
      );
      await expect(
        ruDialog.getByRole("button", { name: textRu("settings.openRelease") }),
      ).toBeVisible();
      expect(
        await ruDialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });

      await ruDialog.getByRole("radio", { name: "English" }).click();
      await expect(dialog.getByRole("status")).toHaveText(
        text("settings.updateAvailable", { version: "v0.2.0" }),
      );
      expect(
        await dialog.evaluate((el) => ({
          vertical: el.scrollHeight <= el.clientHeight + 1,
          horizontal: el.scrollWidth <= el.clientWidth + 1,
        })),
      ).toEqual({ vertical: true, horizontal: true });

      await dialog.getByRole("button", { name: text("settings.openRelease") }).click();
      await expect
        .poll(async () => (await backend.calls("open_release_page")).map((c) => c.args))
        .toEqual([{ tag: "v0.2.0" }]);
    });
  });
});
