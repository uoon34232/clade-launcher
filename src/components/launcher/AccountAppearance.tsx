import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type SubmitEvent,
} from "react";
import {
  MAX_SKIN_BYTES,
  type AccountsApi,
  type AccountsSnapshot,
  type Appearance,
  type Cape,
  type SkinModel,
} from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError, errorCode } from "@/lib/i18n/errors";
import { playSound } from "@/lib/sound/soundService";
import { cn } from "@/lib/utils";
import { ModalSection } from "./Modal";
import { VaultRecovery } from "./VaultRecovery";
import { Segmented } from "./Segmented";
import {
  BUTTON_PRIMARY,
  BUTTON_SMALL_DANGER,
  BUTTON_SMALL_GHOST,
  BUTTON_SMALL_QUIET,
  BUTTON_SMALL_SOFT,
  FIELD,
  LABEL,
} from "./modalStyles";

type Load =
  { kind: "loading" } | { kind: "ready"; look: Appearance } | { kind: "failed"; error: unknown };

type Busy = "upload" | "reset" | "cape" | null;

const SKIN_MODELS = ["classic", "slim"] as const;

interface Props {
  account: { id: string; needsSignIn: boolean };
  skin: boolean;
  cape: boolean;
  api: AccountsApi;
  /**
   * The "restore the default skin?" question is showing. Held by the dialog so that Escape and the
   * backdrop leave it alone, like every other destructive confirmation: only its two buttons answer.
   */
  confirmingReset: boolean;
  setConfirmingReset: (confirming: boolean) => void;
  /** "Start fresh?" (forgotten passphrase) is showing: held by the dialog, like the question above. */
  setRecovering: (recovering: boolean) => void;
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
}

/**
 * The Skin and Cape sections of the account dialog. Everything here is a request to the Minecraft
 * account itself, made by the backend: the dialog only ever shows what the account's profile
 * reports, and keeps showing it (with a plain sentence) when a change is refused.
 */
export function AccountAppearance({
  account,
  skin,
  cape,
  api,
  confirmingReset,
  setConfirmingReset,
  setRecovering,
  onVaultReset,
}: Props) {
  const t = useT();
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [model, setModel] = useState<SkinModel>("classic");
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<unknown>(null);
  const inFlight = useRef(false);
  const read = useRef(false);
  const capeList = useRef<HTMLUListElement>(null);
  const placed = useRef(false);
  const alive = useRef(true);
  const fileInput = useRef<HTMLInputElement>(null);
  const restoreDefaultRef = useRef<HTMLButtonElement>(null);
  const restoreDefaultFocus = useRef(false);
  const needsSignIn = account.needsSignIn;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!confirmingReset && restoreDefaultFocus.current) {
      restoreDefaultFocus.current = false;
      restoreDefaultRef.current?.focus();
    }
  }, [confirmingReset]);

  const show = useCallback((look: Appearance) => {
    setError(null);
    setLoad({ kind: "ready", look });
    // The model of the account's skin, when it says; otherwise whatever was last chosen.
    if (look.skinModel) setModel(look.skinModel);
  }, []);

  const reload = useCallback(() => {
    setError(null);
    setLoad({ kind: "loading" });
    api.appearance(account.id).then(
      (look) => alive.current && show(look),
      (e: unknown) => alive.current && setLoad({ kind: "failed", error: e }),
    );
  }, [api, account.id, show]);

  useEffect(() => {
    // One read per opening, even where React runs the effect twice (development).
    if (needsSignIn || read.current) return;
    read.current = true;
    // reading the account's profile is a request; it has to start when the dialog opens
    reload();
  }, [needsSignIn, reload]);

  /** Runs one change. A second one cannot start while one is running. */
  const change = (kind: Exclude<Busy, null>, run: () => Promise<Appearance>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(kind);
    setError(null);
    playSound("click");
    run().then(
      (look) => {
        inFlight.current = false;
        if (!alive.current) return;
        setBusy(null);
        setConfirmingReset(false);
        show(look);
      },
      (e: unknown) => {
        inFlight.current = false;
        if (!alive.current) return;
        setBusy(null);
        setConfirmingReset(false);
        setError(e); // the current look stays exactly as it was
      },
    );
  };

  const onFile = (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ""; // the same file can be chosen again; the path is never kept
    if (!file) return;
    if (file.size > MAX_SKIN_BYTES) {
      setError({ code: "invalid_skin" });
      return;
    }
    change("upload", async () =>
      api.setSkin(account.id, model, new Uint8Array(await file.arrayBuffer())),
    );
  };

  const ready = load.kind === "ready";
  const look = ready ? load.look : null;
  // With many capes the equipped one may start below the fold of the list: bring it into view once,
  // inside the list only (the dialog itself does not move), and never again, so rows do not jump
  // under the pointer after a change.
  const capeCount = look?.capes.length ?? 0;
  useEffect(() => {
    const list = capeList.current;
    if (placed.current || !list || capeCount === 0) return;
    placed.current = true;
    const row = list.querySelector<HTMLElement>("[data-active]");
    if (row) list.scrollTop = row.offsetTop - (list.clientHeight - row.offsetHeight) / 2;
  }, [capeCount]);
  const idle = ready && busy === null && !confirmingReset;
  const active = look?.capes.some((c) => c.active) ?? false;

  const notice =
    needsSignIn || load.kind === "failed" || error ? (
      <Notice
        needsSignIn={needsSignIn}
        failure={load.kind === "failed" ? load.error : error}
        locked={!needsSignIn && load.kind === "failed" && errorCode(load.error) === "vault_locked"}
        api={api}
        onRetry={reload}
        onUnlocked={reload}
        onAsking={setRecovering}
        onVaultReset={onVaultReset}
      />
    ) : null;

  return (
    <>
      {skin && (
        <ModalSection title={t("accountModal.skin")} compact>
          <div className="flex flex-wrap items-center gap-3">
            <Segmented
              value={model}
              onChange={setModel}
              label={t("accountModal.skinModel")}
              options={SKIN_MODELS.map((m) => ({
                value: m,
                label: t(m === "classic" ? "accountModal.skinClassic" : "accountModal.skinSlim"),
                disabled: !idle,
              }))}
            />
            <input
              ref={fileInput}
              type="file"
              accept="image/png"
              hidden
              tabIndex={-1}
              aria-hidden="true"
              onChange={onFile}
            />
            <button
              type="button"
              disabled={!idle}
              className={BUTTON_SMALL_SOFT}
              onClick={() => fileInput.current?.click()}
            >
              {busy === "upload" ? t("accountModal.uploading") : t("accountModal.choosePng")}
            </button>
          </div>
          <div>
            <button
              ref={restoreDefaultRef}
              type="button"
              disabled={!idle}
              // The same two rows in every language: the model and the upload on the first, this on
              // its own below, its outline starting at the same left edge as the controls above.
              className={BUTTON_SMALL_GHOST}
              onClick={() => setConfirmingReset(true)}
            >
              {t("accountModal.restoreDefault")}
            </button>
          </div>
          {confirmingReset ? (
            <div className="flex flex-wrap items-center gap-3">
              <p className="text-graphite mr-auto min-w-[14rem] flex-1 text-[0.8125rem]">
                {t("accountModal.resetConfirm")}
              </p>
              <button
                type="button"
                autoFocus
                disabled={busy !== null}
                className={BUTTON_SMALL_QUIET}
                onClick={() => {
                  restoreDefaultFocus.current = true;
                  setConfirmingReset(false);
                }}
              >
                {t("accountModal.keepSkin")}
              </button>
              <button
                type="button"
                disabled={busy !== null}
                className={BUTTON_SMALL_DANGER}
                onClick={() => change("reset", () => api.resetSkin(account.id))}
              >
                {busy === "reset" ? t("accountModal.restoring") : t("accountModal.restoreDefault")}
              </button>
            </div>
          ) : (
            <p className="text-ink-muted text-[0.8125rem]">{t("accountModal.skinModelNote")}</p>
          )}
        </ModalSection>
      )}

      {cape && (
        <ModalSection title={t("accountModal.cape")} compact>
          {look === null ? (
            !needsSignIn &&
            load.kind === "loading" && (
              <p className="text-ink-muted text-[0.875rem]">{t("appearance.loading")}</p>
            )
          ) : look.capes.length === 0 ? (
            <p className="text-ink-muted text-[0.875rem]">{t("appearance.noCapes")}</p>
          ) : (
            <>
              <p className="text-ink-muted text-[0.875rem]">{t("accountModal.capeAvailable")}</p>
              {/* One quiet surface: the capes (they scroll when there are many), and the way to hide
                  the cape as its footer, always in view. */}
              <div className="bg-sand/30 ring-graphite/10 overflow-hidden rounded-[0.9rem] ring-1">
                <ul
                  ref={capeList}
                  className="quiet-scroll divide-graphite/10 relative max-h-[15.25rem] divide-y overflow-y-auto"
                >
                  {look.capes.map((c, i) => (
                    <CapeRow
                      key={c.id}
                      cape={c}
                      name={c.alias ?? t("appearance.capeUnnamed", { n: i + 1 })}
                      disabled={!idle}
                      onEquip={() => change("cape", () => api.setCape(account.id, c.id))}
                    />
                  ))}
                </ul>
                {active && (
                  <div className="border-graphite/10 flex justify-end border-t px-1.5 py-1">
                    <button
                      type="button"
                      disabled={!idle}
                      className={BUTTON_SMALL_QUIET}
                      onClick={() => change("cape", () => api.setCape(account.id, null))}
                    >
                      {t("appearance.hideCape")}
                    </button>
                  </div>
                )}
              </div>
            </>
          )}
        </ModalSection>
      )}
      {notice}
    </>
  );
}

function CapeRow({
  cape,
  name,
  disabled,
  onEquip,
}: {
  cape: Cape;
  name: string;
  disabled: boolean;
  onEquip: () => void;
}) {
  const t = useT();
  return (
    <li
      data-active={cape.active ? "" : undefined}
      className={cn(
        "flex min-h-11 items-center gap-3 py-1.5 pr-1.5 pl-3.5 text-[0.875rem]",
        cape.active && "bg-clay/10",
      )}
    >
      <span
        aria-hidden
        className={cn("size-2 shrink-0 rounded-full", cape.active ? "bg-clay" : "bg-graphite/15")}
      />
      <span
        className={cn("text-graphite min-w-0 flex-1 truncate", cape.active && "font-medium")}
        title={name}
      >
        {name}
      </span>
      {cape.active ? (
        <span className="text-clay-deep shrink-0 px-3 text-[0.8125rem] font-medium">
          {t("appearance.equipped")}
        </span>
      ) : (
        <button
          type="button"
          disabled={disabled}
          aria-label={t("appearance.equipNamed", { name })}
          className={BUTTON_SMALL_QUIET}
          onClick={onEquip}
        >
          {t("appearance.equip")}
        </button>
      )}
    </li>
  );
}

/** One sentence about why the look cannot be read or changed, with the way forward when there is one. */
function Notice({
  needsSignIn,
  failure,
  locked,
  api,
  onRetry,
  onUnlocked,
  onAsking,
  onVaultReset,
}: {
  needsSignIn: boolean;
  failure: unknown;
  locked: boolean;
  api: AccountsApi;
  onRetry: () => void;
  onUnlocked: () => void;
  onAsking: (asking: boolean) => void;
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
}) {
  const t = useT();
  const [pass, setPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);

  if (needsSignIn) {
    return (
      <p role="alert" className="text-clay-deep text-[0.875rem]">
        {t("appearance.needsSignIn")}
      </p>
    );
  }

  const unlock = (e: SubmitEvent) => {
    e.preventDefault();
    if (busy || pass.length === 0) return;
    setBusy(true);
    setUnlockError(null);
    api.unlockVault(pass).then(
      () => {
        setPass(""); // the passphrase is never kept
        setBusy(false);
        onUnlocked();
      },
      (err: unknown) => {
        setPass("");
        setBusy(false);
        setUnlockError(describeError("vault", err));
      },
    );
  };

  return (
    <div className="flex flex-col gap-2">
      <p role="alert" className="text-clay-deep text-[0.875rem]">
        {describeError("appearance", failure)}
      </p>
      {locked ? (
        <VaultRecovery
          api={api}
          layout="inline"
          onAsking={onAsking}
          onReset={(snap) => {
            // Every account has to sign in again now; the dialog's own state follows from the list.
            setPass("");
            setUnlockError(null);
            onVaultReset?.(snap);
          }}
        >
          {(recoveryLink) => (
            <>
              <form onSubmit={unlock} className="flex flex-wrap items-end gap-3" noValidate>
                <div className="flex min-w-[12rem] flex-1 flex-col gap-1.5">
                  <label htmlFor="appearance-unlock" className={LABEL}>
                    {t("common.passphrase")}
                  </label>
                  <input
                    id="appearance-unlock"
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={pass}
                    onChange={(e) => setPass(e.target.value)}
                    className={FIELD}
                  />
                </div>
                <button
                  type="submit"
                  disabled={busy || pass.length === 0}
                  className={BUTTON_PRIMARY}
                >
                  {busy ? t("common.working") : t("common.unlock")}
                </button>
                {unlockError && (
                  <p role="alert" className="text-clay-deep w-full text-[0.875rem]">
                    {unlockError}
                  </p>
                )}
              </form>
              {recoveryLink}
            </>
          )}
        </VaultRecovery>
      ) : (
        errorCode(failure) !== "invalid_skin" && (
          <div>
            <button type="button" className={BUTTON_SMALL_QUIET} onClick={onRetry}>
              {t("common.tryAgain")}
            </button>
          </div>
        )
      )}
    </div>
  );
}
