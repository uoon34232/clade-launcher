// Synthetic data for the browser E2E suite. Everything here is obviously fake: the accounts are
// "Fixture…" with all-zero UUIDs, the version list is a short invented catalog, and nothing is ever
// sent anywhere. This file is test-only and is never imported by the application.

export interface FixtureProfile {
  id: string;
  name: string;
  minecraft: string;
  kind: "vanilla" | "modded";
  loader: "fabric" | "forge" | "neoforge" | "quilt" | null;
  loaderVersion: string | null;
  ready: boolean;
  playtimeSeconds: number;
}

export interface FixtureAccount {
  id: string;
  username: string;
  uuid: string;
  needsSignIn: boolean;
  head: string | null;
}

export interface FixtureUiSettings {
  soundEnabled: boolean;
  soundVolume: number;
  lastPhrase: string | null;
  language: "en" | "ru" | null;
  systemLocale: string | null;
  reducedMotion: "system" | "on" | "off";
  transparency: "system" | "on" | "off";
  onboardingDone: boolean;
  memoryMb: number | null;
  /** What the (synthetic) computer allows, in MiB. */
  memory: { min: number; max: number; auto: number; total: number | null };
  onLaunch: "keep_open" | "hide";
  onExit: "show" | "stay_hidden";
  version: string;
}

export interface FixtureLoaderOption {
  loader: "fabric" | "forge" | "neoforge" | "quilt";
  status: "ok" | "none" | "unavailable";
  versions: { version: string; channel: "recommended" | "stable" | "latest" | "prerelease" }[];
  recommended: string | null;
  stale: boolean;
  error: string | null;
}

export interface FixtureCatalog {
  entries: {
    id: string;
    kind: "release" | "snapshot" | "old_beta" | "old_alpha";
    releaseTime: string;
  }[];
  latestRelease: string | null;
  latestSnapshot: string | null;
  fetchedAt: number;
  origin: "network" | "cache_fresh" | "cache_stale";
  stale: boolean;
  networkError: string | null;
}

export interface BackendConfig {
  profiles: FixtureProfile[];
  selected: string | null;
  accounts: FixtureAccount[];
  active: string | null;
  ui: FixtureUiSettings;
  catalog: FixtureCatalog;
  /** Loader offers by Minecraft version; a version not listed offers every loader. */
  loaders: Record<string, FixtureLoaderOption[]>;
  /** Games that are already running when the window opens. */
  runs: Record<string, "launching" | "playing">;
  /** The (synthetic) Windows "Transparency effects" preference, and whether High Contrast is on. */
  windows: { transparencyEffects: boolean; highContrast: boolean };
}

const ZERO_UUID = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

export const ALEX: FixtureAccount = {
  id: "acc-alex",
  username: "FixtureAlex",
  uuid: ZERO_UUID(1),
  needsSignIn: false,
  head: null,
};
export const BLAKE: FixtureAccount = {
  id: "acc-blake",
  username: "FixtureBlake",
  uuid: ZERO_UUID(2),
  needsSignIn: false,
  head: null,
};

export const SURVIVAL: FixtureProfile = {
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 5400,
};
export const CREATIVE: FixtureProfile = {
  id: "creative",
  name: "Creative",
  minecraft: "1.21.4",
  kind: "modded",
  loader: "fabric",
  loaderVersion: "0.16.9",
  ready: true,
  playtimeSeconds: 1200,
};
/** A profile that is not prepared yet: the dock offers "Set up". */
export const FRESH: FixtureProfile = {
  id: "fresh",
  name: "Fresh",
  minecraft: "1.20.1",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: false,
  playtimeSeconds: 0,
};

const offer = (
  v: string,
  channel: "recommended" | "stable" | "latest" | "prerelease" = "stable",
) => ({
  version: v,
  channel,
});

const ok = (
  loader: FixtureLoaderOption["loader"],
  versions: FixtureLoaderOption["versions"],
): FixtureLoaderOption => ({
  loader,
  status: "ok",
  versions,
  recommended: versions.find((v) => v.channel === "recommended")?.version ?? null,
  stale: false,
  error: null,
});
const none = (loader: FixtureLoaderOption["loader"]): FixtureLoaderOption => ({
  loader,
  status: "none",
  versions: [],
  recommended: null,
  stale: false,
  error: null,
});

export const CATALOG: FixtureCatalog = {
  entries: [
    { id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" },
    { id: "1.21.1", kind: "release", releaseTime: "2024-08-08T12:24:45+00:00" },
    { id: "1.20.1", kind: "release", releaseTime: "2023-06-12T13:25:51+00:00" },
    { id: "1.19.4", kind: "release", releaseTime: "2023-03-14T12:56:18+00:00" },
    { id: "24w50a", kind: "snapshot", releaseTime: "2024-12-11T14:00:00+00:00" },
    { id: "b1.7.3", kind: "old_beta", releaseTime: "2011-07-08T00:00:00+00:00" },
    { id: "a1.2.6", kind: "old_alpha", releaseTime: "2010-12-03T00:00:00+00:00" },
  ],
  latestRelease: "1.21.4",
  latestSnapshot: "24w50a",
  fetchedAt: 1_733_000_000,
  origin: "network",
  stale: false,
  networkError: null,
};

export const LOADERS: Record<string, FixtureLoaderOption[]> = {
  "1.21.4": [
    ok("fabric", [offer("0.16.9", "recommended"), offer("0.16.8")]),
    ok("forge", [offer("54.0.1", "recommended"), offer("54.0.0", "latest")]),
    ok("neoforge", [offer("21.4.10", "recommended"), offer("21.4.9")]),
    ok("quilt", [offer("0.27.1", "recommended"), offer("0.27.0")]),
  ],
  // NeoForge has no build for 1.20.1: that loader must be offered as unavailable, never guessed.
  "1.20.1": [
    ok("fabric", [offer("0.15.11", "recommended")]),
    ok("forge", [offer("47.3.0", "recommended")]),
    none("neoforge"),
    ok("quilt", [offer("0.26.4", "recommended")]),
  ],
};

export const DEFAULT_CONFIG: BackendConfig = {
  profiles: [SURVIVAL, CREATIVE, FRESH],
  selected: "survival",
  accounts: [ALEX, BLAKE],
  active: ALEX.id,
  ui: {
    soundEnabled: true,
    soundVolume: 0.5,
    lastPhrase: null,
    // Explicit English, so the run does not depend on the machine's language.
    language: "en",
    systemLocale: "en-US",
    reducedMotion: "system",
    transparency: "system",
    // Most tests are not about the introduction, so it counts as already seen; its own tests turn it on.
    onboardingDone: true,
    memoryMb: null,
    memory: { min: 1024, max: 12288, auto: 4096, total: 16384 },
    onLaunch: "keep_open",
    onExit: "show",
    version: "0.1.0",
  },
  catalog: CATALOG,
  loaders: LOADERS,
  runs: {},
  windows: { transparencyEffects: true, highContrast: false },
};
