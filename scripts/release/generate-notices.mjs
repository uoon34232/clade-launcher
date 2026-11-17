// THIRD-PARTY-NOTICES.txt for the portable package, generated from what is actually distributed:
//
//  - Rust: every crate that is linked into Clade.exe on Windows. The set comes from `cargo metadata`
//    (Cargo.lock, the Windows target, normal dependencies only: build scripts and test-only crates are
//    not part of the binary and are left out). Licence files are read from each crate's own source.
//  - Frontend: every npm package whose code or styles end up in the bundled interface, found by reading
//    what the shipped source imports (not the build tooling) and following those packages'
//    dependencies in the installed tree. Licence files are read from each installed package. This
//    includes the two font families (Fraunces, Instrument Sans), whose licence files carry their copyright.
//  - The bundled WebView2 Fixed Version Runtime, which has its own licence (shipped as a file).
//
// What cannot be verified is said so in the output: a package that ships no licence file is listed by
// name, version and declared licence, never silently covered. Output is sorted and has no timestamps, so
// the same inputs give the same file.
//
//   node scripts/release/generate-notices.mjs --out <file>

import { execFileSync } from "node:child_process";
import {
  existsSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { sha256Hex } from "./common.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
export const LICENSE_SOURCES_DIR = join(HERE, "license-texts");

/**
 * Licence texts that a package does not ship but that were taken from an authoritative upstream source
 * (scripts/release/license-sources.json): the upstream repository file at the exact commit recorded in the
 * published crate, or the vendor's own package. Each text is checked against its recorded SHA-256 every
 * time, and an entry applies only to the exact crate version it was verified for, so a new version of a
 * dependency without a licence file shows up as uncovered again instead of silently inheriting a text.
 */
export function loadLicenseSources(
  manifestPath = join(HERE, "license-sources.json"),
  textsDir = LICENSE_SOURCES_DIR,
) {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  return manifest.sources.map((src) => {
    // The hash is of the file with line endings normalised to LF (a Windows checkout may convert them).
    const normalised = readFileSync(join(textsDir, src.file), "utf8").replaceAll("\r\n", "\n");
    const actual = sha256Hex(normalised);
    if (actual !== src.sha256)
      throw new Error(`licence text ${src.file} does not match its recorded SHA-256 (${actual})`);
    const text = normalised.trim();
    return { ...src, text };
  });
}

const LICENSE_FILE_RE = /^(licen[sc]e|copying|notice|unlicense|copyright)([-_.].*)?$/i;
const MAX_LICENSE_BYTES = 200 * 1024;

const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** Licence-like files at the top of a package directory: `[{ file, text }]`. */
export function readLicenseFiles(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => LICENSE_FILE_RE.test(f) && statSync(join(dir, f)).isFile())
    .filter((f) => !/\.(js|mjs|cjs|ts|json|rs|toml|png|svg|woff2?)$/i.test(f))
    .sort(cmp)
    .map((file) => ({
      file,
      text: readFileSync(join(dir, file), "utf8").replace(/\r\n/g, "\n").trim(),
    }))
    .filter((x) => x.text.length > 0 && Buffer.byteLength(x.text) <= MAX_LICENSE_BYTES);
}

// ------------------------------------------------------------------------------------- Rust

/**
 * From `cargo metadata` output: the packages that are linked into the binary, i.e. reachable from the
 * root through normal (non-build, non-dev) dependency edges. Returns `[{ name, version, license,
 * repository, dir }]`, sorted.
 */
export function rustRuntimePackages(metadata) {
  const byId = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const seen = new Set();
  const stack = [metadata.resolve.root];
  while (stack.length) {
    const id = stack.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      if ((dep.dep_kinds ?? []).some((k) => k.kind === null)) stack.push(dep.pkg);
    }
  }
  seen.delete(metadata.resolve.root); // the project itself is covered by LICENSE
  return [...seen]
    .map((id) => byId.get(id))
    .filter(Boolean)
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: p.license ?? (p.license_file ? `see ${basename(p.license_file)}` : "UNKNOWN"),
      repository: p.repository ?? p.homepage ?? "",
      dir: dirname(p.manifest_path),
    }))
    .sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version));
}

// ------------------------------------------------------------------------------------- npm

const TYPE_ONLY_IMPORT_RE =
  /(?:import|export)\s+type(?:\s+\*(?:\s+as\s+[A-Za-z0-9_$]+)?|\s+[A-Za-z0-9_$]+(?:\s*,\s*\{[^}]*\})?|\s*\{[^}]*\})\s*from\s*["'][^"']+["']\s*;?/g;

const stripTypeOnlyImports = (text) =>
  text
    .replace(TYPE_ONLY_IMPORT_RE, "")
    .replace(/import\s*\{([^}]*)\}\s*from\s*["'][^"']+["']\s*;?/g, (full, clause) => {
      const items = clause
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      return items.length > 0 && items.every((s) => /^type\s+/.test(s)) ? "" : full;
    });

const sourceImports = (text, { isCss = false } = {}) => {
  const cleaned = isCss ? text : stripTypeOnlyImports(text);
  const found = new Set();
  const specifier = /(?:from\s+|import\s*\(\s*|import\s+|@import\s+(?:url\()?\s*)["']([^"']+)["']/g;
  for (const m of cleaned.matchAll(specifier)) found.add(m[1]);
  return found;
};

/** `react-dom/client` -> `react-dom`, `@scope/pkg/x` -> `@scope/pkg`; relative, alias and URL imports -> null. */
export function packageNameOf(spec) {
  if (
    spec.startsWith(".") ||
    spec.startsWith("/") ||
    spec.startsWith("@/") ||
    spec.startsWith("node:") ||
    /^[a-z]+:/i.test(spec)
  )
    return null;
  const parts = spec.split("/");
  return spec.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
}

function* walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) yield* walk(p);
    else yield p;
  }
}

const TEST_DIR_NAMES = new Set(["test", "tests", "__tests__", "__mocks__"]);
const TEST_FILE_RE = /^test[A-Z_.-]|\.(test|spec)\.[a-z]+$/;
const DEV_TOOL_PACKAGE_RE =
  /^(?:vitest|@vitest\/|@testing-library\/|@playwright\/|playwright(?:-core)?$|jsdom$|happy-dom$|@biomejs\/|eslint|typescript$|@tauri-apps\/cli$)/;

/** The npm packages the shipped interface source imports, split into script and style imports. */
export function frontendRoots(srcDir) {
  const scripts = new Set();
  const styles = new Set();
  for (const file of walk(srcDir)) {
    const relParts = relative(srcDir, file).split(sep);
    if (relParts.slice(0, -1).some((part) => TEST_DIR_NAMES.has(part))) continue;
    if (TEST_FILE_RE.test(basename(file))) continue;
    if (!/\.(ts|tsx|js|jsx|mjs|css)$/.test(file)) continue;
    const isCss = file.endsWith(".css");
    const text = readFileSync(file, "utf8");
    for (const spec of sourceImports(text, { isCss })) {
      const name = packageNameOf(spec);
      if (name) (isCss ? styles : scripts).add(name);
    }
  }
  return { scripts: [...scripts].sort(cmp), styles: [...styles].sort(cmp) };
}

/**
 * Packages in `frontendRoots(srcDir)` that belong to test/dev tooling rather than the shipped app:
 * any pure `devDependencies` entry imported by runtime scripts (other than `@fontsource*`), or any
 * known test/lint/build runner in either scripts or styles.
 */
export function forbiddenFrontendRoots(roots, pkgJson = {}) {
  const deps = new Set(Object.keys(pkgJson.dependencies ?? {}));
  const devDeps = new Set(Object.keys(pkgJson.devDependencies ?? {}));
  const bad = new Set();
  for (const name of roots.scripts ?? []) {
    if (
      DEV_TOOL_PACKAGE_RE.test(name) ||
      (devDeps.has(name) && !deps.has(name) && !name.startsWith("@fontsource"))
    ) {
      bad.add(name);
    }
  }
  for (const name of roots.styles ?? []) {
    if (DEV_TOOL_PACKAGE_RE.test(name)) bad.add(name);
  }
  return [...bad].sort(cmp);
}

/** Finds an installed package directory as Node would, starting from `fromDir` (pnpm and npm layouts). */
export function resolvePackageDir(name, fromDir) {
  let dir = fromDir;
  for (;;) {
    for (const cand of [
      join(dir, "node_modules", name),
      basename(dir) === "node_modules" ? join(dir, name) : null,
    ]) {
      if (cand && existsSync(join(cand, "package.json"))) return realpathSync(cand);
    }
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

const readPackage = (dir) => JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));

/** `[{ name, version, license, repository, dir }]` for the roots and (for script roots) their dependencies. */
export function npmRuntimePackages(projectDir, { scripts, styles }) {
  const out = new Map();
  const missing = [];
  const add = (name, fromDir, follow) => {
    const dir = resolvePackageDir(name, fromDir);
    if (!dir) {
      missing.push(name);
      return;
    }
    const pkg = readPackage(dir);
    const key = `${pkg.name}@${pkg.version}`;
    if (out.has(key)) return;
    out.set(key, {
      name: pkg.name,
      version: pkg.version,
      license:
        typeof pkg.license === "string" ? pkg.license : JSON.stringify(pkg.license ?? "UNKNOWN"),
      repository:
        (typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url) ??
        pkg.homepage ??
        "",
      dir,
    });
    if (follow) for (const dep of Object.keys(pkg.dependencies ?? {})) add(dep, dir, true);
  };
  for (const n of scripts) add(n, projectDir, true);
  for (const n of styles) add(n, projectDir, false); // CSS only: the package itself, not its tooling
  return {
    packages: [...out.values()].sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version)),
    missing,
  };
}

// ------------------------------------------------------------------------------------- output

/** Groups packages by identical licence text so each text is printed once, with every package it covers. */
export function groupByLicenseText(packages, sources = []) {
  const groups = new Map();
  const uncovered = [];
  const add = (text, p, origin) => {
    const key = sha256Hex(text);
    if (!groups.has(key)) groups.set(key, { text, packages: [], origin });
    groups.get(key).packages.push(p);
  };
  for (const p of packages) {
    // A component that is not a package (for example a statically linked vendor library) carries its source.
    if (p.source) {
      add(p.source.text, p, p.source.origin);
      continue;
    }
    const files = readLicenseFiles(p.dir);
    if (files.length === 0) {
      // The package ships no file: use a verified upstream text for exactly this crate version, if any.
      const known = sources.find((src) =>
        (src.appliesTo ?? []).some((a) => a.crate === p.name && a.version === p.version),
      );
      if (known) add(known.text, p, known.origin);
      else uncovered.push(p);
      continue;
    }
    // One group per distinct licence text (a package with several files joins one group per text).
    for (const f of files) add(f.text, p);
  }
  // Some licences have one fixed text with nothing specific to a package (no copyright holder line), so
  // a package that declares exactly that licence but ships no file is covered by the same text taken
  // from another package. MIT and BSD texts name their copyright holder, so they are never borrowed.
  const stillUncovered = [];
  for (const p of uncovered) {
    // "A OR B" lets the recipient choose, so any one option with a fixed text is enough.
    const options = p.license.split(/\s+OR\s+|\s*\/\s*/);
    const title = options.map((o) => FIXED_TEXT_LICENSES[o]).find(Boolean);
    const group = title && [...groups.values()].find((g) => g.text.includes(title));
    if (group) group.packages.push(p);
    else stillUncovered.push(p);
  }
  return {
    groups: [...groups.values()].sort((a, b) => cmp(a.packages[0].name, b.packages[0].name)),
    uncovered: stillUncovered,
  };
}

const FIXED_TEXT_LICENSES = {
  "MPL-2.0": "Mozilla Public License Version 2.0",
  "Apache-2.0": "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
};

const line = (c) => c.repeat(78);

function section(title, rust, npm, sources, components) {
  const out = [line("="), title, line("=")];
  const { groups, uncovered } = groupByLicenseText([...rust, ...npm, ...components], sources);
  out.push("");
  for (const g of groups) {
    const names = [...new Map(g.packages.map((p) => [`${p.name}@${p.version}`, p])).values()];
    out.push(line("-"));
    out.push("Licence text that applies to:");
    for (const p of names)
      out.push(
        `  - ${p.name} ${p.version} (${p.license})${p.repository ? ` ${p.repository}` : ""}`,
      );
    out.push(line("-"));
    if (g.origin) {
      out.push(`(Licence text taken from: ${g.origin})`);
      out.push("");
    }
    out.push(g.text);
    out.push("");
  }
  if (uncovered.length) {
    out.push(line("-"));
    out.push(
      "Packages that ship no licence file in their distribution. Their declared licence is listed;",
    );
    out.push(
      "the licence text itself is not included here because it could not be taken from the package:",
    );
    out.push(line("-"));
    for (const p of uncovered)
      out.push(
        `  - ${p.name} ${p.version}: ${p.license}${p.repository ? ` (${p.repository})` : ""}`,
      );
    out.push("");
  }
  return out;
}

/** Assembles the whole notices file. Pure: everything it needs is passed in. */
export function renderNotices({
  rust,
  npm,
  rustMissing = [],
  npmMissing = [],
  projectVersion,
  fontPackages,
  sources = [],
  components = [],
}) {
  const out = [];
  out.push("Clade Launcher: third-party notices");
  out.push("");
  out.push(
    `This file accompanies Clade Launcher ${projectVersion}. Clade Launcher itself is licensed under the`,
  );
  out.push(
    "Apache License 2.0 (see the LICENSE file). It includes and was built with the third-party software",
  );
  out.push(
    "listed below, each under its own licence. The lists are generated from the exact dependency",
  );
  out.push(
    "versions recorded in the project's lock files (Cargo.lock and pnpm-lock.yaml); only components",
  );
  out.push("that are part of the distributed program are listed.");
  out.push("");
  out.push(`Rust crates linked into Clade.exe: ${rust.length}`);
  out.push(`JavaScript packages in the bundled interface: ${npm.length}`);
  out.push(`Fonts: ${fontPackages.map((p) => `${p.name} ${p.version}`).join(", ") || "none"}`);
  out.push("");
  if (rustMissing.length || npmMissing.length) {
    out.push("INCOMPLETE: these dependencies could not be located and are NOT covered:");
    for (const m of [...rustMissing, ...npmMissing]) out.push(`  - ${m}`);
    out.push("");
  }
  out.push(line("="));
  out.push("Components that are not covered by a generated list");
  out.push(line("="));
  out.push("");
  out.push(
    "- Microsoft Edge WebView2 Fixed Version Runtime (runtimes/webview2/). Redistributed under Microsoft's",
  );
  out.push(
    "  WebView2 Fixed Version licence terms, shipped as licenses/WebView2-Fixed-Version-License.txt. The",
  );
  out.push(
    "  runtime is Microsoft's software and may itself contain components from other parties under their",
  );
  out.push("  own licences; those are Microsoft's to list and are not repeated here.");
  out.push(
    "- The Rust standard library, statically linked (MIT OR Apache-2.0), and the Microsoft Visual C++",
  );
  out.push(
    "  runtime, statically linked under Microsoft's redistribution terms for the C runtime.",
  );
  out.push(
    "- Minecraft, its libraries and assets, Java runtimes and mod loaders are NOT part of this package.",
  );
  out.push(
    "  Clade downloads them from their official sources at the person's request; they remain under their",
  );
  out.push("  owners' terms.");
  out.push("");
  out.push(
    ...section(
      "Rust crates, JavaScript packages and linked components (including the Fraunces and Instrument Sans fonts)",
      rust,
      npm,
      sources,
      components,
    ),
  );
  return `${out.join("\n").trimEnd()}\n`;
}

// ------------------------------------------------------------------------------------- main

function readCargoMetadata(tauriDir) {
  const raw = execFileSync(
    "cargo",
    [
      "metadata",
      "--format-version",
      "1",
      "--locked",
      "--filter-platform",
      "x86_64-pc-windows-msvc",
    ],
    {
      cwd: tauriDir,
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
      stdio: ["ignore", "pipe", "inherit"],
    },
  );
  return JSON.parse(raw);
}

const BUNDLED_HELPERS = ["vite"];

export function generate(root, { cargoMetadata = null, licenseSources = null } = {}) {
  const pkg = readPackage(root);
  const roots = frontendRoots(join(root, "src"));
  const forbidden = forbiddenFrontendRoots(roots, pkg);
  if (forbidden.length) {
    throw new Error(
      `refusing to include dev/test tooling in THIRD-PARTY-NOTICES.txt (imported in shipped src/): ${forbidden.join(", ")}`,
    );
  }
  const metadata = cargoMetadata ?? readCargoMetadata(join(root, "src-tauri"));
  const rust = rustRuntimePackages(metadata);
  // Small pieces of build tooling that are copied into the interface bundle (Vite's preload helper).
  const { packages: npm, missing } = npmRuntimePackages(root, {
    scripts: roots.scripts,
    styles: [...roots.styles, ...BUNDLED_HELPERS],
  });
  const fontPackages = npm.filter((p) => p.name.startsWith("@fontsource"));
  const sources = licenseSources ?? loadLicenseSources();
  // Components that are linked into Clade.exe without being a Rust crate or an npm package of their own.
  const components = sources
    .filter((src) => src.component)
    .map((src) => ({
      name: src.component,
      version: "",
      license: src.license,
      repository: src.homepage ?? "",
      source: src,
    }));
  return {
    text: renderNotices({
      rust,
      npm,
      npmMissing: missing,
      projectVersion: pkg.version,
      fontPackages,
      sources,
      components,
    }),
    counts: {
      rust: rust.length,
      npm: npm.length,
      components: components.length,
      uncovered: groupByLicenseText([...rust, ...npm, ...components], sources).uncovered.length,
    },
    uncoveredNames: groupByLicenseText([...rust, ...npm, ...components], sources).uncovered.map(
      (p) => `${p.name} ${p.version}`,
    ),
    missing,
  };
}

function main(argv) {
  const out = argv[argv.indexOf("--out") + 1];
  if (argv.indexOf("--out") < 0 || !out) {
    console.error("usage: generate-notices.mjs --out <file> [--strict]");
    process.exit(2);
  }
  const { text, counts, missing, uncoveredNames } = generate(resolve("."));
  writeFileSync(resolve(out), text, "utf8");
  console.log(
    `notices: ${counts.rust} Rust crates, ${counts.npm} JavaScript packages, ${counts.components} linked components, ${counts.uncovered} without a licence text`,
  );
  if (missing.length) {
    console.error(`notices incomplete: could not locate ${missing.join(", ")}`);
    process.exit(1);
  }
  if (argv.includes("--strict") && counts.uncovered > 0) {
    console.error(
      `notices: --strict and these redistributed packages have no licence text: ${uncoveredNames.join(", ")}.
` + "Add a verified upstream text to scripts/release/license-sources.json, or do not release.",
    );
    process.exit(1);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
