/// <reference types="node" />
// The browser E2E suite runs the real frontend against a fake backend that Playwright injects from
// `e2e/support/`. None of that may leak into the application: production code must not define the
// Tauri bridge itself, import test support or Tauri's mock helpers, or carry fixture identities. This
// scans the frontend source so a release build can never contain a fake backend or fake accounts.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

function bridgeAssignmentRules(name: string): [RegExp, string][] {
  return [
    [
      new RegExp(
        `(?:\\b${name}\\b|["'\`]${name}["'\`]\\s*\\])\\s*(?:\\?\\?|\\|\\||&&|[+\\-*/%&|^])?=[^=>]`,
      ),
      `assigns window.${name}`,
    ],
    [
      new RegExp(
        `\\b(?:Object\\s*\\.\\s*defineProperty|Reflect\\s*\\.\\s*(?:defineProperty|set))\\s*\\((?:[^;{}()]|\\([^;{}()]*\\))*["'\`]${name}["'\`]`,
      ),
      `defines window.${name}`,
    ],
    [
      new RegExp(
        `\\bObject\\s*\\.\\s*(?:assign|defineProperties)\\s*\\((?:[^;()]|\\([^;()]*\\))*(?:\\b${name}\\b|["'\`]${name}["'\`](?:\\s*\\])?)\\s*:`,
      ),
      `assigns window.${name} via Object`,
    ],
  ];
}

const IMPORT_PREFIX = "\\b(?:from\\s+|import\\s+|(?:import|require)\\s*\\(\\s*)[\"'`]";

const FORBIDDEN: [RegExp, string][] = [
  ...bridgeAssignmentRules("__TAURI_INTERNALS__"),
  ...bridgeAssignmentRules("__TAURI_EVENT_PLUGIN_INTERNALS__"),
  [/__e2e\b/, "refers to the E2E control object"],
  [/@tauri-apps\/api\/mocks/, "imports Tauri's mock helpers"],
  [new RegExp(`${IMPORT_PREFIX}[^"'\`]*\\be2e[\\\\/]`), "imports from e2e/"],
  [
    new RegExp(`${IMPORT_PREFIX}[^"'\`]*(?:@|\\bsrc|\\.\\.?)[\\\\/]test(?:[\\\\/]|["'\`])`),
    "imports from src/test/",
  ],
  [/\bFixture(Alex|Blake)\b/, "contains a fixture account"],
];

function sourceFiles(dir: string, testDir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (p !== testDir) sourceFiles(p, testDir, out);
    } else if (/\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name)) {
      out.push(p);
    }
  }
  return out;
}

const REGEX_PREFIX_KEYWORDS = new Set([
  "return",
  "throw",
  "case",
  "default",
  "delete",
  "void",
  "typeof",
  "in",
  "instanceof",
  "new",
  "yield",
  "await",
  "else",
  "do",
  "export",
]);

/** Drops comments while preserving string, template, and regex literals. */
function code(text: string): string {
  let out = "";
  let i = 0;
  let canStartRegex = true;
  const templateBraceDepth: number[] = [];

  const scanTemplateSegment = () => {
    while (i < text.length) {
      const ch = text[i];
      if (ch === "\\") {
        out += text.slice(i, i + 2);
        i += 2;
      } else if (ch === "`") {
        out += "`";
        i += 1;
        canStartRegex = false;
        return;
      } else if (ch === "$" && text[i + 1] === "{") {
        out += "${";
        i += 2;
        templateBraceDepth.push(0);
        canStartRegex = true;
        return;
      } else {
        out += ch;
        i += 1;
      }
    }
  };

  while (i < text.length) {
    const ch = text[i];

    if (ch === " " || ch === "\t" || ch === "\r" || ch === "\n") {
      out += ch;
      i += 1;
      continue;
    }

    if (ch === "/" && text[i + 1] === "/") {
      i += 2;
      while (i < text.length && text[i] !== "\n" && text[i] !== "\r") i += 1;
      continue;
    }

    if (ch === "/" && text[i + 1] === "*") {
      i += 2;
      while (i < text.length && !(text[i] === "*" && text[i + 1] === "/")) i += 1;
      if (i < text.length) i += 2;
      out += " ";
      continue;
    }

    if (ch === '"' || ch === "'") {
      const quote = ch;
      out += quote;
      i += 1;
      while (i < text.length) {
        const c = text[i];
        if (c === "\\") {
          out += text.slice(i, i + 2);
          i += 2;
        } else {
          out += c;
          i += 1;
          if (c === quote) break;
        }
      }
      canStartRegex = false;
      continue;
    }

    if (ch === "`") {
      out += "`";
      i += 1;
      scanTemplateSegment();
      continue;
    }

    if (ch === "/" && canStartRegex) {
      let j = i + 1;
      let inClass = false;
      let closed = false;
      while (j < text.length && text[j] !== "\n" && text[j] !== "\r") {
        const c = text[j];
        if (c === "\\") {
          j += 2;
        } else if (c === "[") {
          inClass = true;
          j += 1;
        } else if (c === "]" && inClass) {
          inClass = false;
          j += 1;
        } else if (c === "/" && !inClass) {
          closed = true;
          j += 1;
          break;
        } else {
          j += 1;
        }
      }
      if (closed) {
        out += text.slice(i, j);
        i = j;
        canStartRegex = false;
        continue;
      }
    }

    if (ch === "<" && text[i + 1] === "/") {
      out += "</";
      i += 2;
      canStartRegex = false;
      continue;
    }

    if ((ch === "+" && text[i + 1] === "+") || (ch === "-" && text[i + 1] === "-")) {
      out += text.slice(i, i + 2);
      i += 2;
      canStartRegex = false;
      continue;
    }

    if (ch === "=" && text[i + 1] === ">") {
      out += "=>";
      i += 2;
      canStartRegex = true;
      continue;
    }

    if (/[A-Za-z_$]/.test(ch)) {
      let j = i + 1;
      while (j < text.length && /[A-Za-z0-9_$]/.test(text[j])) j += 1;
      const ident = text.slice(i, j);
      out += ident;
      i = j;
      canStartRegex = REGEX_PREFIX_KEYWORDS.has(ident);
      continue;
    }

    if (/[0-9]/.test(ch)) {
      let j = i + 1;
      while (j < text.length && /[0-9A-Za-z_.]/.test(text[j])) j += 1;
      out += text.slice(i, j);
      i = j;
      canStartRegex = false;
      continue;
    }

    if (ch === "{") {
      if (templateBraceDepth.length > 0) templateBraceDepth[templateBraceDepth.length - 1] += 1;
      out += ch;
      i += 1;
      canStartRegex = true;
      continue;
    }

    if (ch === "}") {
      out += ch;
      i += 1;
      if (templateBraceDepth.length > 0) {
        if (templateBraceDepth[templateBraceDepth.length - 1] === 0) {
          templateBraceDepth.pop();
          scanTemplateSegment();
          continue;
        }
        templateBraceDepth[templateBraceDepth.length - 1] -= 1;
      }
      canStartRegex = false;
      continue;
    }

    out += ch;
    i += 1;
    canStartRegex = ch !== ")" && ch !== "]";
  }

  return out;
}

describe("test fixtures never ship in the application", () => {
  const root = join(process.cwd(), "src");
  const files = sourceFiles(root, join(root, "test"));

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
      "window.__TAURI_INTERNALS__ ??= {}",
      'window["__TAURI_INTERNALS__"] = {}',
      'Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {} })',
      "Object.assign(window, { __TAURI_INTERNALS__: {} })",
      "w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {}",
      'w["__TAURI_EVENT_PLUGIN_INTERNALS__"] = {}',
      'Reflect.set(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {})',
      'Object.defineProperties(window, { "__TAURI_EVENT_PLUGIN_INTERNALS__": { value: {} } })',
      "window.__e2e.emit()",
      'import { mockIPC } from "@tauri-apps/api/mocks"',
      'import { x } from "../../e2e/support/fixtures"',
      'import "../../e2e/support/fixtures"',
      'const m = await import("../../e2e/support/fixtures")',
      'import /* comment */ "../../e2e/support/fixtures"',
      'import { readyAccountsClient } from "@/test/testAccounts"',
      'import { testSkin } from "../test/testSkin"',
      'import "./test/setup"',
      'const t = await import("@/test/testAccounts")',
      'const name = "FixtureAlex"',
      'const s = "/*"; window.__e2e.emit(); const t = "*/";',
      'const u = "https://example.com // keep"; window.__e2e.emit();',
      "const re = /\\/\\*/; window.__e2e.emit();",
    ];
    for (const snippet of bad) {
      const stripped = code(snippet);
      expect(
        FORBIDDEN.some(([re]) => re.test(stripped)),
        snippet,
      ).toBe(true);
    }

    const allowed = [
      // reading the bridge, which the app does to detect Tauri, is fine
      '"__TAURI_INTERNALS__" in window',
      'const x = window["__TAURI_INTERNALS__"]',
      "if (window.__TAURI_INTERNALS__ === undefined) {}",
      "// window.__TAURI_INTERNALS__ = {}",
      '/* import { x } from "../../e2e/support/fixtures" */',
      '`${/* window.__e2e.emit() */ "ok"}`',
      'import { x } from "./latest/helper"',
    ];
    for (const snippet of allowed) {
      const stripped = code(snippet);
      expect(
        FORBIDDEN.some(([re]) => re.test(stripped)),
        snippet,
      ).toBe(false);
    }
  });
});
