// Launcher UI sounds, synthesised on the fly with the Web Audio API.
//
// An original sound identity: soft, dry and restrained, a short filtered-noise "tick" with a
// tiny warm body underneath, closer to a felt-padded key than to a game. No audio files, no
// network, nothing installed on the system and nothing written outside the launcher folder.
// Preferences are owned by the launcher's Rust settings service (`data/settings.json`), not by
// the WebView: this service receives them through `hydrate` and reports changes through the
// injected `persist` callback.
//
// Rules: sounds play only as a direct result of a user action (the caller invokes `play` from an
// event handler; the AudioContext is created lazily on the first one, which also satisfies the
// browser autoplay policy), never on hover, always quiet, and rapid input is throttled so
// sounds never pile up.

import { saveSoundSettings } from "@/lib/settings/uiSettings";

export type SoundKind = "click" | "primary" | "headline" | "select";

export interface SoundSettings {
  /** Master switch ("Sound effects"). */
  enabled: boolean;
  /** UI sound volume, 0 to 1. Mapped onto a conservative gain ceiling. */
  volume: number;
}

export const DEFAULT_SOUND_SETTINGS: Readonly<SoundSettings> = { enabled: true, volume: 0.5 };

/** Volume 1 maps to this gain; every voice is already quiet, so the loudest setting is still gentle. */
const MASTER_CEILING = 0.45;
/** Minimum gap between any two sounds, and between two of the same kind (ms). */
const MIN_GAP_MS = 55;
const SAME_KIND_GAP_MS = 140;
/** At most this many sounds may ring at once. */
const MAX_VOICES = 3;
/** Silence after the last sound ends before suspending the AudioContext so idle audio threads sleep. */
const IDLE_SUSPEND_MS = 1500;

export interface SoundStats {
  played: number;
  throttled: number;
}

export interface SoundServiceOptions {
  /** Returns an AudioContext, or null where audio is unavailable. Injectable for tests. */
  createContext?: () => AudioContext | null;
  now?: () => number;
  /** Starting preferences (normally what the launcher's settings file holds). */
  initial?: SoundSettings;
  /** Called after every user-initiated settings change, to save it. Failures are swallowed. */
  persist?: (settings: SoundSettings) => void;
}

export interface SoundService {
  play(kind: SoundKind): boolean;
  getSettings(): SoundSettings;
  setSettings(patch: Partial<SoundSettings>): SoundSettings;
  /** Replaces the preferences with values that came from the saved settings (not persisted again). */
  hydrate(settings: SoundSettings): void;
  subscribe(listener: (s: SoundSettings) => void): () => void;
  stats(): SoundStats;
}

function clamp01(n: number): number {
  return Number.isFinite(n) ? Math.min(1, Math.max(0, n)) : DEFAULT_SOUND_SETTINGS.volume;
}

function defaultContext(): AudioContext | null {
  const Ctor = (globalThis as { AudioContext?: typeof AudioContext }).AudioContext;
  return Ctor ? new Ctor({ latencyHint: "interactive" }) : null;
}

// --------------------------------------------------------------------------- voices
// All peak levels are small on purpose; `out` applies the user volume and a gentle compressor.

function noiseBuffer(ctx: AudioContext): AudioBuffer {
  const length = Math.floor(ctx.sampleRate * 0.12);
  const buffer = ctx.createBuffer(1, length, ctx.sampleRate);
  const data = buffer.getChannelData(0);
  for (let i = 0; i < length; i++) data[i] = Math.random() * 2 - 1;
  return buffer;
}

function env(ctx: AudioContext, t: number, peak: number, attack: number, decay: number): GainNode {
  const g = ctx.createGain();
  g.gain.setValueAtTime(0.0001, t);
  g.gain.linearRampToValueAtTime(peak, t + attack);
  g.gain.exponentialRampToValueAtTime(0.0001, t + attack + decay);
  return g;
}

function disconnectNodes(...nodes: AudioNode[]) {
  for (const node of nodes) {
    if (typeof node.disconnect === "function") {
      try {
        node.disconnect();
      } catch {
        /* already disconnected */
      }
    }
  }
}

function tick(
  ctx: AudioContext,
  out: AudioNode,
  noise: AudioBuffer,
  t: number,
  freq: number,
  q: number,
  peak: number,
  decay: number,
  sweepTo?: number,
) {
  const src = ctx.createBufferSource();
  src.buffer = noise;
  const bp = ctx.createBiquadFilter();
  bp.type = "bandpass";
  bp.frequency.setValueAtTime(freq, t);
  if (sweepTo) bp.frequency.exponentialRampToValueAtTime(sweepTo, t + decay);
  bp.Q.value = q;
  const g = env(ctx, t, peak, 0.002, decay);
  src.connect(bp).connect(g).connect(out);
  src.onended = () => disconnectNodes(src, bp, g);
  src.start(t);
  src.stop(t + decay + 0.02);
}

function tone(
  ctx: AudioContext,
  out: AudioNode,
  t: number,
  from: number,
  to: number,
  peak: number,
  attack: number,
  decay: number,
  lowpass = 1400,
) {
  const osc = ctx.createOscillator();
  osc.type = "sine";
  osc.frequency.setValueAtTime(from, t);
  osc.frequency.exponentialRampToValueAtTime(to, t + attack + decay);
  const lp = ctx.createBiquadFilter();
  lp.type = "lowpass";
  lp.frequency.value = lowpass;
  const g = env(ctx, t, peak, attack, decay);
  osc.connect(lp).connect(g).connect(out);
  osc.onended = () => disconnectNodes(osc, lp, g);
  osc.start(t);
  osc.stop(t + attack + decay + 0.02);
}

/** Total length in seconds of each voice, used to track how many are ringing. */
const DURATION: Record<SoundKind, number> = {
  click: 0.07,
  primary: 0.13,
  headline: 0.17,
  select: 0.14,
};

const VOICES: Record<
  SoundKind,
  (ctx: AudioContext, out: AudioNode, noise: AudioBuffer, t: number) => void
> = {
  // Everyday control: a dry, soft tick with a small low "thock".
  click(ctx, out, noise, t) {
    tick(ctx, out, noise, t, 2600, 0.9, 0.1, 0.028);
    tone(ctx, out, t, 210, 130, 0.09, 0.002, 0.05, 900);
  },
  // Primary action (Play): a little deeper and rounder, still short.
  primary(ctx, out, noise, t) {
    tick(ctx, out, noise, t, 1800, 0.9, 0.09, 0.035);
    tone(ctx, out, t, 150, 88, 0.16, 0.003, 0.095, 800);
    tone(ctx, out, t, 300, 176, 0.03, 0.003, 0.06, 1200);
  },
  // Headline: airy and gentle, a soft breath with a faint pluck.
  headline(ctx, out, noise, t) {
    tick(ctx, out, noise, t, 900, 1.2, 0.07, 0.13, 2000);
    tone(ctx, out, t, 440, 330, 0.035, 0.004, 0.1, 1500);
  },
  // Selection confirmed: two soft rising notes, a quiet "got it".
  select(ctx, out, _noise, t) {
    tone(ctx, out, t, 392, 380, 0.08, 0.002, 0.06, 2400);
    tone(ctx, out, t + 0.045, 523, 510, 0.07, 0.002, 0.08, 2400);
  },
};

// --------------------------------------------------------------------------- service

export function createSoundService(options: SoundServiceOptions = {}): SoundService {
  const createContext = options.createContext ?? defaultContext;
  const now = options.now ?? (() => performance.now());

  const normalize = (s: Partial<SoundSettings> | undefined): SoundSettings => ({
    enabled: typeof s?.enabled === "boolean" ? s.enabled : DEFAULT_SOUND_SETTINGS.enabled,
    volume: s?.volume === undefined ? DEFAULT_SOUND_SETTINGS.volume : clamp01(s.volume),
  });
  let settings: SoundSettings = normalize(options.initial);

  const listeners = new Set<(s: SoundSettings) => void>();
  const counters: SoundStats = { played: 0, throttled: 0 };
  const lastByKind = new Map<SoundKind, number>();
  let lastAny = Number.NEGATIVE_INFINITY;
  let ringingUntil: number[] = [];

  let ctx: AudioContext | null = null;
  let master: GainNode | null = null;
  let noise: AudioBuffer | null = null;
  let idleTimer: ReturnType<typeof setTimeout> | null = null;
  let resumeInFlight = false;

  function clearIdleSuspend() {
    if (idleTimer !== null) {
      clearTimeout(idleTimer);
      idleTimer = null;
    }
  }

  function suspendContext() {
    clearIdleSuspend();
    if (ctx && ctx.state === "running" && typeof ctx.suspend === "function") {
      void ctx.suspend().catch(() => undefined);
    }
  }

  function scheduleIdleSuspend(durationSec: number) {
    clearIdleSuspend();
    idleTimer = setTimeout(
      () => {
        idleTimer = null;
        if (ctx && ctx.state === "running" && typeof ctx.suspend === "function") {
          void ctx.suspend().catch(() => undefined);
        }
      },
      durationSec * 1000 + IDLE_SUSPEND_MS,
    );
  }

  function ensureAudio(): boolean {
    if (ctx && master && noise) return true;
    try {
      ctx = createContext();
      if (!ctx) return false;
      master = ctx.createGain();
      const initialGain = settings.volume * MASTER_CEILING;
      master.gain.value = initialGain;
      master.gain.setValueAtTime(initialGain, ctx.currentTime);
      const compressor = ctx.createDynamicsCompressor();
      master.connect(compressor).connect(ctx.destination);
      noise = noiseBuffer(ctx);
      return true;
    } catch {
      ctx = null;
      master = null;
      return false; // audio is a nicety: never let it break the UI
    }
  }

  function applyVolume() {
    if (ctx && master)
      master.gain.setTargetAtTime(settings.volume * MASTER_CEILING, ctx.currentTime, 0.01);
  }

  return {
    play(kind) {
      if (!settings.enabled || settings.volume <= 0) return false;
      const t = now();
      ringingUntil = ringingUntil.filter((end) => end > t);
      const sameKind = lastByKind.get(kind) ?? Number.NEGATIVE_INFINITY;
      if (
        t - lastAny < MIN_GAP_MS ||
        t - sameKind < SAME_KIND_GAP_MS ||
        ringingUntil.length >= MAX_VOICES
      ) {
        counters.throttled++;
        return false;
      }
      if (!ensureAudio() || !ctx || !master || !noise) return false;
      if (resumeInFlight) return false;
      const scheduleVoice = (): boolean => {
        if (!ctx || !master || !noise) return false;
        if (!settings.enabled || settings.volume <= 0) {
          suspendContext();
          return false;
        }
        applyVolume();
        VOICES[kind](ctx, master, noise, ctx.currentTime + 0.005);
        scheduleIdleSuspend(DURATION[kind]);
        return true;
      };
      try {
        if (ctx.state === "suspended") {
          clearIdleSuspend();
          resumeInFlight = true;
          let resuming: Promise<void> | void;
          try {
            resuming = ctx.resume();
          } catch (err) {
            resumeInFlight = false;
            throw err;
          }
          if ((ctx.state as AudioContextState) === "running") {
            resumeInFlight = false;
            void Promise.resolve(resuming).catch(() => undefined);
            if (!scheduleVoice()) return false;
          } else {
            void Promise.resolve(resuming).then(
              () => {
                resumeInFlight = false;
                try {
                  scheduleVoice();
                } catch {
                  /* audio is a nicety: never let it break the UI */
                }
              },
              () => {
                resumeInFlight = false;
              },
            );
          }
        } else {
          if (!scheduleVoice()) return false;
        }
      } catch {
        return false;
      }
      lastAny = t;
      lastByKind.set(kind, t);
      ringingUntil.push(t + DURATION[kind] * 1000);
      counters.played++;
      return true;
    },

    getSettings: () => ({ ...settings }),

    setSettings(patch) {
      settings = {
        enabled: patch.enabled ?? settings.enabled,
        volume: patch.volume === undefined ? settings.volume : clamp01(patch.volume),
      };
      try {
        options.persist?.({ ...settings });
      } catch {
        /* not saved; still applies for this session */
      }
      applyVolume();
      if (!settings.enabled || settings.volume <= 0) suspendContext();
      for (const l of listeners) l({ ...settings });
      return { ...settings };
    },

    hydrate(next) {
      settings = normalize(next);
      applyVolume();
      if (!settings.enabled || settings.volume <= 0) suspendContext();
      for (const l of listeners) l({ ...settings });
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    stats: () => ({ ...counters }),
  };
}

/**
 * The app-wide instance. `main.tsx` hydrates it from the launcher's settings file at startup,
 * and a settings screen (later) talks to it: `sound.setSettings({ enabled })`. Every change is
 * saved by the Rust settings service.
 */
export const sound = createSoundService({
  persist: (s) => saveSoundSettings({ enabled: s.enabled, volume: s.volume }),
});
export const playSound = (kind: SoundKind) => sound.play(kind);
