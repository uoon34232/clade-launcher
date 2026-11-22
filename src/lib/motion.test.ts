import { afterEach, describe, expect, it, vi } from "vitest";
import { getMotionMode, prefersReducedMotion, setMotionMode } from "./motion";

const REDUCED_QUERY = "(prefers-reduced-motion: reduce)";

const system = (reduce: boolean) =>
  vi.stubGlobal("matchMedia", (q: string) => {
    expect(q).toBe(REDUCED_QUERY);
    return { matches: reduce };
  });

afterEach(() => {
  setMotionMode("system");
  vi.unstubAllGlobals();
});

describe("reduced motion", () => {
  it("follows Windows by default", () => {
    system(true);
    expect(prefersReducedMotion()).toBe(true);
    system(false);
    expect(prefersReducedMotion()).toBe(false);
    expect(getMotionMode()).toBe("system");
  });

  it("returns false when matchMedia is unavailable", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(prefersReducedMotion()).toBe(false);
  });

  it("On and Off beat the system preference", () => {
    system(false);
    setMotionMode("on");
    expect(prefersReducedMotion()).toBe(true);
    system(true);
    setMotionMode("off");
    expect(prefersReducedMotion()).toBe(false);
  });

  it("marks the page so the stylesheet can follow the same choice", () => {
    setMotionMode("on");
    expect(document.documentElement.dataset.motion).toBe("reduce");
    setMotionMode("off");
    expect(document.documentElement.dataset.motion).toBe("full");
    setMotionMode("system");
    expect(document.documentElement.dataset.motion).toBe("system");
  });
});
