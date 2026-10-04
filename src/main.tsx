import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installInputModality } from "@/lib/inputModality";
import { resolveLocale, setLocale } from "@/lib/i18n";
import { loadUiSettings, onUiSettingsChanged, type UiSettings } from "@/lib/settings/uiSettings";
import { setMotionMode } from "@/lib/motion";
import { sound } from "@/lib/sound/soundService";

const localeOf = (s: UiSettings) => resolveLocale(s.language, s.systemLocale);
const soundOf = (s: UiSettings) => ({ enabled: s.soundEnabled, volume: s.soundVolume });

async function main() {
  installInputModality();

  // Launcher preferences live in the portable settings file (data/settings.json), owned by the Rust
  // backend. Load them before the first render so the UI starts with the right values; a slow or
  // failing backend falls back to defaults rather than blocking the launcher.
  const ui = await loadUiSettings();
  sound.hydrate(soundOf(ui));
  setMotionMode(ui.reducedMotion);
  // An explicit language choice wins; before one is ever made the Windows language decides.
  setLocale(localeOf(ui));
  void onUiSettingsChanged((s) => {
    sound.hydrate(soundOf(s));
    setMotionMode(s.reducedMotion);
    setLocale(localeOf(s));
  });

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App initialUi={ui} />
    </React.StrictMode>,
  );
}

void main();
