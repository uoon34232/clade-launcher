import { useEffect, useRef, useState } from "react";
import { DownloadIcon, LoaderCircleIcon, LogInIcon, PlayIcon } from "lucide-react";
import { emitBurst } from "@/lib/fx/burst";
import { useT, type MessageKey } from "@/lib/i18n";
import { prefersReducedMotion } from "@/lib/motion";
import { playSound } from "@/lib/sound/soundService";

/** What the primary dock button is doing. */
export type PlayState = "play" | "setup" | "launching" | "playing" | "repairing" | "signin";

interface Props {
  onClick?: () => void;
  state?: PlayState;
}

const LABEL: Record<PlayState, MessageKey> = {
  play: "play.play",
  setup: "play.setup",
  launching: "play.launching",
  playing: "play.playing",
  repairing: "play.repairing",
  signin: "play.signIn",
};

/**
 * The dock's primary action. It reads "Set up" for a profile that is not prepared, "Play" for a
 * ready one, and while a game is starting, running or being repaired it shows that quietly and
 * cannot be pressed again (no accidental second launch). Without an account that can play it reads
 * "Sign in to continue" and starts the sign-in instead of an action that would be refused.
 */
export function PlayButton({ onClick, state = "play" }: Props) {
  const t = useT();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [pulseSeq, setPulseSeq] = useState(0);
  const pulseTimerRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (pulseTimerRef.current !== null) window.clearTimeout(pulseTimerRef.current);
    },
    [],
  );

  const busy = state === "launching" || state === "repairing";
  const disabled = busy || state === "playing";
  return (
    <button
      ref={btnRef}
      type="button"
      disabled={disabled}
      aria-busy={busy}
      data-state={state}
      onClick={(e) => {
        if (state === "play") {
          let x = e.clientX;
          let y = e.clientY;
          if (e.detail === 0 && btnRef.current) {
            const r = btnRef.current.getBoundingClientRect();
            x = r.left + r.width / 2;
            y = r.top + r.height / 2;
          }
          const reduced = prefersReducedMotion();
          emitBurst(x, y, { count: 10, reducedMotion: reduced });
          if (!reduced) {
            setPulseSeq((n) => n + 1);
            if (pulseTimerRef.current !== null) window.clearTimeout(pulseTimerRef.current);
            pulseTimerRef.current = window.setTimeout(() => {
              pulseTimerRef.current = null;
              setPulseSeq(0);
            }, 560);
          }
        }
        playSound("primary");
        onClick?.();
      }}
      className={`dock-item dock-item--primary group bg-clay focus-visible:ring-clay-soft focus-visible:ring-offset-graphite active:bg-clay-deep disabled:hover:bg-clay data-[state=playing]:bg-clay/55 relative flex h-12 min-w-[7.75rem] shrink-0 cursor-default items-center justify-center gap-2 overflow-visible rounded-[1.25rem] px-6 text-[0.9375rem] leading-none font-semibold tracking-[0.01em] whitespace-nowrap text-[#fbf6ee] shadow-[inset_0_1px_0_rgb(255_255_255/0.18),0_6px_18px_-6px_rgb(163_82_61/0.7)] outline-none hover:bg-[#944834] hover:shadow-[inset_0_1px_0_rgb(255_255_255/0.22),0_10px_26px_-8px_rgb(163_82_61/0.85)] focus-visible:ring-2 focus-visible:ring-offset-2 disabled:cursor-default disabled:opacity-80 disabled:hover:scale-100 ${state === "launching" ? "play-launching-glow" : "disabled:shadow-none"}`}
    >
      {pulseSeq > 0 && (
        <span
          key={pulseSeq}
          aria-hidden="true"
          className="play-click-ring pointer-events-none absolute inset-0 rounded-[1.25rem]"
        />
      )}
      {state === "signin" ? (
        <LogInIcon aria-hidden strokeWidth={2.25} className="size-3.5 shrink-0" />
      ) : state === "setup" ? (
        <DownloadIcon aria-hidden strokeWidth={2.25} className="size-3.5 shrink-0" />
      ) : busy ? (
        <LoaderCircleIcon
          aria-hidden
          strokeWidth={2.25}
          className="size-3.5 shrink-0 animate-spin"
        />
      ) : state === "playing" ? (
        <span aria-hidden className="size-2 shrink-0 rounded-full bg-[#fbf6ee]/80" />
      ) : (
        <PlayIcon aria-hidden fill="currentColor" strokeWidth={0} className="size-3.5 shrink-0" />
      )}
      <span className="-translate-y-px">{t(LABEL[state])}</span>
    </button>
  );
}
