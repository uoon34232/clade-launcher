import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Account, AccountsSnapshot } from "@/lib/accounts/api";
import { readyAccountsClient } from "@/test/testAccounts";
import { testSkin } from "@/test/testSkin";
import { setLocale } from "@/lib/i18n";
import type { LaunchApi } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const profile = (p: Partial<ProfileSummary> = {}): ProfileSummary => ({
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: false,
  playtimeSeconds: 0,
  ...p,
});
const snap = (...profiles: ProfileSummary[]): ProfilesSnapshot => ({
  profiles,
  selected: profiles[0]?.id ?? null,
  totalPlaytimeSeconds: 0,
});
const account = (
  id: string,
  name: string,
  needsSignIn = false,
  skin: string | null = null,
): Account => ({
  id,
  username: name,
  uuid: `00000000-0000-4000-8000-00000000000${id === "a" ? 1 : 2}`,
  needsSignIn,
  skin,
});
const accountsOf = (active: string | null, ...accounts: Account[]): AccountsSnapshot => ({
  accounts,
  active,
  capabilities: { signIn: true, skin: false, cape: false },
});

function setup(profiles: ProfilesSnapshot, accounts: AccountsSnapshot) {
  const api = {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(profiles)),
    select: vi.fn<ProfilesApi["select"]>(() => Promise.resolve()),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.resolve({
        entries: [{ id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" }],
        latestRelease: "1.21.4",
        latestSnapshot: null,
        fetchedAt: 1,
        origin: "network",
        stale: false,
        networkError: null,
      }),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve([])),
    create: vi.fn<ProfilesApi["create"]>(() =>
      Promise.resolve(snap(profile({ id: "new", name: "Fresh" }))),
    ),
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
    prepare: vi.fn<ProfilesApi["prepare"]>(() => new Promise<ProfilesSnapshot>(() => undefined)),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve()),
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  } satisfies ProfilesApi;
  const launch = {
    launch: vi.fn<LaunchApi["launch"]>(() => Promise.resolve()),
    status: vi.fn<LaunchApi["status"]>(() => Promise.resolve([])),
    openLogs: vi.fn<LaunchApi["openLogs"]>(() => Promise.resolve()),
    onState: vi.fn<LaunchApi["onState"]>(() => Promise.resolve(() => undefined)),
  } satisfies LaunchApi;
  const acc = readyAccountsClient(accounts);
  render(<LauncherHome api={api} accountsClient={acc} launchClient={launch} />);
  return { api, launch, acc };
}

const flush = async () => act(async () => void (await Promise.resolve()));
const button = (name: string | RegExp) => screen.getByRole("button", { name });

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

describe("without an account that can play", () => {
  it("no accounts and no profiles: creating a profile works", async () => {
    const { api } = setup(snap(), accountsOf(null));
    await flush();
    fireEvent.click(button("Create a profile"));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("New profile")).toBeTruthy();
    await waitFor(() => expect(api.versionCatalog).toHaveBeenCalled()); // version lookup is only metadata
    expect(api.prepare).not.toHaveBeenCalled();
  });

  it("a profile never starts a download: the primary action asks to sign in and opens the sign-in", async () => {
    const { api, acc } = setup(snap(profile()), accountsOf(null));
    await flush();
    expect(button("Sign in to continue")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    fireEvent.click(button("Sign in to continue"));
    await waitFor(() => expect(acc.vaultStatus).toHaveBeenCalled());
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(api.prepare).not.toHaveBeenCalled();
  });

  it("an account that must sign in again gates set up, verify and play, keeps the profiles and re-signs the SAME account in", async () => {
    const { api, launch, acc } = setup(
      snap(profile({ ready: true })),
      accountsOf("a", account("a", "Alex", true)),
    );
    await flush();
    expect(button("Sign in to continue")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    // The profile is intact and still selectable; only game files are gated.
    fireEvent.click(button(/Profile: Survival/));
    const verify = await screen.findByRole("menuitem", { name: "Verify files" });
    expect(verify.hasAttribute("data-disabled")).toBe(true);
    fireEvent.click(verify);
    await flush();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.prepare).not.toHaveBeenCalled();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

    fireEvent.click(button("Sign in to continue"));
    await waitFor(() => expect(acc.signIn).toHaveBeenCalledWith("a")); // the same account, not a new one
    expect(launch.launch).not.toHaveBeenCalled();
  });

  it("gating follows the ACTIVE account: a ready account next to it never stands in", async () => {
    const accounts = accountsOf("a", account("a", "Alex", true), account("b", "Blake"));
    const { launch, acc } = setup(snap(profile({ ready: true })), accounts);
    await flush();
    expect(button("Sign in to continue")).toBeTruthy();
    expect(launch.launch).not.toHaveBeenCalled();
    // Choosing the account that can play is what unlocks Play (and launches with that account).
    acc.select.mockResolvedValueOnce({ ...accounts, active: "b" });
    fireEvent.click(button(/Account: Alex/));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /Blake/ }));
    await waitFor(() => expect(button("Play")).toBeTruthy());
    fireEvent.click(button("Play"));
    await waitFor(() => expect(launch.launch).toHaveBeenCalledWith("survival"));
  });

  it("signing in updates the dock at once, without restarting", async () => {
    const { acc, api } = setup(snap(profile()), accountsOf(null));
    await flush();
    acc.signIn.mockResolvedValueOnce(accountsOf("a", account("a", "Alex")));
    fireEvent.click(button("Sign in to continue"));
    await waitFor(() => expect(acc.signIn).toHaveBeenCalledWith(null));
    await waitFor(() => expect(button("Set up")).toBeTruthy());
    expect(screen.queryByRole("button", { name: "Sign in to continue" })).toBeNull();
    expect(api.prepare).not.toHaveBeenCalled(); // signing in does not start anything by itself
  });

  it("is worded in Russian too", async () => {
    setLocale("ru");
    setup(snap(profile()), accountsOf(null));
    await flush();
    expect(button("Войти для продолжения")).toBeTruthy();
  });
});

describe("with an account that can play", () => {
  it("an unprepared profile sets up, a prepared one plays", async () => {
    const a = accountsOf("a", account("a", "Alex"));
    const unprepared = setup(snap(profile()), a);
    await flush();
    fireEvent.click(button("Set up"));
    await waitFor(() => expect(unprepared.api.prepare).toHaveBeenCalled());
    expect(unprepared.api.prepare.mock.calls[0][0]).toBe("survival");
    cleanup();

    const prepared = setup(snap(profile({ ready: true })), a);
    await flush();
    fireEvent.click(button("Play"));
    await waitFor(() => expect(prepared.launch.launch).toHaveBeenCalledWith("survival"));
    expect(prepared.api.prepare).not.toHaveBeenCalled();
  });

  it("Verify files is available and runs the verification", async () => {
    const { api } = setup(snap(profile({ ready: true })), accountsOf("a", account("a", "Alex")));
    await flush();
    fireEvent.click(button(/Profile: Survival/));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Verify files" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalled());
    expect(api.prepare.mock.calls[0]).toEqual(["survival", { verify: true }]);
  });
});

describe("the account avatar", () => {
  const head = (el: HTMLElement) =>
    el.querySelector("svg[data-skin-head] image")?.getAttribute("href");
  const placeholder = (el: HTMLElement) => el.querySelectorAll("svg rect").length === 64;

  it("shows the head of the active account's own skin and follows account switching", async () => {
    const [skinA, skinB] = [testSkin(64, 64, 1), testSkin(64, 64, 2)];
    const accounts = accountsOf(
      "a",
      account("a", "Alex", false, skinA),
      account("b", "Blake", false, skinB),
    );
    const { acc } = setup(snap(profile()), accounts);
    await flush();
    const trigger = () => button(/Account: /);
    expect(head(trigger())).toBe(skinA);
    acc.select.mockResolvedValueOnce({ ...accounts, active: "b" });
    fireEvent.click(trigger());
    // In the menu every account shows its own avatar.
    const items = await screen.findAllByRole("menuitemradio");
    expect(items.map((i) => head(i))).toEqual([skinA, skinB]);
    fireEvent.click(items[1]);
    await waitFor(() => expect(head(trigger())).toBe(skinB));
  });

  it("falls back to the neutral face when an account has no skin, and never breaks the menu", async () => {
    const accounts = accountsOf(
      "a",
      account("a", "Alex", false, null),
      account("b", "Blake", false, testSkin()),
    );
    setup(snap(profile()), accounts);
    await flush();
    const trigger = button(/Account: Alex/);
    expect(placeholder(trigger)).toBe(true);
    fireEvent.click(trigger);
    const items = await screen.findAllByRole("menuitemradio");
    expect(placeholder(items[0])).toBe(true);
    expect(head(items[1])).toBe(testSkin());
  });

  it("picks up an avatar fetched in the background when the backend says the accounts changed", async () => {
    const before = accountsOf("a", account("a", "Alex", false, null));
    const after = accountsOf("a", account("a", "Alex", false, testSkin(64, 64, 7)));
    let changed: (() => void) | null = null;
    const acc = {
      ...readyAccountsClient(before),
      onChanged: vi.fn((cb: () => void) => {
        changed = cb;
        return Promise.resolve(() => undefined);
      }),
    };
    acc.list.mockResolvedValueOnce(before).mockResolvedValue(after);
    const { api, launch } = setup(snap(profile()), before);
    cleanup(); // only the apis are reused; the app below gets its own accounts client
    render(<LauncherHome api={api} accountsClient={acc} launchClient={launch} />);
    await flush();
    expect(acc.onChanged).toHaveBeenCalled();
    const trigger = button(/Account: Alex/);
    expect(placeholder(trigger)).toBe(true);
    act(() => changed?.());
    await waitFor(() => expect(head(trigger)).toBe(testSkin(64, 64, 7)));
  });
});
