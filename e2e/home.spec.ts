import { ALEX, BLAKE, DEFAULT_CONFIG, SURVIVAL } from "./support/fixtures";
import { expect, test, type Page } from "./support/test";
import {
  accountTrigger,
  dock,
  headline,
  headlineSettled,
  playButton,
  profileTrigger,
  settingsButton,
  text,
  textRu,
} from "./support/ui";

const PHRASE_IDS = [
  "ready-when-you-are",
  "your-world-is-waiting",
  "welcome-back",
  "pick-up-where-you-left-off",
  "where-to-today",
  "one-more-world",
  "take-your-time",
  "choose-your-world",
] as const;

type PhraseId = (typeof PHRASE_IDS)[number];

const PHRASES = PHRASE_IDS.map((id) => text(`phrase.${id}`));

const PHRASE_BY_EN = new Map<string, PhraseId>(PHRASE_IDS.map((id) => [text(`phrase.${id}`), id]));

function phraseIdOf(rendered: string): PhraseId {
  const id = PHRASE_BY_EN.get(rendered);
  if (!id) throw new Error(`Unexpected headline text: ${rendered}`);
  return id;
}

const escapeRegex = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

const playedPattern = (locale: "en" | "ru" = "en") => {
  const template = (locale === "ru" ? textRu : text)("hero.played", { time: "__TIME__" });
  return new RegExp(`^${escapeRegex(template).replace("__TIME__", ".+")}$`);
};

async function waitForHomeSettled(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForFunction(() => {
    const targets = Array.from(document.querySelectorAll(".hero-stage, .launcher-dock-wrap"));
    return (
      targets.length === 2 &&
      targets.every((el) =>
        el.getAnimations({ subtree: true }).every((a) => a.playState === "finished"),
      )
    );
  });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
}

async function assertHomeLayout(page: Page, width: number, height: number): Promise<void> {
  await waitForHomeSettled(page);

  const overflow = await page.evaluate(() => {
    const d = document.documentElement;
    const main = document.querySelector("main");
    const stage = document.querySelector(".hero-stage");
    const heroContent = stage?.firstElementChild ?? null;
    const h1 = document.querySelector("h1");
    const dockWrap = document.querySelector(".launcher-dock-wrap");
    const dockNav = dockWrap?.querySelector('nav[aria-label="Clade"]') ?? null;

    const elements: [string, Element | null][] = [
      ["html", d],
      ["main", main],
      ["hero-stage", stage],
      ["hero-content", heroContent],
      ["h1", h1],
      ["dock-wrap", dockWrap],
      ["dock-nav", dockNav],
    ];
    if (dockNav) {
      Array.from(dockNav.children).forEach((child, index) => {
        elements.push([`dock-item-${index}`, child]);
      });
    }

    const issues: string[] = [];
    if (d.scrollWidth > window.innerWidth || d.scrollHeight > window.innerHeight) {
      issues.push(
        `viewport:${d.scrollWidth}x${d.scrollHeight}>${window.innerWidth}x${window.innerHeight}`,
      );
    }
    for (const [label, el] of elements) {
      if (!el) {
        issues.push(`missing:${label}`);
        continue;
      }
      if (el.scrollWidth > el.clientWidth) {
        issues.push(`horizontal:${label}:${el.scrollWidth}>${el.clientWidth}`);
      }
      if (
        (label === "html" ||
          label === "main" ||
          label === "hero-stage" ||
          label === "dock-wrap" ||
          label === "dock-nav") &&
        el.scrollHeight > el.clientHeight
      ) {
        issues.push(`vertical:${label}:${el.scrollHeight}>${el.clientHeight}`);
      }
    }
    return issues;
  });
  expect(overflow).toEqual([]);

  const heroBox = await page.locator(".hero-stage > *").first().boundingBox();
  const headlineBox = await headline(page).boundingBox();
  const dockBox = await dock(page).boundingBox();

  expect(heroBox).not.toBeNull();
  expect(headlineBox).not.toBeNull();
  expect(dockBox).not.toBeNull();

  if (heroBox && headlineBox && dockBox) {
    for (const [name, box] of [
      ["hero", heroBox],
      ["headline", headlineBox],
      ["dock", dockBox],
    ] as const) {
      expect(box.x, `${name} left`).toBeGreaterThanOrEqual(0);
      expect(box.y, `${name} top`).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width, `${name} right`).toBeLessThanOrEqual(width);
      expect(box.y + box.height, `${name} bottom`).toBeLessThanOrEqual(height);
    }
    // Keep Hero below the 44px window chrome strip and clear above the dock.
    expect(heroBox.y, "hero below window chrome").toBeGreaterThanOrEqual(44);
    expect(heroBox.y + heroBox.height, "hero must not overlap dock").toBeLessThanOrEqual(dockBox.y);
  }
}

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
      text("profile.trigger", { label: "Survival" }),
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
    const expectedId = phraseIdOf(shown);
    expect((await backend.state()).ui.lastPhrase).toBe(expectedId);
    expect((await backend.calls("set_last_phrase")).map((c) => c.args)).toEqual([
      { id: expectedId },
    ]);
    // Launcher state belongs to the backend: browser storage stays empty.
    const storage = await page.evaluate(() => ({
      local: localStorage.length,
      session: sessionStorage.length,
    }));
    expect(storage).toEqual({ local: 0, session: 0 });
  });

  test("debounces headline shuffles by 800 ms and saves the updated phrase ID", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect.poll(async () => (await backend.calls("set_last_phrase")).length).toBe(1);
    const initialText = ((await headline(page).textContent()) ?? "").trim();
    const initialId = phraseIdOf(initialText);
    await headlineSettled(page);

    await headline(page).getByRole("button").click();
    await expect(headline(page)).not.toHaveText(initialText, { useInnerText: true });
    await headlineSettled(page);

    const nextText = ((await headline(page).textContent()) ?? "").trim();
    const nextId = phraseIdOf(nextText);
    expect(nextId).not.toBe(initialId);

    await expect.poll(async () => (await backend.calls("set_last_phrase")).length).toBe(2);
    expect((await backend.calls("set_last_phrase")).map((c) => c.args)).toEqual([
      { id: initialId },
      { id: nextId },
    ]);
    expect((await backend.state()).ui.lastPhrase).toBe(nextId);
  });

  test("preserves the active headline phrase ID across a live language switch", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect.poll(async () => (await backend.calls("set_last_phrase")).length).toBe(1);
    const shownEn = ((await headline(page).textContent()) ?? "").trim();
    const phraseId = phraseIdOf(shownEn);

    await settingsButton(page).click();
    await page.getByRole("radio", { name: "Русский" }).click();
    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await expect(headline(page)).toHaveText(textRu(`phrase.${phraseId}`));
    expect((await backend.state()).ui.lastPhrase).toBe(phraseId);

    await settingsButton(page).click();
    await page.getByRole("radio", { name: "English" }).click();
    await page.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await expect(headline(page)).toHaveText(text(`phrase.${phraseId}`));
  });

  test("activates TitleWordmark on click and reduced-motion keyboard without storing state", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const wordmark = page.getByRole("button", { name: text("window.wordmark") });
    const live = wordmark.locator('[aria-live="polite"]');

    await expect(wordmark).toBeVisible();
    await expect(wordmark).toHaveAttribute("data-phase", "off");
    await expect(live).toHaveText("");

    const callsBefore = (await backend.calls()).length;

    await wordmark.click();
    await expect(wordmark).toHaveAttribute("data-phase", "on");
    await expect(wordmark).toHaveAttribute("data-mode", "full");
    await expect(live).toHaveText(text("onboarding.tagline"));
    await expect(wordmark).toHaveAttribute("data-phase", "off");
    await expect(live).toHaveText("");

    await page.emulateMedia({ reducedMotion: "reduce" });
    await wordmark.focus();
    await page.keyboard.press("Enter");
    await expect(wordmark).toHaveAttribute("data-phase", "on");
    await expect(wordmark).toHaveAttribute("data-mode", "reduced");
    await expect(live).toHaveText(text("onboarding.tagline"));
    await expect(wordmark).toHaveAttribute("data-phase", "off");

    expect((await backend.calls()).length).toBe(callsBefore);
    const storage = await page.evaluate(() => ({
      local: localStorage.length,
      session: sessionStorage.length,
    }));
    expect(storage).toEqual({ local: 0, session: 0 });
  });
});

test.describe("headline shuffle coalescing", () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" } } });

  test("coalesces rapid headline shuffles so intermediate phrases are not saved", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect.poll(async () => (await backend.calls("set_last_phrase")).length).toBe(1);
    const startText = ((await headline(page).textContent()) ?? "").trim();
    const startId = phraseIdOf(startText);
    await headlineSettled(page);

    const btn = headline(page).getByRole("button");
    await btn.click();
    await expect(headline(page)).not.toHaveText(startText, { useInnerText: true });
    await headlineSettled(page);
    const intermediateText = ((await headline(page).textContent()) ?? "").trim();
    const intermediateId = phraseIdOf(intermediateText);
    expect(intermediateId).not.toBe(startId);

    await btn.click();
    await expect(headline(page)).not.toHaveText(intermediateText, { useInnerText: true });
    await headlineSettled(page);
    let finalText = ((await headline(page).textContent()) ?? "").trim();

    // If the second shuffle randomly landed back on startText, Hero skips saving a no-op;
    // advance once more so the settled target differs from startId.
    if (finalText === startText) {
      await btn.click();
      await expect(headline(page)).not.toHaveText(startText, { useInnerText: true });
      await headlineSettled(page);
      finalText = ((await headline(page).textContent()) ?? "").trim();
    }

    const finalId = phraseIdOf(finalText);
    expect(finalId).not.toBe(startId);

    await expect.poll(async () => (await backend.calls("set_last_phrase")).length).toBe(2);
    expect((await backend.calls("set_last_phrase")).map((c) => c.args)).toEqual([
      { id: startId },
      { id: finalId },
    ]);
    expect((await backend.state()).ui.lastPhrase).toBe(finalId);
  });
});

test.describe('home after a session that ended on "Welcome back."', () => {
  test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, lastPhrase: "welcome-back" } } });

  test("a start never repeats the headline shown last time", async ({ page, backend, openApp }) => {
    for (let i = 0; i < 5; i++) {
      await openApp();
      await expect(headline(page)).not.toHaveText(text("phrase.welcome-back"));
      const shownId = phraseIdOf(((await headline(page).textContent()) ?? "").trim());
      expect(shownId).not.toBe("welcome-back");
      await expect.poll(async () => (await backend.state()).ui.lastPhrase).toBe(shownId);
    }
  });
});

test.describe("home on a fresh install", () => {
  test.use({ backendConfig: { profiles: [], selected: null, accounts: [], active: null } });

  test("asks to create a profile and to add an account", async ({ page, openApp }) => {
    await openApp();
    await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    await expect(page.getByText(playedPattern())).toHaveCount(0);
  });

  test("the Play button starts creating the first profile", async ({ page, openApp }) => {
    await openApp();
    await playButton(page).click();
    await expect(page.getByRole("dialog", { name: text("create.title") })).toBeVisible();
  });
});

test.describe("playtime formatting and live updates", () => {
  test.use({
    backendConfig: {
      profiles: [{ ...SURVIVAL, playtimeSeconds: 45 }],
      selected: SURVIVAL.id,
    },
  });

  test("formats sub-hour playtime (<1m) and refreshes to minutes after launch stops", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(
      page.getByText(text("hero.played", { time: text("playtime.lessThanMinute") })),
    ).toBeVisible();

    const listsBefore = (await backend.calls("list_profiles")).length;
    await playButton(page).click();
    await backend.playing(SURVIVAL.id);

    await page.evaluate(() => {
      const w = window as unknown as {
        __e2e: { state: { profiles: { id: string; playtimeSeconds: number }[] } };
      };
      const profile = w.__e2e.state.profiles.find((p) => p.id === "survival");
      if (profile) profile.playtimeSeconds = 1200;
    });
    await backend.stopped(SURVIVAL.id, { kind: "normal", reason: "closed", code: 0 });

    await expect
      .poll(async () => (await backend.calls("list_profiles")).length)
      .toBeGreaterThan(listsBefore);
    await expect(
      page.getByText(text("hero.played", { time: text("playtime.minutes", { minutes: 20 }) })),
    ).toBeVisible();
  });
});

test.describe("home error and fallback handling", () => {
  test("falls back to empty profiles if list_profiles rejects on startup", async ({
    page,
    backend,
  }) => {
    await page.addInitScript(() => {
      (
        window as unknown as { __e2e?: { failNext: (cmd: string, code: string) => void } }
      ).__e2e?.failNext("list_profiles", "storage");
    });
    await page.goto("/");

    await expect(headline(page)).toBeVisible();
    await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: ALEX.username }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(page.getByText(playedPattern())).toHaveCount(0);
    expect((await backend.calls("list_profiles")).length).toBeGreaterThanOrEqual(1);
  });

  test("falls back to no accounts and sign-in prompt if list_accounts rejects on startup", async ({
    page,
    backend,
  }) => {
    await page.addInitScript(() => {
      (
        window as unknown as { __e2e?: { failNext: (cmd: string, code: string) => void } }
      ).__e2e?.failNext("list_accounts", "storage");
    });
    await page.goto("/");

    await expect(headline(page)).toBeVisible();
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    await expect(accountTrigger(page)).toContainText(text("account.signIn"));
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: SURVIVAL.name }),
    );
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    expect((await backend.calls("list_accounts")).length).toBeGreaterThanOrEqual(1);
  });

  test.describe("get_ui_settings startup failures", () => {
    test.use({
      backendConfig: {
        ui: { ...DEFAULT_CONFIG.ui, language: "ru", onboardingDone: false },
      },
    });

    test("falls back to default settings if get_ui_settings rejects on startup", async ({
      page,
      backend,
      openApp,
    }) => {
      await page.addInitScript(() => {
        const e2e = (
          window as unknown as { __e2e?: { failNext: (cmd: string, code: string) => void } }
        ).__e2e;
        e2e?.failNext("get_ui_settings", "storage");
        e2e?.failNext("set_last_phrase", "storage");
      });
      await openApp();

      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
      await expect(playButton(page)).toHaveText(text("play.play"));
      expect(await backend.calls("get_ui_settings")).toHaveLength(1);
    });

    test("falls back to default settings if get_ui_settings exceeds the 1500 ms startup timeout", async ({
      page,
      backend,
      openApp,
    }) => {
      await page.addInitScript(() => {
        const e2e = (
          window as unknown as {
            __e2e?: {
              holdNext: (cmd: string) => void;
              failNext: (cmd: string, code: string) => void;
            };
          }
        ).__e2e;
        e2e?.holdNext("get_ui_settings");
        e2e?.failNext("set_last_phrase", "storage");
      });
      await openApp();

      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(page.getByText(text("hero.subtitle"))).toBeVisible();
      await expect(playButton(page)).toHaveText(text("play.play"));
      expect(await backend.calls("get_ui_settings")).toHaveLength(1);
    });
  });

  test("rolls back optimistic profile selection if select_profile rejects", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const listsBefore = (await backend.calls("list_profiles")).length;
    await backend.failNext("select_profile", "not_found");

    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();

    await expect
      .poll(async () => (await backend.calls("select_profile")).map((c) => c.args))
      .toEqual([{ id: "fresh" }]);
    await expect
      .poll(async () => (await backend.calls("list_profiles")).length)
      .toBeGreaterThan(listsBefore);
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: SURVIVAL.name }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    expect((await backend.state()).selected).toBe(SURVIVAL.id);
  });

  test("rolls back optimistic account selection if select_account rejects", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const listsBefore = (await backend.calls("list_accounts")).length;
    await backend.failNext("select_account", "storage");

    await accountTrigger(page).click();
    await page.getByRole("menu").getByRole("menuitemradio", { name: BLAKE.username }).click();

    await expect
      .poll(async () => (await backend.calls("select_account")).map((c) => c.args))
      .toEqual([{ id: BLAKE.id }]);
    await expect
      .poll(async () => (await backend.calls("list_accounts")).length)
      .toBeGreaterThan(listsBefore);
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: ALEX.username }),
    );
    expect((await backend.state()).active).toBe(ALEX.id);
  });
});

test.describe("home unreadable-profile banner", () => {
  test.use({ backendConfig: { unreadable: ["broken-profile"] } });

  test("shows an inline alert if open_instances_folder rejects and clears it on retry", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const banner = page.getByRole("status");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText(
      text("profile.unreadableNotice", { count: 1, folders: "broken-profile" }),
    );

    await backend.failNext("open_instances_folder", "open_failed");
    await banner.getByRole("button", { name: text("profile.openInstancesFolder") }).click();
    await expect(banner.getByRole("alert")).toHaveText(text("error.profile.open_failed"));

    await banner.getByRole("button", { name: text("profile.openInstancesFolder") }).click();
    await expect(banner.getByRole("alert")).toHaveCount(0);
    expect(await backend.calls("open_instances_folder")).toHaveLength(2);
  });
});

test.describe("layout", () => {
  for (const [width, height] of [
    [960, 600],
    [1280, 720],
    [1440, 900],
    [1920, 1080],
  ]) {
    test(`the home screen has no overflow or overlap at ${width}x${height}`, async ({
      page,
      openApp,
    }) => {
      await page.setViewportSize({ width, height });
      await openApp();
      await assertHomeLayout(page, width, height);
    });
  }

  test("the home screen has no overflow or overlap in Russian at the smallest window", async ({
    page,
    openApp,
  }) => {
    await page.setViewportSize({ width: 960, height: 600 });
    await openApp();
    await settingsButton(page).click();
    await page.getByRole("radio", { name: "Русский" }).click();
    await page.getByRole("button", { name: textRu("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator("html")).toHaveAttribute("lang", "ru");
    await expect(playButton(page)).toHaveText(textRu("play.play"));
    await assertHomeLayout(page, 960, 600);
  });

  test.describe("Russian fresh-install layout at 960x600", () => {
    test.use({
      backendConfig: {
        profiles: [],
        selected: null,
        accounts: [],
        active: null,
        ui: { ...DEFAULT_CONFIG.ui, language: "ru" },
      },
    });

    test("fits Create first profile and Add account without overflow or overlap", async ({
      page,
      openApp,
    }) => {
      await page.setViewportSize({ width: 960, height: 600 });
      await openApp();
      await expect(profileTrigger(page)).toContainText(textRu("profile.createFirst"));
      await expect(accountTrigger(page)).toHaveAccessibleName(textRu("account.triggerNone"));
      await expect(playButton(page)).toHaveText(textRu("play.play"));
      await assertHomeLayout(page, 960, 600);
    });
  });

  test.describe("Russian sign-in-required layout at 960x600", () => {
    test.use({
      backendConfig: {
        profiles: [SURVIVAL],
        selected: SURVIVAL.id,
        accounts: [{ ...ALEX, needsSignIn: true }],
        active: ALEX.id,
        ui: { ...DEFAULT_CONFIG.ui, language: "ru" },
      },
    });

    test("fits Sign in to continue without overflow or overlap", async ({ page, openApp }) => {
      await page.setViewportSize({ width: 960, height: 600 });
      await openApp();
      await expect(accountTrigger(page)).toHaveAccessibleName(
        textRu("account.trigger", { name: ALEX.username }),
      );
      await expect(profileTrigger(page)).toHaveAccessibleName(
        textRu("profile.trigger", { label: SURVIVAL.name }),
      );
      await expect(playButton(page)).toHaveText(textRu("play.signIn"));
      await assertHomeLayout(page, 960, 600);
    });
  });
});
