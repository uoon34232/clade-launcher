// Frontend side of profile management and version/loader discovery.
//
// Everything here is a thin, validating wrapper over Rust commands. The frontend never fetches
// Minecraft or loader metadata itself and never touches the filesystem: network policy, caching,
// validation and file ownership all live in the backend. Outside the launcher (plain-browser
// development, unit tests) there is no backend and nothing is invented: calls reject with a clear
// message.

import { listen } from "@tauri-apps/api/event";
import { call } from "@/lib/ipc";
import { inTauri } from "@/lib/settings/uiSettings";

export type LoaderKind = "fabric" | "forge" | "neoforge" | "quilt";
/** The display order of the loaders. */
export const LOADER_ORDER: readonly LoaderKind[] = ["fabric", "forge", "neoforge", "quilt"];
export const LOADER_NAMES: Readonly<Record<LoaderKind, string>> = {
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
  quilt: "Quilt",
};

export type VersionKind = "release" | "snapshot" | "old_beta" | "old_alpha";
export type Channel = "recommended" | "stable" | "latest" | "prerelease";

export const MIN_RESOLUTION_WIDTH = 320;
export const MAX_RESOLUTION_WIDTH = 7680;
export const MIN_RESOLUTION_HEIGHT = 240;
export const MAX_RESOLUTION_HEIGHT = 4320;

export interface ProfileResolution {
  width: number;
  height: number;
}

export interface ProfileSummary {
  id: string;
  name: string;
  minecraft: string;
  kind: "vanilla" | "modded";
  loader: LoaderKind | null;
  /** Pinned or installed loader version, when known. */
  loaderVersion: string | null;
  /** False until Minecraft (and the loader) are installed, which happens later. */
  ready: boolean;
  playtimeSeconds: number;
  /** Per-profile memory override in MiB, or null to follow Launcher Settings. */
  memoryMb?: number | null;
  /** Custom initial game window resolution, or null for Minecraft's default. */
  resolution?: ProfileResolution | null;
  /** Validated extra JVM arguments for this profile. */
  jvmArgs?: readonly string[];
}

export interface RemovedProfileSummary {
  folder: string;
  name: string;
  minecraft: string;
  kind: "vanilla" | "modded";
  loader: LoaderKind | null;
  loaderVersion: string | null;
  playtimeSeconds: number;
  removedAt: number | null;
}

export interface ProfilesSnapshot {
  profiles: ProfileSummary[];
  selected: string | null;
  totalPlaytimeSeconds: number;
  /** Folder names inside instances/ whose profile metadata could not be read (files are kept untouched). */
  unreadable?: readonly string[];
  /** Profiles moved to instances/.removed/ that can be restored in one click. */
  removed?: readonly RemovedProfileSummary[];
}

export interface CatalogEntry {
  id: string;
  kind: VersionKind;
  releaseTime: string;
}

export interface VersionCatalog {
  entries: CatalogEntry[];
  latestRelease: string | null;
  latestSnapshot: string | null;
  fetchedAt: number;
  origin: "network" | "cache_fresh" | "cache_stale";
  /** True when this is a saved copy used because Mojang could not be reached (or it is old). */
  stale: boolean;
  networkError: string | null;
}

export interface LoaderOffer {
  version: string;
  channel: Channel;
}

export interface LoaderOption {
  loader: LoaderKind;
  /** ok: compatible versions exist. none: the official catalog has none for this Minecraft version.
   *  unavailable: could not be checked (offline, never cached), so it must not be offered. */
  status: "ok" | "none" | "unavailable";
  versions: LoaderOffer[];
  recommended: string | null;
  stale: boolean;
  error: string | null;
}

export type LoaderVersionChoice = { mode: "auto" } | { mode: "pinned"; version: string };

export type NewProfileRequest = {
  name: string;
  minecraft: string;
  profile:
    | { type: "vanilla" }
    | {
        type: "modded";
        loader: LoaderKind;
        loader_version: LoaderVersionChoice;
      };
};

// ------------------------------------------------------------------ validation of backend data

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown, fallback = 0): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;
const isLoader = (v: unknown): v is LoaderKind =>
  typeof v === "string" && (LOADER_ORDER as readonly string[]).includes(v);
const VERSION_KINDS: readonly string[] = ["release", "snapshot", "old_beta", "old_alpha"];
const CHANNELS: readonly string[] = ["recommended", "stable", "latest", "prerelease"];

const MAX_JVM_ARGS = 64;
const MAX_JVM_ARG_LEN = 512;

function parseResolution(raw: unknown): ProfileResolution | null {
  if (!isObj(raw)) return null;
  const { width, height } = raw;
  if (
    typeof width !== "number" ||
    typeof height !== "number" ||
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width < MIN_RESOLUTION_WIDTH ||
    width > MAX_RESOLUTION_WIDTH ||
    height < MIN_RESOLUTION_HEIGHT ||
    height > MAX_RESOLUTION_HEIGHT
  ) {
    return null;
  }
  return { width, height };
}

function parseJvmArgs(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const item of raw) {
    if (typeof item === "string" && item.length > 0 && item.length <= MAX_JVM_ARG_LEN) {
      out.push(item);
      if (out.length >= MAX_JVM_ARGS) break;
    }
  }
  return out;
}

export function parseProfiles(raw: unknown): ProfilesSnapshot {
  const o = isObj(raw) ? raw : {};
  const profiles: ProfileSummary[] = [];
  for (const p of Array.isArray(o.profiles) ? o.profiles : []) {
    if (!isObj(p)) continue;
    const id = str(p.id);
    const name = str(p.name);
    const minecraft = str(p.minecraft);
    if (!id || !name || !minecraft || (p.kind !== "vanilla" && p.kind !== "modded")) continue;
    profiles.push({
      id,
      name,
      minecraft,
      kind: p.kind,
      loader: isLoader(p.loader) ? p.loader : null,
      loaderVersion: str(p.loaderVersion),
      ready: p.ready === true,
      playtimeSeconds: Math.max(0, num(p.playtimeSeconds)),
      memoryMb:
        typeof p.memoryMb === "number" && Number.isFinite(p.memoryMb) && p.memoryMb > 0
          ? p.memoryMb
          : null,
      resolution: parseResolution(p.resolution),
      jvmArgs: parseJvmArgs(p.jvmArgs),
    });
  }
  const unreadable: string[] = [];
  for (const u of Array.isArray(o.unreadable) ? o.unreadable : []) {
    if (typeof u === "string" && u.length > 0) unreadable.push(u);
  }
  const removed: RemovedProfileSummary[] = [];
  for (const r of Array.isArray(o.removed) ? o.removed : []) {
    if (!isObj(r)) continue;
    const folder = str(r.folder);
    const name = str(r.name);
    const minecraft = str(r.minecraft);
    if (!folder || !name || !minecraft || (r.kind !== "vanilla" && r.kind !== "modded")) continue;
    removed.push({
      folder,
      name,
      minecraft,
      kind: r.kind,
      loader: isLoader(r.loader) ? r.loader : null,
      loaderVersion: str(r.loaderVersion),
      playtimeSeconds: Math.max(0, num(r.playtimeSeconds)),
      removedAt:
        typeof r.removedAt === "number" && Number.isFinite(r.removedAt) && r.removedAt > 0
          ? r.removedAt
          : null,
    });
  }
  const selected = str(o.selected);
  return {
    profiles,
    selected:
      selected && profiles.some((p) => p.id === selected) ? selected : (profiles[0]?.id ?? null),
    totalPlaytimeSeconds: Math.max(0, num(o.totalPlaytimeSeconds)),
    unreadable,
    removed,
  };
}

export function parseCatalog(raw: unknown): VersionCatalog {
  const o = isObj(raw) ? raw : {};
  const entries: CatalogEntry[] = [];
  for (const e of Array.isArray(o.entries) ? o.entries : []) {
    if (!isObj(e)) continue;
    const id = str(e.id);
    if (id && typeof e.kind === "string" && VERSION_KINDS.includes(e.kind)) {
      entries.push({ id, kind: e.kind as VersionKind, releaseTime: str(e.releaseTime) ?? "" });
    }
  }
  const origin =
    o.origin === "network" || o.origin === "cache_fresh" || o.origin === "cache_stale"
      ? o.origin
      : "cache_stale";
  return {
    entries,
    latestRelease: str(o.latestRelease),
    latestSnapshot: str(o.latestSnapshot),
    fetchedAt: num(o.fetchedAt),
    origin,
    stale: o.stale === true,
    networkError: str(o.networkError),
  };
}

export function parseLoaderOptions(raw: unknown): LoaderOption[] {
  const out: LoaderOption[] = [];
  for (const o of Array.isArray(raw) ? raw : []) {
    if (!isObj(o) || !isLoader(o.loader)) continue;
    const versions: LoaderOffer[] = [];
    for (const v of Array.isArray(o.versions) ? o.versions : []) {
      if (
        isObj(v) &&
        str(v.version) &&
        typeof v.channel === "string" &&
        CHANNELS.includes(v.channel)
      ) {
        versions.push({ version: v.version as string, channel: v.channel as Channel });
      }
    }
    const status = o.status === "ok" || o.status === "none" ? o.status : "unavailable";
    out.push({
      loader: o.loader,
      // Never trust an "ok" without versions.
      status: status === "ok" && versions.length === 0 ? "none" : status,
      versions,
      recommended: str(o.recommended),
      stale: o.stale === true,
      error: str(o.error),
    });
  }
  return out;
}

// ------------------------------------------------------------------ preparation progress

export type PrepareStage =
  | "planning"
  | "java"
  | "minecraft"
  | "libraries"
  | "assets"
  | "natives"
  | "loader"
  | "verifying"
  | "done";
const PREPARE_STAGES: readonly string[] = [
  "planning",
  "java",
  "minecraft",
  "libraries",
  "assets",
  "natives",
  "loader",
  "verifying",
  "done",
];

/** Where preparing a profile stands. A count or fraction is `null` unless the backend knows the total. */
export interface PrepareProgress {
  instanceId: string;
  stage: PrepareStage;
  label: string;
  itemsDone: number | null;
  itemsTotal: number | null;
  bytesDone: number | null;
  bytesTotal: number | null;
  /** 0 to 1 over the whole preparation, or `null` when it cannot be measured (never invented). */
  overall: number | null;
}

const count = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null;

export function parsePrepareProgress(raw: unknown): PrepareProgress | null {
  if (!isObj(raw)) return null;
  const instanceId = str(raw.instanceId);
  const label = str(raw.label);
  if (!instanceId || !label || typeof raw.stage !== "string" || !PREPARE_STAGES.includes(raw.stage))
    return null;
  const overall = count(raw.overall);
  return {
    instanceId,
    stage: raw.stage as PrepareStage,
    label,
    itemsDone: count(raw.itemsDone),
    itemsTotal: count(raw.itemsTotal),
    bytesDone: count(raw.bytesDone),
    bytesTotal: count(raw.bytesTotal),
    overall: overall === null ? null : Math.min(1, overall),
  };
}

// ------------------------------------------------------------------ calls

export const profilesApi = {
  list: async () => parseProfiles(await call("list_profiles")),
  select: async (id: string): Promise<void> => {
    await call("select_profile", { id });
  },
  versionCatalog: async (force = false) =>
    parseCatalog(await call("get_version_catalog", { force })),
  loaderOptions: async (minecraft: string) =>
    parseLoaderOptions(await call("get_loader_options", { minecraft })),
  create: async (request: NewProfileRequest) =>
    parseProfiles(await call("create_profile", { request })),
  rename: async (id: string, name: string) =>
    parseProfiles(await call("rename_profile", { id, name })),
  setMemory: async (id: string, mb: number | null) =>
    parseProfiles(await call("set_profile_memory", { id, mb })),
  setResolution: async (id: string, resolution: ProfileResolution | null) =>
    parseProfiles(await call("set_profile_resolution", { id, resolution })),
  setJvmArgs: async (id: string, args: readonly string[]) =>
    parseProfiles(await call("set_profile_jvm_args", { id, args: [...args] })),
  setVersion: async (
    id: string,
    minecraft: string,
    loaderVersion: LoaderVersionChoice | null = null,
  ) =>
    parseProfiles(
      await call("set_profile_version", { id, minecraft, loaderVersion: loaderVersion ?? null }),
    ),
  duplicate: async (id: string) => parseProfiles(await call("duplicate_profile", { id })),
  openFolder: async (id: string): Promise<void> => {
    await call("open_profile_folder", { id });
  },
  openInstancesFolder: async (): Promise<void> => {
    await call("open_instances_folder");
  },
  /**
   * Installs everything the profile needs (Minecraft, Java, the loader) and marks it Ready, then
   * resolves with the updated profile list. Rejects with a `BackendError`; its `code` is
   * "cancelled" when the person stopped it. Safe to repeat: verified files are reused.
   */
  prepare: async (id: string, options?: { verify?: boolean }) =>
    parseProfiles(await call("prepare_profile", { id, verify: options?.verify === true })),
  cancelPrepare: async (): Promise<void> => {
    await call("cancel_prepare");
  },
  /**
   * Removes a profile from Clade WITHOUT deleting anything: its whole folder (worlds, mods,
   * settings, screenshots, everything) is moved to `instances/.removed/` inside the Clade folder.
   * Resolves with the updated list and a valid selection. Rejects with a `BackendError` whose code
   * says why not (the profile is running or being set up, or the folder could not be moved).
   */
  remove: async (id: string) => parseProfiles(await call("remove_profile", { id })),
  /** Restores a retained profile from `instances/.removed/<folder>` back into `instances/`. */
  restore: async (folder: string) => parseProfiles(await call("restore_profile", { folder })),
  /** Opens `instances/.removed/` inside the Clade folder in File Explorer. */
  openRemovedFolder: async (): Promise<void> => {
    await call("open_removed_profiles_folder");
  },
  /** Progress events of a running preparation. Returns a function that stops listening. */
  onPrepareProgress: async (callback: (p: PrepareProgress) => void): Promise<() => void> => {
    if (!inTauri()) return () => undefined;
    return listen<unknown>("prepare-progress", (e) => {
      const p = parsePrepareProgress(e.payload);
      if (p) callback(p);
    });
  },
};

export type ProfilesApi = Omit<typeof profilesApi, "restore" | "openRemovedFolder"> &
  Partial<Pick<typeof profilesApi, "restore" | "openRemovedFolder">>;
