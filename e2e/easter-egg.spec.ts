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
const wordmark = (page: Page) => page.getByRole("button", { name: text("window.wordmark") });

const pauseClock = async (page: Page) => {
  await page.clock.pauseAt((await page.evaluate(() => Date.now())) + 5000);
};

/** Names of the properties the cat's parts are animating right now (empty when it is at rest). */
const animatedProperties = (page: Page) =>
  cat(page).evaluate((button) =>
    button
      .getAnimations({ subtree: true })
      .filter((a) => a instanceof CSSAnimation)
      .flatMap((a) => (a.effect as KeyframeEffect).getKeyframes().flatMap((k) => Object.keys(k)))
      .filter((k) => !["offset", "easing", "composite", "computedOffset"].includes(k)),
  );

const animationTimings = (page: Page) =>
  cat(page).evaluate((button) =>
    button
      .getAnimations({ subtree: true })
      .filter((a) => a instanceof CSSAnimation)
      .map((a) => ({
        startTime: Number(a.startTime ?? 0),
        currentTime: Number(a.currentTime ?? 0),
      })),
  );
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
  cat(page).evaluate(
    (button, duration) =>
      new Promise<number>((resolve) => {
        const dialog = button.closest('[role="dialog"]');
        if (!dialog) return resolve(Number.POSITIVE_INFINITY);
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
    await cat(page)
      .locator("svg")
      .evaluate((svg) =>
        svg.animate([{ transform: "translateX(0)" }, { transform: "translateX(60px)" }], {
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
    await page.clock.install();
    await openApp();
    await openSettings(page);
    await pauseClock(page);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    await page.clock.runFor(650);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    // Advance document.timeline past the first frame so restarted animations get a newer startTime.
    await expect
      .poll(
        async () => {
          const anims = await animationTimings(page);
          return anims.length > 0 && anims.every((a) => a.currentTime >= 20);
        },
        { intervals: [20] },
      )
      .toBe(true);
    const before = await animationTimings(page);
    expect(before.length).toBe(5);

    await page.clock.runFor(400);
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");

    // a click in the middle of a reaction restarts animations and extends the 650 ms timer
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    const after = await animationTimings(page);
    expect(after.length).toBe(before.length);
    const latestBeforeStart = Math.max(...before.map((a) => a.startTime));
    for (const anim of after) {
      expect(anim.startTime).toBeGreaterThan(latestBeforeStart);
    }

    // 400 ms after the second click (800 ms since the first), the reaction is still active
    await page.clock.runFor(400);
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    await page.clock.runFor(250);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
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
    await expect.poll(async () => (await backend.state()).ui.soundEnabled).toBe(false);
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
    await page.clock.install();
    await openApp();
    await openSettings(page);
    await pauseClock(page);

    const before = await position(page);
    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    expect(await animatedProperties(page)).toEqual([]);
    // the eyes are the only thing that changes
    await expect(cat(page).locator(".cat-eyes")).toHaveCSS("opacity", "0.2");
    expectSameSpot(await position(page), before);

    await page.clock.runFor(260);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
    await expect(cat(page).locator(".cat-eyes")).toHaveCSS("opacity", "1");
    await expect(cat(page)).toBeVisible();
  });

  test("can be triggered again, also while it is still reacting", async ({ page, openApp }) => {
    await page.clock.install();
    await openApp();
    await openSettings(page);
    await pauseClock(page);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    await page.clock.runFor(260);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    await page.clock.runFor(150);
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");

    // a click mid-reaction extends the 260 ms reduced-motion timer
    await cat(page).click();
    await page.clock.runFor(150);
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    await page.clock.runFor(110);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
  });
});

test.describe("the cat when Windows asks for reduced motion", () => {
  test("follows the system preference too", async ({ page, openApp }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install();
    await openApp();
    await openSettings(page);
    await pauseClock(page);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "reduced");
    expect(await animatedProperties(page)).toEqual([]);
  });
});

test.describe("the cat when reduced motion is explicitly off", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "off" } } });

  test("ignores the system reduced-motion preference", async ({ page, openApp }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install();
    await openApp();
    await openSettings(page);
    await pauseClock(page);

    await cat(page).click();
    await expect(cat(page)).toHaveAttribute("data-reaction", "full");
    expect(await animatedProperties(page)).toContain("transform");
    await page.clock.runFor(650);
    await expect(cat(page)).not.toHaveAttribute("data-reaction", /.+/);
  });
});

test.describe("the title wordmark", () => {
  test("reveals the tagline on click, extends on mid-reaction click, and settles back", async ({
    page,
    openApp,
  }) => {
    await page.clock.install();
    await openApp();
    await pauseClock(page);

    await expect(wordmark(page)).toBeVisible();
    await expect(wordmark(page)).toHaveAttribute("data-phase", "off");
    await expect(wordmark(page).locator('[aria-live="polite"]')).toHaveText("");

    await wordmark(page).click();
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    await expect(wordmark(page)).toHaveAttribute("data-mode", "full");
    await expect(wordmark(page).locator('[aria-live="polite"]')).toHaveText(
      text("onboarding.tagline"),
    );
    await expect(wordmark(page).locator(".wordmark-line")).toHaveCSS("opacity", "1");
    await expect(wordmark(page).locator(".wordmark-letter").first()).not.toHaveCSS(
      "transform",
      "none",
    );

    await page.clock.runFor(600);
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");

    // clicking mid-reaction extends the 1100 ms hold timer
    await wordmark(page).click();
    await page.clock.runFor(600);
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    await page.clock.runFor(500);
    await expect(wordmark(page)).toHaveAttribute("data-phase", "off");
    await expect(wordmark(page).locator('[aria-live="polite"]')).toHaveText("");
  });

  test("works from the keyboard (Enter and Space) and makes no backend calls", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.clock.install();
    await openApp();
    await pauseClock(page);

    const beforeCalls = (await backend.calls()).length;
    await wordmark(page).focus();
    await page.keyboard.press("Enter");
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    await expect(wordmark(page)).toBeFocused();

    await page.clock.runFor(1100);
    await expect(wordmark(page)).toHaveAttribute("data-phase", "off");

    await page.keyboard.press("Space");
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    expect((await backend.calls()).length).toBe(beforeCalls);
  });
});

test.describe("the title wordmark with reduced motion", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" } } });

  test("crossfades without spreading letters or drawing the line", async ({ page, openApp }) => {
    await page.clock.install();
    await openApp();
    await pauseClock(page);

    await wordmark(page).click();
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    await expect(wordmark(page)).toHaveAttribute("data-mode", "reduced");
    await expect(wordmark(page).locator('[aria-live="polite"]')).toHaveText(
      text("onboarding.tagline"),
    );
    await expect(wordmark(page).locator(".wordmark-line")).toHaveCSS("opacity", "0");
    await expect(wordmark(page).locator(".wordmark-letter").first()).toHaveCSS("transform", "none");

    await page.clock.runFor(1100);
    await expect(wordmark(page)).toHaveAttribute("data-phase", "off");
  });
});

test.describe("the title wordmark when reduced motion is explicitly off", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "off" } } });

  test("ignores the system reduced-motion preference", async ({ page, openApp }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install();
    await openApp();
    await pauseClock(page);

    await wordmark(page).click();
    await expect(wordmark(page)).toHaveAttribute("data-phase", "on");
    await expect(wordmark(page)).toHaveAttribute("data-mode", "full");
  });
});
