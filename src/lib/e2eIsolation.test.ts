/// <reference types="node" />
// The browser E2E suite runs the real frontend against a fake backend that Playwright injects from
// `e2e/support/`. None of that may leak into the application: production code must not define the
// Tauri bridge itself, import test support or Tauri's mock helpers, or carry fixture identities. This
// scans the frontend source so a release build can never contain a fake backend or fake accounts.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const FORBIDDEN: [RegExp, string][] = [
  [/__TAURI_INTERNALS__\s*=[^=]/, "assigns window.__TAURI_INTERNALS__"],
  [/__TAURI_EVENT_PLUGIN_INTERNALS__\s*=[^=]/, "assigns window.__TAURI_EVENT_PLUGIN_INTERNALS__"],
  [/__e2e\b/, "refers to the E2E control object"],
  [/@tauri-apps\/api\/mocks/, "imports Tauri's mock helpers"],
  [/from\s+["'][^"']*\be2e\//, "imports from e2e/"],
  [/\bFixture(Alex|Blake)\b/, "contains a fixture account"],
];

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) sourceFiles(p, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name)) out.push(p);
  }
  return out;
}

/** Drops comments so explanations may mention the forbidden names. */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

describe("test fixtures never ship in the application", () => {
  const root = join(process.cwd(), "src");
  const files = sourceFiles(root);

  it("scans a meaningful number of files", () => {
    expect(files.length).toBeGreaterThan(15);
  });

  it("production code has no fake backend, mock helper or fixture account", () => {
    const hits: string[] = [];
    for (const f of files) {
      const text = code(readFileSync(f, "utf8"));
      for (const [re, what] of FORBIDDEN)
        if (re.test(text)) hits.push(`${relative(root, f)} ${what}`);
    }
    expect(hits, "Fixtures belong in e2e/support/ and the unit tests, never in src/").toEqual([]);
  });

  it("detects violations (the guard itself works)", () => {
    const bad = [
      "window.__TAURI_INTERNALS__ = {}",
      "w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {}",
      "window.__e2e.emit()",
      'import { mockIPC } from "@tauri-apps/api/mocks"',
      'import { x } from "../../e2e/support/fixtures"',
      'const name = "FixtureAlex"',
    ];
    for (const snippet of bad) {
      expect(
        FORBIDDEN.some(([re]) => re.test(snippet)),
        snippet,
      ).toBe(true);
    }
    // reading the bridge, which the app does to detect Tauri, is fine
    expect(FORBIDDEN.some(([re]) => re.test('"__TAURI_INTERNALS__" in window'))).toBe(false);
  });
});
