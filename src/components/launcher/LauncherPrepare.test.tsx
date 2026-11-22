import { Profiler, StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type {
  PrepareProgress,
  ProfileSummary,
  ProfilesApi,
  ProfilesSnapshot,
} from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";
import { readyAccountsClient } from "@/test/testAccounts";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const pending: ProfileSummary = {
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: false,
  playtimeSeconds: 0,
};
const moddedPending: ProfileSummary = {
  id: "modded",
  name: "Fabric Survival",
  minecraft: "1.21.4",
  kind: "modded",
  loader: "fabric",
  loaderVersion: "0.16.9",
  ready: false,
  playtimeSeconds: 0,
};
const snap = (p: ProfileSummary): ProfilesSnapshot => ({
  profiles: [p],
  selected: p.id,
  totalPlaytimeSeconds: 0,
});

function makeApi(initial: ProfilesSnapshot) {
  let current = initial;
  let progress: ((p: PrepareProgress) => void) | null = null;
  let running: { resolve: (s: ProfilesSnapshot) => void; reject: (e: unknown) => void } | null =
    null;
  let cancelRequested = false;
  const list = vi.fn<ProfilesApi["list"]>(() => Promise.resolve(current));
  const prepare = vi.fn<ProfilesApi["prepare"]>(() => {
    if (running) {
      return Promise.reject(new BackendError("Another profile is being set up.", "busy"));
    }
    cancelRequested = false;
    return new Promise<ProfilesSnapshot>((resolve, reject) => {
      running = {
        resolve: (s) => {
          running = null;
          resolve(s);
        },
        reject: (e: unknown) => {
          running = null;
          reject(e instanceof Error ? e : new Error(String(e)));
        },
      };
    });
  });
  // Mirrors Rust's cancel_prepare: records cancellation while prepare_profile unwinds asynchronously.
  const cancelPrepare = vi.fn<ProfilesApi["cancelPrepare"]>(() => {
    cancelRequested = true;
    return Promise.resolve();
  });
  const onPrepareProgress = vi.fn<ProfilesApi["onPrepareProgress"]>((cb) => {
    progress = cb;
    return Promise.resolve(() => {
      progress = null;
    });
  });
  return {
    list,
    select: vi.fn<ProfilesApi["select"]>(() => Promise.resolve()),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.reject(new Error("offline")),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve([])),
    create: vi.fn<ProfilesApi["create"]>(() => Promise.reject(new Error("no"))),
    rename: vi.fn<ProfilesApi["rename"]>(() => Promise.reject(new Error("not used in this test"))),
    setMemory: vi.fn<ProfilesApi["setMemory"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setResolution: vi.fn<ProfilesApi["setResolution"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setJvmArgs: vi.fn<ProfilesApi["setJvmArgs"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setVersion: vi.fn<ProfilesApi["setVersion"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    duplicate: vi.fn<ProfilesApi["duplicate"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    openFolder: vi.fn<ProfilesApi["openFolder"]>(() => Promise.resolve()),
    openInstancesFolder: vi.fn<ProfilesApi["openInstancesFolder"]>(() => Promise.resolve()),
    prepare,
    cancelPrepare,
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress,
    isCancelRequested: () => cancelRequested,
    emit: (p: Partial<PrepareProgress>) =>
      act(() =>
        progress?.({
          instanceId: current.selected ?? "survival",
          stage: "libraries",
          label: "Downloading libraries",
          itemsDone: null,
          itemsTotal: null,
          bytesDone: null,
          bytesTotal: null,
          overall: null,
          ...p,
        }),
      ),
    finish: (next: ProfilesSnapshot) =>
      act(async () => {
        current = next;
        running?.resolve(next);
        await Promise.resolve();
        await Promise.resolve();
      }),
    fail: (e: unknown) =>
      act(async () => {
        running?.reject(e);
        await Promise.resolve();
        await Promise.resolve();
      }),
  };
}

const flush = async () => {
  await act(async () => {
    for (let i = 0; i < 6; i++) await Promise.resolve();
  });
};

function captureAnimationFrames() {
  let nextId = 1;
  const callbacks = new Map<number, FrameRequestCallback>();
  const requestSpy = vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
    const id = nextId++;
    callbacks.set(id, cb);
    return id;
  });
  const cancelSpy = vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => {
    callbacks.delete(id);
  });
  return {
    requestSpy,
    cancelSpy,
    pendingCount: () => callbacks.size,
    flushFrame: (ts = 16) => {
      const entries = [...callbacks.entries()];
      callbacks.clear();
      act(() => {
        for (const [, cb] of entries) cb(ts);
      });
    },
    restore: () => {
      requestSpy.mockRestore();
      cancelSpy.mockRestore();
    },
  };
}

beforeEach(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  if (typeof window.matchMedia !== "function") {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (q: string) => ({
        matches: false,
        media: q,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
      }),
    });
  }
});
afterEach(() => cleanup());

async function open(initial: ProfilesSnapshot, label: string) {
  const api = makeApi(initial);
  const rendered = render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
  await flush();
  fireEvent.click(await screen.findByRole("button", { name: label }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(1));
  return { api, dialog, ...rendered };
}

describe("setting a profile up", () => {
  it("starts from the primary action, shows real progress, and ends with Ready to play.", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    expect(api.prepare).toHaveBeenCalledWith("survival");
    expect(dialog.textContent).toContain("Survival");

    // Totals unknown: an honest indeterminate bar, no percentage.
    api.emit({ stage: "planning", label: "Preparing Minecraft" });
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBeNull();

    // Totals known: real counts and a real fraction.
    api.emit({
      itemsDone: 412,
      itemsTotal: 640,
      bytesDone: 184 * 1024 * 1024,
      bytesTotal: 521 * 1024 * 1024,
      overall: 0.35,
    });
    expect(dialog.textContent).toContain("Downloading libraries");
    expect(dialog.textContent).toContain("412 of 640 files");
    expect(dialog.textContent).toContain("184 MB of 521 MB");
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("35");

    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("Ready to play."));
    expect(screen.getByRole("dialog").textContent).toContain("is installed on this computer");
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // The dock is back to the normal Play-ready state.
    expect(screen.getByRole("button", { name: "Play" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
  });

  it("coalesces same-stage progress updates within one animation frame and deduplicates identical payloads", async () => {
    const onRender = vi.fn();
    const api = makeApi(snap(pending));
    render(
      <Profiler id="prepare" onRender={onRender}>
        <LauncherHome api={api} accountsClient={readyAccountsClient()} />
      </Profiler>,
    );
    await flush();
    fireEvent.click(await screen.findByRole("button", { name: "Set up" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(1));

    const raf = captureAnimationFrames();
    try {
      // First update in a stage commits synchronously and opens a rAF coalescing window.
      api.emit({ stage: "libraries", itemsDone: 10, itemsTotal: 100, overall: 0.1 });
      expect(dialog.textContent).toContain("10 of 100 files");
      expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("10");
      expect(raf.pendingCount()).toBe(1);

      // Subsequent same-stage updates inside the same frame are coalesced until rAF fires.
      api.emit({ stage: "libraries", itemsDone: 20, itemsTotal: 100, overall: 0.2 });
      api.emit({ stage: "libraries", itemsDone: 50, itemsTotal: 100, overall: 0.5 });
      expect(dialog.textContent).toContain("10 of 100 files");
      expect(dialog.textContent).not.toContain("20 of 100 files");
      expect(raf.pendingCount()).toBe(1);

      raf.flushFrame();
      expect(dialog.textContent).toContain("50 of 100 files");
      expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("50");
      expect(raf.pendingCount()).toBe(0);

      // Identical payloads (including overall changes within 1e-4) are deduplicated without re-rendering.
      onRender.mockClear();
      api.emit({ stage: "libraries", itemsDone: 50, itemsTotal: 100, overall: 0.5 });
      expect(onRender).not.toHaveBeenCalled();
      raf.flushFrame();
      expect(onRender).not.toHaveBeenCalled();

      api.emit({ stage: "libraries", itemsDone: 50, itemsTotal: 100, overall: 0.50005 });
      expect(onRender).not.toHaveBeenCalled();
      raf.flushFrame();
      expect(onRender).not.toHaveBeenCalled();

      // A stage transition cancels any stale queued same-stage update and commits immediately.
      api.emit({ stage: "libraries", itemsDone: 60, itemsTotal: 100, overall: 0.6 });
      api.emit({ stage: "libraries", itemsDone: 70, itemsTotal: 100, overall: 0.7 });
      api.emit({ stage: "assets", itemsDone: 1, itemsTotal: 10, overall: 0.8 });
      expect(dialog.textContent).toContain("Downloading assets…");
      expect(dialog.textContent).toContain("1 of 10 files");
      raf.flushFrame();
      expect(dialog.textContent).toContain("Downloading assets…");
      expect(dialog.textContent).toContain("1 of 10 files");
    } finally {
      raf.restore();
    }
  });

  it("cancels pending animation frames on finish, failure/cancel, and unmount", async () => {
    // 1. Cleanup on finish
    const first = await open(snap(pending), "Set up");
    const raf1 = captureAnimationFrames();
    try {
      first.api.emit({ stage: "libraries", itemsDone: 1, itemsTotal: 10, overall: 0.1 });
      first.api.emit({ stage: "libraries", itemsDone: 2, itemsTotal: 10, overall: 0.2 });
      expect(raf1.pendingCount()).toBe(1);
      await first.api.finish(snap({ ...pending, ready: true }));
      expect(raf1.cancelSpy).toHaveBeenCalled();
      expect(raf1.pendingCount()).toBe(0);
    } finally {
      raf1.restore();
      first.unmount();
    }

    // 2. Cleanup on fail / cancel
    const second = await open(snap(pending), "Set up");
    const raf2 = captureAnimationFrames();
    try {
      second.api.emit({ stage: "libraries", itemsDone: 1, itemsTotal: 10, overall: 0.1 });
      second.api.emit({ stage: "libraries", itemsDone: 2, itemsTotal: 10, overall: 0.2 });
      expect(raf2.pendingCount()).toBe(1);
      await second.api.fail(new BackendError("Setup was cancelled.", "cancelled"));
      expect(raf2.cancelSpy).toHaveBeenCalled();
      expect(raf2.pendingCount()).toBe(0);
    } finally {
      raf2.restore();
      second.unmount();
    }

    // 3. Cleanup on unmount
    const third = await open(snap(pending), "Set up");
    const raf3 = captureAnimationFrames();
    try {
      third.api.emit({ stage: "libraries", itemsDone: 1, itemsTotal: 10, overall: 0.1 });
      third.api.emit({ stage: "libraries", itemsDone: 2, itemsTotal: 10, overall: 0.2 });
      expect(raf3.pendingCount()).toBe(1);
      third.unmount();
      expect(raf3.cancelSpy).toHaveBeenCalled();
      expect(raf3.pendingCount()).toBe(0);
    } finally {
      raf3.restore();
    }
  });

  it("starts the setup exactly once even when effects run twice (development StrictMode)", async () => {
    const api = makeApi(snap(pending));
    render(
      <StrictMode>
        <LauncherHome api={api} accountsClient={readyAccountsClient()} />
      </StrictMode>,
    );
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Set up" }));
    await screen.findByRole("dialog");
    await flush();
    expect(api.prepare).toHaveBeenCalledTimes(1);
  });

  it("ignores progress that belongs to another profile", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    api.emit({ instanceId: "someone-else", label: "Downloading assets", overall: 0.9 });
    expect(dialog.textContent).not.toContain("Downloading assets");
  });

  it("does not dismiss on backdrop click while preparing", async () => {
    const { api } = await open(snap(pending), "Set up");
    const overlay = document.querySelector('[data-slot="dialog-overlay"]');
    expect(overlay).toBeTruthy();
    fireEvent.pointerDown(overlay!);
    fireEvent.click(overlay!);
    await flush();
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(api.cancelPrepare).not.toHaveBeenCalled();
  });

  it("Cancel requests cancellation asynchronously, closes only when prepare settles as cancelled, and leaves the profile to set up", async () => {
    const { api } = await open(snap(pending), "Set up");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(api.cancelPrepare).toHaveBeenCalledTimes(1));
    expect(api.isCancelRequested()).toBe(true);
    // Cancel only sets the backend flag; the dialog stays open until prepare unwinds.
    expect(screen.getByRole("dialog")).toBeTruthy();

    await api.fail(new BackendError("Setup was cancelled.", "cancelled"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("ignores cancelPrepare IPC rejection and still closes when prepare unwinds as cancelled", async () => {
    const { api } = await open(snap(pending), "Set up");
    api.cancelPrepare.mockRejectedValueOnce(new Error("ipc failed"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(api.cancelPrepare).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("dialog")).toBeTruthy();

    await api.fail(new BackendError("Setup was cancelled.", "cancelled"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("Escape closes immediately, requests cancellation, and ignores late prepare completion after dismissal", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.cancelPrepare).toHaveBeenCalledTimes(1);

    // Late Ok resolution after Escape dismissal must not reopen or mark the profile ready.
    await api.finish(snap({ ...pending, ready: true }));
    await flush();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("ignores late prepare rejection after Escape dismissal", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.cancelPrepare).toHaveBeenCalledTimes(1);

    await api.fail(new BackendError("Offline", "offline"));
    await flush();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("shows the busy error when reopened while a cancelled run is still unwinding, then succeeds on retry", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(screen.getByRole("button", { name: "Set up" }));
    const reopened = await screen.findByRole("dialog");
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(2));
    expect(await within(reopened).findByRole("alert")).toBeTruthy();
    expect(reopened.textContent).toContain(
      "Another profile is being set up. Wait for it to finish.",
    );

    // The earlier cancelled run finishes unwinding, freeing the backend lock.
    await api.fail(new BackendError("Setup was cancelled.", "cancelled"));
    fireEvent.click(within(reopened).getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(3));
    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(reopened.textContent).toContain("Ready to play."));
  });

  it("shows a failure in plain words and lets the person try again", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    await api.fail(
      new BackendError(
        "Can't reach the network, and some of the files it needs isn't saved on this computer yet.",
        "offline",
      ),
    );
    expect(dialog.textContent).toContain("Can’t reach the network");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(2));
    expect(dialog.textContent).toContain("Preparing Minecraft");
    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("Ready to play."));
  });

  it("lets the person close the dialog from the error state without cancelling", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    await api.fail(new BackendError("Download failed", "network"));
    expect(within(dialog).getByRole("alert").textContent).toContain("A download failed");
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.cancelPrepare).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("interpolates the loader name for a modded profile during the loader stage", async () => {
    const { api, dialog } = await open(snap(moddedPending), "Set up");
    api.emit({ stage: "loader" });
    expect(dialog.textContent).toContain("Installing Fabric…");
  });

  it("triggers verification from the profile menu, shows repairing state on the dock, and words verifying stages", async () => {
    const readyProfile: ProfileSummary = { ...pending, ready: true };
    const api = makeApi(snap(readyProfile));
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();

    fireEvent.click(
      await screen.findByRole("button", { name: "Profile: Survival. Change profile" }),
    );
    fireEvent.click(await screen.findByRole("menuitem", { name: "Verify files" }));

    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(api.prepare).toHaveBeenCalledWith("survival", { verify: true }));

    const dockPlay = screen.getByRole<HTMLButtonElement>("button", {
      name: "Repairing…",
      hidden: true,
    });
    expect(dockPlay.disabled).toBe(true);
    expect(dockPlay.dataset.state).toBe("repairing");

    api.emit({ stage: "java" });
    expect(dialog.textContent).toContain("Verifying Java…");
    api.emit({ stage: "minecraft" });
    expect(dialog.textContent).toContain("Verifying Minecraft…");
    api.emit({ stage: "libraries" });
    expect(dialog.textContent).toContain("Verifying libraries…");
    api.emit({ stage: "assets" });
    expect(dialog.textContent).toContain("Verifying assets…");

    await api.finish(snap(readyProfile));
    await waitFor(() => expect(dialog.textContent).toContain("Ready to play."));
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Play" })).toBeTruthy();
  });

  it("with no profile at all, the primary action starts creating one", async () => {
    const api = makeApi({ profiles: [], selected: null, totalPlaytimeSeconds: 0 });
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("New profile");
    expect(api.prepare).not.toHaveBeenCalled();
  });
});

describe("preparing in Russian", () => {
  it("words stages, counts, sizes and the ready state in Russian", async () => {
    setLocale("ru");
    const { api, dialog } = await open(snap(pending), "Подготовить");
    api.emit({
      stage: "libraries",
      itemsDone: 412,
      itemsTotal: 640,
      bytesDone: 184 * 1024 * 1024,
      bytesTotal: 521 * 1024 * 1024,
      overall: 0.4,
    });
    expect(dialog.textContent).toContain("Загрузка библиотек…");
    expect(dialog.textContent).toContain("412 из 640 файлов");
    expect(dialog.textContent).toContain("184 МБ из 521 МБ");
    expect(screen.getByRole("progressbar", { name: "Ход подготовки" })).toBeTruthy();
    api.emit({ stage: "natives" });
    expect(dialog.textContent).toContain("Распаковка нативных библиотек…");
    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("Готово к игре."));
    expect(screen.getByRole("dialog").textContent).toContain("установлен на этом компьютере");
  });

  it("interpolates the loader name for a modded profile in Russian", async () => {
    setLocale("ru");
    const { api, dialog } = await open(snap(moddedPending), "Подготовить");
    api.emit({ stage: "loader" });
    expect(dialog.textContent).toContain("Установка Fabric…");
  });

  it("words verification stages and the repairing dock state in Russian", async () => {
    setLocale("ru");
    const readyProfile: ProfileSummary = { ...pending, ready: true };
    const api = makeApi(snap(readyProfile));
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();

    fireEvent.click(
      await screen.findByRole("button", { name: "Профиль: Survival. Сменить профиль" }),
    );
    fireEvent.click(await screen.findByRole("menuitem", { name: "Проверить файлы" }));

    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(api.prepare).toHaveBeenCalledWith("survival", { verify: true }));

    const dockPlay = screen.getByRole<HTMLButtonElement>("button", {
      name: "Восстановление…",
      hidden: true,
    });
    expect(dockPlay.disabled).toBe(true);
    expect(dockPlay.dataset.state).toBe("repairing");

    api.emit({ stage: "java" });
    expect(dialog.textContent).toContain("Проверка Java…");
    api.emit({ stage: "minecraft" });
    expect(dialog.textContent).toContain("Проверка Minecraft…");
    api.emit({ stage: "libraries" });
    expect(dialog.textContent).toContain("Проверка библиотек…");
    api.emit({ stage: "assets" });
    expect(dialog.textContent).toContain("Проверка ресурсов…");

    await api.finish(snap(readyProfile));
    await waitFor(() => expect(dialog.textContent).toContain("Готово к игре."));
  });

  it("explains a full disk with the real sizes, in Russian, without the backend's English text", async () => {
    setLocale("ru");
    const { api, dialog } = await open(snap(pending), "Подготовить");
    await api.fail(
      new BackendError("English detail that must never be shown", "disk_space", {
        params: { requiredBytes: 3 * 1024 ** 3, availableBytes: 200 * 1024 ** 2 },
      }),
    );
    expect(dialog.textContent).toContain("не хватает места: нужно около 3,0 ГБ, доступно 200 МБ");
    expect(dialog.textContent).not.toContain("English detail");
    fireEvent.click(screen.getByRole("button", { name: "Повторить" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(2));
  });

  it("switching language while preparing re-words the open dialog in place", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    api.emit({ stage: "assets", itemsDone: 1, itemsTotal: 2 });
    expect(dialog.textContent).toContain("Downloading assets…");
    act(() => setLocale("ru"));
    expect(screen.getByRole("dialog")).toBe(dialog);
    expect(dialog.textContent).toContain("Загрузка ресурсов…");
    expect(dialog.textContent).toContain("1 из 2 файлов");
    expect(api.prepare).toHaveBeenCalledTimes(1); // nothing restarted
  });
});
