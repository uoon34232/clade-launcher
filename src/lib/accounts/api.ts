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
  /**
   * The account's current Minecraft skin texture as an inline PNG, cached by the backend from the
   * account's own profile. The avatar is cut out of it (see PlayerHead); null draws a placeholder.
   */
  skin: string | null;
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

/**
 * The pixel size of an inline skin PNG, read from its header: 64 times a whole factor wide, as
 * tall as wide or (the old format) half as tall. Null when it is not such an image. The head and
 * hat layers are cut out of it with `width / 64` as the scale.
 */
export function skinSize(dataUri: string): { width: number; height: number } | null {
  const m = /^data:image\/png;base64,([A-Za-z0-9+/]{32,}={0,2})$/.exec(dataUri);
  if (!m) return null;
  let bytes: string;
  try {
    bytes = atob(m[1].slice(0, 32)); // the first 24 bytes: signature, IHDR length and type, width, height
  } catch {
    return null;
  }
  if (!bytes.startsWith("\x89PNG\r\n\x1a\n") || bytes.slice(12, 16) !== "IHDR") return null;
  const u32 = (at: number) =>
    ((bytes.charCodeAt(at) << 24) |
      (bytes.charCodeAt(at + 1) << 16) |
      (bytes.charCodeAt(at + 2) << 8) |
      bytes.charCodeAt(at + 3)) >>>
    0;
  const w = u32(16);
  const h = u32(20);
  const ok = w >= 64 && w <= 1024 && w % 64 === 0 && (h === w || h * 2 === w);
  return ok ? { width: w, height: h } : null;
}

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
    const skin = str(a.skin);
    accounts.push({
      id,
      username,
      uuid: str(a.uuid) ?? "",
      needsSignIn: a.needsSignIn === true,
      // Only an inline PNG of a plausible skin size is ever drawn; nothing is loaded from a URL.
      skin: skin && skinSize(skin) !== null ? skin : null,
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

/** The two Java skin models. */
export type SkinModel = "classic" | "slim";

/** A cape the account owns. The id is only ever handed back to equip it; it is never shown. */
export interface Cape {
  id: string;
  /** The service's name for the cape, when it gives one. */
  alias: string | null;
  active: boolean;
}

/** What the account's Minecraft profile says about its look right now. */
export interface Appearance {
  /** The model of the active skin; null when the account has no custom skin or it is not reported. */
  skinModel: SkinModel | null;
  capes: Cape[];
}

/** The largest skin file the launcher sends (the backend enforces the same bound). */
export const MAX_SKIN_BYTES = 256 * 1024;

const CAPE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function parseAppearance(raw: unknown): Appearance {
  const o = isObj(raw) ? raw : {};
  const capes: Cape[] = [];
  for (const c of Array.isArray(o.capes) ? o.capes : []) {
    if (!isObj(c)) continue;
    const id = str(c.id);
    if (!id || !CAPE_ID.test(id)) continue;
    capes.push({ id, alias: str(c.alias), active: c.active === true });
  }
  return {
    skinModel: o.skinVariant === "classic" || o.skinVariant === "slim" ? o.skinVariant : null,
    capes,
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

  // Skin and cape changes are made on the Minecraft account itself, by the backend, with
  // credentials the frontend never sees. Each one names the account; none can name a path or a URL.
  /** The account's current skin model and the capes it owns. */
  appearance: async (id: string) => parseAppearance(await call("account_appearance", { id })),
  /** Uploads PNG bytes as the account's skin with the chosen model. The backend checks the image. */
  setSkin: async (id: string, model: SkinModel, png: Uint8Array) =>
    parseAppearance(await call("set_account_skin", { id, variant: model, png: Array.from(png) })),
  /** Goes back to the account's default skin. */
  resetSkin: async (id: string) => parseAppearance(await call("reset_account_skin", { id })),
  /** Equips one of the account's own capes, or hides the cape (null). */
  setCape: async (id: string, capeId: string | null) =>
    parseAppearance(await call("set_account_cape", { id, capeId })),

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

/** The backend finished fetching avatars in the background: list the accounts again. */
async function onChanged(callback: () => void): Promise<() => void> {
  if (!inTauri()) return () => undefined;
  return listen<unknown>("accounts-changed", () => callback());
}

export const accountsEvents = { onChanged };

// `onChanged` is optional for the launcher (tests and previews may not provide it).
export type AccountsApi = typeof accountsApi & Partial<typeof accountsEvents>;
