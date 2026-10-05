// The WebView2 Fixed Version Runtime input of a release: one pinned version, one pinned SHA-256 of
// the CAB. The configuration lives in scripts/release/webview2-runtime.json and is never filled in
// with guessed values: until a person supplies the approved version, URL and hash, every release step
// that needs the runtime fails with a message saying exactly what is missing.
//
//   node scripts/release/webview2.mjs status                       # is it configured? (exit 1 if not)
//   node scripts/release/webview2.mjs fetch --out <dir>            # download the pinned CAB, verify its hash
//   node scripts/release/webview2.mjs verify --cab <file>          # verify a CAB you already have
//
// For local experiments only, `--version` and `--sha256` replace the configuration (and the output says
// so); the release workflow never passes them.

import { createHash } from "node:crypto";
import {
  createReadStream,
  createWriteStream,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));
export const CONFIG_PATH = join(HERE, "webview2-runtime.json");

export const WEBVIEW2_VERSION_RE = /^\d+\.\d+\.\d+\.\d+$/;
const SHA256_RE = /^[0-9a-f]{64}$/;

export function loadConfig(path = CONFIG_PATH) {
  return JSON.parse(readFileSync(path, "utf8"));
}

/** What is still missing or malformed. An empty list means the configuration can be used. */
export function configProblems(config, { needUrl = true } = {}) {
  const problems = [];
  if (config.architecture !== "x64") problems.push('"architecture" must be "x64"');
  if (typeof config.version !== "string" || !WEBVIEW2_VERSION_RE.test(config.version))
    problems.push('"version" is not set (an exact four-part version such as 1.2.3.4 is required)');
  if (typeof config.sha256 !== "string" || !SHA256_RE.test(config.sha256))
    problems.push('"sha256" is not set (the 64-hex SHA-256 of the CAB is required)');
  if (needUrl) {
    let ok = false;
    try {
      const u = new URL(config.downloadUrl);
      ok = u.protocol === "https:" && !u.username && !u.password;
    } catch {
      /* not a URL */
    }
    if (!ok) problems.push('"downloadUrl" is not set (an https URL from Microsoft is required)');
  }
  if (typeof config.approvedBy !== "string" || !config.approvedBy.trim())
    problems.push('"approvedBy" is not set (who accepted the redistribution terms)');
  return problems;
}

export function sha256OfFile(path) {
  return new Promise((resolveHash, reject) => {
    const h = createHash("sha256");
    createReadStream(path)
      .on("data", (c) => h.update(c))
      .on("error", reject)
      .on("end", () => resolveHash(h.digest("hex")));
  });
}

/** Throws unless the file's SHA-256 equals `expected`. */
export async function verifyCab(path, expected) {
  if (!SHA256_RE.test(expected ?? "")) throw new Error("no valid expected SHA-256 was given");
  const actual = await sha256OfFile(path);
  if (actual !== expected.toLowerCase())
    throw new Error(
      `SHA-256 mismatch for ${basename(path)}: expected ${expected}, got ${actual}. The file was not used.`,
    );
  return actual;
}

/** Downloads the pinned CAB into `outDir`, verifies it, and returns its path. Nothing is kept on mismatch. */
export async function fetchCab(config, outDir, { fetchImpl = fetch } = {}) {
  const problems = configProblems(config);
  if (problems.length)
    throw new Error(`the WebView2 runtime is not configured: ${problems.join("; ")}`);
  const url = new URL(config.downloadUrl);
  mkdirSync(outDir, { recursive: true });
  const name = basename(url.pathname) || `webview2-${config.version}.cab`;
  const part = join(outDir, `${name}.part`);
  const dest = join(outDir, name);
  const res = await fetchImpl(url, { redirect: "follow" });
  if (!res.ok || !res.body) throw new Error(`download failed: HTTP ${res.status}`);
  await pipeline(Readable.fromWeb(res.body), createWriteStream(part));
  try {
    await verifyCab(part, config.sha256);
  } catch (e) {
    rmSync(part, { force: true });
    throw e;
  }
  renameSync(part, dest);
  return dest;
}

async function main(argv) {
  const [cmd] = argv;
  const arg = (n) => {
    const i = argv.indexOf(n);
    return i >= 0 ? argv[i + 1] : null;
  };
  const config = loadConfig();
  const override = arg("--sha256") || arg("--version");
  if (arg("--sha256")) config.sha256 = arg("--sha256");
  if (arg("--version")) config.version = arg("--version");
  if (override)
    console.warn(
      "WARNING: the configured WebView2 pin was overridden for this run (not a release).",
    );
  try {
    if (cmd === "status") {
      const problems = configProblems(config, { needUrl: argv.includes("--need-url") });
      if (problems.length) {
        console.error(
          `The WebView2 Fixed Version Runtime is not configured:\n - ${problems.join("\n - ")}`,
        );
        process.exit(1);
      }
      console.log(`WebView2 Fixed Version ${config.version}, SHA-256 ${config.sha256}`);
    } else if (cmd === "fetch") {
      console.log(await fetchCab(config, resolve(arg("--out") ?? ".")));
    } else if (cmd === "verify") {
      const cab = arg("--cab");
      if (!cab) throw new Error("--cab <file> is required");
      await verifyCab(resolve(cab), config.sha256);
      console.log(`SHA-256 verified for ${basename(cab)}`);
    } else {
      throw new Error("usage: webview2.mjs status | fetch --out <dir> | verify --cab <file>");
    }
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href)
  await main(process.argv.slice(2));
