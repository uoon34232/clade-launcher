// Post-package validation of the portable ZIP. It fails closed: an entry is accepted only if it matches
// the explicit allowlist below, so anything unexpected (a stray log, a vault, a source folder) is an error.
//
//   node scripts/release/validate-package.mjs <zip> [--version X.Y.Z]
//        [--forbid-string <text>]...   # developer-specific strings that must not appear in Clade.exe
//        [--csp-from <tauri.conf.json>] # Clade.exe must embed that release CSP
//        [--allow-missing-webview2]    # development packages only
//
// What it checks: every entry name; that all required files exist; that the text files carry no developer
// paths or obvious secrets; that Clade.exe (read-only, never modified) contains none of the given
// developer-specific strings; that the executable's embedded version agrees when one is given.

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { PACKAGE_ROOT, readZipEntry, readZipFile } from "./common.mjs";

export const REQUIRED_FILES = [
  "Clade.exe",
  "README.txt",
  "LICENSE",
  "THIRD-PARTY-NOTICES.txt",
  "licenses/WebView2-Fixed-Version-License.txt",
];
export const WEBVIEW2_EXE = "runtimes/webview2/msedgewebview2.exe";
const TOP_LEVEL_FILES = new Set(["Clade.exe", "README.txt", "LICENSE", "THIRD-PARTY-NOTICES.txt"]);

// Names that are never acceptable, wherever they appear (compared case-insensitively on the file name).
const FORBIDDEN_NAMES = [
  /^vault\.bin(\.bak)?$/i,
  /^accounts\.json$/i,
  /^settings\.json$/i,
  /^playtime.*\.json$/i,
  /^clade\.lock$/i,
  /^claude\.md$/i,
  /^\.env(\..*)?$/i,
  /\.log$/i,
  /\.pem$/i,
  /\.pfx$/i,
  /\.p12$/i,
  /\.key$/i,
  /\.bak$/i,
  /\.corrupt(-.*)?$/i,
  /\.tmp$/i,
  /\.part$/i,
  /\.pdb$/i,
  /\.zip$/i,
  /^thumbs\.db$/i,
  /^\.ds_store$/i,
  /^diagnostics.*\.zip$/i,
];
// Directories that never belong anywhere in the package.
const FORBIDDEN_DIRS = new Set([
  ".git",
  ".github",
  ".vscode",
  ".idea",
  "node_modules",
  "target",
  "test-results",
  "playwright-report",
  "screenshots",
  "shots",
  "e2e",
  "src",
  "src-tauri",
  "dist",
]);
// Directories the launcher creates for the user. Forbidden at the top of the package.
const USER_DATA_DIRS = new Set([
  "data",
  "instances",
  "logs",
  "cache",
  "minecraft",
  "libraries",
  "assets",
  "worlds",
  "saves",
  "mods",
  "downloads",
]);

const TEXT_SCANNED = (name) =>
  name === "README.txt" ||
  name === "LICENSE" ||
  name === "THIRD-PARTY-NOTICES.txt" ||
  name.startsWith("licenses/");

// Obvious developer paths and obvious secrets. Deliberately conservative: a false alarm blocks a release,
// which is the safe direction.
const DEV_PATHS = [
  /[A-Za-z]:\\Users\\(?!Public\b|Default\b|All Users\b)[^\\\s"'<>|]+/i,
  /[A-Za-z]:\/Users\/(?!Public\b|Default\b)[^/\s"'<>|]+/i,
  /\/home\/[a-z_][a-z0-9_-]*\//i,
  /\/Users\/[A-Za-z][^/\s]*\//,
];
const SECRETS = [
  /eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/, // a JWT
  /\bBearer\s+[A-Za-z0-9._~+/-]{20,}/,
  /\bgh[pousr]_[A-Za-z0-9]{30,}/,
  /\bgithub_pat_[A-Za-z0-9_]{30,}/,
  /\bAKIA[0-9A-Z]{16}\b/,
  /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----/,
  /\b(?:access|refresh)_token\s*[=:]\s*["']?[A-Za-z0-9._-]{16,}/i,
  /\b(?:client_secret|api[_-]?key|password)\s*[=:]\s*["']?[^\s"']{8,}/i,
];

/** Problems with the entry names alone. `names` are paths inside the package (without the top folder). */
export function validateNames(names, { allowMissingWebview2 = false } = {}) {
  const problems = [];
  const seen = new Set();
  for (const name of names) {
    if (seen.has(name.toLowerCase())) problems.push(`duplicate entry: ${name}`);
    seen.add(name.toLowerCase());
    if (
      !name ||
      name.startsWith("/") ||
      name.includes("\\") ||
      /^[A-Za-z]:/.test(name) ||
      name.split("/").some((s) => s === ".." || s === "." || s === "")
    )
      problems.push(`unsafe entry name: ${JSON.stringify(name)}`);
    const segments = name.split("/");
    const file = segments[segments.length - 1];
    const top = segments[0];
    const inRuntime = name.startsWith("runtimes/webview2/");
    for (const dir of segments.slice(0, -1))
      if (FORBIDDEN_DIRS.has(dir.toLowerCase()) && !(inRuntime && dir.toLowerCase() === "src"))
        problems.push(`forbidden folder in the package: ${name}`);
    if (segments.length > 1 && USER_DATA_DIRS.has(top.toLowerCase()))
      problems.push(`user data folder in the package: ${name}`);
    for (const re of FORBIDDEN_NAMES)
      if (re.test(file)) {
        problems.push(`forbidden file in the package: ${name}`);
        break;
      }
    // the allowlist itself (fail closed)
    const allowed =
      TOP_LEVEL_FILES.has(name) ||
      (name.startsWith("licenses/") && segments.length === 2 && /\.(txt|md)$/i.test(file)) ||
      (inRuntime && segments.length >= 3);
    if (!allowed) problems.push(`not on the allowlist: ${name}`);
  }
  const have = new Set(names);
  for (const req of REQUIRED_FILES)
    if (!have.has(req)) problems.push(`required file is missing: ${req}`);
  if (!allowMissingWebview2 && !have.has(WEBVIEW2_EXE))
    problems.push(`the bundled WebView2 runtime is missing (${WEBVIEW2_EXE})`);
  return [...new Set(problems)];
}

/** Scans one text file's content for developer paths and secrets. */
export function scanText(name, text) {
  const problems = [];
  for (const re of DEV_PATHS)
    if (re.test(text)) problems.push(`${name}: contains a developer path (${re.exec(text)[0]})`);
  for (const re of SECRETS)
    if (re.test(text)) problems.push(`${name}: contains something that looks like a secret`);
  return problems;
}

/** Looks for each developer-specific string, as ASCII/UTF-8 and as UTF-16LE, in the executable's bytes. */
export function scanExecutable(buf, forbidStrings) {
  const problems = [];
  for (const s of forbidStrings.filter(Boolean)) {
    const variants = [
      Buffer.from(s, "utf8"),
      Buffer.from(s, "utf16le"),
      Buffer.from(s.toLowerCase(), "utf8"),
      Buffer.from(s.replace(/\\/g, "/"), "utf8"),
    ];
    if (variants.some((v) => buf.includes(v)))
      problems.push(`Clade.exe contains the developer-specific text "${s}"`);
  }
  return problems;
}

/** The product version recorded in the executable's version resource, or null. Read-only. */
export function executableVersion(buf) {
  // The resource strings are UTF-16LE: "ProductVersion\0", optional padding, then the value. The bytes
  // can also occur elsewhere in a large executable, so every occurrence is tried and only a value that
  // looks like a version is accepted.
  const key = Buffer.from("ProductVersion\0", "utf16le");
  for (let i = buf.indexOf(key); i >= 0; i = buf.indexOf(key, i + 1)) {
    let p = i + key.length;
    for (let pad = 0; pad < 2 && p + 1 < buf.length && buf.readUInt16LE(p) === 0; pad++) p += 2;
    let out = "";
    while (p + 1 < buf.length && out.length <= 40) {
      const c = buf.readUInt16LE(p);
      if (c === 0) break;
      out += String.fromCharCode(c);
      p += 2;
    }
    if (/^\d+\.\d+\.\d+(\.\d+)?$/.test(out)) return out;
  }
  return null;
}

/** The release CSP from tauri.conf.json is embedded in a correctly built executable. */
export function embeddedCsp(buf, csp) {
  return Boolean(csp) && buf.includes(Buffer.from(csp, "utf8"));
}

/** Validates a ZIP file on disk. Returns `{ problems, entries }`. */
export function validateZipFile(
  zipPath,
  { version = null, forbidStrings = [], allowMissingWebview2 = false, csp = null } = {},
) {
  const problems = [];
  const { buf, entries } = readZipFile(zipPath);
  const prefix = `${PACKAGE_ROOT}/`;
  const names = [];
  for (const e of entries) {
    if (!e.name.startsWith(prefix)) {
      problems.push(`outside the ${PACKAGE_ROOT}/ folder: ${e.name}`);
      continue;
    }
    if (e.name.endsWith("/")) continue; // an explicit folder entry carries no file
    names.push(e.name.slice(prefix.length));
  }
  problems.push(...validateNames(names, { allowMissingWebview2 }));
  const byName = new Map(entries.map((e) => [e.name.slice(prefix.length), e]));
  for (const name of names) {
    if (!TEXT_SCANNED(name)) continue;
    problems.push(...scanText(name, readZipEntry(buf, byName.get(name)).toString("utf8")));
  }
  const exe = byName.get("Clade.exe");
  if (exe) {
    const bytes = readZipEntry(buf, exe);
    if (forbidStrings.length) problems.push(...scanExecutable(bytes, forbidStrings));
    if (csp && !embeddedCsp(bytes, csp))
      problems.push(
        "Clade.exe does not contain the release Content Security Policy from tauri.conf.json",
      );
    if (version) {
      const found = executableVersion(bytes);
      if (found === null) problems.push("Clade.exe has no readable version resource");
      else if (found !== version)
        problems.push(`Clade.exe says version ${found}, expected ${version}`);
    }
  }
  return { problems, entries: names };
}

function main(argv) {
  const zip = argv[0];
  const values = (flag) => argv.flatMap((a, i) => (a === flag ? [argv[i + 1]] : []));
  if (!zip || !existsSync(resolve(zip))) {
    console.error(
      "usage: validate-package.mjs <zip> [--version X.Y.Z] [--forbid-string text]... [--allow-missing-webview2]",
    );
    process.exit(2);
  }
  const { problems, entries } = validateZipFile(resolve(zip), {
    version: values("--version")[0] ?? null,
    forbidStrings: values("--forbid-string"),
    allowMissingWebview2: argv.includes("--allow-missing-webview2"),
    csp: values("--csp-from")[0]
      ? (JSON.parse(readFileSync(resolve(values("--csp-from")[0]), "utf8")).app?.security?.csp ??
        null)
      : null,
  });
  if (problems.length) {
    for (const p of problems) console.error(`package validation failed: ${p}`);
    process.exit(1);
  }
  console.log(`package valid: ${entries.length} files, all on the allowlist`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
