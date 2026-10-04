// Frontend side of the account list. Metadata only (profile name, UUID, sign-in state): tokens
// never reach the frontend. Accounts are independent from instances; selecting one changes only
// which account the launcher will use, never a profile.
//
// Thin, validating wrapper over Rust commands. Outside the launcher nothing is invented.

import { listen } from "@tauri-apps/api/event";
import { call } from "@/lib/ipc";
import { inTauri } from "@/lib/settings/uiSettings";

export interface Account {
  id: string;
  /** The Minecraft Java profile name. */
  username: string;
  uuid: string;
  needsSignIn: boolean;
  /** Data URI of the player's head once the backend has one; otherwise a placeholder is drawn. */
  head: string | null;
}

export interface Capabilities {
  signIn: boolean;
  skin: boolean;
  cape: boolean;
}

export interface AccountsSnapshot {
  accounts: Account[];
  active: string | null;
  capabilities: Capabilities;
}

export const NO_ACCOUNTS: AccountsSnapshot = {
  accounts: [],
  active: null,
  capabilities: { signIn: false, skin: false, cape: false },
};

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const str = (v: unknown): string | null => (typeof v === "string" && v.length > 0 ? v : null);

export function parseAccounts(raw: unknown): AccountsSnapshot {
  const o = isObj(raw) ? raw : {};
  const accounts: Account[] = [];
  for (const a of Array.isArray(o.accounts) ? o.accounts : []) {
    if (!isObj(a)) continue;
    const id = str(a.id);
    const username = str(a.username);
    if (!id || !username) continue;
    const head = str(a.head);
    accounts.push({
      id,
      username,
      uuid: str(a.uuid) ?? "",
      needsSignIn: a.needsSignIn === true,
      // Only an inline PNG is ever drawn; nothing is ever loaded from a URL.
      head: head && /^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(head) ? head : null,
    });
  }
  const c = isObj(o.capabilities) ? o.capabilities : {};
  const active = str(o.active);
  return {
    accounts,
    active: accounts.some((a) => a.id === active) ? active : (accounts[0]?.id ?? null),
    capabilities: { signIn: c.signIn === true, skin: c.skin === true, cape: c.cape === true },
  };
}

export interface VaultStatus {
  /** An encrypted account store exists in the launcher folder. */
  exists: boolean;
  /** It is unlocked for this session. */
  unlocked: boolean;
}

export type SignInStage = "opening" | "waiting" | "finishing";

const STAGES: readonly string[] = ["opening", "waiting", "finishing"];

export const accountsApi = {
  list: async () => parseAccounts(await call("list_accounts")),
  select: async (id: string) => parseAccounts(await call("select_account", { id })),
  /** Signs out: removes the account and its stored credentials. Never touches a profile. */
  remove: async (id: string) => parseAccounts(await call("remove_account", { id })),

  vaultStatus: async (): Promise<VaultStatus> => {
    const raw = await call("vault_status");
    const o = isObj(raw) ? raw : {};
    return { exists: o.exists === true, unlocked: o.unlocked === true };
  },
  /** The passphrase goes straight to the backend and is never kept here. */
  createVault: async (passphrase: string): Promise<void> => {
    await call("create_vault", { passphrase });
  },
  unlockVault: async (passphrase: string): Promise<void> => {
    await call("unlock_vault", { passphrase });
  },
  /**
   * Runs Microsoft's official sign-in in the system browser and resolves with the updated list.
   * Pass an account id to sign that same account in again. Rejects with a `BackendError` whose
   * `code` is "cancelled" when the person stops it.
   */
  signIn: async (accountId: string | null) =>
    parseAccounts(await call("begin_sign_in", { accountId })),
  cancelSignIn: async (): Promise<void> => {
    await call("cancel_sign_in");
  },
  /** Progress of a running sign-in. Returns a function that stops listening. */
  onSignInProgress: async (callback: (stage: SignInStage) => void): Promise<() => void> => {
    if (!inTauri()) return () => undefined;
    return listen<unknown>("sign-in-progress", (e) => {
      if (typeof e.payload === "string" && STAGES.includes(e.payload))
        callback(e.payload as SignInStage);
    });
  },
};

export type AccountsApi = typeof accountsApi;
