// The test object every spec uses: a Playwright `test` whose page runs the real frontend on top of
// the fake backend, and which fails a test that logs a console error or an uncaught exception.

import { expect, test as base, type Page } from "@playwright/test";
import { Backend } from "./fakeBackend";
import type { BackendConfig } from "./fixtures";

interface E2eWindowState {
  calls: { cmd: string }[];
  state: {
    profiles: { id: string; name: string; ready: boolean }[];
    selected: string | null;
    unreadable: string[];
    removed: unknown[];
    accounts: { id: string; username: string; needsSignIn: boolean }[];
    active: string | null;
    runs: Record<string, "launching" | "playing">;
    embed: { active: boolean };
  };
}

const STARTUP_COMMANDS: string[] = [
  "list_profiles",
  "list_accounts",
  "launch_status",
  "get_game_embed_status",
];

interface Fixtures {
  /** Overrides for the fake backend's starting data (set with `test.use`). */
  backendConfig: Partial<BackendConfig>;
  /** The fake backend, installed before the page loads. */
  backend: Backend;
  /** Opens the launcher and waits until initial IPC hydration and stage/dock rendering settle. */
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
    await provide(async () => {
      await page.goto("/");
      await page.waitForFunction((startupCmds: string[]) => {
        const e2e = (window as unknown as { __e2e?: E2eWindowState }).__e2e;
        if (!e2e) return false;
        const called = new Set(e2e.calls.map((c) => c.cmd));
        if (!startupCmds.every((cmd) => called.has(cmd))) return false;

        const stage = document.querySelector('[data-testid="game-stage"]');
        if (!stage) return false;

        if (e2e.state.embed.active) {
          return (
            stage.getAttribute("aria-hidden") === "false" && document.querySelector("h1") === null
          );
        }

        if (stage.getAttribute("aria-hidden") !== "true") return false;
        if (!document.querySelector("h1")) return false;

        const nav = document.querySelector('nav[aria-label="Clade"]');
        if (!nav) return false;

        const menuButtons = Array.from(
          nav.querySelectorAll('button[data-slot="dropdown-menu-trigger"]'),
        );
        const { profiles, selected, unreadable, removed, accounts, active, runs } = e2e.state;
        const activeAccount = accounts.find((a) => a.id === active) ?? accounts[0] ?? null;
        if (
          activeAccount &&
          !(menuButtons[0]?.textContent ?? "").includes(activeAccount.username)
        ) {
          return false;
        }

        const selectedProfile = profiles.find((p) => p.id === selected) ?? profiles[0] ?? null;
        if (selectedProfile) {
          if (
            menuButtons.length < 2 ||
            !(menuButtons[1]?.textContent ?? "").includes(selectedProfile.name)
          ) {
            return false;
          }
          const run = runs[selectedProfile.id];
          const accountReady = activeAccount !== null && !activeAccount.needsSignIn;
          const expectedPlayState =
            run ?? (!accountReady ? "signin" : !selectedProfile.ready ? "setup" : "play");
          const playBtn = nav.querySelector("button[data-state]");
          if (playBtn?.getAttribute("data-state") !== expectedPlayState) return false;
        } else {
          if (menuButtons.length !== 1) return false;
          if (
            (unreadable.length > 0 || removed.length > 0) &&
            !document.querySelector('[role="status"]')
          ) {
            return false;
          }
        }

        return true;
      }, STARTUP_COMMANDS);

      const { embed } = await backend.state();
      if (embed.active) {
        await expect(page.getByTestId("game-stage")).toBeVisible();
      } else {
        await expect(page.locator("h1")).toBeVisible();
      }
    });
  },
});

export { expect };
export type { Page };
