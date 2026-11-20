import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readyAccountsClient } from "@/test/testAccounts";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type { LaunchApi } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const profile = (id: string, name: string): ProfileSummary => ({
  id,
  name,
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 0,
});
const snap = (selected: string | null, ...profiles: ProfileSummary[]): ProfilesSnapshot => ({
  profiles,
  selected,
  totalPlaytimeSeconds: 0,
});

/** A backend double that really forgets a removed profile, like the Rust command does. */
function setup(
  initial: ProfilesSnapshot,
  opts: { running?: string; signedIn?: boolean; failWith?: BackendError } = {},
) {
  let current = initial;
  const api = {
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
      const gone = current.profiles.find((p) => p.id === id);
      const profiles = current.profiles.filter((p) => p.id !== id);
      const removed = gone
        ? [
            {
              folder: `${gone.id}-1700000000-01020304`,
              name: gone.name,
              minecraft: gone.minecraft,
              kind: gone.kind,
              loader: gone.loader,
              loaderVersion: gone.loaderVersion,
              playtimeSeconds: gone.playtimeSeconds,
              removedAt: 1_700_000_000,
            },
            ...(current.removed ?? []),
          ]
        : (current.removed ?? []);
      // the backend keeps a valid selection: the next remaining profile, or none
      current = { ...current, profiles, removed, selected: profiles[0]?.id ?? null };
      return Promise.resolve(current);
    }),
    restore: vi.fn<NonNullable<ProfilesApi["restore"]>>((folder) => {
      const list = [...(current.removed ?? [])];
      const idx = list.findIndex((r) => r.folder === folder);
      if (idx === -1) return Promise.reject(new BackendError("missing", "not_found"));
      const [item] = list.splice(idx, 1);
      const id = item.folder.replace(/-\d+-[0-9a-f]+$/, "");
      const restored = profile(id, item.name);
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

const flush = async () => act(async () => void (await Promise.resolve()));
const openMenu = async (name: RegExp = /Profile: /) => {
  fireEvent.click(screen.getByRole("button", { name }));
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
    await flush();
    const menu = await openMenu();
    const items = screen.getAllByRole("menuitem").map((i) => i.textContent);
    expect(items).toEqual([
      "+ Create profile",
      "Profile settings…",
      "Open profile folder",
      "Verify files",
      "Remove from Clade…",
    ]);
    // two separators: the list | the normal actions | the removal
    expect(menu.querySelectorAll('[data-slot="dropdown-menu-separator"]').length).toBe(2);
    expect(menu.querySelectorAll("button").length).toBe(0); // no per-row trash buttons
    expect(screen.getAllByRole("menuitemradio")).toHaveLength(2);
  });

  it("asks first, names the profile, and says nothing is deleted; Cancel changes nothing", async () => {
    const { api } = setup(snap("a", profile("a", "Alpha"), profile("b", "Beta")));
    await flush();
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
    await flush();
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
    await flush();
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
    await flush();
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

  it("works without any account, and is unavailable while the profile's game runs", async () => {
    setup(snap("a", profile("a", "Alpha")), { signedIn: false });
    await flush();
    await openMenu();
    expect(
      screen.getByRole("menuitem", { name: "Remove from Clade…" }).hasAttribute("data-disabled"),
    ).toBe(false);
    cleanup();

    setup(snap("a", profile("a", "Alpha")), { running: "a" });
    await flush();
    await openMenu();
    expect(
      screen.getByRole("menuitem", { name: "Remove from Clade…" }).hasAttribute("data-disabled"),
    ).toBe(true);
  });

  it("is worded in Russian", async () => {
    setLocale("ru");
    const { api } = setup(snap("a", profile("a", "Альфа"), profile("b", "Бета")));
    await flush();
    await openMenu(/Профиль: /);
    fireEvent.click(screen.getByRole("menuitem", { name: "Убрать из Clade…" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Убрать «Альфа» из Clade?");
    expect(dialog.textContent).toContain(
      "Профиль исчезнет из Clade, но его миры, моды, настройки, скриншоты и другие файлы останутся сохранены в папке Clade. Ничего не будет удалено безвозвратно.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Убрать из Clade" }));
    await waitFor(() => expect(api.remove).toHaveBeenCalledWith("a"));
  });

  it("the interface has no way to erase a profile's files: the only call is remove", () => {
    const names = Object.keys(setup(snap("a", profile("a", "Alpha"))).api);
    expect(names.filter((n) => /delete|erase|purge/i.test(n))).toEqual([]);
  });

  it("restores a removed profile from the menu and from the empty-profile banner, and opens the retained folder", async () => {
    const { api } = setup(snap("a", profile("a", "Alpha"), profile("b", "Beta")));
    await flush();
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
});
