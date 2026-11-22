// Shared styling for everything inside a Modal, so every secondary surface has the same fields,
// labels and buttons.

const FIELD_SURFACE =
  "rounded-xl border border-sand-border bg-sand/45 text-graphite outline-hidden transition-shadow duration-200 placeholder:text-ink-muted/85 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 forced-colors:border-[ButtonText] forced-colors:disabled:opacity-100";
export const FIELD = `${FIELD_SURFACE} h-11 w-full px-3.5 text-[0.9375rem]`;
export const FIELD_COMPACT = `${FIELD_SURFACE} h-9 px-2.5 text-[0.875rem]`;
export const LABEL = "text-[0.8125rem] font-medium text-ink-muted";

const BUTTON =
  "dock-item min-h-11 cursor-default rounded-[1.1rem] border border-transparent px-5 py-2 text-[0.9375rem] font-medium outline-hidden focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:scale-100 forced-colors:border-[ButtonText]";

export const BUTTON_QUIET = `${BUTTON} text-ink-muted enabled:hover:bg-[var(--quiet-hover)] enabled:hover:text-graphite focus-visible:ring-ring`;
export const BUTTON_SOFT = `${BUTTON} border-sand-border/70 dark:border-sand-border bg-sand-soft text-graphite enabled:hover:bg-[var(--soft-hover)] focus-visible:ring-ring`;
export const BUTTON_PRIMARY = `${BUTTON} dock-item--primary bg-clay px-7 font-semibold text-[#fbf6ee] shadow-[inset_0_1px_0_rgb(255_255_255/0.18),0_6px_18px_-8px_rgb(163_82_61/0.7)] hover:bg-clay-hover active:bg-clay-active focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card disabled:hover:bg-clay`;
export const BUTTON_DANGER = `${BUTTON} text-clay-deep hover:bg-clay/10 focus-visible:ring-ring`;

// Compact buttons for dense groups (Settings > Support): the same look, a little smaller.
const BUTTON_SMALL =
  "dock-item min-h-9 cursor-default rounded-[0.9rem] border border-transparent px-3 py-1.5 text-[0.8125rem] leading-tight font-medium outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:scale-100 forced-colors:border-[ButtonText]";
export const BUTTON_SMALL_SOFT = `${BUTTON_SMALL} border-sand-border/70 dark:border-sand-border bg-sand-soft text-graphite enabled:hover:bg-[var(--soft-hover)]`;
// A quiet outlined button: clearly something to press, yet calmer than the filled soft one.
export const BUTTON_SMALL_GHOST = `${BUTTON_SMALL} text-ink-muted ring-1 ring-inset ring-sand-border/80 dark:ring-sand-border enabled:hover:bg-[var(--quiet-hover)] enabled:hover:text-graphite enabled:hover:ring-graphite/35`;
export const BUTTON_SMALL_DANGER = `${BUTTON_SMALL} text-clay-deep hover:bg-clay/10`;
export const BUTTON_SMALL_QUIET = `${BUTTON_SMALL} text-ink-muted enabled:hover:bg-[var(--quiet-hover)] enabled:hover:text-graphite`;
export const BUTTON_SMALL_PRIMARY = `${BUTTON_SMALL} dock-item--primary bg-clay px-4 font-semibold text-[#fbf6ee] shadow-[inset_0_1px_0_rgb(255_255_255/0.18),0_6px_18px_-8px_rgb(163_82_61/0.7)] hover:bg-clay-hover active:bg-clay-active focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card disabled:hover:bg-clay`;
