import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (c: string) => invoke(c) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { applyGlass, loadGlassStatus, OPAQUE, parseGlassStatus } from "./glass";
import { setMotionMode } from "./motion";

const root = () => document.documentElement;

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  delete root().dataset.glass;
  delete root().dataset.glassLive;
  setMotionMode("system");
  invoke.mockReset();
  vi.restoreAllMocks();
});

describe("the glass state", () => {
  it("accepts only a well-formed status and treats anything else as opaque", () => {
    expect(parseGlassStatus({ active: true, live: true })).toEqual({ active: true, live: true });
    expect(parseGlassStatus({ active: true, live: false })).toEqual({ active: true, live: false });
    for (const bad of [null, undefined, 3, "on", {}, { active: "yes" }])
      expect(parseGlassStatus(bad)).toEqual(OPAQUE);
    // an inactive material is never reported as live
    expect(parseGlassStatus({ active: false, live: true })).toEqual(OPAQUE);
  });

  it("marks the page when glass is on, and says whether Windows really draws it", () => {
    applyGlass({ active: true, live: true });
    expect(root().dataset.glass).toBe("on");
    expect(root().dataset.glassLive).toBe("true");
    applyGlass({ active: true, live: false });
    expect(root().dataset.glassLive).toBe("false");
  });

  it("leaves no glass state behind when the opaque look is chosen", () => {
    applyGlass({ active: true, live: true });
    applyGlass(OPAQUE);
    expect(root().dataset.glass).toBe("off");
    expect(root().dataset.glassLive).toBeUndefined();
  });

  it("is opaque outside the launcher and when the backend cannot answer", async () => {
    expect(await loadGlassStatus()).toEqual(OPAQUE);
    expect(invoke).not.toHaveBeenCalled();
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    vi.spyOn(console, "warn").mockImplementation(() => {});
    invoke.mockRejectedValue(new Error("no backend"));
    expect(await loadGlassStatus()).toEqual(OPAQUE);
  });

  it("is not changed by reduced motion, and does not change it", () => {
    applyGlass({ active: true, live: true });
    setMotionMode("on");
    expect(root().dataset.glass).toBe("on");
    expect(root().dataset.motion).toBe("reduce");
    setMotionMode("off");
    expect(root().dataset.glass).toBe("on");
    applyGlass(OPAQUE);
    expect(root().dataset.motion).toBe("full");
  });
});
