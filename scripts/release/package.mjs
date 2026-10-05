// Assembles the portable package from explicitly named inputs and writes the ZIP and its SHA-256 file.
//
//   node scripts/release/package.mjs --exe <Clade.exe> --webview2 <runtime dir> --license <LICENSE>
//        --notices <THIRD-PARTY-NOTICES.txt> --webview2-license <file> --version X.Y.Z --out <dir>
//        [--no-webview2]    # development package: no runtime, named ...-NO-WEBVIEW2.zip
//
// The package is built from a clean staging folder that receives only those inputs; the whole repository
// is never copied. The staged file list is validated against the allowlist BEFORE the ZIP is written and
// the finished ZIP is validated again afterwards, so a bad input stops the build instead of shipping.

import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  PACKAGE_ROOT,
  ZIP_NAME,
  buildZip,
  listFiles,
  parseSha256File,
  sha256File,
  sha256Line,
} from "./common.mjs";
import { validateNames, validateZipFile } from "./validate-package.mjs";

const HERE = new URL(".", import.meta.url);
const README_TEMPLATE = new URL("portable-readme.txt", HERE);

export function portableReadme(version) {
  return readFileSync(README_TEMPLATE, "utf8")
    .replace(/\r\n/g, "\n")
    .replaceAll("{{VERSION}}", version);
}

/** Stages the package under `<out>/Clade-Launcher` from a clean folder. Returns the staging path. */
export function stage({
  exe,
  webview2Dir = null,
  license,
  notices,
  webview2License,
  version,
  out,
}) {
  for (const [what, p] of [
    ["Clade.exe", exe],
    ["LICENSE", license],
    ["THIRD-PARTY-NOTICES.txt", notices],
    ["the WebView2 licence", webview2License],
  ]) {
    if (!p || !existsSync(p)) throw new Error(`missing input: ${what} (${p ?? "not given"})`);
  }
  if (webview2Dir && !existsSync(join(webview2Dir, "msedgewebview2.exe")))
    throw new Error(`missing input: msedgewebview2.exe in ${webview2Dir}`);
  const dir = join(out, PACKAGE_ROOT);
  rmSync(dir, { recursive: true, force: true }); // always start clean
  mkdirSync(join(dir, "licenses"), { recursive: true });
  copyFileSync(exe, join(dir, "Clade.exe"));
  copyFileSync(license, join(dir, "LICENSE"));
  copyFileSync(notices, join(dir, "THIRD-PARTY-NOTICES.txt"));
  copyFileSync(webview2License, join(dir, "licenses", "WebView2-Fixed-Version-License.txt"));
  writeFileSync(join(dir, "README.txt"), portableReadme(version), "utf8");
  if (webview2Dir) cpSync(webview2Dir, join(dir, "runtimes", "webview2"), { recursive: true });
  return dir;
}

/** Builds the package. Throws, writing no ZIP, if the staged files are not acceptable. */
export function buildPackage({ withoutWebview2 = false, epoch = 315532800, ...inputs }) {
  const out = resolve(inputs.out);
  mkdirSync(out, { recursive: true });
  const dir = stage({ ...inputs, out, webview2Dir: withoutWebview2 ? null : inputs.webview2Dir });
  const files = listFiles(dir);
  const problems = validateNames(files, { allowMissingWebview2: withoutWebview2 });
  if (problems.length)
    throw new Error(`the staged package is not acceptable:\n - ${problems.join("\n - ")}`);
  const entries = files.map((name) => ({
    name: `${PACKAGE_ROOT}/${name}`,
    data: readFileSync(join(dir, name)),
  }));
  const zipName = withoutWebview2 ? ZIP_NAME.replace(".zip", "-NO-WEBVIEW2.zip") : ZIP_NAME;
  const zipPath = join(out, zipName);
  writeFileSync(zipPath, buildZip(entries, { epoch }));
  const hex = sha256File(zipPath);
  writeFileSync(`${zipPath}.sha256`, sha256Line(hex, zipName), "utf8");
  return { zipPath, zipName, sha256: hex, files };
}

/** True when the `.sha256` file next to the ZIP names it and matches its content. */
export function checkSha256File(zipPath) {
  const sidecar = `${zipPath}.sha256`;
  if (!existsSync(sidecar)) return { ok: false, reason: "the .sha256 file is missing" };
  const parsed = parseSha256File(readFileSync(sidecar, "utf8"));
  if (!parsed) return { ok: false, reason: "the .sha256 file is not in the expected format" };
  if (parsed.name !== zipPath.split(/[\\/]/).pop())
    return { ok: false, reason: `the .sha256 file names ${parsed.name}` };
  const actual = sha256File(zipPath);
  return actual === parsed.hex
    ? { ok: true, hex: actual }
    : { ok: false, reason: `SHA-256 mismatch: file says ${parsed.hex}, the ZIP is ${actual}` };
}

function main(argv) {
  const arg = (n) => {
    const i = argv.indexOf(n);
    return i >= 0 ? argv[i + 1] : null;
  };
  const need = (n) => {
    const v = arg(n);
    if (!v) {
      console.error(`missing ${n}`);
      process.exit(2);
    }
    return v;
  };
  const withoutWebview2 = argv.includes("--no-webview2");
  try {
    const r = buildPackage({
      exe: resolve(need("--exe")),
      webview2Dir: withoutWebview2 ? null : resolve(need("--webview2")),
      license: resolve(need("--license")),
      notices: resolve(need("--notices")),
      webview2License: resolve(need("--webview2-license")),
      version: need("--version"),
      out: resolve(need("--out")),
      withoutWebview2,
      epoch: process.env.SOURCE_DATE_EPOCH ? Number(process.env.SOURCE_DATE_EPOCH) : undefined,
    });
    const check = validateZipFile(r.zipPath, {
      version: null,
      allowMissingWebview2: withoutWebview2,
    });
    if (check.problems.length)
      throw new Error(`the finished ZIP failed validation:\n - ${check.problems.join("\n - ")}`);
    console.log(`${r.zipName}  ${r.files.length} files  SHA-256 ${r.sha256}`);
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
