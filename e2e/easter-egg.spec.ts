import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { settingsButton, text } from "./support/ui";

const openSettings = async (page: Page) => {
  await settingsButton(page).click();
  const dialog = page.getByRole("dialog", { name: text("settings.title") });
  await expect(dialog).toBeVisible();
  // let the dialog finish its own opening motion before anything is measured
  await expect.poll(() => dialog.evaluate((d) => d.getAnimations().length)).toBe(0);
};
const cat = (page: Page) => page.getByRole("button", { name: text("settings.cat") });

/** Names of the properties the cat's parts are animating right now (empty when it is at rest). */
const animatedProperties = (page: Page) =>
  page.evaluate(() => {
    const button = document.querySelector('[aria-label="Cat"]');
    if (!button) return [];
    return button
      .getAnimations({ subtree: true })
      .filter((a) => a instanceof CSSAnimation)
      .flatMap((a) => (a.effect as KeyframeEffect).getKeyframes().flatMap((k) => Object.keys(k)))
      .filter((k) => !["offset", "easing", "composite", "computedOffset"].includes(k));
  });
const position = (page: Page) =>
  cat(page).evaluate((el) => {
    const b = el.getBoundingClientRect();
    return [Math.round(b.left), Math.round(b.top)];
  });

test.describe("the cat in Settings", () => {
  test("reacts to a click in place and stays there", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    const before = await position(page);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    // ears, eyes, tail and body move; the button itself never does
    const moving = await animatedProperties(page);
    expect(moving).toContain("transform");
    expect(await position(page)).toEqual(before);
    // it settles back by itself and is still there
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await expect(cat(page)).toBeVisible();
    expect(await position(page)).toEqual(before);
    expect(await animatedProperties(page)).toEqual([]);
  });

  test("works from the keyboard (Enter and Space) and keeps focus", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    await cat(page).focus();
    await page.keyboard.press("Enter");
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await expect(cat(page)).toBeFocused();
    await page.keyboard.press("Space");
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    await expect(cat(page)).toBeVisible();
  });

  test("can be triggered again, also while it is still reacting", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    // a click in the middle of a reaction restarts it instead of being ignored
    const first = await animatedProperties(page);
    await cat(page).click();
    expect((await animatedProperties(page)).length).toBe(first.length);
    await expect(cat(page)).toBeVisible();
  });

  test("never leaves: it is there every time Settings opens", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    await cat(page).click();
    await page.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await openSettings(page);
    await expect(cat(page)).toBeVisible();
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
  });

  test("is never needed to use Settings", async ({ page, backend, openApp }) => {
    await openApp();
    await openSettings(page);
    await page.getByRole("switch", { name: text("settings.sound") }).click();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect((await backend.state()).ui.soundEnabled).toBe(false);
  });

  test("saves nothing and uses no browser storage", async ({ page, backend, openApp }) => {
    await openApp();
    await openSettings(page);
    const before = (await backend.calls()).length;
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    expect((await backend.calls()).length).toBe(before);
    const stored = await page.evaluate(() => localStorage.length + sessionStorage.length);
    expect(stored).toBe(0);
  });
});

test.describe("the cat with reduced motion", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" } } });

  test("only changes its expression: no transform animation at all", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    const before = await position(page);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    expect(await animatedProperties(page)).not.toContain("transform");
    expect(await animatedProperties(page)).toEqual([]);
    // the eyes are the only thing that changes
    await expect(cat(page).locator(".cat-eyes")).toHaveCSS("opacity", "0.2");
    expect(await position(page)).toEqual(before);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await expect(cat(page).locator(".cat-eyes")).toHaveCSS("opacity", "1");
    await expect(cat(page)).toBeVisible();
  });

  test("can be triggered again", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    await cat(page).click();
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
  });
});

test.describe("the cat when Windows asks for reduced motion", () => {
  test("follows the system preference too", async ({ page, openApp }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openApp();
    await openSettings(page);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    expect(await animatedProperties(page)).toEqual([]);
  });
});
