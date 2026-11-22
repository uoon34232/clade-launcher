import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BASE_PHRASES, phraseText } from "@/features/launcher/phrases";

const emitBurst = vi.fn<(...args: unknown[]) => number>(() => 7);
const playSound = vi.fn<(...args: unknown[]) => boolean>(() => true);
const saveLastPhrase = vi.fn<(id: string) => void>();
vi.mock("@/lib/fx/burst", () => ({ emitBurst: (...a: unknown[]) => emitBurst(...a) }));
vi.mock("@/lib/sound/soundService", () => ({ playSound: (...a: unknown[]) => playSound(...a) }));
vi.mock("@/lib/settings/uiSettings", () => ({
  saveLastPhrase: (id: string) => saveLastPhrase(id),
}));

import { Hero } from "./Hero";

const settle = async () => act(async () => void (await Promise.resolve()));
const texts = BASE_PHRASES.map((p) => phraseText(p));
const heading = () => screen.getByRole("heading", { level: 1 });
const headline = () => heading().textContent ?? "";

beforeEach(() => {
  emitBurst.mockClear();
  playSound.mockClear();
  saveLastPhrase.mockClear();
});
afterEach(() => {
  vi.useRealTimers();
  cleanup();
  vi.restoreAllMocks();
  delete (HTMLElement.prototype as { animate?: unknown }).animate;
  delete (window as { matchMedia?: unknown }).matchMedia;
});

describe("Hero headline without animation support (instant swap)", () => {
  it("starts with a phrase from the pool, inside a native button within the h1", () => {
    render(<Hero />);
    expect(texts).toContain(headline());
    const button = heading().querySelector("button");
    expect(button).not.toBeNull();
    expect(button?.textContent).toBe(headline());
    expect(button?.getAttribute("type")).toBe("button");
  });

  it("never starts with the phrase saved by the previous launch", () => {
    for (const p of BASE_PHRASES) {
      for (let i = 0; i < 20; i++) {
        cleanup();
        render(<Hero lastPhraseId={p.id} />);
        expect(headline()).not.toBe(phraseText(p));
      }
    }
  });

  it("asks the launcher's settings service to remember whatever is on screen, debouncing shuffles", () => {
    vi.useFakeTimers();
    render(<Hero />);
    const shown = BASE_PHRASES.find((p) => phraseText(p) === headline());
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
    expect(saveLastPhrase).toHaveBeenLastCalledWith(shown?.id);

    const button = heading().querySelector("button") as HTMLButtonElement;
    fireEvent.click(button, { detail: 1 });
    const next = BASE_PHRASES.find((p) => phraseText(p) === headline());
    expect(next?.id).not.toBe(shown?.id);

    // Debounced by 800 ms while shuffling.
    vi.advanceTimersByTime(799);
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(saveLastPhrase).toHaveBeenCalledTimes(2);
    expect(saveLastPhrase).toHaveBeenLastCalledWith(next?.id);

    // Rapid shuffles coalesce into at most one save (and skip if landing back on the last saved id).
    saveLastPhrase.mockClear();
    for (let i = 0; i < 5; i++) {
      if (i > 0) vi.advanceTimersByTime(200);
      fireEvent.click(button, { detail: 1 });
    }
    let final = BASE_PHRASES.find((p) => phraseText(p) === headline());
    if (final?.id === next?.id) {
      // Landing back on the already-saved phrase skips redundant IPC even after 800 ms.
      vi.advanceTimersByTime(800);
      expect(saveLastPhrase).not.toHaveBeenCalled();
      fireEvent.click(button, { detail: 1 });
      final = BASE_PHRASES.find((p) => phraseText(p) === headline());
    }
    expect(saveLastPhrase).not.toHaveBeenCalled();
    vi.advanceTimersByTime(799);
    expect(saveLastPhrase).toHaveBeenCalledTimes(0);
    vi.advanceTimersByTime(1);
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
    expect(saveLastPhrase).toHaveBeenLastCalledWith(final?.id);
  });

  it("flushes a pending debounced phrase save synchronously on unmount", () => {
    vi.useFakeTimers();
    const { unmount } = render(<Hero />);
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
    saveLastPhrase.mockClear();

    const button = heading().querySelector("button") as HTMLButtonElement;
    fireEvent.click(button, { detail: 1 });
    const next = BASE_PHRASES.find((p) => phraseText(p) === headline());

    // Still inside the 800 ms debounce window: no IPC yet.
    vi.advanceTimersByTime(400);
    expect(saveLastPhrase).not.toHaveBeenCalled();

    // Unmounting mid-debounce flushes the pending phrase immediately and cancels the timer.
    unmount();
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
    expect(saveLastPhrase).toHaveBeenLastCalledWith(next?.id);

    vi.advanceTimersByTime(1000);
    expect(saveLastPhrase).toHaveBeenCalledTimes(1);
  });

  it("does not touch any WebView storage", () => {
    const get = vi.spyOn(Storage.prototype, "getItem");
    const set = vi.spyOn(Storage.prototype, "setItem");
    render(<Hero lastPhraseId="welcome-back" />);
    fireEvent.click(heading().querySelector("button") as HTMLButtonElement, { detail: 1 });
    expect(get).not.toHaveBeenCalled();
    expect(set).not.toHaveBeenCalled();
  });

  it("changes to a different phrase on every click, never the same twice in a row", () => {
    render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    let previous = headline();
    for (let i = 0; i < 60; i++) {
      fireEvent.click(button, { clientX: 100, clientY: 50, detail: 1 });
      const now = headline();
      expect(texts).toContain(now);
      expect(now).not.toBe(previous);
      previous = now;
    }
  });

  it("bursts from the exact cursor position and plays the headline sound on a mouse click", () => {
    render(<Hero />);
    fireEvent.click(heading().querySelector("button") as HTMLButtonElement, {
      clientX: 321,
      clientY: 123,
      detail: 1,
    });
    expect(emitBurst).toHaveBeenCalledTimes(1);
    expect(emitBurst.mock.calls[0].slice(0, 2)).toEqual([321, 123]);
    expect(playSound).toHaveBeenCalledWith("headline");
  });

  it("treats keyboard activation (a click with detail 0) the same: new phrase, center burst, sound", () => {
    render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    const textSpan = button.querySelector(".hero-text") as HTMLSpanElement;
    vi.spyOn(textSpan, "getBoundingClientRect").mockReturnValue({
      left: 100,
      top: 40,
      width: 200,
      height: 50,
      right: 300,
      bottom: 90,
      x: 100,
      y: 40,
      toJSON: () => ({}),
    });
    const before = headline();
    act(() => button.click()); // what Enter/Space produce in a browser
    expect(headline()).not.toBe(before);
    expect(emitBurst).toHaveBeenCalledTimes(1);
    expect(emitBurst.mock.calls[0].slice(0, 2)).toEqual([200, 65]);
    expect(playSound).toHaveBeenCalledWith("headline");
  });

  it("is focusable and exposes a description without adding visible chrome", () => {
    render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    button.focus();
    expect(document.activeElement).toBe(button);
    expect(button.tabIndex).toBe(0);
    expect(button.getAttribute("aria-description")).toMatch(/different message/i);
    expect(button.className).toBe("hero-link");
  });

  it("hides playtime when zero and shows formatted playtime when positive", () => {
    const { container, rerender } = render(<Hero playtimeSeconds={0} />);
    const playtime = container.querySelectorAll("p")[1];
    expect(playtime.getAttribute("aria-hidden")).toBe("true");
    expect(playtime.className).toContain("opacity-0");
    expect(playtime.className).not.toContain("opacity-85");
    expect(playtime.textContent?.trim()).toBe("");

    rerender(<Hero playtimeSeconds={3720} />);
    expect(playtime.getAttribute("aria-hidden")).toBeNull();
    expect(playtime.className).toContain("opacity-85");
    expect(playtime.className).not.toContain("opacity-0");
    expect(playtime.textContent).toBe("Played 1h 2m");
  });

  it("applies entrance and returning classes correctly", () => {
    const { container, rerender } = render(<Hero />);
    const root = container.firstElementChild as HTMLElement;
    expect(root.classList.contains("hero-rise")).toBe(true);
    expect(root.classList.contains("hero-return")).toBe(false);

    rerender(<Hero returning />);
    expect(root.classList.contains("hero-return")).toBe(true);
    expect(root.classList.contains("hero-rise")).toBe(false);

    rerender(<Hero entrance="settled" returning />);
    expect(root.classList.contains("hero-rise")).toBe(false);
    expect(root.classList.contains("hero-return")).toBe(false);

    rerender(<Hero entrance="return" />);
    expect(root.classList.contains("hero-return")).toBe(true);
    expect(root.classList.contains("hero-rise")).toBe(false);

    rerender(<Hero entrance="rise" returning />);
    expect(root.classList.contains("hero-rise")).toBe(true);
    expect(root.classList.contains("hero-return")).toBe(false);
  });
});

describe("Hero transition lock (with a controllable Web Animations API)", () => {
  type Fake = {
    target: Element;
    resolve: () => void;
    reject: (reason?: unknown) => void;
    cancelled: boolean;
  };
  let fakes: Fake[];

  beforeEach(() => {
    fakes = [];
    (HTMLElement.prototype as { animate?: unknown }).animate = function (this: Element) {
      let resolve!: () => void;
      let reject!: (reason?: unknown) => void;
      let settled = false;
      let finishedAccessed = false;
      const finished = new Promise<void>((res, rej) => {
        resolve = () => {
          if (settled) return;
          settled = true;
          res();
        };
        reject = (reason?: unknown) => {
          if (settled) return;
          settled = true;
          rej(reason instanceof Error ? reason : new Error(String(reason)));
        };
      });
      const entry: Fake = {
        target: this,
        resolve: () => resolve(),
        reject: (reason = new DOMException("Animation cancelled", "AbortError")) => reject(reason),
        cancelled: false,
      };
      fakes.push(entry);
      return {
        get finished() {
          finishedAccessed = true;
          return finished;
        },
        cancel: () => {
          entry.cancelled = true;
          if (finishedAccessed && !settled) entry.reject();
        },
        onfinish: null,
        oncancel: null,
      };
    };
  });

  const textAnimations = () => fakes.filter((f) => f.target.classList.contains("hero-text"));

  it("absorbs rapid clicks during a transition, then accepts the next one afterwards", async () => {
    render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    const first = headline();

    for (let i = 0; i < 8; i++) fireEvent.click(button, { clientX: 5, clientY: 5, detail: 1 });
    expect(textAnimations()).toHaveLength(1); // one fade-out only, not eight
    expect(headline()).toBe(first); // text swaps only once the fade-out has finished

    // Visual feedback is still immediate for every click (burst + sound), but bounded elsewhere.
    expect(emitBurst).toHaveBeenCalledTimes(8);

    act(() => textAnimations()[0].resolve()); // fade-out done: swap + fade-in starts
    await settle();
    const second = headline();
    expect(second).not.toBe(first);
    expect(textAnimations()).toHaveLength(2);
    expect(textAnimations()[0].cancelled).toBe(true); // the forwards-filled fade-out was released

    fireEvent.click(button, { detail: 1 }); // still fading in: ignored
    expect(textAnimations()).toHaveLength(2);

    act(() => textAnimations()[1].resolve()); // fade-in done
    await settle();
    fireEvent.click(button, { detail: 1 });
    expect(textAnimations()).toHaveLength(3); // a new transition begins
  });

  it("cancels active animations and settles safely when unmounted mid-fade-out", async () => {
    const { unmount } = render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    const first = headline();

    fireEvent.click(button, { detail: 1 });
    expect(textAnimations()).toHaveLength(1);
    expect(headline()).toBe(first);

    unmount();
    await settle();
    expect(textAnimations()[0].cancelled).toBe(true);
    expect(fakes.every((f) => f.cancelled)).toBe(true);
  });

  it("resets the transition lock if fade-out or fade-in animation rejects", async () => {
    render(<Hero />);
    const button = heading().querySelector("button") as HTMLButtonElement;
    const first = headline();

    // Fade-out rejected: phrase stays unchanged and busy resets for the next click.
    fireEvent.click(button, { detail: 1 });
    expect(textAnimations()).toHaveLength(1);
    act(() => textAnimations()[0].reject(new DOMException("Aborted", "AbortError")));
    await settle();
    expect(headline()).toBe(first);

    fireEvent.click(button, { detail: 1 });
    expect(textAnimations()).toHaveLength(2);
    act(() => textAnimations()[1].resolve());
    await settle();
    const second = headline();
    expect(second).not.toBe(first);
    expect(textAnimations()).toHaveLength(3);

    // Fade-in rejected: busy resets so another click starts a new transition.
    act(() => textAnimations()[2].reject(new DOMException("Aborted", "AbortError")));
    await settle();
    fireEvent.click(button, { detail: 1 });
    expect(textAnimations()).toHaveLength(4);
  });

  it("uses a fade out then a fade in with small vertical movement, totalling about 400 ms", async () => {
    const calls: { frames: Keyframe[]; options: KeyframeAnimationOptions }[] = [];
    (HTMLElement.prototype as { animate?: unknown }).animate = function (
      this: Element,
      frames: Keyframe[],
      options: KeyframeAnimationOptions,
    ) {
      if (this.classList.contains("hero-text")) calls.push({ frames, options });
      return { finished: Promise.resolve(), cancel: () => {}, onfinish: null, oncancel: null };
    };
    render(<Hero />);
    fireEvent.click(heading().querySelector("button") as HTMLButtonElement, { detail: 1 });
    await settle();
    await settle();
    const [out, inn] = calls;
    expect(out.frames[1]).toMatchObject({ opacity: 0, transform: "translateY(-6px)" });
    expect(inn.frames[0]).toMatchObject({ opacity: 0, transform: "translateY(8px)" });
    const total = Number(out.options.duration) + Number(inn.options.duration);
    expect(total).toBeGreaterThanOrEqual(300);
    expect(total).toBeLessThanOrEqual(450);
  });

  it("under reduced motion still changes the headline (a short fade) but skips movement, press and particles", async () => {
    window.matchMedia = ((q: string) => ({
      matches: q.includes("reduce"),
      media: q,
      addEventListener() {},
      removeEventListener() {},
    })) as unknown as typeof window.matchMedia;
    const frames: Keyframe[][] = [];
    (HTMLElement.prototype as { animate?: unknown }).animate = function (
      this: Element,
      f: Keyframe[],
    ) {
      frames.push(f);
      return { finished: Promise.resolve(), cancel: () => {}, onfinish: null, oncancel: null };
    };
    render(<Hero />);
    const before = headline();
    fireEvent.click(heading().querySelector("button") as HTMLButtonElement, { detail: 1 });
    await settle();
    await settle();
    expect(headline()).not.toBe(before);
    expect(frames.every((f) => f.every((k) => !("transform" in k) && !("scale" in k)))).toBe(true);
    expect(emitBurst.mock.calls[0][2]).toMatchObject({ reducedMotion: true });
  });
});
