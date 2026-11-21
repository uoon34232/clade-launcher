import { useEffect, useRef, useState } from "react";
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
  saveTransparency,
  saveUiMode,
  type MotionMode,
  type OnExit,
  type OnLaunch,
  type SaveResult,
  type TransparencyMode,
  type UiMode,
} from "@/lib/settings/uiSettings";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound, sound } from "@/lib/sound/soundService";
import { supportApi, type SupportApi, type UpdateProgress } from "@/lib/support/api";
import { CatEgg } from "./CatEgg";
import { Modal, ModalHeader, ModalSection } from "./Modal";
import {
  BUTTON_PRIMARY,
  BUTTON_QUIET,
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
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (!open) {
      setConfirmAction(null);
      setUpdateState((prev) =>
        prev.kind === "ready" ? { kind: "ready", version: prev.version } : { kind: "idle" },
      );
    }
  }
  const updateBusy = updateState.kind === "working" || updateState.kind === "applying";
  const locked = confirmAction !== null || updateBusy;
  const close = (next: boolean) => {
    if (!next) {
      if (locked) return;
      setConfirmAction(null);
      if (updateState.kind !== "ready") {
        setUpdateState({ kind: "idle" });
      }
    }
    onOpenChange(next);
  };

  return (
    <Modal open={open} onOpenChange={close} size="xl" locked={locked}>
      <div className="mb-1 flex flex-wrap items-center justify-between gap-4">
        <div className="-mb-5">
          <ModalHeader title={t("settings.title")} />
        </div>
        <Segmented<UiMode>
          compact
          disabled={locked}
          label={t("settings.mode")}
          value={ui.uiMode}
          onChange={(mode) => {
            playSound("select");
            void saveUiMode(mode);
          }}
          options={[
            { value: "simple", label: t("settings.mode.simple") },
            { value: "advanced", label: t("settings.mode.advanced") },
          ]}
        />
      </div>
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
    if (!open) setCancellingUpdate(false);
  }
  const openRef = useRef(open);
  const checkSeqRef = useRef(0);
  const installActiveRef = useRef(false);
  const updateBusy = updateState.kind === "working" || updateState.kind === "applying";
  const controlsLocked = confirmAction !== null || updateBusy;

  useEffect(() => {
    openRef.current = open;
    if (!open) {
      checkSeqRef.current += 1;
      installActiveRef.current = false;
      return;
    }
    let cancelled = false;
    if (support.getUpdateStatus) {
      support
        .getUpdateStatus()
        .then((p) => {
          if (cancelled) return;
          if (p.stage === "downloading" || p.stage === "verifying" || p.stage === "extracting") {
            installActiveRef.current = true;
          }
          if (p.stage !== "idle") {
            setUpdateState((prev) => applyUpdateProgress(prev, p, true));
          }
        })
        .catch(() => undefined);
    }
    let unlisten: (() => void) | null = null;
    if (support.onUpdateProgress) {
      void support
        .onUpdateProgress((p) => {
          if (!cancelled) {
            setUpdateState((prev) => applyUpdateProgress(prev, p, installActiveRef.current));
          }
        })
        .then((stop) => {
          if (cancelled) stop();
          else unlisten = stop;
        })
        .catch(() => undefined);
    }
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [open, support, setUpdateState]);

  const checkUpdates = () => {
    if (!support.checkForUpdates || updateState.kind === "checking" || controlsLocked) {
      return;
    }
    const seq = ++checkSeqRef.current;
    playSound("click");
    setUpdateState({ kind: "checking" });
    support.checkForUpdates().then(
      (res) => {
        if (!openRef.current || checkSeqRef.current !== seq) return;
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
        if (!openRef.current || checkSeqRef.current !== seq) return;
        setUpdateState({ kind: "error", error: e });
      },
    );
  };

  const openRelease = (tag: string) => {
    if (!support.openReleasePage || controlsLocked) return;
    const seq = ++checkSeqRef.current;
    playSound("click");
    support.openReleasePage(tag).catch((e: unknown) => {
      if (!openRef.current || checkSeqRef.current !== seq) return;
      setUpdateState({ kind: "error", error: e });
    });
  };

  const installUpdate = () => {
    if (updateState.kind !== "available" || !support.installUpdate || controlsLocked) return;
    const { latestVersion, canInstall, assetSize } = updateState;
    playSound("click");
    setCancellingUpdate(false);
    installActiveRef.current = true;
    setUpdateState({
      kind: "working",
      stage: "downloading",
      version: latestVersion,
      bytesDone: 0,
      bytesTotal: assetSize ?? 0,
    });
    support.installUpdate(latestVersion).then(
      (res) => {
        installActiveRef.current = false;
        setCancellingUpdate(false);
        if (res.stage === "ready") {
          setUpdateState({ kind: "ready", version: res.version ?? latestVersion });
        } else {
          setUpdateState({ kind: "available", latestVersion, canInstall, assetSize });
        }
      },
      (e: unknown) => {
        installActiveRef.current = false;
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
    if (updateState.kind !== "working" || !support.cancelUpdate || cancellingUpdate) return;
    playSound("click");
    setCancellingUpdate(true);
    void support.cancelUpdate().catch(() => {
      setCancellingUpdate(false);
    });
  };

  const restartToUpdate = () => {
    if (updateState.kind !== "ready" || !support.applyStagedUpdate || confirmAction !== null) {
      return;
    }
    const { version } = updateState;
    playSound("click");
    setUpdateState({ kind: "applying", version });
    support.applyStagedUpdate().catch((e: unknown) => {
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

  return (
    <>
      <div className="grid gap-x-6 sm:grid-cols-2 lg:gap-x-8">
        <div className="flex min-w-0 flex-col">
          <InterfaceSection isAdvanced={isAdvanced} disabled={controlsLocked} />
          {isAdvanced && <MinecraftSection disabled={controlsLocked} />}
        </div>
        <div className="flex min-w-0 flex-col">
          {isAdvanced && (
            <BehaviorSection onLaunch={s.onLaunch} onExit={s.onExit} disabled={controlsLocked} />
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
      <div className="border-graphite/10 mt-1 flex items-center justify-between gap-3 border-t pt-2">
        <div className="flex shrink-0 items-center gap-4">
          <p className="text-ink-muted/75 text-[0.75rem] leading-snug">
            <span className="block">Clade</span>
            <span className="block">{COPYRIGHT_LINE}</span>
            <span className="block">{versionLabel(s.version)}</span>
          </p>
          <CatEgg />
        </div>
        <div className="flex min-w-0 flex-1 flex-wrap items-center justify-end gap-2">
          {(support.checkForUpdates || updateState.kind !== "idle") && (
            <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
              {updateState.kind === "up_to_date" && (
                <span role="status" className="text-ink-muted truncate text-[0.8125rem]">
                  {t("settings.updatesUpToDate")}
                </span>
              )}
              {updateState.kind === "available" && (
                <span
                  role="status"
                  className="text-clay-deep truncate text-[0.8125rem] font-medium"
                >
                  {t("settings.updateAvailable", { version: updateState.latestVersion })}
                </span>
              )}
              {updateState.kind === "working" && (
                <>
                  <span role="status" className="text-ink-muted text-[0.8125rem] tabular-nums">
                    {updateState.stage === "downloading"
                      ? downloadPercent !== null
                        ? t("settings.updatingDownloading", { percent: downloadPercent })
                        : t("settings.updatingDownloadingUnknown")
                      : updateState.stage === "verifying"
                        ? t("settings.updatingVerifying")
                        : t("settings.updatingExtracting")}
                  </span>
                  {updateState.stage === "downloading" && (
                    <div
                      role="progressbar"
                      aria-label={t("settings.updateProgress")}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={downloadPercent ?? undefined}
                      className="bg-graphite/12 h-1.5 w-14 shrink-0 overflow-hidden rounded-full"
                    >
                      {downloadPercent !== null && (
                        <div
                          className="bg-clay h-full rounded-full transition-[width] duration-150"
                          style={{ width: `${downloadPercent}%` }}
                        />
                      )}
                    </div>
                  )}
                </>
              )}
              {updateState.kind === "ready" && !errorText && (
                <span
                  role="status"
                  className="text-clay-deep truncate text-[0.8125rem] font-medium"
                >
                  {t("settings.updateReady")}
                </span>
              )}
              {updateState.kind === "applying" && (
                <span role="status" className="text-ink-muted truncate text-[0.8125rem]">
                  {t("settings.updatingRestarting")}
                </span>
              )}
              {errorText && (
                <span
                  role="alert"
                  title={errorText}
                  className="text-clay-deep max-w-[18rem] truncate text-[0.8125rem]"
                >
                  {errorText}
                </span>
              )}
              {updateState.kind === "available" ? (
                <>
                  {updateState.canInstall && support.installUpdate && (
                    <button
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
          )}
          <button
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
          : "flex min-w-0 items-center justify-between gap-4"
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

function InterfaceSection({ isAdvanced, disabled }: { isAdvanced: boolean; disabled: boolean }) {
  const t = useT();
  const locale = useLocale();
  const ui = useUiSettings();
  const [s, setS] = useState(() => sound.getSettings());
  useEffect(() => sound.subscribe(setS), []);

  return (
    <ModalSection title={t("settings.interface")} compact className="gap-1.5 py-2">
      <Row id="language-label" label={t("settings.language")}>
        <Segmented
          compact
          disabled={disabled}
          label={t("settings.language")}
          value={locale}
          onChange={(next) => {
            playSound("select");
            void chooseLanguage(next);
          }}
          options={LOCALES.map((l) => ({ value: l, label: LANGUAGE_NAMES[l] }))}
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
          className={`dock-item focus-visible:ring-clay/50 focus-visible:ring-offset-card relative h-7 w-12 shrink-0 cursor-default rounded-full transition-colors duration-200 outline-none focus-visible:ring-2 focus-visible:ring-offset-2 disabled:opacity-50 ${s.enabled ? "bg-clay" : "bg-sand"}`}
        >
          <span
            aria-hidden
            className={`ease-out-soft absolute top-1 left-1 size-5 rounded-full bg-[#fbf6ee] shadow-sm transition-transform duration-200 ${s.enabled ? "translate-x-5" : ""}`}
          />
        </button>
      </Row>
      <div
        className={`flex items-center gap-4 transition-opacity duration-200 ${s.enabled && !disabled ? "" : "opacity-45"}`}
      >
        <label htmlFor="ui-volume" className="text-graphite text-[0.9375rem]">
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
          onKeyUp={() => playSound("select")}
          aria-valuetext={`${Math.round(s.volume * 100)}%`}
          className="accent-clay focus-visible:ring-clay/45 ml-auto h-6 w-36 cursor-default outline-none focus-visible:ring-2 disabled:cursor-not-allowed"
        />
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
                playSound("select");
                void saveReducedMotion(mode);
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
                playSound("select");
                void saveTransparency(mode);
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
    </ModalSection>
  );
}

// ------------------------------------------------------------------------------ Minecraft

function MinecraftSection({ disabled }: { disabled: boolean }) {
  const t = useT();
  const ui = useUiSettings();
  const { min, max, auto } = ui.memory;
  const custom = ui.memoryMb !== null;
  const [draft, setDraft] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const value = Math.min(max, Math.max(min, draft ?? ui.memoryMb ?? auto));

  const report = (r: SaveResult) => setError(r.ok ? null : describeError("settings", r.error));
  const commit = (mb: number) => {
    if (mb === ui.memoryMb) {
      setDraft(null);
      return;
    }
    void saveMemory(mb)
      .then(report)
      .finally(() => setDraft(null));
  };

  return (
    <ModalSection title={t("settings.minecraft")} compact className="gap-1.5 py-2">
      <Row id="memory-label" label={t("settings.memory")} stacked>
        <Segmented<"auto" | "custom">
          compact
          disabled={disabled}
          label={t("settings.memory")}
          value={custom ? "custom" : "auto"}
          onChange={(mode) => {
            playSound("select");
            setDraft(null);
            void (mode === "auto" ? saveMemory(null) : saveMemory(auto)).then(report);
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
            onChange={(e) => setDraft(Number(e.target.value))}
            onPointerUp={() => commit(value)}
            onKeyUp={() => commit(value)}
            onBlur={() => commit(value)}
            aria-label={t("settings.memory.slider")}
            aria-valuetext={formatGigabytes(value)}
            className="accent-clay focus-visible:ring-clay/45 h-6 min-w-0 flex-1 cursor-default outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-50"
          />
          <span aria-hidden className="text-graphite w-14 text-right text-[0.875rem] tabular-nums">
            {formatGigabytes(value)}
          </span>
        </div>
      ) : (
        <p className="text-ink-muted text-[0.8125rem]">
          {t("settings.memory.autoNote", { value: formatGigabytes(auto) })}
        </p>
      )}
      {error && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {error}
        </p>
      )}
    </ModalSection>
  );
}

// ------------------------------------------------------------------------------- Behavior

function BehaviorSection({
  onLaunch,
  onExit,
  disabled,
}: {
  onLaunch: OnLaunch;
  onExit: OnExit;
  disabled: boolean;
}) {
  const t = useT();
  return (
    <ModalSection title={t("settings.behavior")} compact>
      <Row id="on-launch-label" label={t("settings.onLaunch")} stacked>
        <Segmented<OnLaunch>
          compact
          disabled={disabled}
          label={t("settings.onLaunch")}
          value={onLaunch}
          onChange={(v) => {
            playSound("select");
            void saveBehavior({ onLaunch: v });
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
            playSound("select");
            void saveBehavior({ onExit: v });
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
  const [retiredCount, setRetiredCount] = useState(0);
  const [clearingVaults, setClearingVaults] = useState(false);
  const resetTriggerRef = useRef<HTMLButtonElement>(null);
  const restoreResetFocus = useRef(false);
  const retiredTriggerRef = useRef<HTMLButtonElement>(null);
  const restoreRetiredFocus = useRef(false);

  useEffect(() => {
    if (!open || !support.retiredVaultsCount) return;
    let cancelled = false;
    support.retiredVaultsCount().then(
      (n) => {
        if (!cancelled) setRetiredCount(n);
      },
      () => {
        if (!cancelled) setRetiredCount(0);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, support]);

  useEffect(() => {
    if (confirmAction !== null) return;
    if (restoreResetFocus.current) {
      restoreResetFocus.current = false;
      resetTriggerRef.current?.focus();
    }
    if (restoreRetiredFocus.current) {
      restoreRetiredFocus.current = false;
      retiredTriggerRef.current?.focus();
    }
  }, [confirmAction]);

  const fail = (e: unknown) => setNotice({ kind: "error", text: describeError("settings", e) });
  const run = (action: () => Promise<void>) => {
    playSound("click");
    setNotice(null);
    action().catch(fail);
  };

  const exportDiagnostics = () => {
    playSound("click");
    setNotice(null);
    setExporting(true);
    support.exportDiagnostics().then(
      (r) => {
        setExporting(false);
        setNotice({ kind: "ok", text: t("settings.diagnosticsDone", { name: r.name }) });
      },
      (e: unknown) => {
        setExporting(false);
        fail(e);
      },
    );
  };

  const reset = () => {
    void resetUiSettings().then((r) => {
      setConfirmAction(null);
      if (!r.ok) fail(r.error);
    });
  };

  const clearRetiredVaults = () => {
    if (!support.clearRetiredVaults || clearingVaults) return;
    playSound("click");
    setClearingVaults(true);
    support.clearRetiredVaults().then(
      (removed) => {
        setClearingVaults(false);
        const deleted = removed > 0 ? removed : retiredCount;
        setRetiredCount(0);
        setConfirmAction(null);
        setNotice({ kind: "ok", text: t("settings.retiredVaultsCleared", { count: deleted }) });
      },
      (e: unknown) => {
        setClearingVaults(false);
        setConfirmAction(null);
        fail(e);
      },
    );
  };

  return (
    <ModalSection title={t("settings.support")} compact>
      {confirmAction === "reset" ? (
        <div className="flex flex-col gap-3">
          <p role="alert" className="text-graphite text-[0.9375rem]">
            {t("settings.resetConfirm")}
          </p>
          <div className="flex flex-wrap items-center justify-end gap-3">
            <button
              type="button"
              autoFocus
              className={BUTTON_QUIET}
              onClick={() => {
                playSound("click");
                restoreResetFocus.current = true;
                setConfirmAction(null);
              }}
            >
              {t("settings.resetKeep")}
            </button>
            <button type="button" className={BUTTON_PRIMARY} onClick={reset}>
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
              className={BUTTON_SMALL_PRIMARY}
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
          <div className="grid grid-cols-2 gap-2">
            <button
              type="button"
              disabled={disabled}
              className={`${BUTTON_SMALL_SOFT} min-w-0 truncate px-2.5`}
              onClick={() => run(() => support.openCladeFolder())}
            >
              {t("settings.openFolder")}
            </button>
            {isAdvanced && (
              <>
                <button
                  type="button"
                  disabled={disabled}
                  className={`${BUTTON_SMALL_SOFT} min-w-0 truncate px-2.5`}
                  onClick={() => run(() => support.openLogsFolder())}
                >
                  {t("common.openLogs")}
                </button>
                <button
                  type="button"
                  className={`${BUTTON_SMALL_SOFT} min-w-0 truncate px-2.5`}
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
              className={`${BUTTON_SMALL_QUIET} min-w-0 truncate px-2.5`}
              onClick={() => {
                playSound("click");
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
                className={`${BUTTON_SMALL_QUIET} col-span-2 min-w-0 truncate px-2.5`}
                onClick={() => {
                  playSound("click");
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
      {notice && (
        <p
          role={notice.kind === "error" ? "alert" : "status"}
          className={`text-[0.8125rem] ${notice.kind === "error" ? "text-clay-deep" : "text-ink-muted"}`}
        >
          {notice.text}
        </p>
      )}
    </ModalSection>
  );
}
