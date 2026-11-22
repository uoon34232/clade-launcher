import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { setMotionMode } from "@/lib/motion";
import type { SupportApi, UpdateCheckResult, UpdateProgress } from "@/lib/support/api";

const playSound = vi.fn<(kind: string) => boolean>(() => true);
vi.mock("@/lib/sound/soundService", () => ({
  playSound: (kind: string) => playSound(kind),
  sound: {
    getSettings: () => ({ enabled: true, volume: 0.5 }),
    subscribe: () => () => undefined,
    setSettings: () => undefined,
  },
}));

import { SettingsDialog } from "./SettingsDialog";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function updateStatusText(): string {
  // SupportSection is first and the update footer is the second pre-mounted role="status" live region.
  const statuses = screen.queryAllByRole("status");
  return statuses[statuses.length - 1]?.textContent ?? "";
}

function makeSupport(
  checkForUpdates: () => Promise<UpdateCheckResult>,
  openReleasePage: (tag?: string | null) => Promise<void> = () => Promise.resolve(),
  overrides: Partial<SupportApi> = {},
): SupportApi {
  return {
    openCladeFolder: vi.fn(() => Promise.resolve()),
    openLogsFolder: vi.fn(() => Promise.resolve()),
    exportDiagnostics: vi.fn(() => Promise.resolve({ name: "Clade-Diagnostics-test.zip" })),
    retiredVaultsCount: vi.fn(() => Promise.resolve(0)),
    clearRetiredVaults: vi.fn(() => Promise.resolve(0)),
    checkForUpdates: vi.fn(checkForUpdates),
    openReleasePage: vi.fn(openReleasePage),
    ...overrides,
  };
}

beforeEach(() => {
  setLocale("en");
  setMotionMode("off");
  playSound.mockClear();
});

afterEach(() => {
  cleanup();
  setLocale("en");
  setMotionMode("system");
});

describe("Manual update check in Settings", () => {
  it("never calls the backend automatically and shows Checking… then Up to date on click", async () => {
    const check = deferred<UpdateCheckResult>();
    const support = makeSupport(() => check.promise);
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    expect(support.checkForUpdates).not.toHaveBeenCalled();
    const trigger = screen.getByRole("button", { name: "Check for updates" });
    fireEvent.click(trigger);

    expect(support.checkForUpdates).toHaveBeenCalledTimes(1);
    const checkingBtn = screen.getByRole<HTMLButtonElement>("button", { name: "Checking…" });
    expect(checkingBtn.disabled).toBe(true);

    check.resolve({ status: "up_to_date", currentVersion: "v0.1.0", latestVersion: "v0.1.0" });
    await waitFor(() => expect(updateStatusText()).toBe("Up to date"));
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
  });

  it("shows the newer release tag and opens the canonical release page via Rust", async () => {
    const support = makeSupport(() =>
      Promise.resolve({
        status: "update_available",
        currentVersion: "v0.1.0-preview.3",
        latestVersion: "v0.1.0-preview.4",
      }),
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(updateStatusText()).toBe("v0.1.0-preview.4 available"));

    const openBtn = screen.getByRole("button", { name: "Open release" });
    fireEvent.click(openBtn);
    await waitFor(() => expect(support.openReleasePage).toHaveBeenCalledWith("v0.1.0-preview.4"));
  });

  it("handles in-app portable download, progress, cancellation, persistence, and localized error codes", async () => {
    let emitProgress: ((p: UpdateProgress) => void) | null = null;
    let resolveInstall!: (p: UpdateProgress) => void;
    let rejectInstall!: (e: unknown) => void;
    let currentStatus: UpdateProgress = {
      stage: "idle",
      version: null,
      bytesDone: 0,
      bytesTotal: 0,
    };

    const support = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0-preview.4",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: 200,
        }),
      () => Promise.resolve(),
      {
        getUpdateStatus: vi.fn(() => Promise.resolve(currentStatus)),
        onUpdateProgress: vi.fn((cb: (p: UpdateProgress) => void) => {
          emitProgress = cb;
          return Promise.resolve(() => {
            emitProgress = null;
          });
        }),
        installUpdate: vi.fn(
          () =>
            new Promise<UpdateProgress>((resolve, reject) => {
              resolveInstall = resolve;
              rejectInstall = reject;
            }),
        ),
        cancelUpdate: vi.fn(() => Promise.resolve()),
        applyStagedUpdate: vi.fn(() => {
          emitProgress?.({ stage: "applying", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 });
          return Promise.resolve();
        }),
      },
    );

    const onOpenChange = vi.fn();
    const { unmount } = render(
      <SettingsDialog open onOpenChange={onOpenChange} support={support} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(updateStatusText()).toBe("v0.2.0 available"));
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open release" })).toBeTruthy();

    // Start download -> streams progress, then Rust emits "idle" before rejecting with update_cancelled.
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    expect(support.installUpdate).toHaveBeenCalledWith("v0.2.0");
    await waitFor(() => expect(emitProgress).not.toBeNull());

    act(() =>
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 100, bytesTotal: 200 }),
    );
    await waitFor(() => expect(screen.getByText("Downloading… 50%")).toBeTruthy());
    expect(updateStatusText()).toBe("Downloading…");
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
    // While updating, Done is disabled and dialog stays locked.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(support.cancelUpdate).toHaveBeenCalledTimes(1);
    act(() => emitProgress?.({ stage: "idle", version: null, bytesDone: 0, bytesTotal: 0 }));
    await waitFor(() => expect(updateStatusText()).toBe(""));
    act(() => rejectInstall(new BackendError("cancelled", "update_cancelled")));
    await waitFor(() => expect(updateStatusText()).toBe("v0.2.0 available"));

    // Retry download -> verify -> extract -> ready event -> fresh mount persistence -> restart handoff.
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    act(() =>
      emitProgress?.({ stage: "verifying", version: "v0.2.0", bytesDone: 200, bytesTotal: 200 }),
    );
    await waitFor(() => expect(updateStatusText()).toBe("Verifying…"));

    act(() =>
      emitProgress?.({ stage: "extracting", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 }),
    );
    await waitFor(() => expect(updateStatusText()).toBe("Unpacking…"));
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);

    currentStatus = { stage: "ready", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 };
    act(() => emitProgress?.(currentStatus));
    await waitFor(() => expect(updateStatusText()).toBe("Ready to update"));
    act(() => resolveInstall(currentStatus));
    await waitFor(() => expect(updateStatusText()).toBe("Ready to update"));

    // Unmounting and mounting a fresh dialog rehydrates ready state from getUpdateStatus without re-checking network.
    unmount();
    render(<SettingsDialog open onOpenChange={onOpenChange} support={support} />);
    await waitFor(() => expect(updateStatusText()).toBe("Ready to update"));
    expect(support.getUpdateStatus).toHaveBeenCalledTimes(2);
    expect(support.checkForUpdates).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    expect(support.applyStagedUpdate).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(updateStatusText()).toBe("Restarting…"));

    // Verify not_enough_space parameter formatting in RU after re-check failure.
    const errSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: 200,
        }),
      () => Promise.resolve(),
      {
        installUpdate: vi.fn(() =>
          Promise.reject(
            new BackendError("no space", "not_enough_space", {
              params: { requiredBytes: 2 * 1024 * 1024 * 1024, availableBytes: 512 * 1024 * 1024 },
            }),
          ),
        ),
      },
    );
    cleanup();
    act(() => setLocale("ru"));
    render(<SettingsDialog open onOpenChange={vi.fn()} support={errSupport} />);
    fireEvent.click(screen.getByRole("button", { name: "Проверить обновления" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Обновить" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("Недостаточно места"),
    );
  });

  it("ignores unsolicited working and idle progress events when no install is active", async () => {
    let emitProgress: ((p: UpdateProgress) => void) | null = null;
    const support = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: 200,
        }),
      () => Promise.resolve(),
      {
        getUpdateStatus: vi.fn(() =>
          Promise.resolve<UpdateProgress>({
            stage: "idle",
            version: null,
            bytesDone: 0,
            bytesTotal: 0,
          }),
        ),
        onUpdateProgress: vi.fn((cb: (p: UpdateProgress) => void) => {
          emitProgress = cb;
          return Promise.resolve(() => {
            emitProgress = null;
          });
        }),
        installUpdate: vi.fn(() => deferred<UpdateProgress>().promise),
      },
    );

    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);
    await waitFor(() => expect(emitProgress).not.toBeNull());
    await waitFor(() => expect(support.getUpdateStatus).toHaveBeenCalledTimes(1));

    act(() => {
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 50, bytesTotal: 100 });
      emitProgress?.({ stage: "verifying", version: "v0.2.0", bytesDone: 100, bytesTotal: 100 });
      emitProgress?.({ stage: "extracting", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 });
    });
    expect(updateStatusText()).toBe("");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(updateStatusText()).toBe("v0.2.0 available"));

    act(() => {
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 50, bytesTotal: 100 });
      emitProgress?.({ stage: "idle", version: null, bytesDone: 0, bytesTotal: 0 });
    });
    expect(updateStatusText()).toBe("v0.2.0 available");
    expect(screen.queryByRole("progressbar")).toBeNull();
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
  });

  it("rehydrates freshly mounted Settings from getUpdateStatus for ready and mid-install stages", async () => {
    const readyStatus = deferred<UpdateProgress>();
    const readySupport = makeSupport(
      () =>
        Promise.resolve({
          status: "up_to_date",
          currentVersion: "v0.1.0",
          latestVersion: "v0.1.0",
        }),
      undefined,
      {
        getUpdateStatus: vi.fn(() => readyStatus.promise),
        applyStagedUpdate: vi.fn(() => Promise.resolve()),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={readySupport} />);
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    expect(updateStatusText()).toBe("");

    readyStatus.resolve({ stage: "ready", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 });
    await waitFor(() => expect(updateStatusText()).toBe("Ready to update"));
    expect(readySupport.getUpdateStatus).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Restart to update" })).toBeTruthy();

    // Downloading stage rehydrates and enables subsequent progress events.
    cleanup();
    let emitProgress: ((p: UpdateProgress) => void) | null = null;
    const downloadStatus = deferred<UpdateProgress>();
    const downloadSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "up_to_date",
          currentVersion: "v0.1.0",
          latestVersion: "v0.1.0",
        }),
      undefined,
      {
        getUpdateStatus: vi.fn(() => downloadStatus.promise),
        onUpdateProgress: vi.fn((cb: (p: UpdateProgress) => void) => {
          emitProgress = cb;
          return Promise.resolve(() => {
            emitProgress = null;
          });
        }),
        cancelUpdate: vi.fn(() => Promise.resolve()),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={downloadSupport} />);
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    await waitFor(() => expect(emitProgress).not.toBeNull());

    downloadStatus.resolve({
      stage: "downloading",
      version: "v0.2.0",
      bytesDone: 50,
      bytesTotal: 200,
    });
    await waitFor(() => expect(screen.getByText("Downloading… 25%")).toBeTruthy());
    expect(downloadSupport.getUpdateStatus).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("25");
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();

    act(() =>
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 150, bytesTotal: 200 }),
    );
    await waitFor(() => expect(screen.getByText("Downloading… 75%")).toBeTruthy());

    // Verifying stage rehydration.
    cleanup();
    const verifyStatus = deferred<UpdateProgress>();
    const verifySupport = makeSupport(
      () =>
        Promise.resolve({
          status: "up_to_date",
          currentVersion: "v0.1.0",
          latestVersion: "v0.1.0",
        }),
      undefined,
      {
        getUpdateStatus: vi.fn(() => verifyStatus.promise),
        cancelUpdate: vi.fn(() => Promise.resolve()),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={verifySupport} />);
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    verifyStatus.resolve({
      stage: "verifying",
      version: "v0.2.0",
      bytesDone: 200,
      bytesTotal: 200,
    });
    await waitFor(() => expect(updateStatusText()).toBe("Verifying…"));
    expect(verifySupport.getUpdateStatus).toHaveBeenCalledTimes(1);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(false);

    // Extracting stage rehydration locks cancellation.
    cleanup();
    const extractStatus = deferred<UpdateProgress>();
    const extractSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "up_to_date",
          currentVersion: "v0.1.0",
          latestVersion: "v0.1.0",
        }),
      undefined,
      {
        getUpdateStatus: vi.fn(() => extractStatus.promise),
        cancelUpdate: vi.fn(() => Promise.resolve()),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={extractSupport} />);
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    extractStatus.resolve({ stage: "extracting", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 });
    await waitFor(() => expect(updateStatusText()).toBe("Unpacking…"));
    expect(extractSupport.getUpdateStatus).toHaveBeenCalledTimes(1);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);
  });

  it("keeps ready state on recoverable applyStagedUpdate errors and transitions to error on fatal errors", async () => {
    const recoverable = [
      ["update_game_running", t("error.settings.update_game_running")],
      ["update_prepare_running", t("error.settings.update_prepare_running")],
      ["update_install_failed", t("error.settings.update_install_failed")],
    ] as const;
    const errors = [
      ...recoverable.map(([code]) => new BackendError(code, code)),
      new BackendError("verify failed", "update_verify_failed"),
    ];
    let attempt = 0;
    const support = makeSupport(
      () =>
        Promise.resolve({
          status: "up_to_date",
          currentVersion: "v0.1.0",
          latestVersion: "v0.1.0",
        }),
      undefined,
      {
        getUpdateStatus: vi.fn(() =>
          Promise.resolve<UpdateProgress>({
            stage: "ready",
            version: "v0.2.0",
            bytesDone: 0,
            bytesTotal: 0,
          }),
        ),
        applyStagedUpdate: vi.fn(() => Promise.reject(errors[attempt++])),
      },
    );

    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);
    await waitFor(() => expect(updateStatusText()).toBe("Ready to update"));

    for (const [, expectedText] of recoverable) {
      fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
      await waitFor(() => expect(screen.getByRole("alert").textContent).toBe(expectedText));
      expect(screen.getByRole("button", { name: "Restart to update" })).toBeTruthy();
    }

    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(t("error.settings.update_verify_failed")),
    );
    expect(screen.queryByRole("button", { name: "Restart to update" })).toBeNull();
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
  });

  it("surfaces openReleasePage failures and resets cancellingUpdate when cancelUpdate rejects", async () => {
    const browserErrSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: false,
          assetSize: null,
        }),
      () => Promise.reject(new BackendError("no browser", "browser_unavailable")),
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={browserErrSupport} />);
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Open release" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Open release" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(t("error.settings.browser_unavailable")),
    );
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();

    cleanup();
    let cancelCalls = 0;
    const cancelSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: 200,
        }),
      undefined,
      {
        installUpdate: vi.fn(() => deferred<UpdateProgress>().promise),
        cancelUpdate: vi.fn(() => {
          cancelCalls += 1;
          return cancelCalls === 1
            ? Promise.reject(new BackendError("busy", "update_in_progress"))
            : Promise.resolve();
        }),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={cancelSupport} />);
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Update" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));

    const cancelBtn = screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" });
    fireEvent.click(cancelBtn);
    expect(cancelBtn.disabled).toBe(true);
    await waitFor(() => expect(cancelBtn.disabled).toBe(false));

    fireEvent.click(cancelBtn);
    expect(cancelSupport.cancelUpdate).toHaveBeenCalledTimes(2);
  });

  it("renders indeterminate progress when bytesTotal is zero and ignores stale checkForUpdates after closing", async () => {
    let emitProgress: ((p: UpdateProgress) => void) | null = null;
    const unknownSizeSupport = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: null,
        }),
      undefined,
      {
        onUpdateProgress: vi.fn((cb: (p: UpdateProgress) => void) => {
          emitProgress = cb;
          return Promise.resolve(() => {
            emitProgress = null;
          });
        }),
        installUpdate: vi.fn(() => deferred<UpdateProgress>().promise),
        cancelUpdate: vi.fn(() => Promise.resolve()),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={unknownSizeSupport} />);
    await waitFor(() => expect(emitProgress).not.toBeNull());

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Update" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Update" }));

    expect(updateStatusText()).toBe(t("settings.updatingDownloadingUnknown"));
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBeNull();
    expect(updateStatusText()).not.toMatch(/%/);

    act(() =>
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 64, bytesTotal: 0 }),
    );
    expect(updateStatusText()).toBe("Downloading…");
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBeNull();
    expect(updateStatusText()).not.toMatch(/%/);

    // Closing while checkForUpdates is pending discards its stale resolution on reopen.
    cleanup();
    const staleCheck = deferred<UpdateCheckResult>();
    let checkAttempt = 0;
    const staleSupport = makeSupport(() => {
      checkAttempt += 1;
      return checkAttempt === 1
        ? staleCheck.promise
        : Promise.resolve({
            status: "up_to_date",
            currentVersion: "v0.1.0",
            latestVersion: "v0.1.0",
          });
    });
    const { rerender } = render(
      <SettingsDialog open onOpenChange={vi.fn()} support={staleSupport} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Checking…" }).disabled).toBe(
      true,
    );

    rerender(<SettingsDialog open={false} onOpenChange={vi.fn()} support={staleSupport} />);
    await act(async () => {
      staleCheck.resolve({
        status: "update_available",
        currentVersion: "v0.1.0",
        latestVersion: "v0.9.9",
        canInstall: true,
        assetSize: 100,
      });
      await Promise.resolve();
    });

    rerender(<SettingsDialog open onOpenChange={vi.fn()} support={staleSupport} />);
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeTruthy();
    expect(updateStatusText()).toBe("");
    expect(screen.queryByRole("alert")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(updateStatusText()).toBe("Up to date"));
    expect(staleSupport.checkForUpdates).toHaveBeenCalledTimes(2);
  });

  it("hides in-app Update when release assets are missing and surfaces offline/rate-limit errors", async () => {
    let attempt = 0;
    const support = makeSupport(
      () => {
        attempt += 1;
        if (attempt === 1) return Promise.reject(new BackendError("dns", "offline"));
        if (attempt === 2) return Promise.reject(new BackendError("429", "update_rate_limited"));
        if (attempt === 3) {
          return Promise.resolve({
            status: "up_to_date",
            currentVersion: "v0.1.0",
            latestVersion: "v0.1.0",
          });
        }
        return Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: false,
          assetSize: null,
        });
      },
      undefined,
      {
        installUpdate: vi.fn(() => deferred<UpdateProgress>().promise),
      },
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(t("error.settings.offline")),
    );

    act(() => setLocale("ru"));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("Нет сети. Повтори попытку."),
    );

    fireEvent.click(screen.getByRole("button", { name: "Повторить" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe("Слишком часто. Попробуй позже."),
    );

    fireEvent.click(screen.getByRole("button", { name: "Повторить" }));
    await waitFor(() => expect(updateStatusText()).toBe("Обновлений нет"));

    act(() => setLocale("en"));
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(updateStatusText()).toBe("v0.2.0 available"));
    expect(screen.getByRole("button", { name: "Open release" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
    expect(support.installUpdate).not.toHaveBeenCalled();
  });

  it("keeps both status live regions pre-mounted and hands focus cleanly across install and apply transitions", async () => {
    let resolveInstall!: (p: UpdateProgress) => void;
    let rejectApply!: (e: unknown) => void;
    const installUpdate = vi.fn(
      () =>
        new Promise<UpdateProgress>((resolve) => {
          resolveInstall = resolve;
        }),
    );
    const applyStagedUpdate = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectApply = reject;
        }),
    );
    const cancelUpdate = vi.fn(() => Promise.resolve());
    const support = makeSupport(
      () =>
        Promise.resolve({
          status: "update_available",
          currentVersion: "v0.1.0",
          latestVersion: "v0.2.0",
          canInstall: true,
          assetSize: 1024,
        }),
      undefined,
      {
        installUpdate,
        cancelUpdate,
        applyStagedUpdate,
      },
    );

    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);
    expect(screen.getAllByRole("status")).toHaveLength(2);

    const checkBtn = screen.getByRole("button", { name: "Check for updates" });
    checkBtn.focus();
    fireEvent.click(checkBtn);
    const updateBtn = await screen.findByRole("button", { name: "Update" });
    expect(screen.getAllByRole("status")).toHaveLength(2);

    updateBtn.focus();
    fireEvent.click(updateBtn);
    const cancelBtn = await screen.findByRole("button", { name: "Cancel" });
    expect(document.activeElement).toBe(cancelBtn);

    await act(async () => {
      resolveInstall({
        stage: "ready",
        bytesDone: 1024,
        bytesTotal: 1024,
        version: "v0.2.0",
      });
      await Promise.resolve();
    });
    const restartBtn = await screen.findByRole("button", { name: "Restart to update" });
    expect(document.activeElement).toBe(restartBtn);

    fireEvent.click(restartBtn);
    await waitFor(() => expect(applyStagedUpdate).toHaveBeenCalledTimes(1));
    await act(async () => {
      rejectApply(new BackendError("game", "update_game_running"));
      await Promise.resolve();
    });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Restart to update" }));
    expect(screen.getByRole("alert").textContent).toBe(t("error.settings.update_game_running"));
  });
});
