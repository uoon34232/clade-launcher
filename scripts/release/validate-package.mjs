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
import { crc32 } from "node:zlib";
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

/** Problems with explicit directory entries inside the package (paths without the top folder, with or without trailing `/`). */
export function validateDirectoryNames(dirNames) {
  const problems = [];
  const seen = new Set();
  for (const raw of dirNames) {
    const rel = raw.endsWith("/") ? raw.slice(0, -1) : raw;
    if (rel === "") {
      if (seen.has("")) problems.push(`duplicate entry: ${PACKAGE_ROOT}/`);
      seen.add("");
      continue;
    }
    if (seen.has(rel.toLowerCase())) problems.push(`duplicate entry: ${rel}/`);
    seen.add(rel.toLowerCase());
    if (
      rel.startsWith("/") ||
      rel.includes("\\") ||
      /^[A-Za-z]:/.test(rel) ||
      rel.split("/").some((s) => s === ".." || s === "." || s === "")
    ) {
      problems.push(`unsafe entry name: ${JSON.stringify(`${rel}/`)}`);
      continue;
    }
    const segments = rel.split("/");
    const top = segments[0];
    const inRuntime = rel === "runtimes/webview2" || rel.startsWith("runtimes/webview2/");
    for (const dir of segments) {
      if (FORBIDDEN_DIRS.has(dir.toLowerCase()) && !(inRuntime && dir.toLowerCase() === "src"))
        problems.push(`forbidden folder in the package: ${rel}/`);
      for (const re of FORBIDDEN_NAMES) {
        if (re.test(dir)) {
          problems.push(`forbidden folder in the package: ${rel}/`);
          break;
        }
      }
    }
    if (USER_DATA_DIRS.has(top.toLowerCase()))
      problems.push(`user data folder in the package: ${rel}/`);
    const allowed = rel === "licenses" || rel === "runtimes" || inRuntime;
    if (!allowed) problems.push(`not on the allowlist: ${rel}/`);
  }
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

/**
 * Looks for each developer-specific string, case-insensitively in UTF-8/ASCII and UTF-16LE
 * (at both even and odd byte alignments), in the executable's bytes.
 */
export function scanExecutable(buf, forbidStrings) {
  const filtered = forbidStrings.filter(Boolean);
  if (filtered.length === 0) return [];
  const utf8Lower = buf.toString("utf8").toLowerCase();
  const latin1Lower = buf.toString("latin1").toLowerCase();
  const utf16EvenLower = buf.toString("utf16le").toLowerCase();
  const utf16OddLower = buf.length > 1 ? buf.subarray(1).toString("utf16le").toLowerCase() : "";
  const problems = [];
  for (const s of filtered) {
    const slashVariants = [...new Set([s, s.replace(/\\/g, "/"), s.replace(/\//g, "\\")])];
    const found = slashVariants.some((v) => {
      const lower = v.toLowerCase();
      return (
        buf.includes(Buffer.from(v, "utf8")) ||
        buf.includes(Buffer.from(v, "utf16le")) ||
        utf8Lower.includes(lower) ||
        latin1Lower.includes(lower) ||
        utf16EvenLower.includes(lower) ||
        utf16OddLower.includes(lower)
      );
    });
    if (found) problems.push(`Clade.exe contains the developer-specific text "${s}"`);
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

function validateZipStructure(buf, entries, cdOffset) {
  const problems = [];
  const contents = new Map();
  const spans = [];
  for (const e of entries) {
    const unixType = ((e.externalAttr ?? 0) >>> 16) & 0o170000;
    const isDirAttr = ((e.externalAttr ?? 0) & 0x10) !== 0 || unixType === 0o040000;
    if (unixType === 0o120000 || ((e.externalAttr ?? 0) & 0x0400) !== 0) {
      problems.push(`symlink or reparse point in the package: ${e.name}`);
    } else if (unixType !== 0 && unixType !== 0o100000 && unixType !== 0o040000) {
      problems.push(`special file entry in the package: ${e.name}`);
    }
    if (isDirAttr && !e.name.endsWith("/")) {
      problems.push(`malformed directory entry (missing trailing "/"): ${e.name}`);
    }
    if (e.name.endsWith("/") && (e.size !== 0 || e.compressedSize !== 0)) {
      problems.push(`directory entry has non-empty payload: ${e.name}`);
    }
    const flags = e.flags ?? 0;
    if ((flags & 0x0001) !== 0) problems.push(`encrypted ZIP entry is not allowed: ${e.name}`);
    if ((flags & 0x0008) !== 0) problems.push(`ZIP data descriptor is not allowed: ${e.name}`);

    const p = e.offset;
    if (p + 30 > cdOffset || p + 30 > buf.length || buf.readUInt32LE(p) !== 0x04034b50) {
      problems.push(`local/central header mismatch for ${e.name}: invalid local header`);
      continue;
    }
    const localFlags = buf.readUInt16LE(p + 6);
    const localMethod = buf.readUInt16LE(p + 8);
    const localCrc = buf.readUInt32LE(p + 14);
    const localCompressedSize = buf.readUInt32LE(p + 18);
    const localSize = buf.readUInt32LE(p + 22);
    const localNameLen = buf.readUInt16LE(p + 26);
    const localExtraLen = buf.readUInt16LE(p + 28);
    const nameEnd = p + 30 + localNameLen;
    const dataStart = nameEnd + localExtraLen;
    const dataEnd = dataStart + e.compressedSize;
    if (nameEnd > cdOffset || dataStart > cdOffset || dataEnd > cdOffset || dataEnd > buf.length) {
      problems.push(
        `local/central header mismatch for ${e.name}: entry extends past central directory`,
      );
      continue;
    }
    spans.push({ name: e.name, start: p, end: dataEnd });

    if ((localFlags & 0x0001) !== 0)
      problems.push(`local/central header mismatch for ${e.name}: encrypted local header`);
    if ((localFlags & 0x0008) !== 0)
      problems.push(
        `local/central header mismatch for ${e.name}: data descriptor flag in local header`,
      );
    if ((localFlags & 0x0800) !== (flags & 0x0800))
      problems.push(`local/central header mismatch for ${e.name}: UTF-8 flag mismatch`);

    const localName = buf.toString(localFlags & 0x0800 ? "utf8" : "latin1", p + 30, nameEnd);
    if (localName !== e.name)
      problems.push(
        `local/central header mismatch for ${e.name}: local header names "${localName}"`,
      );
    if (localMethod !== e.method)
      problems.push(
        `local/central header mismatch for ${e.name}: compression method ${localMethod} != ${e.method}`,
      );
    if (e.crc !== undefined && localCrc !== e.crc)
      problems.push(`local/central header mismatch for ${e.name}: CRC-32 mismatch`);
    if (localCompressedSize !== e.compressedSize)
      problems.push(
        `local/central header mismatch for ${e.name}: compressed size ${localCompressedSize} != ${e.compressedSize}`,
      );
    if (localSize !== e.size)
      problems.push(
        `local/central header mismatch for ${e.name}: uncompressed size ${localSize} != ${e.size}`,
      );

    try {
      const content = readZipEntry(buf, e);
      if (content.length !== e.size) {
        problems.push(
          `local/central header mismatch for ${e.name}: inflated size ${content.length} != ${e.size}`,
        );
      } else if (e.crc !== undefined && crc32(content) >>> 0 !== e.crc) {
        problems.push(`corrupt ZIP entry payload (CRC-32 mismatch): ${e.name}`);
      } else {
        contents.set(e, content);
      }
    } catch (err) {
      problems.push(`corrupt ZIP entry ${e.name}: ${err.message}`);
    }
  }

  const ordered = [...spans].sort((a, b) => a.start - b.start || a.end - b.end);
  for (let i = 1; i < ordered.length; i++) {
    if (ordered[i].start < ordered[i - 1].end) {
      problems.push(`overlapping ZIP entries: ${ordered[i - 1].name} and ${ordered[i].name}`);
    }
  }
  return { problems, contents };
}

/** Validates a ZIP file on disk. Returns `{ problems, entries }`. */
export function validateZipFile(
  zipPath,
  { version = null, forbidStrings = [], allowMissingWebview2 = false, csp = null } = {},
) {
  let zip;
  try {
    zip = readZipFile(zipPath);
  } catch (e) {
    return { problems: [e.message], entries: [] };
  }
  const { buf, entries, cdOffset } = zip;
  const structure = validateZipStructure(buf, entries, cdOffset);
  const problems = [...structure.problems];
  const prefix = `${PACKAGE_ROOT}/`;
  const names = [];
  const dirNames = [];
  const byName = new Map();
  for (const e of entries) {
    if (!e.name.startsWith(prefix)) {
      problems.push(`outside the ${PACKAGE_ROOT}/ folder: ${e.name}`);
      continue;
    }
    if (e.name.endsWith("/")) {
      dirNames.push(e.name.slice(prefix.length));
      continue;
    }
    const rel = e.name.slice(prefix.length);
    names.push(rel);
    byName.set(rel, e);
  }
  problems.push(...validateDirectoryNames(dirNames));
  problems.push(...validateNames(names, { allowMissingWebview2 }));
  for (const name of names) {
    if (!TEXT_SCANNED(name)) continue;
    const entry = byName.get(name);
    const bytes = entry ? structure.contents.get(entry) : null;
    if (bytes) problems.push(...scanText(name, bytes.toString("utf8")));
  }
  const exe = byName.get("Clade.exe");
  const exeBytes = exe ? structure.contents.get(exe) : null;
  if (exeBytes) {
    if (forbidStrings.length) problems.push(...scanExecutable(exeBytes, forbidStrings));
    if (csp && !embeddedCsp(exeBytes, csp))
      problems.push(
        "Clade.exe does not contain the release Content Security Policy from tauri.conf.json",
      );
    if (version) {
      const found = executableVersion(exeBytes);
      if (found === null) problems.push("Clade.exe has no readable version resource");
      else if (found !== version)
        problems.push(`Clade.exe says version ${found}, expected ${version}`);
    }
  }
  return { problems: [...new Set(problems)], entries: names };
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
