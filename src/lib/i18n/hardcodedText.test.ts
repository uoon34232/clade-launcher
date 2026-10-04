/// <reference types="node" />
// A guard against new user-visible text slipping in outside the localization layer. It reads the
// real syntax tree (not a regular expression over text), so it flags what a person would actually
// see and leaves technical strings alone:
//   - text written directly between JSX tags,
//   - string literals inside a JSX child expression (`{cond ? "Yes" : "No"}`),
//   - string values of user-facing props (`aria-label`, `placeholder`, `title`, ...),
//   - string values of user-facing object properties (`label`, `title`, `description`, ...).
// className, ids, data attributes, keys, CSS values, events, sound names and the like are fine.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const ROOT = join(process.cwd(), "src");

/** Folders that are not Clade-owned UI copy. */
const SKIP_DIRS = ["locales", join("components", "ui"), "test"];

const USER_FACING_PROPS = new Set([
  "aria-label",
  "aria-description",
  "aria-placeholder",
  "aria-roledescription",
  "placeholder",
  "title",
  "alt",
  "label",
  "description",
  "tooltip",
]);
const USER_FACING_KEYS = new Set([
  "label",
  "title",
  "description",
  "message",
  "placeholder",
  "text",
]);

/** Text that is a brand or a technical token, never copy to translate. */
const ALLOWED_TEXT = new Set(["Clade", "Clade Launcher"]);

const hasWords = (s: string) => /[A-Za-zА-Яа-яЁё]{2,}/.test(s);

function files(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (!SKIP_DIRS.some((d) => relative(ROOT, p) === d)) files(p, out);
    } else if (
      /\.(ts|tsx)$/.test(name) &&
      !/\.test\.(ts|tsx)$/.test(name) &&
      !name.endsWith(".d.ts")
    )
      out.push(p);
  }
  return out;
}

export function findHardcodedText(source: string, fileName = "x.tsx"): string[] {
  const sf = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const hits: string[] = [];
  const at = (n: ts.Node, what: string) => {
    const { line } = sf.getLineAndCharacterOfPosition(n.getStart(sf));
    hits.push(`${fileName}:${line + 1} ${what}`);
  };
  const literalText = (n: ts.Node): string | null =>
    ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)
      ? n.text
      : ts.isTemplateExpression(n)
        ? n.head.text + n.templateSpans.map((s) => s.literal.text).join("")
        : null;

  const visit = (n: ts.Node) => {
    if (ts.isJsxText(n)) {
      const text = n.text.trim();
      if (hasWords(text) && !ALLOWED_TEXT.has(text)) at(n, `JSX text "${text}"`);
    }
    if (ts.isJsxExpression(n) && (ts.isJsxElement(n.parent) || ts.isJsxFragment(n.parent))) {
      const check = (x: ts.Node) => {
        const text = literalText(x);
        if (text !== null && hasWords(text) && !ALLOWED_TEXT.has(text))
          at(x, `string in JSX "${text}"`);
        // Only look through the expression's own branches, not into calls like t("key").
        if (ts.isConditionalExpression(x)) {
          check(x.whenTrue);
          check(x.whenFalse);
        } else if (
          ts.isBinaryExpression(x) &&
          [
            ts.SyntaxKind.AmpersandAmpersandToken,
            ts.SyntaxKind.BarBarToken,
            ts.SyntaxKind.QuestionQuestionToken,
          ].includes(x.operatorToken.kind)
        ) {
          // `a && <p/>` and `a ?? "text"` show their operands; `a === "ready"` only compares.
          check(x.left);
          check(x.right);
        } else if (ts.isParenthesizedExpression(x)) check(x.expression);
      };
      if (n.expression) check(n.expression);
    }
    if (ts.isJsxAttribute(n) && USER_FACING_PROPS.has(n.name.getText(sf)) && n.initializer) {
      const init = ts.isJsxExpression(n.initializer) ? n.initializer.expression : n.initializer;
      const text = init ? literalText(init) : null;
      if (text !== null && hasWords(text) && !ALLOWED_TEXT.has(text))
        at(n, `${n.name.getText(sf)}="${text}"`);
    }
    if (
      ts.isPropertyAssignment(n) &&
      USER_FACING_KEYS.has(n.name.getText(sf).replace(/["']/g, ""))
    ) {
      const text = literalText(n.initializer);
      if (text !== null && hasWords(text) && !ALLOWED_TEXT.has(text))
        at(n, `${n.name.getText(sf)}: "${text}"`);
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
  return hits;
}

describe("no user-facing text outside the localization layer", () => {
  const all = files(ROOT);

  it("scans the real UI source", () => {
    expect(all.length).toBeGreaterThan(30);
    expect(all.some((f) => f.endsWith(`launcher${sep}PrepareDialog.tsx`))).toBe(true);
  });

  it("finds no hardcoded copy in components, features or lib", () => {
    const hits = all.flatMap((f) => findHardcodedText(readFileSync(f, "utf8"), relative(ROOT, f)));
    expect(
      hits,
      "Add the text to src/locales/en.ts and ru.ts and read it with t(...) (see CLAUDE.md, Localization)",
    ).toEqual([]);
  });

  it("detects the constructs it exists to catch (the guard itself works)", () => {
    const bad = [
      "const A = () => <p>Save your world</p>;",
      'const A = () => <input placeholder="Enter a name" />;',
      'const A = () => <button aria-label="Close dialog" />;',
      'const A = () => <p>{ok ? "All good" : t("x")}</p>;',
      'const items = [{ value: "a", label: "Release notes" }];',
      "const A = () => <p>Привет, мир</p>;",
    ];
    for (const src of bad) expect(findHardcodedText(src).length, src).toBeGreaterThan(0);
  });

  it("leaves technical identifiers, brand names and translated text alone", () => {
    const fine = [
      'const A = () => <div className="flex items-center gap-2" id="profile-name" data-state="open" />;',
      'const A = () => <button aria-label={t("window.close")}>{t("common.close")}</button>;',
      "const A = () => <span>Clade</span>;",
      'const A = () => <p>{" "}</p>;',
      "const A = () => <span aria-hidden>+</span>;",
      'playSound("click");',
      'const items = [{ value: "classic", label: t("accountModal.skinClassic") }];',
      "const A = () => <p>{count} / {total}</p>;",
    ];
    for (const src of fine) expect(findHardcodedText(src), src).toEqual([]);
  });
});
