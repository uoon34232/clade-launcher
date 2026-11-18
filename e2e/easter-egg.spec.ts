import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { settingsButton, text } from "./support/ui";

const openSettings = async (page: Page) => {
  await settingsButton(page).click();
  const dialog = page.getByRole("dialog", { name: text("settings.title") });
  await expect(dialog).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
  // Let the dialog finish its opening scale/fade motion (and reach scale 1) before measuring.
  await expect
    .poll(() =>
      dialog.evaluate(
        (d) =>
          !d.hasAttribute("data-starting-style") &&
          d.getAnimations({ subtree: true }).length === 0 &&
          Math.abs(d.getBoundingClientRect().width - (d as HTMLElement).offsetWidth) < 0.5,
      ),
    )
    .toBe(true);
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
/** Where the cat sits inside Settings. Relative to the dialog, so the dialog re-centring itself
 *  (late font loading, a slow machine) is not mistaken for the cat moving. */
const position = (page: Page) =>
  cat(page).evaluate((el) => {
    const b = el.getBoundingClientRect();
    const d = el.closest('[role="dialog"]')?.getBoundingClientRect();
    return [b.left - (d?.left ?? 0), b.top - (d?.top ?? 0)] as const;
  });

const expectSameSpot = (actual: readonly [number, number], before: readonly [number, number]) => {
  expect(Math.abs(actual[0] - before[0])).toBeLessThanOrEqual(1);
  expect(Math.abs(actual[1] - before[1])).toBeLessThanOrEqual(1);
};

/**
 * Watches the drawn cat (the button and the union of its strokes) frame by frame for `ms` and
 * returns the largest distance, in pixels, that any edge strayed from where it started. Everything
 * is measured relative to the Settings dialog, so a re-centring dialog is not mistaken for the cat
 * moving, and nothing depends on when a frame is sampled: a cat that really moves is seen in any
 * frame, whatever the timing.
 */
const maxDisplacement = (page: Page, ms: number) =>
  page.evaluate(
    (duration) =>
      new Promise<number>((resolve) => {
        const button = document.querySelector('[aria-label="Cat"]');
        const dialog = button?.closest('[role="dialog"]');
        if (!button || !dialog) return resolve(Number.POSITIVE_INFINITY);
        const edges = () => {
          const d = dialog.getBoundingClientRect();
          const b = button.getBoundingClientRect();
          const parts = [...button.querySelectorAll("path")].map((p) => p.getBoundingClientRect());
          const left = Math.min(...parts.map((r) => r.left));
          const right = Math.max(...parts.map((r) => r.right));
          const bottom = Math.max(...parts.map((r) => r.bottom));
          return [b.left - d.left, b.top - d.top, left - d.left, right - d.left, bottom - d.top];
        };
        const start = edges();
        let worst = 0;
        const t0 = performance.now();
        const frame = () => {
          const now = edges();
          worst = Math.max(worst, ...now.map((v, i) => Math.abs(v - start[i])));
          if (performance.now() - t0 < duration) requestAnimationFrame(frame);
          else resolve(worst);
        };
        frame();
      }),
    ms,
  );

test.describe("the cat in Settings", () => {
  test("reacts to a click in place and stays there", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    const before = await position(page);
    // watch every frame of the whole reaction while it is triggered
    const watching = maxDisplacement(page, 900);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    // ears, eyes, tail and body move; the button itself never does
    const moving = await animatedProperties(page);
    expect(moving).toContain("transform");
    expectSameSpot(await position(page), before);
    // the small movements of ears, tail and body stay within a few pixels: it never runs anywhere
    expect(await watching).toBeLessThanOrEqual(4);
    // it settles back by itself and is still there
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await expect(cat(page)).toBeVisible();
    expectSameSpot(await position(page), before);
    expect(await animatedProperties(page)).toEqual([]);
  });

  test("the displacement check itself notices a cat that runs away", async ({ page, openApp }) => {
    await openApp();
    await openSettings(page);
    const watching = maxDisplacement(page, 600);
    // simulate the old behaviour: the whole drawing slides 60 px to the right
    await page.evaluate(() =>
      document
        .querySelector('[aria-label="Cat"] svg')
        ?.animate([{ transform: "translateX(0)" }, { transform: "translateX(60px)" }], {
          duration: 300,
          fill: "forwards",
        }),
    );
    expect(await watching).toBeGreaterThan(20);
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
    expectSameSpot(await position(page), before);
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
