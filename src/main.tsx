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

  // Launcher preferences live in the portable settings file (data/settings.json), owned by the Rust
  // backend, and the window backdrop status comes from the same native layer. Fetch both in parallel
  // before the first render so startup only pays one IPC round-trip.
  const [ui, glassStatus] = await Promise.all([loadUiSettings(), loadGlassStatus()]);
  sound.hydrate(soundOf(ui));
  setMotionMode(ui.reducedMotion);
  // The window material is cosmetic: the page starts opaque and only switches once the backend says
  // the material is really on.
  applyGlass(glassStatus);
  void onGlassStatusChanged(applyGlass);
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
