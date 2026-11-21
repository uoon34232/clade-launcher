import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import type { AccountsApi, AccountsSnapshot } from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError, errorCode } from "@/lib/i18n/errors";
import { playSound } from "@/lib/sound/soundService";
import { cn } from "@/lib/utils";
import { ModalFooter, ModalHeader } from "./Modal";
import {
  BUTTON_DANGER,
  BUTTON_QUIET,
  BUTTON_SMALL_DANGER,
  BUTTON_SMALL_QUIET,
} from "./modalStyles";

interface Props {
  api: AccountsApi;
  /**
   * The question is showing. The dialog around it holds this, so that Escape and the backdrop leave
   * it alone (like every other destructive confirmation): only its two buttons answer.
   */
  onAsking: (asking: boolean) => void;
  /** The old encrypted sign-in store was archived. Receives the updated accounts snapshot. */
  onReset: (snapshot: AccountsSnapshot) => void;
  /** `modal`: the question is the whole dialog (title and footer). `inline`: a block inside one. */
  layout: "modal" | "inline";
  /** The normal passphrase form; it receives the quiet "Forgot passphrase?" link to place under its field. */
  children: (link: ReactNode) => ReactNode;
}

/**
 * "Forgot passphrase?" and the "Start fresh?" question behind it, shared by every place that asks
 * for the vault passphrase. Starting fresh archives the old encrypted vault inside the Clade folder
 * and makes every account sign in again; profiles, worlds, playtime and settings are untouched, and
 * nothing is revoked at Microsoft. The backend does the work (`vault_recovery.rs`).
 */
export function VaultRecovery({ api, onAsking, onReset, layout, children }: Props) {
  const t = useT();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [archived, setArchived] = useState(false);
  const titleId = useId();
  const linkRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);

  // Never leave the dialog around it locked if this goes away while the question is open.
  useEffect(() => () => onAsking(false), [onAsking]);

  useEffect(() => {
    if (!asking && restoreFocus.current) {
      restoreFocus.current = false;
      linkRef.current?.focus();
    }
  }, [asking]);

  useEffect(() => {
    if (asking && archived && !busy) {
      confirmRef.current?.focus();
    }
  }, [asking, archived, busy]);

  const ask = (next: boolean) => {
    setAsking(next);
    setError(null);
    if (!next) setArchived(false);
    onAsking(next);
  };

  const cancel = () => {
    restoreFocus.current = true;
    ask(false);
  };

  const startFresh = () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    playSound("click");
    api.resetVault().then(
      (snap) => {
        setBusy(false);
        ask(false);
        onReset(snap);
      },
      (e: unknown) => {
        setBusy(false);
        if (errorCode(e) === "accounts_not_updated") setArchived(true);
        setError(describeError("vault", e)); // the question stays: retry, or keep trying the passphrase
      },
    );
  };

  const link = (
    <div>
      <button
        ref={linkRef}
        type="button"
        className={cn(BUTTON_SMALL_QUIET, "-ml-1 px-1")}
        onClick={() => {
          playSound("click");
          ask(true);
        }}
      >
        {t("recovery.link")}
      </button>
    </div>
  );

  if (!asking) return <>{children(link)}</>;

  const body = (
    <p className="text-graphite text-[0.9375rem] leading-relaxed">{t("recovery.body")}</p>
  );
  const alert = error && (
    <p role="alert" className="text-clay-deep text-[0.875rem]">
      {error}
    </p>
  );

  if (layout === "modal") {
    return (
      <div className="flex flex-col gap-4">
        <ModalHeader title={t("recovery.title")} />
        {body}
        {alert}
        <ModalFooter>
          {!archived && (
            <button
              type="button"
              autoFocus
              disabled={busy}
              className={BUTTON_QUIET}
              onClick={cancel}
            >
              {t("recovery.keep")}
            </button>
          )}
          <button
            ref={confirmRef}
            type="button"
            disabled={busy}
            className={BUTTON_DANGER}
            onClick={startFresh}
          >
            {busy ? t("common.working") : t("recovery.confirm")}
          </button>
        </ModalFooter>
      </div>
    );
  }

  return (
    <div role="group" aria-labelledby={titleId} className="flex flex-col gap-2.5">
      <h4 id={titleId} className="text-graphite text-[0.9375rem] font-medium">
        {t("recovery.title")}
      </h4>
      {body}
      {alert}
      <div className="flex flex-wrap items-center justify-end gap-3">
        {!archived && (
          <button
            type="button"
            autoFocus
            disabled={busy}
            className={BUTTON_SMALL_QUIET}
            onClick={cancel}
          >
            {t("recovery.keep")}
          </button>
        )}
        <button
          ref={confirmRef}
          type="button"
          disabled={busy}
          className={BUTTON_SMALL_DANGER}
          onClick={startFresh}
        >
          {busy ? t("common.working") : t("recovery.confirm")}
        </button>
      </div>
    </div>
  );
}
