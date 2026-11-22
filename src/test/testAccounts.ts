// Test helper only (imported by tests, never by the app): an accounts client for a launcher whose
// active account is signed in and able to play, so tests of preparing, verifying and launching
// are not blocked by the "Sign in to continue" gate. Obviously fake data only.
import { vi } from "vitest";
import type { AccountsApi, AccountsSnapshot, Appearance } from "@/lib/accounts/api";

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

function cloneSnapshot(snapshot: AccountsSnapshot): AccountsSnapshot {
  return {
    ...snapshot,
    accounts: snapshot.accounts.map((a) => ({
      ...a,
      skinDimensions: a.skinDimensions ? { ...a.skinDimensions } : a.skinDimensions,
    })),
    capabilities: { ...snapshot.capabilities },
  };
}

/** Returns a post-reset snapshot where every account must sign in again, matching Rust `reset_vault`. */
export function resetAccountsSnapshot(
  snapshot: AccountsSnapshot = READY_ACCOUNTS,
): AccountsSnapshot {
  const next = cloneSnapshot(snapshot);
  for (const account of next.accounts) {
    account.needsSignIn = true;
  }
  return next;
}

export const RESET_ACCOUNTS: AccountsSnapshot = resetAccountsSnapshot(READY_ACCOUNTS);

/**
 * "Start fresh" stub. Defaults to `RESET_ACCOUNTS` (`needsSignIn: true`, matching Rust
 * `reset_vault`). Pass an explicit snapshot to override the resolved value.
 */
export function recoveryStubs(snapshot: AccountsSnapshot = RESET_ACCOUNTS) {
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

function effectiveActive(
  accounts: AccountsSnapshot["accounts"],
  active: string | null,
): string | null {
  return accounts.some((a) => a.id === active) ? active : (accounts[0]?.id ?? null);
}

/**
 * Stateful accounts client stub initialized from `snapshot`.
 *
 * `select` and `remove` update the active/remaining accounts just like Rust `accounts_api`.
 * Note: `resetVault` defaults to resolving with `snapshot` unchanged via `recoveryStubs(snapshot)`
 * so existing appearance-forwarding tests keep their expectations; tests exercising post-reset
 * `needsSignIn` transitions should override `resetVault` with `recoveryStubs()` or
 * `recoveryStubs(resetAccountsSnapshot(snapshot))`.
 */
export function readyAccountsClient(snapshot: AccountsSnapshot = READY_ACCOUNTS) {
  let current = cloneSnapshot(snapshot);
  return {
    list: vi.fn<AccountsApi["list"]>(() => Promise.resolve(current)),
    select: vi.fn<AccountsApi["select"]>((id) => {
      const nextActive = current.accounts.some((a) => a.id === id) ? id : current.active;
      current = { ...current, active: effectiveActive(current.accounts, nextActive) };
      return Promise.resolve(current);
    }),
    remove: vi.fn<AccountsApi["remove"]>((id) => {
      const remaining = current.accounts.filter((a) => a.id !== id);
      const nextActive = current.active === id ? (remaining[0]?.id ?? null) : current.active;
      current = {
        ...current,
        accounts: remaining,
        active: effectiveActive(remaining, nextActive),
      };
      return Promise.resolve(current);
    }),
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
    onChanged: vi.fn<NonNullable<AccountsApi["onChanged"]>>(() => Promise.resolve(() => undefined)),
  };
}
