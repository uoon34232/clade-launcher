import { useEffect, useState } from "react";
import { MinusIcon, XIcon } from "lucide-react";
import type { Window } from "@tauri-apps/api/window";
import { useT } from "@/lib/i18n";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Frameless window: a drag strip with the quiet product name centred in it, plus the window controls.
// Renders nothing in a plain browser.
export function WindowChrome() {
  const t = useT();
  const [win, setWin] = useState<Window | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    void import("@tauri-apps/api/window").then((m) => setWin(m.getCurrentWindow()));
  }, []);

  if (!inTauri) return null;

  const btn =
    "grid h-8 w-10 cursor-default place-items-center rounded-lg text-graphite/45 transition-colors duration-150 outline-none hover:bg-graphite/[0.07] hover:text-graphite focus-visible:ring-2 focus-visible:ring-clay";

  return (
    <div
      data-tauri-drag-region
      className="absolute inset-x-0 top-0 z-20 flex h-11 justify-end px-2 pt-1.5"
    >
      {/* Decorative and click-through, so the whole strip still drags the window. The window itself is titled "Clade Launcher". */}
      <span
        aria-hidden
        className="text-graphite/50 pointer-events-none absolute inset-x-0 top-1.5 flex h-8 items-center justify-center font-serif text-[0.9375rem] leading-none font-normal tracking-[0.04em] select-none"
        style={{ fontVariationSettings: '"opsz" 24, "SOFT" 60' }}
      >
        Clade
      </span>
      <button
        type="button"
        aria-label={t("window.minimize")}
        className={btn}
        onClick={() => void win?.minimize()}
      >
        <MinusIcon aria-hidden className="size-4" />
      </button>
      <button
        type="button"
        aria-label={t("window.close")}
        className={btn}
        onClick={() => void win?.close()}
      >
        <XIcon aria-hidden className="size-4" />
      </button>
    </div>
  );
}
