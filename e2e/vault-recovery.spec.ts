import type { Locator, Page } from "@playwright/test";
import { ALEX, DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, playButton, text } from "./support/ui";

// "Forgot passphrase?" and "Start fresh" in the three places that ask for the vault passphrase, against
// the fake backend: nothing here can touch a real vault, account or file.

const LOCKED = { vault: { exists: true, unlocked: false } };

const dialog = (page: Page) => page.getByRole("dialog");
const forgot = (d: Locator) => d.getByRole("button", { name: text("recovery.link") });
const startFresh = (d: Locator) => d.getByRole("button", { name: text("recovery.confirm") });
const keepTrying = (d: Locator) => d.getByRole("button", { name: text("recovery.keep") });

/** A closing dialog loses `data-open` at once; wait two frames so a click or key has been handled. */
const stillOpen = async (page: Page, d: Locator) => {
  await page.evaluate(
    () =>
      new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  );
  await expect(d).toHaveAttribute("data-open", "");
  await expect(d).not.toHaveAttribute("data-closed", "");
};

const openAddAccount = async (page: Page) => {
  await accountTrigger(page).click();
  await page.getByRole("menuitem", { name: text("account.add") }).click();
  await expect(dialog(page)).toBeVisible();
};

test.describe("in the sign-in dialog", () => {
  test.use({ backendConfig: LOCKED });

  test("a locked vault: Start fresh archives it and continues to the new-passphrase step", async ({
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

    // Escape and the backdrop leave the question alone
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await page.mouse.click(8, 8);
    await stillOpen(page, d);
    await expect(d.getByRole("heading", { name: text("recovery.title") })).toBeVisible();

    // Cancel changes nothing, and the passphrase form is back
    await keepTrying(d).click();
    await expect(d.getByLabel(text("common.passphrase"))).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(0);
    expect((await backend.state()).vault).toEqual({ exists: true, unlocked: false });

    // the explicit answer
    await forgot(d).click();
    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
    await expect(d.getByLabel(text("signin.repeat"))).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);
    const state = await backend.state();
    expect(state.vault).toEqual({ exists: false, unlocked: false });
    // no new vault was made, and the accounts are kept but must sign in again
    expect(await backend.calls("create_vault")).toHaveLength(0);
    expect(state.accounts.length).toBe(2);
    expect(state.accounts.every((a) => a.needsSignIn)).toBe(true);
    await expect(page.getByRole("alert")).toHaveCount(0);
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
    // and it can be answered again
    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
  });

  test("a vault file that is missing offers the same way out", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await openAddAccount(page);
    const d = dialog(page);
    await d.getByLabel(text("common.passphrase")).fill("fake test passphrase");
    await backend.failNext("unlock_vault", "vault_missing");
    await d.getByRole("button", { name: text("common.unlock") }).click();
    await expect(d.getByRole("alert")).toHaveText(text("error.vault.vault_missing"));
    await forgot(d).click();
    await startFresh(d).click();
    await expect(d.getByRole("heading", { name: text("signin.createTitle") })).toBeVisible();
  });
});

test.describe("in the launch dialog", () => {
  test.use({ backendConfig: LOCKED });

  test("unlock to play: Start fresh turns it into Sign in again", async ({
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
    expect(await backend.calls("reset_vault")).toHaveLength(0);

    await forgot(d).click();
    await startFresh(d).click();
    await expect(
      d.getByRole("heading", { name: text("problem.title.needs_sign_in") }),
    ).toBeVisible();
    const again = d.getByRole("button", { name: text("common.signInAgain") });
    await expect(again).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);

    // from there the sign-in starts at the new-passphrase step
    await again.click();
    await expect(
      dialog(page).getByRole("heading", { name: text("signin.createTitle") }),
    ).toBeVisible();
  });
});

test.describe("in the account dialog", () => {
  test.use({
    backendConfig: { ...LOCKED, appearance: { [ALEX.id]: { skinModel: "classic", capes: [] } } },
  });

  test("the appearance unlock offers Start fresh, and the account then needs to sign in again", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.failNext("account_appearance", "vault_locked");
    await accountTrigger(page).click();
    await page
      .getByRole("menuitem", { name: text("account.manage", { name: ALEX.username }) })
      .click();
    const d = dialog(page);
    await expect(d.getByRole("alert").first()).toHaveText(text("error.appearance.vault_locked"));

    await forgot(d).click();
    await expect(d.getByRole("group", { name: text("recovery.title") })).toBeVisible();
    // the dialog around it holds still
    await page.keyboard.press("Escape");
    await stillOpen(page, d);
    await page.mouse.click(8, 8);
    await stillOpen(page, d);
    await keepTrying(d).click();
    await expect(d.getByLabel(text("common.passphrase"))).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(0);

    await forgot(d).click();
    await startFresh(d).click();
    // the existing signed-out state: the explanation, no controls, and the way back in
    await expect(d.getByRole("alert")).toHaveText(text("appearance.needsSignIn"));
    await expect(d.getByRole("button", { name: text("accountModal.choosePng") })).toBeDisabled();
    await expect(d.getByRole("button", { name: text("common.signInAgain") })).toBeVisible();
    await expect(d.getByText(text("accountModal.statusNeedsSignIn"))).toBeVisible();
    // Escape works again once the question is gone
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toHaveCount(0);
  });
});

test.describe("in Russian", () => {
  test.use({
    backendConfig: { ...LOCKED, ui: { ...DEFAULT_CONFIG.ui, language: "ru" } },
  });

  test("the whole flow reads in Russian", async ({ page, backend, openApp }) => {
    await openApp();
    await accountTrigger(page).click();
    await page.getByRole("menuitem", { name: /Добавить аккаунт/ }).click();
    const d = dialog(page);
    await d.getByRole("button", { name: "Забыл парольную фразу?" }).click();
    await expect(d.getByRole("heading", { name: "Начать заново?" })).toBeVisible();
    await expect(d.getByText(/Профили, миры, время игры и настройки сохранятся\./)).toBeVisible();
    await expect(d.getByRole("button", { name: "Продолжить попытки" })).toBeFocused();
    await d.getByRole("button", { name: "Начать заново" }).click();
    await expect(d.getByRole("heading", { name: "Защита аккаунтов" })).toBeVisible();
    expect(await backend.calls("reset_vault")).toHaveLength(1);
  });
});
