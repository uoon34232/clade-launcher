/// <reference types="node" />
// Launcher-owned state must live in the Rust settings service (data/settings.json), never in
// the WebView's storage: the WebView profile (data/webview2) is disposable browser-engine cache
// that can be deleted at any time. This scans the frontend source so that stays true.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const FORBIDDEN: [RegExp, string][] = [
  [/\blocalStorage\b/, "localStorage"],
  [/\bsessionStorage\b/, "sessionStorage"],
  [/\bindexedDB\b/, "indexedDB"],
  [/document\s*\.\s*cookie/, "document.cookie"],
  [/\bcaches\s*\.\s*(open|match|keys)\b/, "Cache API"],
  [/\bopenDatabase\b/, "WebSQL"],
];

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) sourceFiles(p, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name)) out.push(p);
  }
  return out;
}

/** Drops comments so explanations may mention the forbidden APIs. */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

describe("the frontend never stores launcher state in the WebView", () => {
  const root = join(process.cwd(), "src");
  const files = sourceFiles(root);

  it("scans a meaningful number of files", () => {
    expect(files.length).toBeGreaterThan(15);
  });

  it("uses no localStorage, sessionStorage, IndexedDB, cookies or Cache API in production code", () => {
    const hits: string[] = [];
    for (const f of files) {
      const text = code(readFileSync(f, "utf8"));
      for (const [re, name] of FORBIDDEN)
        if (re.test(text)) hits.push(`${relative(root, f)} uses ${name}`);
    }
    expect(
      hits,
      "Persist launcher state through src/lib/settings/uiSettings.ts and the Rust settings service instead",
    ).toEqual([]);
  });

  it("detects violations (the guard itself works)", () => {
    const bad = [
      'localStorage.setItem("a","b")',
      "window.sessionStorage",
      "indexedDB.open('x')",
      "document.cookie = 'a=b'",
      "caches.open('v1')",
    ];
    for (const b of bad) expect(FORBIDDEN.some(([re]) => re.test(code(b)))).toBe(true);
    expect(FORBIDDEN.some(([re]) => re.test(code("// localStorage is not used here")))).toBe(false);
  });
});
