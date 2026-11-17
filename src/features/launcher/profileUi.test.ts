import { afterEach, describe, expect, it, vi } from "vitest";
import { setLocale } from "@/lib/i18n";
import type { CatalogEntry, ProfileSummary } from "@/lib/profiles/api";
import {
  buildLabel,
  defaultVersion,
  filterVersions,
  formatBytes,
  formatGigabytes,
  formatPlaytime,
  itemsText,
  profileDetail,
  profileLabel,
  releaseDay,
  staleNote,
  validateName,
} from "./profileUi";

afterEach(() => {
  setLocale("en");
  vi.restoreAllMocks();
});

const p = (over: Partial<ProfileSummary> = {}): ProfileSummary => ({
  id: "a",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: false,
  playtimeSeconds: 0,
  ...over,
});

const e = (id: string, kind: CatalogEntry["kind"]): CatalogEntry => ({
  id,
  kind,
  releaseTime: "2024-12-03T10:12:57+00:00",
});
const entries = [
  e("26.4-snapshot-2", "snapshot"),
  e("26.3", "release"),
  e("1.21.4", "release"),
  e("1.14 Pre-Release 1", "snapshot"),
  e("b1.7.3", "old_beta"),
  e("rd-132211", "old_alpha"),
];

describe("labels", () => {
  it("the dock label is only the profile's own name, never the version or loader", () => {
    expect(profileLabel(p())).toBe("Survival");
    expect(
      profileLabel(p({ name: "Modded", kind: "modded", loader: "fabric", minecraft: "1.20.1" })),
    ).toBe("Modded");
    // Technical words appear only when they are literally part of the name.
    expect(profileLabel(p({ name: "Vanilla" }))).toBe("Vanilla");
    expect(
      profileLabel(p({ name: "My Forge", kind: "modded", loader: "forge", minecraft: "1.21.4" })),
    ).toBe("My Forge");
    // The technical details stay available for the profile menu.
    expect(profileDetail(p({ kind: "modded", loader: "fabric", loaderVersion: "0.16.9" }))).toBe(
      "Fabric 1.21.4 · 0.16.9",
    );
    expect(buildLabel(p({ kind: "modded", loader: "neoforge", minecraft: "26.1" }))).toBe(
      "NeoForge 26.1",
    );
  });

  it("adds the loader version to the detail line when it is known", () => {
    expect(profileDetail(p())).toBe("Vanilla 1.21.4");
    expect(profileDetail(p({ kind: "modded", loader: "forge", loaderVersion: "54.1.14" }))).toBe(
      "Forge 1.21.4 · 54.1.14",
    );
    expect(profileDetail(p({ kind: "modded", loader: "quilt" }))).toBe("Quilt 1.21.4");
  });
});

describe("version filters", () => {
  it("defaults to stable releases and keeps snapshots and historical versions behind their filters", () => {
    expect(filterVersions(entries, "releases").map((x) => x.id)).toEqual(["26.3", "1.21.4"]);
    expect(filterVersions(entries, "snapshots").map((x) => x.id)).toEqual([
      "26.4-snapshot-2",
      "1.14 Pre-Release 1",
    ]);
    expect(filterVersions(entries, "historical").map((x) => x.id)).toEqual(["b1.7.3", "rd-132211"]);
  });

  it("preselects the official latest where it exists and the newest listed otherwise", () => {
    const cat = { latestRelease: "26.3", latestSnapshot: "26.4-snapshot-2" };
    expect(defaultVersion(cat, filterVersions(entries, "releases"), "releases")).toBe("26.3");
    expect(defaultVersion(cat, filterVersions(entries, "snapshots"), "snapshots")).toBe(
      "26.4-snapshot-2",
    );
    expect(defaultVersion(cat, filterVersions(entries, "historical"), "historical")).toBe("b1.7.3");
    expect(
      defaultVersion(
        { latestRelease: "9.9", latestSnapshot: null },
        filterVersions(entries, "releases"),
        "releases",
      ),
    ).toBe("26.3");
    expect(defaultVersion(cat, [], "releases")).toBeNull();
  });
});

describe("validateName", () => {
  it("accepts reasonable names and trims", () => {
    expect(validateName("  Survival  ", [])).toBeNull();
    expect(validateName("Ünïcode ✨ world", [])).toBeNull();
  });

  it("rejects empty, long, control-character and duplicate names (ignoring case)", () => {
    expect(validateName("   ", [])).toBe("validate.nameEmpty");
    expect(validateName("x".repeat(65), [])).toBe("validate.nameLong");
    expect(validateName("a\u0007b", [])).toBe("validate.nameControl");
    expect(validateName("survival", ["Survival"])).toBe("validate.nameDuplicate");
    expect(validateName("  SURVIVAL ", ["Survival"])).toBe("validate.nameDuplicate");
    expect(validateName("Creative", ["Survival"])).toBeNull();
  });
});

describe("formatPlaytime", () => {
  it("is compact and understated", () => {
    expect(formatPlaytime(0)).toBe("0m");
    expect(formatPlaytime(30)).toBe("<1m");
    expect(formatPlaytime(60)).toBe("1m");
    expect(formatPlaytime(42 * 60 + 59)).toBe("42m");
    expect(formatPlaytime(3600)).toBe("1h 0m");
    expect(formatPlaytime(27 * 3600 + 42 * 60)).toBe("27h 42m");
    expect(formatPlaytime(1234 * 3600 + 5 * 60)).toBe("1234h 5m");
    expect(formatPlaytime(-5)).toBe("0m");
  });
});

describe("small helpers", () => {
  it("extracts the release day and words the offline note", () => {
    expect(releaseDay("2024-12-03T10:12:57+00:00")).toBe("2024-12-03");
    expect(releaseDay("")).toBe("");
    const now = Date.UTC(2026, 9, 4);
    expect(staleNote(Date.UTC(2026, 9, 3) / 1000, now)).toBe(
      "Offline: using the version list from 3 Oct.",
    );
    expect(staleNote(Date.UTC(2025, 0, 15, 12) / 1000, now)).toContain("2025");
    expect(staleNote(0)).toMatch(/saved version list/);
  });
});

describe("preparation text", () => {
  it("formats sizes and counts without inventing totals", () => {
    expect(formatBytes(512)).toBe("1 KB");
    expect(formatBytes(184 * 1024 * 1024)).toBe("184 MB");
    expect(formatBytes(1.4 * 1024 * 1024 * 1024)).toBe("1.4 GB");
    expect(itemsText(412, 640)).toBe("412 of 640 files");
    expect(itemsText(null, 640)).toBeNull();
    expect(itemsText(3, null)).toBeNull();
    expect(itemsText(0, 0)).toBeNull();
  });

  it("caches Intl.NumberFormat and Intl.DateTimeFormat instances across progress ticks and locales", () => {
    const OriginalNumberFormat = Intl.NumberFormat;
    const OriginalDateTimeFormat = Intl.DateTimeFormat;
    let nfCalls = 0;
    let dfCalls = 0;
    vi.spyOn(Intl, "NumberFormat").mockImplementation(function (
      ...args: ConstructorParameters<typeof Intl.NumberFormat>
    ) {
      nfCalls++;
      return new OriginalNumberFormat(...args);
    });
    vi.spyOn(Intl, "DateTimeFormat").mockImplementation(function (
      ...args: ConstructorParameters<typeof Intl.DateTimeFormat>
    ) {
      dfCalls++;
      return new OriginalDateTimeFormat(...args);
    });

    // Cold cache for (en, 0..1) and (ru, 0..1) + (ru, 1..1) + (ru, date).
    expect(formatGigabytes(4096)).toBe("4 GB");
    expect(formatGigabytes(4608)).toBe("4.5 GB");
    for (let i = 0; i < 20; i++) {
      formatGigabytes(2048 + i * 512);
      formatBytes((1.1 + i * 0.1) * 1024 * 1024 * 1024);
    }
    expect(nfCalls).toBe(1); // formatBytes(en) was already cached above; formatGigabytes(en) constructed once

    setLocale("ru");
    expect(formatGigabytes(4608)).toBe("4,5 ГБ");
    expect(formatBytes(1.4 * 1024 * 1024 * 1024)).toBe("1,4 ГБ");
    const now = Date.UTC(2026, 9, 4);
    for (let i = 0; i < 10; i++) {
      formatGigabytes(4096);
      formatBytes(2.2 * 1024 * 1024 * 1024);
      staleNote(Date.UTC(2026, 9, 3) / 1000, now);
    }
    // Only 2 more NumberFormat constructions for Russian (0..1 and 1..1) and 1 DateTimeFormat for Russian (md).
    expect(nfCalls).toBe(3);
    expect(dfCalls).toBe(1);
  });
});
