// @vitest-environment node
// Static checks of the GitHub Actions workflows. A real tag run cannot be tested without creating a tag
// (and a draft release), so the properties that matter for safety are asserted on the files themselves:
// what triggers the release, what it may write, that it can only ever create a draft, and that every action
// stays pinned to a commit.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = join(import.meta.dirname, "..", "..");
const workflowDir = join(root, ".github", "workflows");
const read = (name) => readFileSync(join(workflowDir, name), "utf8").replaceAll("\r\n", "\n");
const release = read("release.yml");
const ci = read("ci.yml");

/** The text of one top-level job (from its `  name:` line to the next job or the end). */
function job(text, name) {
  const start = text.search(new RegExp(`^  ${name}:`, "m"));
  expect(start, `job ${name}`).toBeGreaterThanOrEqual(0);
  const rest = text.slice(start + 1);
  const next = rest.search(/^ {2}[a-z][a-z0-9-]*:/m);
  return next < 0 ? text.slice(start) : text.slice(start, start + 1 + next);
}

/** Top-level job names inside the `jobs:` section of a workflow file. */
function jobNames(text) {
  const jobsIdx = text.search(/^jobs:\s*$/m);
  expect(jobsIdx).toBeGreaterThanOrEqual(0);
  return [...text.slice(jobsIdx).matchAll(/^ {2}([a-z][a-z0-9-]*):/gm)].map((m) => m[1]);
}

/** Parsed steps (`{ raw, shell, run, lines }`) from one job's `steps:` block. */
function stepsOf(jobText) {
  const stepsIdx = jobText.search(/^ {4}steps:\s*$/m);
  expect(stepsIdx).toBeGreaterThanOrEqual(0);
  const body = jobText.slice(stepsIdx).replace(/^ {4}steps:\s*\n/, "");
  return body
    .split(/^ {6}- /m)
    .slice(1)
    .map((chunk) => {
      const normalized = `        ${chunk}`;
      const shellMatch = normalized.match(/^ {8}shell:\s*(\S+)\s*$/m);
      const blockRun = normalized.match(/^ {8}run:\s*\|\s*\n([\s\S]*)$/m);
      if (blockRun) {
        const lines = [];
        for (const l of blockRun[1].split("\n")) {
          if (l.trim() === "") continue;
          const indent = l.match(/^ */)[0].length;
          if (indent < 10) break;
          const trimmed = l.slice(10).trim();
          if (!trimmed.startsWith("#")) lines.push(trimmed);
        }
        return { raw: normalized, shell: shellMatch?.[1] ?? null, run: lines.join("\n"), lines };
      }
      const inlineRun = normalized.match(/^ {8}run:\s*(.+)$/m);
      if (inlineRun) {
        const cmd = inlineRun[1].trim();
        return { raw: normalized, shell: shellMatch?.[1] ?? null, run: cmd, lines: [cmd] };
      }
      return { raw: normalized, shell: shellMatch?.[1] ?? null, run: null, lines: [] };
    });
}

const NATIVE_CMD_RE = /^(?:\$[A-Za-z_]\w*\s*=\s*)?(?:node|git|cargo|pnpm|rustup|\.\/scripts\/)\b/;

describe("every workflow", () => {
  it("pins every action to a full commit SHA", () => {
    for (const f of readdirSync(workflowDir).filter((n) => n.endsWith(".yml"))) {
      const uses = [...read(f).matchAll(/^\s*-?\s*uses:\s*(\S+)/gm)].map((m) => m[1]);
      expect(uses.length, f).toBeGreaterThan(0);
      for (const u of uses) expect(u, `${f}: ${u}`).toMatch(/^[\w.-]+\/[\w./-]+@[0-9a-f]{40}$/);
    }
  });

  it("uses no secret other than the standard GitHub Actions token", () => {
    for (const f of readdirSync(workflowDir).filter((n) => n.endsWith(".yml"))) {
      const refs = [...read(f).matchAll(/\$\{\{\s*secrets\.([A-Za-z0-9_]+)/g)].map((m) => m[1]);
      expect(
        refs.filter((r) => r !== "GITHUB_TOKEN"),
        f,
      ).toEqual([]);
    }
  });

  it("never interpolates ${{ ... }} expressions directly inside run: scripts", () => {
    for (const f of readdirSync(workflowDir).filter((n) => n.endsWith(".yml"))) {
      const wf = read(f);
      let checked = 0;
      for (const j of jobNames(wf)) {
        for (const s of stepsOf(job(wf, j))) {
          if (s.run === null) continue;
          checked++;
          expect(s.run, `${f} (${j})`).not.toContain("${{");
        }
      }
      expect(checked, f).toBeGreaterThan(0);
    }
  });

  it("uses $env: syntax and checks $LASTEXITCODE after native commands in multi-line Windows pwsh steps", () => {
    for (const f of readdirSync(workflowDir).filter((n) => n.endsWith(".yml"))) {
      const wf = read(f);
      for (const j of jobNames(wf)) {
        const jobText = job(wf, j);
        if (!/runs-on:\s*windows-/.test(jobText)) continue;
        for (const s of stepsOf(jobText)) {
          if (s.run === null) continue;
          const isPwsh = s.shell === null || s.shell === "pwsh" || s.shell === "powershell";
          if (!isPwsh) continue;
          expect(s.run, `${f} (${j}): bare POSIX env var in pwsh`).not.toMatch(
            /\$(?!env:)(?:GITHUB_|RUNNER_)/,
          );
          if (s.lines.length > 1) {
            expect(s.shell, `${f} (${j}): explicit shell: pwsh on multi-line step`).toBe("pwsh");
            for (let i = 0; i < s.lines.length; i++) {
              if (!NATIVE_CMD_RE.test(s.lines[i])) continue;
              expect(
                s.lines[i + 1] ?? "",
                `${f} (${j}): missing $LASTEXITCODE check after "${s.lines[i]}"`,
              ).toMatch(/^if\s*\(\$LASTEXITCODE\)/);
            }
          }
        }
      }
    }
  });
});

describe("the release workflow", () => {
  it("runs only for pushed version tags, never for branches or pull requests", () => {
    const on = release.slice(release.indexOf("\non:"), release.indexOf("\npermissions:"));
    expect(on).toMatch(/push:\s*\n\s*tags:\s*\n\s*- "v\*"/);
    expect(on).not.toMatch(/branches|pull_request|workflow_dispatch|schedule/);
  });

  it("has no default permissions and only the draft job can write", () => {
    expect(release).toMatch(/^permissions: \{\}$/m);
    const writers = [...release.matchAll(/contents:\s*write/g)];
    expect(writers.length).toBe(1);
    expect(job(release, "draft-release")).toMatch(/contents:\s*write/);
    expect(job(release, "build")).toMatch(/contents:\s*read/);
    expect(job(release, "build")).not.toMatch(/write/);
  });

  it("only ever creates a DRAFT release and marks previews as pre-releases", () => {
    const draft = job(release, "draft-release");
    expect(draft).toMatch(/gh release create/);
    expect(draft).toMatch(/flags="--draft"/);
    expect(draft).toMatch(/--prerelease/);
    expect(draft).toMatch(/--verify-tag/);
    expect(release).not.toMatch(
      /--draft=false|--latest|gh release edit|gh release upload|--prerelease=false/,
    );
    expect(release).not.toMatch(/softprops|ncipollo|marvinpinto|actions\/create-release/);
    // the draft job does not build anything and does not check the repository out
    expect(draft).not.toMatch(/actions\/checkout|pnpm|cargo/);
  });

  it("checks the tag, the versions, the changelog and that the tag is on main before building", () => {
    const build = job(release, "build");
    const check = build.indexOf("check-versions.mjs --tag");
    expect(check).toBeGreaterThan(0);
    expect(build.indexOf("merge-base --is-ancestor")).toBeGreaterThan(check);
    expect(check).toBeLessThan(build.indexOf("pnpm install"));
    expect(build).toMatch(/fetch-depth: 0/);
    expect(build).toMatch(/persist-credentials: false/);
  });

  it("uses locked, frozen installs, the pinned toolchain and the same gates as CI", () => {
    const build = job(release, "build");
    expect(build).toMatch(/pnpm install --frozen-lockfile/);
    expect(build).toMatch(/rustup toolchain install\s*$/m);
    expect(build).toMatch(/cargo clippy --locked --all-targets -- -D warnings/);
    expect(build).toMatch(/cargo test --locked/);
    expect(build).toMatch(/cargo fmt --check/);
    expect(build).toMatch(/pnpm check/);
    expect(build).toMatch(/pnpm test:e2e/);
    expect(build).not.toMatch(/actions\/cache/);
  });

  it("builds through the packaging script, verifies the pinned runtime and smoke tests the result", () => {
    const build = job(release, "build");
    expect(build).toMatch(/webview2\.mjs status/);
    expect(build).toMatch(/webview2\.mjs fetch/);
    expect(build).toMatch(/package-portable\.ps1 -WebView2Cab/);
    expect(build).toMatch(/smoke-test-package\.ps1/);
    expect(build).not.toMatch(/cargo build --release/);
    // the package script, not the workflow, runs the Tauri production build
    expect(readFileSync(join(root, "scripts", "package-portable.ps1"), "utf8")).toMatch(
      /pnpm tauri build --no-bundle --target \$target/,
    );
  });

  it("uploads only the ZIP, its checksum and the notes", () => {
    const build = job(release, "build");
    const paths = build.slice(build.indexOf("path: |"), build.indexOf("if-no-files-found"));
    const lines = paths
      .split("\n")
      .slice(1)
      .map((l) => l.trim())
      .filter(Boolean);
    expect(lines).toEqual([
      "dist-portable/Clade-Launcher-Portable.zip",
      "dist-portable/Clade-Launcher-Portable.zip.sha256",
      "dist-portable/release-notes.md",
    ]);
  });

  it("verifies the checksum again before creating the draft", () => {
    expect(job(release, "draft-release")).toMatch(
      /sha256sum --check Clade-Launcher-Portable\.zip\.sha256/,
    );
  });
});

describe("normal CI", () => {
  it("keeps its required check names", () => {
    for (const name of [
      "Frontend (typecheck, lint, unit tests, build)",
      "Rust (fmt, clippy, tests)",
      "Rust Linux (fmt, clippy, tests)",
      "E2E (Playwright, Chromium)",
    ])
      expect(ci).toContain(`name: ${name}`);
  });

  it("is read-only and does not publish", () => {
    expect(ci).toMatch(/^permissions:\n\s+contents: read$/m);
    expect(ci).not.toMatch(/contents:\s*write|gh release/);
  });

  it("pins the Rust toolchain and builds in locked mode", () => {
    expect(ci).not.toMatch(/toolchain install stable/);
    expect(ci).toMatch(/rustup toolchain install\s*$/m);
    expect(ci).toMatch(/cargo clippy --locked/);
    expect(ci).toMatch(/cargo test --locked/);
    expect(ci.match(/pnpm install --frozen-lockfile/g)?.length).toBe(2);
    const toolchain = readFileSync(join(root, "rust-toolchain.toml"), "utf8");
    expect(toolchain).toMatch(/channel = "\d+\.\d+\.\d+"/);
    expect(toolchain).toMatch(/components = \["rustfmt", "clippy"\]/);
  });

  it("checks that the version is the same everywhere and verifies third-party notices in --strict mode", () => {
    const frontend = job(ci, "frontend");
    expect(frontend).toMatch(/node scripts\/release\/check-versions\.mjs/);
    expect(frontend).toMatch(
      /node scripts\/release\/generate-notices\.mjs --out "\$env:RUNNER_TEMP\/THIRD-PARTY-NOTICES\.txt" --strict/,
    );
    expect(frontend).toMatch(
      /node scripts\/release\/generate-notices\.mjs --target x86_64-unknown-linux-gnu --out "\$env:RUNNER_TEMP\/THIRD-PARTY-NOTICES-linux\.txt" --strict/,
    );
  });
});
