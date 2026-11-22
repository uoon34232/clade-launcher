import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { Account, AccountsApi, AccountsSnapshot } from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { playSound } from "@/lib/sound/soundService";
import { AccountAppearance } from "./AccountAppearance";
import { Modal, ModalFooter, ModalHeader, ModalSection } from "./Modal";
import { BUTTON_DANGER, BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT } from "./modalStyles";
import { PlayerHead } from "./PlayerHead";

interface Props {
  /** The account being managed; `null` keeps the dialog closed. */
  account: Account | null;
  capabilities: AccountsSnapshot["capabilities"];
  onClose: () => void;
  onSignInAgain: (id: string) => void;
  onRemoved: (snapshot: AccountsSnapshot) => void;
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
  api: AccountsApi;
}

/**
 * Everything about one Minecraft Java account, and nothing else: profile, skin, cape, removal.
 * Skin and cape changes are made on the account itself through Mojang's authenticated profile
 * service (see `profile_service.rs` and `AccountAppearance`), so they show up in the game and the
 * official launcher too.
 */
export function AccountDialog({
  account,
  capabilities,
  onClose,
  onRemoved,
  onSignInAgain,
  onVaultReset,
  api,
}: Props) {
  // Keep the last account while the closing animation runs.
  const [shown, setShown] = useState(account);
  const open = account !== null;
  const [wasOpen, setWasOpen] = useState(open);
  const [session, setSession] = useState(0);
  const doneRef = useRef<HTMLButtonElement>(null);
  // Lives here, not in the body, so the modal can ignore Escape and the backdrop while the removal
  // is being confirmed: only Keep or Remove answer it. A closed dialog or another account starts over.
  const [confirming, setConfirming] = useState(false);
  // The same for the "restore the default skin?" question inside the appearance section.
  const [confirmingReset, setConfirmingReset] = useState(false);
  // ...and the "Start fresh?" question of a forgotten passphrase.
  const [recovering, setRecovering] = useState(false);

  const accountChanged = account !== null && account.id !== shown?.id;
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setSession((n) => n + 1);
      setConfirming(false);
      setConfirmingReset(false);
      setRecovering(false);
    }
  } else if (accountChanged) {
    setConfirming(false);
    setConfirmingReset(false);
    setRecovering(false);
  }
  if (account && account !== shown) setShown(account);
  const current = account ?? shown;

  return (
    <Modal
      open={open}
      onOpenChange={(o) => !o && onClose()}
      size="md"
      initialFocus={doneRef}
      locked={open && (confirming || confirmingReset || recovering)}
    >
      {current && (
        <Body
          key={`${current.id}:${session}`}
          account={current}
          open={open}
          confirming={confirming}
          setConfirming={setConfirming}
          confirmingReset={confirmingReset}
          setConfirmingReset={setConfirmingReset}
          recovering={recovering}
          setRecovering={setRecovering}
          capabilities={capabilities}
          onClose={onClose}
          onRemoved={onRemoved}
          onSignInAgain={onSignInAgain}
          onVaultReset={onVaultReset}
          api={api}
          doneRef={doneRef}
        />
      )}
    </Modal>
  );
}

function Body({
  account,
  open,
  confirming,
  setConfirming,
  confirmingReset,
  setConfirmingReset,
  recovering,
  setRecovering,
  capabilities,
  onClose,
  onSignInAgain,
  onRemoved,
  onVaultReset,
  api,
  doneRef,
}: Omit<Props, "account"> & {
  account: Account;
  open: boolean;
  confirming: boolean;
  setConfirming: (confirming: boolean) => void;
  confirmingReset: boolean;
  setConfirmingReset: (confirming: boolean) => void;
  recovering: boolean;
  setRecovering: (recovering: boolean) => void;
  doneRef: RefObject<HTMLButtonElement | null>;
}) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const removeTriggerRef = useRef<HTMLButtonElement>(null);
  const signInAgainRef = useRef<HTMLButtonElement>(null);
  const keepBtnRef = useRef<HTMLButtonElement>(null);
  const restoreRemoveFocus = useRef(false);
  const pendingVaultFocus = useRef<"reset" | "unlock" | null>(null);
  const [unlockFocusSeq, setUnlockFocusSeq] = useState(0);
  const mountedRef = useRef(true);
  const openRef = useRef(open);
  const inFlightRef = useRef(false);
  const inlineConfirming = confirmingReset || recovering;

  useLayoutEffect(() => {
    openRef.current = open;
    if (!open) {
      pendingVaultFocus.current = null;
    }
  }, [open]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!confirming && restoreRemoveFocus.current) {
      restoreRemoveFocus.current = false;
      removeTriggerRef.current?.focus();
    }
  }, [confirming]);

  useEffect(() => {
    if (!open || !pendingVaultFocus.current) return;
    const mode = pendingVaultFocus.current;
    if (mode === "reset" && account.needsSignIn) {
      pendingVaultFocus.current = null;
      (signInAgainRef.current ?? doneRef.current)?.focus();
    } else if (mode === "unlock" && !inlineConfirming) {
      pendingVaultFocus.current = null;
      doneRef.current?.focus();
    }
  }, [open, account.needsSignIn, inlineConfirming, unlockFocusSeq, doneRef]);

  const remove = () => {
    if (busy || inFlightRef.current) return;
    inFlightRef.current = true;
    playSound("click");
    setBusy(true);
    setError(null);
    api.remove(account.id).then(
      (snapshot) => {
        inFlightRef.current = false;
        if (!mountedRef.current || !openRef.current) return;
        setBusy(false);
        playSound("primary");
        onRemoved(snapshot);
      },
      (e: unknown) => {
        inFlightRef.current = false;
        if (!mountedRef.current || !openRef.current) return;
        setBusy(false);
        setError(describeError("account", e));
        window.queueMicrotask(() => {
          const activeEl = document.activeElement;
          if (!activeEl || activeEl === document.body || activeEl === document.documentElement) {
            keepBtnRef.current?.focus();
          }
        });
      },
    );
  };

  return (
    <>
      <ModalHeader title={account.username} description={t("accountModal.description")} />

      <ModalSection title={t("accountModal.profile")}>
        <div className="flex items-center gap-4">
          <PlayerHead
            account={account}
            className="ring-graphite/10 size-14 shrink-0 rounded-[0.75rem] ring-1"
          />
          <dl className="grid min-w-0 flex-1 gap-1 text-[0.875rem]">
            <div className="flex min-w-0 gap-2">
              <dt className="text-ink-muted w-14 shrink-0">{t("accountModal.name")}</dt>
              <dd className="text-graphite truncate">{account.username}</dd>
            </div>
            {account.uuid && (
              <div className="flex min-w-0 gap-2">
                <dt className="text-ink-muted w-14 shrink-0">{t("accountModal.uuid")}</dt>
                <dd className="text-graphite truncate select-text">{account.uuid}</dd>
              </div>
            )}
            <div className="flex min-w-0 gap-2">
              <dt className="text-ink-muted w-14 shrink-0">{t("accountModal.status")}</dt>
              <dd className="text-graphite">
                {account.needsSignIn
                  ? t("accountModal.statusNeedsSignIn")
                  : t("accountModal.statusSignedIn")}
              </dd>
            </div>
          </dl>
        </div>
      </ModalSection>

      <AccountAppearance
        account={account}
        skin={capabilities.skin}
        cape={capabilities.cape}
        api={api}
        disabled={confirming || busy}
        confirmingReset={confirmingReset}
        setConfirmingReset={setConfirmingReset}
        setRecovering={setRecovering}
        onUnlockFocus={() => {
          pendingVaultFocus.current = "unlock";
          setUnlockFocusSeq((n) => n + 1);
        }}
        onVaultReset={(snap) => {
          pendingVaultFocus.current = "reset";
          onVaultReset?.(snap);
        }}
      />

      <ModalFooter>
        {confirming ? (
          <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-x-2.5 gap-y-2">
            <p className="text-graphite mr-auto min-w-[12rem] flex-1 text-[0.8125rem]">
              {t("accountModal.confirmRemove", { name: account.username })}
            </p>
            <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
              <button
                ref={keepBtnRef}
                type="button"
                autoFocus
                disabled={busy}
                className={BUTTON_QUIET}
                onClick={() => {
                  playSound("click");
                  restoreRemoveFocus.current = true;
                  setError(null);
                  setConfirming(false);
                }}
              >
                {t("accountModal.keep")}
              </button>
              <button type="button" disabled={busy} className={BUTTON_DANGER} onClick={remove}>
                {busy ? t("accountModal.removing") : t("accountModal.remove")}
              </button>
            </div>
          </div>
        ) : (
          <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-x-2.5 gap-y-2">
            <button
              ref={removeTriggerRef}
              type="button"
              disabled={inlineConfirming}
              className={`${BUTTON_DANGER} mr-auto`}
              onClick={() => {
                playSound("click");
                setError(null);
                setConfirming(true);
              }}
            >
              {t("accountModal.signOutRemove")}
            </button>
            {account.needsSignIn && (
              <button
                ref={signInAgainRef}
                type="button"
                disabled={inlineConfirming}
                className={BUTTON_PRIMARY}
                onClick={() => {
                  playSound("click");
                  onSignInAgain(account.id);
                }}
              >
                {t("common.signInAgain")}
              </button>
            )}
            <button
              ref={doneRef}
              type="button"
              disabled={inlineConfirming}
              className={BUTTON_SOFT}
              onClick={() => {
                playSound("click");
                onClose();
              }}
            >
              {t("common.done")}
            </button>
          </div>
        )}
      </ModalFooter>
      {error && (
        <p role="alert" className="text-clay-deep mt-3 text-right text-[0.875rem]">
          {error}
        </p>
      )}
    </>
  );
}
