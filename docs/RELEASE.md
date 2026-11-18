# Releasing Clade Launcher

This document is the operating procedure for producing a portable Windows release. It describes what the
tooling does, what a person must supply, how to abort, and what still has to be verified by hand. It is not
user documentation.

The first public build is a **Preview**: an unsigned portable ZIP. Nothing in this pipeline publishes
anything by itself. A tag creates a **draft** GitHub release; a person reviews it and publishes it, or deletes it.

## 1. Release model

One artifact: `Clade-Launcher-Portable.zip`, plus `Clade-Launcher-Portable.zip.sha256`.

```
Clade-Launcher/
├─ Clade.exe
├─ README.txt                      portable instructions (scripts/release/portable-readme.txt)
├─ LICENSE                         Apache License 2.0
├─ THIRD-PARTY-NOTICES.txt         generated from the lock files
├─ licenses/
│  └─ WebView2-Fixed-Version-License.txt
└─ runtimes/
   └─ webview2/                    the pinned WebView2 Fixed Version Runtime (msedgewebview2.exe, ...)
```

Everything else is created by Clade next to `Clade.exe` the first time it runs (`data/`, `instances/`,
`logs/`, `cache/`, `minecraft/`, `libraries/`, `assets/`, `runtimes/java/`). **None of those, and no user file
of any kind, may be in the package.** The validator enforces that (section 6).

## 2. What a release needs before it can run

| Input                                                                                                                   | Where                                                                                                       | Status                                                                                          |
| ----------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| A pinned WebView2 Fixed Version: exact version, download URL, SHA-256 of the CAB, who accepted the redistribution terms | `scripts/release/webview2-runtime.json`                                                                     | **Pinned (154.0.4258.53). Approval fields empty: a person must record who accepted the terms.** |
| A version that agrees everywhere, and a `## [X.Y.Z]` entry in `CHANGELOG.md`                                            | `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.lock`, `CHANGELOG.md` | Checked automatically                                                                           |
| Mojang's approval of the Microsoft application ID, and a real-account launch verified by hand                           | external                                                                                                    | **Done.** Approved; verified on a development PC.                                               |
| A completed clean-machine validation (section 9)                                                                        | external                                                                                                    | Not done yet                                                                                    |

### The WebView2 pin

`scripts/release/webview2-runtime.json` holds the one runtime a release may ship:

```json
{
  "version": "154.0.4258.53",
  "architecture": "x64",
  "downloadUrl": "<Microsoft's download URL of the x64 Fixed Version CAB>",
  "sha256": "<SHA-256 of that CAB>",
  "approvedBy": null,
  "approvedOn": null
}
```

Status: `version`, `downloadUrl` and `sha256` come from Microsoft's official WebView2 download page (the stable
Fixed Version, x64), and the SHA-256 was computed locally from the downloaded CAB, whose Authenticode signature
(Microsoft Corporation) and `msedgewebview2.exe` ProductVersion were verified. The runtime that happens to be
installed on a developer machine is never used as a pin.

**`approvedBy` and `approvedOn` are intentionally empty and are the remaining manual step.** The release
workflow refuses to run while they are empty (`webview2.mjs status`). A person must:

1. Read the redistribution terms (`scripts/licenses/WebView2-Fixed-Version-License.txt`, from Microsoft). They
   require that end users are bound by terms protecting Microsoft at least as much, that the runtime is obtained
   directly from Microsoft, and that the licence text ships with it (it does: `licenses/`).
2. Fill in `approvedBy` (who) and `approvedOn` (date, `YYYY-MM-DD`), and commit the file in its own change,
   reviewed like code.

The release workflow downloads the CAB from the pinned URL, compares its SHA-256 **before using it for anything**
and rejects any mismatch; the packaging script then requires a valid Microsoft Authenticode signature and the
pinned version on `msedgewebview2.exe`. Updating the runtime is the same change again, deliberately: the runtime
is not updated automatically (see Known limits). When updating, re-check Microsoft's page and re-download; never
edit the hash by hand.

Clade itself contains the contract that a portable release uses the bundled runtime: the smoke test requires the
log line `webview=Bundled` and WebView2 processes running from `runtimes/webview2`. (The application still has a
development fallback to an installed runtime when the bundled folder is absent; a package without the bundled
runtime cannot pass validation, so it cannot be released.)

### WebView2 Fixed Version: where it can and cannot run

What Microsoft documents for the Fixed Version Runtime (WebView2 distribution guide), and how Clade handles each
environment at startup. Clean-machine verification is tracked in the matrix in section 9.

- **Windows 10 (`build < 22000`), unpackaged Win32 app, Fixed Version 120 or later:** Microsoft documents that the
  runtime folder must grant inheritable read-and-execute permissions to the AppContainer SIDs
  `ALL APPLICATION PACKAGES` (`S-1-15-2-1`) and `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`). At startup,
  Clade checks the OS build (`RtlGetVersion`) and the volume flags (`GetVolumeInformationW` for
  `FILE_PERSISTENT_ACLS`). When `runtimes/webview2/` is on an ACL-capable volume (such as NTFS), Clade inspects its
  DACL through Win32 security APIs (`GetNamedSecurityInfoW`, `SetEntriesInAclW`, `SetNamedSecurityInfoW` — never
  spawning `icacls.exe`, and needing no administrator rights on a normal user-owned folder) and writes those
  inheritable RX grants only if they are missing. Windows 11 (`build >= 22000`) does not require this step.
- **Windows 10 on non-ACL volumes (`FAT32` / `exFAT`):** those file systems lack `FILE_PERSISTENT_ACLS`, so the
  AppContainer permissions cannot be stored on the volume. When `FILE_PERSISTENT_ACLS` is absent on Windows 10 (or if
  applying the ACL fails), Clade falls back to the PC's installed Evergreen WebView2 runtime if one is present, and
  otherwise still attempts the bundled runtime as a last resort.
- **Network / UNC paths:** Chromium's Fixed Version sandbox refuses to start renderer processes from SMB/UNC shares;
  Clade logs a warning and falls back to the system WebView2 runtime, and UNC paths remain unsupported as a portable
  root.
- **Extraction:** the CAB must be expanded with `expand`, not Explorer (the release script does).
- **Security software** that rewrites file ACLs can break the runtime.

| Row | Environment                                 | Expected behaviour of Clade's current design                                                                                                                                                                                               |
| --- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A   | Windows 11, local NTFS                      | Supported by design (`webview=Bundled`, no ACL grant needed). To be confirmed in the clean VM.                                                                                                                                             |
| B   | Windows 11, removable drive (NTFS or exFAT) | Supported by design (`webview=Bundled` on both NTFS and exFAT, with slower removable-drive I/O). To be confirmed.                                                                                                                          |
| C   | Windows 10, NTFS, user-owned folder         | Supported. `FILE_PERSISTENT_ACLS` is present; Clade grants the inheritable RX ACEs for `S-1-15-2-1` and `S-1-15-2-2` on `runtimes/webview2` automatically (idempotently, without elevation) and runs `webview=Bundled`. To be confirmed.   |
| D   | Windows 10, FAT32 / exFAT removable drive   | `FILE_PERSISTENT_ACLS` is absent, so Clade falls back to the PC's Evergreen WebView2 runtime when installed (`webview=System`) and tries the bundled runtime only if no Evergreen runtime exists. Recommend NTFS for clean Windows 10 PCs. |
| E   | UNC / network path                          | Unsupported for Fixed Version sandbox and portable folder locking/atomic writes; Clade logs a warning and falls back to the system WebView2 runtime.                                                                                       |

Design notes:

- **Windows 10 NTFS needs no installer or elevation**: the Win32 DACL check on `runtimes/webview2` runs in-process
  only on Windows 10 (`build < 22000`) when `FILE_PERSISTENT_ACLS` is set, touches nothing outside `runtimes/webview2`,
  and does nothing once both AppContainer RX ACEs are already present.
- **FAT32/exFAT on Windows 10 is a real filesystem limit**: persistent ACLs do not exist on those volumes, which Clade
  cannot change. Falling back to an installed Evergreen WebView2 runtime keeps exFAT USB drives working on ordinary
  Windows 10 PCs that already have Edge/WebView2, while NTFS remains recommended when carrying Clade to a clean
  Windows 10 machine that may have no WebView2 installed.
- **The bundled-runtime release contract is unchanged**: a release package must ship `runtimes/webview2` and pass the
  `webview=Bundled` smoke test on local NTFS.
- **UNC paths remain unsupported**: Clade warns in the log and avoids pointing the Fixed Version sandbox at a UNC
  share, and portable folder locking and atomic renames still require a local or removable drive.

## 3. Versions and tags

- The version is plain `X.Y.Z` and must be identical in `package.json`, `src-tauri/Cargo.toml`,
  `src-tauri/tauri.conf.json` and the `clade-launcher` entry of `src-tauri/Cargo.lock`
  (`node scripts/release/check-versions.mjs`, also run by normal CI).
- A release tag is `vX.Y.Z`, optionally followed by `-preview`, `-preview.N` or `-rc.N`. The `X.Y.Z` of the tag must
  equal the version in the files. The first release is expected to be `v0.1.0-preview` for version `0.1.0`
  (the executable's version resource is numeric, so the suffix lives only in the tag).
- A tagged release also requires a non-empty `## [X.Y.Z]` section in `CHANGELOG.md`; that section becomes the
  release notes.
- A tag with a suffix, and any `0.x` release, is created as a **pre-release**. Every release is created as a
  **draft**.
- The tagged commit must be reachable from `main`.

## 4. Procedure

1. On a branch, make the version bump (if any) and write the `CHANGELOG.md` entry for the version. Make sure the
   WebView2 pin is configured (section 2). Merge through a normal pull request; the required CI checks must pass.
2. Update local `main`, and check the dry run (section 7) on your machine.
3. Create the tag **on the merge commit on `main`** and push only that tag:
   `git tag -a v0.1.0-preview -m "Clade Launcher 0.1.0 Preview"` then `git push origin v0.1.0-preview`.
4. The `Release` workflow runs: tag, versions and changelog agree and the commit is on `main`; the pinned WebView2
   runtime is fetched and verified; the same gates as CI run (`pnpm check`, `cargo fmt`, `clippy`, `cargo test`,
   Playwright); the production Tauri build; notices; the ZIP; allowlist validation; the smoke test; then a **draft**
   release with the ZIP and its `.sha256` is created.
5. Review the draft: download the ZIP, verify the checksum, run the manual validation (section 9). Publish it by
   hand only when satisfied.
6. If anything is wrong, follow section 8.

## 5. Verifying a download

```powershell
(Get-FileHash .\Clade-Launcher-Portable.zip -Algorithm SHA256).Hash.ToLower()
Get-Content .\Clade-Launcher-Portable.zip.sha256
```

The two hashes must be identical. (`sha256sum --check Clade-Launcher-Portable.zip.sha256` also works.) The
checksum protects against a damaged or swapped download only as far as the page it came from is trusted; the
package is not code-signed.

## 6. What the tooling checks

All of it is in `scripts/release/` (Node, no extra dependencies) and covered by tests that need no network.

- **Allowlist validation** (`validate-package.mjs`): fails closed. Only the five top-level files, `licenses/*.txt|md`
  and `runtimes/webview2/**` are accepted. It rejects `.git`, `.github`, `src`, `src-tauri`, `node_modules`, `target`,
  test output, screenshots, `CLAUDE.md`, `.env`, logs, keys, backups, `.pdb`, nested ZIPs, `data/`, `instances/`,
  `logs/`, `cache/`, `minecraft/`, `libraries/`, `assets/`, worlds, `vault.bin`, `accounts.json`, `settings.json`,
  playtime files and anything else that is not named. It requires `Clade.exe`, `README.txt`, `LICENSE`,
  `THIRD-PARTY-NOTICES.txt`, the WebView2 licence and `msedgewebview2.exe`.
- **Content scans**: the shipped text files must contain no developer path and nothing that looks like a token or key.
  `Clade.exe` is read, never modified, and must not contain this machine's profile, source or Cargo folders (the build
  remaps those out), must carry the expected version, and must embed the release Content Security Policy.
- **Deterministic ZIP** (`package.mjs`): built from a clean staging folder that receives only the named inputs;
  entries sorted, fixed timestamps and attributes, so the same inputs give the same ZIP bytes. `Clade.exe` itself
  is built by the normal toolchain with a pinned compiler and `--locked`; bit-for-bit reproducibility of the
  executable across machines is not claimed.
- **Smoke test** (`scripts/smoke-test-package.ps1`): checksum file, validation, extraction into a folder with a
  space in its name, version, licences, no user data before the first start, then Clade is started: it must keep
  running, create its portable folders **beside** `Clade.exe` (a debug build would use `clade-root`), log
  `webview=Bundled`, run WebView2 from `runtimes/webview2`, and never touch a canary listener on the Vite
  development port 1420.

## 7. Local dry run

Without the approved WebView2 pin you can still exercise everything except the real runtime:

```powershell
pnpm install --frozen-lockfile
./scripts/package-portable.ps1 -WithoutWebView2 -OutDir dist-portable
./scripts/smoke-test-package.ps1 -Zip dist-portable/Clade-Launcher-Portable-NO-WEBVIEW2.zip -ExpectedVersion 0.1.0 -WithoutWebView2
```

That produces `Clade-Launcher-Portable-NO-WEBVIEW2.zip`: a differently named development package that uses the
PC's WebView2 and **fails the release validation by design**. A real package needs
`-WebView2Cab <the approved CAB>` and the pin (section 2). `dist-portable/` is git-ignored.

Tests: `pnpm exec vitest run scripts` (also part of `pnpm check`).

## 8. Aborting or rolling back

- **A run fails before the draft is created**: nothing was published. Fix the cause and either re-run the failed
  workflow or delete the tag and push a corrected one:
  `git push origin :refs/tags/v0.1.0-preview` and `git tag -d v0.1.0-preview`, then tag again on the right commit.
- **A draft exists but should not be released**: delete the draft release in GitHub (Releases), then delete the tag as
  above. A draft is not visible to the public.
- **A release was published and is wrong**: mark it as a draft again or delete it, delete the tag, and say so in the
  next release's notes. Do not replace files of a published release silently: a published checksum would stop
  matching.
- The tag workflow uses only the standard `GITHUB_TOKEN`; only the draft job has `contents: write`. No other secret
  exists, so nothing needs to be rotated after an abort.

## 9. Manual release validation (not automated)

Run on the downloaded draft ZIP before publishing, on **clean Windows 11 x64 and Windows 10 x64 virtual machines**
with no Java and no system-wide WebView2 assumed (plus an Evergreen WebView2 check for Row D on Windows 10 exFAT).
Repeat the start-up checks on every row of the portability matrix (section 2) that the release claims to support; a
row that was not tested must be listed as untested in the release notes.

| Matrix row                                                                                                      | Tested | Result |
| --------------------------------------------------------------------------------------------------------------- | ------ | ------ |
| A. Windows 11, local NTFS                                                                                       | [ ]    |        |
| B. Windows 11, removable drive (NTFS and exFAT)                                                                 | [ ]    |        |
| C. Windows 10, NTFS, user-owned folder (confirm automatic `S-1-15-2-1` / `S-1-15-2-2` RX ACL grant + `Bundled`) | [ ]    |        |
| D. Windows 10, FAT32/exFAT removable drive (confirm Evergreen WebView2 fallback when installed)                 | [ ]    |        |
| E. UNC / network path (unsupported; confirm warning log and system WebView2 fallback)                           | [ ]    |        |

Checks:

- [ ] Checksum matches (section 5); the ZIP extracts; `README.txt` reads correctly.
- [ ] Starts from a normal ASCII path (`D:\Clade-Launcher`); the folders appear beside `Clade.exe`.
- [ ] Starts from a path with spaces.
- [ ] Starts from a Cyrillic path and prepares and launches a **modern** Minecraft version (older Java-era versions
      are known to be less tolerant of non-ASCII paths).
- [ ] Folder moved after the first launch; everything still works.
- [ ] Folder on another drive; on a USB/external drive where available.
- [ ] A previously prepared profile launches offline.
- [ ] Low disk space: the preparation stops early with a clear message.
- [ ] Damaged `settings.json` (empty, truncated, a wrong value): Clade starts, the damaged file is kept as `.corrupt`.
- [ ] Damaged Java runtime file: "Verify files" repairs it.
- [ ] A second start while Clade is running brings the first window forward.
- [ ] Minimum window size 960×600 and the 100%, 125%, 150% and 200% display scalings, on 1080p, 1440p and 4K.
- [ ] The log shows the bundled WebView2; the folder has no files outside what Clade created.
- [ ] A real sign-in and launch with a Microsoft account, on the clean machine, from the release package.

## 10. Known limits

- The package is **not code-signed**; Windows SmartScreen and antivirus software may warn.
- The bundled WebView2 runtime is **not updated automatically**; a security update means a new pinned runtime and a
  new release.
- `THIRD-PARTY-NOTICES.txt` is generated from the lock files and the installed dependency sources, and the
  packaging script runs it with `--strict`: a redistributed package with no licence text fails the build. Packages
  that ship no licence file (`alloc-stdlib`, `webview2-com`, `webview2-com-sys`, `webview2-com-macros`) use the
  upstream repository's own licence file at the commit recorded in the published crate, and the statically linked
  Microsoft WebView2 loader uses the LICENSE.txt of Microsoft's own SDK package. Those texts are stored verbatim in
  `scripts/release/license-texts/`, hash-checked, and bound to the exact crate version they were verified for
  (`license-sources.json`); no copyright line is written by hand. Updating one of those crates means reviewing its
  entry again; an unreviewed version fails `--strict`. The notices also say which components (the WebView2
  runtime, the Rust standard library, the C runtime) are covered by their own terms.
- `LICENSE` is the canonical, unmodified Apache License 2.0. The `[yyyy] [name of copyright owner]` text in its
  appendix is part of the standard licence text (instructions for applying it), not a field to fill in, and it is
  not an open decision.
- On Windows 10, FAT32 and exFAT drives do not support persistent ACLs (`FILE_PERSISTENT_ACLS`), so Clade cannot
  grant the AppContainer permissions the bundled Fixed Version WebView2 runtime needs there; on those drives Clade
  falls back to an installed Evergreen WebView2 runtime when present (see "WebView2 Fixed Version: where it can and
  cannot run"). Removable drives meant for clean Windows 10 PCs without WebView2 should be formatted as NTFS.
- No SBOM or build-provenance attestation is produced yet.
- The real tag workflow cannot be exercised without creating a tag and a draft release; its structure is verified by
  static tests, and its first real run is a manual validation step.
