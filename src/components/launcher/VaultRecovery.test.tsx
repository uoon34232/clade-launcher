import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { READY_ACCOUNTS, readyAccountsClient } from "@/test/testAccounts";
import { Modal } from "./Modal";
import { VaultRecovery } from "./VaultRecovery";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

function setup(layout: "inline" | "modal", resetVault?: () => Promise<typeof READY_ACCOUNTS>) {
  const api = readyAccountsClient();
  if (resetVault) api.resetVault = vi.fn(resetVault);
  const onAsking = vi.fn();
  const onReset = vi.fn();
  const view = render(
    <Modal open onOpenChange={() => undefined}>
      <VaultRecovery api={api} layout={layout} onAsking={onAsking} onReset={onReset}>
        {(link) => (
          <form>
            <label>
              Passphrase
              <input type="password" />
            </label>
            {link}
          </form>
        )}
      </VaultRecovery>
    </Modal>,
  );
  return { api, onAsking, onReset, ...view };
}

const question =
  "Clade can’t recover a forgotten passphrase. Starting fresh archives the old encrypted sign-in store and stops using those saved sign-ins in this Clade folder. You’ll sign in to your Microsoft accounts again. Your profiles, worlds, playtime and settings are kept.";

beforeEach(() => setLocale("en"));
afterEach(() => cleanup());

describe.each(["inline", "modal"] as const)("Forgot passphrase (%s)", (layout) => {
  it("shows a quiet link under the normal form, and asking changes nothing yet", () => {
    const { api, onAsking } = setup(layout);
    expect(screen.getByLabelText("Passphrase")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(onAsking).toHaveBeenLastCalledWith(true);
    expect(screen.getByText("Start fresh?")).toBeTruthy();
    expect(screen.getByText(question)).toBeTruthy();
    expect(screen.queryByLabelText("Passphrase")).toBeNull();
    expect(api.resetVault).not.toHaveBeenCalled();
  });

  it("puts focus on the safe answer, and Keep trying brings the form back and restores focus", () => {
    const { api, onAsking } = setup(layout);
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep trying" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep trying" }));
    expect(onAsking).toHaveBeenLastCalledWith(false);
    expect(screen.getByLabelText("Passphrase")).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(api.resetVault).not.toHaveBeenCalled();
  });

  it("only Start fresh resets, once, while a second click is ignored", async () => {
    let done: (v: typeof READY_ACCOUNTS) => void = () => undefined;
    const { api, onReset, onAsking } = setup(layout, () => new Promise((r) => (done = r)));
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    const confirm = screen.getByRole("button", { name: "Start fresh" });
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(api.resetVault).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Working…" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Keep trying" })).toHaveProperty("disabled", true);
    await act(() => Promise.resolve(done(READY_ACCOUNTS)));
    await waitFor(() => expect(onReset).toHaveBeenCalledTimes(1));
    expect(onReset).toHaveBeenCalledWith(READY_ACCOUNTS);
    expect(onAsking).toHaveBeenLastCalledWith(false);
  });

  it("a failed reset says why, keeps the question open and can be tried again", async () => {
    const fail = vi
      .fn<() => Promise<typeof READY_ACCOUNTS>>()
      .mockRejectedValueOnce(new BackendError("no", "reset_failed"))
      .mockResolvedValue(READY_ACCOUNTS);
    const { api, onReset } = setup(layout, fail);
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Clade couldn’t archive the old sign-in store, so nothing was changed. Close anything using the Clade folder and try again.",
    );
    expect(screen.getByRole("button", { name: "Keep trying" })).toHaveProperty("disabled", false);
    expect(onReset).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    await waitFor(() => expect(onReset).toHaveBeenCalledWith(READY_ACCOUNTS));
    expect(api.resetVault).toHaveBeenCalledTimes(2);
  });

  it("shows the fallback vault error when resetVault rejects with a non-BackendError", async () => {
    const fail = vi
      .fn<() => Promise<typeof READY_ACCOUNTS>>()
      .mockRejectedValueOnce(new Error("raw failure detail"))
      .mockResolvedValue(READY_ACCOUNTS);
    const { api, onReset } = setup(layout, fail);
    await waitFor(() =>
      expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    expect((await screen.findByRole("alert")).textContent).toBe("That didn’t work.");
    expect(screen.getByRole("button", { name: "Keep trying" })).toHaveProperty("disabled", false);
    expect(onReset).not.toHaveBeenCalled();
    const confirm = screen.getByRole("button", { name: "Start fresh" });
    expect(document.activeElement).toBe(confirm);
    fireEvent.click(confirm);
    await waitFor(() => expect(onReset).toHaveBeenCalledWith(READY_ACCOUNTS));
    expect(api.resetVault).toHaveBeenCalledTimes(2);
  });

  it("hides Keep trying when the vault was already archived (accounts_not_updated), leaving Start fresh to retry", async () => {
    const fail = vi
      .fn<() => Promise<typeof READY_ACCOUNTS>>()
      .mockRejectedValueOnce(new BackendError("no", "accounts_not_updated"))
      .mockResolvedValue(READY_ACCOUNTS);
    const { api, onReset } = setup(layout, fail);
    await waitFor(() =>
      expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "The old sign-in store was archived, but the account list couldn’t be updated. Try Start fresh again.",
    );
    expect(screen.queryByRole("button", { name: "Keep trying" })).toBeNull();
    const confirm = screen.getByRole("button", { name: "Start fresh" });
    expect(confirm).toHaveProperty("disabled", false);
    expect(document.activeElement).toBe(confirm);
    fireEvent.click(confirm);
    await waitFor(() => expect(onReset).toHaveBeenCalledWith(READY_ACCOUNTS));
    expect(api.resetVault).toHaveBeenCalledTimes(2);
  });
});

describe("Forgot passphrase wording and cleanup", () => {
  it.each(["inline", "modal"] as const)("is worded in Russian (%s)", (layout) => {
    setLocale("ru");
    setup(layout);
    fireEvent.click(screen.getByRole("button", { name: "Не помнишь парольную фразу?" }));
    expect(screen.getByText("Начать заново?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Продолжить попытки" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Начать заново" })).toBeTruthy();
    expect(screen.getByText(/Профили, миры, время в игре и настройки сохранятся\./)).toBeTruthy();
  });

  it("never leaves the dialog around it locked when it goes away while asking", () => {
    const { onAsking, unmount } = setup("inline");
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    unmount();
    expect(onAsking).toHaveBeenLastCalledWith(false);
  });

  it("ignores an in-flight resetVault resolution after unmount", async () => {
    let resolveReset: (v: typeof READY_ACCOUNTS) => void = () => undefined;
    const { onAsking, onReset, unmount } = setup(
      "inline",
      () => new Promise((r) => (resolveReset = r)),
    );
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    unmount();
    expect(onAsking).toHaveBeenLastCalledWith(false);
    const askingCallsAfterUnmount = onAsking.mock.calls.length;

    await act(() => {
      resolveReset(READY_ACCOUNTS);
      return Promise.resolve();
    });

    expect(onReset).not.toHaveBeenCalled();
    expect(onAsking).toHaveBeenCalledTimes(askingCallsAfterUnmount);
  });

  it("ignores an in-flight resetVault rejection after unmount", async () => {
    let rejectReset: (err: unknown) => void = () => undefined;
    const { onAsking, onReset, unmount } = setup(
      "inline",
      () => new Promise((_, rej) => (rejectReset = rej)),
    );
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    unmount();
    expect(onAsking).toHaveBeenLastCalledWith(false);
    const askingCallsAfterUnmount = onAsking.mock.calls.length;

    await act(() => {
      rejectReset(new BackendError("no", "accounts_not_updated"));
      return Promise.resolve();
    });

    expect(onReset).not.toHaveBeenCalled();
    expect(onAsking).toHaveBeenCalledTimes(askingCallsAfterUnmount);
  });

  it("does not claim that Microsoft sessions are revoked", () => {
    setup("inline");
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(document.body.textContent).not.toMatch(/revok|sign(ed)? out everywhere/i);
  });
});
