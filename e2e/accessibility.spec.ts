import type { Page } from "@playwright/test";
import { expect, test } from "./support/test";
import {
  accountTrigger,
  headline,
  playButton,
  profileTrigger,
  settingsButton,
  text,
} from "./support/ui";

// Focus is moved back into the dialog by the dialog primitive a moment after a Tab, so poll for it.
const focusIsInsideDialog = (page: Page) =>
  expect
    .poll(() => page.evaluate(() => document.activeElement?.closest('[role="dialog"]') !== null))
    .toBe(true);

test.describe("modal basics", () => {
  test("focus moves into the dialog, stays trapped while Tabbing, and returns on close", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const trigger = settingsButton(page);
    await trigger.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();
    await focusIsInsideDialog(page);

    // more Tabs than the dialog has controls: focus must wrap inside it, forwards and backwards
    for (let i = 0; i < 9; i++) {
      await page.keyboard.press("Tab");
      await focusIsInsideDialog(page);
    }
    for (let i = 0; i < 9; i++) {
      await page.keyboard.press("Shift+Tab");
      await focusIsInsideDialog(page);
    }

    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(trigger).toBeFocused();
  });

  test("Create profile puts focus in the Name field and traps it there too", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });
    await expect(dialog.getByLabel(text("create.name"))).toBeFocused();
    for (let i = 0; i < 14; i++) {
      await page.keyboard.press("Tab");
      await focusIsInsideDialog(page);
    }
  });
});

test.describe("keyboard operation", () => {
  test("the dock's controls are reachable in order and Play works from the keyboard", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await headline(page).getByRole("button").focus();

    const names: string[] = [];
    for (let i = 0; i < 4; i++) {
      await page.keyboard.press("Tab");
      names.push(
        (
          await page.evaluate(
            () =>
              document.activeElement?.getAttribute("aria-label") ??
              document.activeElement?.textContent ??
              "",
          )
        ).trim(),
      );
    }
    expect(names[0]).toMatch(/^Account: FixtureAlex/);
    expect(names[1]).toMatch(/^Profile: Survival/);
    expect(names[2]).toBe(text("play.play"));
    expect(names[3]).toBe(text("dock.settings"));

    await playButton(page).focus();
    await page.keyboard.press("Space");
    await expect(playButton(page)).toHaveText(text("play.launching"));
    expect(await backend.calls("launch_profile")).toHaveLength(1);
  });

  test("the focus ring appears for the keyboard and not after a mouse click", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await page.keyboard.press("Tab");
    await expect(page.locator("html")).toHaveAttribute("data-input", "keyboard");
    await headline(page).getByRole("button").click();
    await expect(page.locator("html")).toHaveAttribute("data-input", "pointer");
  });

  test("segmented choices follow the arrow keys", async ({ page, openApp }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    const dialog = page.getByRole("dialog", { name: text("create.title") });
    const vanilla = dialog.getByRole("radio", { name: text("profile.vanilla") });
    const modded = dialog.getByRole("radio", { name: text("profile.modded") });
    await vanilla.focus();
    await page.keyboard.press("ArrowRight");
    await expect(modded).toBeChecked();
    await expect(modded).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(vanilla).toBeChecked();
  });

  test("the account menu opens and closes from the keyboard", async ({ page, openApp }) => {
    await openApp();
    await accountTrigger(page).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();
  });
});
