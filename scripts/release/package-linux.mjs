#!/usr/bin/env node
// Stages the portable Linux x64 Clade Launcher folder and builds a deterministic
// Clade-Launcher-Linux-x64.tar.gz archive alongside its .sha256 file.

import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { sha256Hex, sha256Line } from "./common.mjs";

export const LINUX_ARCHIVE_NAME = "Clade-Launcher-Linux-x64.tar.gz";
export const EXPECTED_LINUX_FILES = ["Clade", "LICENSE", "README.txt", "THIRD-PARTY-NOTICES.txt"];
const REPRODUCIBLE_MTIME = "@315532800";

const README_TEMPLATE_PATH = join(import.meta.dirname, "portable-readme-linux.txt");

export function portableReadmeLinux(version) {
  if (!version || typeof version !== "string") {
    throw new Error("missing input: version");
  }
  return readFileSync(README_TEMPLATE_PATH, "utf8").replaceAll("{{VERSION}}", version);
}

function assertInputFile(filePath, label) {
  if (!filePath || typeof filePath !== "string") {
    throw new Error(`missing input: ${label}`);
  }
  const abs = resolve(filePath);
  let st;
  try {
    st = statSync(abs);
  } catch {
    throw new Error(`missing input: ${label}: ${abs}`);
  }
  if (!st.isFile() || st.size === 0) {
    throw new Error(`invalid input (expected non-empty file): ${label}: ${abs}`);
  }
  return abs;
}

/** Stages <out>/Clade-Launcher with exact portable Linux files and permissions. */
export function stageLinux({ bin, license, notices, version, out }) {
  const binPath = assertInputFile(bin, "Clade binary");
  const licensePath = assertInputFile(license, "LICENSE");
  const noticesPath = assertInputFile(notices, "THIRD-PARTY-NOTICES.txt");
  if (!out || typeof out !== "string") {
    throw new Error("missing output directory");
  }

  const outDir = resolve(out);
  const stageDir = join(outDir, "Clade-Launcher");
  rmSync(stageDir, { recursive: true, force: true });
  mkdirSync(stageDir, { recursive: true });
  chmodSync(stageDir, 0o755);

  const destBin = join(stageDir, "Clade");
  copyFileSync(binPath, destBin);
  chmodSync(destBin, 0o755);

  const destLicense = join(stageDir, "LICENSE");
  copyFileSync(licensePath, destLicense);
  chmodSync(destLicense, 0o644);

  const destNotices = join(stageDir, "THIRD-PARTY-NOTICES.txt");
  copyFileSync(noticesPath, destNotices);
  chmodSync(destNotices, 0o644);

  const destReadme = join(stageDir, "README.txt");
  writeFileSync(destReadme, portableReadmeLinux(version), { encoding: "utf8", mode: 0o644 });
  chmodSync(destReadme, 0o644);

  const stagedEntries = readdirSync(stageDir, { withFileTypes: true })
    .map((e) => {
      if (!e.isFile()) {
        throw new Error(`unexpected non-file entry in stage directory: ${e.name}`);
      }
      return e.name;
    })
    .sort();
  if (JSON.stringify(stagedEntries) !== JSON.stringify([...EXPECTED_LINUX_FILES].sort())) {
    throw new Error(
      `stage allowlist mismatch: got [${stagedEntries.join(", ")}], expected [${EXPECTED_LINUX_FILES.join(", ")}]`,
    );
  }

  return stageDir;
}

/** Validates the generated tar.gz archive members against a strict fail-closed allowlist. */
export function validateLinuxArchive(archivePath) {
  const listing = execFileSync("tar", ["-tzvf", resolve(archivePath)], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  const lines = listing
    .split(/\r?\n/)
    .map((l) => l.trimEnd())
    .filter(Boolean);
  const expected = new Set([
    "Clade-Launcher/",
    ...EXPECTED_LINUX_FILES.map((name) => `Clade-Launcher/${name}`),
  ]);
  const seen = new Set();

  for (const line of lines) {
    // Format: drwxr-xr-x 0/0 0 1980-01-01 00:00 Clade-Launcher/
    const m = /^(\S+)\s+(\S+)\s+(\d+)\s+\S+\s+\S+\s+(.+)$/.exec(line);
    if (!m) {
      throw new Error(`unparseable tar entry: ${line}`);
    }
    const [, mode, owner, , name] = m;
    if (name.startsWith("/") || name.includes("..")) {
      throw new Error(`unsafe tar entry path: ${name}`);
    }
    if (!expected.has(name)) {
      throw new Error(`forbidden entry in Linux archive: ${name}`);
    }
    if (seen.has(name)) {
      throw new Error(`duplicate entry in Linux archive: ${name}`);
    }
    seen.add(name);
    if (owner !== "0/0") {
      throw new Error(`non-numeric/non-root owner on ${name}: ${owner}`);
    }
    if (name === "Clade-Launcher/") {
      if (!mode.startsWith("d")) {
        throw new Error(`expected directory entry for Clade-Launcher/: ${mode}`);
      }
    } else if (name === "Clade-Launcher/Clade") {
      if (!mode.startsWith("-rwx")) {
        throw new Error(`expected executable permissions on Clade-Launcher/Clade: ${mode}`);
      }
    } else if (!mode.startsWith("-r--") && !mode.startsWith("-rw-")) {
      throw new Error(`expected regular file permissions on ${name}: ${mode}`);
    }
  }

  for (const req of expected) {
    if (!seen.has(req)) {
      throw new Error(`missing required entry in Linux archive: ${req}`);
    }
  }
}

/** Stages and archives Clade-Launcher-Linux-x64.tar.gz plus its .sha256 sidecar. */
export function buildLinuxPackage({ bin, license, notices, version, out }) {
  const stageDir = stageLinux({ bin, license, notices, version, out });
  const outDir = resolve(out);
  const archivePath = join(outDir, LINUX_ARCHIVE_NAME);
  rmSync(archivePath, { force: true });
  rmSync(`${archivePath}.sha256`, { force: true });

  execFileSync(
    "tar",
    [
      "--sort=name",
      `--mtime=${REPRODUCIBLE_MTIME}`,
      "--owner=0",
      "--group=0",
      "--numeric-owner",
      "-czf",
      archivePath,
      "-C",
      outDir,
      "Clade-Launcher",
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );

  validateLinuxArchive(archivePath);
  const sha256 = sha256Hex(readFileSync(archivePath));
  writeFileSync(`${archivePath}.sha256`, sha256Line(sha256, LINUX_ARCHIVE_NAME), "utf8");

  return {
    archiveName: LINUX_ARCHIVE_NAME,
    archivePath,
    sha256,
    stageDir,
    files: [...EXPECTED_LINUX_FILES],
  };
}

function readFlag(argv, name) {
  const idx = argv.indexOf(name);
  if (idx < 0) return null;
  const val = argv[idx + 1];
  if (!val || val.startsWith("-")) return null;
  return val;
}

function main(argv) {
  const bin = readFlag(argv, "--bin");
  const license = readFlag(argv, "--license");
  const notices = readFlag(argv, "--notices");
  const version = readFlag(argv, "--version");
  const out = readFlag(argv, "--out");
  if (!bin || !license || !notices || !version || !out) {
    console.error(
      "usage: package-linux.mjs --bin <path> --license <file> --notices <file> --version <ver> --out <dir>",
    );
    process.exit(2);
  }
  const result = buildLinuxPackage({ bin, license, notices, version, out });
  console.log(`linux package: ${result.archivePath} (${result.sha256})`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main(process.argv.slice(2));
