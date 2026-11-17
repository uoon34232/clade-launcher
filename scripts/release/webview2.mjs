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
const APPROVED_DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
export const DEFAULT_FETCH_TIMEOUT_MS = 300_000;
export const ALLOWED_WEBVIEW2_HOST_SUFFIXES = [".microsoft.com", ".msecnd.net", ".azureedge.net"];

export function loadConfig(path = CONFIG_PATH) {
  return JSON.parse(readFileSync(path, "utf8"));
}

/** True when `value` is a strict YYYY-MM-DD calendar date that is not in the future. */
export function isValidApprovedDate(value, { now = new Date() } = {}) {
  if (typeof value !== "string" || !APPROVED_DATE_RE.test(value)) return false;
  const [year, month, day] = value.split("-").map(Number);
  if (year < 2020 || month < 1 || month > 12 || day < 1 || day > 31) return false;
  const parsed = new Date(Date.UTC(year, month - 1, day));
  if (
    parsed.getUTCFullYear() !== year ||
    parsed.getUTCMonth() + 1 !== month ||
    parsed.getUTCDate() !== day
  ) {
    return false;
  }
  const latestAllowed = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  return parsed.getTime() <= latestAllowed;
}

/** True when `rawUrl` is an https URL without credentials on an official Microsoft CDN/download host. */
export function isAllowedWebview2Url(rawUrl) {
  if (typeof rawUrl !== "string" || !rawUrl) return false;
  let u;
  try {
    u = new URL(rawUrl);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.username || u.password) return false;
  const host = u.hostname.toLowerCase().replace(/\.$/, "");
  if (!host || host.startsWith(".") || host.includes("..")) return false;
  return (
    host === "microsoft.com" ||
    ALLOWED_WEBVIEW2_HOST_SUFFIXES.some(
      (suffix) => host.length > suffix.length && host.endsWith(suffix),
    )
  );
}

/** What is still missing or malformed. An empty list means the configuration can be used. */
export function configProblems(config, { needUrl = true, now = new Date() } = {}) {
  const problems = [];
  if (config.architecture !== "x64") problems.push('"architecture" must be "x64"');
  if (typeof config.version !== "string" || !WEBVIEW2_VERSION_RE.test(config.version))
    problems.push('"version" is not set (an exact four-part version such as 1.2.3.4 is required)');
  if (typeof config.sha256 !== "string" || !SHA256_RE.test(config.sha256))
    problems.push('"sha256" is not set (the 64-hex SHA-256 of the CAB is required)');
  const hasUrl =
    config.downloadUrl !== null && config.downloadUrl !== undefined && config.downloadUrl !== "";
  if (needUrl || hasUrl) {
    if (!isAllowedWebview2Url(config.downloadUrl)) {
      problems.push(
        '"downloadUrl" is not set to an official Microsoft https URL (*.microsoft.com, *.msecnd.net, *.azureedge.net)',
      );
    }
  }
  if (typeof config.approvedBy !== "string" || !config.approvedBy.trim())
    problems.push('"approvedBy" is not set (who accepted the redistribution terms)');
  if (!isValidApprovedDate(config.approvedOn, { now }))
    problems.push('"approvedOn" is not set (a valid YYYY-MM-DD approval date is required)');
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
export async function fetchCab(
  config,
  outDir,
  { fetchImpl = fetch, timeoutMs = DEFAULT_FETCH_TIMEOUT_MS, now } = {},
) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0)
    throw new Error("timeoutMs must be a positive number");
  const problems = configProblems(config, { needUrl: true, now });
  if (problems.length)
    throw new Error(`the WebView2 runtime is not configured: ${problems.join("; ")}`);
  const url = new URL(config.downloadUrl);
  mkdirSync(outDir, { recursive: true });
  const name = basename(url.pathname) || `webview2-${config.version}.cab`;
  const part = join(outDir, `${name}.part`);
  const dest = join(outDir, name);
  const signal = AbortSignal.timeout(timeoutMs);
  const res = await fetchImpl(url, { redirect: "follow", signal });
  if (res.url && !isAllowedWebview2Url(res.url)) {
    await res.body?.cancel?.().catch(() => {});
    throw new Error(`download redirected to an untrusted URL: ${res.url}`);
  }
  if (!res.ok || !res.body) throw new Error(`download failed: HTTP ${res.status}`);
  try {
    await pipeline(Readable.fromWeb(res.body), createWriteStream(part), { signal });
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
