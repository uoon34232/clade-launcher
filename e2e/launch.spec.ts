import { BLAKE, CREATIVE, DEFAULT_CONFIG, SURVIVAL } from "./support/fixtures";
import { expect, test } from "./support/test";
import { dock, playButton, profileTrigger, settingsButton, text } from "./support/ui";

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
    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
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
    // Seed an active backend session without emitting launch-state so clicking Play hits
    // `already_running` and re-syncs via `launch_status`.
    await page.evaluate(() => {
      const w = window as unknown as {
        __e2e: { state: { runs: Record<string, "launching" | "playing"> } };
      };
      w.__e2e.state.runs.survival = "playing";
    });
    const statusCallsBefore = (await backend.calls("launch_status")).length;
    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.playing"));
    await expect(playButton(page)).toBeDisabled();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("launch_status")).length)
      .toBeGreaterThan(statusCallsBefore);
  });

  test("a stopped event with exit: null resets to Play without a dialog and refreshes snapshots", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const profilesBefore = (await backend.calls("list_profiles")).length;
    const accountsBefore = (await backend.calls("list_accounts")).length;

    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));

    await backend.stopped("survival", null);
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("list_profiles")).length)
      .toBeGreaterThan(profilesBefore);
    await expect
      .poll(async () => (await backend.calls("list_accounts")).length)
      .toBeGreaterThan(accountsBefore);
  });
});

test.describe("a window opened while the game is already running", () => {
  test.use({
    backendConfig: {
      runs: { survival: "playing" },
      ui: { ...DEFAULT_CONFIG.ui, onLaunch: "keep_open" },
    },
  });

  test("shows Playing and offers nothing to press", async ({ page, backend, openApp }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.playing"));
    await expect(playButton(page)).toBeDisabled();
    expect(await backend.calls("launch_profile")).toHaveLength(0);
    // and when that game ends, the button comes back
    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
  });
});

test.describe("a window reopened while a framed game is already embedded", () => {
  test.use({
    backendConfig: {
      runs: { survival: "playing" },
      embed: {
        active: true,
        watching: false,
        instanceId: "survival",
        maximized: false,
        closing: false,
      },
    },
  });

  test("reattaches straight into the framed stage and restores the menu on exit", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveCount(0);
    await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "false");
    await expect(page.getByRole("button", { name: text("window.minimize") })).toBeVisible();
    await expect(page.getByRole("button", { name: text("window.maximize") })).toBeVisible();
    await expect(page.getByRole("button", { name: text("window.close") })).toBeVisible();
    expect(await backend.calls("launch_profile")).toHaveLength(0);

    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
    await expect(page.getByRole("button", { name: text("window.maximize") })).toHaveCount(0);
  });
});

test.describe("when the game ends badly", () => {
  const cases = [
    { kind: "crashed", reason: "out_of_memory", message: text("exit.out_of_memory") },
    { kind: "crashed", reason: "wrong_java", message: text("exit.wrong_java") },
    { kind: "crashed", reason: "mod_loader", message: text("exit.mod_loader") },
    { kind: "failed_to_start", reason: "jvm_start", message: text("exit.jvm_start") },
    {
      kind: "failed_to_start",
      reason: "early_exit",
      code: 255,
      message: text("exit.early_exit", { code: 255 }),
    },
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
      await backend.stopped("survival", {
        kind: c.kind,
        reason: c.reason,
        code: "code" in c ? c.code : 1,
      });

      const dialog = page.getByRole("dialog", { name: text("problem.endedTitle") });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("alert")).toHaveText(c.message);
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

  test("an unrecognized exit reason falls back to exit.generic", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).click();
    await backend.playing("survival");
    await backend.stopped("survival", {
      kind: "crashed",
      reason: "unrecognized_reason",
      code: 42,
    });
    await expect(page.getByRole("dialog").getByRole("alert")).toHaveText(text("exit.generic"));
  });

  test("an exit with code: null (stopped) shows its plain sentence", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).click();
    await backend.playing("survival");
    await backend.stopped("survival", { kind: "crashed", reason: "stopped", code: null });
    await expect(page.getByRole("dialog").getByRole("alert")).toHaveText(text("exit.stopped"));
  });

  test.describe("while a modal surface is open", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, onLaunch: "keep_open" } } });

    test("a crash closes the open modal and ignores failing open_game_logs silently", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await playButton(page).click();
      await backend.playing("survival");

      await settingsButton(page).click();
      await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();

      await backend.stopped("survival", { kind: "crashed", reason: "out_of_memory", code: 1 });
      await expect(page.getByRole("dialog", { name: text("settings.title") })).toHaveCount(0);

      const dialog = page.getByRole("dialog", { name: text("problem.endedTitle") });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("alert")).toHaveText(text("exit.out_of_memory"));

      await backend.failNext("open_game_logs", "storage");
      await dialog.getByRole("button", { name: text("common.openLogs") }).click();
      await expect.poll(async () => (await backend.calls("open_game_logs")).length).toBe(1);
      await expect(dialog).toBeVisible();

      await dialog.getByRole("button", { name: text("common.close") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });
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

  test.describe("needs_sign_in with a targeted account", () => {
    test.use({ backendConfig: { vault: { exists: true, unlocked: true } } });

    test("Sign in again hands off the refused accountId to SignInDialog", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await backend.refuseNextLaunch("needs_sign_in", BLAKE.id);
      await playButton(page).click();

      const dialog = page.getByRole("dialog", { name: text("problem.title.needs_sign_in") });
      await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.needs_sign_in"));
      await expect(dialog.getByRole("button", { name: text("common.close") })).toBeVisible();
      await dialog.getByRole("button", { name: text("common.signInAgain") }).click();

      const signIn = page.getByRole("dialog", { name: text("signin.againTitle") });
      await expect(signIn).toBeVisible();
      await expect
        .poll(async () => (await backend.calls("begin_sign_in")).map((c) => c.args))
        .toEqual([{ accountId: BLAKE.id }]);

      await signIn.getByRole("button", { name: text("common.cancel") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });
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

  test("other refusal codes use problem.title.other with a single Close action", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await backend.refuseNextLaunch("spawn_failed");
    await playButton(page).click();

    const dialog = page.getByRole("dialog", { name: text("problem.title.other") });
    await expect(dialog.getByRole("alert")).toHaveText(text("error.launch.spawn_failed"));
    await expect(dialog.getByRole("button")).toHaveCount(1);
    await dialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await backend.refuseNextLaunch("storage");
    await playButton(page).click();
    const storageDialog = page.getByRole("dialog", { name: text("problem.title.other") });
    await expect(storageDialog.getByRole("alert")).toHaveText(text("error.common.storage"));
    await storageDialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await backend.refuseNextLaunch("no_java_profile");
    await playButton(page).click();
    const profileDialog = page.getByRole("dialog", { name: text("problem.title.other") });
    await expect(profileDialog.getByRole("alert")).toHaveText(text("error.launch.no_java_profile"));
    await profileDialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);

    await backend.refuseNextLaunch("unrecognized_launch_code");
    await playButton(page).click();
    const unknownDialog = page.getByRole("dialog", { name: text("problem.title.other") });
    await expect(unknownDialog.getByRole("alert")).toHaveText(text("error.launch.unknown"));
    await unknownDialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test.describe("locked vault", () => {
    test.use({ backendConfig: { vault: { exists: true, unlocked: false } } });

    test("asks for the passphrase, clears it on reopen, rejects wrong passphrases, and retries on unlock", async ({
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
      const unlock = dialog.getByRole("button", { name: text("common.unlock") });
      await expect(pass).toHaveAttribute("type", "password");
      await expect(unlock).toBeDisabled();

      await pass.fill("typed-in-a-test");
      await expect(unlock).toBeEnabled();
      await dialog.getByRole("button", { name: text("common.cancel") }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      // cancelling never reached the backend with the passphrase
      expect(await backend.calls("unlock_vault")).toHaveLength(0);

      // Reopening starts with an empty passphrase input rather than retaining cancelled text.
      await backend.refuseNextLaunch("vault_locked");
      await playButton(page).click();
      await expect(dialog).toBeVisible();
      await expect(pass).toHaveValue("");
      await expect(unlock).toBeDisabled();

      // Wrong passphrase clears the input and shows the vault error without retrying launch.
      await backend.failNext("unlock_vault", "passphrase_mismatch");
      await pass.fill("wrong-passphrase");
      await unlock.click();
      await expect(dialog.getByRole("alert")).toHaveText(text("error.vault.passphrase_mismatch"));
      await expect(pass).toHaveValue("");
      await expect(unlock).toBeDisabled();
      expect(await backend.calls("unlock_vault")).toHaveLength(1);
      expect(await backend.calls("launch_profile")).toHaveLength(2);

      // Correct passphrase unlocks the vault and automatically retries launch_profile.
      await pass.fill("right-passphrase");
      await unlock.click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(playButton(page)).toHaveText(text("play.launching"));
      expect((await backend.state()).vault.unlocked).toBe(true);
      expect(await backend.calls("unlock_vault")).toHaveLength(2);
      expect(await backend.calls("launch_profile")).toHaveLength(3);
    });
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
  test("Verify files and Remove are unavailable while the game starts or runs, and come back afterwards", async ({
    page,
    backend,
    openApp,
  }) => {
    const verify = () => page.getByRole("menuitem", { name: text("profile.verify") });
    const remove = () => page.getByRole("menuitem", { name: text("profile.remove") });
    await openApp();
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeEnabled();
    await expect(remove()).toBeEnabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeDisabled();
    await expect(remove()).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await backend.playing("survival");
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeDisabled();
    await expect(remove()).toBeDisabled();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);

    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await profileTrigger(page).click();
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(verify()).toBeEnabled();
    await expect(remove()).toBeEnabled();
    expect(await backend.calls("prepare_profile")).toHaveLength(0);
    expect(await backend.calls("remove_profile")).toHaveLength(0);
  });

  test("Profile settings disables editing actions while the profile is starting or running", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).click();
    await expect(playButton(page)).toHaveText(text("play.launching"));

    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.settings") }).click();
    const dialog = page.getByRole("dialog", { name: SURVIVAL.name });
    await expect(dialog).toBeVisible();

    const expectLocked = async (locked: boolean) => {
      const nameInput = dialog.getByLabel(text("create.name"));
      const changeVersion = dialog.getByRole("button", {
        name: text("profileModal.changeVersion"),
      });
      const verifyBtn = dialog.getByRole("button", { name: text("profile.verify") });
      const removeBtn = dialog.getByRole("button", { name: text("profile.remove") });
      const duplicateBtn = dialog.getByRole("button", { name: text("profileModal.duplicate") });
      if (locked) {
        await expect(nameInput).toBeDisabled();
        await expect(changeVersion).toBeDisabled();
        await expect(verifyBtn).toBeDisabled();
        await expect(removeBtn).toBeDisabled();
        await expect(duplicateBtn).toBeDisabled();
      } else {
        await expect(nameInput).toBeEnabled();
        await expect(changeVersion).toBeEnabled();
        await expect(verifyBtn).toBeEnabled();
        await expect(removeBtn).toBeEnabled();
        await expect(duplicateBtn).toBeEnabled();
      }
      await expect(dialog.getByRole("button", { name: text("profile.openFolder") })).toBeEnabled();
      await expect(dialog.getByRole("button", { name: text("common.done") })).toBeEnabled();
    };

    await expectLocked(true);
    await backend.playing("survival");
    await expectLocked(true);
    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await expectLocked(false);

    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  });

  test.describe("multi-profile run isolation", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, onLaunch: "keep_open" } } });

    test("switching to an idle profile keeps Play and menu actions enabled while another profile runs", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await playButton(page).click();
      await backend.playing("survival");
      await expect(dock(page)).toBeVisible();
      await expect(playButton(page)).toHaveText(text("play.playing"));
      await expect(playButton(page)).toBeDisabled();

      await profileTrigger(page).click();
      await page.getByRole("menuitemradio", { name: new RegExp(`^${CREATIVE.name}`) }).click();

      await expect(playButton(page)).toHaveText(text("play.play"));
      await expect(playButton(page)).toBeEnabled();
      await profileTrigger(page).click();
      await expect(page.getByRole("menuitem", { name: text("profile.verify") })).toBeEnabled();
      await expect(page.getByRole("menuitem", { name: text("profile.remove") })).toBeEnabled();
      await page.keyboard.press("Escape");

      await playButton(page).click();
      await expect(playButton(page)).toHaveText(text("play.launching"));
      expect((await backend.state()).runs).toEqual({ survival: "playing", creative: "launching" });

      await profileTrigger(page).click();
      await page.getByRole("menuitemradio", { name: new RegExp(`^${SURVIVAL.name}`) }).click();
      await expect(playButton(page)).toHaveText(text("play.playing"));
      await expect(playButton(page)).toBeDisabled();
    });
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

test.describe("non-embed launch modes", () => {
  test.describe("keep_open", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, onLaunch: "keep_open" } } });

    test("never mounts the embed launch veil and keeps the dock intact", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await playButton(page).click();
      await expect(playButton(page)).toHaveText(text("play.launching"));
      await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
      await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "true");

      await backend.playing("survival");
      await expect(dock(page)).toBeVisible();
      await expect(playButton(page)).toHaveText(text("play.playing"));
      await expect(playButton(page)).toBeDisabled();
      await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
      await expect(page.getByRole("button", { name: text("window.maximize") })).toHaveCount(0);

      await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
      await expect(playButton(page)).toHaveText(text("play.play"));
      await expect(playButton(page)).toBeEnabled();
    });
  });

  test.describe("hide", () => {
    test.use({ backendConfig: { ui: { ...DEFAULT_CONFIG.ui, onLaunch: "hide" } } });

    test("never mounts the embed launch veil or framed window controls", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await playButton(page).click();
      await expect(playButton(page)).toHaveText(text("play.launching"));
      await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
      await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "true");

      await backend.playing("survival");
      await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
      await expect(page.getByRole("button", { name: text("window.maximize") })).toHaveCount(0);
    });
  });
});

test.describe("framed game mode (inside Clade)", () => {
  test("hides the main menu, shows the framed stage with Maximize/Restore, and restores on exit", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.clock.install();
    await openApp();
    await expect(playButton(page)).toBeVisible();
    await expect(page.getByRole("button", { name: text("window.maximize") })).toHaveCount(0);
    await expect(page.getByRole("button", { name: text("window.restore") })).toHaveCount(0);
    await expect
      .poll(async () => (await backend.calls("set_game_stage_bounds")).length)
      .toBeGreaterThan(0);
    expect((await backend.calls("set_game_stage_bounds")).at(-1)?.args).toEqual({
      x: 0,
      y: 44,
      width: 1280,
      height: 676,
      radius: 0,
    });

    const boundsCountBeforeResize = (await backend.calls("set_game_stage_bounds")).length;
    await page.setViewportSize({ width: 1024, height: 640 });
    await expect
      .poll(async () => (await backend.calls("set_game_stage_bounds")).length)
      .toBeGreaterThan(boundsCountBeforeResize);
    expect((await backend.calls("set_game_stage_bounds")).at(-1)?.args).toEqual({
      x: 0,
      y: 44,
      width: 1024,
      height: 596,
      radius: 0,
    });

    // Double-clicking the header strip while unframed must not trigger game maximize.
    await page.mouse.dblclick(120, 20);
    expect(await backend.calls("toggle_game_maximize")).toHaveLength(0);

    await playButton(page).click();
    await expect(page.getByTestId("game-stage-launching")).toBeVisible();
    await backend.playing("survival");
    // Launch transition stays active after the 2.5s "playing" milestone until the game window embeds.
    await expect(page.getByTestId("game-stage-launching")).toBeVisible();
    await backend.embedded("survival", true);

    await expect(playButton(page)).toHaveCount(0);
    await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
    await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "false");
    const minimizeBtn = page.getByRole("button", { name: text("window.minimize") });
    const closeBtn = page.getByRole("button", { name: text("window.close") });
    await expect(minimizeBtn).toBeVisible();
    await expect(page.getByRole("button", { name: text("window.maximize") })).toBeVisible();
    await expect(closeBtn).toBeVisible();

    await page.getByRole("button", { name: text("window.maximize") }).click();
    await expect.poll(async () => (await backend.calls("toggle_game_maximize")).length).toBe(1);
    await expect(page.getByRole("button", { name: text("window.restore") })).toBeVisible();

    await page.getByRole("button", { name: text("window.restore") }).click();
    await expect.poll(async () => (await backend.calls("toggle_game_maximize")).length).toBe(2);
    await expect(page.getByRole("button", { name: text("window.maximize") })).toBeVisible();

    // Double-clicking the empty drag strip while framed toggles maximize.
    await page.mouse.dblclick(120, 20);
    await expect.poll(async () => (await backend.calls("toggle_game_maximize")).length).toBe(3);
    await expect(page.getByRole("button", { name: text("window.restore") })).toBeVisible();

    // Closing pulse reflects `embed.closing`.
    await backend.embedded("survival", true, true, true);
    await expect(closeBtn).toHaveClass(/chrome-closing-pulse/);
    await backend.embedded("survival", true, true, false);
    await expect(closeBtn).not.toHaveClass(/chrome-closing-pulse/);

    // Window minimize/close buttons invoke Tauri window commands without errors.
    await page.evaluate(() => {
      const w = window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
        };
        __windowCalls?: string[];
      };
      const recorded: string[] = [];
      w.__windowCalls = recorded;
      const orig = w.__TAURI_INTERNALS__.invoke;
      w.__TAURI_INTERNALS__.invoke = (cmd: string, args?: Record<string, unknown>) => {
        if (cmd.startsWith("plugin:window|")) recorded.push(cmd);
        return orig(cmd, args);
      };
    });
    await minimizeBtn.click();
    await closeBtn.click();
    await expect
      .poll(() =>
        page.evaluate(
          () => (window as unknown as { __windowCalls?: string[] }).__windowCalls ?? [],
        ),
      )
      .toEqual(["plugin:window|minimize", "plugin:window|close"]);

    // Freeze timers before exiting so the 560ms fold-out transition layer is asserted without races.
    const now = await page.evaluate(() => Date.now());
    await page.clock.pauseAt(now + 1000);
    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await page.clock.runFor(50);

    await expect(playButton(page)).toHaveText(text("play.play"));
    expect(await page.getByTestId("game-stage-exit").count()).toBe(1);

    await page.clock.runFor(450);
    expect(await page.getByTestId("game-stage-exit").count()).toBe(1);

    await page.clock.runFor(150);
    expect(await page.getByTestId("game-stage-exit").count()).toBe(0);
    await page.clock.resume();
    await expect(page.getByTestId("game-stage-exit")).toHaveCount(0);
    await expect(page.getByRole("button", { name: text("window.maximize") })).toHaveCount(0);
    await expect(page.getByRole("button", { name: text("window.restore") })).toHaveCount(0);
    await expect(playButton(page)).toBeEnabled();
  });

  test("entering framed mode closes an open modal surface", async ({ page, backend, openApp }) => {
    await openApp();
    await playButton(page).click();
    await settingsButton(page).click();
    await expect(page.getByRole("dialog", { name: text("settings.title") })).toBeVisible();

    await backend.embedded("survival", true);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "false");
  });

  test("a pre-embed failure clears the launch veil and reports the exit without mounting game-stage-exit", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await playButton(page).click();
    await expect(page.getByTestId("game-stage-launching")).toBeVisible();

    await backend.stopped("survival", { kind: "failed_to_start", reason: "early_exit", code: 1 });
    await expect(page.getByTestId("game-stage-launching")).toHaveCount(0);
    await expect(page.getByTestId("game-stage-exit")).toHaveCount(0);
    await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "true");
    const dialog = page.getByRole("dialog", { name: text("problem.endedTitle") });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveText(text("exit.early_exit", { code: 1 }));
    expect((await backend.state()).embed).toEqual({
      active: false,
      watching: false,
      instanceId: null,
      maximized: false,
      closing: false,
    });
  });
});

test.describe("framed game mode with reduced motion", () => {
  test.use({
    backendConfig: {
      ui: { ...DEFAULT_CONFIG.ui, reducedMotion: "on" },
    },
  });

  test("skips the close fold-out transition layer when reduced motion is enabled", async ({
    page,
    backend,
    openApp,
  }) => {
    await page.clock.install();
    await openApp();
    await playButton(page).click();
    await backend.playing("survival");
    await backend.embedded("survival", true);
    await expect(page.getByTestId("game-stage")).toHaveAttribute("aria-hidden", "false");

    // Pause before stopping so the check runs well within 560ms and cannot wait out a leaked timer.
    const now = await page.evaluate(() => Date.now());
    await page.clock.pauseAt(now + 1000);
    await backend.stopped("survival", { kind: "normal", reason: "closed", code: 0 });
    await page.clock.runFor(50);

    await expect(playButton(page)).toHaveText(text("play.play"));
    expect(await page.getByTestId("game-stage-exit").count()).toBe(0);

    await page.clock.resume();
    await expect(page.getByTestId("game-stage-exit")).toHaveCount(0);
  });
});
