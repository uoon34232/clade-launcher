import { useEffect, useRef, useState } from "react";
import { useT } from "@/lib/i18n";
import { prefersReducedMotion } from "@/lib/motion";

const LETTERS = ["C", "l", "a", "d", "e"];
const HOLD_MS = 1100;

type Mode = "full" | "reduced";

/**
 * The centred "Clade" in the title strip. At rest it is plain, quiet type. Hovering nudges each
 * letter up by a pixel; activating it (click, Enter or Space) lets the letters part a few pixels, a
 * thin clay line passes under the word and the brand phrase takes its place for about a second
 * before everything settles back. With reduced motion it is a plain crossfade. No sound, nothing
 * stored. It is a real button, so it is not part of the window's drag region.
 */
export function TitleWordmark() {
  const t = useT();
  const [on, setOn] = useState(false);
  const [mode, setMode] = useState<Mode>("full");
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const activate = () => {
    window.clearTimeout(timer.current);
    setMode(prefersReducedMotion() ? "reduced" : "full");
    setOn(true);
    timer.current = window.setTimeout(() => setOn(false), HOLD_MS);
  };

  return (
    <div className="pointer-events-none absolute inset-x-0 inset-y-0 flex items-center justify-center">
      <button
        type="button"
        aria-label={t("window.wordmark")}
        data-phase={on ? "on" : "off"}
        data-mode={mode}
        onClick={activate}
        className="wordmark text-graphite/80 pointer-events-auto relative -mx-2 cursor-default rounded-md px-2 py-0.5 font-serif text-[0.9375rem] leading-none font-normal tracking-[0.04em] whitespace-nowrap outline-hidden select-none"
        style={{ fontVariationSettings: '"opsz" 24, "SOFT" 60' }}
      >
        <span className="sr-only" aria-live="polite">
          {on ? t("onboarding.tagline") : ""}
        </span>
        <span aria-hidden className="wordmark-word wm-fade block">
          {LETTERS.map((letter, i) => (
            <span
              key={letter + String(i)}
              className="wordmark-letter inline-block"
              style={{ "--i": i, "--spread": `${(i - 2) * 2.5}px` } as React.CSSProperties}
            >
              {letter}
            </span>
          ))}
        </span>
        <span
          aria-hidden
          className="pointer-events-none absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 py-0.5 whitespace-nowrap"
        >
          <span className="wordmark-tagline wm-fade block">{t("onboarding.tagline")}</span>
          <span className="wordmark-line pointer-events-none absolute inset-x-0 bottom-0 w-full" />
        </span>
      </button>
    </div>
  );
}
