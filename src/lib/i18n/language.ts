// Choosing a language: the UI switches at once, then the choice is saved by the Rust settings
// service (`data/settings.json`). If it cannot be saved the switch is undone, so what is on screen
// never disagrees with what the next start will show.

import { saveLanguage } from "@/lib/settings/uiSettings";
import { getLocale, setLocale, type Locale } from "./index";

export async function chooseLanguage(next: Locale): Promise<void> {
  const previous = getLocale();
  if (next === previous) return;
  setLocale(next);
  if (!(await saveLanguage(next)) && getLocale() === next) setLocale(previous);
}
