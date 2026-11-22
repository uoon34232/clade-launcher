import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_ACTIVE_PARTICLES, activeParticleCount, emitBurst } from "./burst";

interface MockAnimation {
  onfinish: (() => void) | null;
  oncancel: (() => void) | null;
}

interface AnimateCall {
  node: Element;
  keyframes: Keyframe[] | PropertyIndexedKeyframes | null;
  options: number | KeyframeAnimationOptions | undefined;
  anim: MockAnimation;
}

let restoreAnimate: (() => void) | null = null;

function stubElementAnimate(): AnimateCall[] {
  restoreAnimate?.();
  const original = Object.getOwnPropertyDescriptor(Element.prototype, "animate");
  const calls: AnimateCall[] = [];

  const animateStub = function (
    this: Element,
    keyframes: Keyframe[] | PropertyIndexedKeyframes | null,
    options?: number | KeyframeAnimationOptions,
  ): MockAnimation {
    const anim: MockAnimation = { onfinish: null, oncancel: null };
    calls.push({ node: this, keyframes, options, anim });
    return anim;
  };

  Object.defineProperty(Element.prototype, "animate", {
    configurable: true,
    writable: true,
    value: animateStub,
  });

  const restore = () => {
    if (original) {
      Object.defineProperty(Element.prototype, "animate", original);
    } else {
      Reflect.deleteProperty(Element.prototype, "animate");
    }
    if (restoreAnimate === restore) {
      restoreAnimate = null;
    }
  };
  restoreAnimate = restore;
  return calls;
}

afterEach(() => {
  if (vi.isFakeTimers()) {
    vi.runAllTimers();
    vi.clearAllTimers();
  }
  vi.restoreAllMocks();
  if (vi.isFakeTimers()) {
    vi.useRealTimers();
  }
  restoreAnimate?.();
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

  it("switches burst accents cleanly in dark theme", () => {
    vi.useFakeTimers();
    document.documentElement.dataset.theme = "dark";
    emitBurst(0, 0, { count: 12 });
    const allowedDark = [
      "rgb(207, 116, 89)",
      "rgb(226, 161, 139)",
      "rgb(239, 232, 221)",
      "rgb(199, 185, 166)",
      "rgb(246, 240, 230)",
    ];
    for (const el of particles()) {
      expect(allowedDark).toContain((el as HTMLElement).style.background);
    }
    delete document.documentElement.dataset.theme;
    vi.advanceTimersByTime(1000);
  });

  it("emits nothing under reduced motion", () => {
    expect(emitBurst(5, 5, { reducedMotion: true })).toBe(0);
    expect(layerCount()).toBe(0);
  });

  it("returns 0 without creating a layer or timer for zero or negative counts", () => {
    vi.useFakeTimers();
    const timeoutSpy = vi.spyOn(window, "setTimeout");

    expect(emitBurst(5, 5, { count: 0 })).toBe(0);
    expect(emitBurst(5, 5, { count: -3 })).toBe(0);

    expect(layerCount()).toBe(0);
    expect(activeParticleCount()).toBe(0);
    expect(timeoutSpy).not.toHaveBeenCalled();
  });

  it("respects deterministic random for count bounds and dot vs stroke geometry", () => {
    vi.useFakeTimers();

    expect(emitBurst(10, 20, { random: () => 0 })).toBe(6);
    const strokes = Array.from(particles()) as HTMLElement[];
    expect(strokes).toHaveLength(6);
    for (const el of strokes) {
      expect(el.style.width).toBe("5px");
      expect(el.style.height).toBe("1.5px");
      expect(el.style.background).toBe("rgb(181, 96, 74)");
    }
    vi.runAllTimers();
    expect(activeParticleCount()).toBe(0);

    expect(emitBurst(10, 20, { random: () => 0.875 })).toBe(12);
    const dots = Array.from(particles()) as HTMLElement[];
    expect(dots).toHaveLength(12);
    for (const el of dots) {
      expect(el.style.width).toBe("3.75px");
      expect(el.style.height).toBe("3.75px");
      expect(el.style.background).toBe("rgb(246, 240, 230)");
    }
    vi.runAllTimers();
    expect(activeParticleCount()).toBe(0);
  });

  it("animates dots and strokes via Element.animate and cancels the failsafe timer", () => {
    vi.useFakeTimers();
    const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
    const calls = stubElementAnimate();

    // baseAngle, then 6 values per particle (jitter, distance, life, color, stroke, size/length)
    const seq = [0, 0.5, 0, 0.5, 0, 0.5, 0.25, 0.5, 0.5, 0.75, 0.375, 0.125, 0.5];
    let idx = 0;
    const count = emitBurst(100, 200, { count: 2, random: () => seq[idx++] ?? 0.5 });

    expect(count).toBe(2);
    expect(calls).toHaveLength(2);
    expect(activeParticleCount()).toBe(2);
    expect(vi.getTimerCount()).toBe(1);

    const [dotCall, strokeCall] = calls;
    const dotEl = dotCall.node as HTMLElement;
    const strokeEl = strokeCall.node as HTMLElement;

    expect(dotEl.style.width).toBe("2.5px");
    expect(dotEl.style.height).toBe("2.5px");
    expect(dotCall.keyframes).toEqual([
      {
        transform: "translate(-50%, -50%) translate(0px, 0px)  scale(1)",
        opacity: 0.95,
      },
      {
        transform: "translate(-50%, -50%) translate(16px, 5px)  scale(0.35)",
        opacity: 0,
      },
    ]);
    expect(dotCall.options).toEqual({
      duration: 500,
      easing: "cubic-bezier(0.16, 0.8, 0.3, 1)",
      fill: "forwards",
    });

    expect(strokeEl.style.width).toBe("6.5px");
    expect(strokeEl.style.height).toBe("1.5px");
    const strokeDx = Math.cos(Math.PI) * 31;
    const strokeDy = Math.sin(Math.PI) * 31 + 5;
    const strokeMove = `translate(${strokeDx}px, ${strokeDy}px)`;
    expect(strokeCall.keyframes).toEqual([
      {
        transform: "translate(-50%, -50%) translate(0px, 0px) rotate(180deg) scale(1)",
        opacity: 0.95,
      },
      {
        transform: `translate(-50%, -50%) ${strokeMove} rotate(180deg) scale(0.35)`,
        opacity: 0,
      },
    ]);
    expect(strokeCall.options).toEqual({
      duration: 575,
      easing: "cubic-bezier(0.16, 0.8, 0.3, 1)",
      fill: "forwards",
    });

    expect(typeof dotCall.anim.onfinish).toBe("function");
    dotCall.anim.onfinish?.();
    expect(dotEl.isConnected).toBe(false);
    expect(strokeEl.isConnected).toBe(true);
    expect(activeParticleCount()).toBe(1);
    expect(layerCount()).toBe(1);
    expect(clearTimeoutSpy).not.toHaveBeenCalled();

    expect(typeof strokeCall.anim.oncancel).toBe("function");
    strokeCall.anim.oncancel?.();
    expect(strokeEl.isConnected).toBe(false);
    expect(activeParticleCount()).toBe(0);
    expect(layerCount()).toBe(0);
    expect(clearTimeoutSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("treats duplicate completion and post-failsafe callbacks as idempotent", () => {
    vi.useFakeTimers();
    const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
    const calls = stubElementAnimate();

    emitBurst(10, 20, { count: 2, random: () => 0.5 });
    emitBurst(30, 40, { count: 1, random: () => 0.5 });
    expect(calls).toHaveLength(3);
    expect(activeParticleCount()).toBe(3);
    expect(vi.getTimerCount()).toBe(2);

    // Releasing the first particle leaves both its burst timer and the shared layer active.
    calls[0].anim.onfinish?.();
    expect(activeParticleCount()).toBe(2);
    expect(layerCount()).toBe(1);
    expect(clearTimeoutSpy).not.toHaveBeenCalled();

    // Duplicate onfinish / oncancel on the same entry must not double-decrement or clear timers.
    calls[0].anim.onfinish?.();
    calls[0].anim.oncancel?.();
    expect(activeParticleCount()).toBe(2);
    expect(layerCount()).toBe(1);
    expect(calls[1].node.isConnected).toBe(true);
    expect(calls[2].node.isConnected).toBe(true);
    expect(clearTimeoutSpy).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(2);

    // Completing burst 1's second particle clears only burst 1's failsafe timer.
    calls[1].anim.oncancel?.();
    expect(activeParticleCount()).toBe(1);
    expect(layerCount()).toBe(1);
    expect(clearTimeoutSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);

    // Burst 2 is cleaned up by its failsafe timer; late animation callbacks are no-ops.
    vi.advanceTimersByTime(1000);
    expect(activeParticleCount()).toBe(0);
    expect(layerCount()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);

    calls[2].anim.onfinish?.();
    calls[2].anim.oncancel?.();
    expect(activeParticleCount()).toBe(0);
    expect(layerCount()).toBe(0);
    expect(clearTimeoutSpy).toHaveBeenCalledTimes(1);
  });

  it("recovers from a detached layer and isolates stale generation callbacks", () => {
    vi.useFakeTimers();
    const calls = stubElementAnimate();

    // Saturate active particles on generation 1 (350 ms life -> 470 ms failsafe).
    const emitted = emitBurst(10, 10, { count: MAX_ACTIVE_PARTICLES, random: () => 0 });
    expect(emitted).toBe(MAX_ACTIVE_PARTICLES);
    expect(activeParticleCount()).toBe(MAX_ACTIVE_PARTICLES);
    expect(emitBurst(10, 10, { count: 3 })).toBe(0);

    const staleLayer = document.getElementById("fx-layer") as HTMLElement;
    staleLayer.remove();
    expect(staleLayer.isConnected).toBe(false);

    // Next burst detects the detached layer, resets active count, and mounts a fresh layer.
    const emittedFresh = emitBurst(20, 30, { count: 3, random: () => 0.875 });
    expect(emittedFresh).toBe(3);
    const freshLayer = document.getElementById("fx-layer") as HTMLElement;
    expect(freshLayer).not.toBe(staleLayer);
    expect(freshLayer.isConnected).toBe(true);
    expect(layerCount()).toBe(1);
    expect(activeParticleCount()).toBe(3);
    expect(particles().length).toBe(3);

    // Stale animation callbacks from the detached layer must not alter the fresh layer.
    calls[0].anim.onfinish?.();
    calls[1].anim.oncancel?.();
    expect(activeParticleCount()).toBe(3);
    expect(freshLayer.isConnected).toBe(true);
    expect(particles().length).toBe(3);

    // Stale failsafe timer firing at 470 ms must also leave the fresh layer (733 ms) intact.
    vi.advanceTimersByTime(500);
    expect(activeParticleCount()).toBe(3);
    expect(freshLayer.isConnected).toBe(true);
    expect(particles().length).toBe(3);
    expect(vi.getTimerCount()).toBe(1);

    // Fresh layer's own animations still clean up cleanly to zero.
    const freshCalls = calls.slice(MAX_ACTIVE_PARTICLES);
    expect(freshCalls).toHaveLength(3);
    for (const call of freshCalls) call.anim.onfinish?.();
    expect(activeParticleCount()).toBe(0);
    expect(layerCount()).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });
});
