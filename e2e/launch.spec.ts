import { expect, test } from "./support/test";
import { playButton, profileTrigger, text } from "./support/ui";

// These tests cover the launch UI lifecycle against the fake backend. The real process handling,
// the command line and the pre-launch checks are covered by the Rust tests.

test.describe("launching", () => {
  test("Play -> Launching -> Playing -> Play, and a clean exit shows nothing", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.play"));

    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));
    await expect(playButton(page)).toBeDisabled();
    expect((await backend.calls("launch_profile")).map((c) => c.args)).toEqual([
      { id: "survival" },
    ]);

    await backend.playing("survival");
    await expect(playButton(page)).toHaveText(text("play.playing"));
    await expect(playButton(page)).toBeDisabled();

    const listsBefore = (await backend.calls("list_profiles")).length;
    await backend.stopped("survival", { kind: "normal", reason: "normal", code: 0 });
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // playtime moved while the game ran, so the profiles are read again
    await expect
      .poll(async () => (await backend.calls("list_profiles")).length)
      .toBeGreaterThan(listsBefore);
  });

  test("a double click starts the game only once", async ({ page, backend, openApp }) => {
    await openApp();
    await playButton(page).dblclick();
    await expect(playButton(page)).toHaveText(text("play.launching"));
    expect(await backend.calls("launch_profile")).toHaveLength(1);
    // and while it runs there is nothing to press again
    await backend.playing("survival");
    await expect(playButton(page)).toBeDisabled();
    expect(await backend.calls("launch_profile")).toHaveLength(1);
  });

  test("the backend saying it is already running is shown as Playing, not as an error", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("already_running");
    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.playing"));
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });
});

test.describe("a window opened while the game is already running", () => {
  test.use({ backendConfig: { runs: { survival: "playing" } } });

  test("shows Playing and offers nothing to press", async ({ page, backend, openApp }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.playing"));
    await expect(playButton(page)).toBeDisabled();
    expect(await backend.calls("launch_profile")).toHaveLength(0);
    // and when that game ends, the button comes back
    await backend.stopped("survival", { kind: "normal", reason: "normal", code: 0 });
    await expect(playButton(page)).toHaveText(text("play.play"));
  });
});

test.describe("when the game ends badly", () => {
  const cases = [
    { kind: "crashed", reason: "out_of_memory", message: "exit.out_of_memory" },
    { kind: "crashed", reason: "wrong_java", message: "exit.wrong_java" },
    { kind: "crashed", reason: "mod_loader", message: "exit.mod_loader" },
    { kind: "failed_to_start", reason: "jvm_start", message: "exit.jvm_start" },
  ] as const;

  for (const c of cases) {
    test(`${c.kind} (${c.reason}) shows one short sentence, Open logs and Close`, async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await playButton(page).click();
      await backend.playing("survival");
      await backend.stopped("survival", { kind: c.kind, reason: c.reason, code: 1 });

      const dialog = page.getByRole("dialog", { name: text("problem.endedTitle") });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("alert")).toHaveText(text(c.message));
      // the button is back, so the person can try again
      await dialog.getByRole("button", { name: text("common.openLogs") }).click();
      expect((await backend.calls("open_game_logs")).map((x) => x.args)).toEqual([
        { id: "survival" },
      ]);

      await dialog.getByRole("button", { name: text("common.close") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(playButton(page)).toHaveText(text("play.play"));
      await expect(playButton(page)).toBeEnabled();
    });
  }

  test("an unknown exit shows the generic sentence with the exit code", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).click();
    await backend.playing("survival");
    await backend.stopped("survival", { kind: "crashed", reason: "unexpected", code: 7 });
    await expect(page.getByRole("dialog").getByRole("alert")).toHaveText(
      text("exit.unexpected", { code: 7 }),
    );
  });
});

test.describe("when the launch is refused", () => {
  test("no account: explains and offers to add one, without starting anything", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("no_account");
    await playButton(page).click();

    const dialog = page.getByRole("dialog", { name: text("problem.title.no_account") });
    await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.no_account"));
    await dialog.getByRole("button", { name: text("problem.addAccount") }).click();
    const signIn = page.getByRole("dialog", { name: text("signin.createTitle") });
    await expect(signIn).toBeVisible();
    await signIn.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // nothing was started, and the button is as it was
    await expect(playButton(page)).toHaveText(text("play.play"));
    expect(await backend.calls("begin_sign_in")).toHaveLength(0);
  });

  test("sign-in not approved yet: says so plainly", async ({ page, backend, openApp }) => {
    await openApp();
    await backend.refuseNextLaunch("approval_pending");
    await playButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("problem.title.approval_pending") });
    await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.approval_pending"));
    await dialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test("locked vault: asks for the passphrase, which is never kept in the page", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("vault_locked");
    await playButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("problem.unlockTitle") });
    await expect(dialog).toBeVisible();
    const pass = dialog.getByLabel(text("common.passphrase"));
    await expect(pass).toHaveAttribute("type", "password");
    await pass.fill("typed-in-a-test");
    await expect(dialog.getByRole("button", { name: text("common.unlock") })).toBeEnabled();
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // cancelling never reached the backend with the passphrase
    expect(await backend.calls("unlock_vault")).toHaveLength(0);
  });

  test("offline: Try again launches again", async ({ page, backend, openApp }) => {
    await openApp();
    await backend.refuseNextLaunch("offline");
    await playButton(page).click();
    const dialog = page.getByRole("dialog", { name: text("problem.title.offline") });
    await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.offline"));
    await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));
    expect(await backend.calls("launch_profile")).toHaveLength(2);
  });
});

test.describe("a profile's files and its game exclude each other", () => {
  test("Verify files is unavailable while the game starts or runs, and comes back afterwards", async ({
    page,
    backend,
    openApp,
  }) => {
    const verify = () => page.getByRole("menuitem", { name: text("profile.verify") });
    await openApp();
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeEnabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await backend.playing("survival");
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await backend.stopped("survival", { kind: "normal", reason: "normal", code: 0 });
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeEnabled();
    expect(await backend.calls("prepare_profile")).toHaveLength(0);
  });

  test("a launch refused because the profile is being set up says so in words", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("already_preparing");
    await playButton(page).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.already_preparing"));
    await dialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveText(text("play.play"));
  });
});
