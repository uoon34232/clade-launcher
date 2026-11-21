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
    let resolveCheck!: (res: UpdateCheckResult) => void;
    const support = makeSupport(
      () =>
        new Promise<UpdateCheckResult>((resolve) => {
          resolveCheck = resolve;
        }),
    );
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    expect(support.checkForUpdates).not.toHaveBeenCalled();
    const trigger = screen.getByRole("button", { name: "Check for updates" });
    fireEvent.click(trigger);

    expect(support.checkForUpdates).toHaveBeenCalledTimes(1);
    const checkingBtn = screen.getByRole<HTMLButtonElement>("button", { name: "Checking…" });
    expect(checkingBtn.disabled).toBe(true);

    resolveCheck({ status: "up_to_date", currentVersion: "v0.1.0", latestVersion: "v0.1.0" });
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Up to date"));
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
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("v0.1.0-preview.4 available"),
    );

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
        applyStagedUpdate: vi.fn(() => Promise.resolve()),
      },
    );

    const onOpenChange = vi.fn();
    const { rerender } = render(
      <SettingsDialog open onOpenChange={onOpenChange} support={support} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("v0.2.0 available"));
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Open release" })).toBeTruthy();

    // Start download -> streams progress, then cancel cleanly restores available state.
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    expect(support.installUpdate).toHaveBeenCalledWith("v0.2.0");
    await waitFor(() => expect(emitProgress).not.toBeNull());

    act(() =>
      emitProgress?.({ stage: "downloading", version: "v0.2.0", bytesDone: 100, bytesTotal: 200 }),
    );
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Downloading… 50%"));
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
    // While updating, Done is disabled and dialog stays locked.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(support.cancelUpdate).toHaveBeenCalledTimes(1);
    act(() => rejectInstall(new BackendError("cancelled", "update_cancelled")));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("v0.2.0 available"));

    // Retry download -> verify -> extract -> ready -> restart handoff.
    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    act(() =>
      emitProgress?.({ stage: "verifying", version: "v0.2.0", bytesDone: 200, bytesTotal: 200 }),
    );
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Verifying…"));

    act(() =>
      emitProgress?.({ stage: "extracting", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 }),
    );
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Unpacking…"));
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);

    currentStatus = { stage: "ready", version: "v0.2.0", bytesDone: 0, bytesTotal: 0 };
    act(() => resolveInstall(currentStatus));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Ready to update"));

    // Closing & reopening Settings syncs ready state via getUpdateStatus without re-checking network.
    rerender(<SettingsDialog open={false} onOpenChange={onOpenChange} support={support} />);
    rerender(<SettingsDialog open onOpenChange={onOpenChange} support={support} />);
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Ready to update"));
    expect(support.checkForUpdates).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    expect(support.applyStagedUpdate).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Restarting…"));

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

  it("hides in-app Update when release assets are missing and surfaces offline/rate-limit errors", async () => {
    let attempt = 0;
    const support = makeSupport(() => {
      attempt += 1;
      if (attempt === 1) return Promise.reject(new BackendError("dns", "offline"));
      if (attempt === 2) return Promise.reject(new BackendError("429", "update_rate_limited"));
      return Promise.resolve({
        status: "up_to_date",
        currentVersion: "v0.1.0",
        latestVersion: "v0.1.0",
      });
    });
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
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Обновлений нет"));
  });
});
