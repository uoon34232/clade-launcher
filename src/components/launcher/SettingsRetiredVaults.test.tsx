import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { setMotionMode } from "@/lib/motion";
import type { SupportApi } from "@/lib/support/api";

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

function makeSupport(count: number, err?: BackendError): SupportApi {
  let current = count;
  return {
    openCladeFolder: vi.fn(() => Promise.resolve()),
    openLogsFolder: vi.fn(() => Promise.resolve()),
    exportDiagnostics: vi.fn(() => Promise.resolve({ name: "Clade-Diagnostics-test.zip" })),
    retiredVaultsCount: vi.fn(() => Promise.resolve(current)),
    clearRetiredVaults: vi.fn(() => {
      if (err) return Promise.reject(err);
      const removed = current;
      current = 0;
      return Promise.resolve(removed);
    }),
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

describe("Clearing archived vaults in Settings", () => {
  it("hides the action when there are no archived vaults or the check fails", async () => {
    const emptySupport = makeSupport(0);
    const { rerender } = render(
      <SettingsDialog open onOpenChange={vi.fn()} support={emptySupport} />,
    );
    await waitFor(() => expect(emptySupport.retiredVaultsCount).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();

    const failingCount: SupportApi = {
      ...makeSupport(0),
      retiredVaultsCount: vi.fn(() =>
        Promise.reject(new BackendError("no backend", "backend_unavailable")),
      ),
    };
    rerender(<SettingsDialog open onOpenChange={vi.fn()} support={failingCount} />);
    await waitFor(() => expect(failingCount.retiredVaultsCount).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();
  });

  it("asks first, locks Done, cancels with Keep archives restoring focus, then deletes permanently", async () => {
    const support = makeSupport(2);
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    const trigger = await screen.findByRole("button", { name: "Clear archived vaults (2)" });
    fireEvent.click(trigger);

    expect(screen.getByRole("alert").textContent).toContain("Delete 2 archived vaults?");
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);
    expect(support.clearRetiredVaults).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Keep archives" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Clear archived vaults (2)" })).toBeTruthy(),
    );
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Clear archived vaults (2)" }),
    );
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);
    expect(support.clearRetiredVaults).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Clear archived vaults (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(support.clearRetiredVaults).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);
    expect(screen.getByRole("status").textContent).toBe("Deleted 2 archived vaults.");
  });

  it("surfaces a failure in plain words and keeps the action available to retry", async () => {
    const support = makeSupport(2, new BackendError("ro", "retired_vaults_clear_failed"));
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Clear archived vaults (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(
        t("error.settings.retired_vaults_clear_failed"),
      ),
    );
    expect(screen.getByRole("button", { name: "Clear archived vaults (2)" })).toBeTruthy();
  });

  it("is worded in Russian", async () => {
    setLocale("ru");
    const support = makeSupport(1);
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Удалить архивные хранилища (1)" }));
    expect(screen.getByRole("alert").textContent).toContain("Удалить 1 архивное хранилище?");
    fireEvent.click(screen.getByRole("button", { name: "Удалить" }));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("Удалено 1 архивное хранилище."),
    );
  });
});
