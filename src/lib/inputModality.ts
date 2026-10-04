// Tracks whether the user last interacted with the keyboard or a pointing device, exposed as
// `<html data-input="keyboard|pointer">`.
//
// Why: `:focus-visible` alone keeps matching on an element that was focused with the keyboard
// even after the user clicks it with the mouse, leaving a focus ring behind. Styles that need
// "keyboard navigation only" combine `:focus-visible` with `html[data-input="keyboard"]`.
// Until the first key press the attribute is `pointer`, so no ring shows on a fresh start.

export type InputModality = "keyboard" | "pointer";

const MODIFIER_KEYS = new Set([
  "Shift",
  "Control",
  "Alt",
  "Meta",
  "AltGraph",
  "CapsLock",
  "NumLock",
  "ScrollLock",
]);

/** Installs the listeners; returns a function that removes them. */
export function installInputModality(doc: Document = document): () => void {
  const root = doc.documentElement;
  const set = (m: InputModality) => {
    if (root.dataset.input !== m) root.dataset.input = m;
  };
  const onKeyDown = (e: KeyboardEvent) => {
    // Pure modifier presses and OS/browser shortcuts are not navigation.
    if (MODIFIER_KEYS.has(e.key) || e.ctrlKey || e.metaKey || e.altKey) return;
    set("keyboard");
  };
  const onPointer = () => set("pointer");

  set("pointer");
  // Capture phase, so the mode is already correct when the browser moves focus or activates a control.
  doc.addEventListener("keydown", onKeyDown, true);
  doc.addEventListener("pointerdown", onPointer, true);
  doc.addEventListener("mousedown", onPointer, true);
  doc.addEventListener("touchstart", onPointer, true);
  return () => {
    doc.removeEventListener("keydown", onKeyDown, true);
    doc.removeEventListener("pointerdown", onPointer, true);
    doc.removeEventListener("mousedown", onPointer, true);
    doc.removeEventListener("touchstart", onPointer, true);
  };
}
