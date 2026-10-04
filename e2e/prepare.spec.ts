import type { Page } from "@playwright/test";
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
      itemsDone: 412,
      itemsTotal: 640,
      bytesDone: 184 * MB,
      bytesTotal: 521 * MB,
      overall: 0.4,
    });
    await expect(dialog.getByText(`${text("prepare.stage.libraries")}…`)).toBeVisible();
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

  test("Verify files re-checks the selected profile", async ({ page, backend, openApp }) => {
    await openApp();
    await profileTrigger(page).click();
    await page.getByRole("menuitem", { name: text("profile.verify") }).click();
    await expect(page.getByRole("dialog", { name: "Survival" })).toBeVisible();
    const dialog = page.getByRole("dialog");
    await expect
      .poll(async () => (await backend.calls("prepare_profile")).map((c) => c.args))
      .toEqual([{ id: "survival", verify: true }]);
    await backend.prepareProgress({
      instanceId: "survival",
      stage: "libraries",
      label: "Verifying libraries",
      overall: 0.3,
    });
    await expect(dialog.getByText(`${text("prepare.stage.libraries.verify")}…`)).toBeVisible();
    await backend.finishPrepare();
    await expect(dialog.getByRole("heading", { name: text("prepare.ready") })).toBeVisible();
  });
});

test.describe("a profile that needs repair", () => {
  test("a refused launch opens the repair, and the button reads Repairing meanwhile", async ({
    page,
    backend,
    openApp,
  }) => {
    await openApp();
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

    await backend.finishPrepare();
    await dialog.getByRole("button", { name: text("common.done") }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(playButton(page)).toHaveText(text("play.play"));
    await expect(playButton(page)).toBeEnabled();
    // the launch was refused once and not silently retried
    expect(await backend.calls("launch_profile")).toHaveLength(1);
  });
});
