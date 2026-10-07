import { ALEX, BLAKE, FRESH, SURVIVAL } from "./support/fixtures";
import { expect, test } from "./support/test";
import { dock, playButton, profileTrigger, text } from "./support/ui";

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
    await expect(playButton(page)).toHaveText(text("play.signIn"));

    // Verify files is unavailable too.
    await profileTrigger(page).click();
    await expect(page.getByRole("menuitem", { name: text("profile.verify") })).toHaveAttribute(
      "data-disabled",
      /.*/,
    );
    await page.keyboard.press("Escape");

    // The primary action starts the normal sign-in (it asks for the passphrase first).
    await playButton(page).click();
    await expect(page.getByRole("dialog", { name: text("signin.createTitle") })).toBeVisible();
    await page.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("launch_profile")).toHaveLength(0);
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
  });

  test("the backend itself refuses a direct prepare or verify request", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const code = (verify: boolean) =>
      page.evaluate(async (v) => {
        const internals = (
          window as unknown as {
            __TAURI_INTERNALS__: {
              invoke: (c: string, a: unknown) => Promise<unknown>;
            };
          }
        ).__TAURI_INTERNALS__;
        try {
          await internals.invoke("prepare_profile", { id: "fresh", verify: v });
          return "accepted";
        } catch (e) {
          return (e as { code?: string }).code ?? "unknown";
        }
      }, verify);
    expect(await code(false)).toBe("account_required");
    expect(await code(true)).toBe("account_required");
    expect(await backend.state()).toBeTruthy();
  });
});

test.describe("an account that must sign in again", () => {
  test.use({
    backendConfig: { accounts: [{ ...ALEX, needsSignIn: true }, BLAKE], active: ALEX.id },
  });

  test("keeps the profiles, gates Play and Verify files, and re-signs the same account in", async ({
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
    await expect(page.getByRole("menuitemradio")).toHaveCount(3); // nothing was removed
    await expect(page.getByRole("menuitem", { name: text("profile.verify") })).toHaveAttribute(
      "data-disabled",
      /.*/,
    );
    await page.keyboard.press("Escape");

    await playButton(page).click();
    // The normal sign-in opens (the vault is created or unlocked first); nothing is downloaded.
    await expect(page.getByRole("dialog")).toBeVisible();
    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("launch_profile")).toHaveLength(0);
  });

  test("a ready account next to it is never used instead; choosing it unlocks Play", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.signIn"));
    await page
      .getByRole("button", { name: text("account.trigger", { name: ALEX.username }) })
      .click();
    await page.getByRole("menuitemradio", { name: new RegExp(BLAKE.username) }).click();
    await expect(playButton(page)).toHaveText(text("play.play"));
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
