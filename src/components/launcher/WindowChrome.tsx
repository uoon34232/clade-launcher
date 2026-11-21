import { useEffect, useRef, useState } from "react";
import { Maximize2Icon, Minimize2Icon, MinusIcon, XIcon } from "lucide-react";
import type { Window } from "@tauri-apps/api/window";
import { useT } from "@/lib/i18n";
import { inTauri } from "@/lib/settings/uiSettings";
import { TitleWordmark } from "./TitleWordmark";

interface Props {
  framed?: boolean;
  closing?: boolean;
  maximized?: boolean;
  onToggleMaximize?: () => void;
}

// Frameless window: a drag strip with the quiet product name centred in it, plus the window controls.
// Renders nothing in a plain browser. Maximize/Restore is available only while a game is framed
// inside Clade (`framed = true`).
export function WindowChrome({
  framed = false,
  closing = false,
  maximized = false,
  onToggleMaximize,
}: Props) {
  const t = useT();
  const [win, setWin] = useState<Window | null>(null);
  const strip = useRef<HTMLDivElement>(null);
  const tauri = inTauri();

  useEffect(() => {
    if (!tauri) return;
    void import("@tauri-apps/api/window")
      .then((m) => setWin(m.getCurrentWindow()))
      .catch(() => undefined);
  }, [tauri]);

  // Tauri's injected drag script turns a double click on the strip into the denied JS toggle-maximize
  // capability. Swallow the second press before it gets there so nothing is invoked and nothing is
  // rejected; when a framed game is open, trigger the Rust-gated toggle instead. Dragging (the first
  // press) is untouched.
  useEffect(() => {
    const el = strip.current;
    if (!el) return;
    const handleDoubleClick = (e: MouseEvent) => {
      if (e.button === 0 && e.detail >= 2) {
        e.preventDefault();
        e.stopPropagation();
        if (framed && !(e.target as Element | null)?.closest("button")) {
          onToggleMaximize?.();
        }
      }
    };
    el.addEventListener("mousedown", handleDoubleClick);
    return () => el.removeEventListener("mousedown", handleDoubleClick);
  }, [tauri, framed, onToggleMaximize]);

  if (!tauri) return null;

  const withWindow = (action: (w: Window) => Promise<void>) => {
    void (
      win
        ? Promise.resolve(win)
        : import("@tauri-apps/api/window").then((m) => m.getCurrentWindow())
    )
      .then(action)
      .catch(() => undefined);
  };

  const btn = `window-btn grid h-8 w-10 cursor-default place-items-center rounded-lg text-graphite ${
    framed ? "opacity-65" : "opacity-60"
  } transition-[opacity,background-color] duration-150 outline-none hover:bg-graphite/[0.07] hover:opacity-100 active:bg-graphite/[0.12]`;

  const blurIfPointerOrFramed = (el: HTMLButtonElement, detail: number) => {
    if (framed || detail > 0) {
      el.blur();
    }
  };

  return (
    <div
      ref={strip}
      {...(!maximized ? { "data-tauri-drag-region": true } : {})}
      className={`absolute inset-x-0 top-0 z-20 flex justify-end border-b px-2 transition-[border-color] duration-300 ${
        framed ? "border-graphite/10" : "border-transparent"
      } ${framed ? "h-[44px] items-center" : "h-11 pt-1.5"}`}
    >
      {/* The wordmark is a button (not a drag region); the empty strip around it still drags the window. */}
      <TitleWordmark />
      <button
        type="button"
        aria-label={t("window.minimize")}
        className={btn}
        onClick={(e) => {
          blurIfPointerOrFramed(e.currentTarget, e.detail);
          withWindow((w) => w.minimize());
        }}
      >
        <MinusIcon aria-hidden className="size-4" />
      </button>
      {framed && (
        <button
          type="button"
          aria-label={maximized ? t("window.restore") : t("window.maximize")}
          className={`${btn} chrome-btn-enter`}
          onClick={(e) => {
            blurIfPointerOrFramed(e.currentTarget, e.detail);
            onToggleMaximize?.();
          }}
        >
          {maximized ? (
            <Minimize2Icon aria-hidden className="size-3.5" />
          ) : (
            <Maximize2Icon aria-hidden className="size-3.5" />
          )}
        </button>
      )}
      <button
        type="button"
        aria-label={t("window.close")}
        className={`${btn} ${closing ? "chrome-closing-pulse" : ""}`}
        onClick={(e) => {
          blurIfPointerOrFramed(e.currentTarget, e.detail);
          withWindow((w) => w.close());
        }}
      >
        <XIcon aria-hidden className="size-4" />
      </button>
    </div>
  );
}
