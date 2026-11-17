import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_ACTIVE_PARTICLES, activeParticleCount, emitBurst } from "./burst";

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

const layerCount = () => document.querySelectorAll("#fx-layer").length;
const particles = () => document.querySelectorAll("#fx-layer span");

describe("emitBurst", () => {
  it("creates 6-12 particles at the exact click position, inside a non-interactive layer", () => {
    vi.useFakeTimers();
    for (let i = 0; i < 50; i++) {
      const n = emitBurst(321, 123.5);
      expect(n).toBeGreaterThanOrEqual(6);
      expect(n).toBeLessThanOrEqual(12);
      vi.runAllTimers();
    }
    emitBurst(321, 123.5, { count: 7 });
    const first = particles()[0] as HTMLElement;
    expect(first.style.left).toBe("321px");
    expect(first.style.top).toBe("123.5px");
    const host = document.getElementById("fx-layer") as HTMLElement;
    expect(host.style.pointerEvents).toBe("none");
    expect(host.getAttribute("aria-hidden")).toBe("true");
    vi.runAllTimers();
  });

  it("cleans up every particle and the layer itself after the lifetime, with no orphans", () => {
    vi.useFakeTimers();
    for (let i = 0; i < 20; i++) emitBurst(10 + i, 10);
    expect(activeParticleCount()).toBeGreaterThan(0);
    expect(layerCount()).toBe(1);
    vi.advanceTimersByTime(1000); // longer than the longest life (650 ms) plus the failsafe
    expect(particles().length).toBe(0);
    expect(activeParticleCount()).toBe(0);
    expect(layerCount()).toBe(0);
  });

  it("caps concurrent particles at 24 when clicks are mashed and uses one batch timer per burst", () => {
    vi.useFakeTimers();
    expect(MAX_ACTIVE_PARTICLES).toBe(24);
    const timeoutSpy = vi.spyOn(window, "setTimeout");
    emitBurst(50, 50, { count: 12 });
    expect(timeoutSpy).toHaveBeenCalledTimes(1);

    for (let i = 0; i < 40; i++) emitBurst(50, 50, { count: 12 });
    expect(particles().length).toBe(24);
    expect(particles().length).toBeLessThanOrEqual(MAX_ACTIVE_PARTICLES);
    expect(activeParticleCount()).toBe(particles().length);
    vi.advanceTimersByTime(1000);
    expect(activeParticleCount()).toBe(0);
    timeoutSpy.mockRestore();
  });

  it("keeps particles tiny, within the palette, and free of per-particle layer hints or shadows", () => {
    vi.useFakeTimers();
    emitBurst(0, 0, { count: 12 });
    const allowed = [
      "rgb(181, 96, 74)",
      "rgb(156, 80, 57)",
      "rgb(43, 38, 34)",
      "rgb(228, 213, 190)",
      "rgb(246, 240, 230)",
    ];
    for (const el of particles()) {
      const s = (el as HTMLElement).style;
      expect(parseFloat(s.width)).toBeLessThanOrEqual(8);
      expect(parseFloat(s.height)).toBeLessThanOrEqual(4);
      expect(allowed).toContain(s.background);
      expect(s.willChange).toBe("");
      expect(s.boxShadow).toBe("");
    }
    vi.advanceTimersByTime(1000);
  });

  it("emits nothing under reduced motion", () => {
    expect(emitBurst(5, 5, { reducedMotion: true })).toBe(0);
    expect(layerCount()).toBe(0);
  });
});
