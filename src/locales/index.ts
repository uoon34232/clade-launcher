// The two catalogs Clade ships. English is canonical: its keys define `MessageKey`, and the
// Russian catalog is typed against it, so a missing or extra key fails the build.

import type { Catalog, Locale } from "@/lib/i18n/core";
import { en } from "./en";
import { ru } from "./ru";

export type MessageKey = keyof typeof en;

export const CATALOGS: Readonly<Record<Locale, Catalog>> = { en, ru };

export { en, ru };
