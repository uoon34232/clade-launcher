// A stand-in for the Tauri backend, used only by the browser E2E suite.
//
// The real application talks to Rust through `window.__TAURI_INTERNALS__` (commands) and Tauri's
// event system. Playwright injects this script before the page loads, so the UNMODIFIED production
// frontend runs against synthetic data: nothing in `src/` knows about tests, no fixture or fake
// token ships in a build, and nothing touches the network, the disk or a real account.
//
// The fake follows the real command contract (see `src/lib/*/api.ts`). Long operations (preparing
// a profile) are held open until the test finishes them, so a test sees every intermediate state
// deterministically instead of racing a timer. State lives only in the page and is gone on reload.

/* eslint-disable @typescript-eslint/prefer-promise-reject-errors -- Tauri rejects an invoke() with the
   backend's serialised error object (`{ code, message, params }`), not with an Error; the fake does the same. */

import type { Page } from "@playwright/test";
import { DEFAULT_CONFIG, type BackendConfig } from "./fixtures";

/** Everything the injected script exposes to the test as `window.__e2e`. */
interface Control {
  calls: { cmd: string; args: unknown }[];
  state: {
    profiles: BackendConfig["profiles"];
    selected: string | null;
    accounts: BackendConfig["accounts"];
    active: string | null;
    ui: BackendConfig["ui"];
    runs: Record<string, "launching" | "playing">;
  };
  emit: (event: string, payload: unknown) => void;
  finishPrepare: () => boolean;
  failPrepare: (code: string, params?: Record<string, unknown>) => boolean;
  refuseNextLaunch: (code: string, accountId?: string | null) => void;
  failNext: (cmd: string, code: string, params?: Record<string, unknown>) => void;
  hasPendingPrepare: () => boolean;
}

/**
 * Runs inside the page (serialised by Playwright), so it must not use anything from this module's
 * scope: it receives the configuration as its only argument.
 */
function install(cfg: BackendConfig): void {
  const w = window as unknown as {
    __TAURI_INTERNALS__: Record<string, unknown>;
    __TAURI_EVENT_PLUGIN_INTERNALS__: Record<string, unknown>;
    __e2e: Control;
  };
  const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
  const state: Control["state"] = {
    profiles: clone(cfg.profiles),
    selected: cfg.selected,
    accounts: clone(cfg.accounts),
    active: cfg.active,
    ui: clone(cfg.ui),
    runs: clone(cfg.runs),
  };
  const calls: Control["calls"] = [];

  // ---- the event system (what `listen` talks to)
  const callbacks = new Map<number, (e: unknown) => void>();
  const listeners = new Map<string, Set<number>>();
  let nextId = 1;
  const emit = (event: string, payload: unknown) => {
    // Keep the fake's own idea of who is running in step with what it announces.
    if (event === "launch-state") {
      const ev = payload as { instanceId: string; state: "launching" | "playing" | "stopped" };
      if (ev.state === "stopped") delete state.runs[ev.instanceId];
      else state.runs[ev.instanceId] = ev.state;
    }
    for (const id of [...(listeners.get(event) ?? [])]) callbacks.get(id)?.({ event, id, payload });
  };

  // ---- held-open operations
  let pendingPrepare: {
    id: string;
    resolve: (v: unknown) => void;
    reject: (e: unknown) => void;
  } | null = null;
  let launchRefusal: { code: string; accountId: string | null } | null = null;
  // One-shot failures a test asks for, by command (support actions, settings saves, ...).
  const failures = new Map<string, { code: string; params: Record<string, unknown> }>();

  const profilesSnapshot = () => ({
    profiles: clone(state.profiles),
    selected: state.selected,
    totalPlaytimeSeconds: state.profiles.reduce((s, p) => s + p.playtimeSeconds, 0),
  });
  const accountsSnapshot = () => ({
    accounts: clone(state.accounts),
    active: state.active,
    // What this build can do: sign-in exists, skin and cape changes do not yet.
    capabilities: { signIn: true, skin: false, cape: false },
  });
  const fail = (code: string, message: string, extra: Record<string, unknown> = {}) =>
    Promise.reject({ code, message, ...extra });
  const slug = (name: string) =>
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "profile";

  const handle = (cmd: string, args: Record<string, unknown>): unknown => {
    const planned = failures.get(cmd);
    if (planned) {
      failures.delete(cmd);
      return fail(planned.code, `${cmd} was made to fail by the test`, { params: planned.params });
    }
    const announce = () => emit("ui-settings-changed", clone(state.ui));
    // The same decision the real backend makes (src-tauri/src/glass.rs): High Contrast is always opaque,
    // "on" and "off" are explicit, "system" follows the Windows preference.
    const glassStatus = () => {
      const { transparencyEffects, highContrast } = cfg.windows;
      const active =
        !highContrast &&
        (state.ui.transparency === "on" ||
          (state.ui.transparency === "system" && transparencyEffects));
      return { active, live: active && transparencyEffects };
    };
    const announceGlass = () => emit("glass-status-changed", glassStatus());
    switch (cmd) {
      // ---- launcher settings
      case "get_ui_settings":
        return clone(state.ui);
      case "set_sound_settings":
        if (typeof args.enabled === "boolean") state.ui.soundEnabled = args.enabled;
        if (typeof args.volume === "number") state.ui.soundVolume = args.volume;
        emit("ui-settings-changed", clone(state.ui));
        return null;
      case "set_language":
        state.ui.language = args.language as "en" | "ru";
        emit("ui-settings-changed", clone(state.ui));
        return null;
      case "set_last_phrase":
        state.ui.lastPhrase = args.id as string;
        return null;
      case "set_reduced_motion":
        state.ui.reducedMotion = args.mode as "system" | "on" | "off";
        announce();
        return null;
      case "set_transparency":
        state.ui.transparency = args.mode as "system" | "on" | "off";
        announce();
        announceGlass();
        return null;
      case "get_glass_status":
        return glassStatus();
      case "set_memory": {
        const mb = args.mb as number | null;
        const { min, max } = state.ui.memory;
        if (mb !== null && (mb < min || mb > max))
          return fail("invalid_memory", "memory is outside the safe range", {
            params: { min, max },
          });
        state.ui.memoryMb = mb;
        announce();
        return null;
      }
      case "set_behavior":
        if (args.onLaunch) state.ui.onLaunch = args.onLaunch as "keep_open" | "hide";
        if (args.onExit) state.ui.onExit = args.onExit as "show" | "stay_hidden";
        announce();
        return null;
      case "complete_onboarding":
        state.ui.onboardingDone = true;
        announce();
        return null;
      case "reset_settings":
        // Preferences go back to their defaults; the introduction stays seen and nothing else is touched.
        state.ui.soundEnabled = true;
        state.ui.soundVolume = 0.5;
        state.ui.language = null;
        state.ui.reducedMotion = "system";
        state.ui.transparency = "system";
        state.ui.memoryMb = null;
        state.ui.onLaunch = "keep_open";
        state.ui.onExit = "show";
        announce();
        announceGlass();
        return null;

      // ---- support actions (nothing is opened or written here)
      case "open_clade_folder":
      case "open_logs_folder":
        return null;
      case "export_diagnostics":
        return { name: "Clade-Diagnostics-20261004-120000.zip" };

      // ---- profiles and the version catalog
      case "list_profiles":
        return profilesSnapshot();
      case "select_profile":
        state.selected = args.id as string;
        return null;
      case "get_version_catalog":
        return clone(cfg.catalog);
      case "get_loader_options": {
        const listed = cfg.loaders[args.minecraft as string];
        return clone(
          listed ??
            (["fabric", "forge", "neoforge", "quilt"] as const).map((loader) => ({
              loader,
              status: "ok",
              versions: [{ version: "1.0.0", channel: "recommended" }],
              recommended: "1.0.0",
              stale: false,
              error: null,
            })),
        );
      }
      case "create_profile": {
        const req = args.request as {
          name: string;
          minecraft: string;
          profile:
            | { type: "vanilla" }
            | {
                type: "modded";
                loader: "fabric" | "forge" | "neoforge" | "quilt";
                loader_version: { mode: "auto" } | { mode: "pinned"; version: string };
              };
        };
        if (state.profiles.some((p) => p.name.toLowerCase() === req.name.trim().toLowerCase()))
          return fail("duplicate_name", "A profile with this name already exists.");
        const modded = req.profile.type === "modded" ? req.profile : null;
        const id = `${slug(req.name)}-${state.profiles.length + 1}`;
        state.profiles.push({
          id,
          name: req.name.trim(),
          minecraft: req.minecraft,
          kind: modded ? "modded" : "vanilla",
          loader: modded ? modded.loader : null,
          loaderVersion:
            modded && modded.loader_version.mode === "pinned"
              ? modded.loader_version.version
              : null,
          ready: false,
          playtimeSeconds: 0,
        });
        state.selected = id;
        return profilesSnapshot();
      }

      // ---- preparing a profile: held open until the test finishes or fails it
      case "prepare_profile": {
        // The real backend refuses before anything is claimed or fetched: only a ready ACTIVE
        // account (the effective one, with no stand-in) may prepare or verify.
        const active = state.accounts.find((a) => a.id === state.active) ?? state.accounts[0];
        if (!active || active.needsSignIn)
          return fail("account_required", "Sign in with a Minecraft account first.");
        return new Promise((resolve, reject) => {
          pendingPrepare = { id: args.id as string, resolve, reject };
        });
      }
      case "cancel_prepare":
        if (pendingPrepare) {
          pendingPrepare.reject({ code: "cancelled", message: "Preparation was cancelled." });
          pendingPrepare = null;
        }
        return null;

      // ---- launching
      case "launch_profile": {
        const id = args.id as string;
        if (launchRefusal) {
          const r = launchRefusal;
          launchRefusal = null;
          return fail(r.code, "The launch was refused.", { accountId: r.accountId });
        }
        if (state.runs[id]) return fail("already_running", "This profile is already running.");
        state.runs[id] = "launching";
        return null;
      }
      case "launch_status":
        return Object.entries(state.runs).map(([instanceId, s]) => ({ instanceId, state: s }));
      case "open_game_logs":
        return null;

      // ---- accounts (no Microsoft flow is ever started here)
      case "list_accounts":
        return accountsSnapshot();
      case "select_account":
        state.active = args.id as string;
        return accountsSnapshot();
      case "remove_account":
        state.accounts = state.accounts.filter((a) => a.id !== args.id);
        if (state.active === args.id) state.active = state.accounts[0]?.id ?? null;
        return accountsSnapshot();
      case "vault_status":
        return { exists: false, unlocked: false };
      case "create_vault":
      case "unlock_vault":
      case "cancel_sign_in":
        return null;
      case "begin_sign_in":
        return fail("approval_required", "Sign-in is not available in tests.");
      default:
        return fail("unknown_command", `The fake backend does not implement "${cmd}".`);
    }
  };

  const invoke = (cmd: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    // Tauri's event plugin
    if (cmd === "plugin:event|listen") {
      const event = args.event as string;
      const id = args.handler as number;
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)?.add(id);
      return Promise.resolve(id);
    }
    if (cmd === "plugin:event|unlisten") {
      listeners.get(args.event as string)?.delete(args.eventId as number);
      return Promise.resolve(null);
    }
    // Window controls: nothing to do in a browser.
    if (cmd.startsWith("plugin:")) return Promise.resolve(null);
    calls.push({ cmd, args: clone(args) });
    try {
      return Promise.resolve(handle(cmd, args));
    } catch (e) {
      return Promise.reject(e);
    }
  };

  w.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback: (cb: (e: unknown) => void) => {
      const id = nextId++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback: (id: number) => callbacks.delete(id),
    convertFileSrc: (p: string) => p,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
  };
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };

  w.__e2e = {
    calls,
    state,
    emit,
    hasPendingPrepare: () => pendingPrepare !== null,
    finishPrepare: () => {
      if (!pendingPrepare) return false;
      const p = state.profiles.find((x) => x.id === pendingPrepare?.id);
      if (p) p.ready = true;
      pendingPrepare.resolve(profilesSnapshot());
      pendingPrepare = null;
      return true;
    },
    failPrepare: (code, params = {}) => {
      if (!pendingPrepare) return false;
      pendingPrepare.reject({ code, params, message: "Preparation failed." });
      pendingPrepare = null;
      return true;
    },
    refuseNextLaunch: (code, accountId = null) => {
      launchRefusal = { code, accountId };
    },
    failNext: (cmd, code, params = {}) => {
      failures.set(cmd, { code, params });
    },
  };
}

/** The test's handle on the fake backend of one page. */
export class Backend {
  constructor(private readonly page: Page) {}

  /** Installs the fake before any page script runs. Call before `page.goto`. */
  static async install(page: Page, overrides: Partial<BackendConfig> = {}): Promise<Backend> {
    await page.addInitScript(install, { ...DEFAULT_CONFIG, ...overrides });
    return new Backend(page);
  }

  /** Every command the frontend has sent so far, in order. */
  async calls(cmd?: string): Promise<{ cmd: string; args: unknown }[]> {
    const all = await this.page.evaluate(
      () => (window as unknown as { __e2e: Control }).__e2e.calls,
    );
    return cmd ? all.filter((c) => c.cmd === cmd) : all;
  }

  /** The backend-side state, for checking what the UI actually asked it to change. */
  state(): Promise<Control["state"]> {
    return this.page.evaluate(() => (window as unknown as { __e2e: Control }).__e2e.state);
  }

  /** Sends an event to the frontend exactly as the Rust side would. */
  async emit(event: string, payload: unknown): Promise<void> {
    await this.page.evaluate(
      ([e, p]) => (window as unknown as { __e2e: Control }).__e2e.emit(e, p),
      [event, payload] as const,
    );
  }

  /** Progress of a preparation (`prepare-progress`). */
  prepareProgress(p: {
    instanceId: string;
    stage: string;
    label: string;
    itemsDone?: number;
    itemsTotal?: number;
    bytesDone?: number;
    bytesTotal?: number;
    overall?: number;
  }): Promise<void> {
    return this.emit("prepare-progress", {
      itemsDone: null,
      itemsTotal: null,
      bytesDone: null,
      bytesTotal: null,
      overall: null,
      ...p,
    });
  }

  async finishPrepare(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as { __e2e: Control }).__e2e.finishPrepare());
  }

  async failPrepare(code: string, params: Record<string, unknown> = {}): Promise<void> {
    await this.page.evaluate(
      ([c, p]) => (window as unknown as { __e2e: Control }).__e2e.failPrepare(c, p),
      [code, params] as const,
    );
  }

  /** Makes the next call of a command fail once with a coded error, as the Rust side would. */
  async failNext(cmd: string, code: string, params: Record<string, unknown> = {}): Promise<void> {
    await this.page.evaluate(
      ([c, k, p]) => (window as unknown as { __e2e: Control }).__e2e.failNext(c, k, p),
      [cmd, code, params] as const,
    );
  }

  async refuseNextLaunch(code: string, accountId: string | null = null): Promise<void> {
    await this.page.evaluate(
      ([c, a]) => (window as unknown as { __e2e: Control }).__e2e.refuseNextLaunch(c, a),
      [code, accountId] as const,
    );
  }

  /** The game process is alive (`launch-state`: playing). */
  playing(instanceId: string): Promise<void> {
    return this.emit("launch-state", { instanceId, state: "playing" });
  }

  /** The game ended (`launch-state`: stopped), cleanly or with a reason code. */
  stopped(
    instanceId: string,
    exit: {
      kind: "normal" | "crashed" | "failed_to_start";
      reason: string;
      code?: number | null;
      seconds?: number;
    } | null,
  ): Promise<void> {
    return this.emit("launch-state", {
      instanceId,
      state: "stopped",
      exit: exit ? { code: null, seconds: 30, ...exit } : null,
    });
  }
}
