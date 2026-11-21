import { useState } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountsSnapshot, Appearance } from "@/lib/accounts/api";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { READY_ACCOUNTS, readyAccountsClient, type appearanceStubs } from "@/test/testAccounts";
import { AccountAppearance } from "./AccountAppearance";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const CAPE = "11111111-2222-4333-8444-555555555555";
const look: Appearance = {
  skinModel: "slim",
  capes: [{ id: CAPE, alias: "Migrator", active: false }],
};

/** The dialog holds the "restore the default skin?" question; this stands in for it. */
function Harness(
  props: Omit<
    Parameters<typeof AccountAppearance>[0],
    "confirmingReset" | "setConfirmingReset" | "setRecovering"
  >,
) {
  const [confirmingReset, setConfirmingReset] = useState(false);
  const [, setRecovering] = useState(false);
  return (
    <AccountAppearance
      {...props}
      confirmingReset={confirmingReset}
      setConfirmingReset={setConfirmingReset}
      setRecovering={setRecovering}
    />
  );
}

function setup(
  over: Partial<ReturnType<typeof appearanceStubs>> = {},
  needsSignIn = false,
  onVaultReset?: (snapshot: AccountsSnapshot) => void,
) {
  const api = { ...readyAccountsClient(), ...over };
  api.appearance = over.appearance ?? vi.fn(() => Promise.resolve(look));
  const view = render(
    <Harness
      account={{ id: "acc", needsSignIn }}
      skin
      cape
      api={api}
      onVaultReset={onVaultReset}
    />,
  );
  return { api, ...view };
}

const choose = (name = "skin.png") => {
  const input = document.querySelector<HTMLInputElement>('input[type="file"]');
  if (!input) throw new Error("no file input");
  const file = new File([new Uint8Array(32)], name, { type: "image/png" });
  Object.defineProperty(file, "arrayBuffer", { value: () => Promise.resolve(new ArrayBuffer(32)) });
  fireEvent.change(input, { target: { files: [file] } });
};

beforeEach(() => setLocale("en"));
afterEach(() => cleanup());

describe("account appearance", () => {
  it("reflects the account's current model and lists the capes it owns", async () => {
    setup();
    await waitFor(() =>
      expect(screen.getByRole("radio", { name: "Slim" }).getAttribute("aria-checked")).toBe("true"),
    );
    expect(screen.getByText("Migrator")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Equip Migrator" })).toBeTruthy();
  });

  it("uploads with the chosen model and never twice at once", async () => {
    let finish: (l: Appearance) => void = () => undefined;
    const setSkin = vi.fn(() => new Promise<Appearance>((r) => (finish = r)));
    const { api } = setup({ setSkin });
    await waitFor(() => expect(screen.getByText("Migrator")).toBeTruthy());
    fireEvent.click(screen.getByRole("radio", { name: "Classic" }));
    choose();
    await waitFor(() => expect(api.setSkin).toHaveBeenCalledTimes(1));
    expect(api.setSkin).toHaveBeenCalledWith("acc", "classic", expect.any(Uint8Array));
    expect(screen.getByRole("button", { name: "Uploading…" })).toHaveProperty("disabled", true);
    choose("second.png"); // meanwhile: ignored
    finish({ skinModel: "classic", capes: [] });
    await waitFor(() =>
      expect(screen.getByText("No capes available for this account.")).toBeTruthy(),
    );
    expect(api.setSkin).toHaveBeenCalledTimes(1);
  });

  it("keeps the look and says it in words when a change is refused", async () => {
    const setCape = vi.fn(() => Promise.reject(new BackendError("no", "cape_not_owned")));
    setup({ setCape });
    fireEvent.click(await screen.findByRole("button", { name: "Equip Migrator" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "This account doesn’t own that cape.",
    );
    expect(screen.getByRole("button", { name: "Equip Migrator" })).toBeTruthy();
  });

  it("asks nothing and offers nothing for an account that has to sign in again", () => {
    const { api } = setup({}, true);
    expect(api.appearance).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toBe(
      "Sign in again to change your Minecraft appearance.",
    );
    expect(screen.getByRole("button", { name: "Choose PNG…" })).toHaveProperty("disabled", true);
  });

  it("speaks Russian", async () => {
    setLocale("ru");
    setup();
    expect(await screen.findByRole("button", { name: "Надеть «Migrator»" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Выбрать PNG…" })).toBeTruthy();
  });

  it("forwards the updated accounts snapshot when Start fresh resets a locked vault", async () => {
    const onVaultReset = vi.fn();
    const appearance = vi.fn(() => Promise.reject(new BackendError("locked", "vault_locked")));
    setup({ appearance }, false, onVaultReset);
    fireEvent.click(await screen.findByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    await waitFor(() => expect(onVaultReset).toHaveBeenCalledWith(READY_ACCOUNTS));
  });
});

describe("restoring the default skin", () => {
  const restore = () => screen.getByRole("button", { name: "Restore default" });
  const question =
    "Restore the default skin? Your current custom skin will be removed from this Minecraft account.";

  it("asks first: the first click changes nothing on the account", async () => {
    const { api } = setup();
    await screen.findByText("Migrator");
    fireEvent.click(restore());
    expect(screen.getByText(question)).toBeTruthy();
    expect(api.resetSkin).not.toHaveBeenCalled();
    // the other appearance actions wait for the answer
    expect(screen.getByRole("button", { name: "Choose PNG…" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Equip Migrator" })).toHaveProperty("disabled", true);
  });

  it("Keep current skin closes the question, restores focus and changes nothing", async () => {
    const { api } = setup();
    await screen.findByText("Migrator");
    fireEvent.click(restore());
    fireEvent.click(screen.getByRole("button", { name: "Keep current skin" }));
    expect(screen.queryByText(question)).toBeNull();
    expect(document.activeElement).toBe(restore());
    expect(api.resetSkin).not.toHaveBeenCalled();
    expect(screen.getByRole("radio", { name: "Slim" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("button", { name: "Choose PNG…" })).toHaveProperty("disabled", false);
  });

  it("only the explicit confirmation resets, once, and a second click is ignored while it runs", async () => {
    let finish: (l: Appearance) => void = () => undefined;
    const resetSkin = vi.fn(() => new Promise<Appearance>((r) => (finish = r)));
    const { api } = setup({ resetSkin });
    await screen.findByText("Migrator");
    fireEvent.click(restore());
    const confirm = screen.getAllByRole("button", { name: "Restore default" })[1];
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(api.resetSkin).toHaveBeenCalledTimes(1);
    expect(api.resetSkin).toHaveBeenCalledWith("acc");
    expect(screen.getByRole("button", { name: "Restoring…" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Keep current skin" })).toHaveProperty(
      "disabled",
      true,
    );
    finish({ skinModel: null, capes: [] });
    await waitFor(() => expect(screen.queryByText(question)).toBeNull());
    expect(api.resetSkin).toHaveBeenCalledTimes(1);
  });

  it("a refused reset keeps the current look and says why", async () => {
    const resetSkin = vi.fn(() => Promise.reject(new BackendError("no", "rate_limited")));
    setup({ resetSkin });
    await screen.findByText("Migrator");
    fireEvent.click(restore());
    fireEvent.click(screen.getAllByRole("button", { name: "Restore default" })[1]);
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Too many skin changes. Try again later.",
    );
    expect(screen.queryByText(question)).toBeNull();
    expect(screen.getByRole("radio", { name: "Slim" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText("Migrator")).toBeTruthy();
  });

  it("is worded in Russian", async () => {
    setLocale("ru");
    const { api } = setup();
    await screen.findByText("Migrator");
    fireEvent.click(screen.getByRole("button", { name: "Вернуть стандартный" }));
    expect(
      screen.getByText(
        "Вернуть стандартный скин? Текущий скин будет удалён с этого аккаунта Minecraft.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Оставить текущий скин" })).toBeTruthy();
    expect(api.resetSkin).not.toHaveBeenCalled();
  });
});
