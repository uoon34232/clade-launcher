import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));

const listen =
  vi.fn<(event: string, cb: (e: { payload: unknown }) => void) => Promise<() => void>>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: unknown }) => void) => listen(event, cb),
}));

import { BackendError } from "@/lib/ipc";
import {
  parseCatalog,
  parseLoaderOptions,
  parseProfiles,
  profilesApi,
  type NewProfileRequest,
} from "./api";

const tauri = (on: boolean) => {
  if (on) (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  else delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue({});
  listen.mockReset();
  listen.mockResolvedValue(() => undefined);
});
afterEach(() => tauri(false));

describe("parsers validate backend data instead of trusting it", () => {
  it("profiles: keeps valid ones, drops malformed, repairs the selection", () => {
    const s = parseProfiles({
      profiles: [
        {
          id: "a",
          name: "A",
          minecraft: "1.21.4",
          kind: "vanilla",
          loader: null,
          loaderVersion: null,
          ready: false,
          playtimeSeconds: 61,
        },
        {
          id: "b",
          name: "B",
          minecraft: "1.20.1",
          kind: "modded",
          loader: "neoforge",
          loaderVersion: "21.4.1",
          ready: true,
          playtimeSeconds: -5,
        },
        { id: "c", name: "C", minecraft: "1", kind: "weird" },
        { name: "no id" },
        "junk",
        { id: "d", name: "D", minecraft: "1.0", kind: "modded", loader: "notaloader" },
      ],
      selected: "ghost",
      totalPlaytimeSeconds: 99720,
    });
    expect(s.profiles.map((p) => p.id)).toEqual(["a", "b", "d"]);
    expect(s.profiles[1]).toMatchObject({
      loader: "neoforge",
      loaderVersion: "21.4.1",
      ready: true,
      playtimeSeconds: 0,
    });
    expect(s.profiles[2].loader).toBeNull();
    expect(s.profiles[0].memoryMb).toBeNull();
    expect(s.profiles[0].resolution).toBeNull();
    expect(s.profiles[0].jvmArgs).toEqual([]);
    expect(s.selected).toBe("a");
    expect(s.totalPlaytimeSeconds).toBe(99720);
    expect(s.unreadable).toEqual([]);
    expect(s.removed).toEqual([]);
    expect(parseProfiles(null)).toEqual({
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      unreadable: [],
      removed: [],
    });
  });

  it("profiles: validates memoryMb overrides and unreadable instance folder names", () => {
    const mk = (id: string, memoryMb: unknown) => ({
      id,
      name: id,
      minecraft: "1.21.4",
      kind: "vanilla",
      memoryMb,
    });
    const s = parseProfiles({
      profiles: [
        mk("pos", 6144),
        mk("nul", null),
        mk("zero", 0),
        mk("neg", -512),
        mk("nan", Number.NaN),
        mk("inf", Number.POSITIVE_INFINITY),
        mk("str", "4096"),
      ],
      unreadable: ["corrupt-a", "", 42, null, "broken-b"],
    });
    expect(s.profiles.map((p) => [p.id, p.memoryMb])).toEqual([
      ["pos", 6144],
      ["nul", null],
      ["zero", null],
      ["neg", null],
      ["nan", null],
      ["inf", null],
      ["str", null],
    ]);
    expect(s.unreadable).toEqual(["corrupt-a", "broken-b"]);
    expect(parseProfiles({ profiles: [], unreadable: "not-an-array" }).unreadable).toEqual([]);
  });

  it("profiles: validates resolution bounds and jvmArgs strings", () => {
    const mk = (id: string, resolution: unknown, jvmArgs: unknown) => ({
      id,
      name: id,
      minecraft: "1.21.4",
      kind: "vanilla",
      resolution,
      jvmArgs,
    });
    const seventyArgs = Array.from({ length: 70 }, (_, i) => `-Darg${i}`);
    const maxLenArg = "a".repeat(512);
    const overLenArg = "b".repeat(513);
    const s = parseProfiles({
      profiles: [
        mk("ok", { width: 1280, height: 720 }, ["-XX:+UseStringDeduplication", "-ea"]),
        mk("min-bound", { width: 320, height: 240 }, [maxLenArg, overLenArg]),
        mk("max-bound", { width: 7680, height: 4320 }, seventyArgs),
        mk("small", { width: 100, height: 720 }, [
          "",
          42,
          "x".repeat(600),
          "-Dfml.readTimeout=120",
        ]),
        mk("low-w", { width: 319, height: 240 }, null),
        mk("low-h", { width: 1280, height: 239 }, null),
        mk("high-h", { width: 1280, height: 4321 }, null),
        mk("float", { width: 1280.5, height: 720 }, "not-an-array"),
        mk("float-h", { width: 1280, height: 720.5 }, null),
        mk("big", { width: 9999, height: 4320 }, null),
      ],
    });
    expect(s.profiles[0].resolution).toEqual({ width: 1280, height: 720 });
    expect(s.profiles[0].jvmArgs).toEqual(["-XX:+UseStringDeduplication", "-ea"]);
    expect(s.profiles[1].resolution).toEqual({ width: 320, height: 240 });
    expect(s.profiles[1].jvmArgs).toEqual([maxLenArg]);
    expect(s.profiles[2].resolution).toEqual({ width: 7680, height: 4320 });
    expect(s.profiles[2].jvmArgs).toHaveLength(64);
    expect(s.profiles[2].jvmArgs?.[0]).toBe("-Darg0");
    expect(s.profiles[2].jvmArgs?.[63]).toBe("-Darg63");
    expect(s.profiles[3].resolution).toBeNull();
    expect(s.profiles[3].jvmArgs).toEqual(["-Dfml.readTimeout=120"]);
    expect(s.profiles[4].resolution).toBeNull();
    expect(s.profiles[5].resolution).toBeNull();
    expect(s.profiles[6].resolution).toBeNull();
    expect(s.profiles[7].resolution).toBeNull();
    expect(s.profiles[7].jvmArgs).toEqual([]);
    expect(s.profiles[8].resolution).toBeNull();
    expect(s.profiles[9].resolution).toBeNull();
  });

  it("profiles: validates removed entries, loader fallback, playtime clamping, and removedAt", () => {
    const s = parseProfiles({
      profiles: [],
      removed: [
        {
          folder: "survival-100-abcd1234",
          name: "Survival",
          minecraft: "1.21.4",
          kind: "vanilla",
          loader: null,
          loaderVersion: null,
          playtimeSeconds: 3600,
          removedAt: 1730000000,
        },
        {
          folder: "modded-200-efgh5678",
          name: "Modded",
          minecraft: "1.20.1",
          kind: "modded",
          loader: "fabric",
          loaderVersion: "0.16.9",
          playtimeSeconds: -45,
          removedAt: 0,
        },
        {
          folder: "bad-loader-300",
          name: "BadLoader",
          minecraft: "1.20.1",
          kind: "modded",
          loader: "notaloader",
          loaderVersion: 42,
          playtimeSeconds: "nope",
          removedAt: -10,
        },
        {
          folder: "bad-ts-400",
          name: "BadTs",
          minecraft: "1.21.4",
          kind: "vanilla",
          removedAt: Number.NaN,
        },
        {
          folder: "inf-ts-500",
          name: "InfTs",
          minecraft: "1.21.4",
          kind: "vanilla",
          removedAt: Number.POSITIVE_INFINITY,
        },
        {
          folder: "str-ts-600",
          name: "StrTs",
          minecraft: "1.21.4",
          kind: "vanilla",
          removedAt: "1730000000",
        },
        { folder: "", name: "EmptyFolder", minecraft: "1.21.4", kind: "vanilla" },
        { folder: "missing-name", minecraft: "1.21.4", kind: "vanilla" },
        { folder: "weird-kind", name: "Weird", minecraft: "1.21.4", kind: "weird" },
        null,
        "junk",
      ],
    });
    expect(s.removed).toEqual([
      {
        folder: "survival-100-abcd1234",
        name: "Survival",
        minecraft: "1.21.4",
        kind: "vanilla",
        loader: null,
        loaderVersion: null,
        playtimeSeconds: 3600,
        removedAt: 1730000000,
      },
      {
        folder: "modded-200-efgh5678",
        name: "Modded",
        minecraft: "1.20.1",
        kind: "modded",
        loader: "fabric",
        loaderVersion: "0.16.9",
        playtimeSeconds: 0,
        removedAt: null,
      },
      {
        folder: "bad-loader-300",
        name: "BadLoader",
        minecraft: "1.20.1",
        kind: "modded",
        loader: null,
        loaderVersion: null,
        playtimeSeconds: 0,
        removedAt: null,
      },
      {
        folder: "bad-ts-400",
        name: "BadTs",
        minecraft: "1.21.4",
        kind: "vanilla",
        loader: null,
        loaderVersion: null,
        playtimeSeconds: 0,
        removedAt: null,
      },
      {
        folder: "inf-ts-500",
        name: "InfTs",
        minecraft: "1.21.4",
        kind: "vanilla",
        loader: null,
        loaderVersion: null,
        playtimeSeconds: 0,
        removedAt: null,
      },
      {
        folder: "str-ts-600",
        name: "StrTs",
        minecraft: "1.21.4",
        kind: "vanilla",
        loader: null,
        loaderVersion: null,
        playtimeSeconds: 0,
        removedAt: null,
      },
    ]);
    expect(parseProfiles({ profiles: [], removed: "not-an-array" }).removed).toEqual([]);
  });

  it("catalog: only known kinds, and an unknown origin counts as stale", () => {
    const c = parseCatalog({
      entries: [
        { id: "26.3", kind: "release", releaseTime: "2026-09-01T10:00:00+00:00" },
        { id: "x", kind: "hologram", releaseTime: "t" },
        { id: 5, kind: "release" },
        { id: "b1.7.3", kind: "old_beta", releaseTime: "2011" },
      ],
      latestRelease: "26.3",
      latestSnapshot: null,
      fetchedAt: 123,
      origin: "???",
      stale: false,
    });
    expect(c.entries.map((e) => e.id)).toEqual(["26.3", "b1.7.3"]);
    expect(c.origin).toBe("cache_stale");
    expect(c.latestRelease).toBe("26.3");
  });

  it("loader options: never trusts 'ok' without versions and skips unknown loaders", () => {
    const o = parseLoaderOptions([
      {
        loader: "fabric",
        status: "ok",
        versions: [
          { version: "0.19.5", channel: "stable" },
          { version: 3, channel: "stable" },
          { version: "x", channel: "bogus" },
        ],
        recommended: "0.19.5",
        stale: false,
        error: null,
      },
      { loader: "forge", status: "ok", versions: [], recommended: null },
      { loader: "quilt", status: "weird" },
      { loader: "spoon", status: "ok" },
    ]);
    expect(o.map((x) => x.loader)).toEqual(["fabric", "forge", "quilt"]);
    expect(o[0].versions).toEqual([{ version: "0.19.5", channel: "stable" }]);
    expect(o[1].status).toBe("none");
    expect(o[2].status).toBe("unavailable");
  });
});

describe("calls", () => {
  it("never invent data outside the launcher", async () => {
    tauri(false);
    await expect(profilesApi.list()).rejects.toThrow(/not available/);
    await expect(profilesApi.select("my-world")).rejects.toThrow(/not available/);
    await expect(profilesApi.versionCatalog()).rejects.toThrow(/not available/);
    await expect(profilesApi.loaderOptions("1.21.4")).rejects.toThrow(/not available/);
    await expect(
      profilesApi.create({ name: "N", minecraft: "1.21.4", profile: { type: "vanilla" } }),
    ).rejects.toThrow(/not available/);
    await expect(profilesApi.rename("survival", "Hardcore")).rejects.toThrow(/not available/);
    await expect(profilesApi.setMemory("survival", 6144)).rejects.toThrow(/not available/);
    await expect(
      profilesApi.setResolution("survival", { width: 1280, height: 720 }),
    ).rejects.toThrow(/not available/);
    await expect(profilesApi.setJvmArgs("survival", ["-ea"])).rejects.toThrow(/not available/);
    await expect(profilesApi.setVersion("survival", "1.21.4")).rejects.toThrow(/not available/);
    await expect(profilesApi.duplicate("survival")).rejects.toThrow(/not available/);
    await expect(profilesApi.openFolder("survival")).rejects.toThrow(/not available/);
    await expect(profilesApi.openInstancesFolder()).rejects.toThrow(/not available/);
    await expect(profilesApi.cancelPrepare()).rejects.toThrow(/not available/);
    await expect(profilesApi.remove("survival")).rejects.toThrow(/not available/);
    await expect(profilesApi.restore("survival-100-abcd1234")).rejects.toThrow(/not available/);
    await expect(profilesApi.openRemovedFolder()).rejects.toThrow(/not available/);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("call the matching backend commands with the documented arguments", async () => {
    tauri(true);
    await profilesApi.list();
    await profilesApi.select("my-world");
    await profilesApi.versionCatalog();
    await profilesApi.versionCatalog(true);
    await profilesApi.loaderOptions("1.21.4");
    const req: NewProfileRequest = {
      name: "N",
      minecraft: "1.21.4",
      profile: {
        type: "modded",
        loader: "fabric",
        loader_version: { mode: "pinned", version: "0.19.5" },
      },
    };
    await profilesApi.create(req);
    await profilesApi.rename("survival", "Hardcore");
    await profilesApi.setMemory("survival", 6144);
    await profilesApi.setMemory("survival", null);
    await profilesApi.setResolution("survival", { width: 1280, height: 720 });
    await profilesApi.setResolution("survival", null);
    await profilesApi.setJvmArgs("survival", ["-XX:+UseStringDeduplication"]);
    await profilesApi.setVersion("survival", "1.21.4");
    await profilesApi.setVersion("survival", "1.21.4", { mode: "pinned", version: "0.19.5" });
    await profilesApi.duplicate("survival");
    await profilesApi.openFolder("survival");
    await profilesApi.openInstancesFolder();
    await profilesApi.cancelPrepare();
    await profilesApi.remove("survival");
    await profilesApi.restore("survival-100-abcd1234");
    await profilesApi.openRemovedFolder();
    expect(invoke.mock.calls).toEqual([
      ["list_profiles", undefined],
      ["select_profile", { id: "my-world" }],
      ["get_version_catalog", { force: false }],
      ["get_version_catalog", { force: true }],
      ["get_loader_options", { minecraft: "1.21.4" }],
      ["create_profile", { request: req }],
      ["rename_profile", { id: "survival", name: "Hardcore" }],
      ["set_profile_memory", { id: "survival", mb: 6144 }],
      ["set_profile_memory", { id: "survival", mb: null }],
      ["set_profile_resolution", { id: "survival", resolution: { width: 1280, height: 720 } }],
      ["set_profile_resolution", { id: "survival", resolution: null }],
      ["set_profile_jvm_args", { id: "survival", args: ["-XX:+UseStringDeduplication"] }],
      ["set_profile_version", { id: "survival", minecraft: "1.21.4", loaderVersion: null }],
      [
        "set_profile_version",
        {
          id: "survival",
          minecraft: "1.21.4",
          loaderVersion: { mode: "pinned", version: "0.19.5" },
        },
      ],
      ["duplicate_profile", { id: "survival" }],
      ["open_profile_folder", { id: "survival" }],
      ["open_instances_folder", undefined],
      ["cancel_prepare", undefined],
      ["remove_profile", { id: "survival" }],
      ["restore_profile", { folder: "survival-100-abcd1234" }],
      ["open_removed_profiles_folder", undefined],
    ]);
  });

  it("surfaces string and structured CodedError rejections as BackendError", async () => {
    tauri(true);
    invoke.mockRejectedValueOnce("A profile named “Survival” already exists.");
    await expect(
      profilesApi.create({ name: "Survival", minecraft: "1.21.4", profile: { type: "vanilla" } }),
    ).rejects.toThrow(/already exists/);

    invoke.mockRejectedValueOnce({
      code: "profile_already_exists",
      params: { name: "Survival" },
      message: "A profile with this name already exists.",
    });
    let caught: unknown;
    try {
      await profilesApi.create({
        name: "Survival",
        minecraft: "1.21.4",
        profile: { type: "vanilla" },
      });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(BackendError);
    expect(caught).toMatchObject({
      code: "profile_already_exists",
      params: { name: "Survival" },
      message: "A profile with this name already exists.",
    });
  });
});

describe("preparation progress", () => {
  it("validates events and never turns an unknown total into a number", async () => {
    const { parsePrepareProgress } = await import("./api");
    const ok = parsePrepareProgress({
      instanceId: "a",
      stage: "assets",
      label: "Downloading assets",
      itemsDone: 3,
      itemsTotal: 10,
      bytesDone: null,
      bytesTotal: null,
      overall: null,
    });
    expect(ok).toMatchObject({
      stage: "assets",
      itemsDone: 3,
      itemsTotal: 10,
      bytesDone: null,
      overall: null,
    });
    expect(
      parsePrepareProgress({ instanceId: "a", stage: "libraries", label: "x", overall: 7 })
        ?.overall,
    ).toBe(1);
    expect(
      parsePrepareProgress({ instanceId: "a", stage: "libraries", label: "x", overall: -1 })
        ?.overall,
    ).toBeNull();
    expect(parsePrepareProgress({ instanceId: "a", stage: "warp", label: "x" })).toBeNull();
    expect(parsePrepareProgress({ stage: "java", label: "x" })).toBeNull();
    expect(parsePrepareProgress("junk")).toBeNull();
  });

  it("prepares through the backend and rejects outside the launcher", async () => {
    await expect(profilesApi.prepare("a")).rejects.toThrow("not available");
    tauri(true);
    invoke.mockResolvedValue({ profiles: [], selected: null, totalPlaytimeSeconds: 0 });
    await profilesApi.prepare("survival");
    expect(invoke).toHaveBeenCalledWith("prepare_profile", { id: "survival", verify: false });
    await profilesApi.prepare("survival", { verify: true });
    expect(invoke).toHaveBeenLastCalledWith("prepare_profile", { id: "survival", verify: true });
  });

  it("subscribes to prepare-progress events, filters invalid payloads, and cleans up", async () => {
    tauri(false);
    const cb = vi.fn();
    const noopUnlisten = await profilesApi.onPrepareProgress(cb);
    expect(listen).not.toHaveBeenCalled();
    noopUnlisten();
    expect(cb).not.toHaveBeenCalled();

    tauri(true);
    const unlistenMock = vi.fn();
    let emit: ((e: { payload: unknown }) => void) | undefined;
    listen.mockImplementationOnce((_event, handler) => {
      emit = handler;
      return Promise.resolve(unlistenMock);
    });

    const unlisten = await profilesApi.onPrepareProgress(cb);
    expect(listen).toHaveBeenCalledWith("prepare-progress", expect.any(Function));

    emit?.({ payload: "junk" });
    emit?.({ payload: { instanceId: "a", stage: "warp", label: "x" } });
    emit?.({ payload: { stage: "java", label: "x" } });
    expect(cb).not.toHaveBeenCalled();

    emit?.({
      payload: {
        instanceId: "survival",
        stage: "assets",
        label: "Downloading assets",
        itemsDone: 2,
        itemsTotal: 5,
        bytesDone: 1024,
        bytesTotal: 2048,
        overall: 0.4,
      },
    });
    expect(cb).toHaveBeenCalledTimes(1);
    expect(cb).toHaveBeenCalledWith({
      instanceId: "survival",
      stage: "assets",
      label: "Downloading assets",
      itemsDone: 2,
      itemsTotal: 5,
      bytesDone: 1024,
      bytesTotal: 2048,
      overall: 0.4,
    });

    unlisten();
    expect(unlistenMock).toHaveBeenCalledTimes(1);
  });
});
