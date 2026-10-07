import { useState, type ReactNode, type SubmitEvent } from "react";
import type { AccountsApi } from "@/lib/accounts/api";
import { hasMessage, tDynamic, useT, type MessageKey } from "@/lib/i18n";
import { describeCode, describeError } from "@/lib/i18n/errors";
import type { LaunchExit } from "@/lib/launch/api";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { VaultRecovery } from "./VaultRecovery";
import { BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT, FIELD, LABEL } from "./modalStyles";

/** What went wrong, from a launch that was refused or a game that ended badly. */
export type Problem =
  | { kind: "refused"; profileId: string; code: string; accountId: string | null }
  | { kind: "ended"; profileId: string; exit: LaunchExit };

interface Props {
  problem: Problem | null;
  onClose: () => void;
  onRepair: (profileId: string) => void;
  onSignIn: (accountId: string | null) => void;
  onRetry: (profileId: string) => void;
  onOpenLogs: (profileId: string) => void;
  /** "Start fresh" archived the old vault: every account has to sign in again now. */
  onVaultReset: () => void;
  accountsClient: AccountsApi;
}

/**
 * One small surface for everything that can stop a game: the sign-in state, damaged files, a
 * locked account store, or Minecraft itself ending badly. It says what happened in a sentence and
 * offers the one fitting next step; the full detail stays in the local session log.
 */
export function LaunchProblemDialog(p: Props) {
  // Keep the last problem while the closing animation runs.
  const [shown, setShown] = useState(p.problem);
  if (p.problem && p.problem !== shown) setShown(p.problem);
  const problem = p.problem ?? shown;
  // "Start fresh?" is showing: Escape and the backdrop leave it alone.
  const [asking, setAsking] = useState(false);
  return (
    <Modal
      open={p.problem !== null}
      onOpenChange={(o) => !o && p.onClose()}
      size="sm"
      closeOnBackdrop={false}
      locked={asking}
    >
      {problem && <Body key={problemKey(problem)} {...p} problem={problem} onAsking={setAsking} />}
    </Modal>
  );
}

/** Why the game ended, in words: the backend names the reason, the wording is ours. */
function exitText(exit: LaunchExit): string {
  const key = `exit.${exit.reason}`;
  return tDynamic(hasMessage(key) ? key : "exit.generic", { code: exit.code ?? "" });
}

const TITLES: Readonly<Record<string, MessageKey>> = {
  no_account: "problem.title.no_account",
  needs_sign_in: "problem.title.needs_sign_in",
  needs_repair: "problem.title.needs_repair",
  approval_pending: "problem.title.approval_pending",
  offline: "problem.title.offline",
};

const problemKey = (x: Problem) =>
  x.kind === "refused" ? `r-${x.code}-${x.profileId}` : `e-${x.profileId}`;

function Body({
  problem,
  onClose,
  onRepair,
  onSignIn,
  onRetry,
  onOpenLogs,
  onVaultReset,
  onAsking,
  accountsClient,
}: Props & { problem: Problem; onAsking: (asking: boolean) => void }) {
  const t = useT();
  const [pass, setPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const quiet = (label: string, fn: () => void) => (
    <button
      type="button"
      className={BUTTON_QUIET}
      onClick={() => {
        playSound("click");
        fn();
      }}
    >
      {label}
    </button>
  );
  const main = (label: string, fn: () => void) => (
    <button
      type="button"
      autoFocus
      className={BUTTON_PRIMARY}
      onClick={() => {
        playSound("click");
        fn();
      }}
    >
      {label}
    </button>
  );

  if (problem.kind === "ended") {
    return (
      <>
        <ModalHeader title={t("problem.endedTitle")} />
        <p role="alert" className="text-graphite mb-5 text-[0.9375rem]">
          {exitText(problem.exit)}
        </p>
        <ModalFooter>
          <button
            type="button"
            className={BUTTON_SOFT}
            onClick={() => onOpenLogs(problem.profileId)}
          >
            {t("common.openLogs")}
          </button>
          {main(t("common.close"), onClose)}
        </ModalFooter>
      </>
    );
  }

  const { code, profileId, accountId } = problem;

  if (code === "vault_locked") {
    const submit = (e: SubmitEvent) => {
      e.preventDefault();
      if (busy || pass.length === 0) return;
      setBusy(true);
      setError(null);
      accountsClient.unlockVault(pass).then(
        () => {
          setPass("");
          onRetry(profileId); // unlocked: play again straight away
        },
        (err: unknown) => {
          setBusy(false);
          setPass("");
          setError(describeError("vault", err));
        },
      );
    };
    const form = (recoveryLink: ReactNode) => (
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <ModalHeader
          title={t("problem.unlockTitle")}
          description={t("problem.unlockDescription")}
        />
        <div className="flex flex-col gap-1.5">
          <label htmlFor="unlock-pass" className={LABEL}>
            {t("common.passphrase")}
          </label>
          <input
            id="unlock-pass"
            autoFocus
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={pass}
            onChange={(e) => setPass(e.target.value)}
            className={FIELD}
          />
        </div>
        {error && (
          <p role="alert" className="text-clay-deep text-[0.875rem]">
            {error}
          </p>
        )}
        {recoveryLink}
        <ModalFooter>
          {quiet(t("common.cancel"), onClose)}
          <button type="submit" disabled={busy || pass.length === 0} className={BUTTON_PRIMARY}>
            {busy ? t("common.working") : t("common.unlock")}
          </button>
        </ModalFooter>
      </form>
    );
    return (
      <VaultRecovery
        api={accountsClient}
        layout="modal"
        onAsking={onAsking}
        onDone={() => {
          setPass("");
          setError(null);
          onVaultReset(); // the accounts must sign in again: the dialog says so next
        }}
      >
        {form}
      </VaultRecovery>
    );
  }

  const action =
    code === "needs_repair"
      ? main(t("problem.repair"), () => onRepair(profileId))
      : code === "no_account"
        ? main(t("problem.addAccount"), () => onSignIn(null))
        : code === "needs_sign_in"
          ? main(t("common.signInAgain"), () => onSignIn(accountId))
          : code === "offline"
            ? main(t("common.tryAgain"), () => onRetry(profileId))
            : main(t("common.close"), onClose);
  const showClose =
    code === "needs_repair" ||
    code === "no_account" ||
    code === "needs_sign_in" ||
    code === "offline";
  return (
    <>
      <ModalHeader title={t(TITLES[code] ?? "problem.title.other")} />
      <p role="alert" className="text-graphite mb-5 text-[0.9375rem]">
        {describeCode("launch", code)}
      </p>
      <ModalFooter>
        {showClose && quiet(t("common.close"), onClose)}
        {action}
      </ModalFooter>
    </>
  );
}
