// Pure presentation helpers for profiles and the version list. No I/O.

import { INTL_TAG, getLocale, t, type MessageKey } from "@/lib/i18n";
import {
  LOADER_NAMES,
  type CatalogEntry,
  type ProfileSummary,
  type VersionCatalog,
} from "@/lib/profiles/api";

export type VersionFilter = "releases" | "snapshots" | "historical";

export const FILTER_KEYS: Readonly<Record<VersionFilter, MessageKey>> = {
  releases: "create.filter.releases",
  snapshots: "create.filter.snapshots",
  historical: "create.filter.historical",
};

/** "Vanilla 1.21.4" or "Fabric 1.20.1" (the loader's own name is a product name and stays as is). */
export function buildLabel(p: Pick<ProfileSummary, "kind" | "loader" | "minecraft">): string {
  const what = p.kind === "vanilla" || !p.loader ? t("profile.vanilla") : LOADER_NAMES[p.loader];
  return `${what} ${p.minecraft}`;
}

/**
 * The dock button's text: only the name the person gave the profile ("Survival"). Minecraft
 * version, loader and the like belong to the profile menu (see `profileDetail`), never here.
 */
export function profileLabel(p: Pick<ProfileSummary, "name">): string {
  return p.name;
}

/** Second line in the profile menu: the build, plus the loader version when known. */
export function profileDetail(p: ProfileSummary): string {
  return p.kind === "modded" && p.loaderVersion
    ? `${buildLabel(p)} · ${p.loaderVersion}`
    : buildLabel(p);
}

/** Which catalog entries a filter shows. Releases is the default; Historical is the old beta and alpha versions. */
export function filterVersions(
  entries: readonly CatalogEntry[],
  filter: VersionFilter,
): CatalogEntry[] {
  const kinds =
    filter === "releases"
      ? ["release"]
      : filter === "snapshots"
        ? ["snapshot"]
        : ["old_beta", "old_alpha"];
  return entries.filter((e) => kinds.includes(e.kind));
}

/** The version to preselect for a filter: the official "latest" where one exists, otherwise the newest listed. */
export function defaultVersion(
  catalog: Pick<VersionCatalog, "latestRelease" | "latestSnapshot">,
  shown: readonly CatalogEntry[],
  filter: VersionFilter,
): string | null {
  const latest =
    filter === "releases"
      ? catalog.latestRelease
      : filter === "snapshots"
        ? catalog.latestSnapshot
        : null;
  if (latest && shown.some((e) => e.id === latest)) return latest;
  return shown[0]?.id ?? null;
}

/** The longest profile name, in characters (the backend enforces the same limit). */
export const MAX_NAME_CHARS = 64;

/**
 * Why a name is not acceptable, as a translation key, or null. Same rules as the backend (which
 * is authoritative): 1-64 characters, no control characters, unique ignoring case.
 */
export function validateName(name: string, existing: readonly string[]): MessageKey | null {
  const n = name.trim();
  if (n.length === 0) return "validate.nameEmpty";
  if ([...n].length > MAX_NAME_CHARS) return "validate.nameLong";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(n)) return "validate.nameControl";
  if (existing.some((e) => e.trim().toLowerCase() === n.toLowerCase()))
    return "validate.nameDuplicate";
  return null;
}

/**
 * "27h 42m" / "27 ч 42 мин", "42m", "<1m". Zero is the caller's business (the home screen hides
 * it). Only the raw duration is ever stored; this formats it for the current language.
 */
export function formatPlaytime(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return s === 0 ? t("playtime.zero") : t("playtime.lessThanMinute");
  const hours = Math.floor(s / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  return hours === 0
    ? t("playtime.minutes", { minutes })
    : t("playtime.hoursMinutes", { hours, minutes });
}

/** "184 MB", "1.4 GB" (a decimal comma in Russian). */
export function formatBytes(n: number): string {
  if (n < 1024 * 1024) return t("size.kb", { value: Math.max(1, Math.round(n / 1024)) });
  if (n < 1024 * 1024 * 1024) return t("size.mb", { value: Math.round(n / (1024 * 1024)) });
  const gb = new Intl.NumberFormat(INTL_TAG[getLocale()], {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(n / (1024 * 1024 * 1024));
  return t("size.gb", { value: gb });
}

/** Memory given in MiB, as "4 GB" / "4,5 ГБ" (at most one decimal, none when it is a whole number). */
export function formatGigabytes(mib: number): string {
  const value = new Intl.NumberFormat(INTL_TAG[getLocale()], {
    minimumFractionDigits: 0,
    maximumFractionDigits: 1,
  }).format(mib / 1024);
  return t("settings.memory.gb", { value });
}

/** "412 of 640 files", or null when the total is unknown. */
export function itemsText(done: number | null, total: number | null): string | null {
  return done !== null && total !== null && total > 0 ? t("prepare.files", { done, total }) : null;
}

/** "2024-12-03" from an ISO timestamp, for the subtle date next to a version. */
export function releaseDay(releaseTime: string): string {
  return /^\d{4}-\d{2}-\d{2}/.test(releaseTime) ? releaseTime.slice(0, 10) : "";
}

/** "Offline: using the version list from 3 Oct." for a stale catalog. */
export function staleNote(fetchedAt: number, now = Date.now()): string {
  if (!fetchedAt) return t("catalog.staleUnknown");
  const d = new Date(fetchedAt * 1000);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  const date = d.toLocaleDateString(INTL_TAG[getLocale()], {
    day: "numeric",
    month: "short",
    ...(sameYear ? {} : { year: "numeric" }),
  });
  // Russian short months end in a dot ("сент."); the sentence supplies its own full stop.
  return t("catalog.stale", { date: date.replace(/\.$/, "") });
}
