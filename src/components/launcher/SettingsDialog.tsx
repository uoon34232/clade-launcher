import { useEffect, useState } from "react";
import { formatGigabytes } from "@/features/launcher/profileUi";
import { COPYRIGHT_LINE, versionLabel } from "@/lib/appInfo";
import { LANGUAGE_NAMES, LOCALES, useLocale, useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { chooseLanguage } from "@/lib/i18n/language";
import {
  resetUiSettings,
  saveBehavior,
  saveMemory,
  saveReducedMotion,
  saveTransparency,
  type MotionMode,
  type OnExit,
  type OnLaunch,
  type SaveResult,
  type TransparencyMode,
} from "@/lib/settings/uiSettings";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound, sound } from "@/lib/sound/soundService";
import { supportApi, type SupportApi } from "@/lib/support/api";
import { CatEgg } from "./CatEgg";
import { Modal, ModalHeader, ModalSection } from "./Modal";
import {
  BUTTON_PRIMARY,
  BUTTON_QUIET,
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

/**
 * Launcher settings: global launcher behaviour only, and intentionally short. Anything that belongs
 * to one profile lives in that profile's settings; anything about a player lives in the account's.
 * A new option needs a real reason; a safe automatic default beats another switch.
 */
export function SettingsDialog({ open, onOpenChange, support = supportApi }: Props) {
  const t = useT();
  // A pending "Reset settings?" confirmation must not be dismissed by a stray click or Escape.
  const [confirming, setConfirming] = useState(false);
  const close = (next: boolean) => {
    if (!next) setConfirming(false);
    onOpenChange(next);
  };

  return (
    <Modal open={open} onOpenChange={close} size="xl" locked={confirming}>
      <div className="-mb-4">
        <ModalHeader title={t("settings.title")} />
      </div>
      <SettingsBody
        support={support}
        confirming={confirming}
        setConfirming={setConfirming}
        onDone={() => close(false)}
      />
    </Modal>
  );
}

function SettingsBody({
  support,
  confirming,
  setConfirming,
  onDone,
}: {
  support: SupportApi;
  confirming: boolean;
  setConfirming: (v: boolean) => void;
  onDone: () => void;
}) {
  const t = useT();
  const s = useUiSettings();

  return (
    <>
      <div className="grid gap-x-8 sm:grid-cols-2">
        <div className="flex flex-col">
          <InterfaceSection />
          <MinecraftSection />
        </div>
        <div className="flex flex-col">
          <BehaviorSection onLaunch={s.onLaunch} onExit={s.onExit} />
          <SupportSection support={support} confirming={confirming} setConfirming={setConfirming} />
        </div>
      </div>
      <div className="border-graphite/10 mt-1 flex items-center justify-between gap-4 border-t pt-2">
        <div className="flex items-center gap-4">
          <p className="text-ink-muted/75 text-[0.75rem] leading-snug">
            <span className="block">Clade</span>
            <span className="block">{COPYRIGHT_LINE}</span>
            <span className="block">{versionLabel(s.version)}</span>
          </p>
          <CatEgg />
        </div>
        <button
          type="button"
          className={BUTTON_SOFT}
          disabled={confirming}
          onClick={() => {
            playSound("click");
            onDone();
          }}
        >
          {t("common.done")}
        </button>
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
    <div className={stacked ? "flex flex-col gap-1.5" : "flex items-center justify-between gap-4"}>
      <span id={id} className="text-graphite text-[0.9375rem]">
        {label}
      </span>
      {children}
    </div>
  );
}

// ------------------------------------------------------------------------------ Interface

function InterfaceSection() {
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
          aria-checked={s.enabled}
          aria-labelledby="sound-effects-label"
          onClick={() => {
            sound.setSettings({ enabled: !s.enabled });
            if (!s.enabled) playSound("select"); // a tick to confirm it is back on
          }}
          className={`dock-item focus-visible:ring-clay/50 focus-visible:ring-offset-card relative h-7 w-12 shrink-0 cursor-default rounded-full transition-colors duration-200 outline-none focus-visible:ring-2 focus-visible:ring-offset-2 ${s.enabled ? "bg-clay" : "bg-sand"}`}
        >
          <span
            aria-hidden
            className={`ease-out-soft absolute top-1 left-1 size-5 rounded-full bg-[#fbf6ee] shadow-sm transition-transform duration-200 ${s.enabled ? "translate-x-5" : ""}`}
          />
        </button>
      </Row>
      <div
        className={`flex items-center gap-4 transition-opacity duration-200 ${s.enabled ? "" : "opacity-45"}`}
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
          disabled={!s.enabled}
          value={Math.round(s.volume * 100)}
          onChange={(e) => sound.setSettings({ volume: Number(e.target.value) / 100 })}
          onPointerUp={() => playSound("select")}
          onKeyUp={() => playSound("select")}
          aria-valuetext={`${Math.round(s.volume * 100)}%`}
          className="accent-clay focus-visible:ring-clay/45 ml-auto h-6 w-36 cursor-default outline-none focus-visible:ring-2 disabled:cursor-not-allowed"
        />
      </div>

      <Row id="motion-label" label={t("settings.motion")} stacked>
        <Segmented<MotionMode>
          compact
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
    </ModalSection>
  );
}

// ------------------------------------------------------------------------------ Minecraft

function MinecraftSection() {
  const t = useT();
  const ui = useUiSettings();
  const { min, max, auto } = ui.memory;
  const custom = ui.memoryMb !== null;
  const [draft, setDraft] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const value = draft ?? ui.memoryMb ?? auto;

  const report = (r: SaveResult) => setError(r.ok ? null : describeError("settings", r.error));
  const commit = (mb: number) => {
    setDraft(null);
    if (mb !== ui.memoryMb) void saveMemory(mb).then(report);
  };

  return (
    <ModalSection title={t("settings.minecraft")} compact className="gap-1.5 py-2">
      <Row id="memory-label" label={t("settings.memory")} stacked>
        <Segmented<"auto" | "custom">
          compact
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
            value={value}
            onChange={(e) => setDraft(Number(e.target.value))}
            onPointerUp={() => commit(value)}
            onKeyUp={() => commit(value)}
            onBlur={() => commit(value)}
            aria-label={t("settings.memory.slider")}
            aria-valuetext={formatGigabytes(value)}
            className="accent-clay focus-visible:ring-clay/45 h-6 min-w-0 flex-1 cursor-default outline-none focus-visible:ring-2"
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

function BehaviorSection({ onLaunch, onExit }: { onLaunch: OnLaunch; onExit: OnExit }) {
  const t = useT();
  return (
    <ModalSection title={t("settings.behavior")} compact>
      <Row id="on-launch-label" label={t("settings.onLaunch")} stacked>
        <Segmented<OnLaunch>
          compact
          label={t("settings.onLaunch")}
          value={onLaunch}
          onChange={(v) => {
            playSound("select");
            void saveBehavior({ onLaunch: v });
          }}
          options={[
            { value: "keep_open", label: t("settings.onLaunch.keep") },
            { value: "hide", label: t("settings.onLaunch.hide") },
          ]}
        />
      </Row>
      <Row id="on-exit-label" label={t("settings.onExit")} stacked>
        <Segmented<OnExit>
          compact
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
  support,
  confirming,
  setConfirming,
}: {
  support: SupportApi;
  confirming: boolean;
  setConfirming: (v: boolean) => void;
}) {
  const t = useT();
  const [notice, setNotice] = useState<Notice>(null);
  const [exporting, setExporting] = useState(false);

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
      setConfirming(false);
      if (!r.ok) fail(r.error);
    });
  };

  return (
    <ModalSection title={t("settings.support")} compact>
      {confirming ? (
        <div className="flex flex-col gap-3">
          <p role="alert" className="text-graphite text-[0.9375rem]">
            {t("settings.resetConfirm")}
          </p>
          <div className="flex items-center justify-end gap-3">
            <button
              type="button"
              autoFocus
              className={BUTTON_QUIET}
              onClick={() => {
                playSound("click");
                setConfirming(false);
              }}
            >
              {t("settings.resetKeep")}
            </button>
            <button type="button" className={BUTTON_PRIMARY} onClick={reset}>
              {t("settings.resetDo")}
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-2">
            <button
              type="button"
              className={BUTTON_SMALL_SOFT}
              onClick={() => run(() => support.openCladeFolder())}
            >
              {t("settings.openFolder")}
            </button>
            <button
              type="button"
              className={BUTTON_SMALL_SOFT}
              onClick={() => run(() => support.openLogsFolder())}
            >
              {t("common.openLogs")}
            </button>
            <button
              type="button"
              className={BUTTON_SMALL_SOFT}
              disabled={exporting}
              onClick={exportDiagnostics}
            >
              {exporting ? t("settings.exporting") : t("settings.exportDiagnostics")}
            </button>
            <button
              type="button"
              className={BUTTON_SMALL_QUIET}
              onClick={() => {
                playSound("click");
                setNotice(null);
                setConfirming(true);
              }}
            >
              {t("settings.reset")}
            </button>
          </div>
          <p className="text-ink-muted text-[0.8125rem]">{t("settings.diagnosticsNote")}</p>
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
