import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_SKIN_BYTES, type AccountsSnapshot, type Appearance } from "@/lib/accounts/api";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { READY_ACCOUNTS, readyAccountsClient } from "@/test/testAccounts";
import { AccountAppearance } from "./AccountAppearance";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const CAPE = "11111111-2222-4333-8444-555555555555";
const CAPE_2 = "22222222-3333-4444-8555-666666666666";
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
  over: Partial<ReturnType<typeof readyAccountsClient>> = {},
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

const choose = (name = "skin.png", bytes = 32) => {
  const input = document.querySelector<HTMLInputElement>('input[type="file"]');
  if (!input) throw new Error("no file input");
  const file = new File([new Uint8Array(bytes)], name, { type: "image/png" });
  Object.defineProperty(file, "arrayBuffer", {
    value: () => Promise.resolve(new ArrayBuffer(bytes)),
  });
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

  it("rejects an oversized skin file without uploading and hides Try again", async () => {
    const { api } = setup();
    await screen.findByText("Migrator");
    choose("huge.png", MAX_SKIN_BYTES + 1);
    expect(api.setSkin).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toBe("Choose a 64×64 or 64×32 PNG skin.");
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });

  it("shows a load error and reloads appearance when Try again is clicked", async () => {
    const appearance = vi
      .fn()
      .mockRejectedValueOnce(new BackendError("offline", "offline"))
      .mockResolvedValueOnce(look);
    const { api } = setup({ appearance });
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Connect to the internet to change your Minecraft appearance.",
    );
    expect(screen.queryByLabelText("Passphrase")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByText("Migrator");
    expect(api.appearance).toHaveBeenCalledTimes(2);
    expect(api.appearance).toHaveBeenCalledWith("acc");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("disables Unlock while the passphrase is empty and shows a vault error when unlocking fails", async () => {
    const appearance = vi.fn(() => Promise.reject(new BackendError("locked", "vault_locked")));
    const unlockVault = vi.fn(() => Promise.reject(new BackendError("bad", "passphrase_mismatch")));
    const { api } = setup({ appearance, unlockVault });
    const input = await screen.findByLabelText<HTMLInputElement>("Passphrase");
    const unlock = screen.getByRole("button", { name: "Unlock" });
    expect(unlock).toHaveProperty("disabled", true);
    expect(input.getAttribute("aria-invalid")).toBeNull();
    expect(input.getAttribute("aria-describedby")).toBeNull();

    fireEvent.submit(input.closest("form")!);
    expect(api.unlockVault).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "wrong-pass" } });
    expect(unlock).toHaveProperty("disabled", false);
    fireEvent.click(unlock);

    await waitFor(() => expect(api.unlockVault).toHaveBeenCalledWith("wrong-pass"));
    const error = await screen.findByText("That passphrase doesn’t match.");
    expect(error.getAttribute("role")).toBe("alert");
    expect(error.id).toBe("appearance-unlock-error");
    expect(input.value).toBe("");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toBe("appearance-unlock-error");
    expect(unlock).toHaveProperty("disabled", true);
    expect(document.activeElement).toBe(input);
  });

  it("unlocks a locked vault with the passphrase and reloads appearance", async () => {
    const appearance = vi
      .fn()
      .mockRejectedValueOnce(new BackendError("locked", "vault_locked"))
      .mockResolvedValueOnce(look);
    const unlockVault = vi.fn(() => Promise.resolve());
    const { api } = setup({ appearance, unlockVault });
    const input = await screen.findByLabelText("Passphrase");
    fireEvent.change(input, { target: { value: "correct-passphrase" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await waitFor(() => expect(api.unlockVault).toHaveBeenCalledWith("correct-passphrase"));
    await screen.findByText("Migrator");
    expect(api.appearance).toHaveBeenCalledTimes(2);
    expect(screen.queryByLabelText("Passphrase")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows Equipped, labels unnamed capes, and hides an active cape via Hide cape", async () => {
    const activeLook: Appearance = {
      skinModel: "slim",
      capes: [
        { id: CAPE, alias: "Migrator", active: true },
        { id: CAPE_2, alias: null, active: false },
      ],
    };
    const hiddenLook: Appearance = {
      skinModel: "slim",
      capes: [
        { id: CAPE, alias: "Migrator", active: false },
        { id: CAPE_2, alias: null, active: false },
      ],
    };
    const setCape = vi.fn(() => Promise.resolve(hiddenLook));
    const { api } = setup({
      appearance: vi.fn(() => Promise.resolve(activeLook)),
      setCape,
    });
    await screen.findByText("Migrator");
    expect(screen.getByText("Equipped")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Equip Migrator" })).toBeNull();
    expect(screen.getByText("Cape 2")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Equip Cape 2" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Hide cape" }));
    await waitFor(() => expect(api.setCape).toHaveBeenCalledWith("acc", null));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Hide cape" })).toBeNull());
    expect(screen.queryByText("Equipped")).toBeNull();
    expect(screen.getByRole("button", { name: "Equip Migrator" })).toBeTruthy();
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

  it("disables Forgot passphrase while inline unlock is in flight or when AccountAppearance is disabled", async () => {
    let rejectUnlock!: (err: unknown) => void;
    const appearance = vi.fn(() => Promise.reject(new BackendError("locked", "vault_locked")));
    const unlockVault = vi.fn(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectUnlock = reject;
        }),
    );
    const { api } = setup({ appearance, unlockVault });

    const input = await screen.findByLabelText("Passphrase");
    fireEvent.change(input, { target: { value: "attempt" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));

    const forgot = screen.getByRole<HTMLButtonElement>("button", { name: "Forgot passphrase?" });
    expect(forgot.disabled).toBe(true);

    await act(async () => {
      rejectUnlock(new BackendError("bad", "passphrase_mismatch"));
      await Promise.resolve();
    });
    expect(forgot.disabled).toBe(false);
    expect(document.activeElement).toBe(input);

    cleanup();
    const disabledApi = {
      ...api,
      appearance: vi.fn(() => Promise.reject(new BackendError("locked", "vault_locked"))),
    };
    render(
      <Harness account={{ id: "acc", needsSignIn: false }} skin cape api={disabledApi} disabled />,
    );
    expect(
      (await screen.findByRole<HTMLButtonElement>("button", { name: "Forgot passphrase?" }))
        .disabled,
    ).toBe(true);
  });
});
