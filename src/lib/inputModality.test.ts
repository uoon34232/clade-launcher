import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installInputModality } from "./inputModality";

let uninstall: () => void;
const mode = () => document.documentElement.dataset.input;
const key = (k: string, init: KeyboardEventInit = {}) =>
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
const pointer = (type = "pointerdown") =>
  document.body.dispatchEvent(new Event(type, { bubbles: true }));

beforeEach(() => {
  delete document.documentElement.dataset.input;
  uninstall = installInputModality();
});
afterEach(() => uninstall());

describe("input modality", () => {
  it("starts as pointer, so a fresh start shows no keyboard focus ring", () => {
    expect(mode()).toBe("pointer");
  });

  it("switches to keyboard on navigation keys and back to pointer on a click", () => {
    for (const k of ["Tab", "Enter", " ", "ArrowDown", "a"]) {
      pointer();
      expect(mode()).toBe("pointer");
      key(k);
      expect(mode()).toBe("keyboard");
    }
    pointer("mousedown");
    expect(mode()).toBe("pointer");
    key("Tab");
    pointer("touchstart");
    expect(mode()).toBe("pointer");
  });

  it("ignores bare modifiers and shortcuts", () => {
    for (const k of ["Shift", "Control", "Alt", "Meta"]) key(k);
    key("c", { ctrlKey: true });
    key("Tab", { altKey: true });
    expect(mode()).toBe("pointer");
  });

  it("cleans up its listeners", () => {
    uninstall();
    pointer();
    key("Tab");
    expect(mode()).toBe("pointer"); // no longer tracking: left as it was
  });

  it("registers pointer listeners as passive capture so pointer events never block scrolling", () => {
    uninstall();
    const addSpy = vi.spyOn(document, "addEventListener");
    uninstall = installInputModality(document);
    const pointerCall = addSpy.mock.calls.find(([type]) => type === "pointerdown");
    expect(pointerCall?.[2]).toEqual({ capture: true, passive: true });
    addSpy.mockRestore();
  });
});
