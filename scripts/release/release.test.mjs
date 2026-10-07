// @vitest-environment node
// Tests for the release tooling. They build everything they need in temporary folders (a fake
// executable, a fake WebView2 folder, synthetic cargo metadata) and need no network, no account and no
// real runtime.

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  buildZip,
  parseSha256File,
  readZipDirectory,
  readZipEntry,
  sha256Hex,
  sha256Line,
} from "./common.mjs";
import { changelogSection, checkVersions, isPrerelease, parseTag } from "./check-versions.mjs";
import {
  frontendRoots,
  groupByLicenseText,
  loadLicenseSources,
  packageNameOf,
  renderNotices,
  rustRuntimePackages,
} from "./generate-notices.mjs";
import { buildPackage, checkSha256File, portableReadme, stage } from "./package.mjs";
import {
  executableVersion,
  scanExecutable,
  scanText,
  validateNames,
  validateZipFile,
} from "./validate-package.mjs";
import { configProblems, fetchCab, verifyCab } from "./webview2.mjs";

let tmp;
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "clade-release-test-"));
});
afterEach(() => rmSync(tmp, { recursive: true, force: true }));

const put = (rel, text) => {
  const p = join(tmp, rel);
  mkdirSync(join(p, ".."), { recursive: true });
  writeFileSync(p, text);
  return p;
};

// ------------------------------------------------------------------------------ version checks

function repo({
  pkg = "0.1.0",
  cargo = "0.1.0",
  tauri = "0.1.0",
  lock = "0.1.0",
  changelog = "## [0.1.0] - 2026-10-05\n\n- First preview.\n",
} = {}) {
  put("package.json", JSON.stringify({ name: "clade-launcher", version: pkg }));
  put(
    "src-tauri/Cargo.toml",
    `[package]\nname = "clade-launcher"\nversion = "${cargo}"\n\n[dependencies]\nserde = { version = "1", features = [] }\n`,
  );
  put("src-tauri/tauri.conf.json", JSON.stringify({ version: tauri }));
  put(
    "src-tauri/Cargo.lock",
    `[[package]]\nname = "other"\nversion = "9.9.9"\n\n[[package]]\nname = "clade-launcher"\nversion = "${lock}"\n`,
  );
  if (changelog !== null)
    put("CHANGELOG.md", `# Changelog\n\n## [Unreleased]\n\n- Work.\n\n${changelog}`);
  return tmp;
}

describe("version consistency", () => {
  it("accepts four files that agree", () => {
    expect(checkVersions({ root: repo() })).toEqual({ problems: [], version: "0.1.0" });
  });

  it("rejects any file that disagrees, naming all of them", () => {
    for (const bad of [
      { pkg: "0.2.0" },
      { cargo: "0.2.0" },
      { tauri: "0.2.0" },
      { lock: "0.2.0" },
    ]) {
      const { problems } = checkVersions({ root: repo(bad) });
      expect(problems.join("\n")).toMatch(/version sources disagree/);
      expect(problems.join("\n")).toMatch(
        /package\.json=.*Cargo\.toml=.*tauri\.conf\.json=.*Cargo\.lock=/,
      );
    }
  });

  it("rejects a version that is not plain X.Y.Z and a missing file", () => {
    expect(
      checkVersions({
        root: repo({
          pkg: "0.1.0-preview",
          cargo: "0.1.0-preview",
          tauri: "0.1.0-preview",
          lock: "0.1.0-preview",
        }),
      }).problems.join(),
    ).toMatch(/not a plain X\.Y\.Z/);
    const root = repo();
    rmSync(join(root, "src-tauri/tauri.conf.json"));
    expect(checkVersions({ root }).problems).toContain("src-tauri/tauri.conf.json is missing");
  });

  it("accepts the supported tag shapes and rejects the rest", () => {
    for (const ok of ["v0.1.0", "v0.1.0-preview", "v0.1.0-preview.2", "v1.2.3-rc.1"])
      expect(parseTag(ok)).not.toBeNull();
    for (const bad of [
      "0.1.0",
      "v0.1",
      "v0.1.0-beta",
      "v0.1.0-preview.",
      "v01.2.3",
      "release-1",
      "v0.1.0-preview+x",
      "",
    ])
      expect(parseTag(bad), bad).toBeNull();
    const { problems } = checkVersions({ root: repo(), tag: "v0.1-preview" });
    expect(problems.join()).toMatch(/is not a release tag/);
  });

  it("requires the tag to agree with the files", () => {
    expect(checkVersions({ root: repo(), tag: "v0.1.0-preview" }).problems).toEqual([]);
    expect(checkVersions({ root: repo(), tag: "v0.2.0-preview" }).problems.join()).toMatch(
      /tag v0\.2\.0-preview is for 0\.2\.0 but the files say 0\.1\.0/,
    );
  });

  it("requires a changelog entry for a tagged release", () => {
    const none = checkVersions({ root: repo({ changelog: "" }), tag: "v0.1.0-preview" });
    expect(none.problems.join()).toMatch(/no "## \[0\.1\.0\]" entry/);
    const root = repo();
    rmSync(join(root, "CHANGELOG.md"));
    const missing = checkVersions({ root, tag: "v0.1.0-preview" });
    expect(missing.problems).toContain("CHANGELOG.md is missing");
    const empty = checkVersions({
      root: repo({ changelog: "## [0.1.0]\n\n## [0.0.9]\n\n- Old.\n" }),
      tag: "v0.1.0",
    });
    expect(empty.problems.join()).toMatch(/entry for 0\.1\.0 is empty/);
    // without a tag the changelog is not required
    expect(checkVersions({ root: repo({ changelog: "" }) }).problems).toEqual([]);
  });

  it("marks previews and 0.x releases as pre-releases", () => {
    expect(isPrerelease("v0.1.0-preview")).toBe(true);
    expect(isPrerelease("v0.1.0")).toBe(true);
    expect(isPrerelease("v1.0.0-rc.1")).toBe(true);
    expect(isPrerelease("v1.0.0")).toBe(false);
    expect(isPrerelease("garbage")).toBe(true);
  });

  it("extracts exactly one changelog section", () => {
    const text = "## [Unreleased]\n\n- a\n\n## [0.1.0] - d\n\n- b\n- c\n\n## [0.0.1]\n\n- old\n";
    expect(changelogSection(text, "0.1.0")).toBe("- b\n- c");
    expect(changelogSection(text, "0.1")).toBeNull();
  });
});

// ------------------------------------------------------------------------------ packaging

const file = (name, text = "x") => ({ name: `Clade-Launcher/${name}`, data: Buffer.from(text) });
const goodEntries = () => [
  file("Clade.exe", "MZ fake executable"),
  file("README.txt", "Clade is portable."),
  file("LICENSE", "Apache License"),
  file("THIRD-PARTY-NOTICES.txt", "notices"),
  file("licenses/WebView2-Fixed-Version-License.txt", "terms"),
  file("runtimes/webview2/msedgewebview2.exe", "MZ webview2"),
  file("runtimes/webview2/Locales/en-US.pak", "pak"),
];
const zipOf = (entries) => {
  const p = join(tmp, "test.zip");
  writeFileSync(p, buildZip(entries));
  return p;
};
const problemsOf = (entries, options) => validateZipFile(zipOf(entries), options).problems;
const without = (name) => goodEntries().filter((e) => e.name !== `Clade-Launcher/${name}`);

describe("package allowlist", () => {
  it("accepts a valid portable package", () => {
    expect(problemsOf(goodEntries())).toEqual([]);
  });

  it("rejects an unexpected file or folder, even a harmless-looking one", () => {
    for (const extra of [
      "notes.txt",
      "src/main.rs",
      "src-tauri/src/lib.rs",
      "docs/TECHNICAL.md",
      "licenses/extra.dll",
      "licenses/sub/a.txt",
      "other/file.txt",
      "tools/run.ps1",
    ]) {
      const p = problemsOf([...goodEntries(), file(extra)]);
      expect(p.join(), extra).toMatch(/not on the allowlist|forbidden/);
    }
  });

  it("rejects user data and development leftovers by name", () => {
    for (const bad of [
      "data/settings.json",
      "data/vault.bin",
      "data/accounts.json",
      "instances/a/instance.json",
      "logs/clade.log",
      "cache/x",
      "minecraft/versions/a.json",
      "libraries/a.jar",
      "assets/a",
      "worlds/w/level.dat",
      "settings.json",
      "vault.bin",
      "accounts.json",
      "playtime.json",
      "clade.log",
      ".env",
      "CLAUDE.md",
      ".git/config",
      ".github/ci.yml",
      "node_modules/a/index.js",
      "target/release/a.exe",
      "test-results/a.png",
      "screenshots/a.png",
      "key.pem",
      "a.bak",
      "settings.json.corrupt",
      "Clade.pdb",
      "Diagnostics.zip",
      "runtimes/webview2/nested/node_modules/x.js",
    ]) {
      expect(problemsOf([...goodEntries(), file(bad)]).length, bad).toBeGreaterThan(0);
    }
  });

  it("rejects a vault or settings file hidden inside the runtime folder too", () => {
    expect(problemsOf([...goodEntries(), file("runtimes/webview2/vault.bin")]).join()).toMatch(
      /forbidden file/,
    );
    expect(
      problemsOf([...goodEntries(), file("runtimes/webview2/x/settings.json")]).join(),
    ).toMatch(/forbidden file/);
  });

  it("rejects missing LICENSE, notices, WebView2 licence and runtime", () => {
    expect(problemsOf(without("LICENSE")).join()).toMatch(/required file is missing: LICENSE/);
    expect(problemsOf(without("THIRD-PARTY-NOTICES.txt")).join()).toMatch(
      /THIRD-PARTY-NOTICES\.txt/,
    );
    expect(problemsOf(without("licenses/WebView2-Fixed-Version-License.txt")).join()).toMatch(
      /WebView2-Fixed-Version-License/,
    );
    expect(problemsOf(without("README.txt")).join()).toMatch(/README\.txt/);
    expect(problemsOf(without("Clade.exe")).join()).toMatch(/Clade\.exe/);
    const noRuntime = goodEntries().filter((e) => !e.name.includes("runtimes/"));
    expect(problemsOf(noRuntime).join()).toMatch(/bundled WebView2 runtime is missing/);
    // only a development package may lack the runtime, and only when it says so
    expect(problemsOf(noRuntime, { allowMissingWebview2: true })).toEqual([]);
  });

  it("rejects entries outside the single top folder", () => {
    expect(
      problemsOf([...goodEntries(), { name: "Other/x.txt", data: Buffer.from("x") }]).join(),
    ).toMatch(/outside the Clade-Launcher\/ folder/);
    expect(
      problemsOf([...goodEntries(), { name: "loose.txt", data: Buffer.from("x") }]).join(),
    ).toMatch(/outside/);
  });

  it("rejects unsafe or duplicate names", () => {
    expect(validateNames(["Clade.exe", "../evil"]).join()).toMatch(/unsafe entry name/);
    expect(validateNames(["Clade.exe", "C:/x"]).join()).toMatch(/unsafe entry name/);
    expect(validateNames(["Clade.exe", "clade.exe"]).join()).toMatch(/duplicate entry/);
    expect(() => buildZip([file("a"), file("a")])).toThrow(/duplicate/);
    expect(() => buildZip([{ name: "../a", data: Buffer.from("") }])).toThrow(/unsafe/);
  });

  it("finds developer paths and secrets in the shipped text", () => {
    const withText = (name, text) =>
      goodEntries().map((e) =>
        e.name === `Clade-Launcher/${name}` ? { ...e, data: Buffer.from(text) } : e,
      );
    expect(
      problemsOf(withText("README.txt", "built in C:\\Users\\someone\\Projects\\clade")).join(),
    ).toMatch(/developer path/);
    expect(problemsOf(withText("THIRD-PARTY-NOTICES.txt", "see /home/dev/x/")).join()).toMatch(
      /developer path/,
    );
    expect(scanText("a", "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789")[0]).toMatch(
      /secret/,
    );
    expect(
      scanText("a", "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkw.abcdefghijklmnop")[0],
    ).toMatch(/secret/);
    expect(scanText("a", "-----BEGIN PRIVATE KEY-----").length).toBe(1);
    expect(scanText("a", "ghp_" + "a".repeat(36)).length).toBe(1);
    // ordinary licence text and public Windows folders are fine
    expect(
      scanText("a", "Permission is hereby granted... C:\\Users\\Public\\Documents").length,
    ).toBe(0);
  });

  it("looks for developer-specific strings in the executable without changing it", () => {
    const exe = Buffer.from("MZ....C:\\Users\\devname\\.cargo\\registry....");
    expect(scanExecutable(exe, ["C:\\Users\\devname"]).join()).toMatch(/developer-specific text/);
    expect(
      scanExecutable(Buffer.from("C:\\Users\\devname", "utf16le"), ["C:\\Users\\devname"]).length,
    ).toBe(1);
    expect(scanExecutable(exe, ["somebody-else"]).length).toBe(0);
    expect(problemsOf(goodEntries(), { forbidStrings: ["fake executable"] }).join()).toMatch(
      /Clade\.exe contains/,
    );
  });

  it("reads the product version from a version resource", () => {
    const resource = Buffer.concat([
      Buffer.from("....", "latin1"),
      Buffer.from("ProductVersion\0", "utf16le"),
      Buffer.from("0.1.0\0", "utf16le"),
    ]);
    expect(executableVersion(resource)).toBe("0.1.0");
    expect(executableVersion(Buffer.from("no resource here"))).toBeNull();
    const entries = goodEntries().map((e) =>
      e.name.endsWith("Clade.exe") ? { ...e, data: resource } : e,
    );
    expect(problemsOf(entries, { version: "0.1.0" })).toEqual([]);
    expect(problemsOf(entries, { version: "0.2.0" }).join()).toMatch(
      /says version 0\.1\.0, expected 0\.2\.0/,
    );
  });
});

describe("the ZIP writer and reader", () => {
  it("round-trips, is deterministic and independent of input order", () => {
    const a = buildZip([file("b.txt", "bbb".repeat(100)), file("a.txt", "")]);
    const b = buildZip([file("a.txt", ""), file("b.txt", "bbb".repeat(100))]);
    expect(a.equals(b)).toBe(true);
    const entries = readZipDirectory(a);
    expect(entries.map((e) => e.name)).toEqual(["Clade-Launcher/a.txt", "Clade-Launcher/b.txt"]);
    expect(readZipEntry(a, entries[1]).toString()).toBe("bbb".repeat(100));
    expect(readZipEntry(a, entries[0]).length).toBe(0);
  });
  it("refuses something that is not a ZIP", () => {
    expect(() => readZipDirectory(Buffer.from("not a zip at all, definitely not"))).toThrow(
      /not a ZIP/,
    );
  });
});

// ------------------------------------------------------------------------------ build + checksums

function inputs() {
  const exe = put("in/Clade.exe", "MZ fake executable");
  const license = put("in/LICENSE", "Apache License text");
  const notices = put("in/notices.txt", "notices");
  const wvLicense = put("in/wv.txt", "webview2 terms");
  put("in/wv2/msedgewebview2.exe", "MZ runtime");
  put("in/wv2/Locales/en-US.pak", "pak");
  return {
    exe,
    license,
    notices,
    webview2License: wvLicense,
    webview2Dir: join(tmp, "in/wv2"),
    version: "0.1.0",
    out: join(tmp, "out"),
  };
}

describe("building the package", () => {
  it("builds a valid portable ZIP with a matching .sha256 file", () => {
    const r = buildPackage(inputs());
    expect(r.zipName).toBe("Clade-Launcher-Portable.zip");
    expect(r.files).toEqual([
      "Clade.exe",
      "LICENSE",
      "README.txt",
      "THIRD-PARTY-NOTICES.txt",
      "licenses/WebView2-Fixed-Version-License.txt",
      "runtimes/webview2/Locales/en-US.pak",
      "runtimes/webview2/msedgewebview2.exe",
    ]);
    expect(validateZipFile(r.zipPath).problems).toEqual([]);
    expect(checkSha256File(r.zipPath)).toEqual({ ok: true, hex: r.sha256 });
    expect(readFileSync(`${r.zipPath}.sha256`, "utf8")).toBe(
      sha256Line(r.sha256, "Clade-Launcher-Portable.zip"),
    );
  });

  it("is deterministic: the same inputs give the same bytes", () => {
    const first = buildPackage(inputs()).sha256;
    const second = buildPackage(inputs()).sha256;
    expect(second).toBe(first);
  });

  it("detects an incorrect SHA-256", () => {
    const r = buildPackage(inputs());
    writeFileSync(`${r.zipPath}.sha256`, sha256Line("0".repeat(64), "Clade-Launcher-Portable.zip"));
    expect(checkSha256File(r.zipPath).reason).toMatch(/SHA-256 mismatch/);
    writeFileSync(`${r.zipPath}.sha256`, sha256Line(r.sha256, "Other.zip"));
    expect(checkSha256File(r.zipPath).reason).toMatch(/names Other\.zip/);
    writeFileSync(`${r.zipPath}.sha256`, "garbage");
    expect(checkSha256File(r.zipPath).reason).toMatch(/not in the expected format/);
    rmSync(`${r.zipPath}.sha256`);
    expect(checkSha256File(r.zipPath).reason).toMatch(/missing/);
    // a tampered ZIP no longer matches its file
    const good = buildPackage(inputs());
    writeFileSync(good.zipPath, Buffer.concat([readFileSync(good.zipPath), Buffer.from("x")]));
    expect(checkSha256File(good.zipPath).ok).toBe(false);
  });

  it("refuses missing inputs: no WebView2, no LICENSE, no notices", () => {
    const i = inputs();
    expect(() => buildPackage({ ...i, webview2Dir: join(tmp, "nowhere") })).toThrow(
      /msedgewebview2\.exe/,
    );
    expect(() => buildPackage({ ...i, license: join(tmp, "nope") })).toThrow(
      /missing input: LICENSE/,
    );
    expect(() => buildPackage({ ...i, notices: undefined })).toThrow(/THIRD-PARTY-NOTICES/);
  });

  it("starts from a clean staging folder and refuses a runtime folder that holds user data", () => {
    const i = inputs();
    const dir = stage({ ...i, out: i.out });
    put("out/Clade-Launcher/stray.txt", "left over");
    put("out/Clade-Launcher/data/settings.json", "{}");
    const r = buildPackage(i);
    expect(r.files).not.toContain("stray.txt");
    expect(dir).toContain("Clade-Launcher");
    put("in/wv2/vault.bin", "secret");
    expect(() => buildPackage(i)).toThrow(/forbidden file/);
  });

  it("writes a development package only when asked, under a different name", () => {
    const i = inputs();
    const r = buildPackage({ ...i, withoutWebview2: true });
    expect(r.zipName).toBe("Clade-Launcher-Portable-NO-WEBVIEW2.zip");
    expect(r.files.some((f) => f.startsWith("runtimes/"))).toBe(false);
    expect(validateZipFile(r.zipPath).problems.join()).toMatch(
      /bundled WebView2 runtime is missing/,
    );
    expect(validateZipFile(r.zipPath, { allowMissingWebview2: true }).problems).toEqual([]);
  });

  it("fills the version into the portable README and keeps it factual", () => {
    const text = portableReadme("0.1.0");
    expect(text).toContain("Clade Launcher 0.1.0 (portable)");
    expect(text).not.toContain("{{VERSION}}");
    for (const needle of [
      "Clade.exe",
      "Do not run Clade.exe from inside the ZIP",
      "next to Clade.exe",
      "English letters",
      "reasonably",
      "SmartScreen",
      "Windows 11, 64-bit (x64)",
      "Smart App Control",
      "not code-signed",
      "issues/new/choose",
      "github.com/uoon34232/clade-launcher",
    ])
      expect(text, needle).toContain(needle);
    // honest about the platform and the missing signature, and never advises weakening Windows security
    expect(text).toContain("Please do not disable or bypass Smart App Control");
    expect(text).not.toMatch(/Windows 10 (is|are) supported|run as administrator|disable (Smart App Control|Defender)/i);
    expect(scanText("README.txt", text)).toEqual([]);
  });

  it("parses the .sha256 format", () => {
    expect(parseSha256File(`${"a".repeat(64)}  x.zip\n`)).toEqual({
      hex: "a".repeat(64),
      name: "x.zip",
    });
    expect(parseSha256File("nothing")).toBeNull();
    expect(sha256Hex("abc")).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });
});

// ------------------------------------------------------------------------------ WebView2 input

describe("the WebView2 Fixed Version Runtime input", () => {
  const complete = {
    architecture: "x64",
    version: "130.0.2849.80",
    downloadUrl: "https://example.test/runtime.cab",
    sha256: "a".repeat(64),
    approvedBy: "someone",
  };

  it("is unusable until every value is supplied", () => {
    expect(configProblems(complete)).toEqual([]);
    expect(
      configProblems({
        architecture: "x64",
        version: null,
        downloadUrl: null,
        sha256: null,
        approvedBy: null,
      }).length,
    ).toBe(4);
    expect(configProblems({ ...complete, version: "130" }).join()).toMatch(/"version"/);
    expect(configProblems({ ...complete, sha256: "xyz" }).join()).toMatch(/"sha256"/);
    expect(
      configProblems({ ...complete, downloadUrl: "http://example.test/a.cab" }).join(),
    ).toMatch(/"downloadUrl"/);
    expect(configProblems({ ...complete, approvedBy: " " }).join()).toMatch(/"approvedBy"/);
    expect(configProblems({ ...complete, architecture: "arm64" }).join()).toMatch(/"architecture"/);
  });

  it("verifies a CAB against the pinned hash and rejects any mismatch", async () => {
    const cab = put("a.cab", "cab bytes");
    await expect(verifyCab(cab, sha256Hex("cab bytes"))).resolves.toBe(sha256Hex("cab bytes"));
    await expect(verifyCab(cab, sha256Hex("other"))).rejects.toThrow(/SHA-256 mismatch/);
    await expect(verifyCab(cab, "nope")).rejects.toThrow(/no valid expected SHA-256/);
  });

  it("downloads only the pinned URL and keeps nothing that fails verification", async () => {
    const bytes = Buffer.from("pretend this is a cab");
    const ok = async () => new Response(bytes);
    const cfg = { ...complete, sha256: sha256Hex(bytes) };
    const path = await fetchCab(cfg, join(tmp, "dl"), { fetchImpl: ok });
    expect(readFileSync(path).equals(bytes)).toBe(true);
    const tampered = async () => new Response(Buffer.from("something else"));
    await expect(fetchCab(cfg, join(tmp, "dl2"), { fetchImpl: tampered })).rejects.toThrow(
      /mismatch/,
    );
    expect(() => readFileSync(join(tmp, "dl2", "runtime.cab"))).toThrow();
    expect(() => readFileSync(join(tmp, "dl2", "runtime.cab.part"))).toThrow();
    await expect(
      fetchCab({ ...cfg, sha256: null }, join(tmp, "dl3"), { fetchImpl: ok }),
    ).rejects.toThrow(/not configured/);
    await expect(
      fetchCab(cfg, join(tmp, "dl4"), {
        fetchImpl: async () => new Response("no", { status: 404 }),
      }),
    ).rejects.toThrow(/HTTP 404/);
  });
});

// ------------------------------------------------------------------------------ notices

describe("third-party notices", () => {
  const metadata = () => ({
    packages: [
      { id: "root", name: "clade-launcher", version: "0.1.0", manifest_path: "x" },
      {
        id: "a",
        name: "alpha",
        version: "1.0.0",
        license: "MIT",
        repository: "https://example.test/alpha",
        manifest_path: join(tmp, "a", "Cargo.toml"),
      },
      {
        id: "b",
        name: "beta",
        version: "2.0.0",
        license: "Apache-2.0",
        manifest_path: join(tmp, "b", "Cargo.toml"),
      },
      { id: "buildonly", name: "gamma", version: "3.0.0", license: "MIT", manifest_path: "y" },
      { id: "devonly", name: "delta", version: "4.0.0", license: "MIT", manifest_path: "z" },
    ],
    resolve: {
      root: "root",
      nodes: [
        {
          id: "root",
          deps: [
            { pkg: "a", dep_kinds: [{ kind: null }] },
            { pkg: "buildonly", dep_kinds: [{ kind: "build" }] },
            { pkg: "devonly", dep_kinds: [{ kind: "dev" }] },
          ],
        },
        { id: "a", deps: [{ pkg: "b", dep_kinds: [{ kind: null }, { kind: "build" }] }] },
        { id: "b", deps: [] },
        { id: "buildonly", deps: [] },
        { id: "devonly", deps: [] },
      ],
    },
  });

  it("lists only crates that are linked into the program", () => {
    const names = rustRuntimePackages(metadata()).map((p) => p.name);
    expect(names).toEqual(["alpha", "beta"]);
  });

  it("prints each distinct licence text once and says plainly what it could not cover", () => {
    put("a/LICENSE-MIT", "MIT License\n\nCopyright (c) Alpha authors");
    put(
      "b/LICENSE",
      "Apache License\n   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION\n...",
    );
    const pkgs = rustRuntimePackages(metadata());
    const { groups, uncovered } = groupByLicenseText(pkgs);
    expect(groups.length).toBe(2);
    expect(uncovered).toEqual([]);
    // a package with no licence file is reported, and an MIT text with someone else's copyright is never borrowed
    const lone = [
      { name: "lone", version: "1.0.0", license: "MIT", repository: "", dir: join(tmp, "nothing") },
    ];
    expect(groupByLicenseText([...pkgs, ...lone]).uncovered.map((p) => p.name)).toEqual(["lone"]);
    // a fixed-text licence (Apache-2.0, also as one option of an OR) is covered by the same text
    const dual = [
      {
        name: "dual",
        version: "1.0.0",
        license: "MIT OR Apache-2.0",
        repository: "",
        dir: join(tmp, "nothing"),
      },
    ];
    expect(groupByLicenseText([...pkgs, ...dual]).uncovered).toEqual([]);
    const text = renderNotices({ rust: pkgs, npm: [], projectVersion: "0.1.0", fontPackages: [] });
    expect(text).toContain("Copyright (c) Alpha authors");
    expect(text).toContain("Minecraft");
    expect(text).toContain("WebView2");
    expect(text).not.toMatch(/C:\\Users/);
    expect(renderNotices({ rust: pkgs, npm: [], projectVersion: "0.1.0", fontPackages: [] })).toBe(
      text,
    ); // deterministic
  });

  it("covers a package without a licence file only with a verified text for that exact version", () => {
    const sources = [
      {
        text: "Redistribution and use ... (upstream text)",
        origin: "https://example.test/LICENSE",
        appliesTo: [{ crate: "bare", version: "1.0.0" }],
      },
    ];
    const pkg = (version) => ({ name: "bare", version, license: "BSD-3-Clause", repository: "", dir: join(tmp, "nothing") });
    const ok = groupByLicenseText([pkg("1.0.0")], sources);
    expect(ok.uncovered).toEqual([]);
    expect(ok.groups[0].origin).toBe("https://example.test/LICENSE");
    // a different version must be reviewed again, not silently inherit the text
    expect(groupByLicenseText([pkg("1.0.1")], sources).uncovered.map((p) => p.name)).toEqual(["bare"]);
  });

  it("carries a linked component's own licence text", () => {
    const component = {
      name: "Vendor SDK",
      version: "",
      license: "BSD-style",
      repository: "",
      source: { text: "Vendor licence text", origin: "https://vendor.test" },
    };
    const { groups, uncovered } = groupByLicenseText([component]);
    expect(uncovered).toEqual([]);
    expect(groups[0].text).toBe("Vendor licence text");
  });

  it("checks every stored upstream licence text against its recorded SHA-256", () => {
    const sources = loadLicenseSources();
    expect(sources.length).toBeGreaterThan(0);
    const names = sources.flatMap((x) => (x.appliesTo ?? []).map((a) => a.crate));
    for (const crate of ["alloc-stdlib", "webview2-com", "webview2-com-sys", "webview2-com-macros"])
      expect(names).toContain(crate);
    mkdirSync(join(tmp, "texts"), { recursive: true });
    writeFileSync(join(tmp, "texts", "x.txt"), "tampered");
    writeFileSync(
      join(tmp, "m.json"),
      JSON.stringify({ sources: [{ file: "x.txt", sha256: "0".repeat(64), appliesTo: [] }] }),
    );
    expect(() => loadLicenseSources(join(tmp, "m.json"), join(tmp, "texts"))).toThrow(/SHA-256/);
  });

  it("flags dependencies it could not find instead of pretending", () => {
    const text = renderNotices({
      rust: [],
      npm: [],
      npmMissing: ["ghost-package"],
      projectVersion: "0.1.0",
      fontPackages: [],
    });
    expect(text).toMatch(/INCOMPLETE[\s\S]*ghost-package/);
  });

  it("finds the packages the shipped source imports, and ignores tests and relative imports", () => {
    put(
      "src/a.ts",
      'import React from "react";\nimport { x } from "@base-ui/react/dialog";\nimport "./local";\nimport y from "@/lib/y";\nconst z = import("lucide-react");',
    );
    put("src/styles.css", '@import "tailwindcss";\n@import "tw-animate-css";');
    put("src/a.test.ts", 'import "only-in-tests";');
    put("src/test/setup.ts", 'import "also-only-tests";');
    expect(frontendRoots(join(tmp, "src"))).toEqual({
      scripts: ["@base-ui/react", "lucide-react", "react"],
      styles: ["tailwindcss", "tw-animate-css"],
    });
    expect(packageNameOf("react-dom/client")).toBe("react-dom");
    expect(packageNameOf("@scope/pkg/deep/file")).toBe("@scope/pkg");
    for (const none of ["./a", "../a", "@/lib/a", "node:fs", "https://x"])
      expect(packageNameOf(none)).toBeNull();
  });
});
