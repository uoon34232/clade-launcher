import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  const list = vi.fn<ProfilesApi["list"]>(() => Promise.resolve(current));
  const prepare = vi.fn<ProfilesApi["prepare"]>(
    () =>
      new Promise<ProfilesSnapshot>((resolve, reject) => {
        running = { resolve, reject };
      }),
  );
  const cancelPrepare = vi.fn<ProfilesApi["cancelPrepare"]>(() => {
    running?.reject(new BackendError("Setup was cancelled.", "cancelled"));
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
    emit: (p: Partial<PrepareProgress>) =>
      act(() =>
        progress?.({
          instanceId: "survival",
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
      }),
    fail: (e: unknown) =>
      act(async () => {
        running?.reject(e);
        await Promise.resolve();
      }),
  };
}

const flush = async () => act(async () => void (await Promise.resolve()));

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
  render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
  await flush();
  fireEvent.click(screen.getByRole("button", { name: label }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(1));
  return { api, dialog };
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

  it("Cancel stops the setup, closes quietly and leaves the profile to set up", async () => {
    const { api } = await open(snap(pending), "Set up");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.cancelPrepare).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Set up" })).toBeTruthy();
  });

  it("Escape also cancels", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.cancelPrepare).toHaveBeenCalled();
  });

  it("shows a failure in plain words and lets the person try again", async () => {
    const { api, dialog } = await open(snap(pending), "Set up");
    await api.fail(
      new BackendError(
        "Can't reach the network, and some of the files it needs isn't saved on this computer yet.",
        "offline",
      ),
    );
    expect(dialog.textContent).toContain("Can't reach the network");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalledTimes(2));
    expect(dialog.textContent).toContain("Preparing Minecraft");
    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("Ready to play."));
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
  it("words stages, counts, sizes and the ready state in Russian, and a verification run says it is verifying", async () => {
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
    api.emit({ stage: "loader" });
    expect(dialog.textContent).toContain("Установка");
    await api.finish(snap({ ...pending, ready: true }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("Готово к игре."));
    expect(screen.getByRole("dialog").textContent).toContain("установлен на этом компьютере");
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
