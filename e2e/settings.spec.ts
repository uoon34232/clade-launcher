import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { settingsButton, text } from "./support/ui";

test.describe("settings", () => {
  test("shows Sound effects, Volume and Language", async ({ page, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("settings.title") });
    await expect(dialog).toBeVisible();

    await expect(dialog.getByRole("switch", { name: text("settings.sound") })).toBeChecked();
    const volume = dialog.getByRole("slider", { name: text("settings.volume") });
    await expect(volume).toBeEnabled();
    await expect(volume).toHaveValue("50");
    await expect(volume).toHaveAttribute("aria-valuetext", "50%");
    const language = dialog.getByRole("radiogroup", { name: text("settings.language") });
    await expect(language.getByRole("radio", { name: "English" })).toBeChecked();
    await expect(language.getByRole("radio", { name: "Русский" })).not.toBeChecked();
    await expect(dialog.getByRole("button", { name: text("common.done") })).toBeVisible();
  });

  test("turning sound effects off and on disables volume and is saved through the backend", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await settingsButton(page).click();
    const sound = page.getByRole("switch", { name: text("settings.sound") });
    const volume = page.getByRole("slider", { name: text("settings.volume") });
    await expect(volume).toBeEnabled();

    await sound.click();
    await expect(sound).not.toBeChecked();
    await expect(volume).toBeDisabled();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(false);

    await sound.click();
    await expect(sound).toBeChecked();
    await expect(volume).toBeEnabled();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(true);
  });

  test("the volume slider responds to the keyboard and is saved", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await settingsButton(page).click();
    const volume = page.getByRole("slider", { name: text("settings.volume") });
    await volume.focus();

    await page.keyboard.press("ArrowRight");
    await expect(volume).toHaveValue("55");
    await expect(volume).toHaveAttribute("aria-valuetext", "55%");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(0.55);

    await page.keyboard.press("ArrowLeft");
    await expect(volume).toHaveValue("50");
    await expect(volume).toHaveAttribute("aria-valuetext", "50%");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(0.5);

    await page.keyboard.press("End");
    await expect(volume).toHaveValue("100");
    await expect(volume).toHaveAttribute("aria-valuetext", "100%");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(1);

    await page.keyboard.press("Home");
    await expect(volume).toHaveValue("0");
    await expect(volume).toHaveAttribute("aria-valuetext", "0%");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(0);
  });

  test("rolls back Language when saving fails", async ({ page, backend, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("settings.title") });
    const language = dialog.getByRole("radiogroup", { name: text("settings.language") });

    await backend.failNext("set_language", "storage");
    await language.getByRole("radio", { name: "Русский" }).click();

    await expect.poll(async () => (await backend.calls("set_language")).length).toBe(1);
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("common.done") })).toBeVisible();
    await expect(language.getByRole("radio", { name: "English" })).toBeChecked();
    await expect(language.getByRole("radio", { name: "Русский" })).not.toBeChecked();
    expect((await backend.state()).ui.language).toBe("en");
  });

  test("keeps sound toggling resilient when saving fails", async ({ page, backend, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    const sound = page.getByRole("switch", { name: text("settings.sound") });
    const volume = page.getByRole("slider", { name: text("settings.volume") });

    await backend.failNext("set_sound_settings", "storage");
    await sound.click();
    await expect(sound).not.toBeChecked();
    await expect(volume).toBeDisabled();
    await expect.poll(async () => (await backend.calls("set_sound_settings")).length).toBe(1);
    expect((await backend.state()).ui.soundEnabled).toBe(true);

    await sound.click();
    await expect(sound).toBeChecked();
    await expect(volume).toBeEnabled();
    await expect.poll(async () => (await backend.calls("set_sound_settings")).length).toBe(2);
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(true);
  });

  test("closes with Done, Escape and backdrop click, and returns focus to the Settings button", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const button = settingsButton(page);

    await button.click();
    await page.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(button).toBeFocused();

    await button.click();
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(button).toBeFocused();

    await button.click();
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();
    await page.mouse.click(8, 8);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(button).toBeFocused();
  });

  test("changes no profile, account or playtime", async ({ page, backend, openApp }) => {
    await openApp();
    const before = await backend.state();

    await settingsButton(page).click();
    await page.getByRole("switch", { name: text("settings.sound") }).click();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(false);

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);

    const calls = await backend.calls();
    const mutating = calls
      .map((c) => c.cmd)
      .filter(
        (cmd) =>
          !/^(get_|list_|launch_status$|retired_vaults_count$|vault_status$|set_last_phrase$|set_sound_settings$|set_game_stage_bounds$)/.test(
            cmd,
          ),
      );
    expect(mutating).toEqual([]);

    const after = await backend.state();
    expect(after.profiles).toEqual(before.profiles);
    expect(after.accounts).toEqual(before.accounts);
    expect(after.selected).toBe(before.selected);
    expect(after.active).toBe(before.active);
    expect(after.vault).toEqual(before.vault);
  });
});

test.describe("settings with non-default sound preferences", () => {
  test.use({
    backendConfig: {
      ui: { ...DEFAULT_CONFIG.ui, soundEnabled: false, soundVolume: 0.25 },
    },
  });

  test("hydrates initial sound state and disables Volume on open", async ({ page, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("settings.title") });
    await expect(dialog).toBeVisible();

    await expect(dialog.getByRole("switch", { name: text("settings.sound") })).not.toBeChecked();
    const volume = dialog.getByRole("slider", { name: text("settings.volume") });
    await expect(volume).toBeDisabled();
    await expect(volume).toHaveValue("25");
    await expect(volume).toHaveAttribute("aria-valuetext", "25%");
  });
});
