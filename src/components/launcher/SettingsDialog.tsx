import { useEffect, useState } from "react";
import { LANGUAGE_NAMES, LOCALES, useLocale, useT } from "@/lib/i18n";
import { chooseLanguage } from "@/lib/i18n/language";
import { playSound, sound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_SOFT, LABEL } from "./modalStyles";
import { Segmented } from "./Segmented";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Launcher settings: global launcher behaviour only, and intentionally tiny. Anything that belongs
 * to one profile lives in that profile's settings; anything about a player lives in the account's.
 * A new option needs a real reason; a safe automatic default beats another switch.
 */
export function SettingsDialog({ open, onOpenChange }: Props) {
  const t = useT();
  return (
    <Modal open={open} onOpenChange={onOpenChange} size="sm">
      <ModalHeader title={t("settings.title")} />
      <SoundSettings />
      <ModalFooter>
        <button type="button" className={BUTTON_SOFT} onClick={() => onOpenChange(false)}>
          {t("common.done")}
        </button>
      </ModalFooter>
    </Modal>
  );
}

function SoundSettings() {
  const t = useT();
  const locale = useLocale();
  const [s, setS] = useState(() => sound.getSettings());
  useEffect(() => sound.subscribe(setS), []);

  return (
    <div className="mb-6 flex flex-col gap-5">
      <div className="flex items-center justify-between gap-4">
        <span id="sound-effects-label" className="text-graphite text-[0.9375rem]">
          {t("settings.sound")}
        </span>
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
      </div>
      <div
        className={`flex flex-col gap-2 transition-opacity duration-200 ${s.enabled ? "" : "opacity-45"}`}
      >
        <label htmlFor="ui-volume" className={LABEL}>
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
          className="accent-clay focus-visible:ring-clay/45 h-6 w-full cursor-default outline-none focus-visible:ring-2 disabled:cursor-not-allowed"
        />
      </div>
      <div className="flex items-center justify-between gap-4">
        <span id="language-label" className="text-graphite text-[0.9375rem]">
          {t("settings.language")}
        </span>
        <Segmented
          label={t("settings.language")}
          value={locale}
          onChange={(next) => {
            playSound("select");
            void chooseLanguage(next);
          }}
          options={LOCALES.map((l) => ({ value: l, label: LANGUAGE_NAMES[l] }))}
        />
      </div>
    </div>
  );
}
