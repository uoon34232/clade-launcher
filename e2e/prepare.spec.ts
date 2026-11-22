import type { Page } from "@playwright/test";
import type { Backend } from "./support/fakeBackend";
import { CREATIVE } from "./support/fixtures";
import { expect, test } from "./support/test";
import { playButton, profileTrigger, text } from "./support/ui";

const MB = 1024 * 1024;

/** Selects the unprepared "Fresh" profile and presses "Set up". */
async function startSetup(page: Page) {
  await profileTrigger(page).click();
  await page.getByRole("menuitemradio", { name: /^Fresh/ }).click();
  await expect(playButton(page)).toHaveText(text("play.setup"));
  await playButton(page).click();
  await expect(page.getByRole("dialog", { name: "Fresh" })).toBeVisible();
  // Its name changes with its content (the profile, then "Ready to play."), so keep a name-free handle.
  return page.getByRole("dialog");
}

/** Triggers a refused launch (`needs_repair`) on Survival and waits for the repair dialog. */
async function startRepair(page: Page, backend: Backend) {
  await backend.refuseNextLaunch("needs_repair");
  await playButton(page).click();

  // the same preparation dialog repairs the profile
  await expect(page.getByRole("dialog", { name: "Survival" })).toBeVisible();
  const dialog = page.getByRole("dialog");
  await expect
    .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
    .toEqual([{ id: "survival", verify: false }]);
  // (the dock is hidden from assistive technology behind the dialog, so find it by its state)
  await expect(page.locator("button[data-state]")).toHaveAttribute("data-state", "repairing");
  return dialog;
}

/** Asserts that closing/cancelling repair leaves the profile playable without retrying launch. */
async function expectRepairRestoredToPlay(page: Page, backend: Backend) {
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(playButton(page)).toHaveAttribute("data-state", "play");
  await expect(playButton(page)).toHaveText(text("play.play"));
  await expect(playButton(page)).toBeEnabled();
  // the launch was refused once and not silently retried
  expect(await backend.calls("launch_profile")).toHaveLength(1);
}

test.describe("preparing a profile", () => {
  test("Set up shows live progress and ends with Ready to play", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startSetup(page);

    // exactly one preparation was started, for this profile
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: "fresh", verify: false }]);
    // before anything is measured the bar is indeterminate and never invents a percentage
    const bar = dialog.getByRole("progressbar", { name: text("prepare.progress") });
    await expect(bar).toBeVisible();
    await expect(bar).not.toHaveAttribute("aria-valuenow", /.+/);
    await expect(dialog.getByText(`${text("prepare.stage.planning")}…`)).toBeVisible();

    await backend.prepareProgress({
      instanceId: "fresh",
      stage: "libraries",
      label: "Downloading libraries",
      itemsDone: 100,
      itemsTotal: 640,
      overall: 0.2,
    });
    await expect(dialog.getByText(`${text("prepare.stage.libraries")}…`)).toBeVisible();
    await expect(bar).toHaveAttribute("aria-valuenow", "20");
    await expect(dialog.getByText("100 of 640 files")).toBeVisible();

    // Flush the opening animation frame so the burst starts a fresh rAF coalescing window.
    await page.evaluate(() => new Promise<void>((r) => requestAnimationFrame(() => r())));
    await page.evaluate((mb) => {
      const emit = (window as unknown as { __e2e: { emit: (e: string, p: unknown) => void } }).__e2e
        .emit;
      const base = {
        instanceId: "fresh",
        stage: "libraries",
        label: "Downloading libraries",
        itemsTotal: 640,
        bytesDone: null,
        bytesTotal: null,
      };
      emit("prepare-progress", { ...base, itemsDone: 150, overall: 0.25 });
      emit("prepare-progress", { ...base, itemsDone: 250, overall: 0.3 });
      emit("prepare-progress", {
        ...base,
        itemsDone: 412,
        bytesDone: 184 * mb,
        bytesTotal: 521 * mb,
        overall: 0.4,
      });
    }, MB);
    await expect(bar).toHaveAttribute("aria-valuenow", "40");
    await expect(dialog.getByText("412 of 640 files")).toBeVisible();
    await expect(dialog.getByText("184 MB of 521 MB")).toBeVisible();

    await backend.prepareProgress({
      instanceId: "fresh",
      stage: "assets",
      label: "Downloading assets",
      overall: 0.85,
    });
    await expect(dialog.getByText(`${text("prepare.stage.assets")}…`)).toBeVisible();
    await expect(bar).toHaveAttribute("aria-valuenow", "85");
    // Moving to a stage without counts clears the previous stage's file/byte totals.
    await expect(dialog.getByText("412 of 640 files")).toHaveCount(0);
    await expect(dialog.getByText("184 MB of 521 MB")).toHaveCount(0);

    // Moving from a measurable stage to an indeterminate stage removes aria-valuenow.
    await backend.prepareProgress({
      instanceId: "fresh",
      stage: "natives",
      label: "Extracting natives",
    });
    await expect(dialog.getByText(`${text("prepare.stage.natives")}…`)).toBeVisible();
    await expect(bar).not.toHaveAttribute("aria-valuenow", /.+/);

    await backend.prepareProgress({
      instanceId: "fresh",
      stage: "verifying",
      label: "Verifying files",
    });
    await expect(dialog.getByText(`${text("prepare.stage.verifying")}…`)).toBeVisible();
    await expect(bar).not.toHaveAttribute("aria-valuenow", /.+/);

    await backend.finishPrepare();
    await expect(dialog.getByRole("heading", { name: text("prepare.ready") })).toBeVisible();
    await expect(
      dialog.getByText(text("prepare.readyDescription", { name: "Fresh" })),
    ).toBeVisible();
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    // the profile is now ready to play
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
  });

  test("progress for some other profile is ignored", async ({ page, backend, openApp }) => {
    await openApp();
    const dialog = await startSetup(page);
    await backend.prepareProgress({
      instanceId: "someone-else",
      stage: "assets",
      label: "Downloading assets",
      overall: 0.9,
    });
    const bar = dialog.getByRole("progressbar", { name: text("prepare.progress") });
    await expect(bar).not.toHaveAttribute("aria-valuenow", /.+/);
    await expect(dialog.getByText(`${text("prepare.stage.planning")}…`)).toBeVisible();
  });

  test("Cancel stops the preparation and leaves the profile as it was", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startSetup(page);
    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await backend.calls("cancel_prepare")).toHaveLength(1);
    await expect(playButton(page)).toHaveText(text("play.setup"));
  });

  test("Escape also cancels", async ({ page, backend, openApp }) => {
    await openApp();
    await startSetup(page);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect.poll(async () => (await backend.calls("cancel_prepare")).length).toBe(1);
  });

  test("a failure says what happened in plain words, and Try again starts over", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startSetup(page);
    await backend.failPrepare("network");
    await expect(dialog.getByRole("alert")).toHaveText(text("error.prepare.network"));

    await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
    await expect(dialog.getByRole("progressbar")).toBeVisible();
    await expect.poll(async () => (await backend.calls("prepare_profile")).length).toBe(2);

    await backend.finishPrepare();
    await expect(dialog.getByRole("heading", { name: text("prepare.ready") })).toBeVisible();
  });

  test("a failure can be closed, and the profile still offers Set up", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startSetup(page);
    await backend.failPrepare("disk_space", {
      requiredBytes: 4_000_000_000,
      availableBytes: 300 * MB,
    });
    // the message names real sizes taken from the backend's facts
    await expect(dialog.getByRole("alert")).toContainText("3.7 GB");
    await expect(dialog.getByRole("alert")).toContainText("300 MB");
    await dialog.getByRole("button", { name: text("common.close") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveText(text("play.setup"));
  });

  test("reports prepare error codes and falls back for unknown codes", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startSetup(page);
    const errorCases = [
      { code: "offline", key: "error.prepare.offline" },
      { code: "verification", key: "error.prepare.verification" },
      { code: "metadata", key: "error.prepare.metadata" },
      { code: "unsupported", key: "error.prepare.unsupported" },
      { code: "java", key: "error.prepare.java" },
      { code: "loader", key: "error.prepare.loader" },
      { code: "storage", key: "error.prepare.storage" },
      { code: "account_required", key: "error.prepare.account_required" },
      { code: "busy", key: "error.prepare.busy" },
      { code: "already_running", key: "error.prepare.already_running" },
      { code: "unexpected_backend_code", key: "error.prepare.unknown" },
    ] as const;

    for (let i = 0; i < errorCases.length; i++) {
      const c = errorCases[i];
      await expect.poll(async () => (await backend.calls("prepare_profile")).length).toBe(i + 1);
      await backend.failPrepare(c.code);
      await expect(dialog.getByRole("alert")).toHaveText(text(c.key));
      if (i + 1 < errorCases.length) {
        await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
      }
    }
  });

  test("Verify files re-checks the selected profile", async ({ page, backend, openApp }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.verify") }).click();
    await expect(page.getByRole("dialog", { name: "Survival" })).toBeVisible();
    const dialog = page.getByRole("dialog");
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: "survival", verify: true }]);
    await expect(page.locator("button[data-state]")).toHaveAttribute("data-state", "repairing");

    const verifyStages = [
      { stage: "java", key: "prepare.stage.java.verify", overall: 0.1 },
      { stage: "minecraft", key: "prepare.stage.minecraft.verify", overall: 0.2 },
      { stage: "libraries", key: "prepare.stage.libraries.verify", overall: 0.3 },
      { stage: "assets", key: "prepare.stage.assets.verify", overall: 0.7 },
    ] as const;
    for (const step of verifyStages) {
      await backend.prepareProgress({
        instanceId: "survival",
        stage: step.stage,
        label: `Verifying ${step.stage}`,
        overall: step.overall,
      });
      await expect(dialog.getByText(`${text(step.key)}…`)).toBeVisible();
    }

    await backend.finishPrepare();
    await expect(dialog.getByRole("heading", { name: text("prepare.ready") })).toBeVisible();
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveAttribute("data-state", "play");
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
  });

  test("Verify files failure and Try again preserves verify: true", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.verify") }).click();
    await expect(page.getByRole("dialog", { name: "Survival" })).toBeVisible();
    const dialog = page.getByRole("dialog");
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: "survival", verify: true }]);

    await backend.failPrepare("verification");
    await expect(dialog.getByRole("alert")).toHaveText(text("error.prepare.verification"));
    await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([
        { id: "survival", verify: true },
        { id: "survival", verify: true },
      ]);

    await backend.finishPrepare();
    const readyDialog = page.getByRole("dialog", { name: text("prepare.ready") });
    await expect(readyDialog).toBeVisible();
    await readyDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveAttribute("data-state", "play");
    await expect(playButton(page)).toHaveText(text("play.play"));
  });
});

test.describe("preparing a modded profile", () => {
  test.use({
    backendConfig: {
      profiles: [{ ...CREATIVE, ready: false }],
      selected: "creative",
    },
  });

  test("shows modded profile details, Installing {loader} stage, and error recovery", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await expect(playButton(page)).toHaveText(text("play.setup"));
    await playButton(page).click();

    const dialog = page.getByRole("dialog", { name: "Creative" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("Fabric 1.21.4 · 0.16.9")).toBeVisible();
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: "creative", verify: false }]);

    await backend.prepareProgress({
      instanceId: "creative",
      stage: "loader",
      label: "Installing Fabric",
      overall: 0.7,
    });
    await expect(
      dialog.getByText(`${text("prepare.stage.loader", { loader: "Fabric" })}…`),
    ).toBeVisible();

    await backend.failPrepare("loader");
    await expect(dialog.getByText("Fabric 1.21.4 · 0.16.9")).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveText(text("error.prepare.loader"));

    await dialog.getByRole("button", { name: text("common.tryAgain") }).click();
    await expect.poll(async () => (await backend.calls("prepare_profile")).length).toBe(2);

    await backend.finishPrepare();
    const readyDialog = page.getByRole("dialog", { name: text("prepare.ready") });
    await expect(readyDialog).toBeVisible();
    await readyDialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
  });
});

test.describe("a profile that needs repair", () => {
  test("a refused launch opens the repair, and the button reads Repairing meanwhile", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await startRepair(page, backend);

    await backend.finishPrepare();
    const readyDialog = page.getByRole("dialog", { name: text("prepare.ready") });
    await expect(readyDialog).toBeVisible();
    await readyDialog.getByRole("button", { name: text("common.done") }).click();
    await expectRepairRestoredToPlay(page, backend);
  });

  test("cancelling repair with Cancel restores Play without retrying launch", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startRepair(page, backend);

    await dialog.getByRole("button", { name: text("common.cancel") }).click();
    await expect.poll(async () => (await backend.calls("cancel_prepare")).length).toBe(1);
    await expectRepairRestoredToPlay(page, backend);
  });

  test("cancelling repair with Escape restores Play without retrying launch", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    await startRepair(page, backend);

    await page.keyboard.press("Escape");
    await expect.poll(async () => (await backend.calls("cancel_prepare")).length).toBe(1);
    await expectRepairRestoredToPlay(page, backend);
  });

  test("closing after a repair failure restores Play without retrying launch", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
    const dialog = await startRepair(page, backend);

    await backend.failPrepare("network");
    await expect(dialog.getByRole("alert")).toHaveText(text("error.prepare.network"));
    await dialog.getByRole("button", { name: text("common.close") }).click();
    await expectRepairRestoredToPlay(page, backend);
  });
});
