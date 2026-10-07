import { useRef, useState, type RefObject } from "react";
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
  api,
}: Props) {
  // Keep the last account while the closing animation runs.
  const [shown, setShown] = useState(account);
  if (account && account !== shown) setShown(account);
  const current = account ?? shown;
  const doneRef = useRef<HTMLButtonElement>(null);
  // Lives here, not in the body, so the modal can ignore Escape and the backdrop while the removal
  // is being confirmed: only Keep or Remove answer it. A closed dialog or another account starts over.
  const [confirming, setConfirming] = useState(false);
  // The same for the "restore the default skin?" question inside the appearance section.
  const [confirmingReset, setConfirmingReset] = useState(false);
  // ...and the "Start fresh?" question of a forgotten passphrase.
  const [recovering, setRecovering] = useState(false);
  if (
    (confirming || confirmingReset || recovering) &&
    (account === null || account.id !== shown?.id)
  ) {
    setConfirming(false);
    setConfirmingReset(false);
    setRecovering(false);
  }

  return (
    <Modal
      open={account !== null}
      onOpenChange={(o) => !o && onClose()}
      size="md"
      initialFocus={doneRef}
      locked={confirming || confirmingReset || recovering}
    >
      {current && (
        <Body
          key={current.id}
          account={current}
          confirming={confirming}
          setConfirming={setConfirming}
          confirmingReset={confirmingReset}
          setConfirmingReset={setConfirmingReset}
          setRecovering={setRecovering}
          capabilities={capabilities}
          onClose={onClose}
          onRemoved={onRemoved}
          onSignInAgain={onSignInAgain}
          api={api}
          doneRef={doneRef}
        />
      )}
    </Modal>
  );
}

function Body({
  account,
  confirming,
  setConfirming,
  confirmingReset,
  setConfirmingReset,
  setRecovering,
  capabilities,
  onClose,
  onSignInAgain,
  onRemoved,
  api,
  doneRef,
}: Omit<Props, "account"> & {
  account: Account;
  confirming: boolean;
  setConfirming: (confirming: boolean) => void;
  confirmingReset: boolean;
  setConfirmingReset: (confirming: boolean) => void;
  setRecovering: (recovering: boolean) => void;
  doneRef: RefObject<HTMLButtonElement | null>;
}) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const remove = () => {
    setBusy(true);
    setError(null);
    api.remove(account.id).then(
      (snapshot) => {
        playSound("primary");
        onRemoved(snapshot);
      },
      (e: unknown) => {
        setBusy(false);
        setError(describeError("account", e));
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
        confirmingReset={confirmingReset}
        setConfirmingReset={setConfirmingReset}
        setRecovering={setRecovering}
      />

      <ModalFooter>
        {confirming ? (
          <>
            <p className="text-graphite mr-auto text-[0.8125rem]">
              {t("accountModal.confirmRemove", { name: account.username })}
            </p>
            <button
              type="button"
              autoFocus
              disabled={busy}
              className={BUTTON_QUIET}
              onClick={() => setConfirming(false)}
            >
              {t("accountModal.keep")}
            </button>
            <button type="button" disabled={busy} className={BUTTON_DANGER} onClick={remove}>
              {busy ? t("accountModal.removing") : t("accountModal.remove")}
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              disabled={confirmingReset}
              className={`${BUTTON_DANGER} mr-auto`}
              onClick={() => setConfirming(true)}
            >
              {t("accountModal.signOutRemove")}
            </button>
            {account.needsSignIn && (
              <button
                type="button"
                className={BUTTON_PRIMARY}
                onClick={() => onSignInAgain(account.id)}
              >
                {t("common.signInAgain")}
              </button>
            )}
            <button
              ref={doneRef}
              type="button"
              className={BUTTON_SOFT}
              onClick={() => {
                playSound("click");
                onClose();
              }}
            >
              {t("common.done")}
            </button>
          </>
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
