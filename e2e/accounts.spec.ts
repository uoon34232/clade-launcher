import type { Locator, Page } from "@playwright/test";
import { ALEX, BLAKE } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, playButton, profileTrigger, text } from "./support/ui";

const NEW_ACCOUNT = {
  id: "acc-new",
  username: "FixtureNew",
  uuid: "00000000-0000-4000-8000-000000000003",
};

// Valid 64x64 solid-colour PNG skin texture matching the fake backend's synthetic skin format.
const SYNTHETIC_SKIN =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAZklEQVR42u3QQREAAAQAMFHFEFAvcjh7rMCiK+ezECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAu4HLPSxsmj7bLHgAAAAAElFTkSuQmCC";

interface E2eSignInControl {
  hasPendingSignIn: () => boolean;
  finishSignIn: (account?: { id?: string; username?: string; uuid?: string }) => boolean;
  failSignIn: (code: string, params?: Record<string, unknown>) => boolean;
  state: {
    accounts: {
      id: string;
      username: string;
      uuid: string;
      needsSignIn: boolean;
      skin: string | null;
    }[];
    active: string | null;
  };
  emit: (event: string, payload: unknown) => void;
}

const waitForPendingSignIn = async (page: Page) => {
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as unknown as { __e2e: E2eSignInControl }).__e2e.hasPendingSignIn(),
      ),
    )
    .toBe(true);
};

const finishSignIn = async (
  page: Page,
  account?: { id?: string; username?: string; uuid?: string },
) => {
  const ok = await page.evaluate(
    (acc) => (window as unknown as { __e2e: E2eSignInControl }).__e2e.finishSignIn(acc),
    account,
  );
  expect(ok).toBe(true);
};

const failSignIn = async (page: Page, code: string, params: Record<string, unknown> = {}) => {
  const ok = await page.evaluate(
    ([c, p]) => (window as unknown as { __e2e: E2eSignInControl }).__e2e.failSignIn(c, p),
    [code, params] as const,
  );
  expect(ok).toBe(true);
};

test.describe("with no account", () => {
  test.use({ backendConfig: { accounts: [], active: null } });

  test("the dock invites to add one, and the menu offers only that", async ({ page, openApp }) => {
    await openApp();
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    await expect(accountTrigger(page)).toContainText(text("account.signIn"));
    await accountTrigger(page).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio")).toHaveCount(0);
    await expect(menu.getByRole("menuitem")).toHaveCount(1);
    await expect(menu.getByRole("menuitem", { name: text("account.add") })).toBeVisible();
  });

  test("adding an account opens the sign-in dialog, which can be cancelled; no sign-in is started", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();
    const dialog = page.getByRole("dialog", { name: text("signin.createTitle") });
    await expect(dialog).toBeVisible();
    // the encrypted store has to exist before the browser is ever opened
    await expect(dialog.getByLabel(text("common.passphrase"), { exact: true })).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("common.continue") })).toBeDisabled();

    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
  });

  test("validates new passphrase length and confirmation, shows create_vault busy state, and completes sign-in", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.createTitle") });
    await expect(dialog).toBeVisible();
    const pass = dialog.getByLabel(text("common.passphrase"), { exact: true });
    const confirm = dialog.getByLabel(text("signin.repeat"));
    const submit = dialog.getByRole("button", { name: text("common.continue") });

    // Under 8 characters keeps Continue disabled even when both fields match.
    await pass.fill("short");
    await confirm.fill("short");
    await expect(submit).toBeDisabled();

    // Non-matching confirmation shows the mismatch note and keeps Continue disabled.
    await pass.fill("correct horse battery");
    await confirm.fill("different");
    await expect(dialog.getByText(text("signin.mismatch"))).toBeVisible();
    await expect(confirm).toHaveAttribute("aria-invalid", "true");
    await expect(submit).toBeDisabled();

    // Matching 8+ character passphrases enable Continue.
    await confirm.fill("correct horse battery");
    await expect(dialog.getByText(text("signin.mismatch"))).toHaveCount(0);
    await expect(submit).toBeEnabled();

    await backend.holdNext("create_vault");
    await submit.click();
    await expect(dialog.getByRole("button", { name: text("common.working") })).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("common.cancel") })).toBeDisabled();
    await expect(pass).toBeDisabled();
    await expect(confirm).toBeDisabled();

    await backend.release();
    await expect
      .poll(async () => (await backend.calls("create_vault")).map((c) => c.args))
      .toEqual([{ passphrase: "correct horse battery" }]);

    const working = page.getByRole("dialog", { name: text("signin.addTitle") });
    await expect(working).toBeVisible();
    await expect(working.getByText(text("signin.stage.opening"))).toBeVisible();
    await waitForPendingSignIn(page);
    await finishSignIn(page, NEW_ACCOUNT);

    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: NEW_ACCOUNT.username }),
    );
  });
});

test.describe("vault unlock and sign-in flow", () => {
  test.use({
    backendConfig: { accounts: [], active: null, vault: { exists: true, unlocked: false } },
  });

  test("handles unlock_vault failure, busy state, and sign-in-progress stage transitions", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.unlockTitle") });
    await expect(dialog).toBeVisible();
    const pass = dialog.getByLabel(text("common.passphrase"), { exact: true });
    const unlock = dialog.getByRole("button", { name: text("common.unlock") });
    await expect(unlock).toBeDisabled();

    await backend.failNext("unlock_vault", "passphrase_mismatch");
    await pass.fill("wrong passphrase");
    await unlock.click();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.vault.passphrase_mismatch"));
    await expect(pass).toHaveValue("");
    await expect(pass).toHaveAttribute("aria-invalid", "true");

    await backend.holdNext("unlock_vault");
    await pass.fill("correct horse battery");
    await unlock.click();
    await expect(dialog.getByRole("button", { name: text("common.working") })).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("common.cancel") })).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("recovery.link") })).toBeDisabled();
    await expect(pass).toBeDisabled();

    await backend.release();
    const working = page.getByRole("dialog", { name: text("signin.addTitle") });
    await expect(working).toBeVisible();
    await expect(working.getByText(text("signin.stage.opening"))).toBeVisible();
    await waitForPendingSignIn(page);

    await backend.emit("sign-in-progress", "waiting");
    await expect(working.getByText(text("signin.stage.waiting"))).toBeVisible();

    await backend.emit("sign-in-progress", "finishing");
    await expect(working.getByText(text("signin.stage.finishing"))).toBeVisible();

    await finishSignIn(page, NEW_ACCOUNT);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: NEW_ACCOUNT.username }),
    );
  });
});

test.describe("sign-in cancellation and errors", () => {
  test.use({
    backendConfig: { accounts: [ALEX], active: ALEX.id, vault: { exists: true, unlocked: true } },
  });

  test("cancelling a running sign-in calls cancel_sign_in and keeps accounts unchanged", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.addTitle") });
    await expect(dialog.getByText(text("signin.stage.opening"))).toBeVisible();
    await waitForPendingSignIn(page);

    await backend.emit("sign-in-progress", "waiting");
    await expect(dialog.getByText(text("signin.stage.waiting"))).toBeVisible();

    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("cancel_sign_in")).length).toBe(1);
    expect((await backend.state()).accounts).toHaveLength(1);
  });

  test("approval_required shows approval note without Try again", async ({ page, openApp }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.addTitle") });
    await waitForPendingSignIn(page);
    await failSignIn(page, "approval_required");

    await expect(dialog.getByRole("alert")).toHaveText(text("error.signIn.approval_required"));
    await expect(dialog.getByText(text("signin.approvalNote"))).toBeVisible();
    await expect(dialog.getByRole("button", { name: text("common.tryAgain") })).toHaveCount(0);

    const closeBtn = dialog.getByRole("button", { name: text("common.close") });
    await expect(closeBtn).toBeFocused();
    await closeBtn.click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("retryable sign-in error offers Try again and completes on retry", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.add") }).click();

    const dialog = page.getByRole("dialog", { name: text("signin.addTitle") });
    await waitForPendingSignIn(page);
    await failSignIn(page, "no_java_profile");

    await expect(dialog.getByRole("alert")).toHaveText(text("error.signIn.no_java_profile"));
    const tryAgain = dialog.getByRole("button", { name: text("common.tryAgain") });
    await expect(tryAgain).toBeVisible();
    await expect(tryAgain).toBeFocused();

    await tryAgain.click();
    await expect.poll(async () => (await backend.calls("begin_sign_in")).length).toBe(2);
    await expect(dialog.getByText(text("signin.stage.opening"))).toBeVisible();
    await waitForPendingSignIn(page);

    await finishSignIn(page, BLAKE);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: BLAKE.username }),
    );
  });
});

test.describe("with two accounts", () => {
  test("lists both, marks the active one, and switching changes only the account", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const profileBefore = await profileTrigger(page).getAttribute("aria-label");

    await accountTrigger(page).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio")).toHaveCount(2);
    await expect(menu.getByRole("menuitemradio", { name: ALEX.username })).toBeChecked();
    await expect(menu.getByRole("menuitemradio", { name: BLAKE.username })).not.toBeChecked();

    await menu.getByRole("menuitemradio", { name: BLAKE.username }).click();
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: BLAKE.username }),
    );
    await expect
      .poll(async () => (await backend.calls("select_account")).map((c) => c.args))
      .toEqual([{ id: BLAKE.id }]);

    // An account is never part of a profile: nothing about the profile changed, and no profile
    // command was sent.
    await expect(profileTrigger(page)).toHaveAttribute("aria-label", profileBefore ?? "");
    await expect(playButton(page)).toHaveText(text("play.play"));
    const changes = (await backend.calls())
      .map((c) => c.cmd)
      .filter((c) => /^(select_profile|create_profile|prepare_profile)$/.test(c));
    expect(changes).toEqual([]);
  });

  test("reverts optimistic account switch when select_account fails", async ({
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

    await accountTrigger(page).click();
    const menu = page.getByRole("menu");
    await expect(menu.getByRole("menuitemradio", { name: ALEX.username })).toBeChecked();
    await expect(menu.getByRole("menuitemradio", { name: BLAKE.username })).not.toBeChecked();
  });

  test("refreshes account avatars and sign-in status live on accounts-changed", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(accountTrigger(page).locator("[data-skin-head]")).toHaveCount(0);
    const listsBefore = (await backend.calls("list_accounts")).length;

    await page.evaluate((skin) => {
      const e2e = (window as unknown as { __e2e: E2eSignInControl }).__e2e;
      e2e.state.accounts = e2e.state.accounts.map((a, idx) =>
        idx === 0 ? { ...a, skin } : { ...a, needsSignIn: true },
      );
      e2e.emit("accounts-changed", null);
    }, SYNTHETIC_SKIN);

    await expect
      .poll(async () => (await backend.calls("list_accounts")).length)
      .toBeGreaterThan(listsBefore);
    await expect(accountTrigger(page).locator("[data-skin-head]")).toHaveCount(1);

    await accountTrigger(page).click();
    const blakeRow = page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) });
    await expect(blakeRow).toContainText(text("account.needsSignIn"));
  });
});

test.describe("with an account that needs to sign in again", () => {
  test.use({
    backendConfig: {
      accounts: [ALEX, { ...BLAKE, needsSignIn: true }],
      active: ALEX.id,
      vault: { exists: true, unlocked: true },
    },
  });

  test("the menu marks it", async ({ page, openApp }) => {
    await openApp();
    await accountTrigger(page).click();
    const row = page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) });
    await expect(row).toContainText(text("account.needsSignIn"));
  });

  test("Sign in again from AccountDialog transitions to SignInDialog for that account", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: text("account.manage", { name: BLAKE.username }) })
      .click();
    const accountDialog = page.getByRole("dialog", { name: BLAKE.username });
    await expect(accountDialog).toBeVisible();
    await expect(accountDialog.getByText(text("accountModal.statusNeedsSignIn"))).toBeVisible();

    await accountDialog.getByRole("button", { name: text("common.signInAgain") }).click();
    const signInDialog = page.getByRole("dialog", { name: text("signin.againTitle") });
    await expect(signInDialog).toBeVisible();
    await expect(
      signInDialog.getByText(text("signin.againDescription", { name: BLAKE.username })),
    ).toBeVisible();
    await expect
      .poll(async () => (await backend.calls("begin_sign_in")).map((c) => c.args))
      .toEqual([{ accountId: BLAKE.id }]);

    await waitForPendingSignIn(page);
    await finishSignIn(page);
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await accountTrigger(page).click();
    const row = page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) });
    await expect(row).not.toContainText(text("account.needsSignIn"));
  });
});

test.describe("account management", () => {
  const openManage = async (page: Page, name: string) => {
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: text("account.manage", { name }) }).click();
    const dialog = page.getByRole("dialog", { name });
    await expect(dialog).toBeVisible();
    return dialog;
  };

  test("shows the account's details, with the skin and cape controls ready", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await expect(dialog.getByText(text("accountModal.description"))).toBeVisible();
    await expect(dialog.getByText(BLAKE.uuid)).toBeVisible();
    await expect(dialog.getByText(text("accountModal.statusSignedIn"))).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: text("accountModal.choosePng") }),
    ).toBeEnabled();
    // Initial focus never lands on the destructive control.
    await expect(
      dialog.getByRole("button", { name: text("accountModal.signOutRemove") }),
    ).not.toBeFocused();
  });

  test("Done closes it and returns focus to the dock", async ({ page, openApp }) => {
    await openApp();
    const dialog = await openManage(page, ALEX.username);
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toBeFocused();
  });

  test("removal asks first, and can be cancelled without changing anything", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await expect(
      dialog.getByText(text("accountModal.confirmRemove", { name: BLAKE.username })),
    ).toBeVisible();

    await dialog.getByRole("button", { name: text("accountModal.keep") }).click();
    await expect(
      dialog.getByText(text("accountModal.confirmRemove", { name: BLAKE.username })),
    ).toHaveCount(0);
    await expect(
      dialog.getByRole("button", { name: text("accountModal.signOutRemove") }),
    ).toBeFocused();
    expect(await backend.calls("remove_account")).toHaveLength(0);
    expect((await backend.state()).accounts).toHaveLength(2);
  });

  // The dialog stays mounted for its exit animation, so "still visible" right after a click proves
  // nothing. A dialog that is closing loses `data-open` at once; wait two frames so the click has
  // been handled and rendered, then look at that state.
  const rendered = (page: Page) =>
    page.evaluate(
      () =>
        new Promise<void>((done) =>
          requestAnimationFrame(() => requestAnimationFrame(() => done())),
        ),
    );
  const staysOpen = async (page: Page, dialog: Locator) => {
    await rendered(page);
    await expect(dialog).toHaveAttribute("data-open", "");
    await expect(dialog).not.toHaveAttribute("data-closed", "");
    await expect(dialog.getByRole("button", { name: text("accountModal.remove") })).toBeVisible();
  };

  test("a stray click on the backdrop does not close the confirmation", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    // the click really lands on the backdrop, not on the window chrome
    expect(
      await page.evaluate(() => document.elementFromPoint(8, 8)?.getAttribute("data-slot")),
    ).toBe("dialog-overlay");
    await page.mouse.click(8, 8);
    await staysOpen(page, dialog);
  });

  test("Escape does not close the confirmation either; Keep gives the dialog its normal dismissal back", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await page.keyboard.press("Escape");
    await staysOpen(page, dialog);
    await dialog.getByRole("button", { name: text("accountModal.keep") }).click();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("shows a coded error alert and keeps the dialog open when remove_account fails", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await backend.failNext("remove_account", "unknown_account");
    await dialog.getByRole("button", { name: text("accountModal.remove") }).click();

    await expect(dialog.getByRole("alert")).toHaveText(text("error.account.unknown_account"));
    await staysOpen(page, dialog);
    await expect(dialog.getByRole("button", { name: text("accountModal.keep") })).toBeEnabled();
    expect((await backend.state()).accounts).toHaveLength(2);

    await dialog.getByRole("button", { name: text("accountModal.keep") }).click();
    await expect(dialog.getByRole("alert")).toHaveCount(0);
  });

  test("shows Removing… and disables Keep account while remove_account is in flight", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await backend.holdNext("remove_account");
    await dialog.getByRole("button", { name: text("accountModal.remove") }).click();

    await expect(
      dialog.getByRole("button", { name: text("accountModal.removing") }),
    ).toBeDisabled();
    await expect(dialog.getByRole("button", { name: text("accountModal.keep") })).toBeDisabled();
    await expect(
      dialog.getByRole("button", { name: text("accountModal.choosePng") }),
    ).toBeDisabled();

    await backend.release();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("remove_account")).map((c) => c.args))
      .toEqual([{ id: BLAKE.id }]);
  });

  test("removing the active account falls back to the remaining account", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, ALEX.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await dialog.getByRole("button", { name: text("accountModal.remove") }).click();

    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toHaveAccessibleName(
      text("account.trigger", { name: BLAKE.username }),
    );
    await expect(playButton(page)).toHaveText(text("play.play"));
    const state = await backend.state();
    expect(state.active).toBe(BLAKE.id);
    expect(state.accounts).toHaveLength(1);
  });

  test("confirming removes only the account; profiles and playtime stay as they are", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const profileBefore = await profileTrigger(page).getAttribute("aria-label");
    const dialog = await openManage(page, BLAKE.username);
    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await dialog.getByRole("button", { name: text("accountModal.remove") }).click();

    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("remove_account")).map((c) => c.args))
      .toEqual([{ id: BLAKE.id }]);
    await accountTrigger(page).click();
    await expect(page.getByRole("menuitemradio")).toHaveCount(1);
    await page.keyboard.press("Escape");
    await expect(profileTrigger(page)).toHaveAttribute("aria-label", profileBefore ?? "");
    await expect(page.getByText(text("hero.played", { time: "1h 50m" }))).toBeVisible();
  });
});

test.describe("removing the last remaining account", () => {
  test.use({ backendConfig: { accounts: [ALEX], active: ALEX.id } });

  test("reverts the dock trigger to Sign in and Play to Sign in to continue", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: text("account.manage", { name: ALEX.username }) })
      .click();
    const dialog = page.getByRole("dialog", { name: ALEX.username });
    await expect(dialog).toBeVisible();

    await dialog.getByRole("button", { name: text("accountModal.signOutRemove") }).click();
    await dialog.getByRole("button", { name: text("accountModal.remove") }).click();

    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(accountTrigger(page)).toHaveAccessibleName(text("account.triggerNone"));
    await expect(accountTrigger(page)).toContainText(text("account.signIn"));
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    const state = await backend.state();
    expect(state.accounts).toEqual([]);
    expect(state.active).toBeNull();
  });
});
