import type { Locator, Page } from "@playwright/test";
import type { Backend } from "./support/fakeBackend";
import { ALEX, BLAKE, DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, playButton, text, textRu } from "./support/ui";

// "Forgot passphrase?" and "Start fresh" in the three places that ask for the vault passphrase, against
// the fake backend: nothing here can touch a real vault, account or file.

const LOCKED = { vault: { exists: true, unlocked: false } };

const dialog = (page: Page) => page.getByRole("dialog");
const forgot = (d: Locator) => d.getByRole("button", { name: text("recovery.link") });
const startFresh = (d: Locator) => d.getByRole("button", { name: text("recovery.confirm") });
const keepTrying = (d: Locator) => d.getByRole("button", { name: text("recovery.keep") });
const workingButton = (d: Locator) => d.getByRole("button", { name: text("common.working") });

/** A closing dialog loses `data-open` at once; wait two frames so a click or key has been handled. */
const stillOpen = async (page: Page, d: Locator) => {
  await page.evaluate(
    () =>
      new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  );
  await expect(d).toHaveAttribute("data-open", "");
  await expect(d).not.toHaveAttribute("data-closed", "");
};

/** Clicks the dialog overlay in its exposed margin outside the centered dialog card. */
const clickBackdrop = async (page: Page, d: Locator) => {
  const overlay = page.locator('[data-slot="dialog-overlay"]');
  await expect(overlay).toBeVisible();
  const box = await d.boundingBox();
  const x = box && box.x > 16 ? Math.floor(box.x / 2) : 8;
  const y = box && box.y > 16 ? Math.floor(box.y / 2) : 8;
  await overlay.click({ position: { x, y } });
};

const openAddAccount = async (page: Page) => {
  await accountTrigger(page).click();
  await page.getByRole("menuitem", { name: text("account.add") }).click();
  await expect(dialog(page)).toBeVisible();
};

const openManageAlex = async (page: Page) => {
  await accountTrigger(page).click();
  await page
    .getByRole("menuitem", { name: text("account.manage", { name: ALEX.username }) })
    .click();
  await expect(dialog(page)).toBeVisible();
};

const expectPostResetState = async (backend: Backend, expectedRetired = 1) => {
  const state = await backend.state();
  expect(state.vault).toEqual({ exists: false, unlocked: false });
  expect(state.retiredVaults).toBe(expectedRetired);
  expect(state.active).toBe(ALEX.id);
  expect(state.accounts).toHaveLength(2);
  expect(state.accounts.every((a) => a.needsSignIn)).toBe(true);
  return state;
};

const expectLaunchNeedsSignIn = async (page: Page) => {
  const code = await page.evaluate(async () => {
    try {
      await (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__.invoke("launch_profile", { id: "survival" });
      return null;
    } catch (err) {
      return (err as { code?: string } | null)?.code ?? null;
    }
  });
  expect(code).toBe("needs_sign_in");
};

test.describe("in the sign-in dialog", () => {
  test.use({ backendConfig: LOCKED });

  test("a locked vault: Start fresh archives it, locks while running, and cancels cleanly from the new-passphrase step", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await openAddAccount(page);
    const d = dialog(page);
    await expect(d.getByRole("heading", { name: text("signin.unlockTitle") })).toBeVisible();

    await forgot(d).click();
    await expect(d.getByRole("heading", { name: text("recovery.title") })).toBeVisible();
    await expect(d.getByText(text("recovery.body"))).toBeVisible();
    await expect(keepTrying(d)).toBeFocused();
    expect(await backend.calls("reset_vault")).toHaveLength(0);

    // Escape leaves the question alone (SignInDialog itself sets closeOnBackdrop={false})
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await expect(d.getByRole("heading", { name: text("recovery.title") })).toBeVisible();

    // Cancel changes nothing, and the passphrase form is back with focus restored
    await keepTrying(d).click();
    await expect(d.getByLabel(text("common.passphrase"))).toBeVisible();
    await expect(forgot(d)).toBeFocused();
    expect(await backend.calls("reset_vault")).toHaveLength(0);
    expect((await backend.state()).vault).toEqual({ exists: true, unlocked: false });

    // Hold reset_vault in flight to verify busy copy, disabled controls, single IPC call, and Escape lock
    await forgot(d).click();
    await backend.holdNext("reset_vault");
    await startFresh(d).click();
    const working = workingButton(d);
    await expect(working).toBeVisible();
    await expect(working).toBeDisabled();
    await expect(keepTrying(d)).toBeDisabled();
    await working.click({ force: true });
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    await page.keyboard.press("Escape");
    await stillOpen(page, d);

    await backend.release();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
    await expect(d.getByLabel(text("signin.repeat"))).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    await expectPostResetState(backend);
    expect(await backend.calls("create_vault")).toHaveLength(0);
    await expect(page.getByRole("alert")).toHaveCount(0);

    // Cancelling the new-passphrase step leaves no vault created and marks Play as needing sign-in
    await d.getByRole("button", { name: text("common.cancel") }).click();
    await expect(dialog(page)).toHaveCount(0);
    expect(await backend.calls("create_vault")).toHaveLength(0);
    expect((await backend.state()).vault).toEqual({ exists: false, unlocked: false });
    await expect(playButton(page)).toHaveAttribute("data-state", "signin");
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await expectLaunchNeedsSignIn(page);
  });

  test("a refusal is explained and the question stays open", async ({ page, backend, openApp }) => {
    await openApp();
    await openAddAccount(page);
    const d = dialog(page);
    await forgot(d).click();
    await backend.failNext("reset_vault", "game_starting");
    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.game_starting"));
    await expect(startFresh(d)).toBeEnabled();
    // Cancelling and reopening clears the stale alert
    await keepTrying(d).click();
    await forgot(d).click();
    await expect(d.getByRole("alert")).toHaveCount(0);
    // and it can be answered again
    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
  });

  test("hides Keep trying when the vault was already archived (accounts_not_updated), leaving Start fresh focused to retry", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await openAddAccount(page);
    const d = dialog(page);
    await forgot(d).click();
    const listBefore = (await backend.calls("list_accounts")).length;
    await backend.failNext("reset_vault", "accounts_not_updated");
    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.accounts_not_updated"));
    await expect(keepTrying(d)).toHaveCount(0);
    await expect(startFresh(d)).toBeEnabled();
    await expect(startFresh(d)).toBeFocused();
    await expect
      .poll(async () => (await backend.calls("list_accounts")).length)
      .toBeGreaterThan(listBefore);
    const midState = await backend.state();
    expect(midState.vault).toEqual({ exists: false, unlocked: false });
    expect(midState.retiredVaults).toBe(1);

    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
    await expectPostResetState(backend, 1);
  });

  for (const failure of [
    { code: "vault_missing", key: "error.vault.vault_missing" },
    { code: "vault_unreadable", key: "error.vault.vault_unreadable" },
    { code: "passphrase_mismatch", key: "error.vault.passphrase_mismatch" },
  ] as const) {
    test(`unlock failure (${failure.code}) offers Forgot passphrase? and recovers`, async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await openAddAccount(page);
      const d = dialog(page);
      await d.getByLabel(text("common.passphrase")).fill("fake test passphrase");
      await backend.failNext("unlock_vault", failure.code);
      await d.getByRole("button", { name: text("common.unlock") }).click();
      await expect(d.getByRole("alert")).toHaveText(text(failure.key));
      await forgot(d).click();
      await startFresh(d).click();
      await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
      await expect(d.getByRole("alert")).toHaveCount(0);
    });
  }
});

test.describe("Sign in again on a locked vault", () => {
  test.use({
    backendConfig: {
      ...LOCKED,
      accounts: [{ ...ALEX, needsSignIn: true }, BLAKE],
    },
  });

  test("Start fresh leads to creating a new vault and preserves needs-sign-in status", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveAttribute("data-state", "signin");
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await playButton(page).click();

    const d = dialog(page);
    await expect(d.getByRole("heading", { name: text("signin.unlockTitle") })).toBeVisible();
    await forgot(d).click();
    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();

    await d.getByLabel(text("common.passphrase"), { exact: true }).fill("new test passphrase");
    await d.getByLabel(text("signin.repeat")).fill("new test passphrase");
    await backend.failNext("begin_sign_in", "approval_required");
    await d.getByRole("button", { name: text("common.continue") }).click();

    await expect(d.getByRole("heading", { name: text("signin.againTitle") })).toBeVisible();
    await expect(d.getByRole("alert")).toHaveText(text("error.signIn.approval_required"));
    await d.getByRole("button", { name: text("common.close") }).click();
    await expect(dialog(page)).toHaveCount(0);

    expect(await backend.calls("reset_vault")).toHaveLength(1);
    expect(await backend.calls("create_vault")).toHaveLength(1);
    const state = await backend.state();
    expect(state.vault).toEqual({ exists: true, unlocked: true });
    expect(state.retiredVaults).toBe(1);
    expect(state.accounts.every((a) => a.needsSignIn)).toBe(true);

    await expect(playButton(page)).toHaveAttribute("data-state", "signin");
    await accountTrigger(page).click();
    await expect(
      page.getByRole("menu").getByText(text("account.needsSignIn")).first(),
    ).toBeVisible();
    await page.keyboard.press("Escape");
  });
});

test.describe("in the launch dialog", () => {
  test.use({ backendConfig: LOCKED });

  test("unlock to play: Start fresh turns it into Sign in again and hands off to create passphrase", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const d = dialog(page);
    await expect(d.getByRole("heading", { name: text("problem.unlockTitle") })).toBeVisible();

    await forgot(d).click();
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await keepTrying(d).click();
    await expect(d.getByLabel(text("common.passphrase"))).toBeVisible();
    await expect(forgot(d)).toBeFocused();
    expect(await backend.calls("reset_vault")).toHaveLength(0);

    await forgot(d).click();
    await startFresh(d).click();
    await expect(
      d.getByRole("heading", { name: text("problem.title.needs_sign_in") }),
    ).toBeVisible();
    await expect(d.getByRole("alert")).toHaveText(text("error.launch.needs_sign_in"));
    const again = d.getByRole("button", { name: text("common.signInAgain") });
    await expect(again).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    await expectPostResetState(backend);

    // from there the sign-in starts at the new-passphrase step
    await again.click();
    await expect(
      dialog(page).getByRole("heading", { name: text("signin.createTitle") }),
    ).toBeVisible();
  });

  test("handles reset_failed, accounts_not_updated, and unlocks Escape once Start fresh finishes", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const d = dialog(page);

    await forgot(d).click();
    await backend.failNext("reset_vault", "reset_failed");
    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.reset_failed"));
    await expect(keepTrying(d)).toBeEnabled();
    await expect(startFresh(d)).toBeEnabled();

    // Cancel and reopen to confirm the error alert clears
    await keepTrying(d).click();
    await forgot(d).click();
    await expect(d.getByRole("alert")).toHaveCount(0);

    // Stage 3 partial failure keeps Start fresh focused and removes Keep trying
    await backend.failNext("reset_vault", "accounts_not_updated");
    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.accounts_not_updated"));
    await expect(keepTrying(d)).toHaveCount(0);
    await expect(startFresh(d)).toBeEnabled();
    await expect(startFresh(d)).toBeFocused();
    const midState = await backend.state();
    expect(midState.vault).toEqual({ exists: false, unlocked: false });
    expect(midState.retiredVaults).toBe(1);

    await startFresh(d).click();
    await expect(
      d.getByRole("heading", { name: text("problem.title.needs_sign_in") }),
    ).toBeVisible();
    await expect(d.getByRole("alert")).toHaveText(text("error.launch.needs_sign_in"));

    // Escape works again once VaultRecovery unmounts
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
    await expect(playButton(page)).toHaveAttribute("data-state", "signin");
  });

  test("unlock failures (passphrase_mismatch, vault_unreadable) can recover via Start fresh", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const d = dialog(page);

    await d.getByLabel(text("common.passphrase")).fill("wrong passphrase");
    await backend.failNext("unlock_vault", "passphrase_mismatch");
    await d.getByRole("button", { name: text("common.unlock") }).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.passphrase_mismatch"));

    await d.getByLabel(text("common.passphrase")).fill("another passphrase");
    await backend.failNext("unlock_vault", "vault_unreadable");
    await d.getByRole("button", { name: text("common.unlock") }).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.vault_unreadable"));

    await forgot(d).click();
    await startFresh(d).click();
    await expect(
      d.getByRole("heading", { name: text("problem.title.needs_sign_in") }),
    ).toBeVisible();
    await d.getByRole("button", { name: text("common.close") }).click();
    await expect(dialog(page)).toHaveCount(0);
  });
});

test.describe("in the account dialog", () => {
  test.use({
    backendConfig: { ...LOCKED, appearance: { [ALEX.id]: { skinModel: "classic", capes: [] } } },
  });

  test("the appearance unlock offers Start fresh, locks while pending, and hands off to Sign in again", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.failNext("account_appearance", "vault_locked");
    await openManageAlex(page);
    const d = dialog(page);
    await expect(d.getByRole("alert").first()).toHaveText(text("error.appearance.vault_locked"));

    await forgot(d).click();
    await expect(d.getByRole("group", { name: text("recovery.title") })).toBeVisible();
    await expect(
      d.getByRole("button", { name: text("accountModal.signOutRemove") }),
    ).toBeDisabled();
    await expect(d.getByRole("button", { name: text("common.done") })).toBeDisabled();
    // the dialog around it holds still
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await clickBackdrop(page, d);
    await stillOpen(page, d);
    await keepTrying(d).click();
    await expect(d.getByLabel(text("common.passphrase"))).toBeVisible();
    await expect(forgot(d)).toBeFocused();
    await expect(d.getByRole("button", { name: text("common.done") })).toBeEnabled();
    await expect(d.getByRole("button", { name: text("accountModal.signOutRemove") })).toBeEnabled();
    expect(await backend.calls("reset_vault")).toHaveLength(0);

    // Hold reset_vault in flight to verify inline busy state and modal lock
    await forgot(d).click();
    await backend.holdNext("reset_vault");
    await startFresh(d).click();
    const working = workingButton(d);
    await expect(working).toBeVisible();
    await expect(working).toBeDisabled();
    await expect(keepTrying(d)).toBeDisabled();
    await expect(
      d.getByRole("button", { name: text("accountModal.signOutRemove") }),
    ).toBeDisabled();
    await expect(d.getByRole("button", { name: text("common.done") })).toBeDisabled();
    await working.click({ force: true });
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await clickBackdrop(page, d);
    await stillOpen(page, d);

    await backend.release();
    // the existing signed-out state: the explanation, controls re-enabled, and the way back in
    await expect(d.getByRole("alert")).toHaveText(text("appearance.needsSignIn"));
    await expect(d.getByRole("button", { name: text("accountModal.choosePng") })).toBeDisabled();
    await expect(d.getByRole("button", { name: text("accountModal.signOutRemove") })).toBeEnabled();
    await expect(d.getByRole("button", { name: text("common.done") })).toBeEnabled();
    const again = d.getByRole("button", { name: text("common.signInAgain") });
    await expect(again).toBeVisible();
    await expect(d.getByText(text("accountModal.statusNeedsSignIn"))).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    await expectPostResetState(backend);

    // Sign in again hands off from AccountDialog to SignInDialog at the new-passphrase step
    await again.click();
    await expect(
      dialog(page).getByRole("heading", { name: text("signin.createTitle") }),
    ).toBeVisible();
    await dialog(page)
      .getByRole("button", { name: text("common.cancel") })
      .click();
    await expect(dialog(page)).toHaveCount(0);
  });

  test("inline layout handles reset_failed, keeps modal locked on accounts_not_updated, and unlocks Escape after retry", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.failNext("account_appearance", "vault_locked");
    await openManageAlex(page);
    const d = dialog(page);
    const recoveryGroup = d.getByRole("group", { name: text("recovery.title") });

    await forgot(d).click();
    await backend.failNext("reset_vault", "reset_failed");
    await startFresh(d).click();
    await expect(recoveryGroup.getByRole("alert")).toHaveText(text("error.vault.reset_failed"));
    await expect(keepTrying(d)).toBeEnabled();
    await expect(startFresh(d)).toBeEnabled();

    await keepTrying(d).click();
    await forgot(d).click();
    await expect(recoveryGroup.getByRole("alert")).toHaveCount(0);

    await backend.failNext("reset_vault", "accounts_not_updated");
    await startFresh(d).click();
    await expect(recoveryGroup.getByRole("alert")).toHaveText(
      text("error.vault.accounts_not_updated"),
    );
    await expect(keepTrying(d)).toHaveCount(0);
    await expect(startFresh(d)).toBeEnabled();
    await expect(startFresh(d)).toBeFocused();
    await expect(
      d.getByRole("button", { name: text("accountModal.signOutRemove") }),
    ).toBeDisabled();
    await expect(d.getByRole("button", { name: text("common.done") })).toBeDisabled();
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await clickBackdrop(page, d);
    await stillOpen(page, d);
    const midState = await backend.state();
    expect(midState.vault).toEqual({ exists: false, unlocked: false });
    expect(midState.retiredVaults).toBe(1);

    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("appearance.needsSignIn"));
    // Escape works again once the question is gone
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
  });

  test("inline unlock failures (passphrase_mismatch, vault_unreadable) recover via Start fresh", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.failNext("account_appearance", "vault_locked");
    await openManageAlex(page);
    const d = dialog(page);

    await d.getByLabel(text("common.passphrase")).fill("wrong passphrase");
    await backend.failNext("unlock_vault", "passphrase_mismatch");
    await d.getByRole("button", { name: text("common.unlock") }).click();
    await expect(d.locator("#appearance-unlock-error")).toHaveText(
      text("error.vault.passphrase_mismatch"),
    );

    await d.getByLabel(text("common.passphrase")).fill("another passphrase");
    await backend.failNext("unlock_vault", "vault_unreadable");
    await d.getByRole("button", { name: text("common.unlock") }).click();
    await expect(d.locator("#appearance-unlock-error")).toHaveText(
      text("error.vault.vault_unreadable"),
    );

    await forgot(d).click();
    await startFresh(d).click();
    await expect(d.getByRole("alert")).toHaveText(text("appearance.needsSignIn"));
  });
});

test.describe("in Russian", () => {
  test.use({
    backendConfig: {
      ...LOCKED,
      appearance: { [ALEX.id]: { skinModel: "classic", capes: [] } },
      ui: { ...DEFAULT_CONFIG.ui, language: "ru" },
    },
  });

  test("the sign-in modal recovery flow and vault errors read in Russian", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: textRu("account.add") }).click();
    const d = dialog(page);

    await d.getByLabel(textRu("common.passphrase")).fill("неверная фраза");
    await backend.failNext("unlock_vault", "passphrase_mismatch");
    await d.getByRole("button", { name: textRu("common.unlock") }).click();
    await expect(d.getByRole("alert")).toHaveText(textRu("error.vault.passphrase_mismatch"));

    await d.getByRole("button", { name: textRu("recovery.link") }).click();
    await expect(d.getByRole("heading", { name: textRu("recovery.title") })).toBeVisible();
    await expect(d.getByText(textRu("recovery.body"))).toBeVisible();
    await expect(d.getByRole("button", { name: textRu("recovery.keep") })).toBeFocused();

    await backend.failNext("reset_vault", "accounts_not_updated");
    await d.getByRole("button", { name: textRu("recovery.confirm") }).click();
    await expect(d.getByRole("alert")).toHaveText(textRu("error.vault.accounts_not_updated"));
    await expect(d.getByRole("button", { name: textRu("recovery.keep") })).toHaveCount(0);
    await expect(d.getByRole("button", { name: textRu("recovery.confirm") })).toBeFocused();

    await d.getByRole("button", { name: textRu("recovery.confirm") }).click();
    await expect(d.getByRole("heading", { name: textRu("signin.createTitle") })).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(2);
  });

  test("the launch dialog and inline account dialog recovery flows read in Russian", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();

    // LaunchProblemDialog in Russian
    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const d = dialog(page);
    await expect(d.getByRole("heading", { name: textRu("problem.unlockTitle") })).toBeVisible();
    await d.getByRole("button", { name: textRu("recovery.link") }).click();
    await backend.failNext("reset_vault", "game_starting");
    await d.getByRole("button", { name: textRu("recovery.confirm") }).click();
    await expect(d.getByRole("alert")).toHaveText(textRu("error.vault.game_starting"));
    await d.getByRole("button", { name: textRu("recovery.keep") }).click();
    await d.getByRole("button", { name: textRu("common.cancel") }).click();
    await expect(dialog(page)).toHaveCount(0);

    // AccountDialog (inline layout) in Russian
    await backend.failNext("account_appearance", "vault_locked");
    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: textRu("account.manage", { name: ALEX.username }) })
      .click();
    await expect(d.getByRole("alert").first()).toHaveText(textRu("error.appearance.vault_locked"));

    await d.getByLabel(textRu("common.passphrase")).fill("парольная фраза");
    await backend.failNext("unlock_vault", "vault_unreadable");
    await d.getByRole("button", { name: textRu("common.unlock") }).click();
    await expect(d.locator("#appearance-unlock-error")).toHaveText(
      textRu("error.vault.vault_unreadable"),
    );

    await d.getByRole("button", { name: textRu("recovery.link") }).click();
    const recoveryGroup = d.getByRole("group", { name: textRu("recovery.title") });
    await expect(recoveryGroup).toBeVisible();
    await backend.failNext("reset_vault", "reset_failed");
    await d.getByRole("button", { name: textRu("recovery.confirm") }).click();
    await expect(recoveryGroup.getByRole("alert")).toHaveText(textRu("error.vault.reset_failed"));

    await d.getByRole("button", { name: textRu("recovery.confirm") }).click();
    await expect(d.getByRole("alert")).toHaveText(textRu("appearance.needsSignIn"));
    await expect(d.getByText(textRu("accountModal.statusNeedsSignIn"))).toBeVisible();
    await expect(d.getByRole("button", { name: textRu("common.signInAgain") })).toBeVisible();
  });
});
