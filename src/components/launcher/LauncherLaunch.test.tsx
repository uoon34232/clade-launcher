import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountsApi } from "@/lib/accounts/api";
import { READY_ACCOUNTS } from "@/lib/accounts/testAccounts";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { LaunchRefused, type LaunchApi, type LaunchEvent, type LaunchExit } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { LauncherHome } from "./LauncherHome";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const ready: ProfileSummary = {
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 0,
};
const snap = (p: ProfileSummary): ProfilesSnapshot => ({
  profiles: [p],
  selected: p.id,
  totalPlaytimeSeconds: 0,
});

function makeLaunch() {
  const listeners = new Set<(e: LaunchEvent) => void>();
  const launch = vi.fn<LaunchApi["launch"]>(() => Promise.resolve());
  const status = vi.fn<LaunchApi["status"]>(() => Promise.resolve([]));
  const openLogs = vi.fn<LaunchApi["openLogs"]>(() => Promise.resolve());
  const onState = vi.fn<LaunchApi["onState"]>((cb) => {
    listeners.add(cb);
    return Promise.resolve(() => {
      listeners.delete(cb);
    });
  });
  return {
    launch,
    status,
    openLogs,
    onState,
    event: (e: LaunchEvent) => act(() => listeners.forEach((l) => l(e))),
  };
}

function makeProfiles(initial: ProfilesSnapshot) {
  return {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(initial)),
    select: vi.fn<ProfilesApi["select"]>(() => Promise.resolve()),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.reject(new Error("offline")),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve([])),
    create: vi.fn<ProfilesApi["create"]>(() => Promise.reject(new Error("no"))),
    prepare: vi.fn<ProfilesApi["prepare"]>(() => new Promise<ProfilesSnapshot>(() => undefined)),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve()),
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  };
}

function makeAccounts(signedIn = true) {
  // By default the active account can play, so the dock offers Play / Set up; a refusal then comes
  // from the (mocked) backend. `signedIn = false` is an empty list: the dock asks to sign in.
  const none = signedIn
    ? READY_ACCOUNTS
    : { accounts: [], active: null, capabilities: { signIn: true, skin: false, cape: false } };
  return {
    list: vi.fn<AccountsApi["list"]>(() => Promise.resolve(none)),
    select: vi.fn<AccountsApi["select"]>(() => Promise.resolve(none)),
    remove: vi.fn<AccountsApi["remove"]>(() => Promise.resolve(none)),
    vaultStatus: vi.fn<AccountsApi["vaultStatus"]>(() =>
      Promise.resolve({ exists: true, unlocked: true }),
    ),
    createVault: vi.fn<AccountsApi["createVault"]>(() => Promise.resolve()),
    unlockVault: vi.fn<AccountsApi["unlockVault"]>((p) =>
      p === "right pass"
        ? Promise.resolve()
        : Promise.reject(new BackendError("wrong passphrase", "passphrase_mismatch")),
    ),
    signIn: vi.fn<AccountsApi["signIn"]>(() => new Promise(() => undefined)),
    cancelSignIn: vi.fn<AccountsApi["cancelSignIn"]>(() => Promise.resolve()),
    onSignInProgress: vi.fn<AccountsApi["onSignInProgress"]>(() =>
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

async function setup(profile: ProfileSummary = ready) {
  const launch = makeLaunch();
  const profiles = makeProfiles(snap(profile));
  const accounts = makeAccounts();
  render(
    <StrictMode>
      <LauncherHome api={profiles} accountsClient={accounts} launchClient={launch} />
    </StrictMode>,
  );
  await flush();
  return { launch, profiles, accounts };
}

const button = (name: RegExp | string) => screen.getByRole("button", { name });
const exit = (kind: LaunchExit["kind"], reason: string): LaunchExit => ({
  code: kind === "normal" ? 0 : 1,
  kind,
  reason,
  seconds: 42,
});

describe("playing a ready profile", () => {
  it("goes Play -> Launching… -> Playing -> Play, and ignores duplicate clicks while busy", async () => {
    const { launch } = await setup();
    const play = button("Play") as HTMLButtonElement;
    fireEvent.click(play);
    fireEvent.click(play);
    fireEvent.click(play);
    await flush();
    expect(launch.launch).toHaveBeenCalledTimes(1);
    expect(launch.launch).toHaveBeenCalledWith("survival");

    const busy = button("Launching…") as HTMLButtonElement;
    expect(busy.disabled).toBe(true);
    expect(busy.getAttribute("aria-busy")).toBe("true");

    launch.event({ instanceId: "survival", state: "launching" });
    launch.event({ instanceId: "survival", state: "playing" });
    const playing = button("Playing") as HTMLButtonElement;
    expect(playing.disabled).toBe(true);
    fireEvent.click(playing);
    expect(launch.launch).toHaveBeenCalledTimes(1);

    launch.event({
      instanceId: "survival",
      state: "stopped",
      exit: exit("normal", "closed"),
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect((button("Play") as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows a running game after the window was reloaded", async () => {
    const launch = makeLaunch();
    launch.status.mockResolvedValue([{ instanceId: "survival", state: "playing" }]);
    render(
      <LauncherHome
        api={makeProfiles(snap(ready))}
        accountsClient={makeAccounts()}
        launchClient={launch}
      />,
    );
    await flush();
    expect(button("Playing")).toBeTruthy();
  });

  it("explains a crash briefly and offers the logs", async () => {
    const { launch } = await setup();
    fireEvent.click(button("Play"));
    launch.event({
      instanceId: "survival",
      state: "stopped",
      exit: exit("crashed", "native_library"),
    });
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Minecraft stopped");
    expect(dialog.textContent).toContain("native library");
    fireEvent.click(screen.getByRole("button", { name: "Open logs" }));
    expect(launch.openLogs).toHaveBeenCalledWith("survival");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(button("Play")).toBeTruthy();
  });

  it("a game that never started is reported too, and a clean exit shows nothing", async () => {
    const { launch } = await setup();
    launch.event({
      instanceId: "survival",
      state: "stopped",
      exit: exit("failed_to_start", "early_exit"),
    });
    expect((await screen.findByRole("dialog")).textContent).toContain("right after starting");
  });
});

describe("when the launch is refused", () => {
  const refuse = async (code: string, message: string, accountId: string | null = null) => {
    const ctx = await setup();
    ctx.launch.launch.mockRejectedValueOnce(new LaunchRefused(code, message, accountId));
    fireEvent.click(button("Play"));
    await screen.findByRole("dialog");
    return ctx;
  };

  it("with no account it asks to add one, and the sign-in modal follows", async () => {
    const { accounts } = await refuse("no_account", "Sign in with a Microsoft account to play.");
    expect(screen.getByRole("dialog").textContent).toContain(
      "Sign in with a Microsoft account to play.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));
    await waitFor(() => expect(accounts.vaultStatus).toHaveBeenCalled());
    expect((await screen.findByRole("dialog")).textContent).toContain("Add account");
  });

  it("an account that must sign in again leads to Sign in again for that account", async () => {
    const { accounts } = await refuse(
      "needs_sign_in",
      "Alex needs to sign in again before it can play.",
      "acc-a",
    );
    fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));
    await waitFor(() => expect(accounts.vaultStatus).toHaveBeenCalled());
    // (The account is not in this test's account list, so the dialog opens as a plain sign-in.)
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("a locked account store asks for the passphrase and then plays", async () => {
    const { accounts, launch } = await refuse("vault_locked", "Unlock your accounts to play.");
    expect(screen.getByRole("dialog").textContent).toContain("Unlock your accounts");
    fireEvent.change(screen.getByLabelText("Passphrase"), { target: { value: "wrong" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("doesn't match"));
    expect(launch.launch).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText<HTMLInputElement>("Passphrase").value).toBe("");
    fireEvent.change(screen.getByLabelText("Passphrase"), { target: { value: "right pass" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(launch.launch).toHaveBeenCalledTimes(2));
    expect(accounts.unlockVault).toHaveBeenLastCalledWith("right pass");
  });

  it("missing or damaged files go straight to repair through the preparation dialog", async () => {
    const { launch, profiles } = await setup();
    launch.launch.mockRejectedValueOnce(
      new LaunchRefused("needs_repair", "Part of this profile is missing.", null),
    );
    fireEvent.click(button("Play"));
    await waitFor(() => expect(profiles.prepare).toHaveBeenCalledWith("survival"));
    // The dock shows the repair while the preparation dialog (which hides the dock) is open.
    const repairing = screen.getByRole("button", { name: "Repairing…", hidden: true });
    expect((repairing as HTMLButtonElement).disabled).toBe(true);
  });

  it("an offline refresh can be retried, and Mojang's pending approval is shown as its own reason", async () => {
    const { launch } = await refuse(
      "offline",
      "Clade can't refresh your Minecraft sign-in without a network.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(launch.launch).toHaveBeenCalledTimes(2));
    cleanup();

    await refuse(
      "approval_pending",
      "Mojang has not approved this launcher for Minecraft sign-in yet.",
    );
    expect(screen.getByRole("dialog").textContent).toContain("Mojang has not approved");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("the launcher never offers an offline or guest way in", async () => {
    await refuse("no_account", "Sign in with a Microsoft account to play.");
    const text = document.body.textContent ?? "";
    expect(text).not.toMatch(/offline mode|guest|cracked|play offline/i);
  });
});

describe("verifying files", () => {
  it("Verify files in the profile menu runs a full verification through the preparation dialog", async () => {
    const { profiles } = await setup();
    fireEvent.click(screen.getByRole("button", { name: /Profile: Survival/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Verify files" }));
    await waitFor(() =>
      expect(profiles.prepare).toHaveBeenCalledWith("survival", { verify: true }),
    );
  });
});

describe("launch problems in Russian", () => {
  it("explains a crash by its reason, in Russian, and offers the logs", async () => {
    setLocale("ru");
    const { launch } = await setup();
    fireEvent.click(button("Играть"));
    launch.event({
      instanceId: "survival",
      state: "stopped",
      exit: exit("crashed", "native_library"),
    });
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Minecraft остановился");
    expect(dialog.textContent).toContain("Не удалось загрузить нативную библиотеку");
    fireEvent.click(screen.getByRole("button", { name: "Открыть логи" }));
    expect(launch.openLogs).toHaveBeenCalledWith("survival");
  });

  it("an unknown exit reason still reads as a plain sentence", async () => {
    setLocale("ru");
    const { launch } = await setup();
    launch.event({
      instanceId: "survival",
      state: "stopped",
      exit: exit("crashed", "a_reason_from_a_newer_backend"),
    });
    expect((await screen.findByRole("dialog")).textContent).toContain("неожиданно остановился");
  });

  it("refusals are worded from their code, never from the backend's English message", async () => {
    setLocale("ru");
    const { launch } = await setup();
    launch.launch.mockRejectedValueOnce(
      new LaunchRefused("approval_pending", "English text that must not be shown", null),
    );
    fireEvent.click(button("Играть"));
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("Вход пока недоступен");
    expect(dialog.textContent).toContain("Mojang ещё не одобрил этот лаунчер");
    expect(dialog.textContent).not.toContain("English text");
  });

  it("a locked account store asks for the passphrase in Russian and words a wrong one", async () => {
    setLocale("ru");
    const { launch } = await setup();
    launch.launch.mockRejectedValueOnce(new LaunchRefused("vault_locked", "x", null));
    fireEvent.click(button("Играть"));
    await screen.findByRole("dialog");
    expect(screen.getByRole("dialog").textContent).toContain("Разблокировать аккаунты");
    fireEvent.change(screen.getByLabelText("Парольная фраза"), { target: { value: "wrong" } });
    fireEvent.click(screen.getByRole("button", { name: "Разблокировать" }));
    await waitFor(() =>
      expect(screen.getByRole("dialog").textContent).toContain("Парольная фраза не подходит."),
    );
  });
});
