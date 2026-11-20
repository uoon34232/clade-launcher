import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { setMotionMode } from "@/lib/motion";
import type { SupportApi, UpdateCheckResult } from "@/lib/support/api";

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
): SupportApi {
  return {
    openCladeFolder: vi.fn(() => Promise.resolve()),
    openLogsFolder: vi.fn(() => Promise.resolve()),
    exportDiagnostics: vi.fn(() => Promise.resolve({ name: "Clade-Diagnostics-test.zip" })),
    retiredVaultsCount: vi.fn(() => Promise.resolve(0)),
    clearRetiredVaults: vi.fn(() => Promise.resolve(0)),
    checkForUpdates: vi.fn(checkForUpdates),
    openReleasePage: vi.fn(openReleasePage),
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

  it("surfaces offline/rate-limit errors in both languages and allows retrying", async () => {
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
