import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readyAccountsClient } from "@/test/testAccounts";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type { LaunchApi } from "@/lib/launch/api";
import {
  profilesApi,
  type ProfileSummary,
  type ProfilesApi,
  type ProfilesSnapshot,
  type RemovedProfileSummary,
} from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";
import { RemovedProfilesDialog } from "./RemovedProfilesDialog";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const profile = (
  id: string,
  name: string,
  over: Partial<Omit<ProfileSummary, "id" | "name">> = {},
): ProfileSummary => ({
  id,
  name,
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 0,
  ...over,
});

const removedProfile = (
  folder: string,
  name: string,
  over: Partial<Omit<RemovedProfileSummary, "folder" | "name">> = {},
): RemovedProfileSummary => ({
  folder,
  name,
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  playtimeSeconds: 0,
  removedAt: 1_700_000_000,
  ...over,
});

const snap = (selected: string | null, ...profiles: ProfileSummary[]): ProfilesSnapshot => ({
  profiles,
  selected,
  totalPlaytimeSeconds: 0,
});

const RESERVED_IDS = new Set([
  "con",
  "prn",
  "aux",
  "nul",
  ...Array.from({ length: 10 }, (_, i) => `com${i}`),
  ...Array.from({ length: 10 }, (_, i) => `lpt${i}`),
]);

function slug(name: string): string {
  let s = "";
  for (const ch of name) {
    if (/^[a-zA-Z0-9]$/.test(ch)) {
      s += ch.toLowerCase();
    } else if (!s.endsWith("-")) {
      s += "-";
    }
  }
  const trimmed = s.replace(/^-+|-+$/g, "");
  const capped = [...trimmed]
    .slice(0, 48)
    .join("")
    .replace(/^-+|-+$/g, "");
  const base = capped.length === 0 ? "instance" : capped;
  return RESERVED_IDS.has(base) ? `i-${base}` : base;
}

function stripCopySuffix(name: string): string {
  const trimmed = name.trim();
  const open = trimmed.lastIndexOf(" (");
  if (open !== -1 && trimmed.endsWith(")")) {
    const digits = trimmed.slice(open + 2, -1);
    if (/^\d+$/.test(digits) && Number(digits) >= 2) {
      const stem = trimmed.slice(0, open).trimEnd();
      if (stem.length > 0) return stem;
    }
  }
  return trimmed;
}

function nextDuplicateName(sourceName: string, current: ProfilesSnapshot): string {
  const stem = stripCopySuffix(sourceName);
  const stemChars = [...stem];
  const hasConflict = (candidate: string) => {
    const wanted = candidate.trim().toLowerCase();
    if (current.profiles.some((p) => p.name.trim().toLowerCase() === wanted)) return true;
    const s = slug(candidate).toLowerCase();
    return (current.unreadable ?? []).some((u) => u.toLowerCase() === s);
  };
  for (let n = 2; n <= 9999; n += 1) {
    const suffix = ` (${n})`;
    const maxStem = Math.max(0, 64 - [...suffix].length);
    const head = stemChars.slice(0, maxStem).join("").trimEnd();
    const candidate = head.length === 0 ? `Profile${suffix}` : `${head}${suffix}`;
    if (!hasConflict(candidate)) return candidate;
  }
  return "Profile (copy)";
}

function allocateId(baseId: string, current: ProfilesSnapshot): string {
  const exists = (candidate: string) => {
    const lower = candidate.toLowerCase();
    return (
      current.profiles.some((p) => p.id.toLowerCase() === lower) ||
      (current.unreadable ?? []).some((u) => u.toLowerCase() === lower)
    );
  };
  let target = baseId;
  let k = 2;
  while (exists(target)) {
    target = `${baseId}-${k}`;
    k += 1;
  }
  return target;
}

interface SetupOptions {
  running?: string;
  preparing?: string;
  signedIn?: boolean;
  failWith?: BackendError;
}

/** A backend double matching Rust's `remove_profile` and `restore_profile` semantics. */
function makeProfilesApi(initial: ProfilesSnapshot, opts: SetupOptions = {}) {
  let current = initial;
  const removedExtras = new Map<
    string,
    Pick<ProfileSummary, "ready" | "memoryMb" | "resolution" | "jvmArgs">
  >();
  return {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(current)),
    select: vi.fn<ProfilesApi["select"]>((id) => {
      current = { ...current, selected: id };
      return Promise.resolve();
    }),
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
    prepare: vi.fn<ProfilesApi["prepare"]>(() => new Promise<ProfilesSnapshot>(() => undefined)),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve()),
    remove: vi.fn<ProfilesApi["remove"]>((id) => {
      if (opts.failWith) return Promise.reject(opts.failWith);
      if (opts.running === id) {
        return Promise.reject(
          new BackendError("This profile is starting or running.", "already_running"),
        );
      }
      if (opts.preparing === id) {
        return Promise.reject(new BackendError("This profile is being set up.", "busy"));
      }
      const gone = current.profiles.find((p) => p.id === id);
      if (!gone) return Promise.reject(new BackendError("No such profile.", "not_found"));
      const profiles = current.profiles.filter((p) => p.id !== id);
      let folder = `${gone.id}-1700000000-01020304`;
      let suffix = 1;
      while ((current.removed ?? []).some((r) => r.folder === folder)) {
        folder = `${gone.id}-1700000000-${(0x01020304 + suffix).toString(16).padStart(8, "0")}`;
        suffix += 1;
      }
      removedExtras.set(folder, {
        ready: gone.ready,
        memoryMb: gone.memoryMb,
        resolution: gone.resolution,
        jvmArgs: gone.jvmArgs,
      });
      const removed = [
        {
          folder,
          name: gone.name,
          minecraft: gone.minecraft,
          kind: gone.kind,
          loader: gone.loader,
          loaderVersion: gone.loaderVersion,
          playtimeSeconds: gone.playtimeSeconds,
          removedAt: 1_700_000_000,
        },
        ...(current.removed ?? []),
      ];
      // The backend keeps the active selection if it still exists, otherwise picks the first remaining profile.
      const selected =
        current.selected &&
        current.selected !== id &&
        profiles.some((p) => p.id === current.selected)
          ? current.selected
          : (profiles[0]?.id ?? null);
      current = { ...current, profiles, removed, selected };
      return Promise.resolve(current);
    }),
    restore: vi.fn<NonNullable<ProfilesApi["restore"]>>((folder) => {
      const list = [...(current.removed ?? [])];
      const idx = list.findIndex((r) => r.folder === folder);
      if (idx === -1) return Promise.reject(new BackendError("No such profile.", "not_found"));
      const [item] = list.splice(idx, 1);
      const extras = removedExtras.get(folder);
      removedExtras.delete(folder);
      const hasNameConflict = current.profiles.some(
        (p) => p.name.trim().toLowerCase() === item.name.trim().toLowerCase(),
      );
      const finalName = hasNameConflict ? nextDuplicateName(item.name, current) : item.name;
      const origId = item.folder.replace(/-\d+-[0-9a-f]+$/i, "") || slug(item.name);
      const baseId = finalName.toLowerCase() === item.name.toLowerCase() ? origId : slug(finalName);
      const targetId = allocateId(baseId, current);
      const restored: ProfileSummary = {
        ...extras,
        id: targetId,
        name: finalName,
        minecraft: item.minecraft,
        kind: item.kind,
        loader: item.loader,
        loaderVersion: item.loaderVersion,
        ready: extras?.ready ?? true,
        playtimeSeconds: item.playtimeSeconds,
      };
      current = {
        ...current,
        profiles: [...current.profiles, restored],
        removed: list,
        selected: restored.id,
      };
      return Promise.resolve(current);
    }),
    openRemovedFolder: vi.fn<NonNullable<ProfilesApi["openRemovedFolder"]>>(() =>
      Promise.resolve(),
    ),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  } satisfies ProfilesApi;
}

function setup(initial: ProfilesSnapshot, opts: SetupOptions = {}) {
  const api = makeProfilesApi(initial, opts);
  const launch = {
    launch: vi.fn<LaunchApi["launch"]>(() => Promise.resolve()),
    status: vi.fn<LaunchApi["status"]>(() =>
      Promise.resolve(
        opts.running ? [{ instanceId: opts.running, state: "playing" as const }] : [],
      ),
    ),
    openLogs: vi.fn<LaunchApi["openLogs"]>(() => Promise.resolve()),
    onState: vi.fn<LaunchApi["onState"]>(() => Promise.resolve(() => undefined)),
  } satisfies LaunchApi;
  const accounts = readyAccountsClient(
    opts.signedIn === false
      ? { accounts: [], active: null, capabilities: { signIn: true, skin: false, cape: false } }
      : undefined,
  );
  render(<LauncherHome api={api} accountsClient={accounts} launchClient={launch} />);
  return { api, launch };
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

const openMenu = async (name: RegExp = /Profile: /) => {
  fireEvent.click(await screen.findByRole("button", { name }));
  return screen.findByRole("menu");
};

beforeEach(() => {
  setLocale("en");
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

describe("Remove from Clade", () => {
  it("is a pinned, separated action of the profile menu; profile rows carry no buttons of their own", async () => {
    setup(snap("a", profile("a", "Alpha"), profile("b", "Beta")));
    const menu = await openMenu();
    const items = within(menu)
      .getAllByRole("menuitem")
      .map((i) => i.textContent);
    expect(items).toEqual([
      "+ Create profile",
      "Profile settings…",
      "Open profile folder",
      "Verify files",
      "Remove from Clade…",
    ]);
    // two separators: the list | the normal actions | the removal
    expect(within(menu).getAllByRole("separator")).toHaveLength(2);
    expect(within(menu).queryAllByRole("button")).toHaveLength(0); // no per-row trash buttons
    expect(within(menu).getAllByRole("menuitemradio")).toHaveLength(2);
  });

  it("asks first, names the profile, and says nothing is deleted; Cancel changes nothing", async () => {
    const { api } = setup(snap("a", profile("a", "Alpha"), profile("b", "Beta")));
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Remove “Alpha” from Clade?");
    expect(dialog.textContent).toContain(
      "The profile will disappear from Clade, but its worlds, mods, settings, screenshots and other files will be kept inside the Clade folder. Nothing will be permanently deleted.",
    );
    expect(dialog.textContent).not.toMatch(/delete profile|erase|cannot be undone/i);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.remove).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Profile: Alpha. Change profile" })).toBeTruthy();
  });

  it("removes ONLY the selected profile and immediately selects a remaining one", async () => {
    const { api } = setup(
      snap("b", profile("a", "Alpha"), profile("b", "Beta"), profile("c", "Gamma")),
    );
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    expect((await screen.findByRole("dialog")).textContent).toContain("“Beta”");
    fireEvent.click(screen.getByRole("button", { name: "Remove from Clade" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.remove).toHaveBeenCalledTimes(1);
    expect(api.remove).toHaveBeenCalledWith("b");
    // gone from the list at once, a sane profile is selected, no restart or reload needed
    const trigger = screen.getByRole("button", { name: "Profile: Alpha. Change profile" });
    expect(trigger.textContent).toBe("Alpha");
    await openMenu(/Profile: Alpha/);
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual([
      expect.stringContaining("Alpha"),
      expect.stringContaining("Gamma"),
    ]);
  });

  it("removing the last profile returns to the empty-profile state", async () => {
    setup(snap("a", profile("a", "Only")));
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Remove from Clade" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: /Create a profile/ })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Profile: / })).toBeNull();
  });

  it("explains a refusal in words and keeps the profile (nothing was changed)", async () => {
    setup(snap("a", profile("a", "Alpha")), {
      failWith: new BackendError("busy", "already_running"),
    });
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Remove from Clade" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("This profile is starting or running. Close the game first.");
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove from Clade" })).toHaveProperty(
      "disabled",
      false,
    );
  });

  it("locks RemoveProfileDialog while removal is in flight, suppresses duplicate clicks, and recovers on error", async () => {
    const { api } = setup(snap("a", profile("a", "Alpha")));
    const pending = deferred<ProfilesSnapshot>();
    api.remove.mockImplementationOnce(() => pending.promise);

    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    const dialog = await screen.findByRole("dialog");

    const confirmBtn = within(dialog).getByRole("button", { name: "Remove from Clade" });
    fireEvent.click(confirmBtn);
    fireEvent.click(confirmBtn);

    const removingBtn = await within(dialog).findByRole<HTMLButtonElement>("button", {
      name: "Removing…",
    });
    const cancelBtn = within(dialog).getByRole<HTMLButtonElement>("button", { name: "Cancel" });
    expect(removingBtn.disabled).toBe(true);
    expect(cancelBtn.disabled).toBe(true);

    // Clicking again or pressing Escape while locked does not dismiss or re-submit.
    fireEvent.click(removingBtn);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(api.remove).toHaveBeenCalledTimes(1);

    await act(async () => {
      pending.reject(new BackendError("folder in use", "remove_failed"));
      await Promise.resolve();
    });

    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toBe(
      "The profile could not be moved. Close anything that uses its folder (Explorer, an editor) and try again. Nothing was changed.",
    );
    expect(
      within(dialog).getByRole<HTMLButtonElement>("button", { name: "Remove from Clade" }).disabled,
    ).toBe(false);
    expect(within(dialog).getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(
      false,
    );

    // Once unlocked, Escape closes the dialog without touching the profile.
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Profile: Alpha. Change profile" })).toBeTruthy();
  });

  it("works without any account, and is unavailable while the profile's game runs", async () => {
    setup(snap("a", profile("a", "Alpha")), { signedIn: false });
    await screen.findByRole("button", { name: "Sign in to continue" });
    await openMenu();
    expect(
      screen.getByRole("menuitem", { name: "Remove from Clade…" }).hasAttribute("data-disabled"),
    ).toBe(false);
    expect(
      screen.getByRole("menuitem", { name: "Verify files" }).hasAttribute("data-disabled"),
    ).toBe(true);
    cleanup();

    setup(snap("a", profile("a", "Alpha")), { running: "a" });
    await screen.findByRole("button", { name: "Playing" });
    await openMenu();
    expect(
      screen.getByRole("menuitem", { name: "Remove from Clade…" }).hasAttribute("data-disabled"),
    ).toBe(true);
  });

  it("keeps selection unchanged when a non-selected profile is removed, and refuses running or preparing profiles", async () => {
    const api = makeProfilesApi(
      snap("b", profile("a", "Alpha"), profile("b", "Beta"), profile("c", "Gamma")),
    );
    const afterRemoveA = await api.remove("a");
    expect(afterRemoveA.selected).toBe("b");
    expect(afterRemoveA.profiles.map((p) => p.id)).toEqual(["b", "c"]);
    expect(afterRemoveA.removed?.map((r) => r.name)).toEqual(["Alpha"]);

    const runningApi = makeProfilesApi(snap("a", profile("a", "Alpha")), { running: "a" });
    await expect(runningApi.remove("a")).rejects.toMatchObject({ code: "already_running" });
    expect((await runningApi.list()).profiles.map((p) => p.id)).toEqual(["a"]);

    const preparingApi = makeProfilesApi(snap("a", profile("a", "Alpha")), { preparing: "a" });
    await expect(preparingApi.remove("a")).rejects.toMatchObject({ code: "busy" });
    expect((await preparingApi.list()).profiles.map((p) => p.id)).toEqual(["a"]);
  });

  it("is worded in Russian", async () => {
    setLocale("ru");
    const { api } = setup(snap("a", profile("a", "Альфа"), profile("b", "Бета")));
    await openMenu(/Профиль: /);
    fireEvent.click(screen.getByRole("menuitem", { name: "Убрать из Clade…" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Убрать «Альфа» из Clade?");
    expect(dialog.textContent).toContain(
      "Профиль исчезнет из Clade, но его миры, моды, настройки, скриншоты и другие файлы останутся сохранены в папке Clade. Ничего не будет удалено безвозвратно.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Убрать из Clade" }));
    await waitFor(() => expect(api.remove).toHaveBeenCalledWith("a"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await openMenu(/Профиль: Бета/);
    expect(screen.getByRole("menuitem", { name: "Убранные профили (1)…" })).toBeTruthy();
  });

  it("the interface has no way to erase a profile's files: the only call is remove", () => {
    const realNames = Object.keys(profilesApi);
    expect(realNames).toContain("remove");
    expect(realNames).toContain("restore");
    expect(realNames.filter((n) => /delete|erase|purge/i.test(n))).toEqual([]);

    const doubleNames = Object.keys(makeProfilesApi(snap("a", profile("a", "Alpha"))));
    expect(doubleNames.filter((n) => /delete|erase|purge/i.test(n))).toEqual([]);
    expect(doubleNames).toEqual(expect.arrayContaining(realNames));
  });

  it("restores a removed profile from the menu and from the empty-profile banner, and opens the retained folder", async () => {
    const { api } = setup(snap("a", profile("a", "Alpha"), profile("b", "Beta")));
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Remove from Clade" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await openMenu(/Profile: Beta/);
    fireEvent.click(screen.getByRole("menuitem", { name: "Removed profiles (1)…" }));
    const removedDialog = await screen.findByRole("dialog", { name: "Removed profiles" });
    expect(removedDialog.textContent).toContain("Alpha");

    fireEvent.click(screen.getByRole("button", { name: "Open folder" }));
    expect(api.openRemovedFolder).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Restore" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.restore).toHaveBeenCalledWith("a-1700000000-01020304");
    expect(screen.getByRole("button", { name: "Profile: Alpha. Change profile" })).toBeTruthy();

    // Remove both profiles: the empty-state notice leads into the restore dialog too.
    for (const label of [/Profile: Alpha/, /Profile: Beta/]) {
      await openMenu(label);
      fireEvent.click(screen.getByRole("menuitem", { name: "Remove from Clade…" }));
      await screen.findByRole("dialog");
      fireEvent.click(screen.getByRole("button", { name: "Remove from Clade" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
    expect(screen.getByText("2 removed profiles are kept in this Clade folder.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Restore…" }));
    await screen.findByRole("dialog", { name: "Removed profiles" });
    const restoreButtons = screen.getAllByRole("button", { name: "Restore" });
    expect(restoreButtons).toHaveLength(2);
    fireEvent.click(restoreButtons[0]);
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Restore" })).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: "Profile: Beta. Change profile" })).toBeTruthy();
  });

  it("restores modded metadata and playtime, and resolves name and id collisions", async () => {
    const initial: ProfilesSnapshot = {
      profiles: [profile("alpha-live", "Alpha"), profile("beta", "Unrelated")],
      selected: "alpha-live",
      totalPlaytimeSeconds: 3840,
      removed: [
        removedProfile("alpha-1700000000-01020304", "Alpha", {
          minecraft: "1.20.1",
          kind: "modded",
          loader: "fabric",
          loaderVersion: "0.16.9",
          playtimeSeconds: 3660,
        }),
        removedProfile("beta-1700000000-01020305", "Beta", {
          minecraft: "1.21.1",
          kind: "modded",
          loader: "forge",
          loaderVersion: "52.0.1",
          playtimeSeconds: 180,
        }),
      ],
    };
    const { api } = setup(initial);
    await openMenu(/Profile: Alpha/);
    fireEvent.click(screen.getByRole("menuitem", { name: "Removed profiles (2)…" }));
    const dialog = await screen.findByRole("dialog", { name: "Removed profiles" });

    // First row ("Alpha") collides by name with active "Alpha" -> becomes "Alpha (2)" / id "alpha-2".
    fireEvent.click(within(dialog).getAllByRole("button", { name: "Restore" })[0]);
    await waitFor(() =>
      expect(within(dialog).getAllByRole("button", { name: "Restore" })).toHaveLength(1),
    );
    expect(api.restore).toHaveBeenCalledWith("alpha-1700000000-01020304");
    const afterFirst = await api.list();
    expect(afterFirst.profiles.find((p) => p.name === "Alpha (2)")).toMatchObject({
      id: "alpha-2",
      name: "Alpha (2)",
      minecraft: "1.20.1",
      kind: "modded",
      loader: "fabric",
      loaderVersion: "0.16.9",
      playtimeSeconds: 3660,
    });

    // Second row ("Beta") keeps its name, and resolves its occupied id "beta" -> "beta-2".
    fireEvent.click(within(dialog).getByRole("button", { name: "Restore" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const afterSecond = await api.list();
    expect(afterSecond.profiles.find((p) => p.name === "Beta")).toMatchObject({
      id: "beta-2",
      name: "Beta",
      minecraft: "1.21.1",
      kind: "modded",
      loader: "forge",
      loaderVersion: "52.0.1",
      playtimeSeconds: 180,
    });

    const menu = await openMenu(/Profile: Beta/);
    const rows = within(menu)
      .getAllByRole("menuitemradio")
      .map((item) => item.textContent);
    expect(rows[2]).toContain("Alpha (2)");
    expect(rows[2]).toContain("Fabric 1.20.1 · 0.16.9");
    expect(rows[3]).toContain("Beta");
    expect(rows[3]).toContain("Forge 1.21.1 · 52.0.1");
  });

  it("locks RemovedProfilesDialog while a restore is in flight and unlocks when it resolves", async () => {
    const initial: ProfilesSnapshot = {
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      removed: [
        removedProfile("a-1700000000-01020304", "Alpha"),
        removedProfile("b-1700000000-01020305", "Beta"),
      ],
    };
    const { api } = setup(initial);
    const pending = deferred<ProfilesSnapshot>();
    const resolved = await makeProfilesApi(initial).restore("a-1700000000-01020304");
    api.restore.mockImplementationOnce(() => pending.promise);

    fireEvent.click(await screen.findByRole("button", { name: "Restore…" }));
    const dialog = await screen.findByRole("dialog", { name: "Removed profiles" });
    fireEvent.click(within(dialog).getAllByRole("button", { name: "Restore" })[0]);

    const restoringBtn = await within(dialog).findByRole<HTMLButtonElement>("button", {
      name: "Restoring…",
    });
    const remainingBtn = within(dialog).getByRole<HTMLButtonElement>("button", { name: "Restore" });
    const openFolderBtn = within(dialog).getByRole<HTMLButtonElement>("button", {
      name: "Open folder",
    });
    const doneBtn = within(dialog).getByRole<HTMLButtonElement>("button", { name: "Done" });

    expect(restoringBtn.disabled).toBe(true);
    expect(remainingBtn.disabled).toBe(true);
    expect(openFolderBtn.disabled).toBe(true);
    expect(doneBtn.disabled).toBe(true);

    fireEvent.click(remainingBtn);
    fireEvent.click(openFolderBtn);
    fireEvent.click(doneBtn);
    fireEvent.keyDown(dialog, { key: "Escape" });

    expect(screen.getByRole("dialog", { name: "Removed profiles" })).toBeTruthy();
    expect(api.restore).toHaveBeenCalledTimes(1);
    expect(api.openRemovedFolder).not.toHaveBeenCalled();

    await act(async () => {
      pending.resolve(resolved);
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(within(dialog).queryByRole("button", { name: "Restoring…" })).toBeNull(),
    );
    expect(
      within(dialog).getAllByRole<HTMLButtonElement>("button", { name: "Restore" }),
    ).toHaveLength(1);
    expect(
      within(dialog).getByRole<HTMLButtonElement>("button", { name: "Restore" }).disabled,
    ).toBe(false);
    expect(openFolderBtn.disabled).toBe(false);
    expect(doneBtn.disabled).toBe(false);

    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("shows inline alerts and clears busy state when restore or openRemovedFolder rejects", async () => {
    const { api } = setup({
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      removed: [
        removedProfile("a-1700000000-01020304", "Alpha"),
        removedProfile("b-1700000000-01020305", "Beta"),
      ],
    });
    api.openRemovedFolder.mockRejectedValueOnce(new BackendError("cannot open", "open_failed"));
    api.restore
      .mockRejectedValueOnce(new BackendError("file locked", "restore_failed"))
      .mockRejectedValueOnce(new BackendError("missing", "not_found"));

    fireEvent.click(await screen.findByRole("button", { name: "Restore…" }));
    const dialog = await screen.findByRole("dialog", { name: "Removed profiles" });

    fireEvent.click(within(dialog).getByRole("button", { name: "Open folder" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Windows couldn’t open that folder.",
    );

    // Retrying open folder clears the previous alert once it succeeds.
    fireEvent.click(within(dialog).getByRole("button", { name: "Open folder" }));
    await waitFor(() => expect(within(dialog).queryByRole("alert")).toBeNull());

    const restoreButtons = within(dialog).getAllByRole<HTMLButtonElement>("button", {
      name: "Restore",
    });
    fireEvent.click(restoreButtons[0]);
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Couldn’t restore the profile. Make sure none of its files are open in another program.",
    );
    expect(within(dialog).queryByRole("button", { name: "Restoring…" })).toBeNull();
    for (const btn of within(dialog).getAllByRole<HTMLButtonElement>("button", {
      name: "Restore",
    })) {
      expect(btn.disabled).toBe(false);
    }
    expect(
      within(dialog).getByRole<HTMLButtonElement>("button", { name: "Open folder" }).disabled,
    ).toBe(false);
    expect(within(dialog).getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(
      false,
    );

    fireEvent.click(restoreButtons[1]);
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "That profile no longer exists.",
    );
    expect(screen.getByRole("dialog", { name: "Removed profiles" })).toBeTruthy();
  });

  it("formats modded loaders and playtime >= 60s in RemovedProfilesDialog rows, and handles an empty list", async () => {
    setup({
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      removed: [
        removedProfile("m1-1700000000-00000001", "Fabric Pack", {
          minecraft: "1.20.1",
          kind: "modded",
          loader: "fabric",
          loaderVersion: "0.16.9",
          playtimeSeconds: 3660,
        }),
        removedProfile("m2-1700000000-00000002", "Forge Draft", {
          minecraft: "1.21.1",
          kind: "modded",
          loader: "forge",
          loaderVersion: null,
          playtimeSeconds: 59,
        }),
        removedProfile("v1-1700000000-00000003", "Long Vanilla", {
          minecraft: "1.21.4",
          kind: "vanilla",
          playtimeSeconds: 60,
        }),
        removedProfile("v2-1700000000-00000004", "Quick Vanilla", {
          minecraft: "1.21.4",
          kind: "vanilla",
          playtimeSeconds: 0,
        }),
      ],
    });

    fireEvent.click(await screen.findByRole("button", { name: "Restore…" }));
    const dialog = await screen.findByRole("dialog", { name: "Removed profiles" });
    expect(within(dialog).getByText("Fabric Pack")).toBeTruthy();
    expect(within(dialog).getByText("Fabric 1.20.1 · 0.16.9 · 1h 1m")).toBeTruthy();
    expect(within(dialog).getByText("Forge Draft")).toBeTruthy();
    expect(within(dialog).getByText("Forge 1.21.1")).toBeTruthy();
    expect(within(dialog).getByText("Long Vanilla")).toBeTruthy();
    expect(within(dialog).getByText("Vanilla 1.21.4 · 1m")).toBeTruthy();
    expect(within(dialog).getByText("Quick Vanilla")).toBeTruthy();
    expect(within(dialog).getByText("Vanilla 1.21.4")).toBeTruthy();

    const items = within(dialog).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(within(items[0]).getByRole("button", { name: "Restore" })).toHaveProperty(
      "disabled",
      false,
    );
    cleanup();

    render(
      <RemovedProfilesDialog
        open
        removed={[]}
        onClose={vi.fn()}
        onRestored={vi.fn()}
        api={makeProfilesApi(snap(null))}
      />,
    );
    const emptyDialog = await screen.findByRole("dialog", { name: "Removed profiles" });
    expect(within(emptyDialog).getByText("No removed profiles.")).toBeTruthy();
  });

  it("renders RemovedProfilesDialog and the empty-state banner in Russian", async () => {
    setLocale("ru");
    const oneRemoved: ProfilesSnapshot = {
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      removed: [
        removedProfile("a-1700000000-01020304", "Альфа", {
          minecraft: "1.21.4",
          kind: "vanilla",
          playtimeSeconds: 7500,
        }),
      ],
    };
    const { api } = setup(oneRemoved);
    const pending = deferred<ProfilesSnapshot>();
    api.restore.mockImplementationOnce(() => pending.promise);

    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("В папке Clade сохранён 1 убранный профиль.");
    fireEvent.click(within(status).getByRole("button", { name: "Восстановить…" }));

    const dialog = await screen.findByRole("dialog", { name: "Убранные профили" });
    expect(dialog.textContent).toContain(
      "Миры, моды и настройки нетронутыми хранятся в папке Clade. Восстановление вернёт профиль в список; открой папку, если хочешь удалить файлы вручную.",
    );
    expect(within(dialog).getByText("Альфа")).toBeTruthy();
    expect(within(dialog).getByText("Ванильный 1.21.4 · 2 ч 5 мин")).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Открыть папку" })).toBeTruthy();
    expect(within(dialog).getByRole("button", { name: "Готово" })).toBeTruthy();

    fireEvent.click(within(dialog).getByRole("button", { name: "Восстановить" }));
    expect(
      await within(dialog).findByRole<HTMLButtonElement>("button", { name: "Восстановление…" }),
    ).toHaveProperty("disabled", true);

    await act(async () => {
      pending.reject(new BackendError("locked", "restore_failed"));
      await Promise.resolve();
    });
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Не удалось восстановить профиль. Проверь, не открыты ли его файлы в другой программе.",
    );
    cleanup();

    // Plural form ("few") for 2 removed profiles in the empty-state banner.
    setup({
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      removed: [
        removedProfile("a-1700000000-01020304", "Альфа"),
        removedProfile("b-1700000000-01020305", "Бета"),
      ],
    });
    const twoStatus = await screen.findByRole("status");
    expect(twoStatus.textContent).toContain("В папке Clade сохранены 2 убранных профиля.");
  });
});
