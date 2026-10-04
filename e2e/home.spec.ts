import { ALEX, DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import {
  accountTrigger,
  dock,
  headline,
  headlineSettled,
  playButton,
  profileTrigger,
  settingsButton,
  text,
} from "./support/ui";

const PHRASES = (
  [
    "phrase.ready-when-you-are",
    "phrase.your-world-is-waiting",
    "phrase.welcome-back",
    "phrase.pick-up-where-you-left-off",
    "phrase.where-to-today",
    "phrase.one-more-world",
    "phrase.take-your-time",
    "phrase.choose-your-world",
  ] as const
).map((k) => text(k));

test.describe("home", () => {
  test("shows a headline, the subtitle, the total playtime and the dock", async ({
    page,
    openApp,
  }) => {
    await openApp();
    expect(PHRASES).toContain((await headline(page).textContent())?.trim());
    await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
    // 5400 s + 1200 s of fixture playtime
    await expect(page.getByText(text("hero.played", { time: "1h 50m" }))).toBeVisible();

    await expect(dock(page)).toBeVisible();
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: ALEX.username }),
    );
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Survival · Vanilla 1.21.4" }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
    await expect(settingsButton(page)).toBeVisible();
  });

  test("clicking the headline shows a different phrase, never the same twice in a row", async ({
    page,
    openApp,
  }) => {
    await openApp();
    await headlineSettled(page);
    let previous = ((await headline(page).textContent()) ?? "").trim();
    for (let i = 0; i < 6; i++) {
      await headline(page).getByRole("button").click();
      await expect(headline(page)).not.toHaveText(previous, { useInnerText: true });
      await headlineSettled(page);
      const now = ((await headline(page).textContent()) ?? "").trim();
      expect(PHRASES).toContain(now);
      expect(now).not.toBe(previous);
      previous = now;
    }
  });

  test("the headline can be changed from the keyboard", async ({ page, openApp }) => {
    await openApp();
    await headlineSettled(page);
    const before = ((await headline(page).textContent()) ?? "").trim();
    await headline(page).getByRole("button").focus();
    await page.keyboard.press("Enter");
    await expect(headline(page)).not.toHaveText(before, { useInnerText: true });
  });

  test("remembers the shown headline through the backend, not the browser", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect
      .poll(async () => (await backend.calls("set_last_phrase")).length)
      .toBeGreaterThan(0);
    const shown = ((await headline(page).textContent()) ?? "").trim();
    expect(PHRASES).toContain(shown);
    expect((await backend.state()).ui.lastPhrase).toBeTruthy();
    // Launcher state belongs to the backend: browser storage stays empty.
    const storage = await page.evaluate(() => ({
      local: localStorage.length,
      session: sessionStorage.length,
    }));
    expect(storage).toEqual({ local: 0, session: 0 });
  });
});

test.describe('home after a session that ended on "Welcome back."', () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, lastPhrase: "welcome-back" } } });

  test("a start never repeats the headline shown last time", async ({ page, openApp }) => {
    for (let i = 0; i < 5; i++) {
      await openApp();
      await expect(headline(page)).not.toHaveText(text("phrase.welcome-back"));
      await page.reload();
    }
  });
});

test.describe("home on a fresh install", () => {
  test.use({ backendConfig: { profiles: [], selected: null, accounts: [], active: null } });

  test("asks to create a profile and to add an account", async ({ page, openApp }) => {
    await openApp();
    await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    await expect(page.getByText(/^Played /)).toHaveCount(0);
  });

  test("the Play button starts creating the first profile", async ({ page, openApp }) => {
    await openApp();
    await playButton(page).click();
    await expect(page.getByRole("dialog", { name: text("create.title") })).toBeVisible();
  });
});

test.describe("layout", () => {
  for (const [width, height] of [
    [960, 600],
    [1280, 720],
    [1440, 900],
    [1920, 1080],
  ]) {
    test(`the home screen has no overflow at ${width}x${height}`, async ({ page, openApp }) => {
      await page.setViewportSize({ width, height });
      await openApp();
      const overflow = await page.evaluate(() => {
        const d = document.documentElement;
        return d.scrollWidth > innerWidth || d.scrollHeight > innerHeight;
      });
      expect(overflow).toBe(false);
      // The dock must stay inside the window.
      const box = await dock(page).boundingBox();
      expect(box).not.toBeNull();
      if (box) {
        expect(box.x).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width).toBeLessThanOrEqual(width);
        expect(box.y + box.height).toBeLessThanOrEqual(height);
      }
    });
  }

  test("the home screen has no overflow in Russian at the smallest window", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    await settingsButton(page).click();
    await page.getByRole("radio", { name: "Русский" }).click();
    await page.getByRole("button", { name: "Готово" }).click();
    await expect(dock(page)).toBeVisible();
    const overflow = await page.evaluate(() => {
      const d = document.documentElement;
      return d.scrollWidth > innerWidth || d.scrollHeight > innerHeight;
    });
    expect(overflow).toBe(false);
  });
});
