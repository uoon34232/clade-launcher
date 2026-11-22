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
import { describeError, errorCode } from "@/lib/i18n/errors";
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
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
  api: AccountsApi;
}

type Phase =
  | { kind: "checking" }
  | { kind: "create" }
  | { kind: "unlock" }
  | { kind: "working"; stage: SignInStage }
  | { kind: "error"; code: string | null; message: string; recheckVault?: boolean };

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
export function SignInDialog({
  open,
  onOpenChange,
  account,
  onSignedIn,
  onVaultReset,
  api,
}: Props) {
  // While a sign-in runs, closing the modal (Escape, backdrop) cancels it.
  const cancelRef = useRef<(() => void) | null>(null);
  const dismissedRef = useRef(!open);
  // "Start fresh?" is showing, or vault KDF is running: Escape and backdrop leave it alone.
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [wasOpen, setWasOpen] = useState(open);
  const [session, setSession] = useState(0);

  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setSession((n) => n + 1);
      setAsking(false);
      setBusy(false);
    }
  }

  useEffect(() => {
    const cancel = () => {
      dismissedRef.current = true;
      const fn = cancelRef.current;
      cancelRef.current = null;
      fn?.();
    };
    if (open) {
      dismissedRef.current = false;
      return cancel;
    }
    cancel();
    return undefined;
  }, [open]);

  const dismiss = () => {
    dismissedRef.current = true;
    cancelRef.current?.();
    onOpenChange(false);
  };

  return (
    <Modal
      open={open}
      onOpenChange={(o) => {
        if (!o) dismiss();
        else onOpenChange(true);
      }}
      size="sm"
      closeOnBackdrop={false}
      locked={asking || busy}
    >
      <Flow
        key={session}
        account={account}
        api={api}
        busy={busy}
        setBusy={setBusy}
        cancelRef={cancelRef}
        dismissedRef={dismissedRef}
        onAsking={setAsking}
        onClose={dismiss}
        onSignedIn={onSignedIn}
        onVaultReset={onVaultReset}
      />
    </Modal>
  );
}

function Flow({
  account,
  api,
  busy,
  setBusy,
  cancelRef,
  dismissedRef,
  onAsking,
  onClose,
  onSignedIn,
  onVaultReset,
}: Pick<Props, "account" | "api" | "onSignedIn" | "onVaultReset"> & {
  busy: boolean;
  setBusy: (busy: boolean) => void;
  cancelRef: RefObject<(() => void) | null>;
  dismissedRef: RefObject<boolean>;
  onAsking: (asking: boolean) => void;
  onClose: () => void;
}) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: "checking" });
  const [checkAttempt, setCheckAttempt] = useState(0);
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [prevKind, setPrevKind] = useState(phase.kind);
  if (phase.kind !== prevKind) {
    setPrevKind(phase.kind);
    if (phase.kind !== "working") setCancelling(false);
  }
  const mountedRef = useRef(true);
  const stopProgressRef = useRef<(() => void) | null>(null);
  const attemptSeqRef = useRef(0);
  const vaultOpSeqRef = useRef(0);
  const errorPrimaryRef = useRef<HTMLButtonElement>(null);
  const passInputRef = useRef<HTMLInputElement>(null);
  const pendingVaultFocusRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    const attemptSeq = attemptSeqRef;
    const vaultOpSeq = vaultOpSeqRef;
    return () => {
      mountedRef.current = false;
      attemptSeq.current += 1;
      vaultOpSeq.current += 1;
      cancelRef.current?.();
      stopProgressRef.current?.();
      stopProgressRef.current = null;
    };
  }, [cancelRef]);

  const errorApproval = phase.kind === "error" && phase.code === "approval_required";
  useEffect(() => {
    if (phase.kind === "error" && !dismissedRef.current) {
      errorPrimaryRef.current?.focus();
    }
  }, [phase.kind, errorApproval, dismissedRef]);

  useEffect(() => {
    if (!busy && fieldError && pendingVaultFocusRef.current) {
      pendingVaultFocusRef.current = false;
      passInputRef.current?.focus();
    }
  }, [busy, fieldError]);

  const start = async () => {
    if (!mountedRef.current || dismissedRef.current) return;
    const attempt = ++attemptSeqRef.current;
    const isStale = () =>
      !mountedRef.current || dismissedRef.current || attempt !== attemptSeqRef.current;
    let cancelled = false;
    let finished = false;
    let signInStarted = false;
    let stopProgress: (() => void) | null = null;
    setPhase({ kind: "working", stage: "opening" });
    stopProgressRef.current?.();
    stopProgressRef.current = null;
    const clearProgress = () => {
      const unsub = stopProgress;
      stopProgress = null;
      if (unsub) {
        if (stopProgressRef.current === unsub) stopProgressRef.current = null;
        unsub();
      }
    };
    const myCancel = () => {
      cancelled = true;
      finished = true;
      clearProgress();
      if (cancelRef.current === myCancel) cancelRef.current = null;
      if (signInStarted) {
        void api.cancelSignIn().catch(() => undefined);
      }
    };
    cancelRef.current = myCancel;
    try {
      const unsubscribe = await api.onSignInProgress((stage) => {
        if (finished || cancelled || isStale()) return;
        setPhase((p) => (p.kind === "working" ? { kind: "working", stage } : p));
      });
      if (cancelled || finished || isStale()) {
        unsubscribe();
        if (cancelled && !isStale()) onClose();
        return;
      }
      stopProgress = unsubscribe;
      stopProgressRef.current = unsubscribe;
      signInStarted = true;
      const snapshot = await api.signIn(account?.id ?? null);
      if (cancelled || isStale()) {
        if (cancelled && !isStale()) onClose();
        return;
      }
      playSound("primary");
      onSignedIn(snapshot);
    } catch (e: unknown) {
      if (!mountedRef.current || attempt !== attemptSeqRef.current) return;
      const code = errorCode(e);
      if (code === "cancelled" || cancelled) {
        if (!dismissedRef.current) onClose();
        return;
      }
      if (dismissedRef.current) return;
      setPhase({
        kind: "error",
        code,
        message: describeError("signIn", e),
        recheckVault: code === null || code === "vault_locked",
      });
    } finally {
      if (attempt === attemptSeqRef.current) {
        finished = true;
        clearProgress();
        if (cancelRef.current === myCancel) cancelRef.current = null;
      }
    }
  };

  // Decide the first step: create/unlock the vault, or go straight to the browser.
  useEffect(() => {
    let stale = false;
    api.vaultStatus().then(
      (s) => {
        if (stale || !mountedRef.current || dismissedRef.current) return;
        if (!s.exists) setPhase({ kind: "create" });
        else if (!s.unlocked) setPhase({ kind: "unlock" });
        else void start();
      },
      (e: unknown) => {
        if (stale || !mountedRef.current || dismissedRef.current) return;
        setPhase({
          kind: "error",
          code: errorCode(e),
          message: describeError("signIn", e),
          recheckVault: true,
        });
      },
    );
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once per opening or explicit vault retry
  }, [checkAttempt]);

  const submitPassphrase = (e: SubmitEvent) => {
    e.preventDefault();
    if (busy) return;
    const creating = phase.kind === "create";
    if (!creating && phase.kind !== "unlock") return;
    if (creating && (pass.length < MIN_PASSPHRASE || pass !== confirm)) return;
    const seq = ++vaultOpSeqRef.current;
    setBusy(true);
    setFieldError(null);
    (creating ? api.createVault(pass) : api.unlockVault(pass)).then(
      () => {
        if (!mountedRef.current || vaultOpSeqRef.current !== seq) return;
        setPass("");
        setConfirm("");
        setBusy(false);
        if (dismissedRef.current) return;
        void start();
      },
      (err: unknown) => {
        if (!mountedRef.current || vaultOpSeqRef.current !== seq) return;
        pendingVaultFocusRef.current = true;
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
            ref={passInputRef}
            id="vault-pass"
            autoFocus
            type="password"
            autoComplete="off"
            spellCheck={false}
            disabled={busy}
            value={pass}
            onChange={(e) => setPass(e.target.value)}
            className={FIELD}
            aria-invalid={fieldError ? true : undefined}
            aria-describedby={
              [creating ? "vault-pass-note" : null, fieldError ? "vault-pass-error" : null]
                .filter(Boolean)
                .join(" ") || undefined
            }
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
              disabled={busy}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              aria-invalid={mismatch || undefined}
              aria-describedby={mismatch ? "vault-confirm-error" : undefined}
              className={FIELD}
            />
            {mismatch && (
              <span id="vault-confirm-error" className="text-clay-deep text-[0.8125rem]">
                {t("signin.mismatch")}
              </span>
            )}
          </div>
        )}
        {fieldError && (
          <p id="vault-pass-error" role="alert" className="text-clay-deep text-[0.875rem]">
            {fieldError}
          </p>
        )}
        {recoveryLink}
        <ModalFooter>
          <button
            type="button"
            disabled={busy}
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
        disabled={busy}
        onAsking={onAsking}
        onReset={(snap) => {
          // The old store is archived and the accounts must sign in again: the next step is a new passphrase.
          vaultOpSeqRef.current += 1;
          setBusy(false);
          setPass("");
          setConfirm("");
          setFieldError(null);
          onVaultReset?.(snap);
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
            ref={approval ? errorPrimaryRef : undefined}
            type="button"
            autoFocus={approval}
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
              ref={errorPrimaryRef}
              type="button"
              autoFocus
              className={BUTTON_SOFT}
              onClick={() => {
                playSound("click");
                if (phase.recheckVault || phase.code === null || phase.code === "vault_locked") {
                  setPhase({ kind: "checking" });
                  setCheckAttempt((n) => n + 1);
                } else {
                  void start();
                }
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
          disabled={phase.kind === "working" && cancelling}
          aria-busy={(phase.kind === "working" && cancelling) || undefined}
          className={BUTTON_QUIET}
          onClick={() => {
            if (phase.kind === "working" && cancelling) return;
            if (phase.kind === "working") setCancelling(true);
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
