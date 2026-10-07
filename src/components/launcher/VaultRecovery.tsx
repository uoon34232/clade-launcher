import { useEffect, useId, useState, type ReactNode } from "react";
import type { AccountsApi } from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { playSound } from "@/lib/sound/soundService";
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
  /** The old encrypted sign-in store was archived. The caller carries on from there. */
  onDone: () => void;
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
export function VaultRecovery({ api, onAsking, onDone, layout, children }: Props) {
  const t = useT();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();

  // Never leave the dialog around it locked if this goes away while the question is open.
  useEffect(() => () => onAsking(false), [onAsking]);

  const ask = (next: boolean) => {
    setAsking(next);
    setError(null);
    onAsking(next);
  };

  const startFresh = () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    playSound("click");
    api.resetVault().then(
      () => {
        setBusy(false);
        ask(false);
        onDone();
      },
      (e: unknown) => {
        setBusy(false);
        setError(describeError("vault", e)); // the question stays: retry, or keep trying the passphrase
      },
    );
  };

  const link = (
    <div>
      <button
        type="button"
        className={`${BUTTON_SMALL_QUIET} -ml-1 px-1`}
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
          <button
            type="button"
            autoFocus
            disabled={busy}
            className={BUTTON_QUIET}
            onClick={() => ask(false)}
          >
            {t("recovery.keep")}
          </button>
          <button type="button" disabled={busy} className={BUTTON_DANGER} onClick={startFresh}>
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
        <button
          type="button"
          autoFocus
          disabled={busy}
          className={BUTTON_SMALL_QUIET}
          onClick={() => ask(false)}
        >
          {t("recovery.keep")}
        </button>
        <button type="button" disabled={busy} className={BUTTON_SMALL_DANGER} onClick={startFresh}>
          {busy ? t("common.working") : t("recovery.confirm")}
        </button>
      </div>
    </div>
  );
}
