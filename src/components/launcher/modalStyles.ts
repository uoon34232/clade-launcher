// Shared styling for everything inside a Modal, so every secondary surface has the same fields,
// labels and buttons.

const FIELD_SURFACE =
  "rounded-xl bg-sand/45 text-graphite outline-none transition-shadow duration-200 placeholder:text-ink-muted/60 focus-visible:ring-2 focus-visible:ring-clay/45";
export const FIELD = `${FIELD_SURFACE} h-11 w-full px-3.5 text-[0.9375rem]`;
export const FIELD_COMPACT = `${FIELD_SURFACE} h-9 px-2.5 text-[0.875rem]`;
export const LABEL = "text-[0.8125rem] font-medium text-ink-muted";

const BUTTON =
  "dock-item h-11 cursor-default rounded-[1.1rem] px-5 text-[0.9375rem] font-medium outline-none focus-visible:ring-2 disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:scale-100";

export const BUTTON_QUIET = `${BUTTON} text-ink-muted hover:text-graphite focus-visible:ring-clay/45`;
export const BUTTON_SOFT = `${BUTTON} bg-sand-soft text-graphite hover:bg-sand focus-visible:ring-clay/45`;
export const BUTTON_PRIMARY = `${BUTTON} dock-item--primary bg-clay px-7 font-semibold text-[#fbf6ee] shadow-[inset_0_1px_0_rgb(255_255_255/0.18),0_6px_18px_-8px_rgb(181_96_74/0.7)] hover:bg-[#c26c55] focus-visible:ring-clay/60 focus-visible:ring-offset-2 focus-visible:ring-offset-card disabled:hover:bg-clay`;
export const BUTTON_DANGER = `${BUTTON} text-clay-deep hover:bg-clay/10 focus-visible:ring-clay/45`;

// Compact buttons for dense groups (Settings > Support): the same look, a little smaller.
const BUTTON_SMALL =
  "dock-item min-h-9 cursor-default rounded-[0.9rem] px-3 py-1.5 text-[0.8125rem] leading-tight font-medium outline-none focus-visible:ring-2 focus-visible:ring-clay/45 disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:scale-100";
export const BUTTON_SMALL_SOFT = `${BUTTON_SMALL} bg-sand-soft text-graphite hover:bg-sand`;
// A quiet outlined button: clearly something to press, yet calmer than the filled soft one.
export const BUTTON_SMALL_GHOST = `${BUTTON_SMALL} text-ink-muted ring-1 ring-inset ring-graphite/15 hover:bg-graphite/5 hover:text-graphite hover:ring-graphite/25`;
export const BUTTON_SMALL_DANGER = `${BUTTON_SMALL} text-clay-deep hover:bg-clay/10`;
export const BUTTON_SMALL_QUIET = `${BUTTON_SMALL} text-ink-muted hover:text-graphite`;
