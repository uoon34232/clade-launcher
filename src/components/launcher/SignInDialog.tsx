import {
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type SubmitEvent,
  type RefObject,
} from "react";
import type { AccountsApi, AccountsSnapshot, SignInStage } from "@/lib/accounts/api";
import { useT, type MessageKey } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { BackendError } from "@/lib/ipc";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT, FIELD, LABEL } from "./modalStyles";
import { VaultRecovery } from "./VaultRecovery";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Set to sign an existing account in again (it must be the same Minecraft account). */
  account: { id: string; username: string } | null;
  onSignedIn: (snapshot: AccountsSnapshot) => void;
  api: AccountsApi;
}

type Phase =
  | { kind: "checking" }
  | { kind: "create" }
  | { kind: "unlock" }
  | { kind: "working"; stage: SignInStage }
  | { kind: "error"; code: string | null; message: string };

const STAGE_TEXT: Record<SignInStage, MessageKey> = {
  opening: "signin.stage.opening",
  waiting: "signin.stage.waiting",
  finishing: "signin.stage.finishing",
};

const MIN_PASSPHRASE = 8;

/**
 * "+ Add account" and "Sign in again". The sign-in itself is Microsoft's own page in the system
 * browser; this modal only shows where things stand. Because the account's credentials are
 * stored in the encrypted vault, a passphrase is asked for first (created the first time,
 * entered after that); it is sent to the backend and never kept here.
 */
export function SignInDialog({ open, onOpenChange, account, onSignedIn, api }: Props) {
  // While a sign-in runs, closing the modal (Escape, backdrop) cancels it.
  const cancelRef = useRef<(() => void) | null>(null);
  // "Start fresh?" is showing: Escape and the backdrop leave it alone.
  const [asking, setAsking] = useState(false);

  return (
    <Modal
      open={open}
      onOpenChange={(o) => {
        if (!o) cancelRef.current?.();
        onOpenChange(o);
      }}
      size="sm"
      closeOnBackdrop={false}
      locked={asking}
    >
      <Flow
        account={account}
        api={api}
        cancelRef={cancelRef}
        onAsking={setAsking}
        onClose={() => onOpenChange(false)}
        onSignedIn={onSignedIn}
      />
    </Modal>
  );
}

function Flow({
  account,
  api,
  cancelRef,
  onAsking,
  onClose,
  onSignedIn,
}: Pick<Props, "account" | "api" | "onSignedIn"> & {
  cancelRef: RefObject<(() => void) | null>;
  onAsking: (asking: boolean) => void;
  onClose: () => void;
}) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: "checking" });
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const start = () => {
    setPhase({ kind: "working", stage: "opening" });
    let stop: (() => void) | null = null;
    let finished = false;
    void api
      .onSignInProgress((stage) =>
        setPhase((p) => (p.kind === "working" ? { kind: "working", stage } : p)),
      )
      .then((u) => {
        if (finished) u();
        else stop = u;
      });
    cancelRef.current = () => void api.cancelSignIn().catch(() => undefined);
    api.signIn(account?.id ?? null).then(
      (snapshot) => {
        finished = true;
        stop?.();
        cancelRef.current = null;
        playSound("primary");
        onSignedIn(snapshot);
      },
      (e: unknown) => {
        finished = true;
        stop?.();
        cancelRef.current = null;
        const code = e instanceof BackendError ? e.code : null;
        if (code === "cancelled") return onClose(); // the person chose to stop: nothing to explain
        setPhase({
          kind: "error",
          code,
          message: describeError("signIn", e),
        });
      },
    );
  };

  // Decide the first step once: create/unlock the vault, or go straight to the browser.
  useEffect(() => {
    let stale = false;
    api.vaultStatus().then(
      (s) => {
        if (stale) return;
        if (!s.exists) setPhase({ kind: "create" });
        else if (!s.unlocked) setPhase({ kind: "unlock" });
        else start();
      },
      (e: unknown) =>
        !stale &&
        setPhase({
          kind: "error",
          code: null,
          message: describeError("signIn", e),
        }),
    );
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once per opening
  }, []);

  const submitPassphrase = (e: SubmitEvent) => {
    e.preventDefault();
    if (busy) return;
    const creating = phase.kind === "create";
    if (creating && (pass.length < MIN_PASSPHRASE || pass !== confirm)) return;
    setBusy(true);
    setFieldError(null);
    (creating ? api.createVault(pass) : api.unlockVault(pass)).then(
      () => {
        setPass("");
        setConfirm("");
        setBusy(false);
        start();
      },
      (err: unknown) => {
        setBusy(false);
        setPass("");
        setConfirm("");
        setFieldError(describeError("vault", err));
      },
    );
  };

  const title = account ? t("signin.againTitle") : t("signin.addTitle");
  const who = account
    ? t("signin.againDescription", { name: account.username })
    : t("signin.addDescription");

  if (phase.kind === "create" || phase.kind === "unlock") {
    const creating = phase.kind === "create";
    const mismatch = creating && confirm.length > 0 && confirm !== pass;
    const form = (recoveryLink: ReactNode) => (
      <form onSubmit={submitPassphrase} className="flex flex-col gap-4" noValidate>
        <ModalHeader
          title={creating ? t("signin.createTitle") : t("signin.unlockTitle")}
          description={creating ? t("signin.createDescription") : t("signin.unlockDescription")}
        />
        <div className="flex flex-col gap-1.5">
          <label htmlFor="vault-pass" className={LABEL}>
            {t("common.passphrase")}
          </label>
          <input
            id="vault-pass"
            autoFocus
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={pass}
            onChange={(e) => setPass(e.target.value)}
            className={FIELD}
            aria-describedby="vault-pass-note"
          />
          {creating && (
            <span id="vault-pass-note" className="text-ink-muted text-[0.8125rem]">
              {t("signin.passphraseNote", { count: MIN_PASSPHRASE })}
            </span>
          )}
        </div>
        {creating && (
          <div className="flex flex-col gap-1.5">
            <label htmlFor="vault-confirm" className={LABEL}>
              {t("signin.repeat")}
            </label>
            <input
              id="vault-confirm"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              className={FIELD}
            />
            {mismatch && (
              <span className="text-clay-deep text-[0.8125rem]">{t("signin.mismatch")}</span>
            )}
          </div>
        )}
        {fieldError && (
          <p role="alert" className="text-clay-deep text-[0.875rem]">
            {fieldError}
          </p>
        )}
        {recoveryLink}
        <ModalFooter>
          <button
            type="button"
            className={BUTTON_QUIET}
            onClick={() => {
              playSound("click");
              onClose();
            }}
          >
            {t("common.cancel")}
          </button>
          <button
            type="submit"
            disabled={
              busy ||
              pass.length === 0 ||
              (creating && (pass.length < MIN_PASSPHRASE || pass !== confirm))
            }
            className={BUTTON_PRIMARY}
          >
            {busy ? t("common.working") : creating ? t("common.continue") : t("common.unlock")}
          </button>
        </ModalFooter>
      </form>
    );
    if (creating) return form(null);
    return (
      <VaultRecovery
        api={api}
        layout="modal"
        onAsking={onAsking}
        onDone={() => {
          // The old store is archived and the accounts must sign in again: the next step is a new passphrase.
          setPass("");
          setConfirm("");
          setFieldError(null);
          setPhase({ kind: "create" });
        }}
      >
        {form}
      </VaultRecovery>
    );
  }

  if (phase.kind === "error") {
    const approval = phase.code === "approval_required";
    return (
      <>
        <ModalHeader title={title} />
        <p role="alert" className="text-graphite mb-2 text-[0.9375rem]">
          {phase.message}
        </p>
        {approval && (
          <p className="text-ink-muted mb-5 text-[0.8125rem]">{t("signin.approvalNote")}</p>
        )}
        {!approval && <div className="mb-3" />}
        <ModalFooter>
          <button
            type="button"
            className={BUTTON_QUIET}
            onClick={() => {
              playSound("click");
              onClose();
            }}
          >
            {t("common.close")}
          </button>
          {!approval && (
            <button
              type="button"
              className={BUTTON_SOFT}
              onClick={() => {
                playSound("click");
                start();
              }}
            >
              {t("common.tryAgain")}
            </button>
          )}
        </ModalFooter>
      </>
    );
  }

  return (
    <>
      <ModalHeader title={title} description={who} />
      <p aria-live="polite" className="text-graphite mb-6 min-h-6 text-[0.9375rem]">
        {phase.kind === "working" ? t(STAGE_TEXT[phase.stage]) : ""}
      </p>
      <ModalFooter>
        <button
          type="button"
          className={BUTTON_QUIET}
          onClick={() => {
            playSound("click");
            cancelRef.current?.();
            if (phase.kind !== "working") onClose();
          }}
        >
          {t("common.cancel")}
        </button>
      </ModalFooter>
    </>
  );
}
