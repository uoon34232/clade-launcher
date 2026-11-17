import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));

import { MAX_SKIN_BYTES, accountsApi, parseAccounts, parseAppearance, skinSize } from "./api";
import { testSkin } from "@/test/testSkin";

const tauri = (on: boolean) => {
  if (on) (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  else delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
};

beforeEach(() => invoke.mockReset());
afterEach(() => tauri(false));

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
  });

  it("invents nothing outside the launcher", async () => {
    await expect(accountsApi.list()).rejects.toThrow("not available");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("talks to the backend commands inside the launcher", async () => {
    tauri(true);
    invoke.mockResolvedValue({
      accounts: [{ id: "a", username: "Alex" }],
      active: "a",
      capabilities: {},
    });
    expect((await accountsApi.select("a")).active).toBe("a");
    expect(invoke).toHaveBeenCalledWith("select_account", { id: "a" });
    invoke.mockRejectedValueOnce({
      code: "approval_required",
      message: "Mojang has not approved this launcher.",
    });
    const failure = await accountsApi.signIn(null).catch((e: unknown) => e);
    expect(failure).toMatchObject({
      message: "Mojang has not approved this launcher.",
      code: "approval_required",
    });
    expect(invoke).toHaveBeenLastCalledWith("begin_sign_in", { accountId: null });
  });
});

describe("skinSize", () => {
  it("reads the size from the PNG header and accepts only skin shapes", () => {
    expect(skinSize(testSkin(64, 64))).toEqual({ width: 64, height: 64 });
    expect(skinSize(testSkin(64, 32))).toEqual({ width: 64, height: 32 });
    expect(skinSize(testSkin(512, 512))).toEqual({ width: 512, height: 512 });
    for (const [w, h] of [
      [0, 0],
      [63, 63],
      [64, 48],
      [96, 96],
      [2048, 2048],
      [64, 128],
    ])
      expect(skinSize(testSkin(w, h)), `${w}x${h}`).toBeNull();
    expect(skinSize("https://example.test/skin.png")).toBeNull();
    expect(skinSize("data:image/png;base64,!!!")).toBeNull();
    expect(
      skinSize(`data:image/png;base64,${btoa("not a png at all, just text of some length")}`),
    ).toBeNull();
    // Rejects malformed trailing characters and oversized payloads without backtracking.
    const valid = testSkin(64, 64);
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

  it("sends only an account id, a model, bytes or an owned cape id to the backend", async () => {
    tauri(true);
    invoke.mockResolvedValue({ skinVariant: null, capes: [] });
    await accountsApi.appearance("acc");
    await accountsApi.setSkin("acc", "slim", new Uint8Array([1, 2, 3]));
    await accountsApi.resetSkin("acc");
    await accountsApi.setCape("acc", CAPE);
    await accountsApi.setCape("acc", null);
    expect(invoke.mock.calls).toEqual([
      ["account_appearance", { id: "acc" }],
      ["set_account_skin", { id: "acc", variant: "slim", png: [1, 2, 3] }],
      ["reset_account_skin", { id: "acc" }],
      ["set_account_cape", { id: "acc", capeId: CAPE }],
      ["set_account_cape", { id: "acc", capeId: null }],
    ]);
  });
});
