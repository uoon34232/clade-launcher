import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountsApi, AccountsSnapshot, SignInStage, VaultStatus } from "@/lib/accounts/api";
import { appearanceStubs, recoveryStubs } from "@/lib/accounts/testAccounts";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type { ProfilesApi } from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";

vi.mock("@/lib/sound/soundService", () => {
  let settings = { enabled: true, volume: 0.5 };
  const listeners = new Set<(s: typeof settings) => void>();
  return {
    playSound: vi.fn(() => true),
    sound: {
      getSettings: () => ({ ...settings }),
      setSettings: (patch: Partial<typeof settings>) => {
        settings = { ...settings, ...patch };
        listeners.forEach((l) => l({ ...settings }));
        return settings;
      },
      subscribe: (l: (s: typeof settings) => void) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    },
  };
});

// Test fixtures only: production code never invents accounts.
const CAPS = { signIn: true, skin: true, cape: true };
const two: AccountsSnapshot = {
  accounts: [
    {
      id: "a",
      username: "Alex",
      uuid: "00000000-0000-4000-8000-00000000a001",
      needsSignIn: false,
      skin: null,
    },
    {
      id: "b",
      username: "Blake",
      uuid: "00000000-0000-4000-8000-00000000b002",
      needsSignIn: true,
      skin: null,
    },
  ],
  active: "a",
  capabilities: CAPS,
};

function makeAccounts(
  initial: AccountsSnapshot,
  initialVault: VaultStatus = { exists: true, unlocked: true },
) {
  let snap = initial;
  let vault = initialVault;
  let progress: ((stage: SignInStage) => void) | null = null;
  let pending: { resolve: (s: AccountsSnapshot) => void; reject: (e: unknown) => void } | null =
    null;
  const list = vi.fn<AccountsApi["list"]>(() => Promise.resolve(snap));
  const select = vi.fn<AccountsApi["select"]>((id) => {
    snap = { ...snap, active: id };
    return Promise.resolve(snap);
  });
  const remove = vi.fn<AccountsApi["remove"]>((id) => {
    const accounts = snap.accounts.filter((a) => a.id !== id);
    snap = { ...snap, accounts, active: accounts[0]?.id ?? null };
    return Promise.resolve(snap);
  });
  const vaultStatus = vi.fn<AccountsApi["vaultStatus"]>(() => Promise.resolve(vault));
  const createVault = vi.fn<AccountsApi["createVault"]>(() => {
    vault = { exists: true, unlocked: true };
    return Promise.resolve();
  });
  const unlockVault = vi.fn<AccountsApi["unlockVault"]>((p) => {
    if (p !== "correct horse")
      return Promise.reject(new BackendError("wrong passphrase", "passphrase_mismatch"));
    vault = { exists: true, unlocked: true };
    return Promise.resolve();
  });
  const signIn = vi.fn<AccountsApi["signIn"]>(
    () =>
      new Promise<AccountsSnapshot>((resolve, reject) => {
        pending = { resolve, reject };
      }),
  );
  const cancelSignIn = vi.fn<AccountsApi["cancelSignIn"]>(() => {
    pending?.reject(new BackendError("Sign-in was cancelled.", "cancelled"));
    return Promise.resolve();
  });
  const onSignInProgress = vi.fn<AccountsApi["onSignInProgress"]>((cb) => {
    progress = cb;
    return Promise.resolve(() => {
      progress = null;
    });
  });
  return {
    list,
    select,
    remove,
    ...appearanceStubs(),
    ...recoveryStubs(),
    vaultStatus,
    createVault,
    unlockVault,
    signIn,
    cancelSignIn,
    onSignInProgress,
    // test controls
    emit: (stage: SignInStage) => act(() => progress?.(stage)),
    finish: (next: AccountsSnapshot) =>
      act(async () => {
        snap = next;
        pending?.resolve(next);
        await Promise.resolve();
      }),
    fail: (e: unknown) =>
      act(async () => {
        pending?.reject(e);
        await Promise.resolve();
      }),
  };
}

function makeProfiles() {
  const snapshot = { profiles: [], selected: null, totalPlaytimeSeconds: 0 };
  return {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(snapshot)),
    select: vi.fn<ProfilesApi["select"]>(() => Promise.resolve()),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.reject(new Error("offline")),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve([])),
    create: vi.fn<ProfilesApi["create"]>(() => Promise.reject(new Error("no"))),
    prepare: vi.fn<ProfilesApi["prepare"]>(() => Promise.reject(new Error("no"))),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve()),
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
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

async function setup(snapshot: AccountsSnapshot, vault?: VaultStatus) {
  const accounts = makeAccounts(snapshot, vault);
  const profiles = makeProfiles();
  render(<LauncherHome api={profiles} accountsClient={accounts} />);
  await flush();
  return { accounts, profiles };
}

const openMenu = async () => {
  fireEvent.click(
    screen.getByRole("button", { name: /Account: Alex|Account: Blake|Add an account/ }),
  );
  await waitFor(() => expect(screen.getByRole("menu")).toBeTruthy());
};

describe("account menu in the dock", () => {
  it("shows the active account and lists every account, ending with Add account", async () => {
    await setup(two);
    await openMenu();
    expect(screen.getByRole("menuitemradio", { name: /Alex/ }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(screen.getByRole("menuitemradio", { name: /Blake/ }).textContent).toContain(
      "Sign in again",
    );
    expect(screen.getByRole("menuitem", { name: "Manage Alex" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "+ Add account" })).toBeTruthy();
  });

  it("selecting an account changes only the account: no profile is touched", async () => {
    const { accounts, profiles } = await setup(two);
    await openMenu();
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Blake/ }));
    await flush();
    expect(accounts.select).toHaveBeenCalledWith("b");
    expect(profiles.select).not.toHaveBeenCalled();
    expect(profiles.create).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Account: Blake/ })).toBeTruthy(),
    );
  });

  it("with no accounts it only offers to add one, and invents none", async () => {
    await setup({ accounts: [], active: null, capabilities: CAPS });
    await openMenu();
    expect(screen.queryAllByRole("menuitemradio")).toHaveLength(0);
    expect(screen.getByRole("menuitem", { name: "+ Add account" })).toBeTruthy();
  });
});

const casey: AccountsSnapshot = {
  accounts: [
    ...two.accounts,
    {
      id: "c",
      username: "Casey_9",
      uuid: "11111111-2222-3333-4444-555555555555",
      needsSignIn: false,
      skin: null,
    },
  ],
  active: "c",
  capabilities: CAPS,
};

async function clickAdd() {
  await openMenu();
  fireEvent.click(screen.getByRole("menuitem", { name: "+ Add account" }));
  return screen.findByRole("dialog");
}

describe("signing in", () => {
  it("walks through the stages, then adds the account and makes it active", async () => {
    const { accounts, profiles } = await setup(two);
    const dialog = await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith(null));
    expect(dialog.textContent).toContain("Opening Microsoft sign-in…");
    expect(dialog.querySelector("input")).toBeNull(); // Microsoft's page does the signing in; no form here
    accounts.emit("waiting");
    expect(dialog.textContent).toContain("Waiting for sign-in…");
    accounts.emit("finishing");
    expect(dialog.textContent).toContain("Finishing setup…");
    await accounts.finish(casey);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: /Account: Casey_9/ })).toBeTruthy();
    expect(profiles.select).not.toHaveBeenCalled();
  });

  it("cancelling in the launcher stops the sign-in and closes quietly", async () => {
    const { accounts } = await setup(two);
    await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(accounts.cancelSignIn).toHaveBeenCalled();
  });

  it("Escape while waiting also cancels", async () => {
    const { accounts } = await setup(two);
    const dialog = await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalled());
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(accounts.cancelSignIn).toHaveBeenCalled();
  });

  it("a browser-side cancel closes without an error message", async () => {
    const { accounts } = await setup(two);
    await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalled());
    await accounts.fail(new BackendError("Sign-in was cancelled.", "cancelled"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("explains that Mojang has not approved the launcher yet, and does not offer to retry", async () => {
    const { accounts } = await setup(two);
    const dialog = await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalled());
    await accounts.fail(
      new BackendError(
        "Microsoft sign-in worked, but Mojang has not approved this launcher for Minecraft sign-in yet.",
        "approval_required",
      ),
    );
    expect(dialog.textContent).toContain("Mojang has not approved");
    expect(dialog.textContent).toContain("Nothing was saved");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("tells a missing Java profile apart and lets the person try again", async () => {
    const { accounts } = await setup(two);
    const dialog = await clickAdd();
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledTimes(1));
    await accounts.fail(
      new BackendError(
        "This Microsoft account does not own Minecraft: Java Edition.",
        "no_java_profile",
      ),
    );
    expect(dialog.textContent).toContain("does not own Minecraft: Java Edition");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledTimes(2));
    expect(dialog.textContent).toContain("Opening Microsoft sign-in…");
  });

  it("asks for a passphrase first when there is no encrypted store yet, and never keeps it", async () => {
    const { accounts } = await setup(two, { exists: false, unlocked: false });
    const dialog = await clickAdd();
    await screen.findByText("Protect your accounts");
    const pass = screen.getByLabelText<HTMLInputElement>("Passphrase");
    const again = screen.getByLabelText<HTMLInputElement>("Repeat passphrase");
    const go = screen.getByRole<HTMLButtonElement>("button", { name: "Continue" });
    expect(go.disabled).toBe(true);
    fireEvent.change(pass, { target: { value: "short" } });
    fireEvent.change(again, { target: { value: "short" } });
    expect(go.disabled).toBe(true); // too short
    fireEvent.change(pass, { target: { value: "long enough words" } });
    fireEvent.change(again, { target: { value: "different words!!" } });
    expect(go.disabled).toBe(true);
    expect(dialog.textContent).toContain("match");
    fireEvent.change(again, { target: { value: "long enough words" } });
    expect(go.disabled).toBe(false);
    fireEvent.click(go);
    await waitFor(() => expect(accounts.createVault).toHaveBeenCalledWith("long enough words"));
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith(null));
    expect(dialog.querySelector("input")).toBeNull();
  });

  it("asks to unlock when the store is locked, rejects a wrong passphrase, then continues", async () => {
    const { accounts } = await setup(two, { exists: true, unlocked: false });
    const dialog = await clickAdd();
    await screen.findByText("Unlock your accounts");
    fireEvent.change(screen.getByLabelText("Passphrase"), { target: { value: "wrong wrong" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(dialog.textContent).toContain("doesn't match"));
    expect(accounts.signIn).not.toHaveBeenCalled();
    expect(screen.getByLabelText<HTMLInputElement>("Passphrase").value).toBe("");
    fireEvent.change(screen.getByLabelText("Passphrase"), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith(null));
  });

  it("signing in again is offered only for an account that needs it, and targets that account", async () => {
    const { accounts } = await setup(two);
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Manage Alex" }));
    await screen.findByRole("dialog");
    expect(screen.queryByRole("button", { name: "Sign in again" })).toBeNull();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Manage Blake" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalledWith("b"));
    expect(screen.getByRole("dialog").textContent).toContain("Sign in as Blake");
  });
});

describe("account management modal", () => {
  async function openManage() {
    const ctx = await setup(two);
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Manage Blake" }));
    const dialog = await screen.findByRole("dialog");
    return { ...ctx, dialog };
  }

  it("shows the profile, and says in words that an account signed out cannot change its look", async () => {
    const { dialog, accounts } = await openManage();
    expect(dialog.textContent).toContain("Blake");
    expect(dialog.textContent).toContain("00000000-0000-4000-8000-00000000b002");
    expect(dialog.textContent).toContain("Needs to sign in again");
    expect(dialog.textContent).toContain("Sign in again to change your Minecraft appearance.");
    expect(screen.getByRole<HTMLButtonElement>("button", { name: /Choose PNG/ }).disabled).toBe(
      true,
    );
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Restore default" }).disabled,
    ).toBe(true);
    expect(accounts.appearance).not.toHaveBeenCalled();
    // the old "not available yet" sentences are gone
    expect(dialog.textContent).not.toMatch(/isn.t available in Clade yet|never uploaded/);
  });

  it("shows no skin or cape section when the build does not offer them", async () => {
    const ctx = await setup({ ...two, capabilities: { signIn: true, skin: false, cape: false } });
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Manage Blake" }));
    const dialog = await screen.findByRole("dialog");
    expect(ctx.accounts.appearance).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /Choose PNG/ })).toBeNull();
    expect(dialog.textContent).not.toMatch(/Skin|Cape/);
  });

  it("removing needs a confirmation, then removes only the account", async () => {
    const { accounts, profiles } = await openManage();
    fireEvent.click(screen.getByRole("button", { name: /Sign out and remove/ }));
    expect(accounts.remove).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog").textContent).toContain("profiles, worlds and playtime stay");
    fireEvent.click(screen.getByRole("button", { name: "Remove account" }));
    await waitFor(() => expect(accounts.remove).toHaveBeenCalledWith("b"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(profiles.create).not.toHaveBeenCalled();
    expect(profiles.select).not.toHaveBeenCalled();
  });

  it("Escape closes it", async () => {
    await openManage();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("launcher settings", () => {
  it("is tiny: sound effects and volume, and the switch works", async () => {
    await setup(two);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog");
    const sw = screen.getByRole("switch", { name: "Sound effects" });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    expect(dialog.querySelectorAll("input,[role=switch],select")).toHaveLength(2);
    fireEvent.click(sw);
    await waitFor(() => expect(sw.getAttribute("aria-checked")).toBe("false"));
    expect(screen.getByLabelText<HTMLInputElement>("Volume").disabled).toBe(true);
  });
});

describe("accounts in Russian", () => {
  it("walks sign-in in Russian: passphrase creation, progress, and errors worded by code", async () => {
    setLocale("ru");
    const { accounts } = await setup(two, { exists: false, unlocked: false });
    fireEvent.click(screen.getByRole("button", { name: /Аккаунт: Alex|Аккаунт: Blake/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "+ Добавить аккаунт" }));
    const dialog = await screen.findByRole("dialog");
    await screen.findByText("Защита аккаунтов");
    expect(dialog.textContent).toContain("Восстановить её нельзя");
    expect(dialog.textContent).toContain("Не менее 8 символов");
    const pass = screen.getByLabelText<HTMLInputElement>("Парольная фраза");
    const again = screen.getByLabelText<HTMLInputElement>("Парольная фраза ещё раз");
    fireEvent.change(pass, { target: { value: "long enough words" } });
    fireEvent.change(again, { target: { value: "different" } });
    expect(dialog.textContent).toContain("Парольные фразы не совпадают.");
    fireEvent.change(again, { target: { value: "long enough words" } });
    fireEvent.click(screen.getByRole("button", { name: "Продолжить" }));
    await waitFor(() => expect(accounts.signIn).toHaveBeenCalled());
    expect(dialog.textContent).toContain("Открытие входа Microsoft…");
    await accounts.fail(
      new BackendError("English detail", "no_java_profile", { params: { ownsMinecraft: true } }),
    );
    expect(dialog.textContent).toContain("есть Minecraft, но ещё нет профиля Java");
    expect(dialog.textContent).not.toContain("English detail");
    expect(screen.getByRole("button", { name: "Повторить" })).toBeTruthy();
  });

  it("manages an account in Russian, including the removal confirmation", async () => {
    setLocale("ru");
    await setup(two);
    fireEvent.click(screen.getByRole("button", { name: /Аккаунт: Alex/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Управление: Alex" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Аккаунт Minecraft Java");
    expect(dialog.textContent).toContain("Вход выполнен");
    expect(dialog.textContent).toContain("Скин");
    expect(dialog.textContent).toContain("Плащ");
    expect(screen.getByRole("radio", { name: "Классическая" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Выбрать PNG…" })).toBeTruthy();
    expect(await screen.findByText("У этого аккаунта нет плащей.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Выйти и удалить…" }));
    expect(dialog.textContent).toContain(
      "Удалить Alex? Профили, миры и время игры останутся как есть.",
    );
    expect(screen.getByRole("button", { name: "Оставить аккаунт" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Удалить аккаунт" })).toBeTruthy();
  });
});
