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

What Microsoft documents for the Fixed Version Runtime (WebView2 distribution guide), and what that means for a
portable folder. None of it has been verified on those machines by this project yet (see the matrix in
section 9); this is a reading of the documentation, not a test result.

- **Windows 10, unpackaged Win32 app, Fixed Version 120 or later:** Microsoft documents that the runtime folder
  must grant read-and-execute, with inheritance, to the AppContainer groups `ALL APPLICATION PACKAGES`
  (S-1-15-2-1) and `ALL RESTRICTED APPLICATION PACKAGES` (S-1-15-2-2), for example
  `icacls <runtime folder> /grant *S-1-15-2-2:(OI)(CI)(RX)` and the same for `*S-1-15-2-1`. Without it the
  runtime may fail to start. Windows 11 is not described as needing this.
- **Network / UNC paths:** the Fixed Version runtime must not be assumed to work from a network location.
- **Extraction:** the CAB must be expanded with `expand`, not Explorer (the release script does).
- **Security software** that rewrites file ACLs can break the runtime.

| Row | Environment                                 | Expected behaviour of Clade's current design                                                                                                                                                                                                    |
| --- | ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A   | Windows 11, local NTFS                      | Supported by design; the primary target. To be confirmed in the clean VM.                                                                                                                                                                       |
| B   | Windows 11, removable drive (NTFS or exFAT) | No documented requirement beyond A. Expected to work, with slower I/O. To be confirmed.                                                                                                                                                         |
| C   | Windows 10, NTFS, user-owned folder         | An extracted folder inherits its parent's ACL, which normally lacks the AppContainer groups. **The documented requirement is not met by the package and Clade does not set it; expected to fail or be unreliable on Windows 10 until handled.** |
| D   | Windows 10, FAT32 / exFAT removable drive   | These file systems have no ACLs, so the documented permissions cannot be granted at all. Whether the runtime starts there is **not documented by Microsoft and unknown here**. Treat as unsupported until tested.                               |
| E   | UNC / network path                          | Not supported (see above).                                                                                                                                                                                                                      |

Conclusions, without a speculative workaround:

- **Windows 10 support likely needs an application change**: Clade would have to apply the two ACL grants to
  `runtimes/webview2` (at startup or in a first-run step). That writes ACLs, may need rights that a restricted
  account or a USB stick does not give, and changes the portable contract. It is **not implemented** and is a
  **release blocker for claiming Windows 10 support**. Until it is decided, the README and release notes must not
  promise Windows 10.
- **FAT/exFAT is a real incompatibility with the documented ACL requirement** on Windows 10. It is not a
  Clade-specific bug and Clade cannot fix it.
- The bundled-runtime contract is **not weakened**: a release still requires the bundled runtime and a
  `webview=Bundled` smoke test. A fallback to an installed runtime on Windows 10 would be a deliberate product
  decision, not part of this pipeline.
- A UNC path also contradicts the folder lock and atomic-write assumptions; refusing or warning about it would be a
  later application change.

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

Run on the downloaded draft ZIP before publishing, on a **clean Windows 11 virtual machine** with no Java and
no system-wide WebView2 assumed. Then repeat the start-up checks on every row of the portability matrix (section 2)
that the release claims to support; a row that was not tested must be listed as untested in the release notes.

| Matrix row                                                                      | Tested | Result |
| ------------------------------------------------------------------------------- | ------ | ------ |
| A. Windows 11, local NTFS                                                       | [ ]    |        |
| B. Windows 11, removable drive                                                  | [ ]    |        |
| C. Windows 10, NTFS, user-owned folder (check the AppContainer ACL requirement) | [ ]    |        |
| D. Windows 10, FAT32/exFAT removable drive                                      | [ ]    |        |
| E. UNC / network path (expected unsupported; confirm the failure is clear)      | [ ]    |        |

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
- Windows 10 is not claimed as supported: see "WebView2 Fixed Version: where it can and cannot run".
- No SBOM or build-provenance attestation is produced yet.
- The real tag workflow cannot be exercised without creating a tag and a draft release; its structure is verified by
  static tests, and its first real run is a manual validation step.
