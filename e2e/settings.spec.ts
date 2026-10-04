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
    await expect(volume).toHaveValue("50");
    const language = dialog.getByRole("radiogroup", { name: text("settings.language") });
    await expect(language.getByRole("radio", { name: "English" })).toBeChecked();
    await expect(language.getByRole("radio", { name: "Русский" })).not.toBeChecked();
    await expect(dialog.getByRole("button", { name: text("common.done") })).toBeVisible();
  });

  test("turning sound effects off and on is saved through the backend", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await settingsButton(page).click();
    const sound = page.getByRole("switch", { name: text("settings.sound") });

    await sound.click();
    await expect(sound).not.toBeChecked();
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(false);

    await sound.click();
    await expect(sound).toBeChecked();
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

    await page.keyboard.press("End");
    await expect(volume).toHaveValue("100");
    await expect(volume).toHaveAttribute("aria-valuetext", "100%");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(1);

    await page.keyboard.press("Home");
    await expect(volume).toHaveValue("0");
    await expect.poll(async () => (await backend.state()).ui.soundVolume).toBe(0);
  });

  test("closes with Done and with Escape, and returns focus to the Settings button", async ({
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
  });

  test("changes no profile, account or playtime", async ({ page, backend, openApp }) => {
    await openApp();
    await settingsButton(page).click();
    await page.getByRole("switch", { name: text("settings.sound") }).click();
    await page.keyboard.press("Escape");
    const profileCommands = (await backend.calls()).filter((c) =>
      /profile|account|launch|prepare/.test(c.cmd),
    );
    // only the initial reads, never a change
    expect(
      profileCommands.map((c) => c.cmd).filter((c) => !/^(list_|launch_status)/.test(c)),
    ).toEqual([]);
  });
});
