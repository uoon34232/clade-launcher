import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { formatGigabytes } from "@/features/launcher/profileUi";
import { COPYRIGHT_LINE, versionLabel } from "@/lib/appInfo";
import { LANGUAGE_NAMES, LOCALES, useLocale, useT } from "@/lib/i18n";
import { describeError, errorCode } from "@/lib/i18n/errors";
import { chooseLanguage } from "@/lib/i18n/language";
import {
  resetUiSettings,
  saveBehavior,
  saveMemory,
  saveReducedMotion,
  saveTheme,
  saveTransparency,
  saveUiMode,
  type MotionMode,
  type OnExit,
  type OnLaunch,
  type SaveResult,
  type ThemeMode,
  type TransparencyMode,
  type UiMode,
} from "@/lib/settings/uiSettings";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound, sound } from "@/lib/sound/soundService";
import { supportApi, type SupportApi, type UpdateProgress } from "@/lib/support/api";
import { cn } from "@/lib/utils";
import { CatEgg } from "./CatEgg";
import { Modal, ModalHeader, ModalSection } from "./Modal";
import {
  BUTTON_SMALL_DANGER,
  BUTTON_SMALL_PRIMARY,
  BUTTON_SMALL_QUIET,
  BUTTON_SMALL_SOFT,
  BUTTON_SOFT,
} from "./modalStyles";
import { Segmented } from "./Segmented";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Injectable for tests; the real backend by default. */
  support?: SupportApi;
}

const VALUE_KEYS = new Set([
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

type ConfirmAction = "reset" | "retired-vaults" | null;

type UpdateUiState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up_to_date" }
  | { kind: "available"; latestVersion: string; canInstall: boolean; assetSize: number | null }
  | {
      kind: "working";
      stage: "downloading" | "verifying" | "extracting";
      version: string;
      bytesDone: number;
      bytesTotal: number;
    }
  | { kind: "ready"; version: string; error?: unknown }
  | { kind: "applying"; version: string }
  | { kind: "error"; error: unknown };

function applyUpdateProgress(
  prev: UpdateUiState,
  p: UpdateProgress,
  allowWorking: boolean,
): UpdateUiState {
  if (p.stage === "idle") {
    return prev.kind === "working" || prev.kind === "applying" ? { kind: "idle" } : prev;
  }
  if (p.stage === "downloading" || p.stage === "verifying" || p.stage === "extracting") {
    if (!allowWorking) return prev;
    return {
      kind: "working",
      stage: p.stage,
      version: p.version ?? (prev.kind === "working" ? prev.version : ""),
      bytesDone: p.bytesDone,
      bytesTotal: p.bytesTotal,
    };
  }
  if (p.stage === "ready") {
    return {
      kind: "ready",
      version: p.version ?? (prev.kind === "working" || prev.kind === "ready" ? prev.version : ""),
      error: prev.kind === "ready" ? prev.error : undefined,
    };
  }
  if (p.stage === "applying") {
    return {
      kind: "applying",
      version: p.version ?? (prev.kind === "ready" || prev.kind === "applying" ? prev.version : ""),
    };
  }
  return prev;
}

/**
 * Launcher settings: global launcher behaviour only, and intentionally short. Anything that belongs
 * to one profile lives in that profile's settings; anything about a player lives in the account's.
 * A new option needs a real reason; a safe automatic default beats another switch.
 */
export function SettingsDialog({ open, onOpenChange, support = supportApi }: Props) {
  const t = useT();
  const ui = useUiSettings();
  // A pending confirmation ("Reset settings?" or "Clear archived vaults?") or an active update
  // download/apply must not be dismissed by a stray click or Escape.
  const [confirmAction, setConfirmAction] = useState<ConfirmAction>(null);
  // Only checked on an explicit click; never triggers a network call automatically.
  const [updateState, setUpdateState] = useState<UpdateUiState>({ kind: "idle" });
  const [modeError, setModeError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setConfirmAction(null);
    setModeError(null);
    setUpdateState((prev) =>
      prev.kind === "ready" ? { kind: "ready", version: prev.version } : { kind: "idle" },
    );
  }
  const openRef = useRef(open);
  const modeSeqRef = useRef(0);
  useLayoutEffect(() => {
    openRef.current = open;
    modeSeqRef.current += 1;
    return () => {
      openRef.current = false;
      modeSeqRef.current += 1;
    };
  }, [open]);
  const updateBusy = updateState.kind === "working" || updateState.kind === "applying";
  const locked = confirmAction !== null || updateBusy;
  const close = (next: boolean) => {
    if (!next) {
      if (locked) return;
      setConfirmAction(null);
      setModeError(null);
      if (updateState.kind !== "ready") {
        setUpdateState({ kind: "idle" });
      }
    }
    onOpenChange(next);
  };

  return (
    <Modal open={open} onOpenChange={close} size="xl" locked={locked}>
      <div className="mb-1 flex flex-wrap items-center justify-between gap-4">
        <div className="-mb-6">
          <ModalHeader title={t("settings.title")} />
        </div>
        <Segmented<UiMode>
          compact
          disabled={locked}
          label={t("settings.mode")}
          value={ui.uiMode}
          onChange={(mode) => {
            if (mode === ui.uiMode || locked) return;
            playSound("select");
            const seq = ++modeSeqRef.current;
            setModeError(null);
            void saveUiMode(mode).then((r) => {
              if (!openRef.current || modeSeqRef.current !== seq) return;
              setModeError(r.ok ? null : describeError("settings", r.error));
            });
          }}
          options={[
            { value: "simple", label: t("settings.mode.simple") },
            { value: "advanced", label: t("settings.mode.advanced") },
          ]}
        />
      </div>
      {modeError && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {modeError}
        </p>
      )}
      <SettingsBody
        open={open}
        support={support}
        confirmAction={confirmAction}
        setConfirmAction={setConfirmAction}
        updateState={updateState}
        setUpdateState={setUpdateState}
        onDone={() => close(false)}
      />
    </Modal>
  );
}

function SettingsBody({
  open,
  support,
  confirmAction,
  setConfirmAction,
  updateState,
  setUpdateState,
  onDone,
}: {
  open: boolean;
  support: SupportApi;
  confirmAction: ConfirmAction;
  setConfirmAction: (v: ConfirmAction) => void;
  updateState: UpdateUiState;
  setUpdateState: React.Dispatch<React.SetStateAction<UpdateUiState>>;
  onDone: () => void;
}) {
  const t = useT();
  const s = useUiSettings();
  const isAdvanced = s.uiMode === "advanced";
  const [cancellingUpdate, setCancellingUpdate] = useState(false);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setCancellingUpdate(false);
  }
  const openRef = useRef(open);
  const updateSeqRef = useRef(0);
  const checkingActiveRef = useRef(false);
  const installActiveRef = useRef(false);
  const cancellingRef = useRef(false);
  const applyActiveRef = useRef(false);
  const pendingUpdateFocusRef = useRef(false);
  const checkUpdatesRef = useRef<HTMLButtonElement>(null);
  const installUpdateRef = useRef<HTMLButtonElement>(null);
  const openReleaseRef = useRef<HTMLButtonElement>(null);
  const cancelUpdateRef = useRef<HTMLButtonElement>(null);
  const restartUpdateRef = useRef<HTMLButtonElement>(null);
  const doneRef = useRef<HTMLButtonElement>(null);
  const updateBusy = updateState.kind === "working" || updateState.kind === "applying";
  const controlsLocked = confirmAction !== null || updateBusy;

  useLayoutEffect(() => {
    openRef.current = open;
    updateSeqRef.current += 1;
    if (!open) {
      checkingActiveRef.current = false;
      installActiveRef.current = false;
      cancellingRef.current = false;
      applyActiveRef.current = false;
      pendingUpdateFocusRef.current = false;
    }
    return () => {
      openRef.current = false;
      updateSeqRef.current += 1;
      checkingActiveRef.current = false;
      installActiveRef.current = false;
      cancellingRef.current = false;
      applyActiveRef.current = false;
      pendingUpdateFocusRef.current = false;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    let sawProgressEvent = false;
    const seqAtOpen = updateSeqRef.current;

    const init = async () => {
      if (support.onUpdateProgress) {
        try {
          const stop = await support.onUpdateProgress((p) => {
            if (cancelled) return;
            if (p.stage === "idle" || p.stage === "ready" || p.stage === "applying") {
              sawProgressEvent = true;
              if (p.stage === "ready" || p.stage === "applying") {
                updateSeqRef.current += 1;
              }
              if (installActiveRef.current || applyActiveRef.current) {
                pendingUpdateFocusRef.current = true;
              }
              installActiveRef.current = false;
              applyActiveRef.current = p.stage === "applying";
              cancellingRef.current = false;
              setCancellingUpdate(false);
            } else if (installActiveRef.current) {
              sawProgressEvent = true;
            }
            setUpdateState((prev) => applyUpdateProgress(prev, p, installActiveRef.current));
          });
          if (cancelled) {
            stop();
            return;
          }
          unlisten = stop;
        } catch {
          // ignore listener registration failure
        }
      }
      if (cancelled || !support.getUpdateStatus) return;
      try {
        const p = await support.getUpdateStatus();
        if (cancelled || sawProgressEvent || updateSeqRef.current !== seqAtOpen) return;
        if (p.stage === "downloading" || p.stage === "verifying" || p.stage === "extracting") {
          installActiveRef.current = true;
        } else if (p.stage === "applying") {
          applyActiveRef.current = true;
        }
        if (p.stage !== "idle") {
          setUpdateState((prev) => applyUpdateProgress(prev, p, true));
        }
      } catch {
        // ignore status fetch failure
      }
    };

    void init();

    return () => {
      cancelled = true;
      updateSeqRef.current += 1;
      unlisten?.();
    };
  }, [open, support, setUpdateState]);

  const checkUpdates = () => {
    if (
      !support.checkForUpdates ||
      checkingActiveRef.current ||
      updateState.kind === "checking" ||
      controlsLocked ||
      installActiveRef.current ||
      applyActiveRef.current
    ) {
      return;
    }
    const seq = ++updateSeqRef.current;
    checkingActiveRef.current = true;
    pendingUpdateFocusRef.current = true;
    playSound("click");
    setUpdateState({ kind: "checking" });
    void support.checkForUpdates().then(
      (res) => {
        if (!openRef.current || updateSeqRef.current !== seq) return;
        checkingActiveRef.current = false;
        if (res.status === "update_available" && res.latestVersion) {
          setUpdateState({
            kind: "available",
            latestVersion: res.latestVersion,
            canInstall: Boolean(res.canInstall),
            assetSize: res.assetSize ?? null,
          });
        } else {
          setUpdateState({ kind: "up_to_date" });
        }
      },
      (e: unknown) => {
        if (!openRef.current || updateSeqRef.current !== seq) return;
        checkingActiveRef.current = false;
        setUpdateState({ kind: "error", error: e });
      },
    );
  };

  const openRelease = (tag: string) => {
    if (
      !support.openReleasePage ||
      controlsLocked ||
      installActiveRef.current ||
      applyActiveRef.current
    ) {
      return;
    }
    const seq = ++updateSeqRef.current;
    checkingActiveRef.current = false;
    playSound("click");
    void support.openReleasePage(tag).catch((e: unknown) => {
      if (!openRef.current || updateSeqRef.current !== seq) return;
      setUpdateState({ kind: "error", error: e });
    });
  };

  const installUpdate = () => {
    if (
      installActiveRef.current ||
      applyActiveRef.current ||
      updateState.kind !== "available" ||
      !support.installUpdate ||
      controlsLocked
    ) {
      return;
    }
    const seq = ++updateSeqRef.current;
    const { latestVersion, canInstall, assetSize } = updateState;
    checkingActiveRef.current = false;
    installActiveRef.current = true;
    cancellingRef.current = false;
    pendingUpdateFocusRef.current = true;
    playSound("click");
    setCancellingUpdate(false);
    setUpdateState({
      kind: "working",
      stage: "downloading",
      version: latestVersion,
      bytesDone: 0,
      bytesTotal: assetSize ?? 0,
    });
    void support.installUpdate(latestVersion).then(
      (res) => {
        if (!openRef.current || updateSeqRef.current !== seq) return;
        installActiveRef.current = false;
        cancellingRef.current = false;
        pendingUpdateFocusRef.current = true;
        setCancellingUpdate(false);
        if (res.stage === "ready") {
          setUpdateState({ kind: "ready", version: res.version ?? latestVersion });
        } else {
          setUpdateState({ kind: "available", latestVersion, canInstall, assetSize });
        }
      },
      (e: unknown) => {
        if (!openRef.current || updateSeqRef.current !== seq) return;
        installActiveRef.current = false;
        cancellingRef.current = false;
        pendingUpdateFocusRef.current = true;
        setCancellingUpdate(false);
        if (errorCode(e) === "update_cancelled") {
          setUpdateState({ kind: "available", latestVersion, canInstall, assetSize });
        } else {
          setUpdateState({ kind: "error", error: e });
        }
      },
    );
  };

  const cancelUpdate = () => {
    if (
      cancellingRef.current ||
      cancellingUpdate ||
      updateState.kind !== "working" ||
      updateState.stage === "extracting" ||
      !support.cancelUpdate
    ) {
      return;
    }
    const seq = updateSeqRef.current;
    cancellingRef.current = true;
    pendingUpdateFocusRef.current = true;
    playSound("click");
    setCancellingUpdate(true);
    void support.cancelUpdate().catch(() => {
      if (!openRef.current || updateSeqRef.current !== seq) return;
      cancellingRef.current = false;
      setCancellingUpdate(false);
    });
  };

  const restartToUpdate = () => {
    if (
      applyActiveRef.current ||
      installActiveRef.current ||
      updateState.kind !== "ready" ||
      !support.applyStagedUpdate ||
      confirmAction !== null
    ) {
      return;
    }
    const seq = ++updateSeqRef.current;
    const { version } = updateState;
    checkingActiveRef.current = false;
    applyActiveRef.current = true;
    pendingUpdateFocusRef.current = true;
    playSound("click");
    setUpdateState({ kind: "applying", version });
    void support.applyStagedUpdate().catch((e: unknown) => {
      if (!openRef.current || updateSeqRef.current !== seq) return;
      applyActiveRef.current = false;
      pendingUpdateFocusRef.current = true;
      const code = errorCode(e);
      if (
        code === "update_game_running" ||
        code === "update_prepare_running" ||
        code === "update_install_failed"
      ) {
        setUpdateState({ kind: "ready", version, error: e });
      } else {
        setUpdateState({ kind: "error", error: e });
      }
    });
  };

  const updateStage = updateState.kind === "working" ? updateState.stage : null;

  useEffect(() => {
    if (
      !open ||
      !pendingUpdateFocusRef.current ||
      confirmAction !== null ||
      updateState.kind === "checking" ||
      updateState.kind === "applying" ||
      (updateState.kind === "working" && updateStage === "extracting") ||
      cancellingUpdate
    ) {
      return;
    }
    const target = [
      installUpdateRef.current,
      openReleaseRef.current,
      restartUpdateRef.current,
      cancelUpdateRef.current,
      checkUpdatesRef.current,
      doneRef.current,
    ].find((el): el is HTMLButtonElement => el !== null && !el.disabled);
    if (!target) return;
    pendingUpdateFocusRef.current = false;
    const active = document.activeElement;
    if (active && active !== document.body && active !== document.documentElement) return;
    target.focus();
  }, [open, confirmAction, updateState.kind, updateStage, cancellingUpdate]);

  const downloadPercent =
    updateState.kind === "working" &&
    updateState.stage === "downloading" &&
    updateState.bytesTotal > 0
      ? Math.min(100, Math.round((updateState.bytesDone / updateState.bytesTotal) * 100))
      : null;

  const errorText =
    updateState.kind === "error"
      ? describeError("settings", updateState.error)
      : updateState.kind === "ready" && updateState.error
        ? describeError("settings", updateState.error)
        : null;

  const updateStatus =
    updateState.kind === "checking"
      ? { text: t("settings.checkingUpdates"), srOnly: true, accent: false }
      : updateState.kind === "up_to_date"
        ? { text: t("settings.updatesUpToDate"), srOnly: false, accent: false }
        : updateState.kind === "available"
          ? {
              text: t("settings.updateAvailable", { version: updateState.latestVersion }),
              srOnly: false,
              accent: true,
            }
          : updateState.kind === "working"
            ? updateState.stage === "downloading"
              ? {
                  text: t("settings.updatingDownloadingUnknown"),
                  srOnly: downloadPercent !== null,
                  accent: false,
                }
              : updateState.stage === "verifying"
                ? { text: t("settings.updatingVerifying"), srOnly: false, accent: false }
                : { text: t("settings.updatingExtracting"), srOnly: false, accent: false }
            : updateState.kind === "ready" && !errorText
              ? { text: t("settings.updateReady"), srOnly: false, accent: true }
              : updateState.kind === "applying"
                ? { text: t("settings.updatingRestarting"), srOnly: false, accent: false }
                : null;

  return (
    <>
      <div className="grid gap-x-6 sm:grid-cols-2 lg:gap-x-8">
        <div className="flex min-w-0 flex-col">
          <InterfaceSection open={open} isAdvanced={isAdvanced} disabled={controlsLocked} />
          {isAdvanced && <MinecraftSection open={open} disabled={controlsLocked} />}
        </div>
        <div className="flex min-w-0 flex-col">
          {isAdvanced && (
            <BehaviorSection
              open={open}
              onLaunch={s.onLaunch}
              onExit={s.onExit}
              disabled={controlsLocked}
            />
          )}
          <SupportSection
            open={open}
            support={support}
            isAdvanced={isAdvanced}
            disabled={updateBusy}
            confirmAction={confirmAction}
            setConfirmAction={setConfirmAction}
          />
        </div>
      </div>
      <div className="border-graphite/10 relative mt-1 flex items-center justify-between gap-3 border-t pt-2">
        <div className="flex shrink-0 items-center gap-4">
          <p className="text-ink-muted text-[0.75rem] leading-snug">
            <span className="block">Clade</span>
            <span className="block">{COPYRIGHT_LINE}</span>
            <span className="block">{versionLabel(s.version)}</span>
          </p>
          <CatEgg />
        </div>
        <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-2">
          <div
            className={
              support.checkForUpdates || updateState.kind !== "idle"
                ? "flex min-w-0 flex-wrap items-center justify-end gap-2"
                : "contents"
            }
          >
            <span
              role="status"
              aria-live="polite"
              aria-atomic="true"
              className={
                updateStatus && !updateStatus.srOnly
                  ? updateStatus.accent
                    ? "text-clay-deep truncate text-[0.8125rem] font-medium"
                    : "text-ink-muted truncate text-[0.8125rem] tabular-nums"
                  : "sr-only fixed top-0 left-0"
              }
            >
              {updateStatus?.text ?? ""}
            </span>
            {updateState.kind === "working" && updateState.stage === "downloading" && (
              <>
                {downloadPercent !== null && (
                  <span className="text-ink-muted text-[0.8125rem] tabular-nums">
                    {t("settings.updatingDownloading", { percent: downloadPercent })}
                  </span>
                )}
                <div
                  role="progressbar"
                  aria-label={t("settings.updateProgress")}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={downloadPercent ?? undefined}
                  className="bg-sand border-sand-border/60 h-1.5 w-14 shrink-0 overflow-hidden rounded-full border"
                >
                  {downloadPercent !== null ? (
                    <div
                      className="bg-progress-fill h-full rounded-full transition-[width] duration-150"
                      style={{ width: `${downloadPercent}%` }}
                    />
                  ) : (
                    <div className="indeterminate-bar bg-progress-fill h-full w-1/3 rounded-full" />
                  )}
                </div>
              </>
            )}
            {errorText && (
              <span role="alert" className="text-clay-deep text-[0.8125rem] leading-snug">
                {errorText}
              </span>
            )}
            {updateState.kind === "available" ? (
              <>
                {updateState.canInstall && support.installUpdate && (
                  <button
                    ref={installUpdateRef}
                    type="button"
                    disabled={controlsLocked}
                    onClick={installUpdate}
                    className={`${BUTTON_SMALL_PRIMARY} shrink-0 whitespace-nowrap`}
                  >
                    {t("settings.installUpdate")}
                  </button>
                )}
                {support.openReleasePage && (
                  <button
                    ref={openReleaseRef}
                    type="button"
                    disabled={controlsLocked}
                    onClick={() => openRelease(updateState.latestVersion)}
                    className={`${
                      updateState.canInstall && support.installUpdate
                        ? BUTTON_SMALL_QUIET
                        : BUTTON_SMALL_SOFT
                    } shrink-0 whitespace-nowrap`}
                  >
                    {t("settings.openRelease")}
                  </button>
                )}
              </>
            ) : updateState.kind === "working" ? (
              support.cancelUpdate && (
                <button
                  ref={cancelUpdateRef}
                  type="button"
                  disabled={cancellingUpdate || updateState.stage === "extracting"}
                  onClick={cancelUpdate}
                  className={`${BUTTON_SMALL_QUIET} shrink-0 whitespace-nowrap`}
                >
                  {t("settings.cancelUpdate")}
                </button>
              )
            ) : updateState.kind === "ready" ? (
              support.applyStagedUpdate && (
                <button
                  ref={restartUpdateRef}
                  type="button"
                  disabled={confirmAction !== null}
                  onClick={restartToUpdate}
                  className={`${BUTTON_SMALL_PRIMARY} shrink-0 whitespace-nowrap`}
                >
                  {t("settings.restartToUpdate")}
                </button>
              )
            ) : updateState.kind === "applying" ? null : (
              support.checkForUpdates && (
                <button
                  ref={checkUpdatesRef}
                  type="button"
                  disabled={updateState.kind === "checking" || controlsLocked}
                  onClick={checkUpdates}
                  className={`${BUTTON_SMALL_QUIET} shrink-0 whitespace-nowrap`}
                >
                  {updateState.kind === "checking"
                    ? t("settings.checkingUpdates")
                    : updateState.kind === "up_to_date" || updateState.kind === "error"
                      ? t("settings.checkUpdatesAgain")
                      : t("settings.checkUpdates")}
                </button>
              )
            )}
          </div>
          <button
            ref={doneRef}
            type="button"
            className={`${BUTTON_SOFT} shrink-0`}
            disabled={controlsLocked}
            onClick={() => {
              playSound("click");
              onDone();
            }}
          >
            {t("common.done")}
          </button>
        </div>
      </div>
    </>
  );
}

/** A label with its control to the right (or below it, for wide controls). */
function Row({
  id,
  label,
  stacked = false,
  children,
}: {
  id: string;
  label: string;
  stacked?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className={
        stacked
          ? "flex min-w-0 flex-col gap-1.5"
          : "flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1.5"
      }
    >
      <span id={id} className="text-graphite text-[0.9375rem]">
        {label}
      </span>
      {children}
    </div>
  );
}

// ------------------------------------------------------------------------------ Interface

function InterfaceSection({
  open,
  isAdvanced,
  disabled,
}: {
  open: boolean;
  isAdvanced: boolean;
  disabled: boolean;
}) {
  const t = useT();
  const locale = useLocale();
  const ui = useUiSettings();
  const [s, setS] = useState(() => sound.getSettings());
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setError(null);
  }
  const openRef = useRef(open);
  const saveSeqRef = useRef(0);
  useLayoutEffect(() => {
    openRef.current = open;
    saveSeqRef.current += 1;
    return () => {
      openRef.current = false;
      saveSeqRef.current += 1;
    };
  }, [open]);
  useEffect(() => sound.subscribe(setS), []);

  const report = (seq: number, r: SaveResult) => {
    if (!openRef.current || saveSeqRef.current !== seq) return;
    setError(r.ok ? null : describeError("settings", r.error));
  };

  const volumeActive = s.enabled && !disabled;

  return (
    <ModalSection title={t("settings.interface")} compact className="gap-1 py-1.5">
      <Row id="language-label" label={t("settings.language")}>
        <Segmented
          compact
          disabled={disabled}
          label={t("settings.language")}
          value={locale}
          onChange={(next) => {
            if (next === locale || disabled) return;
            playSound("select");
            setError(null);
            void chooseLanguage(next);
          }}
          options={LOCALES.map((l) => ({ value: l, label: LANGUAGE_NAMES[l] }))}
        />
      </Row>

      <Row id="theme-label" label={t("settings.theme")}>
        <Segmented<ThemeMode>
          compact
          disabled={disabled}
          className="gap-0.5 [&>button]:px-1.5"
          label={t("settings.theme")}
          value={ui.theme}
          onChange={(mode) => {
            if (mode === ui.theme || disabled) return;
            playSound("select");
            const seq = ++saveSeqRef.current;
            setError(null);
            void saveTheme(mode).then((r) => report(seq, r));
          }}
          options={[
            { value: "system", label: t("settings.theme.system") },
            { value: "light", label: t("settings.theme.light") },
            { value: "dark", label: t("settings.theme.dark") },
          ]}
        />
      </Row>

      <Row id="sound-effects-label" label={t("settings.sound")}>
        <button
          type="button"
          role="switch"
          disabled={disabled}
          aria-checked={s.enabled}
          aria-labelledby="sound-effects-label"
          onClick={() => {
            sound.setSettings({ enabled: !s.enabled });
            if (!s.enabled) playSound("select"); // a tick to confirm it is back on
          }}
          className={`dock-item focus-visible:ring-ring focus-visible:ring-offset-card relative h-7 w-12 shrink-0 cursor-default rounded-full border outline-hidden transition-colors duration-200 focus-visible:ring-2 focus-visible:ring-offset-2 disabled:opacity-50 ${s.enabled ? "border-clay bg-clay" : "border-sand-border bg-sand"}`}
        >
          <span
            aria-hidden
            className={`ease-out-soft absolute top-[calc(0.25rem-1px)] left-[calc(0.25rem-1px)] size-5 rounded-full shadow-sm transition-transform duration-200 ${s.enabled ? "translate-x-5 bg-[#fbf6ee]" : "bg-ink-muted"}`}
          />
        </button>
      </Row>
      <div className="flex items-center gap-3">
        <label
          htmlFor="ui-volume"
          className={`shrink-0 text-[0.9375rem] ${
            volumeActive
              ? "text-graphite"
              : "text-ink-muted opacity-70 forced-colors:text-[GrayText] forced-colors:opacity-100"
          }`}
        >
          {t("settings.volume")}
        </label>
        <input
          id="ui-volume"
          type="range"
          min={0}
          max={100}
          step={5}
          disabled={!s.enabled || disabled}
          value={Math.round(s.volume * 100)}
          onChange={(e) => sound.setSettings({ volume: Number(e.target.value) / 100 })}
          onPointerUp={() => playSound("select")}
          onKeyUp={(e) => {
            if (VALUE_KEYS.has(e.key)) playSound("select");
          }}
          aria-valuetext={`${Math.round(s.volume * 100)}%`}
          className="accent-clay focus-visible:ring-ring ml-auto h-6 w-32 cursor-default outline-hidden focus-visible:ring-2 disabled:cursor-not-allowed"
        />
        <span
          aria-hidden
          className={`w-9 shrink-0 text-right text-[0.8125rem] whitespace-nowrap tabular-nums ${
            volumeActive
              ? "text-ink-muted"
              : "text-ink-muted opacity-70 forced-colors:text-[GrayText] forced-colors:opacity-100"
          }`}
        >
          {Math.round(s.volume * 100)}%
        </span>
      </div>

      {isAdvanced && (
        <>
          <Row id="motion-label" label={t("settings.motion")} stacked>
            <Segmented<MotionMode>
              compact
              disabled={disabled}
              label={t("settings.motion")}
              value={ui.reducedMotion}
              onChange={(mode) => {
                if (mode === ui.reducedMotion || disabled) return;
                playSound("select");
                const seq = ++saveSeqRef.current;
                setError(null);
                void saveReducedMotion(mode).then((r) => report(seq, r));
              }}
              options={[
                { value: "system", label: t("settings.motion.system") },
                { value: "on", label: t("settings.motion.on") },
                { value: "off", label: t("settings.motion.off") },
              ]}
            />
          </Row>

          <Row id="transparency-label" label={t("settings.transparency")} stacked>
            <Segmented<TransparencyMode>
              compact
              disabled={disabled}
              label={t("settings.transparency")}
              value={ui.transparency}
              onChange={(mode) => {
                if (mode === ui.transparency || disabled) return;
                playSound("select");
                const seq = ++saveSeqRef.current;
                setError(null);
                void saveTransparency(mode).then((r) => report(seq, r));
              }}
              options={[
                { value: "system", label: t("settings.transparency.system") },
                { value: "on", label: t("settings.transparency.on") },
                { value: "off", label: t("settings.transparency.off") },
              ]}
            />
          </Row>
        </>
      )}
      {error && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {error}
        </p>
      )}
    </ModalSection>
  );
}

// ------------------------------------------------------------------------------ Minecraft

function MinecraftSection({ open, disabled }: { open: boolean; disabled: boolean }) {
  const t = useT();
  const ui = useUiSettings();
  const { min, max, auto } = ui.memory;
  const custom = ui.memoryMb !== null;
  const [draft, setDraft] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setDraft(null);
    setError(null);
  }
  const openRef = useRef(open);
  const saveSeqRef = useRef(0);
  const dragSeqRef = useRef(0);
  const draftRef = useRef<number | null>(null);
  const inFlightMemoryRef = useRef<number | null | undefined>(undefined);
  const value = Math.min(max, Math.max(min, draft ?? ui.memoryMb ?? auto));

  useLayoutEffect(() => {
    openRef.current = open;
    saveSeqRef.current += 1;
    if (!open) {
      dragSeqRef.current += 1;
      draftRef.current = null;
      inFlightMemoryRef.current = undefined;
    }
    return () => {
      openRef.current = false;
      saveSeqRef.current += 1;
      dragSeqRef.current += 1;
      draftRef.current = null;
      inFlightMemoryRef.current = undefined;
    };
  }, [open]);

  const report = (r: SaveResult) => setError(r.ok ? null : describeError("settings", r.error));
  const commit = (rawMb: number) => {
    if (disabled || draftRef.current === null || !Number.isFinite(rawMb)) return;
    const mb = Math.min(max, Math.max(min, rawMb));
    if (mb === ui.memoryMb && inFlightMemoryRef.current === undefined) {
      draftRef.current = null;
      setDraft(null);
      setError(null);
      return;
    }
    if (inFlightMemoryRef.current === mb) {
      return;
    }
    const seq = ++saveSeqRef.current;
    const dragSeq = dragSeqRef.current;
    inFlightMemoryRef.current = mb;
    setError(null);
    void saveMemory(mb).then((r) => {
      if (!openRef.current || saveSeqRef.current !== seq) return;
      inFlightMemoryRef.current = undefined;
      if (dragSeqRef.current === dragSeq) {
        draftRef.current = null;
        setDraft(null);
      }
      report(r);
    });
  };

  return (
    <ModalSection title={t("settings.minecraft")} compact className="gap-1 py-1.5">
      <Row id="memory-label" label={t("settings.memory")}>
        <Segmented<"auto" | "custom">
          compact
          disabled={disabled}
          label={t("settings.memory")}
          value={custom ? "custom" : "auto"}
          onChange={(mode) => {
            const nextMb = mode === "auto" ? null : auto;
            if (
              (mode === "custom") === custom ||
              disabled ||
              inFlightMemoryRef.current === nextMb
            ) {
              return;
            }
            playSound("select");
            dragSeqRef.current += 1;
            const seq = ++saveSeqRef.current;
            draftRef.current = null;
            inFlightMemoryRef.current = nextMb;
            setDraft(null);
            setError(null);
            void saveMemory(nextMb).then((r) => {
              if (!openRef.current || saveSeqRef.current !== seq) return;
              inFlightMemoryRef.current = undefined;
              report(r);
            });
          }}
          options={[
            { value: "auto", label: t("settings.memory.auto") },
            { value: "custom", label: t("settings.memory.custom") },
          ]}
        />
      </Row>
      {custom ? (
        <div className="flex items-center gap-3">
          <input
            id="memory-slider"
            type="range"
            min={min}
            max={max}
            step={256}
            disabled={disabled}
            value={value}
            onChange={(e) => {
              const next = Number(e.target.value);
              dragSeqRef.current += 1;
              draftRef.current = next;
              setDraft(next);
              setError(null);
            }}
            onPointerUp={(e) => commit(Number(e.currentTarget.value))}
            onKeyUp={(e) => {
              if (VALUE_KEYS.has(e.key)) commit(Number(e.currentTarget.value));
            }}
            onBlur={(e) => commit(Number(e.currentTarget.value))}
            aria-label={t("settings.memory.slider")}
            aria-valuetext={formatGigabytes(value)}
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? "minecraft-memory-error" : undefined}
            className="accent-clay focus-visible:ring-ring h-6 min-w-0 flex-1 cursor-default outline-hidden focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
          />
          <span
            aria-hidden
            className={`w-14 shrink-0 text-right text-[0.875rem] whitespace-nowrap tabular-nums ${
              disabled
                ? "text-ink-muted opacity-70 forced-colors:text-[GrayText] forced-colors:opacity-100"
                : "text-graphite"
            }`}
          >
            {formatGigabytes(value)}
          </span>
        </div>
      ) : (
        <p className="text-ink-muted text-[0.8125rem]">
          {t("settings.memory.autoNote", { value: formatGigabytes(auto) })}
        </p>
      )}
      {error && (
        <p id="minecraft-memory-error" role="alert" className="text-clay-deep text-[0.8125rem]">
          {error}
        </p>
      )}
    </ModalSection>
  );
}

// ------------------------------------------------------------------------------- Behavior

function BehaviorSection({
  open,
  onLaunch,
  onExit,
  disabled,
}: {
  open: boolean;
  onLaunch: OnLaunch;
  onExit: OnExit;
  disabled: boolean;
}) {
  const t = useT();
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setError(null);
  }
  const openRef = useRef(open);
  const saveSeqRef = useRef(0);
  useLayoutEffect(() => {
    openRef.current = open;
    saveSeqRef.current += 1;
    return () => {
      openRef.current = false;
      saveSeqRef.current += 1;
    };
  }, [open]);

  const report = (seq: number, r: SaveResult) => {
    if (!openRef.current || saveSeqRef.current !== seq) return;
    setError(r.ok ? null : describeError("settings", r.error));
  };

  return (
    <ModalSection title={t("settings.behavior")} compact className="gap-1 py-1.5">
      <Row id="on-launch-label" label={t("settings.onLaunch")} stacked>
        <Segmented<OnLaunch>
          compact
          disabled={disabled}
          className="gap-0.5 [&>button]:px-1.5"
          label={t("settings.onLaunch")}
          value={onLaunch}
          onChange={(v) => {
            if (v === onLaunch || disabled) return;
            playSound("select");
            const seq = ++saveSeqRef.current;
            setError(null);
            void saveBehavior({ onLaunch: v }).then((r) => report(seq, r));
          }}
          options={[
            { value: "embed", label: t("settings.onLaunch.embed") },
            { value: "keep_open", label: t("settings.onLaunch.keep") },
            { value: "hide", label: t("settings.onLaunch.hide") },
          ]}
        />
      </Row>
      <Row id="on-exit-label" label={t("settings.onExit")} stacked>
        <Segmented<OnExit>
          compact
          disabled={disabled}
          label={t("settings.onExit")}
          value={onExit}
          onChange={(v) => {
            if (v === onExit || disabled) return;
            playSound("select");
            const seq = ++saveSeqRef.current;
            setError(null);
            void saveBehavior({ onExit: v }).then((r) => report(seq, r));
          }}
          options={[
            { value: "show", label: t("settings.onExit.show") },
            { value: "stay_hidden", label: t("settings.onExit.hidden") },
          ]}
        />
      </Row>
      {onExit === "stay_hidden" && (
        <p className="text-ink-muted text-[0.8125rem]">{t("settings.onExit.hiddenNote")}</p>
      )}
      {error && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {error}
        </p>
      )}
    </ModalSection>
  );
}

// --------------------------------------------------------------------------------- Support

type Notice = { kind: "ok" | "error"; text: string } | null;

function SupportSection({
  open,
  support,
  isAdvanced,
  disabled,
  confirmAction,
  setConfirmAction,
}: {
  open: boolean;
  support: SupportApi;
  isAdvanced: boolean;
  disabled: boolean;
  confirmAction: ConfirmAction;
  setConfirmAction: (v: ConfirmAction) => void;
}) {
  const t = useT();
  const [notice, setNotice] = useState<Notice>(null);
  const [exporting, setExporting] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [retiredCount, setRetiredCount] = useState(0);
  const [clearingVaults, setClearingVaults] = useState(false);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    setNotice(null);
    setExporting(false);
    setResetting(false);
    setClearingVaults(false);
    if (!open) setRetiredCount(0);
  }
  const openRef = useRef(open);
  const noticeSeqRef = useRef(0);
  const exportSeqRef = useRef(0);
  const confirmSeqRef = useRef(0);
  const exportingRef = useRef(false);
  const resettingRef = useRef(false);
  const clearingVaultsRef = useRef(false);
  const openFolderRef = useRef<HTMLButtonElement>(null);
  const exportDiagnosticsRef = useRef<HTMLButtonElement>(null);
  const pendingExportFocus = useRef(false);
  const resetTriggerRef = useRef<HTMLButtonElement>(null);
  const restoreResetFocus = useRef(false);
  const retiredTriggerRef = useRef<HTMLButtonElement>(null);
  const restoreRetiredFocus = useRef(false);

  useLayoutEffect(() => {
    openRef.current = open;
    noticeSeqRef.current += 1;
    exportSeqRef.current += 1;
    confirmSeqRef.current += 1;
    if (!open) {
      exportingRef.current = false;
      pendingExportFocus.current = false;
      resettingRef.current = false;
      clearingVaultsRef.current = false;
      restoreResetFocus.current = false;
      restoreRetiredFocus.current = false;
    }
    return () => {
      openRef.current = false;
      noticeSeqRef.current += 1;
      exportSeqRef.current += 1;
      confirmSeqRef.current += 1;
      exportingRef.current = false;
      pendingExportFocus.current = false;
      resettingRef.current = false;
      clearingVaultsRef.current = false;
      restoreResetFocus.current = false;
      restoreRetiredFocus.current = false;
    };
  }, [open]);

  useEffect(() => {
    if (!open || !support.retiredVaultsCount) return;
    let cancelled = false;
    support.retiredVaultsCount().then(
      (n) => {
        if (!cancelled && openRef.current) setRetiredCount(n);
      },
      () => {
        if (!cancelled && openRef.current) setRetiredCount(0);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, support]);

  useEffect(() => {
    if (!open || confirmAction !== null) return;
    if (restoreResetFocus.current) {
      restoreResetFocus.current = false;
      (resetTriggerRef.current ?? openFolderRef.current)?.focus();
    }
    if (restoreRetiredFocus.current) {
      restoreRetiredFocus.current = false;
      (retiredTriggerRef.current ?? openFolderRef.current ?? resetTriggerRef.current)?.focus();
    }
  }, [open, confirmAction]);

  useEffect(() => {
    if (!open || confirmAction !== null || exporting || !pendingExportFocus.current) return;
    pendingExportFocus.current = false;
    const active = document.activeElement;
    if (active && active !== document.body && active !== document.documentElement) return;
    (exportDiagnosticsRef.current ?? openFolderRef.current)?.focus();
  }, [open, confirmAction, exporting]);

  const fail = (e: unknown) => setNotice({ kind: "error", text: describeError("settings", e) });
  const inFlightActionsRef = useRef(new Set<string>());
  useEffect(() => {
    if (!open) inFlightActionsRef.current.clear();
  }, [open]);
  const run = (key: string, action: () => Promise<void>) => {
    if (disabled || confirmAction !== null || inFlightActionsRef.current.has(key)) return;
    inFlightActionsRef.current.add(key);
    const noticeSeq = ++noticeSeqRef.current;
    playSound("click");
    setNotice(null);
    void action().then(
      () => {
        inFlightActionsRef.current.delete(key);
      },
      (e: unknown) => {
        inFlightActionsRef.current.delete(key);
        if (!openRef.current || noticeSeqRef.current !== noticeSeq) return;
        fail(e);
      },
    );
  };

  const exportDiagnostics = () => {
    if (disabled || confirmAction !== null || exportingRef.current) return;
    exportingRef.current = true;
    pendingExportFocus.current = true;
    const exportSeq = ++exportSeqRef.current;
    const noticeSeq = ++noticeSeqRef.current;
    playSound("click");
    setNotice(null);
    setExporting(true);
    void support.exportDiagnostics().then(
      (r) => {
        if (!openRef.current || exportSeqRef.current !== exportSeq) return;
        exportingRef.current = false;
        setExporting(false);
        if (noticeSeqRef.current === noticeSeq) {
          setNotice({ kind: "ok", text: t("settings.diagnosticsDone", { name: r.name }) });
        }
      },
      (e: unknown) => {
        if (!openRef.current || exportSeqRef.current !== exportSeq) return;
        exportingRef.current = false;
        setExporting(false);
        if (noticeSeqRef.current === noticeSeq) fail(e);
      },
    );
  };

  const reset = () => {
    if (resettingRef.current) return;
    resettingRef.current = true;
    const seq = ++confirmSeqRef.current;
    const noticeSeq = ++noticeSeqRef.current;
    playSound("click");
    setResetting(true);
    void resetUiSettings().then((r) => {
      if (!openRef.current || confirmSeqRef.current !== seq) return;
      resettingRef.current = false;
      setResetting(false);
      restoreResetFocus.current = true;
      setConfirmAction(null);
      if (!r.ok && noticeSeqRef.current === noticeSeq) fail(r.error);
    });
  };

  const clearRetiredVaults = () => {
    if (!support.clearRetiredVaults || clearingVaultsRef.current) return;
    clearingVaultsRef.current = true;
    const seq = ++confirmSeqRef.current;
    const noticeSeq = ++noticeSeqRef.current;
    playSound("click");
    setClearingVaults(true);
    void support.clearRetiredVaults().then(
      (removed) => {
        if (!openRef.current || confirmSeqRef.current !== seq) return;
        clearingVaultsRef.current = false;
        setClearingVaults(false);
        const deleted = removed > 0 ? removed : retiredCount;
        setRetiredCount(0);
        restoreRetiredFocus.current = true;
        setConfirmAction(null);
        if (noticeSeqRef.current === noticeSeq) {
          setNotice({ kind: "ok", text: t("settings.retiredVaultsCleared", { count: deleted }) });
        }
      },
      (e: unknown) => {
        if (!openRef.current || confirmSeqRef.current !== seq) return;
        clearingVaultsRef.current = false;
        setClearingVaults(false);
        restoreRetiredFocus.current = true;
        setConfirmAction(null);
        if (noticeSeqRef.current === noticeSeq) fail(e);
      },
    );
  };

  return (
    <ModalSection title={t("settings.support")} compact className="gap-1 py-1.5">
      {confirmAction === "reset" ? (
        <div className="flex flex-col gap-2.5">
          <p role="alert" className="text-graphite text-[0.875rem] leading-snug">
            {t("settings.resetConfirm")}
          </p>
          <div className="flex flex-wrap items-center justify-end gap-2">
            <button
              type="button"
              autoFocus
              disabled={resetting}
              className={BUTTON_SMALL_QUIET}
              onClick={() => {
                if (resettingRef.current) return;
                playSound("click");
                restoreResetFocus.current = true;
                setConfirmAction(null);
              }}
            >
              {t("settings.resetKeep")}
            </button>
            <button
              type="button"
              disabled={resetting}
              className={BUTTON_SMALL_DANGER}
              onClick={reset}
            >
              {t("settings.resetDo")}
            </button>
          </div>
        </div>
      ) : confirmAction === "retired-vaults" ? (
        <div className="flex flex-col gap-2.5">
          <p role="alert" className="text-graphite text-[0.875rem] leading-snug">
            {t("settings.clearRetiredVaultsConfirm", { count: retiredCount })}
          </p>
          <div className="flex flex-wrap items-center justify-end gap-2">
            <button
              type="button"
              autoFocus
              disabled={clearingVaults}
              className={BUTTON_SMALL_QUIET}
              onClick={() => {
                if (clearingVaultsRef.current) return;
                playSound("click");
                restoreRetiredFocus.current = true;
                setConfirmAction(null);
              }}
            >
              {t("settings.clearRetiredVaultsKeep")}
            </button>
            <button
              type="button"
              disabled={clearingVaults}
              className={BUTTON_SMALL_DANGER}
              onClick={clearRetiredVaults}
            >
              {clearingVaults
                ? t("settings.clearingRetiredVaults")
                : t("settings.clearRetiredVaultsDo")}
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="flex flex-wrap gap-1.5">
            <button
              ref={openFolderRef}
              type="button"
              disabled={disabled}
              className={cn(BUTTON_SMALL_SOFT, "px-2.5")}
              onClick={() => run("openFolder", () => support.openCladeFolder())}
            >
              {t("settings.openFolder")}
            </button>
            {isAdvanced && (
              <>
                <button
                  type="button"
                  disabled={disabled}
                  className={cn(BUTTON_SMALL_SOFT, "px-2.5")}
                  onClick={() => run("openLogs", () => support.openLogsFolder())}
                >
                  {t("common.openLogs")}
                </button>
                <button
                  ref={exportDiagnosticsRef}
                  type="button"
                  className={cn(BUTTON_SMALL_SOFT, "px-2.5")}
                  disabled={disabled || exporting}
                  onClick={exportDiagnostics}
                >
                  {exporting ? t("settings.exporting") : t("settings.exportDiagnostics")}
                </button>
              </>
            )}
            <button
              ref={resetTriggerRef}
              type="button"
              disabled={disabled}
              className={cn(BUTTON_SMALL_QUIET, "px-2.5")}
              onClick={() => {
                playSound("click");
                noticeSeqRef.current += 1;
                setNotice(null);
                setConfirmAction("reset");
              }}
            >
              {t("settings.reset")}
            </button>
            {retiredCount > 0 && support.clearRetiredVaults && (
              <button
                ref={retiredTriggerRef}
                type="button"
                disabled={disabled}
                className={cn(BUTTON_SMALL_QUIET, "px-2.5")}
                onClick={() => {
                  playSound("click");
                  noticeSeqRef.current += 1;
                  setNotice(null);
                  setConfirmAction("retired-vaults");
                }}
              >
                {t("settings.clearRetiredVaults", { count: retiredCount })}
              </button>
            )}
          </div>
          {isAdvanced && (
            <p className="text-ink-muted text-[0.8125rem]">{t("settings.diagnosticsNote")}</p>
          )}
        </>
      )}
      <p
        role="status"
        aria-live="polite"
        aria-atomic="true"
        className={
          notice?.kind === "ok" ? "text-ink-muted text-[0.8125rem]" : "sr-only fixed top-0 left-0"
        }
      >
        {notice?.kind === "ok" ? notice.text : ""}
      </p>
      {notice?.kind === "error" && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {notice.text}
        </p>
      )}
    </ModalSection>
  );
}
