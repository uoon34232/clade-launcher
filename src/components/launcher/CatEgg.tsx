import { useEffect, useRef } from "react";
import { useT } from "@/lib/i18n";
import { prefersReducedMotion } from "@/lib/motion";

/**
 * A tiny outline cat in the Settings footer. Click it (or press Enter or Space) and it reacts where
 * it sits: ears dip, eyes squint, the tail flicks once and the body settles back (about 650 ms,
 * see `.cat-*` in App.css). It never moves away or disappears, can be triggered again at any time
 * and plays no sound. With reduced motion there is no movement at all: the eyes just close for a
 * moment. Nothing is saved.
 */
const FULL_MS = 650;
const REDUCED_MS = 260;

export function CatEgg() {
  const t = useT();
  const ref = useRef<HTMLButtonElement>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const react = () => {
    const el = ref.current;
    if (!el) return;
    const reduced = prefersReducedMotion();
    const next = reduced ? "reduced" : "full";
    window.clearTimeout(timer.current);
    // Restart in-flight CSS animations via the Web Animations API without forcing a layout reflow.
    if (el.getAttribute("data-reaction") === next) {
      if (next === "full" && typeof el.getAnimations === "function") {
        for (const a of el.getAnimations({ subtree: true })) {
          if (typeof CSSAnimation === "undefined" || a instanceof CSSAnimation) {
            a.cancel();
            a.play();
          }
        }
      }
    } else {
      el.setAttribute("data-reaction", next);
    }
    timer.current = window.setTimeout(
      () => el.removeAttribute("data-reaction"),
      reduced ? REDUCED_MS : FULL_MS,
    );
  };

  return (
    <button
      ref={ref}
      type="button"
      aria-label={t("settings.cat")}
      onClick={react}
      className="cat text-graphite/40 hover:text-graphite/65 focus-visible:ring-clay/45 -m-1 grid size-9 shrink-0 cursor-default place-items-center rounded-lg transition-colors duration-200 outline-none focus-visible:ring-2"
    >
      <svg
        aria-hidden
        viewBox="0 0 32 32"
        className="cat-body size-6"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        {/* ears */}
        <path className="cat-ear cat-ear-l" d="M7.4 9.2 7 3.4l4.2 2.8" />
        <path className="cat-ear cat-ear-r" d="M16.8 6.2 21 3.4l-.4 5.8" />
        {/* head */}
        <path d="M11.2 6.2C12.1 5.9 13.1 5.8 14 5.8s1.9.1 2.8.4M7.4 9.2c-.8 1-1.2 2.3-1.2 3.6 0 3.4 3 5.7 7.8 5.7s7.8-2.3 7.8-5.7c0-1.3-.4-2.6-1.2-3.6" />
        {/* body, sitting */}
        <path d="M9.6 17.7C7.6 20.4 7 24 7.8 28.2h13.4c.8-4.2.2-7.8-1.8-10.5" />
        {/* tail */}
        <path className="cat-tail" d="M21.2 27.6c3.8.2 6.6-1.6 6.3-4.6-.2-2-2-2.6-2.7-1.2" />
        {/* eyes */}
        <path className="cat-eyes" d="M11.2 11.8v1.2M16.8 11.8v1.2" />
      </svg>
    </button>
  );
}
