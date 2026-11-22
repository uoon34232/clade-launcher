// Every test starts in English (the canonical language) unless it switches on purpose.
import { configure } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { setLocale } from "@/lib/i18n";

// Give React.lazy modal chunks enough headroom when 35 jsdom workers contend on cold import.
configure({ asyncUtilTimeout: 3000 });

beforeEach(() => setLocale("en"));
afterEach(() => setLocale("en"));
