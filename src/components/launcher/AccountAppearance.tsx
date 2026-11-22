import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type RefObject,
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

type PendingCapeFocus =
  { kind: "equip"; capeId: string } | { kind: "hide"; previousActiveIds: string[] } | null;

const SKIN_MODELS = ["classic", "slim"] as const;

interface Props {
  account: { id: string; needsSignIn: boolean };
  skin: boolean;
  cape: boolean;
  api: AccountsApi;
  /** Disables appearance controls when the parent dialog is locked (such as account removal). */
  disabled?: boolean;
  /**
   * The "restore the default skin?" question is showing. Held by the dialog so that Escape and the
   * backdrop leave it alone, like every other destructive confirmation: only its two buttons answer.
   */
  confirmingReset: boolean;
  setConfirmingReset: (confirming: boolean) => void;
  /** "Start fresh?" (forgotten passphrase) is showing: held by the dialog, like the question above. */
  setRecovering: (recovering: boolean) => void;
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
  /** Called when an inline vault unlock removes the unlock form so the dialog can keep focus. */
  onUnlockFocus?: () => void;
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
  disabled = false,
  confirmingReset,
  setConfirmingReset,
  setRecovering,
  onVaultReset,
  onUnlockFocus,
}: Props) {
  const t = useT();
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [model, setModel] = useState<SkinModel>("classic");
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<unknown>(null);
  const needsSignIn = account.needsSignIn;
  const [prevAccountId, setPrevAccountId] = useState(account.id);
  const [prevApi, setPrevApi] = useState(api);
  const [prevNeedsSignIn, setPrevNeedsSignIn] = useState(needsSignIn);
  if (prevAccountId !== account.id || prevApi !== api || prevNeedsSignIn !== needsSignIn) {
    setPrevAccountId(account.id);
    setPrevApi(api);
    setPrevNeedsSignIn(needsSignIn);
    if (prevAccountId !== account.id || prevApi !== api || needsSignIn) {
      setLoad({ kind: "loading" });
      setModel("classic");
      setBusy(null);
      setError(null);
    }
  }

  const inFlight = useRef(false);
  const read = useRef(false);
  const capeList = useRef<HTMLUListElement>(null);
  const capeEmptyRef = useRef<HTMLParagraphElement>(null);
  const hideCapeRef = useRef<HTMLButtonElement>(null);
  const capeButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const placed = useRef(false);
  const alive = useRef(true);
  const requestSeq = useRef(0);
  const needsSignInRef = useRef(needsSignIn);
  const sourceKeyRef = useRef<{
    id: string;
    api: AccountsApi;
    needsSignIn: boolean;
  } | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const uploadButtonRef = useRef<HTMLButtonElement>(null);
  const restoreUploadFocus = useRef(false);
  const restoreDefaultRef = useRef<HTMLButtonElement>(null);
  const restoreDefaultFocus = useRef(false);
  const pendingCapeFocus = useRef<PendingCapeFocus>(null);
  const pendingRetryFocus = useRef(false);
  const retryButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    needsSignInRef.current = needsSignIn;
    const prev = sourceKeyRef.current;
    if (
      prev !== null &&
      (prev.id !== account.id || prev.api !== api || (needsSignIn && !prev.needsSignIn))
    ) {
      requestSeq.current += 1;
      inFlight.current = false;
      read.current = false;
      placed.current = false;
      restoreDefaultFocus.current = false;
      pendingCapeFocus.current = null;
      pendingRetryFocus.current = false;
    }
    sourceKeyRef.current = { id: account.id, api, needsSignIn };
  }, [account.id, api, needsSignIn]);

  useEffect(() => {
    if (!confirmingReset && restoreDefaultFocus.current) {
      if (disabled) {
        restoreDefaultFocus.current = false;
        return;
      }
      if (busy !== null) return;
      restoreDefaultFocus.current = false;
      restoreDefaultRef.current?.focus();
    }
  }, [confirmingReset, busy, disabled]);

  useEffect(() => {
    if (!restoreUploadFocus.current || busy !== null) return;
    if (disabled) {
      restoreUploadFocus.current = false;
      return;
    }
    restoreUploadFocus.current = false;
    uploadButtonRef.current?.focus();
  }, [busy, disabled]);

  useEffect(() => {
    const pending = pendingCapeFocus.current;
    if (!pending) return;
    if (disabled) {
      pendingCapeFocus.current = null;
      return;
    }
    if (busy !== null || confirmingReset || load.kind !== "ready") return;
    pendingCapeFocus.current = null;
    const firstCapeButton =
      capeList.current?.querySelector<HTMLButtonElement>("button:not(:disabled)") ?? null;
    const fallback =
      firstCapeButton ?? capeList.current ?? capeEmptyRef.current ?? restoreDefaultRef.current;
    const anyActive = load.look.capes.some((c) => c.active);
    let target: HTMLElement | null;
    if (pending.kind === "equip") {
      const equipped = load.look.capes.some((c) => c.id === pending.capeId && c.active);
      const equipBtn = capeButtonRefs.current.get(pending.capeId);
      const sameBtn = equipBtn && !equipBtn.disabled ? equipBtn : null;
      target = equipped
        ? (hideCapeRef.current ?? sameBtn ?? fallback)
        : (sameBtn ?? (anyActive ? hideCapeRef.current : null) ?? fallback);
    } else {
      const prevBtn = pending.previousActiveIds
        .map((id) => capeButtonRefs.current.get(id))
        .find((el): el is HTMLButtonElement => el !== undefined && !el.disabled);
      target = anyActive ? (hideCapeRef.current ?? prevBtn ?? fallback) : (prevBtn ?? fallback);
    }
    target?.focus();
  }, [busy, confirmingReset, disabled, load]);

  useEffect(() => {
    if (!pendingRetryFocus.current || load.kind === "loading") return;
    pendingRetryFocus.current = false;
    const activeEl = document.activeElement;
    if (activeEl && activeEl !== document.body && activeEl !== document.documentElement) return;
    if (load.kind === "ready") {
      (uploadButtonRef.current ?? capeList.current ?? capeEmptyRef.current)?.focus();
    } else if (load.kind === "failed") {
      retryButtonRef.current?.focus();
    }
  }, [load.kind]);

  const show = useCallback((look: Appearance) => {
    setError(null);
    setLoad({ kind: "ready", look });
    // The model of the account's skin, when it says; otherwise whatever was last chosen.
    if (look.skinModel) setModel(look.skinModel);
  }, []);

  const reload = useCallback(() => {
    if (!alive.current || needsSignIn || needsSignInRef.current) return;
    const seq = ++requestSeq.current;
    setError(null);
    setLoad({ kind: "loading" });
    api.appearance(account.id).then(
      (look) => {
        if (!alive.current || seq !== requestSeq.current || needsSignInRef.current) return;
        show(look);
      },
      (e: unknown) => {
        if (!alive.current || seq !== requestSeq.current || needsSignInRef.current) return;
        setLoad({ kind: "failed", error: e });
      },
    );
  }, [api, account.id, needsSignIn, show]);

  useEffect(() => {
    // One read per opening, even where React runs the effect twice (development).
    if (needsSignIn || read.current) return;
    read.current = true;
    // reading the account's profile is a request; it has to start when the dialog opens
    reload();
  }, [needsSignIn, reload]);

  /** Runs one change. A second one cannot start while one is running. */
  const change = (
    kind: Exclude<Busy, null>,
    run: () => Promise<Appearance>,
    capeFocus?: PendingCapeFocus,
  ) => {
    if (disabled || inFlight.current) return;
    inFlight.current = true;
    const seq = ++requestSeq.current;
    pendingCapeFocus.current = capeFocus ?? null;
    setBusy(kind);
    setError(null);
    playSound("click");
    run().then(
      (look) => {
        if (!alive.current || seq !== requestSeq.current || needsSignInRef.current) return;
        inFlight.current = false;
        setBusy(null);
        if (kind === "reset") restoreDefaultFocus.current = true;
        if (kind === "upload") restoreUploadFocus.current = true;
        setConfirmingReset(false);
        show(look);
      },
      (e: unknown) => {
        if (!alive.current || seq !== requestSeq.current || needsSignInRef.current) return;
        inFlight.current = false;
        setBusy(null);
        if (kind === "reset") restoreDefaultFocus.current = true;
        if (kind === "upload") restoreUploadFocus.current = true;
        setConfirmingReset(false);
        setError(e); // the current look stays exactly as it was
      },
    );
  };

  const onFile = (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ""; // the same file can be chosen again; the path is never kept
    if (!file || disabled || inFlight.current) return;
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
  const idle = !disabled && ready && busy === null && !confirmingReset;
  const active = look?.capes.some((c) => c.active) ?? false;

  const notice =
    needsSignIn || load.kind === "failed" || error ? (
      <Notice
        needsSignIn={needsSignIn}
        failure={load.kind === "failed" ? load.error : error}
        locked={!needsSignIn && load.kind === "failed" && errorCode(load.error) === "vault_locked"}
        disabled={disabled}
        api={api}
        retryButtonRef={retryButtonRef}
        onRetry={() => {
          pendingRetryFocus.current = true;
          reload();
        }}
        onUnlocked={() => {
          onUnlockFocus?.();
          reload();
        }}
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
              onChange={(next) => {
                playSound("select");
                setModel(next);
              }}
              label={t("accountModal.skinModel")}
              disabled={!idle}
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
              ref={uploadButtonRef}
              type="button"
              disabled={!idle}
              className={BUTTON_SMALL_SOFT}
              onClick={() => {
                if (idle) {
                  playSound("click");
                  fileInput.current?.click();
                }
              }}
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
              onClick={() => {
                if (idle) {
                  playSound("click");
                  setConfirmingReset(true);
                }
              }}
            >
              {t("accountModal.restoreDefault")}
            </button>
          </div>
          {confirmingReset ? (
            <div className="flex flex-wrap items-center justify-end gap-3">
              <p className="text-graphite mr-auto min-w-[14rem] flex-1 text-[0.8125rem]">
                {t("accountModal.resetConfirm")}
              </p>
              <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
                <button
                  type="button"
                  autoFocus
                  disabled={disabled || busy !== null}
                  className={BUTTON_SMALL_QUIET}
                  onClick={() => {
                    if (disabled || busy !== null) return;
                    playSound("click");
                    restoreDefaultFocus.current = true;
                    setConfirmingReset(false);
                  }}
                >
                  {t("accountModal.keepSkin")}
                </button>
                <button
                  type="button"
                  disabled={disabled || busy !== null}
                  className={BUTTON_SMALL_DANGER}
                  onClick={() => change("reset", () => api.resetSkin(account.id))}
                >
                  {busy === "reset"
                    ? t("accountModal.restoring")
                    : t("accountModal.restoreDefault")}
                </button>
              </div>
            </div>
          ) : (
            <p className="text-ink-muted text-[0.8125rem]">{t("accountModal.skinModelNote")}</p>
          )}
        </ModalSection>
      )}

      {cape && !needsSignIn && (
        <ModalSection title={t("accountModal.cape")} compact>
          {look === null ? (
            load.kind === "loading" && (
              <p className="text-ink-muted text-[0.875rem]">{t("appearance.loading")}</p>
            )
          ) : look.capes.length === 0 ? (
            <p
              ref={capeEmptyRef}
              tabIndex={-1}
              className="text-ink-muted text-[0.875rem] outline-hidden"
            >
              {t("appearance.noCapes")}
            </p>
          ) : (
            <>
              <p className="text-ink-muted text-[0.875rem]">{t("accountModal.capeAvailable")}</p>
              {/* One quiet surface: the capes (they scroll when there are many), and the way to hide
                  the cape as its footer, always in view. */}
              <div className="bg-sand/30 ring-graphite/10 overflow-hidden rounded-[0.9rem] ring-1 forced-colors:border forced-colors:border-[CanvasText]">
                <ul
                  ref={capeList}
                  tabIndex={-1}
                  className="quiet-scroll divide-graphite/10 relative max-h-[15.25rem] divide-y overflow-y-auto outline-hidden"
                >
                  {look.capes.map((c, i) => (
                    <CapeRow
                      key={c.id}
                      cape={c}
                      name={c.alias ?? t("appearance.capeUnnamed", { n: i + 1 })}
                      disabled={!idle}
                      buttonRef={(el) => {
                        if (el) capeButtonRefs.current.set(c.id, el);
                        else capeButtonRefs.current.delete(c.id);
                      }}
                      onEquip={() =>
                        change("cape", () => api.setCape(account.id, c.id), {
                          kind: "equip",
                          capeId: c.id,
                        })
                      }
                    />
                  ))}
                </ul>
                {active && (
                  <div className="border-graphite/10 flex justify-end border-t px-1.5 py-1">
                    <button
                      ref={hideCapeRef}
                      type="button"
                      disabled={!idle}
                      className={BUTTON_SMALL_QUIET}
                      onClick={() =>
                        change("cape", () => api.setCape(account.id, null), {
                          kind: "hide",
                          previousActiveIds: look.capes.filter((c) => c.active).map((c) => c.id),
                        })
                      }
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
  buttonRef,
  onEquip,
}: {
  cape: Cape;
  name: string;
  disabled: boolean;
  buttonRef?: (el: HTMLButtonElement | null) => void;
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
        className={cn(
          "size-2 shrink-0 rounded-full forced-colors:border forced-colors:border-[ButtonText]",
          cape.active ? "bg-clay forced-colors:bg-[Highlight]" : "bg-graphite/15",
        )}
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
          ref={buttonRef}
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
  disabled,
  api,
  retryButtonRef,
  onRetry,
  onUnlocked,
  onAsking,
  onVaultReset,
}: {
  needsSignIn: boolean;
  failure: unknown;
  locked: boolean;
  disabled: boolean;
  api: AccountsApi;
  retryButtonRef: RefObject<HTMLButtonElement | null>;
  onRetry: () => void;
  onUnlocked: () => void;
  onAsking: (asking: boolean) => void;
  onVaultReset?: (snapshot: AccountsSnapshot) => void;
}) {
  const t = useT();
  const [pass, setPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);
  const [refocusSeq, setRefocusSeq] = useState(0);
  const [prevApi, setPrevApi] = useState(api);
  const [prevNeedsSignIn, setPrevNeedsSignIn] = useState(needsSignIn);
  if (prevApi !== api || prevNeedsSignIn !== needsSignIn) {
    setPrevApi(api);
    setPrevNeedsSignIn(needsSignIn);
    setBusy(false);
    setUnlockError(null);
    if (needsSignIn) setPass("");
  }
  const alive = useRef(true);
  const unlockSeq = useRef(0);
  const passInputRef = useRef<HTMLInputElement>(null);
  const pendingPassFocus = useRef(false);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      unlockSeq.current += 1;
    };
  }, []);

  useEffect(() => {
    unlockSeq.current += 1;
    pendingPassFocus.current = false;
  }, [api, needsSignIn, locked]);

  useEffect(() => {
    if (!pendingPassFocus.current || disabled || busy) return;
    pendingPassFocus.current = false;
    passInputRef.current?.focus();
  }, [disabled, busy, refocusSeq]);

  if (needsSignIn) {
    return (
      <p role="alert" className="text-clay-deep text-[0.875rem]">
        {t("appearance.needsSignIn")}
      </p>
    );
  }

  const unlock = (e: SubmitEvent) => {
    e.preventDefault();
    if (disabled || busy || pass.length === 0) return;
    const seq = ++unlockSeq.current;
    pendingPassFocus.current = false;
    setBusy(true);
    setUnlockError(null);
    api.unlockVault(pass).then(
      () => {
        if (!alive.current || seq !== unlockSeq.current) return;
        pendingPassFocus.current = false;
        setPass(""); // the passphrase is never kept
        setBusy(false);
        onUnlocked();
      },
      (err: unknown) => {
        if (!alive.current || seq !== unlockSeq.current) return;
        setPass("");
        setBusy(false);
        setUnlockError(describeError("vault", err));
        pendingPassFocus.current = true;
        setRefocusSeq((n) => n + 1);
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
          disabled={disabled || busy}
          onAsking={onAsking}
          onReset={(snap) => {
            // Every account has to sign in again now; the dialog's own state follows from the list.
            pendingPassFocus.current = false;
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
                    ref={passInputRef}
                    id="appearance-unlock"
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    disabled={disabled || busy}
                    value={pass}
                    onChange={(e) => setPass(e.target.value)}
                    aria-invalid={unlockError ? true : undefined}
                    aria-describedby={unlockError ? "appearance-unlock-error" : undefined}
                    className={FIELD}
                  />
                </div>
                <button
                  type="submit"
                  disabled={disabled || busy || pass.length === 0}
                  className={BUTTON_PRIMARY}
                >
                  {busy ? t("common.working") : t("common.unlock")}
                </button>
                {unlockError && (
                  <p
                    id="appearance-unlock-error"
                    role="alert"
                    className="text-clay-deep w-full text-[0.875rem]"
                  >
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
            <button
              ref={retryButtonRef}
              type="button"
              disabled={disabled}
              className={BUTTON_SMALL_QUIET}
              onClick={() => {
                if (!disabled) {
                  playSound("click");
                  onRetry();
                }
              }}
            >
              {t("common.tryAgain")}
            </button>
          </div>
        )
      )}
    </div>
  );
}
