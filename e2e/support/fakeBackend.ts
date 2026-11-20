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
    unreadable: string[];
    removed: NonNullable<BackendConfig["removed"]>;
    accounts: BackendConfig["accounts"];
    active: string | null;
    appearance: BackendConfig["appearance"];
    vault: BackendConfig["vault"];
    ui: BackendConfig["ui"];
    runs: Record<string, "launching" | "playing">;
    embed: { active: boolean; instanceId: string | null; maximized: boolean };
  };
  emit: (event: string, payload: unknown) => void;
  setEmbedded: (instanceId: string | null, active: boolean, maximized?: boolean) => void;
  finishPrepare: () => boolean;
  failPrepare: (code: string, params?: Record<string, unknown>) => boolean;
  refuseNextLaunch: (code: string, accountId?: string | null) => void;
  failNext: (cmd: string, code: string, params?: Record<string, unknown>) => void;
  holdNext: (cmd: string) => void;
  releaseHeld: () => boolean;
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
    unreadable: clone(cfg.unreadable ?? []),
    removed: clone(cfg.removed ?? []),
    accounts: clone(cfg.accounts),
    active: cfg.active,
    appearance: clone(cfg.appearance),
    vault: clone(cfg.vault),
    ui: clone(cfg.ui),
    runs: clone(cfg.runs),
    embed: clone(cfg.embed ?? { active: false, instanceId: null, maximized: false }),
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
      if (ev.state === "stopped") {
        delete state.runs[ev.instanceId];
        if (state.embed.active && state.embed.instanceId === ev.instanceId) {
          state.embed = { active: false, instanceId: null, maximized: false };
          for (const id of [...(listeners.get("game-embed-state") ?? [])]) {
            callbacks.get(id)?.({ event: "game-embed-state", id, payload: clone(state.embed) });
          }
        }
      } else {
        state.runs[ev.instanceId] = ev.state;
      }
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
  // Commands a test wants to stay open until it releases them, to see the busy state.
  const holds = new Set<string>();
  let held: (() => void) | null = null;
  // A real (solid-colour) 64x64 PNG, so the browser can decode it like a skin.
  const NEW_SKIN =
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAZklEQVR42u3QQREAAAQAMFHFEFAvcjh7rMCiK+ezECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAgQIECBAgAABAu4HLPSxsmj7bLHgAAAAAElFTkSuQmCC";
  const lookOf = (id: unknown) => {
    const key = String(id);
    state.appearance[key] ??= { skinModel: null, capes: [] };
    return state.appearance[key];
  };
  // The wire format of the real commands (src-tauri/src/appearance_cmd.rs): `skinVariant`, not the fixture's name.
  const wire = (l: { skinModel: string | null; capes: unknown[] }) => ({
    skinVariant: l.skinModel,
    capes: clone(l.capes),
  });
  const knownAccount = (id: unknown) => state.accounts.some((a) => a.id === id);

  const profilesSnapshot = () => ({
    profiles: clone(state.profiles).map((p) => ({
      ...p,
      memoryMb: p.memoryMb ?? null,
      resolution: p.resolution ?? null,
      jvmArgs: p.jvmArgs ?? [],
    })),
    selected: state.selected,
    totalPlaytimeSeconds: state.profiles.reduce((s, p) => s + p.playtimeSeconds, 0),
    unreadable: clone(state.unreadable),
    removed: clone(state.removed).map((r) => ({
      folder: r.folder,
      name: r.name,
      minecraft: r.minecraft,
      kind: r.kind,
      loader: r.loader,
      loaderVersion: r.loaderVersion,
      playtimeSeconds: r.playtimeSeconds,
      removedAt: r.removedAt ?? null,
    })),
  });
  const accountsSnapshot = () => ({
    accounts: clone(state.accounts),
    active: state.active,
    // What this build can do.
    capabilities: { signIn: true, skin: true, cape: true },
  });
  const fail = (code: string, message: string, extra: Record<string, unknown> = {}) =>
    Promise.reject({ code, message, ...extra });
  const slug = (name: string) =>
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "profile";

  const FORBIDDEN_JVM_PREFIXES = [
    "-javaagent",
    "-agentlib",
    "-agentpath",
    "-xrun",
    "-jar",
    "-cp",
    "-classpath",
    "--class-path",
    "-p",
    "--module-path",
    "--upgrade-module-path",
    "--patch-module",
    "-xbootclasspath",
    "-xx:onerror",
    "-xx:onoutofmemoryerror",
    "-xx:errorfile",
    "-xx:heapdumppath",
    "-xx:logfile",
    "-xx:sharedarchivefile",
    "-xx:archiveclassesatexit",
    "-duser.home",
    "-duser.dir",
    "-djava.io.tmpdir",
    "-djava.library.path",
    "-djava.class.path",
    "-djdk.module.path",
    "-dlog4j.configurationfile",
    "-dlog4j2.formatmsgnolookups",
    "-xmx",
    "-xms",
    "-xx:maxram",
    "-xx:initialram",
  ];
  const FORBIDDEN_XX_OPTS = new Set([
    "onerror",
    "onoutofmemoryerror",
    "errorfile",
    "heapdumppath",
    "logfile",
    "sharedarchivefile",
    "archiveclassesatexit",
  ]);
  const isAllowedJvmArg = (a: string): boolean => {
    const xxRest = a.startsWith("-XX:+")
      ? a.slice(5)
      : a.startsWith("-XX:-")
        ? a.slice(5)
        : a.startsWith("-XX:")
          ? a.slice(4)
          : null;
    if (xxRest !== null) {
      const eq = xxRest.indexOf("=");
      const opt = eq === -1 ? xxRest : xxRest.slice(0, eq);
      const val = eq === -1 ? null : xxRest.slice(eq + 1);
      if (!opt || !/^[A-Za-z0-9_]+$/.test(opt) || val === "") return false;
      const optLower = opt.toLowerCase();
      return (
        !FORBIDDEN_XX_OPTS.has(optLower) &&
        !optLower.startsWith("maxram") &&
        !optLower.startsWith("initialram")
      );
    }
    if (a.startsWith("-Xss") || a.startsWith("-Xmn")) {
      const rest = a.slice(4);
      return rest.length > 0 && /^[A-Za-z0-9]+$/.test(rest);
    }
    if (a.startsWith("-D")) {
      const rest = a.slice(2);
      const eq = rest.indexOf("=");
      const key = eq === -1 ? rest : rest.slice(0, eq);
      return key.length > 0 && /^[A-Za-z0-9._-]+$/.test(key);
    }
    if (a === "-ea" || a === "-da" || a === "-esa" || a === "-dsa") return true;
    for (const pfx of ["--add-opens=", "--add-exports=", "--enable-native-access="]) {
      if (a.startsWith(pfx)) return a.length > pfx.length;
    }
    return false;
  };
  const validJvmArgs = (raw: unknown): raw is string[] => {
    if (!Array.isArray(raw) || raw.length > 64) return false;
    for (const item of raw) {
      if (typeof item !== "string" || item.length === 0 || item.length > 512) return false;
      // eslint-disable-next-line no-control-regex
      if (/[\u0000-\u001f\u007f\s]/.test(item) || !item.startsWith("-")) return false;
      const lower = item.toLowerCase();
      if (FORBIDDEN_JVM_PREFIXES.some((p) => lower.startsWith(p)) || !isAllowedJvmArg(item)) {
        return false;
      }
    }
    return true;
  };

  const handle = (cmd: string, args: Record<string, unknown>): unknown => {
    const planned = failures.get(cmd);
    if (planned) {
      failures.delete(cmd);
      return fail(planned.code, `${cmd} was made to fail by the test`, { params: planned.params });
    }
    if (holds.delete(cmd)) {
      return new Promise((resolve, reject) => {
        held = () => {
          void Promise.resolve(handle(cmd, args)).then(resolve, reject);
        };
      });
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
      case "set_ui_mode": {
        const mode = args.mode;
        if (mode !== "simple" && mode !== "advanced") {
          return fail("unsupported_value", "that setting value is not supported");
        }
        state.ui.uiMode = mode;
        announce();
        return null;
      }
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
        if (args.onLaunch) state.ui.onLaunch = args.onLaunch as "embed" | "keep_open" | "hide";
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
        state.ui.uiMode = "simple";
        state.ui.reducedMotion = "system";
        state.ui.transparency = "system";
        state.ui.memoryMb = null;
        state.ui.onLaunch = "embed";
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
      // "Remove from Clade": the real command moves the folder to instances/.removed/ and deletes
      // nothing. Here the profile moves from `state.profiles` into `state.removed`.
      case "remove_profile": {
        const id = args.id as string;
        const idx = state.profiles.findIndex((p) => p.id === id);
        if (idx === -1) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        const [removed] = state.profiles.splice(idx, 1);
        state.removed.unshift({
          ...clone(removed),
          folder: `${removed.id}-1700000000-${state.removed.length + 1}`,
          removedAt: 1_700_000_000 + state.removed.length,
        });
        state.selected = state.profiles[0]?.id ?? null;
        return profilesSnapshot();
      }
      case "restore_profile": {
        const folder = args.folder as string;
        const idx = state.removed.findIndex((r) => r.folder === folder);
        if (idx === -1) return fail("not_found", "No such profile.");
        const [item] = state.removed.splice(idx, 1);
        let finalName = item.name;
        if (
          state.profiles.some((p) => p.name.trim().toLowerCase() === finalName.trim().toLowerCase())
        ) {
          const stem = finalName.trim().replace(/\s+\(([2-9]\d*|\d{2,})\)$/, "");
          let n = 2;
          while (
            state.profiles.some(
              (p) => p.name.trim().toLowerCase() === `${stem} (${n})`.toLowerCase(),
            )
          ) {
            n += 1;
          }
          finalName = `${stem} (${n})`;
        }
        const baseId =
          finalName.toLowerCase() === item.name.toLowerCase() ? item.id : slug(finalName);
        let targetId = baseId;
        let k = 2;
        while (state.profiles.some((p) => p.id === targetId)) {
          targetId = `${baseId}-${k}`;
          k += 1;
        }
        state.profiles.push({
          id: targetId,
          name: finalName,
          minecraft: item.minecraft,
          kind: item.kind,
          loader: item.loader,
          loaderVersion: item.loaderVersion,
          ready: item.ready,
          playtimeSeconds: item.playtimeSeconds,
          memoryMb: item.memoryMb ?? null,
          resolution: item.resolution ?? null,
          jvmArgs: item.jvmArgs ?? [],
        });
        state.selected = targetId;
        return profilesSnapshot();
      }
      case "open_removed_profiles_folder":
        return null;
      case "rename_profile": {
        const id = args.id as string;
        const target = state.profiles.find((p) => p.id === id);
        if (!target) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        const rawName = typeof args.name === "string" ? args.name : "";
        const trimmed = rawName.trim();
        // eslint-disable-next-line no-control-regex
        const hasControl = /[\u0000-\u001f\u007f]/.test(trimmed);
        if (trimmed.length === 0 || [...trimmed].length > 64 || hasControl)
          return fail("invalid_name", "That name cannot be used.");
        if (
          state.profiles.some(
            (p) => p.id !== id && p.name.trim().toLowerCase() === trimmed.toLowerCase(),
          )
        )
          return fail("duplicate_name", "A profile with this name already exists.", {
            params: { name: trimmed },
          });
        target.name = trimmed;
        return profilesSnapshot();
      }
      case "set_profile_memory": {
        const id = args.id as string;
        const target = state.profiles.find((p) => p.id === id);
        if (!target) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        const mb = args.mb;
        const { min, max } = state.ui.memory;
        if (mb !== null && (typeof mb !== "number" || !Number.isFinite(mb) || mb < min || mb > max))
          return fail("invalid_memory", "Memory is outside the safe range.", {
            params: { min, max },
          });
        target.memoryMb = mb === null ? null : Math.round(mb);
        return profilesSnapshot();
      }
      case "set_profile_resolution": {
        const id = args.id as string;
        const target = state.profiles.find((p) => p.id === id);
        if (!target) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        const res = args.resolution as { width?: unknown; height?: unknown } | null | undefined;
        if (res !== null && res !== undefined) {
          const w = res.width;
          const h = res.height;
          if (
            typeof w !== "number" ||
            typeof h !== "number" ||
            !Number.isInteger(w) ||
            !Number.isInteger(h) ||
            w < 320 ||
            w > 7680 ||
            h < 240 ||
            h > 4320
          ) {
            return fail("invalid_resolution", "Invalid window resolution.", {
              params: { minWidth: 320, maxWidth: 7680, minHeight: 240, maxHeight: 4320 },
            });
          }
          target.resolution = { width: w, height: h };
        } else {
          target.resolution = null;
        }
        return profilesSnapshot();
      }
      case "set_profile_jvm_args": {
        const id = args.id as string;
        const target = state.profiles.find((p) => p.id === id);
        if (!target) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        if (!validJvmArgs(args.args)) {
          return fail("invalid_jvm_args", "Invalid JVM argument.");
        }
        target.jvmArgs = clone(args.args);
        return profilesSnapshot();
      }
      case "set_profile_version": {
        const id = args.id as string;
        const target = state.profiles.find((p) => p.id === id);
        if (!target) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        const minecraft = typeof args.minecraft === "string" ? args.minecraft : "";
        if (!cfg.catalog.entries.some((e) => e.id === minecraft))
          return fail("unknown_minecraft", "Unknown Minecraft version.", {
            params: { minecraft },
          });
        const lv = args.loaderVersion as
          { mode: "auto" } | { mode: "pinned"; version: string } | null | undefined;
        if (target.kind === "vanilla") {
          if (lv !== null && lv !== undefined) return fail("storage", "Vanilla has no loader.");
          if (target.minecraft !== minecraft) {
            target.minecraft = minecraft;
            target.ready = false;
          }
          return profilesSnapshot();
        }
        const loader = target.loader!;
        const listed = cfg.loaders[minecraft];
        const loaderEntry = listed ? listed.find((o) => o.loader === loader) : undefined;
        const resolvedEntry = loaderEntry ?? {
          loader,
          status: "ok" as const,
          versions: [{ version: "1.0.0", channel: "recommended" as const }],
          recommended: "1.0.0",
        };
        if (resolvedEntry.status === "unavailable")
          return fail("cannot_verify_loader", "Loader check unavailable.", {
            params: { loader, minecraft },
          });
        if (resolvedEntry.status === "none")
          return fail("no_compatible_loader", "No compatible loader.", {
            params: { loader, minecraft },
          });
        const nextVersion =
          lv?.mode === "pinned" ? lv.version : (resolvedEntry.recommended ?? null);
        if (
          lv?.mode === "pinned" &&
          !resolvedEntry.versions.some((v) => v.version === lv.version)
        ) {
          return fail("loader_version_not_offered", "Loader version not offered.", {
            params: { loader, version: lv.version, minecraft },
          });
        }
        if (target.minecraft !== minecraft || target.loaderVersion !== nextVersion) {
          target.minecraft = minecraft;
          target.loaderVersion = nextVersion;
          target.ready = false;
        }
        return profilesSnapshot();
      }
      case "duplicate_profile": {
        const id = args.id as string;
        const source = state.profiles.find((p) => p.id === id);
        if (!source) return fail("not_found", "No such profile.");
        if (state.runs[id]) return fail("already_running", "This profile is running.");
        if (pendingPrepare?.id === id) return fail("busy", "This profile is being set up.");
        const stem = source.name.trim().replace(/\s+\(([2-9]\d*|\d{2,})\)$/, "");
        let n = 2;
        let copyName = `${stem} (${n})`;
        while (state.profiles.some((p) => p.name.trim().toLowerCase() === copyName.toLowerCase())) {
          n += 1;
          copyName = `${stem} (${n})`;
        }
        const newId = `${slug(copyName)}-${state.profiles.length + 1}`;
        state.profiles.push({
          ...clone(source),
          id: newId,
          name: copyName,
          playtimeSeconds: 0,
        });
        state.selected = newId;
        return profilesSnapshot();
      }
      case "open_profile_folder": {
        const id = args.id as string;
        if (!state.profiles.some((p) => p.id === id)) return fail("not_found", "No such profile.");
        return null;
      }
      case "open_instances_folder":
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
          memoryMb: null,
          resolution: null,
          jvmArgs: [],
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
      case "get_game_embed_status":
        return clone(state.embed);
      case "set_game_stage_bounds":
        return null;
      case "toggle_game_maximize":
        if (!state.embed.active) return clone(state.embed);
        state.embed.maximized = !state.embed.maximized;
        emit("game-embed-state", clone(state.embed));
        return clone(state.embed);

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
      case "account_appearance":
        if (!knownAccount(args.id)) return fail("unknown_account", "No such account.");
        return wire(lookOf(args.id));
      case "set_account_skin": {
        if (!knownAccount(args.id)) return fail("unknown_account", "No such account.");
        const png = args.png as number[];
        const isPng = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].every(
          (b, i) => png[i] === b,
        );
        const width = ((png[16] << 24) | (png[17] << 16) | (png[18] << 8) | png[19]) >>> 0;
        const height = ((png[20] << 24) | (png[21] << 16) | (png[22] << 8) | png[23]) >>> 0;
        if (!isPng || width !== 64 || ![32, 64].includes(height) || png.length > 256 * 1024)
          return fail("invalid_skin", "Not a skin.");
        lookOf(args.id).skinModel = args.variant as "classic" | "slim";
        const account = state.accounts.find((a) => a.id === args.id);
        if (account) account.skin = NEW_SKIN;
        emit("accounts-changed", null);
        return wire(lookOf(args.id));
      }
      case "reset_account_skin": {
        if (!knownAccount(args.id)) return fail("unknown_account", "No such account.");
        lookOf(args.id).skinModel = null;
        const account = state.accounts.find((a) => a.id === args.id);
        if (account) account.skin = null;
        emit("accounts-changed", null);
        return wire(lookOf(args.id));
      }
      case "set_account_cape": {
        if (!knownAccount(args.id)) return fail("unknown_account", "No such account.");
        const look = lookOf(args.id);
        if (args.capeId !== null && !look.capes.some((c) => c.id === args.capeId))
          return fail("cape_not_owned", "Not owned.");
        for (const c of look.capes) c.active = c.id === args.capeId;
        emit("accounts-changed", null);
        return wire(look);
      }
      case "vault_status":
        return clone(state.vault);
      case "create_vault":
        state.vault = { exists: true, unlocked: true };
        return null;
      case "unlock_vault":
        state.vault.unlocked = true;
        return null;
      // "Start fresh": like the backend, only for a vault that is not open; every account signs in again.
      case "reset_vault":
        if (state.vault.unlocked) return fail("vault_unlocked", "The vault is open.");
        state.vault = { exists: false, unlocked: false };
        for (const a of state.accounts) a.needsSignIn = true;
        emit("accounts-changed", null);
        return accountsSnapshot();
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
    setEmbedded: (instanceId, active, maximized = false) => {
      state.embed = active
        ? { active: true, instanceId, maximized }
        : { active: false, instanceId: null, maximized: false };
      emit("game-embed-state", clone(state.embed));
    },
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
    holdNext: (cmd) => {
      holds.add(cmd);
    },
    releaseHeld: () => {
      if (!held) return false;
      held();
      held = null;
      return true;
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

  /** Keeps the next call of a command open until `release()`, so a test can see the busy state. */
  async holdNext(cmd: string): Promise<void> {
    await this.page.evaluate(
      (c) => (window as unknown as { __e2e: Control }).__e2e.holdNext(c),
      cmd,
    );
  }

  /** Lets a held command run and answer. */
  async release(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as { __e2e: Control }).__e2e.releaseHeld());
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

  /** Simulates native framed-game attachment (`game-embed-state`). */
  async embedded(instanceId: string, active = true, maximized = false): Promise<void> {
    await this.page.evaluate(
      ([id, a, m]) => (window as unknown as { __e2e: Control }).__e2e.setEmbedded(id, a, m),
      [active ? instanceId : null, active, maximized] as const,
    );
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
