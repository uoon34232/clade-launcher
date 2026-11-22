import { ALEX, BLAKE, CREATIVE, FRESH, SURVIVAL } from "./support/fixtures";
import { expect, test, type Page } from "./support/test";
import { accountTrigger, dock, playButton, profileTrigger, text } from "./support/ui";

async function invokeGateCheck(
  page: Page,
  cmd: string,
  args: Record<string, unknown>,
): Promise<{ accepted: true } | { accepted: false; code: string; accountId: string | null }> {
  return page.evaluate(
    async ([c, a]) => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (command: string, payload: unknown) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      try {
        await internals.invoke(c, a);
        return { accepted: true as const };
      } catch (e) {
        const err = e as { code?: string; accountId?: string | null };
        return {
          accepted: false as const,
          code: err.code ?? "unknown",
          accountId: err.accountId ?? null,
        };
      }
    },
    [cmd, args] as const,
  );
}

async function expectVerifyGated(page: Page, profileName: string, gated: boolean): Promise<void> {
  await profileTrigger(page).click();
  const menu = page.locator('[role="menu"][data-open]');
  await expect(menu).toBeVisible();
  const menuVerify = menu.getByRole("menuitem", { name: text("profile.verify") });
  if (gated) {
    await expect(menuVerify).toHaveAttribute("data-disabled", /.*/);
  } else {
    await expect(menuVerify).not.toHaveAttribute("data-disabled", /.*/);
  }

  await menu.getByRole("menuitem", { name: text("profile.settings") }).click();
  const dialog = page.getByRole("dialog", { name: profileName });
  await expect(dialog).toBeVisible();
  const modalVerify = dialog.getByRole("button", { name: text("profile.verify") });
  if (gated) {
    await expect(modalVerify).toBeDisabled();
  } else {
    await expect(modalVerify).toBeEnabled();
  }
  await dialog.getByRole("button", { name: text("common.done") }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
}

async function submitCreateProfile(page: Page, name: string): Promise<void> {
  const dialog = page.getByRole("dialog", { name: text("create.title") });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("combobox", { name: text("create.mcVersion") })).toContainText(
    "1.21.4",
  );
  await dialog.getByLabel(text("create.name")).fill(name);
  await dialog.getByRole("button", { name: text("create.submit") }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
}

test.describe("without an account", () => {
  test.use({ backendConfig: { accounts: [], active: null } });

  test("profiles can be made and chosen, but no game file is touched: the primary action asks to sign in", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    // An unprepared and a prepared profile alike: Set up and Play are replaced by the sign-in.
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();
    await expect(page.getByRole("menu")).toHaveCount(0);
    await expect(playButton(page)).toHaveText(text("play.signIn"));

    // Verify files is unavailable too, both in the profile menu and in Profile settings.
    await expectVerifyGated(page, FRESH.name, true);

    // Creating a profile works without an account, and the new unprepared profile still asks to sign in.
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.create") }).click();
    await submitCreateProfile(page, "Weekend");
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: "Weekend" }),
    );
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    expect((await backend.calls("create_profile")).map((c) => c.args)).toEqual([
      { request: { name: "Weekend", minecraft: "1.21.4", profile: { type: "vanilla" } } },
    ]);

    // The primary action starts the normal sign-in (it asks for the passphrase first).
    await playButton(page).click();
    await expect(page.getByRole("dialog", { name: text("signin.createTitle") })).toBeVisible();
    await page.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("launch_profile")).toHaveLength(0);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
  });

  test.describe("with no profiles either", () => {
    test.use({ backendConfig: { profiles: [], selected: null, accounts: [], active: null } });

    test("keeps the dock focused on creating a profile until one exists, then asks to sign in", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await expect(profileTrigger(page)).toContainText(text("profile.createFirst"));
      await expect(playButton(page)).toHaveText(text("play.play"));

      await playButton(page).click();
      await submitCreateProfile(page, "Starter");
      await expect(profileTrigger(page)).toHaveAccessibleName(
        text("profile.trigger", { label: "Starter" }),
      );
      await expect(playButton(page)).toHaveText(text("play.signIn"));

      expect(await backend.calls("prepare_profile")).toHaveLength(0);
      expect(await backend.calls("launch_profile")).toHaveLength(0);
      expect(await backend.calls("begin_sign_in")).toHaveLength(0);
    });
  });

  test("the backend itself refuses a direct prepare, verify, or launch request", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    expect(await invokeGateCheck(page, "prepare_profile", { id: FRESH.id, verify: false })).toEqual(
      {
        accepted: false,
        code: "account_required",
        accountId: null,
      },
    );
    expect(await invokeGateCheck(page, "prepare_profile", { id: FRESH.id, verify: true })).toEqual({
      accepted: false,
      code: "account_required",
      accountId: null,
    });
    expect(await invokeGateCheck(page, "launch_profile", { id: SURVIVAL.id })).toEqual({
      accepted: false,
      code: "no_account",
      accountId: null,
    });

    const state = await backend.state();
    expect(state.accounts).toEqual([]);
    expect(state.active).toBeNull();
    expect(state.runs).toEqual({});
    expect(state.profiles.map((p) => ({ id: p.id, ready: p.ready }))).toEqual([
      { id: SURVIVAL.id, ready: true },
      { id: CREATIVE.id, ready: true },
      { id: FRESH.id, ready: false },
    ]);
  });
});

test.describe("an account that must sign in again", () => {
  test.use({
    backendConfig: { accounts: [{ ...ALEX, needsSignIn: true }, BLAKE], active: ALEX.id },
  });

  test("keeps the profiles, gates Play and Verify files, and opens the vault step first", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await expect(profileTrigger(page)).toHaveAccessibleName(
      text("profile.trigger", { label: SURVIVAL.name }),
    );
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(page.getByRole("menuitemradio")).toHaveCount(3); // nothing was removed
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await expectVerifyGated(page, SURVIVAL.name, true);

    await playButton(page).click();
    // Without an open vault yet, the passphrase step opens first; nothing is downloaded.
    await expect(page.getByRole("dialog", { name: text("signin.createTitle") })).toBeVisible();
    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("launch_profile")).toHaveLength(0);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
  });

  test.describe("with an unlocked vault", () => {
    test.use({
      backendConfig: {
        accounts: [{ ...ALEX, needsSignIn: true }, BLAKE],
        active: ALEX.id,
        vault: { exists: true, unlocked: true },
      },
    });

    test("re-signs the same active account in when Play is clicked", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await expect(playButton(page)).toHaveText(text("play.signIn"));
      await playButton(page).click();

      await expect(page.getByRole("dialog", { name: text("signin.againTitle") })).toBeVisible();
      await expect
        .poll(async () => (await backend.calls("begin_sign_in")).map((c) => c.args))
        .toEqual([{ accountId: ALEX.id }]);

      expect(await backend.calls("prepare_profile")).toHaveLength(0);
      expect(await backend.calls("launch_profile")).toHaveLength(0);
    });
  });

  test("the backend itself refuses a direct prepare, verify, or launch request even with a ready sibling", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    expect(await invokeGateCheck(page, "prepare_profile", { id: FRESH.id, verify: false })).toEqual(
      {
        accepted: false,
        code: "account_required",
        accountId: null,
      },
    );
    expect(await invokeGateCheck(page, "prepare_profile", { id: FRESH.id, verify: true })).toEqual({
      accepted: false,
      code: "account_required",
      accountId: null,
    });
    expect(await invokeGateCheck(page, "launch_profile", { id: SURVIVAL.id })).toEqual({
      accepted: false,
      code: "needs_sign_in",
      accountId: ALEX.id,
    });

    const state = await backend.state();
    expect(state.accounts).toEqual([{ ...ALEX, needsSignIn: true }, BLAKE]);
    expect(state.active).toBe(ALEX.id);
    expect(state.runs).toEqual({});
    expect(state.profiles.map((p) => ({ id: p.id, ready: p.ready }))).toEqual([
      { id: SURVIVAL.id, ready: true },
      { id: CREATIVE.id, ready: true },
      { id: FRESH.id, ready: false },
    ]);
  });

  test("a ready account next to it is never used instead; switching and removing re-gates every control", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.signIn"));

    // Switching to the ready account unlocks Play and Verify files.
    await accountTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) }).click();
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expectVerifyGated(page, SURVIVAL.name, false);

    // Switching back to the account that needs sign-in re-gates Play and Verify files.
    await accountTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(ALEX.username) }).click();
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await expectVerifyGated(page, SURVIVAL.name, true);

    // With an unprepared profile selected, switching to the ready account unlocks Set up and Verify files.
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(`^${FRESH.name}`) }).click();
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await accountTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) }).click();
    await expect(playButton(page)).toHaveText(text("play.setup"));
    await expectVerifyGated(page, FRESH.name, false);

    // Removing the ready account falls back to ALEX (needsSignIn) and re-gates the dock and Profile settings.
    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: text("account.manage", { name: BLAKE.username }) })
      .click();
    const accountDialog = page.getByRole("dialog", { name: BLAKE.username });
    await expect(accountDialog).toBeVisible();
    await accountDialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await accountDialog.getByRole("button", { name: text("accountModal.remove") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: ALEX.username }),
    );
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await expectVerifyGated(page, FRESH.name, true);

    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("launch_profile")).toHaveLength(0);
  });
});

test.describe("with an account that can play", () => {
  test("Set up and Play are offered as before", async ({ page, openApp }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.play"));
    await profileTrigger(page).click();
    await page.getByRole("menuitemradio", { name: new RegExp(`^${FRESH.name}`) }).click();
    await expect(playButton(page)).toHaveText(text("play.setup"));
    await expect(dock(page)).not.toContainText(text("play.signIn"));
  });

  test("the collapsed profile button shows only the profile's name; the menu keeps the details", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const trigger = profileTrigger(page);
    await expect(trigger).toHaveText("Survival");
    await trigger.click();
    await expect(page.getByRole("menuitemradio", { name: /^Survival/ })).toContainText(
      text("profile.vanilla"),
    );
  });
});
