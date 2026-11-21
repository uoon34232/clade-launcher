import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from "react";
import {
  candidates,
  phraseText,
  pickPhrase,
  type Phrase,
  type PhraseContext,
} from "@/features/launcher/phrases";
import { useT } from "@/lib/i18n";
import { emitBurst } from "@/lib/fx/burst";
import { formatPlaytime } from "@/features/launcher/profileUi";
import { prefersReducedMotion } from "@/lib/motion";
import { saveLastPhrase } from "@/lib/settings/uiSettings";
import { playSound } from "@/lib/sound/soundService";

const OUT_MS = 170;
const IN_MS = 230; // OUT + IN = 400 ms
const REDUCED_FADE_MS = 100;
const SAVE_DEBOUNCE_MS = 800;

interface HeroProps {
  /** Facts for future contextual messages (last instance, playtime, time since last session). */
  context?: PhraseContext;
  /** Id of the headline shown last time, from the launcher's saved settings. */
  lastPhraseId?: string | null;
  /** Total Minecraft playtime across all profiles, in seconds (the real backend value). */
  playtimeSeconds?: number;
  /** True right after an embedded game closes so the headline glides back swiftly instead of the startup entrance. */
  returning?: boolean;
}

export function Hero({ context, lastPhraseId, playtimeSeconds = 0, returning = false }: HeroProps) {
  const t = useT(); // re-renders on a language change; the phrase itself (its id) stays the same
  // A fresh phrase on every start, never the one shown last time.
  const [phrase, setPhrase] = useState<Phrase>(() =>
    pickPhrase(candidates(context), lastPhraseId ?? undefined),
  );

  const initialSavedRef = useRef(false);
  const lastSavedRef = useRef<string | null>(lastPhraseId ?? null);
  const pendingSaveRef = useRef<string | null>(null);
  const saveTimerRef = useRef<number | undefined>(undefined);

  // Whatever is on screen is what gets remembered (saved by the launcher's settings service).
  // The startup phrase is saved right away; rapid headline shuffles are debounced by 800 ms.
  useEffect(() => {
    window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = undefined;

    if (!initialSavedRef.current) {
      initialSavedRef.current = true;
      pendingSaveRef.current = null;
      if (lastSavedRef.current !== phrase.id) {
        lastSavedRef.current = phrase.id;
        saveLastPhrase(phrase.id);
      }
      return;
    }

    if (lastSavedRef.current === phrase.id) {
      pendingSaveRef.current = null;
      return;
    }

    pendingSaveRef.current = phrase.id;
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = undefined;
      const id = pendingSaveRef.current;
      pendingSaveRef.current = null;
      if (id && lastSavedRef.current !== id) {
        lastSavedRef.current = id;
        saveLastPhrase(id);
      }
    }, SAVE_DEBOUNCE_MS);
  }, [phrase.id]);

  const buttonRef = useRef<HTMLButtonElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const busy = useRef(false); // true from the first click until the new phrase has finished fading in
  const incoming = useRef(false); // the next render is a phrase swap that must fade in
  const outAnim = useRef<Animation | null>(null);
  const pressAnim = useRef<Animation | null>(null);

  const rotate = useCallback(() => {
    if (busy.current) return; // rapid clicks during a transition are absorbed, never queued
    busy.current = true;
    const next = pickPhrase(candidates(context), phrase.id);
    const el = textRef.current;

    // Without the Web Animations API (tests, very old engines) swap immediately.
    if (!el || typeof el.animate !== "function") {
      setPhrase(next);
      busy.current = false;
      return;
    }

    const reduced = prefersReducedMotion();
    const out = el.animate(
      reduced
        ? [{ opacity: 1 }, { opacity: 0 }]
        : [
            { opacity: 1, transform: "translateY(0)" },
            { opacity: 0, transform: "translateY(-6px)" },
          ],
      {
        duration: reduced ? REDUCED_FADE_MS : OUT_MS,
        easing: "cubic-bezier(0.4, 0, 1, 1)",
        fill: "forwards",
      },
    );
    outAnim.current = out;
    out.finished.then(
      () => {
        incoming.current = true;
        setPhrase(next);
      },
      () => {
        busy.current = false; // cancelled (unmount): nothing to finish
      },
    );
  }, [context, phrase.id]);

  // The swap happened: fade the new text in from just below, before the browser paints.
  useLayoutEffect(() => {
    if (!incoming.current) return;
    incoming.current = false;
    const el = textRef.current;
    outAnim.current?.cancel();
    outAnim.current = null;
    if (!el || typeof el.animate !== "function") {
      busy.current = false;
      return;
    }
    const reduced = prefersReducedMotion();
    const inn = el.animate(
      reduced
        ? [{ opacity: 0 }, { opacity: 1 }]
        : [
            { opacity: 0, transform: "translateY(8px)" },
            { opacity: 1, transform: "translateY(0)" },
          ],
      { duration: reduced ? REDUCED_FADE_MS : IN_MS, easing: "cubic-bezier(0.22, 1, 0.36, 1)" },
    );
    const done = () => {
      busy.current = false;
    };
    inn.finished.then(done, done);
  }, [phrase]);

  useEffect(
    () => () => {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = undefined;
      const pending = pendingSaveRef.current;
      pendingSaveRef.current = null;
      if (pending && lastSavedRef.current !== pending) {
        lastSavedRef.current = pending;
        saveLastPhrase(pending);
      }
      outAnim.current?.cancel();
      pressAnim.current?.cancel();
      busy.current = false;
    },
    [],
  );

  const onClick = (e: MouseEvent<HTMLButtonElement>) => {
    const button = buttonRef.current;
    const reduced = prefersReducedMotion();

    // Mouse clicks burst from the real cursor position; a keyboard activation has no cursor, so it
    // bursts from the middle of the headline.
    let x = e.clientX;
    let y = e.clientY;
    if (e.detail === 0 && button) {
      const r = (textRef.current ?? button).getBoundingClientRect();
      x = r.left + r.width / 2;
      y = r.top + r.height / 2;
    }

    // A very slight press: scale down and return. Restarted (not stacked) by repeated clicks.
    if (button && !reduced && typeof button.animate === "function") {
      pressAnim.current?.cancel();
      pressAnim.current = button.animate(
        [{ scale: "1" }, { scale: "0.985", offset: 0.4 }, { scale: "1" }],
        {
          duration: 190,
          easing: "cubic-bezier(0.22, 1, 0.36, 1)",
        },
      );
    }

    emitBurst(x, y, { reducedMotion: reduced });
    playSound("headline");
    rotate();
  };

  return (
    <div
      className={`${returning ? "hero-return" : "hero-rise"} flex flex-col items-center px-8 text-center`}
    >
      <h1
        className="text-graphite font-serif"
        style={{
          fontSize: "clamp(3.25rem, 8.6vmin + 1.5vw, 8.5rem)",
          lineHeight: 0.98,
          fontWeight: 340,
          letterSpacing: "-0.028em",
          wordSpacing: "0.04em",
          fontVariationSettings: '"opsz" 144, "SOFT" 60, "WONK" 0',
        }}
      >
        {/* Editorial text that happens to be clickable: a native button (Enter/Space for free) with every visual trace of one removed. */}
        <button
          ref={buttonRef}
          type="button"
          className="hero-link"
          onClick={onClick}
          aria-description={t("hero.shuffleHint")}
        >
          <span ref={textRef} className="hero-text" aria-live="polite" aria-atomic="true">
            {phraseText(phrase)}
          </span>
        </button>
      </h1>
      <p className="text-ink-muted mt-[clamp(1rem,2.4vmin,1.75rem)] text-[clamp(0.9375rem,1.6vmin,1.125rem)] tracking-[0.005em]">
        {t("hero.subtitle")}
      </p>
      {/* Total playtime: small, muted, no container. The line's space is always reserved, so it
          fading in after the data loads (or staying empty for a new launcher) never shifts the hero. */}
      <p
        className={`text-ink-muted ease-out-soft mt-[clamp(0.45rem,1.1vmin,0.8rem)] min-h-[1.25em] text-[clamp(0.78rem,1.3vmin,0.875rem)] tracking-[0.01em] transition-opacity duration-500 ${playtimeSeconds > 0 ? "opacity-85" : "opacity-0"}`}
        aria-hidden={playtimeSeconds > 0 ? undefined : true}
      >
        {playtimeSeconds > 0 ? t("hero.played", { time: formatPlaytime(playtimeSeconds) }) : " "}
      </p>
    </div>
  );
}
