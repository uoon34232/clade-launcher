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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function supportStatusText(): string {
  // SupportSection renders the first pre-mounted status live region before the update footer.
  return screen.getAllByRole("status")[0]?.textContent ?? "";
}

function clickBackdrop() {
  const backdrop = document.querySelector<HTMLElement>('[data-slot="dialog-overlay"]');
  expect(backdrop).not.toBeNull();
  if (!backdrop) return;
  fireEvent.pointerDown(backdrop, { button: 0 });
  fireEvent.mouseDown(backdrop, { button: 0 });
  fireEvent.click(backdrop, { button: 0 });
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
  it("hides the action when there are no archived vaults and resets a prior count if the check fails", async () => {
    const emptySupport = makeSupport(0);
    const { rerender } = render(
      <SettingsDialog open onOpenChange={vi.fn()} support={emptySupport} />,
    );
    await waitFor(() => expect(emptySupport.retiredVaultsCount).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();

    const populatedSupport = makeSupport(2);
    rerender(<SettingsDialog open onOpenChange={vi.fn()} support={populatedSupport} />);
    expect(await screen.findByRole("button", { name: "Clear archived vaults (2)" })).toBeTruthy();

    const failingCount: SupportApi = {
      ...makeSupport(0),
      retiredVaultsCount: vi.fn(() =>
        Promise.reject(new BackendError("no backend", "backend_unavailable")),
      ),
    };
    rerender(<SettingsDialog open onOpenChange={vi.fn()} support={failingCount} />);
    await waitFor(() => expect(failingCount.retiredVaultsCount).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull(),
    );
  });

  it("asks first, locks Done and blocks Escape/backdrop dismissal, cancels with Keep archives restoring focus, then deletes permanently", async () => {
    const onOpenChange = vi.fn();
    const support = makeSupport(2);
    render(<SettingsDialog open onOpenChange={onOpenChange} support={support} />);

    const trigger = await screen.findByRole("button", { name: "Clear archived vaults (2)" });
    fireEvent.click(trigger);

    expect(screen.getByRole("alert").textContent).toContain("Delete 2 archived vaults?");
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);
    expect(support.clearRetiredVaults).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    clickBackdrop();
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("Delete 2 archived vaults?");

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
    expect(supportStatusText()).toBe("Deleted 2 archived vaults.");

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onOpenChange).toHaveBeenCalledWith(false);

    onOpenChange.mockClear();
    clickBackdrop();
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows busy state and ignores duplicate submits while clearing vaults", async () => {
    const pending = deferred<number>();
    const onOpenChange = vi.fn();
    const clearRetiredVaults = vi.fn(() => pending.promise);
    const support: SupportApi = {
      ...makeSupport(2),
      clearRetiredVaults,
    };
    render(<SettingsDialog open onOpenChange={onOpenChange} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Clear archived vaults (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    const clearingButton = screen.getByRole<HTMLButtonElement>("button", { name: "Clearing…" });
    const keepButton = screen.getByRole<HTMLButtonElement>("button", { name: "Keep archives" });
    expect(clearingButton.disabled).toBe(true);
    expect(keepButton.disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);
    expect(clearRetiredVaults).toHaveBeenCalledTimes(1);

    fireEvent.click(clearingButton);
    fireEvent.click(keepButton);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    clickBackdrop();
    expect(clearRetiredVaults).toHaveBeenCalledTimes(1);
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("Delete 2 archived vaults?");

    pending.resolve(2);
    await waitFor(() => expect(supportStatusText()).toBe("Deleted 2 archived vaults."));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);
  });

  it("surfaces a failure in plain words, re-enables Done, and allows retrying to success", async () => {
    const clearRetiredVaults = vi
      .fn<() => Promise<number>>()
      .mockRejectedValueOnce(new BackendError("ro", "retired_vaults_clear_failed"))
      .mockResolvedValueOnce(2);
    const support: SupportApi = {
      ...makeSupport(2),
      clearRetiredVaults,
    };
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Clear archived vaults (2)" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toBe(
        t("error.settings.retired_vaults_clear_failed"),
      ),
    );
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Clear archived vaults (2)" }));
    expect(screen.getByRole("alert").textContent).toContain("Delete 2 archived vaults?");
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(clearRetiredVaults).toHaveBeenCalledTimes(2));
    expect(supportStatusText()).toBe("Deleted 2 archived vaults.");
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false);
  });

  it("falls back to the prior count in the status notice when clearRetiredVaults returns 0", async () => {
    const support: SupportApi = {
      ...makeSupport(3),
      clearRetiredVaults: vi.fn(() => Promise.resolve(0)),
    };
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Clear archived vaults (3)" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(supportStatusText()).toBe("Deleted 3 archived vaults."));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();
  });

  it("re-fetches retiredVaultsCount when reopened", async () => {
    const retiredVaultsCount = vi
      .fn<() => Promise<number>>()
      .mockResolvedValueOnce(0)
      .mockResolvedValueOnce(2);
    const support: SupportApi = {
      ...makeSupport(0),
      retiredVaultsCount,
    };
    const { rerender } = render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    await waitFor(() => expect(retiredVaultsCount).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: /Clear archived vaults/ })).toBeNull();

    rerender(<SettingsDialog open={false} onOpenChange={vi.fn()} support={support} />);
    rerender(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    await waitFor(() => expect(retiredVaultsCount).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("button", { name: "Clear archived vaults (2)" })).toBeTruthy();
  });

  it("is worded in Russian", async () => {
    setLocale("ru");
    const support = makeSupport(1);
    render(<SettingsDialog open onOpenChange={vi.fn()} support={support} />);

    fireEvent.click(await screen.findByRole("button", { name: "Удалить архивные хранилища (1)" }));
    expect(screen.getByRole("alert").textContent).toContain("Удалить 1 архивное хранилище?");
    fireEvent.click(screen.getByRole("button", { name: "Удалить" }));
    await waitFor(() => expect(supportStatusText()).toBe("Удалено 1 архивное хранилище."));
  });
});
