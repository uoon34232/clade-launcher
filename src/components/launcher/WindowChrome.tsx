import { useEffect, useRef, useState } from "react";
import { MinusIcon, XIcon } from "lucide-react";
import type { Window } from "@tauri-apps/api/window";
import { useT } from "@/lib/i18n";
import { TitleWordmark } from "./TitleWordmark";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Frameless window: a drag strip with the quiet product name centred in it, plus the window controls.
// Renders nothing in a plain browser.
export function WindowChrome() {
  const t = useT();
  const [win, setWin] = useState<Window | null>(null);
  const strip = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!inTauri) return;
    void import("@tauri-apps/api/window").then((m) => setWin(m.getCurrentWindow()));
  }, []);

  // Clade is never maximized. Tauri's injected drag script turns a double click on the strip into a
  // toggle-maximize command, which the window refuses (the capability denies it); swallow the second
  // press before it gets there so nothing is invoked and nothing is rejected. Dragging (the first
  // press) is untouched.
  useEffect(() => {
    const el = strip.current;
    if (!el) return;
    const ignoreDoubleClick = (e: MouseEvent) => {
      if (e.button === 0 && e.detail >= 2) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    el.addEventListener("mousedown", ignoreDoubleClick);
    return () => el.removeEventListener("mousedown", ignoreDoubleClick);
  }, []);

  if (!inTauri) return null;

  const btn =
    "window-btn grid h-8 w-10 cursor-default place-items-center rounded-lg text-graphite opacity-30 transition-[opacity,background-color] duration-150 outline-none hover:bg-graphite/[0.07] hover:opacity-100 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-clay active:bg-graphite/[0.12]";

  return (
    <div
      ref={strip}
      data-tauri-drag-region
      className="absolute inset-x-0 top-0 z-20 flex h-11 justify-end px-2 pt-1.5"
    >
      {/* The wordmark is a button (not a drag region); the empty strip around it still drags the window. */}
      <TitleWordmark />
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
