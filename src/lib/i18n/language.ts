// Choosing a language: the UI switches at once, then the choice is saved by the Rust settings
// service (`data/settings.json`). If it cannot be saved the switch is undone, so what is on screen
// never disagrees with what the next start will show.

import { saveLanguage } from "@/lib/settings/uiSettings";
import { getLocale, setLocale, type Locale } from "./index";

let requestId = 0;
let inFlight = 0;
let confirmedLocale: Locale | null = null;
let confirmedToken = 0;

export async function chooseLanguage(next: Locale): Promise<void> {
  if (inFlight === 0 && next === getLocale()) return;

  const token = ++requestId;
  if (inFlight === 0) {
    confirmedLocale = getLocale();
    confirmedToken = 0;
  }
  inFlight += 1;
  setLocale(next);

  let ok = false;
  try {
    ok = await saveLanguage(next);
  } finally {
    inFlight -= 1;
    if (ok) {
      if (token >= confirmedToken) {
        confirmedToken = token;
        confirmedLocale = next;
        if (token === requestId || inFlight === 0) {
          setLocale(next);
        }
      }
    } else if (token === requestId && getLocale() === next && confirmedLocale !== null) {
      setLocale(confirmedLocale);
    }
    if (inFlight === 0) {
      confirmedLocale = null;
      confirmedToken = 0;
    }
  }
}
