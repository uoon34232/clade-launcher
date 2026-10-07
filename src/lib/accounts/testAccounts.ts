// Test helper only (imported by tests, never by the app): an accounts client for a launcher whose
// active account is signed in and able to play, so tests of preparing, verifying and launching
// are not blocked by the "Sign in to continue" gate. Obviously fake data only.
import { vi } from "vitest";
import type { AccountsApi, AccountsSnapshot, Appearance } from "./api";

export const READY_ACCOUNTS: AccountsSnapshot = {
  accounts: [
    {
      id: "acc-ready",
      username: "Alex",
      uuid: "00000000-0000-4000-8000-00000000a001",
      needsSignIn: false,
      skin: null,
    },
  ],
  active: "acc-ready",
  capabilities: { signIn: true, skin: true, cape: true },
};

export const NO_LOOK: Appearance = { skinModel: null, capes: [] };

/** "Start fresh" for tests that do not use it. */
export function recoveryStubs(snapshot: AccountsSnapshot = READY_ACCOUNTS) {
  return { resetVault: vi.fn<AccountsApi["resetVault"]>(() => Promise.resolve(snapshot)) };
}

/** The appearance half of an accounts client for tests that do not use it: nothing owned, nothing changes. */
export function appearanceStubs() {
  return {
    appearance: vi.fn<AccountsApi["appearance"]>(() => Promise.resolve(NO_LOOK)),
    setSkin: vi.fn<AccountsApi["setSkin"]>(() => Promise.resolve(NO_LOOK)),
    resetSkin: vi.fn<AccountsApi["resetSkin"]>(() => Promise.resolve(NO_LOOK)),
    setCape: vi.fn<AccountsApi["setCape"]>(() => Promise.resolve(NO_LOOK)),
  };
}

export function readyAccountsClient(snapshot: AccountsSnapshot = READY_ACCOUNTS) {
  return {
    list: vi.fn<AccountsApi["list"]>(() => Promise.resolve(snapshot)),
    select: vi.fn<AccountsApi["select"]>(() => Promise.resolve(snapshot)),
    remove: vi.fn<AccountsApi["remove"]>(() => Promise.resolve(snapshot)),
    ...appearanceStubs(),
    vaultStatus: vi.fn<AccountsApi["vaultStatus"]>(() =>
      Promise.resolve({ exists: true, unlocked: true }),
    ),
    createVault: vi.fn<AccountsApi["createVault"]>(() => Promise.resolve()),
    unlockVault: vi.fn<AccountsApi["unlockVault"]>(() => Promise.resolve()),
    ...recoveryStubs(snapshot),
    signIn: vi.fn<AccountsApi["signIn"]>(() => new Promise(() => undefined)),
    cancelSignIn: vi.fn<AccountsApi["cancelSignIn"]>(() => Promise.resolve()),
    onSignInProgress: vi.fn<AccountsApi["onSignInProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  };
}
