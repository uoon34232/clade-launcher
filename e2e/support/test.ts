// The test object every spec uses: a Playwright `test` whose page runs the real frontend on top of
// the fake backend, and which fails a test that logs a console error or an uncaught exception.

import { expect, test as base, type Page } from "@playwright/test";
import { Backend } from "./fakeBackend";
import type { BackendConfig } from "./fixtures";

interface Fixtures {
  /** Overrides for the fake backend's starting data (set with `test.use`). */
  backendConfig: Partial<BackendConfig>;
  /** The fake backend, installed before the page loads. */
  backend: Backend;
  /** Opens the launcher and waits until it has rendered. */
  openApp: () => Promise<void>;
}

export const test = base.extend<Fixtures>({
  page: async ({ page }, provide) => {
    const problems: string[] = [];
    page.on("pageerror", (e) => problems.push(`uncaught: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") problems.push(`console.error: ${m.text()}`);
    });
    await provide(page);
    expect(problems, "the page must not log errors").toEqual([]);
  },
  backendConfig: [{}, { option: true }],
  backend: async ({ page, backendConfig }, provide) => {
    await provide(await Backend.install(page, backendConfig));
  },
  openApp: async ({ page, backend }, provide) => {
    void backend;
    await provide(async () => {
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    });
  },
});

export { expect };
export type { Page };
