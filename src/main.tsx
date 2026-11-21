import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installInputModality } from "@/lib/inputModality";
import { resolveLocale, setLocale } from "@/lib/i18n";
import { loadUiSettings, onUiSettingsChanged, type UiSettings } from "@/lib/settings/uiSettings";
import { setMotionMode } from "@/lib/motion";
import { applyGlass, loadGlassStatus, onGlassStatusChanged } from "@/lib/glass";
import { sound } from "@/lib/sound/soundService";

const localeOf = (s: UiSettings) => resolveLocale(s.language, s.systemLocale);
const soundOf = (s: UiSettings) => ({ enabled: s.soundEnabled, volume: s.soundVolume });

async function main() {
  installInputModality();

  // Subscribe to settings and window-material events before reading the initial snapshots so a
  // backend change racing startup is never dropped.
  let sawGlassEvent = false;
  let sawUiEvent = false;
  let initialUi: UiSettings | null = null;
  await Promise.all([
    onGlassStatusChanged((s) => {
      sawGlassEvent = true;
      applyGlass(s);
    }),
    onUiSettingsChanged((s) => {
      sawUiEvent = true;
      initialUi = s;
      sound.hydrate(soundOf(s));
      setMotionMode(s.reducedMotion);
      setLocale(localeOf(s));
    }),
  ]);

  const [fetchedUi, glassStatus] = await Promise.all([loadUiSettings(), loadGlassStatus()]);
  if (!sawUiEvent) {
    sound.hydrate(soundOf(fetchedUi));
    setMotionMode(fetchedUi.reducedMotion);
    setLocale(localeOf(fetchedUi));
  }
  if (!sawGlassEvent) {
    applyGlass(glassStatus);
  }
  const ui = initialUi ?? fetchedUi;

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App initialUi={ui} />
    </React.StrictMode>,
  );
}

void main();
