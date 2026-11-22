import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  skinSize,
  type Account,
  type AccountsApi,
  type AccountsSnapshot,
} from "@/lib/accounts/api";
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
): Account => {
  const skinDimensions = skin ? skinSize(skin) : null;
  return {
    id,
    username: name,
    uuid: `00000000-0000-4000-8000-00000000000${id === "a" ? 1 : 2}`,
    needsSignIn,
    skin: skinDimensions ? skin : null,
    skinDimensions,
  };
};
const accountsOf = (active: string | null, ...accounts: Account[]): AccountsSnapshot => ({
  accounts,
  active,
  capabilities: { signIn: true, skin: true, cape: true },
});

type TestAccountsClient = ReturnType<typeof readyAccountsClient> & Pick<AccountsApi, "onChanged">;

function setup(
  profiles: ProfilesSnapshot,
  accounts: AccountsSnapshot,
  accountsClient?: TestAccountsClient,
) {
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
  const acc = accountsClient ?? readyAccountsClient(accounts);
  render(<LauncherHome api={api} accountsClient={acc} launchClient={launch} />);
  return { api, launch, acc };
}

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
    fireEvent.click(await screen.findByRole("button", { name: "Create a profile" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("New profile")).toBeTruthy();
    await waitFor(() => expect(api.versionCatalog).toHaveBeenCalled()); // version lookup is only metadata
    expect(api.prepare).not.toHaveBeenCalled();
  });

  it("does not engage the sign-in gate before the account list settles", async () => {
    let resolveAccounts!: (snapshot: AccountsSnapshot) => void;
    const acc = readyAccountsClient(accountsOf(null));
    acc.list.mockImplementationOnce(
      () =>
        new Promise<AccountsSnapshot>((resolve) => {
          resolveAccounts = resolve;
        }),
    );
    setup(snap(profile({ ready: true })), accountsOf(null), acc);
    // Until accountsClient.list() settles, accountReady stays true so Play is shown instead of Sign in.
    await screen.findByRole("button", { name: "Play" });
    expect(screen.queryByRole("button", { name: "Sign in to continue" })).toBeNull();

    await act(async () => {
      resolveAccounts(accountsOf(null));
      await Promise.resolve();
    });
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
  });

  it("engages the sign-in gate when loading accounts fails", async () => {
    const acc = readyAccountsClient(accountsOf(null));
    acc.list.mockRejectedValueOnce(new Error("accounts unavailable"));
    const { api, launch } = setup(snap(profile({ ready: true })), accountsOf(null), acc);
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(button("Add an account").textContent).toContain("Sign in");
    expect(api.prepare).not.toHaveBeenCalled();
    expect(launch.launch).not.toHaveBeenCalled();
  });

  it("a profile never starts a download: the primary action asks to sign in and opens the sign-in", async () => {
    const { api, acc } = setup(snap(profile()), accountsOf(null));
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    fireEvent.click(button("Sign in to continue"));
    await waitFor(() => expect(acc.vaultStatus).toHaveBeenCalled());
    expect(await screen.findByRole("dialog")).toBeTruthy();
    expect(api.prepare).not.toHaveBeenCalled();
  });

  it("an account that must sign in again gates set up, verify and play, keeps the profiles and re-signs the SAME account in", async () => {
    const needsSignIn = accountsOf("a", account("a", "Alex", true));
    setup(snap(profile({ ready: false })), needsSignIn);
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    expect(button(/Profile: Survival/)).toBeTruthy();
    cleanup();

    const { api, launch, acc } = setup(snap(profile({ ready: true })), needsSignIn);
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    // The profile is intact and still selectable; only game files are gated.
    fireEvent.click(button(/Profile: Survival/));
    const verify = await screen.findByRole("menuitem", { name: "Verify files" });
    expect(verify.hasAttribute("data-disabled")).toBe(true);
    fireEvent.click(verify);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.prepare).not.toHaveBeenCalled();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

    // ProfileDialog's own "Verify files" button is also disabled while !accountReady.
    fireEvent.click(button(/Profile: Survival/));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Profile settings…" }));
    const profileDialog = await screen.findByRole("dialog");
    const dialogVerify = within(profileDialog).getByRole<HTMLButtonElement>("button", {
      name: "Verify files",
    });
    expect(dialogVerify.disabled).toBe(true);
    fireEvent.click(dialogVerify);
    expect(api.prepare).not.toHaveBeenCalled();
    fireEvent.click(within(profileDialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(button("Sign in to continue"));
    await waitFor(() => expect(acc.signIn).toHaveBeenCalledWith("a")); // the same account, not a new one
    expect(launch.launch).not.toHaveBeenCalled();
  });

  it("gating follows the ACTIVE account: a ready account next to it never stands in", async () => {
    const accounts = accountsOf("a", account("a", "Alex", true), account("b", "Blake"));
    const { launch, acc } = setup(snap(profile({ ready: true })), accounts);
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(launch.launch).not.toHaveBeenCalled();
    // Choosing the account that can play is what unlocks Play (and launches with that account).
    acc.select.mockResolvedValueOnce({ ...accounts, active: "b" });
    fireEvent.click(button(/Account: Alex/));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /Blake/ }));
    await waitFor(() => expect(button("Play")).toBeTruthy());
    fireEvent.click(button("Play"));
    await waitFor(() => expect(launch.launch).toHaveBeenCalledWith("survival"));
  });

  it("rolls back optimistic account selection and re-gates when selecting an account fails", async () => {
    const accounts = accountsOf("a", account("a", "Alex", true), account("b", "Blake"));
    const acc = readyAccountsClient(accounts);
    let rejectSelect!: (err: Error) => void;
    acc.select.mockImplementationOnce(
      () =>
        new Promise<AccountsSnapshot>((_, reject) => {
          rejectSelect = reject;
        }),
    );
    setup(snap(profile({ ready: true })), accounts, acc);
    await screen.findByRole("button", { name: "Sign in to continue" });

    fireEvent.click(button(/Account: Alex/));
    fireEvent.click(await screen.findByRole("menuitemradio", { name: /Blake/ }));
    // Optimistically switches to Blake and unlocks Play while select() is in flight.
    await screen.findByRole("button", { name: /Account: Blake/ });
    expect(button("Play")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Sign in to continue" })).toBeNull();

    await act(async () => {
      rejectSelect(new Error("select failed"));
      await Promise.resolve();
    });
    await screen.findByRole("button", { name: /Account: Alex/ });
    await screen.findByRole("button", { name: "Sign in to continue" });
    expect(screen.queryByRole("button", { name: "Play" })).toBeNull();
    expect(acc.list).toHaveBeenCalledTimes(2);
  });

  it("signing in updates the dock at once, without restarting", async () => {
    const { acc, api } = setup(snap(profile()), accountsOf(null));
    await screen.findByRole("button", { name: "Sign in to continue" });
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
    await screen.findByRole("button", { name: "Войти для продолжения" });
  });
});

describe("with an account that can play", () => {
  it("an unprepared profile sets up, a prepared one plays", async () => {
    const a = accountsOf("a", account("a", "Alex"));
    const unprepared = setup(snap(profile()), a);
    await screen.findByRole("button", { name: "Set up" });
    await screen.findByRole("button", { name: /Account: Alex/ });
    fireEvent.click(button("Set up"));
    await waitFor(() => expect(unprepared.api.prepare).toHaveBeenCalled());
    expect(unprepared.api.prepare.mock.calls[0][0]).toBe("survival");
    cleanup();

    const prepared = setup(snap(profile({ ready: true })), a);
    await screen.findByRole("button", { name: "Play" });
    await screen.findByRole("button", { name: /Account: Alex/ });
    fireEvent.click(button("Play"));
    await waitFor(() => expect(prepared.launch.launch).toHaveBeenCalledWith("survival"));
    expect(prepared.api.prepare).not.toHaveBeenCalled();
  });

  it("Verify files is available and runs the verification", async () => {
    const { api } = setup(snap(profile({ ready: true })), accountsOf("a", account("a", "Alex")));
    await screen.findByRole("button", { name: /Profile: Survival/ });
    await screen.findByRole("button", { name: /Account: Alex/ });
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
    const trigger = () => button(/Account: /);
    await waitFor(() => expect(head(trigger())).toBe(skinA));
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
    const trigger = await screen.findByRole("button", { name: /Account: Alex/ });
    await screen.findByRole("button", { name: "Set up" });
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
    setup(snap(profile()), before, acc);
    await waitFor(() => expect(acc.onChanged).toHaveBeenCalled());
    const trigger = await screen.findByRole("button", { name: /Account: Alex/ });
    expect(placeholder(trigger)).toBe(true);
    act(() => changed?.());
    await waitFor(() => expect(head(trigger)).toBe(testSkin(64, 64, 7)));
  });
});
