import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { READY_ACCOUNTS, readyAccountsClient } from "@/lib/accounts/testAccounts";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import { Modal } from "./Modal";
import { VaultRecovery } from "./VaultRecovery";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

function setup(layout: "inline" | "modal", resetVault?: () => Promise<typeof READY_ACCOUNTS>) {
  const api = readyAccountsClient();
  if (resetVault) api.resetVault = vi.fn(resetVault);
  const onAsking = vi.fn();
  const onDone = vi.fn();
  const view = render(
    <Modal open onOpenChange={() => undefined}>
      <VaultRecovery api={api} layout={layout} onAsking={onAsking} onDone={onDone}>
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
  return { api, onAsking, onDone, ...view };
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

  it("puts focus on the safe answer, and Keep trying brings the form back untouched", () => {
    const { api, onAsking } = setup(layout);
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep trying" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep trying" }));
    expect(onAsking).toHaveBeenLastCalledWith(false);
    expect(screen.getByLabelText("Passphrase")).toBeTruthy();
    expect(api.resetVault).not.toHaveBeenCalled();
  });

  it("only Start fresh resets, once, while a second click is ignored", async () => {
    let done: (v: typeof READY_ACCOUNTS) => void = () => undefined;
    const { api, onDone, onAsking } = setup(layout, () => new Promise((r) => (done = r)));
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    const confirm = screen.getByRole("button", { name: "Start fresh" });
    fireEvent.click(confirm);
    fireEvent.click(confirm);
    expect(api.resetVault).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Working…" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Keep trying" })).toHaveProperty("disabled", true);
    await act(() => Promise.resolve(done(READY_ACCOUNTS)));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(onAsking).toHaveBeenLastCalledWith(false);
  });

  it("a failed reset says why, keeps the question open and can be tried again", async () => {
    const fail = vi
      .fn<() => Promise<typeof READY_ACCOUNTS>>()
      .mockRejectedValueOnce(new BackendError("no", "reset_failed"))
      .mockResolvedValue(READY_ACCOUNTS);
    const { api, onDone } = setup(layout, fail);
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Clade couldn’t archive the old sign-in store, so nothing was changed. Close anything using the Clade folder and try again.",
    );
    expect(onDone).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(api.resetVault).toHaveBeenCalledTimes(2);
  });
});

describe("Forgot passphrase wording and cleanup", () => {
  it("is worded in Russian", () => {
    setLocale("ru");
    setup("inline");
    fireEvent.click(screen.getByRole("button", { name: "Забыл парольную фразу?" }));
    expect(screen.getByText("Начать заново?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Продолжить попытки" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Начать заново" })).toBeTruthy();
    expect(screen.getByText(/Профили, миры, время игры и настройки сохранятся\./)).toBeTruthy();
  });

  it("never leaves the dialog around it locked when it goes away while asking", () => {
    const { onAsking, unmount } = setup("inline");
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    unmount();
    expect(onAsking).toHaveBeenLastCalledWith(false);
  });

  it("does not claim that Microsoft sessions are revoked", () => {
    setup("inline");
    fireEvent.click(screen.getByRole("button", { name: "Forgot passphrase?" }));
    expect(document.body.textContent).not.toMatch(/revok|sign(ed)? out everywhere/i);
  });
});
