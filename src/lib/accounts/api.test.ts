import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));

import { accountsApi, parseAccounts } from "./api";

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
          head: "data:image/png;base64,AAAA",
        },
        { id: "b", username: "Blake", head: "https://example.test/head.png" },
        { id: "", username: "x" },
        "junk",
      ],
      active: "ghost",
      capabilities: { signIn: true },
    });
    expect(s.accounts.map((a) => a.id)).toEqual(["a", "b"]);
    expect(s.accounts[0]).toMatchObject({ needsSignIn: true, head: "data:image/png;base64,AAAA" });
    expect(s.accounts[1].head).toBeNull();
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
