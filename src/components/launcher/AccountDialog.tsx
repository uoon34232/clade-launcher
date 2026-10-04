import { useRef, useState, type RefObject } from "react";
import type { Account, AccountsApi, AccountsSnapshot } from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader, ModalSection } from "./Modal";
import { BUTTON_DANGER, BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT } from "./modalStyles";
import { PlayerHead } from "./PlayerHead";
import { Segmented } from "./Segmented";

interface Props {
  /** The account being managed; `null` keeps the dialog closed. */
  account: Account | null;
  capabilities: AccountsSnapshot["capabilities"];
  onClose: () => void;
  onSignInAgain: (id: string) => void;
  onRemoved: (snapshot: AccountsSnapshot) => void;
  api: AccountsApi;
}

const SKIN_MODELS = ["classic", "slim"] as const;

/**
 * Everything about one Minecraft Java account, and nothing else: profile, skin, cape, removal.
 * Skin and cape changes are made on the account itself through Mojang's authenticated profile
 * service (see `profile_service.rs`), so they show up in the game and the official launcher too.
 * Until sign-in exists those controls are visible but off.
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

  return (
    <Modal
      open={account !== null}
      onOpenChange={(o) => !o && onClose()}
      size="md"
      initialFocus={doneRef}
    >
      {current && (
        <Body
          key={current.id}
          account={current}
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
  capabilities,
  onClose,
  onSignInAgain,
  onRemoved,
  api,
  doneRef,
}: Omit<Props, "account"> & { account: Account; doneRef: RefObject<HTMLButtonElement | null> }) {
  const t = useT();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canSkin = capabilities.skin;

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

      <ModalSection title={t("accountModal.skin")}>
        <div className="flex flex-wrap items-center gap-3">
          <Segmented
            value="classic"
            onChange={() => undefined}
            label={t("accountModal.skinModel")}
            options={SKIN_MODELS.map((m) => ({
              value: m,
              label: t(m === "classic" ? "accountModal.skinClassic" : "accountModal.skinSlim"),
              disabled: !canSkin,
            }))}
          />
          <button type="button" disabled={!canSkin} className={BUTTON_SOFT}>
            {t("accountModal.choosePng")}
          </button>
          <button type="button" disabled={!canSkin} className={BUTTON_QUIET}>
            {t("accountModal.restoreDefault")}
          </button>
        </div>
        {!canSkin && (
          <p className="text-ink-muted text-[0.8125rem]">{t("accountModal.skinUnavailable")}</p>
        )}
      </ModalSection>

      <ModalSection title={t("accountModal.cape")}>
        <p className="text-ink-muted text-[0.875rem]">
          {capabilities.cape ? t("accountModal.capeAvailable") : t("accountModal.capeUnavailable")}
        </p>
      </ModalSection>

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
