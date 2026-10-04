// Every test starts in English (the canonical language) unless it switches on purpose.
import { afterEach, beforeEach } from "vitest";
import { setLocale } from "@/lib/i18n";

beforeEach(() => setLocale("en"));
afterEach(() => setLocale("en"));
