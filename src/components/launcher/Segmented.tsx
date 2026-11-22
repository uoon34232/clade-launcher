import { useRef, type KeyboardEvent } from "react";
import { cn } from "@/lib/utils";

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

interface Props<T extends string> {
  value: T | null;
  onChange: (value: T) => void;
  options: readonly SegmentedOption<T>[];
  /** Accessible name of the group. */
  label: string;
  /** `track`: pills on a soft track (profile type, loaders). `plain`: quiet text toggles (filters). */
  variant?: "track" | "plain";
  /** Slightly smaller segments, for dense groups such as Settings. */
  compact?: boolean;
  disabled?: boolean;
  className?: string;
}

/**
 * A radio group presented as a row of segments. Real radio semantics: one tab stop, arrow keys
 * move the selection (skipping disabled options), Home/End jump to the ends.
 */
export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  variant = "track",
  compact = false,
  disabled = false,
  className,
}: Props<T>) {
  const refs = useRef(new Map<T, HTMLButtonElement>());
  const enabled = disabled ? [] : options.filter((o) => !o.disabled);
  // The single tab stop: the selected option, or the first enabled one if nothing is selected.
  const tabStop = enabled.find((o) => o.value === value)?.value ?? enabled[0]?.value;

  const move = (from: T, delta: number | "first" | "last") => {
    const i = enabled.findIndex((o) => o.value === from);
    const next =
      delta === "first"
        ? enabled[0]
        : delta === "last"
          ? enabled[enabled.length - 1]
          : enabled[(i + delta + enabled.length) % enabled.length];
    if (!next) return;
    onChange(next.value);
    refs.current.get(next.value)?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, v: T) => {
    const k = e.key;
    if (k === "ArrowRight" || k === "ArrowDown") move(v, 1);
    else if (k === "ArrowLeft" || k === "ArrowUp") move(v, -1);
    else if (k === "Home") move(v, "first");
    else if (k === "End") move(v, "last");
    else return;
    e.preventDefault();
  };

  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={cn(
        variant === "track"
          ? "bg-sand/45 inline-flex max-w-full flex-wrap gap-1 rounded-[0.9rem] p-1"
          : "inline-flex max-w-full flex-wrap gap-x-4 gap-y-1",
        className,
      )}
    >
      {options.map((o) => {
        const selected = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              if (el) refs.current.set(o.value, el);
              else refs.current.delete(o.value);
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            disabled={Boolean(disabled || o.disabled)}
            tabIndex={o.value === tabStop ? 0 : -1}
            onClick={() => {
              if (!disabled && !o.disabled) onChange(o.value);
            }}
            onKeyDown={(e) => {
              if (!disabled && !o.disabled) onKeyDown(e, o.value);
            }}
            className={cn(
              "ease-out-soft focus-visible:ring-ring min-w-0 cursor-default border border-transparent outline-hidden transition-[background-color,color,box-shadow,opacity] duration-200 focus-visible:ring-2 disabled:opacity-40",
              selected && "forced-colors:border-[Highlight]",
              variant === "track"
                ? cn(
                    "truncate",
                    compact
                      ? "h-8 rounded-[0.65rem] px-2.5 text-[0.8125rem] font-medium"
                      : "h-9 rounded-[0.7rem] px-3.5 text-[0.875rem] font-medium",
                    selected
                      ? "text-graphite bg-[var(--segmented-active)] shadow-[var(--segmented-shadow)]"
                      : "text-ink-muted enabled:hover:text-graphite enabled:hover:bg-[var(--segmented-hover)]",
                  )
                : cn(
                    "rounded-md px-1.5 py-1.5 text-[0.8125rem]",
                    selected
                      ? "text-graphite font-medium"
                      : "text-ink-muted enabled:hover:text-graphite enabled:hover:bg-[var(--quiet-hover)]",
                  ),
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
