import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string) => Promise<unknown>>();
const listen =
  vi.fn<(event: string, handler: (e: { payload: unknown }) => void) => Promise<() => void>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (c: string) => invoke(c) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: (e: { payload: unknown }) => void) => listen(event, handler),
}));

import {
  applyGlass,
  GLASS_EVENT,
  loadGlassStatus,
  onGlassStatusChanged,
  OPAQUE,
  parseGlassStatus,
} from "./glass";
import { setMotionMode } from "./motion";

const root = () => document.documentElement;

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  delete root().dataset.glass;
  delete root().dataset.glassLive;
  setMotionMode("system");
  invoke.mockReset();
  listen.mockReset();
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
    expect(invoke).toHaveBeenCalledWith("get_glass_status");
  });

  it("loads and parses glass status from the backend inside the launcher", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockResolvedValueOnce({ active: true, live: true });
    expect(await loadGlassStatus()).toEqual({ active: true, live: true });
    expect(invoke).toHaveBeenCalledWith("get_glass_status");

    invoke.mockResolvedValueOnce({ active: true, live: false });
    expect(await loadGlassStatus()).toEqual({ active: true, live: false });

    for (const bad of [null, "on", { active: "yes" }, { active: false, live: true }]) {
      invoke.mockResolvedValueOnce(bad);
      expect(await loadGlassStatus()).toEqual(OPAQUE);
    }
  });

  it("no-ops when subscribing outside the launcher", async () => {
    const callback = vi.fn();
    const unlisten = await onGlassStatusChanged(callback);
    expect(listen).not.toHaveBeenCalled();
    expect(() => unlisten()).not.toThrow();
    expect(callback).not.toHaveBeenCalled();
  });

  it("subscribes to glass-status-changed events, parses payloads, and returns unlisten", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let handler: ((e: { payload: unknown }) => void) | undefined;
    const rawUnlisten = vi.fn();
    listen.mockImplementationOnce((_event, cb) => {
      handler = cb;
      return Promise.resolve(rawUnlisten);
    });

    const callback = vi.fn();
    const unlisten = await onGlassStatusChanged(callback);

    expect(listen).toHaveBeenCalledTimes(1);
    expect(listen).toHaveBeenCalledWith(GLASS_EVENT, expect.any(Function));
    expect(GLASS_EVENT).toBe("glass-status-changed");

    handler?.({ payload: { active: true, live: true } });
    handler?.({ payload: { active: true, live: false } });
    handler?.({ payload: null });
    handler?.({ payload: { active: false, live: true } });

    expect(callback).toHaveBeenNthCalledWith(1, { active: true, live: true });
    expect(callback).toHaveBeenNthCalledWith(2, { active: true, live: false });
    expect(callback).toHaveBeenNthCalledWith(3, OPAQUE);
    expect(callback).toHaveBeenNthCalledWith(4, OPAQUE);

    unlisten();
    expect(rawUnlisten).toHaveBeenCalledTimes(1);
  });

  it("swallows listen rejections when subscribing to glass status changes", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    listen.mockRejectedValueOnce(new Error("event channel unavailable"));

    const callback = vi.fn();
    const unlisten = await onGlassStatusChanged(callback);

    expect(listen).toHaveBeenCalledWith(GLASS_EVENT, expect.any(Function));
    expect(warn).toHaveBeenCalledTimes(1);
    expect(() => unlisten()).not.toThrow();
    expect(callback).not.toHaveBeenCalled();
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
