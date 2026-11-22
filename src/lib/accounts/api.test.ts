import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
const listen =
  vi.fn<(name: string, cb: (e: { payload: unknown }) => void) => Promise<() => void>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (n: string, cb: (e: { payload: unknown }) => void) => listen(n, cb),
}));

import {
  MAX_SKIN_BYTES,
  accountsApi,
  accountsEvents,
  parseAccounts,
  parseAppearance,
  skinSize,
  type SignInStage,
} from "./api";
import { BackendError } from "@/lib/ipc";
import { testSkin } from "@/test/testSkin";

const tauri = (on: boolean) => {
  if (on) (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  else delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
};

beforeEach(() => {
  invoke.mockReset();
  listen.mockReset();
});
afterEach(() => {
  tauri(false);
  vi.restoreAllMocks();
});

describe("accounts api", () => {
  it("validates what the backend sends", () => {
    const s = parseAccounts({
      accounts: [
        {
          id: "a",
          username: "Alex",
          uuid: "u",
          needsSignIn: true,
          skin: testSkin(),
        },
        { id: "b", username: "Blake", skin: "https://example.test/skin.png" },
        { id: "c", username: "Casey", skin: "data:image/png;base64,AAAA" },
        { id: "d", username: "Drew", skin: testSkin(65, 65) },
        { id: "e", username: "Eli", skin: testSkin(128, 100) },
        { id: "f", username: "Fran", skin: testSkin(64, 32) },
        { id: "g", username: "Gus", skin: testSkin(256, 256) },
        { id: "h", username: "Hal", skin: "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=" },
        { id: "", username: "x" },
        { id: "y", username: "" },
        "junk",
      ],
      active: "ghost",
      capabilities: { signIn: true },
    });
    expect(s.accounts.map((a) => a.id)).toEqual(["a", "b", "c", "d", "e", "f", "g", "h"]);
    expect(s.accounts[0]).toMatchObject({
      needsSignIn: true,
      skin: testSkin(),
      skinDimensions: { width: 64, height: 64 },
    });
    expect(s.accounts[5].skinDimensions).toEqual({ width: 64, height: 32 });
    expect(s.accounts[6].skinDimensions).toEqual({ width: 256, height: 256 });
    expect(s.accounts[1].skinDimensions).toBeNull();
    // Only an inline PNG of a skin shape is kept: never a URL, never another type, never odd sizes.
    expect(s.accounts.map((a) => a.skin !== null)).toEqual([
      true, //  64x64
      false, // a URL
      false, // not a real PNG
      false, // 65x65
      false, // 128x100
      true, //  64x32 (old format)
      true, //  256x256 (high resolution)
      false, // another image type
    ]);
    expect(s.active).toBe("a");
    expect(s.capabilities).toEqual({ signIn: true, skin: false, cape: false });
    expect(parseAccounts(null)).toEqual({
      accounts: [],
      active: null,
      capabilities: { signIn: false, skin: false, cape: false },
    });
    expect(parseAccounts({ accounts: [], active: "a" })).toEqual({
      accounts: [],
      active: null,
      capabilities: { signIn: false, skin: false, cape: false },
    });

    const coerced = parseAccounts({
      accounts: [
        { id: "a", username: "Alex", uuid: 123, needsSignIn: "true" },
        { id: "b", username: "Blake", needsSignIn: 1 },
        { id: "c", username: "Casey", needsSignIn: {} },
      ],
      active: "b",
      capabilities: { signIn: "true", skin: 1, cape: "yes" },
    });
    expect(coerced.active).toBe("b");
    expect(coerced.capabilities).toEqual({ signIn: false, skin: false, cape: false });
    expect(coerced.accounts).toEqual([
      {
        id: "a",
        username: "Alex",
        uuid: "",
        needsSignIn: false,
        skin: null,
        skinDimensions: null,
      },
      {
        id: "b",
        username: "Blake",
        uuid: "",
        needsSignIn: false,
        skin: null,
        skinDimensions: null,
      },
      {
        id: "c",
        username: "Casey",
        uuid: "",
        needsSignIn: false,
        skin: null,
        skinDimensions: null,
      },
    ]);
  });

  it("invents nothing outside the launcher", async () => {
    const CAPE = "11111111-2222-4333-8444-555555555555";
    const calls: [string, () => Promise<unknown>][] = [
      ["list", () => accountsApi.list()],
      ["select", () => accountsApi.select("a")],
      ["remove", () => accountsApi.remove("a")],
      ["appearance", () => accountsApi.appearance("a")],
      ["setSkin", () => accountsApi.setSkin("a", "classic", new Uint8Array([1]))],
      ["resetSkin", () => accountsApi.resetSkin("a")],
      ["setCape", () => accountsApi.setCape("a", CAPE)],
      ["vaultStatus", () => accountsApi.vaultStatus()],
      ["createVault", () => accountsApi.createVault("test-passphrase")],
      ["unlockVault", () => accountsApi.unlockVault("test-passphrase")],
      ["signIn", () => accountsApi.signIn(null)],
      ["resetVault", () => accountsApi.resetVault()],
      ["cancelSignIn", () => accountsApi.cancelSignIn()],
    ];
    for (const [label, fn] of calls) {
      const err = await fn().catch((e: unknown) => e);
      if (!(err instanceof BackendError)) throw new Error(`${label}: expected BackendError`);
      expect(err.code, label).toBe("backend_unavailable");
      expect(err.message, label).toContain("not available");
    }
    expect(invoke).not.toHaveBeenCalled();

    const offProgress = await accountsApi.onSignInProgress(() => {});
    const offChanged = await accountsEvents.onChanged(() => {});
    expect(offProgress()).toBeUndefined();
    expect(offChanged()).toBeUndefined();
    expect(listen).not.toHaveBeenCalled();
  });

  it("talks to the backend commands inside the launcher", async () => {
    tauri(true);
    const snapshot = {
      accounts: [{ id: "a", username: "Alex", uuid: "u", needsSignIn: false }],
      active: "a",
      capabilities: { signIn: true, skin: true, cape: true },
    };
    invoke.mockResolvedValueOnce(snapshot);
    expect(await accountsApi.list()).toEqual({
      accounts: [
        {
          id: "a",
          username: "Alex",
          uuid: "u",
          needsSignIn: false,
          skin: null,
          skinDimensions: null,
        },
      ],
      active: "a",
      capabilities: { signIn: true, skin: true, cape: true },
    });
    expect(invoke).toHaveBeenLastCalledWith("list_accounts", undefined);

    invoke.mockResolvedValueOnce(snapshot);
    expect((await accountsApi.select("a")).active).toBe("a");
    expect(invoke).toHaveBeenLastCalledWith("select_account", { id: "a" });

    invoke.mockResolvedValueOnce({ accounts: [], active: null, capabilities: { signIn: true } });
    expect(await accountsApi.remove("a")).toEqual({
      accounts: [],
      active: null,
      capabilities: { signIn: true, skin: false, cape: false },
    });
    expect(invoke).toHaveBeenLastCalledWith("remove_account", { id: "a" });

    invoke.mockRejectedValueOnce({
      code: "unknown_account",
      message: "That account no longer exists.",
    });
    await expect(accountsApi.remove("missing")).rejects.toMatchObject({
      code: "unknown_account",
      message: "That account no longer exists.",
    });

    invoke.mockResolvedValueOnce(snapshot);
    expect((await accountsApi.signIn(null)).active).toBe("a");
    expect(invoke).toHaveBeenLastCalledWith("begin_sign_in", { accountId: null });

    invoke.mockResolvedValueOnce(snapshot);
    expect((await accountsApi.signIn("a")).accounts[0]?.id).toBe("a");
    expect(invoke).toHaveBeenLastCalledWith("begin_sign_in", { accountId: "a" });

    invoke.mockRejectedValueOnce({
      code: "approval_required",
      message: "Mojang has not approved this launcher.",
    });
    const failure = await accountsApi.signIn(null).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(BackendError);
    expect(failure).toMatchObject({
      message: "Mojang has not approved this launcher.",
      code: "approval_required",
    });
    expect(invoke).toHaveBeenLastCalledWith("begin_sign_in", { accountId: null });

    invoke.mockResolvedValueOnce(undefined);
    await expect(accountsApi.cancelSignIn()).resolves.toBeUndefined();
    expect(invoke).toHaveBeenLastCalledWith("cancel_sign_in", undefined);
  });

  it("manages the encrypted account vault through backend commands", async () => {
    tauri(true);

    invoke.mockResolvedValueOnce({ exists: true, unlocked: false });
    expect(await accountsApi.vaultStatus()).toEqual({ exists: true, unlocked: false });
    expect(invoke).toHaveBeenLastCalledWith("vault_status", undefined);

    invoke.mockResolvedValueOnce({ exists: true, unlocked: true });
    expect(await accountsApi.vaultStatus()).toEqual({ exists: true, unlocked: true });

    for (const bad of [
      null,
      "yes",
      123,
      {},
      { exists: "true", unlocked: 1 },
      { exists: null, unlocked: "yes" },
    ]) {
      invoke.mockResolvedValueOnce(bad);
      expect(await accountsApi.vaultStatus()).toEqual({ exists: false, unlocked: false });
    }

    invoke.mockResolvedValueOnce(undefined);
    await expect(accountsApi.createVault("correct horse battery")).resolves.toBeUndefined();
    expect(invoke).toHaveBeenLastCalledWith("create_vault", {
      passphrase: "correct horse battery",
    });

    invoke.mockRejectedValueOnce({
      code: "passphrase_too_short",
      message: "That passphrase is too short.",
    });
    await expect(accountsApi.createVault("short")).rejects.toMatchObject({
      code: "passphrase_too_short",
    });

    invoke.mockResolvedValueOnce(undefined);
    await expect(accountsApi.unlockVault("correct horse battery")).resolves.toBeUndefined();
    expect(invoke).toHaveBeenLastCalledWith("unlock_vault", {
      passphrase: "correct horse battery",
    });

    invoke.mockRejectedValueOnce({
      code: "passphrase_mismatch",
      message: "That passphrase doesn't match.",
    });
    await expect(accountsApi.unlockVault("wrong passphrase")).rejects.toMatchObject({
      code: "passphrase_mismatch",
    });

    invoke.mockResolvedValueOnce({
      accounts: [{ id: "a", username: "Alex", needsSignIn: true }],
      active: "a",
      capabilities: { signIn: true },
    });
    expect(await accountsApi.resetVault()).toEqual({
      accounts: [
        {
          id: "a",
          username: "Alex",
          uuid: "",
          needsSignIn: true,
          skin: null,
          skinDimensions: null,
        },
      ],
      active: "a",
      capabilities: { signIn: true, skin: false, cape: false },
    });
    expect(invoke).toHaveBeenLastCalledWith("reset_vault", undefined);

    invoke.mockRejectedValueOnce({
      code: "reset_failed",
      message: "Could not archive the old sign-in store.",
    });
    await expect(accountsApi.resetVault()).rejects.toMatchObject({ code: "reset_failed" });
  });

  it("subscribes to sign-in progress with stage filtering and supports unlisten", async () => {
    tauri(true);
    const unlisten = vi.fn();
    listen.mockResolvedValueOnce(unlisten);

    const stages: SignInStage[] = [];
    const off = await accountsApi.onSignInProgress((stage) => stages.push(stage));
    expect(listen).toHaveBeenCalledWith("sign-in-progress", expect.any(Function));
    const handler = listen.mock.calls[0]?.[1];
    if (!handler) throw new Error("expected sign-in-progress listener");

    for (const payload of [
      "opening",
      "complete",
      "waiting",
      "",
      null,
      undefined,
      42,
      { stage: "opening" },
      "finishing",
    ]) {
      handler({ payload });
    }
    expect(stages).toEqual(["opening", "waiting", "finishing"]);

    off();
    expect(unlisten).toHaveBeenCalledTimes(1);

    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    listen.mockRejectedValueOnce(new Error("event plugin unavailable"));
    const noopOff = await accountsApi.onSignInProgress(() => {});
    expect(noopOff()).toBeUndefined();
    expect(warn).toHaveBeenCalled();
  });

  it("subscribes to accounts-changed events and supports unlisten", async () => {
    tauri(true);
    const unlisten = vi.fn();
    listen.mockResolvedValueOnce(unlisten);

    const onChanged = vi.fn();
    const off = await accountsEvents.onChanged(onChanged);
    expect(listen).toHaveBeenCalledWith("accounts-changed", expect.any(Function));
    const handler = listen.mock.calls[0]?.[1];
    if (!handler) throw new Error("expected accounts-changed listener");

    handler({ payload: null });
    handler({ payload: { ignored: true } });
    expect(onChanged).toHaveBeenCalledTimes(2);
    expect(onChanged).toHaveBeenNthCalledWith(1);
    expect(onChanged).toHaveBeenNthCalledWith(2);

    off();
    expect(unlisten).toHaveBeenCalledTimes(1);

    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    listen.mockRejectedValueOnce(new Error("event plugin unavailable"));
    const noopOff = await accountsEvents.onChanged(() => {});
    expect(noopOff()).toBeUndefined();
    expect(warn).toHaveBeenCalled();
  });
});

describe("skinSize", () => {
  it("reads the size from the PNG header and accepts only skin shapes", () => {
    expect(skinSize(testSkin(64, 64))).toEqual({ width: 64, height: 64 });
    expect(skinSize(testSkin(64, 32))).toEqual({ width: 64, height: 32 });
    expect(skinSize(testSkin(512, 512))).toEqual({ width: 512, height: 512 });
    expect(skinSize(testSkin(1024, 1024))).toEqual({ width: 1024, height: 1024 });
    expect(skinSize(testSkin(1024, 512))).toEqual({ width: 1024, height: 512 });
    expect(skinSize(testSkin(64, 64, 10))).toEqual({ width: 64, height: 64 });
    expect(skinSize(testSkin(64, 64, 100))).toEqual({ width: 64, height: 64 });
    for (const [w, h] of [
      [0, 0],
      [63, 63],
      [64, 48],
      [96, 96],
      [1024, 256],
      [1088, 1088],
      [2048, 2048],
      [64, 128],
    ])
      expect(skinSize(testSkin(w, h)), `${w}x${h}`).toBeNull();
    expect(skinSize("https://example.test/skin.png")).toBeNull();
    expect(skinSize("data:image/png;base64,!!!")).toBeNull();
    expect(skinSize(`data:image/png;base64,${"!".repeat(32)}`)).toBeNull();
    expect(skinSize(`data:image/png;base64,${"A".repeat(28)}`)).toBeNull();
    expect(
      skinSize(`data:image/png;base64,${btoa("not a png at all, just text of some length")}`),
    ).toBeNull();
    // Rejects unaligned lengths, embedded '=' padding, bad trailing chars, and oversized payloads.
    const valid = testSkin(64, 64);
    expect(skinSize(`${valid}A`)).toBeNull();
    expect(skinSize(valid.slice(0, -1))).toBeNull();
    expect(skinSize(`${valid.slice(0, -8)}=${valid.slice(-7)}`)).toBeNull();
    expect(skinSize(`${valid.slice(0, -4)}=AAA`)).toBeNull();
    expect(skinSize(`${valid.slice(0, -3)}===`)).toBeNull();
    expect(skinSize(`${valid.slice(0, -4)}$$$$`)).toBeNull();
    const oversized = `${valid.replace(/=+$/, "")}${"A".repeat(Math.ceil((MAX_SKIN_BYTES * 4) / 3) + 128)}`;
    expect(skinSize(oversized)).toBeNull();
  });
});

describe("appearance api", () => {
  const CAPE = "11111111-2222-4333-8444-555555555555";

  it("reads the model and the owned capes, and ignores anything else", () => {
    expect(
      parseAppearance({
        skinVariant: "slim",
        capes: [
          { id: CAPE, alias: "Migrator", active: true, url: "https://example.test/cape.png" },
          { id: "not-an-id", alias: "bad", active: false },
          { alias: "no id" },
          "junk",
          { id: CAPE.toUpperCase(), alias: "", active: "yes" },
        ],
        token: "FAKE-TOKEN",
      }),
    ).toEqual({
      skinModel: "slim",
      capes: [
        { id: CAPE, alias: "Migrator", active: true },
        { id: CAPE.toUpperCase(), alias: null, active: false },
      ],
    });
    expect(parseAppearance(null)).toEqual({ skinModel: null, capes: [] });
    expect(parseAppearance({ skinVariant: "wide" }).skinModel).toBeNull();
    expect(parseAppearance({ skinVariant: "classic" }).skinModel).toBe("classic");
  });

  it("sends appearance payloads and parses every response", async () => {
    tauri(true);
    invoke
      .mockResolvedValueOnce({
        skinVariant: "classic",
        capes: [
          { id: CAPE, alias: "Migrator", active: false, url: "https://example.test/cape.png" },
          { id: "bad-id", alias: "ignored", active: true },
        ],
        token: "FAKE-TOKEN",
      })
      .mockResolvedValueOnce({
        skinVariant: "slim",
        capes: [{ id: CAPE, alias: "Migrator", active: false }],
      })
      .mockResolvedValueOnce({ skinVariant: null, capes: [] })
      .mockResolvedValueOnce({
        skinVariant: "classic",
        capes: [{ id: CAPE, alias: "Migrator", active: true }],
      })
      .mockResolvedValueOnce({
        skinVariant: "classic",
        capes: [{ id: CAPE, alias: "Migrator", active: false }],
      });

    expect(await accountsApi.appearance("acc")).toEqual({
      skinModel: "classic",
      capes: [{ id: CAPE, alias: "Migrator", active: false }],
    });
    expect(await accountsApi.setSkin("acc", "slim", new Uint8Array([1, 2, 3]))).toEqual({
      skinModel: "slim",
      capes: [{ id: CAPE, alias: "Migrator", active: false }],
    });
    expect(await accountsApi.resetSkin("acc")).toEqual({ skinModel: null, capes: [] });
    expect(await accountsApi.setCape("acc", CAPE)).toEqual({
      skinModel: "classic",
      capes: [{ id: CAPE, alias: "Migrator", active: true }],
    });
    expect(await accountsApi.setCape("acc", null)).toEqual({
      skinModel: "classic",
      capes: [{ id: CAPE, alias: "Migrator", active: false }],
    });

    expect(invoke.mock.calls).toEqual([
      ["account_appearance", { id: "acc" }],
      ["set_account_skin", { id: "acc", variant: "slim", png: [1, 2, 3] }],
      ["reset_account_skin", { id: "acc" }],
      ["set_account_cape", { id: "acc", capeId: CAPE }],
      ["set_account_cape", { id: "acc", capeId: null }],
    ]);
  });

  it("surfaces backend errors for appearance commands", async () => {
    tauri(true);

    invoke.mockRejectedValueOnce({
      code: "unknown_account",
      message: "That account no longer exists.",
    });
    await expect(accountsApi.appearance("missing")).rejects.toMatchObject({
      code: "unknown_account",
      message: "That account no longer exists.",
    });

    invoke.mockRejectedValueOnce({
      code: "invalid_skin",
      message: "Choose a 64x64 or 64x32 PNG skin.",
    });
    await expect(accountsApi.setSkin("acc", "slim", new Uint8Array([0]))).rejects.toMatchObject({
      code: "invalid_skin",
      message: "Choose a 64x64 or 64x32 PNG skin.",
    });

    invoke.mockRejectedValueOnce({ code: "busy", message: "Another change is still running." });
    await expect(accountsApi.resetSkin("acc")).rejects.toMatchObject({
      code: "busy",
      message: "Another change is still running.",
    });

    invoke.mockRejectedValueOnce({ code: "busy", message: "Another change is still running." });
    await expect(accountsApi.setCape("acc", CAPE)).rejects.toMatchObject({
      code: "busy",
      message: "Another change is still running.",
    });
  });
});
