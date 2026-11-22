/// <reference types="node" />
// Launcher-owned state must live in the Rust settings service (data/settings.json), never in
// the WebView's storage: the WebView profile (data/webview2) is disposable browser-engine cache
// that can be deleted at any time. This scans the frontend source so that stays true.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
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

const COMMENT_OR_STRING =
  /"(?:\\.|[^"\\\r\n])*"|'(?:\\.|[^'\\\r\n])*'|`(?:\\.|[^`\\])*`|\/\*[\s\S]*?\*\/|\/\/[^\r\n]*/g;

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) sourceFiles(p, out);
    else if (/\.(ts|tsx|js|jsx|mjs|cjs|html)$/.test(name) && !/\.test\.[cm]?[jt]sx?$/.test(name))
      out.push(p);
  }
  return out;
}

/**
 * Drops comments in a single ordered pass while preserving string literals so
 * explanations may mention the forbidden APIs without `//` or string contents
 * blinding detection by opening/closing block comments.
 */
function code(text: string): string {
  return text.replace(COMMENT_OR_STRING, (match) => (match.startsWith("/") ? " " : match));
}

describe("the frontend never stores launcher state in the WebView", () => {
  const repoRoot = process.cwd();
  const root = join(repoRoot, "src");
  const indexHtml = join(repoRoot, "index.html");
  const files = existsSync(indexHtml) ? [...sourceFiles(root), indexHtml] : sourceFiles(root);

  it("scans a meaningful number of files", () => {
    expect(files.length).toBeGreaterThan(15);
  });

  it("uses no localStorage, sessionStorage, IndexedDB, cookies or Cache API in production code", () => {
    const hits: string[] = [];
    for (const f of files) {
      const text = code(readFileSync(f, "utf8"));
      for (const [re, name] of FORBIDDEN)
        if (re.test(text)) hits.push(`${relative(repoRoot, f)} uses ${name}`);
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
      "caches.match('/v1')",
      "caches.keys()",
      "openDatabase('db', '1.0', 'desc', 1024)",
      '// comment with /*\nlocalStorage.setItem("a", "b");\n/* later */',
      'const glob = "assets/*";\nlocalStorage.setItem("a", "b");\nconst end = "*/";',
      'const msg = "see // note"; sessionStorage.getItem("k");',
      "const tpl = `/*`;\nindexedDB.open('x');\nconst end = `*/`;",
      'return/*note*/localStorage.getItem("a")',
    ];
    for (const b of bad) {
      expect(
        FORBIDDEN.some(([re]) => re.test(code(b))),
        b,
      ).toBe(true);
    }

    const allowed = [
      "// localStorage is not used here",
      '/* localStorage.setItem("a", "b") */',
      "/*\n * window.sessionStorage\n * openDatabase('db', '1.0', 'd', 1)\n */",
    ];
    for (const a of allowed) {
      expect(
        FORBIDDEN.some(([re]) => re.test(code(a))),
        a,
      ).toBe(false);
    }
  });
});
