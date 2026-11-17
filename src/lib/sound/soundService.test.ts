import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_SOUND_SETTINGS,
  createSoundService,
  type SoundKind,
  type SoundSettings,
} from "./soundService";

// A just-enough fake of the Web Audio API that records what gets built and started.
function fakeAudio() {
  const rec = {
    contexts: 0,
    started: 0,
    resumed: 0,
    suspended: 0,
    disconnected: 0,
    startTimes: [] as number[],
    sources: [] as Array<{ onended: (() => void) | null }>,
    peaks: [] as number[],
    masterGain: 0,
  };
  const param = (isGain = false) => ({
    value: 0,
    setValueAtTime: vi.fn(),
    linearRampToValueAtTime: vi.fn((v: number) => {
      if (isGain) rec.peaks.push(v);
    }),
    exponentialRampToValueAtTime: vi.fn(),
    setTargetAtTime: vi.fn((v: number) => {
      rec.masterGain = v;
    }),
  });
  const node = (extra: object = {}) => {
    const n: Record<string, unknown> = {
      connect: (x: unknown) => x,
      disconnect: vi.fn(() => {
        rec.disconnected++;
      }),
      start: (when = 0) => {
        rec.started++;
        rec.startTimes.push(when);
      },
      stop: vi.fn(),
      ...extra,
    };
    return n;
  };
  const sourceNode = (extra: object = {}) => {
    const src = node({ onended: null, ...extra }) as unknown as { onended: (() => void) | null };
    rec.sources.push(src);
    return src;
  };
  const ctx = {
    sampleRate: 48000,
    currentTime: 0,
    state: "suspended" as AudioContextState,
    destination: node(),
    resume: () => {
      rec.resumed++;
      ctx.state = "running";
      return Promise.resolve();
    },
    suspend: () => {
      rec.suspended++;
      ctx.state = "suspended";
      return Promise.resolve();
    },
    createBuffer: (_c: number, length: number) => ({
      getChannelData: () => new Float32Array(length),
    }),
    createGain: () => node({ gain: param(true) }),
    createDynamicsCompressor: () => node(),
    createBufferSource: () => sourceNode({ buffer: null }),
    createBiquadFilter: () => node({ frequency: param(), Q: { value: 0 }, type: "" }),
    createOscillator: () => sourceNode({ frequency: param(), type: "" }),
  };
  return {
    rec,
    ctx,
    createContext: () => {
      rec.contexts++;
      return ctx as unknown as AudioContext;
    },
  };
}

function setup(clockStart = 1000, extra: Partial<Parameters<typeof createSoundService>[0]> = {}) {
  const audio = fakeAudio();
  let t = clockStart;
  const persist = vi.fn<(s: SoundSettings) => void>();
  const svc = createSoundService({
    createContext: audio.createContext,
    now: () => t,
    persist,
    ...extra,
  });
  return { audio, svc, persist, advance: (ms: number) => (t += ms) };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("sound service", () => {
  it("defaults to enabled at a restrained volume", () => {
    expect(DEFAULT_SOUND_SETTINGS).toEqual({ enabled: true, volume: 0.5 });
    expect(setup().svc.getSettings()).toEqual({ enabled: true, volume: 0.5 });
  });

  it("creates no audio until the first user-triggered play, then reuses one context", () => {
    const { audio, svc, advance } = setup();
    expect(audio.rec.contexts).toBe(0);
    expect(svc.play("click")).toBe(true);
    advance(500);
    svc.play("primary");
    expect(audio.rec.contexts).toBe(1);
    expect(audio.rec.resumed).toBeGreaterThan(0); // autoplay policy: resumed inside the gesture
  });

  it("plays every kind, each with audible voices but only quiet peaks", () => {
    const kinds: SoundKind[] = ["click", "primary", "headline", "select"];
    const { audio, svc, advance } = setup();
    for (const k of kinds) {
      const before = audio.rec.started;
      expect(svc.play(k)).toBe(true);
      expect(audio.rec.started).toBeGreaterThan(before);
      advance(1000);
    }
    // Conservative: no single voice envelope peaks above 0.2 before the master volume applies.
    expect(Math.max(...audio.rec.peaks)).toBeLessThanOrEqual(0.2);
    // Master gain stays well below unity even at full volume.
    svc.setSettings({ volume: 1 });
    svc.play("click");
    expect(audio.rec.masterGain).toBeLessThanOrEqual(0.5);
  });

  it("throttles rapid input so sounds never pile up", () => {
    const { audio, svc, advance } = setup();
    let played = 0;
    for (let i = 0; i < 50; i++) {
      if (svc.play("click")) played++;
      advance(10); // 100 clicks per second
    }
    expect(played).toBeLessThanOrEqual(4); // one per 140 ms in half a second
    expect(svc.stats().throttled).toBeGreaterThan(40);
    // Different kinds closer than the global gap are dropped too.
    const { svc: s2, advance: adv2 } = setup();
    expect(s2.play("click")).toBe(true);
    adv2(20);
    expect(s2.play("select")).toBe(false);

    // At most 3 sounds ring at once: headline (170 ms), primary (130 ms) and select (140 ms)
    // overlap, so a 4th sound at +168 ms is refused until one of them has ended.
    const { svc: s3, advance: adv3 } = setup();
    expect(s3.play("headline")).toBe(true);
    adv3(56);
    expect(s3.play("primary")).toBe(true);
    adv3(56);
    expect(s3.play("select")).toBe(true);
    adv3(56);
    expect(s3.play("click")).toBe(false);
    adv3(60);
    expect(s3.play("click")).toBe(true);
    expect(audio.rec.contexts).toBe(1);
  });

  it("is silent when disabled or at zero volume, and resumes when re-enabled", () => {
    const { audio, svc, advance } = setup();
    svc.setSettings({ enabled: false });
    expect(svc.play("click")).toBe(false);
    svc.setSettings({ enabled: true, volume: 0 });
    expect(svc.play("click")).toBe(false);
    expect(audio.rec.contexts).toBe(0);
    svc.setSettings({ volume: 0.6 });
    advance(500);
    expect(svc.play("click")).toBe(true);
  });

  it("reports every change to persist (the launcher's settings file), clamps bad values, and notifies subscribers", () => {
    const { svc, persist } = setup();
    const seen: number[] = [];
    const off = svc.subscribe((s) => seen.push(s.volume));
    svc.setSettings({ volume: 5 });
    svc.setSettings({ volume: -2 });
    svc.setSettings({ volume: Number.NaN });
    off();
    svc.setSettings({ volume: 0.3, enabled: false });
    expect(seen).toEqual([1, 0, 0.5]);
    expect(persist.mock.calls.map(([s]) => s)).toEqual([
      { enabled: true, volume: 1 },
      { enabled: true, volume: 0 },
      { enabled: true, volume: 0.5 },
      { enabled: false, volume: 0.3 },
    ]);
  });

  it("starts from the saved settings it is given, repairing bad values", () => {
    expect(setup(1000, { initial: { enabled: false, volume: 0.2 } }).svc.getSettings()).toEqual({
      enabled: false,
      volume: 0.2,
    });
    expect(
      setup(1000, {
        initial: { enabled: "yes", volume: 9 } as unknown as SoundSettings,
      }).svc.getSettings(),
    ).toEqual({ enabled: true, volume: 1 });
    expect(setup(1000, { initial: undefined }).svc.getSettings()).toEqual(DEFAULT_SOUND_SETTINGS);
  });

  it("hydrate applies saved settings (startup or a backend change event) without saving them back", () => {
    const { svc, persist, audio, advance } = setup();
    const seen: SoundSettings[] = [];
    svc.subscribe((s) => seen.push(s));
    svc.hydrate({ enabled: false, volume: 0.9 });
    expect(svc.getSettings()).toEqual({ enabled: false, volume: 0.9 });
    expect(persist).not.toHaveBeenCalled(); // no echo loop with the backend event
    expect(seen).toEqual([{ enabled: false, volume: 0.9 }]);
    expect(svc.play("click")).toBe(false); // the hydrated "off" is honoured immediately
    svc.hydrate({ enabled: true, volume: 7 });
    expect(svc.getSettings()).toEqual({ enabled: true, volume: 1 });
    advance(500);
    expect(svc.play("click")).toBe(true);
    expect(audio.rec.masterGain).toBeLessThanOrEqual(0.5);
  });

  it("survives a failing save and a missing or broken AudioContext", () => {
    const svc = createSoundService({
      createContext: () => null,
      persist: () => {
        throw new Error("backend unavailable");
      },
    });
    expect(() => svc.setSettings({ volume: 0.2 })).not.toThrow();
    expect(svc.getSettings().volume).toBe(0.2); // still applies for this session
    expect(svc.play("click")).toBe(false); // no audio available: quietly does nothing
    const broken = createSoundService({
      createContext: () => {
        throw new Error("no audio device");
      },
    });
    expect(broken.play("click")).toBe(false);
  });

  it("does not use any WebView storage", () => {
    const getItem = vi.spyOn(Storage.prototype, "getItem");
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    const { svc, advance } = setup();
    svc.setSettings({ volume: 0.4 });
    svc.hydrate({ enabled: true, volume: 0.6 });
    advance(500);
    svc.play("click");
    expect(getItem).not.toHaveBeenCalled();
    expect(setItem).not.toHaveBeenCalled();
  });

  it("never touches the network", () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);
    const { svc, advance } = setup();
    for (const k of ["click", "primary", "headline", "select"] as SoundKind[]) {
      svc.play(k);
      advance(1000);
    }
    expect(fetchSpy).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("auto-suspends after silence and extends the idle timer when a new sound plays", () => {
    vi.useFakeTimers();
    const { audio, svc, advance } = setup();
    expect(svc.play("click")).toBe(true); // DURATION.click = 70 ms -> idle after 1570 ms
    expect(audio.rec.suspended).toBe(0);

    vi.advanceTimersByTime(1000);
    advance(1000);
    expect(audio.rec.suspended).toBe(0);

    // Playing another sound resets the idle window so the earlier timer does not cut it off.
    expect(svc.play("headline")).toBe(true); // DURATION.headline = 170 ms -> idle after 1670 ms
    vi.advanceTimersByTime(1000);
    advance(1000);
    expect(audio.rec.suspended).toBe(0);

    vi.advanceTimersByTime(670);
    advance(670);
    expect(audio.rec.suspended).toBe(1);
    expect(audio.ctx.state).toBe("suspended");
  });

  it("suspends immediately when disabled or set to zero volume", () => {
    vi.useFakeTimers();
    const { audio, svc, advance } = setup();
    expect(svc.play("click")).toBe(true);
    expect(audio.ctx.state).toBe("running");

    svc.setSettings({ enabled: false });
    expect(audio.rec.suspended).toBe(1);
    expect(audio.ctx.state).toBe("suspended");

    svc.setSettings({ enabled: true, volume: 0.5 });
    advance(500);
    expect(svc.play("click")).toBe(true);
    expect(audio.ctx.state).toBe("running");

    svc.setSettings({ volume: 0 });
    expect(audio.rec.suspended).toBe(2);
    expect(audio.ctx.state).toBe("suspended");

    // Cleared idle timer does not suspend again later.
    vi.advanceTimersByTime(3000);
    expect(audio.rec.suspended).toBe(2);
  });

  it("disconnects source, filter and envelope gain nodes when voices finish", () => {
    const { audio, svc, advance } = setup();
    expect(svc.play("click")).toBe(true); // 1 buffer source + 1 oscillator = 2 chains (6 nodes)
    expect(audio.rec.sources).toHaveLength(2);
    expect(audio.rec.disconnected).toBe(0);

    for (const src of audio.rec.sources) {
      expect(typeof src.onended).toBe("function");
      src.onended?.();
    }
    // Each voice chain disconnects (source/osc + filter + gain) = 3 nodes * 2 voices = 6.
    expect(audio.rec.disconnected).toBe(6);

    // Master chain remains intact for subsequent sounds.
    advance(500);
    expect(svc.play("select")).toBe(true);
    expect(audio.rec.sources).toHaveLength(4);
  });

  it("schedules voices relative to post-resume currentTime when waking an async suspended context", async () => {
    const audio = fakeAudio();
    let resolveResume!: () => void;
    audio.ctx.resume = () => {
      audio.rec.resumed++;
      return new Promise<void>((resolve) => {
        resolveResume = () => {
          audio.ctx.state = "running";
          audio.ctx.currentTime = 4.25;
          resolve();
        };
      });
    };

    const svc = createSoundService({ createContext: audio.createContext });
    expect(svc.play("click")).toBe(true);
    expect(audio.rec.resumed).toBe(1);
    expect(audio.rec.started).toBe(0); // waiting for hardware resume

    resolveResume();
    await Promise.resolve();
    expect(audio.rec.started).toBe(2);
    expect(audio.rec.startTimes[0]).toBeCloseTo(4.255, 5);
  });
});
