import type { ReactNode, RefObject } from "react";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { playSound } from "@/lib/sound/soundService";
import { cn } from "@/lib/utils";

const WIDTH = {
  sm: "sm:max-w-[26rem]",
  md: "sm:max-w-[30rem]",
  lg: "sm:max-w-[40rem]",
  xl: "sm:max-w-[44rem]",
} as const;

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  size?: keyof typeof WIDTH;
  /** Element to focus on open; the first focusable element otherwise. */
  initialFocus?: RefObject<HTMLElement | null>;
  /** While true Escape and the backdrop do nothing (a save is running, or a destructive confirmation is showing). */
  locked?: boolean;
  /** Clicking the backdrop closes the modal. Turn off for anything that would lose input. */
  closeOnBackdrop?: boolean;
  children: ReactNode;
}

/**
 * The one surface for every secondary screen (Create profile, Settings, Account, ...). A large
 * centred card inside the launcher window: the launcher stays visible behind it, softly dimmed and
 * blurred; the card itself stays warm and opaque. Enter and exit are a short fade with a small scale
 * and rise (mirrored on close). Focus is trapped while open and returns to the trigger on close
 * (both handled by the dialog primitive). Escape and, if allowed, the backdrop close it.
 */
export function Modal({
  open,
  onOpenChange,
  size = "md",
  initialFocus,
  locked = false,
  closeOnBackdrop = true,
  children,
}: Props) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next, details) => {
        if (next) return onOpenChange(true);
        const byBackdrop = details.reason === "outside-press";
        if (locked || (byBackdrop && !closeOnBackdrop)) {
          details.cancel();
          return;
        }
        playSound("click");
        onOpenChange(false);
      }}
    >
      <DialogContent
        showCloseButton={false}
        initialFocus={initialFocus}
        className={cn(
          "glass-modal text-graphite data-open:slide-in-from-bottom-2 data-closed:slide-out-to-bottom-2 max-h-[calc(100vh-2rem)] max-w-[calc(100%-2rem)] grid-cols-1 gap-0 overflow-x-hidden overflow-y-auto rounded-[1.75rem] duration-200",
          size === "xl" ? "px-6 py-5" : "p-7",
          WIDTH[size],
        )}
      >
        {children}
      </DialogContent>
    </Dialog>
  );
}

export function ModalHeader({ title, description }: { title: string; description?: string }) {
  return (
    <div className="mb-5 min-w-0">
      <DialogTitle
        className="font-serif text-[1.75rem] leading-tight font-normal tracking-[-0.02em] break-words"
        style={{ fontVariationSettings: '"opsz" 72, "SOFT" 60' }}
      >
        {title}
      </DialogTitle>
      {description && (
        <DialogDescription className="text-ink-muted mt-1.5 text-[0.875rem] break-words">
          {description}
        </DialogDescription>
      )}
    </div>
  );
}

export function ModalFooter({ children }: { children: ReactNode }) {
  return (
    <div className="mt-1 flex flex-wrap items-center justify-end gap-3 gap-y-2 pt-1">
      {children}
    </div>
  );
}

/** A titled group inside a modal body. Sentence-case label, no boxes. `compact` tightens the spacing. */
export function ModalSection({
  title,
  compact = false,
  className,
  children,
}: {
  title: string;
  compact?: boolean;
  /** Local spacing tweaks only (for example a tighter gap where a screen must fit the smallest window). */
  className?: string;
  children: ReactNode;
}) {
  return (
    <section
      className={cn(
        "border-graphite/10 flex flex-col border-t first:border-t-0 first:pt-0",
        compact ? "gap-2.5 py-2.5" : "gap-3 py-5",
        className,
      )}
    >
      <h3 className="text-ink-muted text-[0.8125rem] font-medium">{title}</h3>
      {children}
    </section>
  );
}
