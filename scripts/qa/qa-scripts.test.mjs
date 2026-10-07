// @vitest-environment node
// Static guards for the Windows QA tooling: the helpers must stay READ-ONLY and safe, the sandbox
// configurations must stay minimal, and nothing developer-specific may be committed in them.

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = join(import.meta.dirname, "..", "..");
const read = (...p) => readFileSync(join(root, ...p), "utf8").replaceAll("\r\n", "\n");
const list = (dir, ext) => readdirSync(join(root, dir)).filter((n) => n.endsWith(ext));

const scripts = [
  ...list("scripts/qa", ".ps1").map((n) => ["scripts/qa", n]),
  ...list("qa/windows-sandbox", ".ps1").map((n) => ["qa/windows-sandbox", n]),
];
const configs = list("qa/windows-sandbox", ".wsb");

/** Commands that would change the host (system, display, GPU, devices, registry, features). */
const FORBIDDEN = [
  /\bpnputil\b/i,
  /\bdevcon\b/i,
  /\b(Disable|Enable)-PnpDevice\b/i,
  /\bDisplaySwitch\b/i,
  /\bChangeDisplaySettings/i,
  /\bSetDisplayConfig\b/i,
  /\bSet-DisplayResolution\b/i,
  /\bSetWindowPos\b/i,
  /\bMoveWindow\b/i,
  /\b65535\b/,
  /\breg(\.exe)?\s+(add|delete|import|load|unload|copy|save|restore)\b/i,
  /\b(Set|New|Remove|Rename|Copy|Move|Clear)-ItemProperty\b/i,
  /\bNew-Item\b[^\n]*\b(HK(LM|CU)|Registry::)/i,
  /\bRemove-Item\b/i,
  /\b(Enable|Disable)-WindowsOptionalFeature\b/i,
  /\bdism(\.exe)?\b/i,
  /\bbcdedit\b/i,
  /\bSet-ExecutionPolicy\b/i,
  /\bStop-Process\b|\btaskkill\b/i,
  /\bSet-MpPreference\b|\bNew-NetFirewallRule\b|\bnetsh\b/i,
  /\bSetProcessDpiAwarenessContext\b|\bSetThreadDpiAwarenessContext\b/i,
];

describe("the QA scripts", () => {
  it("exist", () => {
    expect(scripts.map(([, n]) => n).sort()).toEqual(
      [
        "Start-QaSandbox.ps1",
        "environment-report.ps1",
        "portable-audit.ps1",
        "sandbox-run.ps1",
      ].sort(),
    );
  });

  for (const [dir, name] of scripts) {
    describe(`${dir}/${name}`, () => {
      const text = read(dir, name);
      it("is plain ASCII (safe for every PowerShell encoding)", () => {
        expect([...text].every((c) => c.charCodeAt(0) < 128)).toBe(true);
      });
      it("contains no command that changes the host (comments aside)", () => {
        const code = text.replace(/<#[\s\S]*?#>/g, "").replace(/(^|\s)#.*$/gm, "$1");
        for (const re of FORBIDDEN) expect(code, String(re)).not.toMatch(re);
      });
      it("needs no administrator rights and holds no developer path", () => {
        expect(text).not.toMatch(/#Requires\s+-RunAsAdministrator/i);
        expect(text).not.toMatch(/C:\\Users\\(?!WDAGUtilityAccount)[A-Za-z0-9_.-]+/i);
        expect(text).not.toMatch(/\bkulik\b/i);
      });
    });
  }

  it("parse as PowerShell (when PowerShell is available)", () => {
    const probe = spawnSync(
      "powershell",
      ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"],
      {
        encoding: "utf8",
      },
    );
    if (probe.error || probe.status !== 0) return; // not a Windows runner: the static checks above still ran
    const files = scripts.map(([d, n]) => join(root, d, n).replaceAll("'", "''"));
    const cmd = files
      .map(
        (f) =>
          `$e=$null;$t=$null;[void][System.Management.Automation.Language.Parser]::ParseFile('${f}',[ref]$t,[ref]$e);if($e){$e|%{$_.Message};exit 1}`,
      )
      .join(";");
    const r = spawnSync("powershell", ["-NoProfile", "-Command", cmd], { encoding: "utf8" });
    expect(r.stdout + r.stderr).toBe("");
    expect(r.status).toBe(0);
  }, 15_000);
});

describe("portable-audit Compare: registry Run values", () => {
  const HEX = "507B6EAB61E0461F18F13A409140A5A9";
  const EDGE_X86 = String.raw`C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`;
  const edge = (
    exe = EDGE_X86,
    tail = " --no-startup-window --win-session-start",
    name = `MicrosoftEdgeAutoLaunch_${HEX}`,
  ) => `${name}="${exe}"${tail}`;
  const havePs = (() => {
    const p = spawnSync("powershell", ["-NoProfile", "-Command", "1"], { encoding: "utf8" });
    return !p.error && p.status === 0;
  })();

  /** Runs `portable-audit.ps1 -Action Compare` on two synthetic snapshots that differ only in the Run key. */
  function compare(beforeRun, afterRun) {
    const dir = mkdtempSync(join(tmpdir(), "clade-audit-test-"));
    try {
      const snap = (run) => ({
        meta: { label: "t", takenLocal: "2026-01-01 00:00:00 +00:00" },
        files: {},
        topLevelFolders: {},
        registry: {
          "HKCU:\\Software\\Clade": {},
          "HKCU Run (autostart)": run,
          "HKCU RunOnce (autostart)": {},
        },
        windowsOwnedArtifacts: {},
        processes: [],
        cladeRootFiles: [],
        cladeRootBigFolders: {},
      });
      const b = join(dir, "before.json");
      const a = join(dir, "after.json");
      writeFileSync(b, JSON.stringify(snap(beforeRun)));
      writeFileSync(a, JSON.stringify(snap(afterRun)));
      const r = spawnSync(
        "powershell",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          join(root, "scripts/qa/portable-audit.ps1"),
          "-Action",
          "Compare",
          "-Before",
          b,
          "-After",
          a,
        ],
        { encoding: "utf8" },
      );
      return { code: r.status, out: r.stdout.replaceAll("\r\n", "\n") + r.stderr };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  const externalCount = (out) => Number(out.match(/== EXTERNAL changes[^\n]*\((\d+)\) ==/)[1]);
  const T = 30_000;

  it.skipIf(!havePs)(
    "classifies the genuine Microsoft Edge auto-launch value as browser-owned INFO",
    () => {
      for (const exe of [
        EDGE_X86,
        String.raw`C:\Program Files\Microsoft\Edge\Application\msedge.exe`,
      ]) {
        const r = compare({}, edge(exe));
        expect(r.out).toContain("[browser-owned] Microsoft Edge auto-launch ADDED/CHANGED");
        expect(externalCount(r.out)).toBe(0);
        expect(r.code).toBe(0);
      }
    },
    T,
  );

  it.skipIf(!havePs)(
    "still reports similarly named values that are not the genuine Edge shape",
    () => {
      const lookalikes = [
        edge(String.raw`C:\Users\someone\AppData\Local\Temp\msedge.exe`),
        edge(String.raw`C:\Program Files (x86)\Microsoft\Edge\Application\evil.exe`),
        edge(EDGE_X86, " --no-startup-window --load-extension=x"),
        edge(EDGE_X86, "", "MicrosoftEdgeAutoLaunch_NOTHEX"),
        edge(EDGE_X86, "", "MyEdgeAutoLaunch_" + HEX),
        edge(EDGE_X86, "", "CladeAutoStart"),
      ];
      for (const value of lookalikes) {
        const r = compare({}, value);
        expect(externalCount(r.out), value).toBe(1);
        expect(r.out, value).not.toContain("[browser-owned]");
        expect(r.code, value).toBe(1);
      }
    },
    T,
  );

  it.skipIf(!havePs)(
    "never prints a blank ADDED/REMOVED/CHANGED row for empty registry keys",
    () => {
      for (const [b, a] of [
        [{}, {}],
        [{}, edge()],
        [edge(), {}],
      ]) {
        const r = compare(b, a);
        expect(r.out).not.toMatch(/(ADDED|REMOVED)\/CHANGED\s*$/m);
        expect(r.out).not.toMatch(/^ {2}$/m);
      }
      const same = compare({}, {});
      expect(same.out).toContain("RESULT: no change outside the Clade folder");
      expect(externalCount(same.out)).toBe(0);
    },
    T,
  );
});

describe("the in-sandbox runner", () => {
  const run = read("qa/windows-sandbox", "sandbox-run.ps1");
  const code = run.replace(/^\s*#.*$/gm, "");
  it("writes audit-compare.txt as explicit UTF-8, not via Tee-Object / Out-File", () => {
    expect(code).toMatch(/WriteAllText\('\$results\\audit-compare\.txt'.*UTF8Encoding/);
    expect(code).not.toMatch(/Tee-Object|Out-File|>\s*'?\$results/);
  });
  it("creates run-after.cmd with a process-local ExecutionPolicy Bypass and never sets a policy", () => {
    expect(code).toContain("C:\\CladeQA\\run-after.cmd");
    expect(code).toMatch(/-NoProfile -ExecutionPolicy Bypass -File "C:\\CladeQA\\after\.ps1"/);
    expect(code).not.toMatch(/Set-ExecutionPolicy/i);
  });
  it("prints the Clade path, the AFTER command, the results location and the one-instance note", () => {
    expect(run).toMatch(/Extracted Clade folder/);
    expect(run).toMatch(/type exactly this[\s\S]*run-after\.cmd/);
    expect(run).toMatch(/Results appear on the HOST/);
    expect(run).toMatch(/only ONE Sandbox window/);
    expect(run).toMatch(/Clipboard is disabled/);
  });
});

describe("the Windows Sandbox configurations", () => {
  it("are the three documented ones", () => {
    expect(configs.sort()).toEqual([
      "clade-offline.wsb",
      "clade-online-gpu.wsb",
      "clade-online.wsb",
    ]);
  });

  for (const name of configs) {
    describe(name, () => {
      const xml = read("qa/windows-sandbox", name);
      it("is a well-formed minimal configuration", () => {
        expect(xml.trim().startsWith("<Configuration>")).toBe(true);
        expect(xml.trim().endsWith("</Configuration>")).toBe(true);
        for (const tag of [
          "vGPU",
          "Networking",
          "AudioInput",
          "VideoInput",
          "PrinterRedirection",
          "ClipboardRedirection",
        ])
          expect(xml, tag).toMatch(new RegExp(`<${tag}>(Enable|Disable)</${tag}>`));
        expect(xml).toMatch(/<AudioInput>Disable</);
        expect(xml).toMatch(/<VideoInput>Disable</);
        expect(xml).toMatch(/<PrinterRedirection>Disable</);
        expect(xml).toMatch(/<ClipboardRedirection>Disable</);
      });
      it("shares exactly the read-only staging folder and one writable results folder", () => {
        const folders = [...xml.matchAll(/<MappedFolder>([\s\S]*?)<\/MappedFolder>/g)].map(
          (m) => m[1],
        );
        expect(folders.length).toBe(2);
        const [staging, results] = folders;
        expect(staging).toMatch(/<HostFolder>%CLADE_QA_STAGING%<\/HostFolder>/);
        expect(staging).toMatch(/<ReadOnly>true<\/ReadOnly>/);
        expect(results).toMatch(/<HostFolder>%CLADE_QA_RESULTS%<\/HostFolder>/);
        expect(results).toMatch(/<SandboxFolder>C:\\CladeQA\\results<\/SandboxFolder>/);
      });
      it("holds no developer path and never maps the repository, the profile or .minecraft", () => {
        expect(xml).not.toMatch(
          /C:\\Users\\|AppData|\.minecraft|\\Projects\\|%USERPROFILE%|%APPDATA%/i,
        );
      });
      it("runs only the staged in-sandbox script", () => {
        expect(xml).toMatch(
          /<Command>powershell\.exe [^<]*C:\\CladeQA\\in\\qa\\sandbox-run\.ps1<\/Command>/,
        );
      });
    });
  }

  it("differ exactly in network and virtual GPU", () => {
    const get = (n, tag) => read("qa/windows-sandbox", n).match(new RegExp(`<${tag}>(\\w+)<`))[1];
    expect([get("clade-offline.wsb", "Networking"), get("clade-offline.wsb", "vGPU")]).toEqual([
      "Disable",
      "Disable",
    ]);
    expect([get("clade-online.wsb", "Networking"), get("clade-online.wsb", "vGPU")]).toEqual([
      "Enable",
      "Disable",
    ]);
    expect([
      get("clade-online-gpu.wsb", "Networking"),
      get("clade-online-gpu.wsb", "vGPU"),
    ]).toEqual(["Enable", "Enable"]);
  });
});

describe("the runbook", () => {
  const doc = read("docs", "QA-WINDOWS.md");
  it("covers every required row", () => {
    for (const row of [
      "Clean first launch",
      "Relocation",
      "Spaces and Unicode path",
      "Another drive",
      "Bundled WebView2",
      "No Java installed",
      "Managed Java",
      "Modern vanilla",
      "Legacy Java 8",
      "Fabric",
      "Forge",
      "NeoForge",
      "Quilt",
      "Offline prepared launch",
      "Diagnostics export",
      "Malformed settings",
      "Damaged runtime",
      "Interrupted download",
      "Read-only folder",
      "Low disk",
      "Default and minimum size",
      "100% / 125% / 150%",
      "Transparency effects",
      "Second instance",
      "Crash",
      "Launcher killed while Minecraft runs",
      "No external writes",
    ])
      expect(doc, row).toContain(row);
  });
  it("keeps real sign-in a manual row whose credentials are never scripted", () => {
    expect(doc).toMatch(/B10 \| Real sign-in and launch/);
    expect(doc).toMatch(/typed by a person, never scripted/);
    expect(doc).not.toMatch(/BLOCKED \(AppID/);
  });
  it("records that Smart App Control blocks an unsigned Clade.exe and must not be bypassed", () => {
    expect(doc).toMatch(/Smart App Control blocks an unsigned `Clade\.exe`/);
    expect(doc).toMatch(/Do not disable Smart App Control/);
    expect(doc).toMatch(/code-signed \(not implemented yet\)/);
  });
  it("keeps the display, GPU and window-geometry safety rules and the one-Sandbox rule", () => {
    expect(doc).toMatch(/Never change the host's display, GPU, resolution, DPI or driver/);
    expect(doc).toMatch(/976x65535/);
    expect(doc).toMatch(/no window-geometry helper/);
    expect(doc).toMatch(/one instance at a time/);
  });
});
