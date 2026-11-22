// Release version consistency. One version number must be stated identically in package.json,
// src-tauri/Cargo.toml, src-tauri/tauri.conf.json and (the Clade entry of) src-tauri/Cargo.lock.
// For a release tag the tag must agree with it, and CHANGELOG.md must have an entry for it.
//
//   node scripts/release/check-versions.mjs                       # the four files agree
//   node scripts/release/check-versions.mjs --tag v0.1.0-preview  # ...and the tag, and the changelog
//   node scripts/release/check-versions.mjs --notes out.md --tag v0.1.0-preview   # also write the notes
//
// File versions are plain `X.Y.Z` (the Windows executable's version resource is numeric). A release tag
// is `vX.Y.Z`, optionally followed by `-preview` / `-preview.N` / `-rc.N`. The core must equal the file
// version; the suffix only says what kind of release it is.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export const VERSION_RE = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
export const TAG_RE =
  /^v((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))(?:-(preview|rc)(?:\.(0|[1-9]\d*))?)?$/;

/** `{ version, kind }` for a valid release tag, or null. `kind` is "preview", "rc" or "stable". */
export function parseTag(tag) {
  const m = TAG_RE.exec(tag ?? "");
  return m ? { version: m[1], kind: m[2] ?? "stable" } : null;
}

/** A tag marks a pre-release when it has a suffix, or while the product is still 0.x. */
export function isPrerelease(tag) {
  const t = parseTag(tag);
  return !t || t.kind !== "stable" || t.version.startsWith("0.");
}

const read = (root, rel) => readFileSync(join(root, rel), "utf8");

function cargoTomlVersion(text) {
  let inPackage = false;
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed.startsWith("[")) inPackage = trimmed === "[package]";
    else if (inPackage) {
      const m = /^version\s*=\s*"([^"]+)"/.exec(trimmed);
      if (m) return m[1];
    }
  }
  return null;
}

function cargoLockVersion(text, name) {
  const re = new RegExp(`\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = "([^"]+)"`);
  const m = re.exec(text);
  return m ? m[1] : null;
}

/** The version sources that exist in `root`. Missing files are reported by `checkVersions`. */
export function readVersions(root) {
  const get = (rel, parse) => {
    if (!existsSync(join(root, rel))) return { file: rel, version: null, missing: true };
    try {
      return { file: rel, version: parse(read(root, rel)) };
    } catch {
      return { file: rel, version: null };
    }
  };
  return [
    get("package.json", (t) => JSON.parse(t).version ?? null),
    get("src-tauri/Cargo.toml", cargoTomlVersion),
    get("src-tauri/tauri.conf.json", (t) => JSON.parse(t).version ?? null),
    get("src-tauri/Cargo.lock", (t) => cargoLockVersion(t, "clade-launcher")),
  ];
}

/** The text of the `## [version]` section of the changelog, or null when there is none. */
export function changelogSection(changelog, version) {
  const lines = changelog.split(/\r?\n/);
  const head = new RegExp(`^## \\[${version.replace(/\./g, "\\.")}\\](\\s|$)`);
  const start = lines.findIndex((l) => head.test(l));
  if (start < 0) return null;
  let end = lines.findIndex((l, i) => i > start && /^## \[/.test(l));
  if (end < 0) end = lines.length;
  return lines
    .slice(start + 1, end)
    .join("\n")
    .trim();
}

/**
 * Checks everything and returns the problems found (an empty list means consistent).
 * `tag` is the release tag when running for one; its presence also requires a changelog entry.
 */
export function checkVersions({ root, tag = null, requireChangelog = false }) {
  const problems = [];
  const sources = readVersions(root);
  for (const s of sources) {
    if (s.missing) problems.push(`${s.file} is missing`);
    else if (!s.version) problems.push(`${s.file} has no readable version`);
    else if (!VERSION_RE.test(s.version))
      problems.push(`${s.file}: "${s.version}" is not a plain X.Y.Z version`);
  }
  const versions = new Set(sources.filter((s) => s.version).map((s) => s.version));
  if (versions.size > 1)
    problems.push(
      `version sources disagree: ${sources.map((s) => `${s.file}=${s.version ?? "?"}`).join(", ")}`,
    );
  const version = sources.find((s) => s.version)?.version ?? null;

  if (tag !== null) {
    const parsed = parseTag(tag);
    if (!parsed)
      problems.push(
        `"${tag}" is not a release tag: use vX.Y.Z, optionally followed by -preview, -preview.N or -rc.N`,
      );
    else if (version && versions.size === 1 && parsed.version !== version)
      problems.push(`the tag ${tag} is for ${parsed.version} but the files say ${version}`);
  }

  if ((tag !== null || requireChangelog) && version) {
    const path = join(root, "CHANGELOG.md");
    if (!existsSync(path)) problems.push("CHANGELOG.md is missing");
    else {
      const section = changelogSection(readFileSync(path, "utf8"), version);
      if (section === null) problems.push(`CHANGELOG.md has no "## [${version}]" entry`);
      else if (section.length === 0)
        problems.push(`the CHANGELOG.md entry for ${version} is empty`);
    }
  }
  return { problems, version };
}

function main(argv) {
  const arg = (name) => {
    const i = argv.indexOf(name);
    if (i < 0) return null;
    const val = argv[i + 1];
    if (!val || val.startsWith("--")) {
      console.error(`missing value for ${name}`);
      process.exit(2);
    }
    return val;
  };
  const root = resolve(arg("--root") ?? ".");
  const tag = arg("--tag");
  const notes = arg("--notes");
  const { problems, version } = checkVersions({
    root,
    tag,
    requireChangelog: argv.includes("--require-changelog"),
  });
  if (problems.length) {
    for (const p of problems) console.error(`version check failed: ${p}`);
    process.exit(1);
  }
  if (notes && version) {
    const outPath = resolve(notes);
    mkdirSync(dirname(outPath), { recursive: true });
    const text = changelogSection(readFileSync(join(root, "CHANGELOG.md"), "utf8"), version);
    writeFileSync(outPath, `${text ?? ""}\n`, "utf8");
  }
  console.log(
    `version ${version}${tag ? ` matches tag ${tag}` : ""}${tag ? (isPrerelease(tag) ? " (pre-release)" : " (stable)") : ""}`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
