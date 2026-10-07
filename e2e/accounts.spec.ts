import type { Locator, Page } from "@playwright/test";
import { ALEX, BLAKE } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, playButton, profileTrigger, text } from "./support/ui";

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
});

test.describe("with an account that needs to sign in again", () => {
  test.use({
    backendConfig: { accounts: [ALEX, { ...BLAKE, needsSignIn: true }], active: ALEX.id },
  });

  test("the menu marks it", async ({ page, openApp }) => {
    await openApp();
    await accountTrigger(page).click();
    const row = page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) });
    await expect(row).toContainText(text("account.needsSignIn"));
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

  test("shows the account's details, with skin and cape changes unavailable", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openManage(page, BLAKE.username);
    await expect(dialog.getByText(text("accountModal.description"))).toBeVisible();
    await expect(dialog.getByText(BLAKE.uuid)).toBeVisible();
    await expect(dialog.getByText(text("accountModal.statusSignedIn"))).toBeVisible();
    await expect(dialog.getByText(text("accountModal.skinUnavailable"))).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: text("accountModal.choosePng") }),
    ).toBeDisabled();
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
