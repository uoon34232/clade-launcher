# Clade Launcher: technical documentation

This document explains how Clade Launcher is built and which rules its code follows. It is written for contributors and for anyone who wants to check the claims made in the [README](../README.md). For reporting vulnerabilities see [SECURITY.md](../SECURITY.md); for contribution rules see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Pre-release status.** Clade Launcher is currently pre-release. Mojang has approved the Microsoft application ID, and sign-in and launching have been verified with a real account on a development PC. The executable is not code-signed, and the clean-machine validation of a release package (including the bundled WebView2 runtime) has not been done yet.

## Contents

1. [Overview](#1-overview)
2. [Building from source](#2-building-from-source)
3. [Architecture](#3-architecture)
4. [Portable filesystem architecture](#4-portable-filesystem-architecture)
5. [Profiles and isolation](#5-profiles-and-isolation)
6. [Version catalog and profile creation](#6-version-catalog-and-profile-creation)
7. [Preparing a profile](#7-preparing-a-profile)
8. [Loaders: Fabric, Forge, NeoForge, Quilt](#8-loaders-fabric-forge-neoforge-quilt)
9. [Downloads, verification and repair](#9-downloads-verification-and-repair)
10. [Microsoft authentication](#10-microsoft-authentication)
11. [The encrypted credential vault](#11-the-encrypted-credential-vault)
12. [Accounts versus profiles](#12-accounts-versus-profiles)
13. [Launching the game](#13-launching-the-game)
14. [Playtime tracking](#14-playtime-tracking)
15. [Localization](#15-localization)
16. [Security and portability invariants](#16-security-and-portability-invariants)
17. [Automated guards and tests](#17-automated-guards-and-tests)
18. [Release packaging](#18-release-packaging)
19. [Known limitations and current status](#19-known-limitations-and-current-status)
20. [Settings, first-run introduction and diagnostics](#20-settings-first-run-introduction-and-diagnostics)

## 1. Overview

Clade is a Windows desktop launcher for Minecraft: Java Edition on 64-bit Windows 10 and Windows 11 (`x64`). It is built with **Tauri 2**: a Rust backend does all privileged work, and a **React + TypeScript + Vite** frontend (styled with Tailwind CSS) is a thin view layer on top of it.

Two requirements shape most of the design:

- **Full portability.** Clade must never depend on anything installed on the machine (Java, WebView2, a global `.minecraft`) and must never intentionally store anything outside its own folder.
- **Vanilla first, not a content platform.** Clade prepares Minecraft and, optionally, one mod loader. It never browses, downloads or manages mods, modpacks, shaders or resource packs.

## 2. Building from source

You need:

- Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools;
- Node.js and pnpm (the exact pnpm version is pinned in `package.json`);
- the Microsoft Edge WebView2 Runtime for development (present on current Windows 10/11).

```bash
pnpm install        # frontend dependencies
pnpm tauri dev      # builds the Rust side (first build downloads crates) and starts Clade
```

Checks that must pass before a change is proposed (the same checks run in CI on every pull request):

```bash
pnpm check                              # typecheck + eslint + vitest
pnpm build                              # typecheck + production build of the frontend
pnpm test:e2e                           # browser end-to-end tests (needs Chromium, see below)
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The end-to-end tests run in Playwright's Chromium. Install it once with the standard command, then run the suite:

```bash
pnpm exec playwright install chromium
pnpm test:e2e
```

They start the Vite dev server themselves, need no Tauri window, no network and no account, and write only to the git-ignored `test-results/` and `playwright-report/` folders.

During development Clade keeps its data in `src-tauri/target/debug/clade-root`, never in your user profile. Do not use `pnpm tauri build`: installers are disabled on purpose (see [Release packaging](#18-release-packaging)).

**What is in Git, and what is not.** Git holds the source, tests, lockfiles (`pnpm-lock.yaml`, `src-tauri/Cargo.lock`), configuration, localization catalogs, application icons, the packaging script and the documentation. These are fetched or generated when needed and are never committed: `node_modules/`, `src-tauri/target/`, Minecraft versions/libraries/assets, Java runtimes and loaders (downloaded at run time from official hosts), the WebView2 Fixed Version Runtime (downloaded from Microsoft only when packaging a release), and any Clade data folder, vault, accounts, logs or release archives.

## 3. Architecture

```
src-tauri/src/    Rust backend: every privileged operation
src-tauri/tests/  guard tests (portability, product rules, icon)
src/              React frontend: views, localization, a typed IPC layer
src/locales/      English and Russian catalogs
docs/             documentation and published media
scripts/          release packaging
```

**Rust backend modules** (by responsibility):

| Area               | Modules                                                                                                                                              |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Storage and safety | `portable/` (root discovery, validated relative paths, atomic writes, hashing, folder lock), `safe_archive`, `download_policy`, `net`, `coded_error` |
| Settings and state | `settings`, `ui_settings`, `startup`, `startup_text`, `webview`, `glass`, `support_cmd`, `update_check`                                              |
| Profiles           | `instances`, `versions`, `profiles`, `mc_catalog`, `loader_options`, `loaders`                                                                       |
| Preparation        | `prepare`, `prepare_cmd`, `download`, `mc_meta`, `java_install`, `runtimes`, `natives`, `loader_prepare`, `loader_install`                           |
| Accounts           | `vault`, `accounts`, `accounts_api`, `auth`, `ms_auth`, `profile_service`                                                                            |
| Launching          | `launch_args`, `launch_cmd`, `game_session`, `game_embed`, `system`, `playtime`                                                                      |

**Frontend rules.** The frontend calls typed Tauri commands and listens to events. It contains no business logic and no secrets, and it never talks to Mojang or Microsoft itself. All native calls go through one typed boundary (`src/lib/ipc.ts`, with validating bridges per feature under `src/lib/`). Everything crossing that boundary is validated at the edge, and there are no fake profiles or accounts in production code.

**Commands and events.** Commands are small, validated and return a serializable stable-code error. Long tasks (downloads, preparation, launch) run as tokio tasks, report progress through events, and are cancellable.

## 4. Portable filesystem architecture

The **root** is the folder containing `Clade.exe`, found at run time from the executable's path. Debug builds use `target/debug/clade-root` so development never pollutes the repository.

```
Clade-Launcher/
├─ Clade.exe
├─ data/        settings.json, accounts.json, vault.bin, clade.lock, webview2/ profile,
│               retired-vaults/ (only after "Start fresh")
├─ instances/   one folder per profile
├─ minecraft/   shared version files
├─ libraries/   shared libraries
├─ assets/      shared game assets
├─ runtimes/    java/ and webview2/
├─ cache/       scratch, catalog and skin (avatar) caches
└─ logs/
```

Rules that keep it portable:

- **No absolute paths are ever stored.** Persisted paths are `RelPath` values: validated, `/`-separated and relative to the root. They reject absolute paths, drive letters, UNC paths, `..`, backslashes, `:` (alternate data streams), reserved device names, trailing dots or spaces and control characters. Moving the folder therefore cannot break a saved reference.
- **Nothing in `%APPDATA%`, `%LOCALAPPDATA%`, Documents or a global `.minecraft`.** The WebView2 profile is redirected into `data/webview2`.
- **Managed Java only.** Java is resolved from the launcher's own runtimes and re-verified; `PATH`, `JAVA_HOME`, the registry and Program Files are never consulted.
- **Atomic, verified writes.** Files are written to a temporary file, flushed and renamed. Downloads are verified before they are moved into place.
- **One user of the folder at a time.** An OS file lock on `data/clade.lock` stops two PCs from corrupting the same folder; it is released automatically if the process dies or the stick is pulled.
- **No administrator rights.** The manifest requests `asInvoker`, and the executable is never named setup, install or update.
- **Versioned formats.** Every on-disk format carries a version, and a folder newer than the build is refused instead of silently rewritten.
- **No required redistributables.** The release build links the C runtime statically; the WebView2 loader is static.

**Launcher state versus WebView state.** Launcher-owned state (sound settings, language, last phrase, last profile) lives in `data/settings.json`, written only by the Rust settings service, sanitized on load and saved atomically. The WebView profile is a disposable browser cache, so the frontend never uses `localStorage`, `sessionStorage`, `indexedDB`, cookies or the Cache API for launcher state (a test scans the frontend to enforce this).

**WebView2.** Release builds ship the Fixed Version runtime in `runtimes/webview2/` and select the runtime before any window exists. On Windows 11 (`RtlGetVersion` `build >= 22000`), Clade points `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` straight at `runtimes/webview2/`. On Windows 10 (`build < 22000`), unpackaged Win32 apps using a Fixed Version runtime must grant inheritable read-and-execute permissions on `runtimes/webview2/` to the AppContainer SIDs `ALL APPLICATION PACKAGES` (`S-1-15-2-1`) and `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`). Clade checks the volume flags with `GetVolumeInformationW`: when `FILE_PERSISTENT_ACLS` is present (such as on NTFS), it inspects the folder's DACL through Win32 security APIs (`GetNamedSecurityInfoW`, `SetEntriesInAclW`, `SetNamedSecurityInfoW` — never spawning `icacls.exe`, and needing no administrator rights on a normal user-owned folder) and writes those grants only if they are missing. On Windows 10 volumes without `FILE_PERSISTENT_ACLS` (`FAT32` and `exFAT`), or if applying the ACL fails, Clade falls back to the PC's installed Evergreen WebView2 runtime when one is present, and otherwise still tries the bundled runtime as a last resort. Running from a UNC share (`\\server\share`) also logs a warning and falls back to the system runtime because Chromium's Fixed Version sandbox refuses SMB shares. If neither the bundled nor an installed runtime loads, Clade shows a native message and exits; it never downloads or runs an installer.

## 5. Profiles and isolation

A profile (internally an _instance_) is created with two required answers: **Vanilla or Modded**, and **which Minecraft version**. There is no default for either. A profile's type is fixed at creation.

```
instances/<id>/
├─ instance.json   manifest (the id equals the folder name and never changes)
├─ game/           the game directory and working directory
├─ natives/        disposable, rebuilt on every launch
├─ home/           stands in for the user profile while the game runs
└─ playtime.json
```

- **Vanilla** profiles install only the selected Minecraft version and what it needs. `game/` starts empty.
- **Modded** profiles also select one loader (Fabric, Forge, NeoForge or Quilt) and create empty `mods`, `config`, `resourcepacks`, `shaderpacks` and `saves` folders inside `game/`. Clade never fills, lists, searches, enables, disables or updates their content.
- **Per-profile settings and folder actions.** A profile can be **renamed** at any time: Rust validates the new display name (including case-insensitive uniqueness against existing profiles and any unreadable folders in `instances/`) and updates `name` in `instance.json` while keeping the folder name and `id` unchanged. A profile's **Minecraft version** (and, for Modded profiles, its compatible loader version for the same loader) can also be changed in place after checking the official catalogs: user files inside `game/` stay untouched, disposable `natives/` are cleared, and `setup` resets to `Pending` so the new version is prepared before the next launch. A profile can be **duplicated** into a new isolated folder (`Name (2)`, `Name (3)`, ...), copying `game/` without following links while keeping shared-store `Ready` status and starting the copy's own playtime at zero. Each profile can also store its own **memory override** (`memory_mb` in `instance.json`, in MiB on the same portable step grid as the global setting and clamped at launch to the current PC's safe ceiling, or `null` to follow the global / automatic setting), an optional initial **game window resolution** (`resolution` in `instance.json`, `320×240..=7680×4320`, activating Mojang's `has_custom_resolution` launch feature rule, exposed in Advanced mode), and validated **extra JVM arguments** (`extra_jvm_args`, restricted to an allowlist such as `-XX:`, `-D`, `-Xss`, `-Xmn`, `--add-opens=` while forbidding heap, classpath, agent, and path overrides, exposed in Advanced mode). Clade can also **open the profile's `game/` folder** directly from Rust (verifying the profile is a real folder inside `instances/` and not a link, creating `game/` if needed, with no generic opener permission exposed to the frontend).
- **Unreadable profile folders never vanish silently.** When a folder under `instances/` has a missing, truncated or malformed `instance.json`, Clade leaves all of its files untouched, omits it from the playable profile list so healthy profiles keep working, writes a warning to the log and the diagnostics summary, and returns the unreadable folder names with `list_profiles` so the interface shows a non-destructive warning.
- **Removing a profile never deletes files.** _Remove from Clade_ (profile menu, after a confirmation) takes the selected profile out of the list and moves its whole folder, unchanged, to `instances/.removed/<profile-id>-<unix-time>-<random>/` (one atomic rename on the same drive: worlds, mods, config, screenshots, options, game logs, `home/`, `playtime.json` and any unknown file stay exactly as they were). `.removed` is not a valid profile id, and the profile scanner skips it, so a retained profile is never listed, launched or reported as a problem. The Rust command refuses a profile whose game is starting or running and one that is being prepared or verified (they share the per-profile claim), refuses anything that is not a readable Clade profile or is a link, and checks all of this before moving anything. If the rename fails (for example a file inside is open) the profile stays where it was and the error is shown; there is deliberately no copy-then-delete fallback. The selection moves to the next remaining profile, or to the empty state after the last one. When `instances/.removed/` contains valid retained profiles, Clade offers **Removed profiles** (in the profile menu and in the empty-profile notice) to restore any removed profile back into `instances/` in one click (disambiguating duplicate names as `Name (2)` while preserving all user files) or open `instances/.removed/` in File Explorer so you can delete folders manually if you want to free disk space. Clade itself still never deletes profile files: the store's recursive delete exists only in test builds.
- **Isolation is absolute.** Everything a player or a mod can modify lives under that profile's `game/`. There is intentionally no shared mods, config, saves, resource-pack or shader folder, and no links or junctions between profiles.
- **What may be shared** (`minecraft/`, `libraries/`, `assets/`, `runtimes/`): only immutable, hash-verified files published by Mojang or the loader project.
- Minecraft versions are treated as opaque, path-safe ids. Nothing in the launcher's own logic hard-codes or special-cases a Minecraft version.
- Deleting a profile removes only its own folder.
- JVM arguments and memory are user input: validated, shown before launch and passed as separate arguments, never through a shell.

## 6. Version catalog and profile creation

The frontend never fetches Minecraft or loader metadata and never edits profile manifests directly. Rust handles discovery and profile management through `profiles`: listing profiles alongside any unreadable-folder warnings (`list_profiles`), selecting a profile (`select_profile`), querying the version and loader catalogs (`get_version_catalog`, `get_loader_options`), creating a profile (`create_profile`), renaming a profile, setting or clearing a profile's memory override, and opening a profile's `game/` folder.

- **Network layer.** A `Fetcher` trait with an HTTPS-only implementation that re-checks every redirect host against an allow-list, caps response size, uses short connect and total timeouts and an anonymous `Clade/<version>` User-Agent. Tests inject a fake fetcher.
- **Minecraft catalog.** Only Mojang's `version_manifest_v2.json`. Releases, snapshots, old betas and old alphas are kept; releases are the default view, never the only one.
- **Cache.** `cache/catalogs/` (inside the root, atomic writes). The manifest is fresh for one hour. After that a refresh is attempted, and on network failure the last valid copy is used and flagged stale. A response is saved only after it parses and passes a plausibility check, so a truncated response cannot destroy a good cache.
- **Loader catalogs.** Forge and NeoForge publish one catalog for all Minecraft versions, so the raw catalog is cached and filtered per request. Fabric and Quilt answer per version and are cached per version. An already-filtered result is never cached under a name shared by several versions.
- **Creation is verified server-side.** The UI is not trusted: Rust checks the name, that the version is in the catalog, that the loader has a compatible version, and that a pinned loader version was actually offered. It never guesses.

## 7. Preparing a profile

`prepare` turns a pending profile into a ready one without starting the game. The manifest only becomes `Ready` after the final check passes, so a crash cannot leave a half-prepared "ready" profile. `Preparing` and `Failed` are runtime states.

Order of work:

1. Read the version JSON (found through the catalog entry's URL and SHA-1, verified and saved).
2. Read the asset index and plan Java and the loader.
3. **Plan totals before any big download starts**, then check free disk space (the bytes still to download plus headroom).
4. Install Java, the client jar, libraries, assets, natives and the loader.
5. Run a final file check and mark the profile ready.

**What is needed** is decided only by Mojang's metadata: libraries follow Mojang `rules` for Windows x64, with every launcher feature switched off, and anything not understood counts as "does not apply" rather than being guessed. Old and modern library forms, native libraries, and the legacy, virtual and `map_to_resources` asset layouts are all supported.

**Java.** Mojang's runtime index (`windows-x64`) lists a component manifest, which is checked against its SHA-1. Every file is downloaded and verified into a staging folder, then the folder is renamed into `runtimes/java/` in one step. A runtime is reused while `bin/java.exe` still matches its recorded SHA-256; that is the only check on an ordinary launch, because hashing a whole runtime every time would be slow. The install also records every file of the runtime with the SHA-1 and size from Mojang's manifest (`runtime-files.json`). The explicit **Verify files** action uses that list to check every file of the profile's runtime, offline, and fetches again the ones that are missing or damaged. A runtime installed before the list existed is checked against Mojang's current manifest when it describes exactly the same version; otherwise only `java.exe` is covered, and nothing is invented for files no manifest lists. A version without a stated Java requirement uses the legacy runtime.

**Natives** are unpacked by the safe archive extractor into scratch space, filtered, and then put in place of `instances/<id>/natives/` (the old folder is removed, then the new one is renamed in). Because that replaces files a running game has loaded, a profile is never prepared or verified while its game runs (see below).

**Progress** is reported as stages with real file and byte counts, or as indeterminate while nothing can be measured (for example while a Forge installer runs).

**One operation per profile.** Preparing or verifying a profile and launching it exclude each other. A shared registry (`profile_lock`) claims the profile atomically (one lock, one check-and-insert) for the whole preparation, or from the first Play click until the game has ended. A profile whose game is starting or running cannot be prepared or verified (`already_running`), and a profile that is being prepared cannot be launched (`already_preparing`). The checks are in the Rust commands, not only in the UI; the dock additionally disables "Verify files" while the selected profile's game runs. Other profiles are unaffected: one preparation still runs at a time overall, but a running game never blocks preparing another profile.

## 8. Loaders: Fabric, Forge, NeoForge, Quilt

Clade may download and install **only the loader the user selected, from that loader's official source**. A loader is infrastructure needed to run a modded profile, not content.

- **Official sources only.** Each URL is built from validated tokens and checked against a loader-specific host list that is stricter than the global allow-list: Fabric and Quilt use their meta services and Maven hosts; Forge and NeoForge use their official file and Maven hosts. No mirrors and no third-party download sites.
- **Compatible versions.** The catalog yields versions compatible with the chosen Minecraft version, newest first, labelled Recommended, Stable, Latest or Prerelease. An empty list means "no compatible loader"; the UI says so and Clade never guesses. `Auto` picks recommended, then stable, then latest, then prerelease.
- **Fabric and Quilt** need no installer process. The official profile JSON is saved and every library is verified with the hash and size inside it. Library coordinates go through a strict Maven-path builder that can only produce paths under `libraries/`.
- **Forge and NeoForge** use the official installer jar. It is verified against the checksum published beside it and only then run, with the managed Java, inside a sandbox (below). Residual trust: that checksum comes from the same host as the jar, so it catches corruption and tampering but not a compromised loader project, and the installer makes its own library downloads, which Clade cannot restrict.
- **Loader output** (profile JSON, libraries) is immutable and goes into the shared stores. A loader is never installed into a profile's `game/` folder.

**Installer sandbox.** The Forge and NeoForge installers would normally write to `%APPDATA%\.minecraft`. Instead, each installation gets a fresh staging area under `cache/loader-install/<id>/` with its own stand-in user profile, a stand-in `AppData\Roaming\.minecraft` target folder (so both `--installClient <target>` and older Forge installers that read `%APPDATA%\.minecraft` stay inside the stage) and a stand-in TEMP directory. The installer process runs inside a kill-on-close Windows Job Object with a **completely cleared environment** plus a few explicit variables pointing into the stage, never as administrator and never with a mirror option; when the target Minecraft version uses a legacy Java 8 runtime, a modern managed Java 21 runtime executes the installer so TLS handshakes against official Maven hosts succeed. The command line is verified twice (also right before spawning) and refused if any program, argument or environment value is an absolute path outside the root. A tripwire watches the real `.minecraft` metadata and fails the installation loudly if anything changes. Only verified `libraries/` and `versions/` are then merged into the shared stores, with links refused and damaged files quarantined rather than deadlocking repair.

## 9. Downloads, verification and repair

- **HTTPS only**, through the allow-list in `download_policy`: Mojang, Microsoft sign-in, and loader hosts. Content platforms are never on it, and tests assert that. A separate single-host policy (`UPDATE_CHECK_HOSTS = ["api.github.com"]`, no redirects, never used for game or loader downloads and never called in the background) is used only when the player clicks **Check for updates** in Settings (`update_check.rs`), and **Open release** constructs only canonical `https://github.com/uoon34232/clade-launcher/releases` URLs in Rust.
- Every job carries a mandatory SHA-1 or SHA-256 and usually a size. Existing verified files are reused with no network.
- Bytes stream into a `.part` file and resume with `Range` where the server supports it. A part is never trusted until it verifies in full. When the size is known it is a hard ceiling for the whole part (bytes kept from an earlier attempt count): a server that announces more, or keeps sending past it, is stopped at once and the part is discarded. Finalisation is an atomic rename.
- Transient failures are retried with backoff; `404` and similar fail at once; a good file is never replaced by an unverified one. Concurrency is bounded (default 8).
- Archives (natives, Java, installers) go through `safe_archive`, which guards against path traversal, absolute and drive paths, links, case collisions and decompression bombs.
- **Offline check.** For a ready profile, Clade checks only that each required file exists with its exact size, with no hashing and no network, so a prepared profile is confirmed instantly while offline. Same-size corruption needs the full **Verify files** action, which re-hashes everything.
- **Repair.** A refused launch (`needs_repair`) opens the normal preparation dialog, which repairs through the same pipeline. Libraries that publish neither size nor hash are verified once through a `.sha1` published beside them, and their size is recorded so truncation is noticed offline later.

## 10. Microsoft authentication

Sign-in follows the flow documented by Microsoft and the community Minecraft documentation; Mojang does not publish a formal specification for the Minecraft-side steps.

1. **Microsoft OAuth 2.0 authorization code + PKCE** (S256) in the **system browser**, against the `consumers` tenant with scope `XboxLive.signin offline_access`. Clade never shows a login form and never sees a password. The public client ID is a constant in the source (it is not a secret; no client secret exists or is used).
2. **Xbox Live** (`user.auth.xboxlive.com`) exchanges the Microsoft token for an Xbox user token.
3. **XSTS** (`xsts.auth.xboxlive.com`) issues a token for the Minecraft services relying party.
4. **Minecraft services** (`login_with_xbox`) returns the Minecraft access token.
5. `GET /minecraft/profile` returns the Java profile. An account is launch-ready only when this returns a valid name and UUID. A missing profile is reported as "no Java profile", and a separate entitlements check only decides which message to show.

**Redirect listener.** A loopback listener on `127.0.0.1` with a random port accepts exactly one valid request: `GET /`, a matching `Host` header (a DNS-rebinding guard) and a `state` value compared in constant time. Anything else gets a 404 and is ignored. It enforces read, size and overall time limits, and cancelling stops the listener and any in-flight requests.

**Secrets handling.** Tokens are held in a zeroizing `Secret` type with a redacted `Debug`. Errors and logs carry stable codes, never response bodies, tokens or headers.

**Token lifecycle.** A cached Minecraft token is used while it has more than five minutes left. Otherwise the Microsoft token is refreshed and the chain repeated, and the rotated refresh token is written to the vault as soon as Microsoft returns it, before the Xbox and Minecraft steps, so a later failure there cannot lose it (the short-lived Minecraft token is only stored once the whole chain has succeeded). Revoked or expired credentials mark the account as _needs sign-in again_ and keep the account and its data; network failures change nothing. Accounts are never deleted because credentials expired. Signing in again must return the same Minecraft UUID or it is refused.

**Error codes.** The UI distinguishes `cancelled`, `timed_out`, `browser_unavailable`, `offline`, `oauth`, `needs_sign_in`, `no_xbox_account`, `xbox_region`, `adult_verification`, `child_account`, `xbox_banned`, `xsts`, `approval_required`, `no_java_profile`, `bad_response`, `vault_locked`, `storage` and `wrong_account`.

**Skin and cape changes** go through Mojang's authenticated Minecraft services only (`profile_service.rs`, commands in `appearance_cmd.rs`), so one profile looks the same in the game, the official launcher and account pages. There are no launcher-local cosmetic overrides and no third-party skin services. The account dialog can upload a 64x64 or 64x32 PNG as the skin (Classic or Slim), restore the default skin, equip one of the capes the account owns and hide the cape; capes cannot be uploaded. Every call names one account, which must exist and be `Ready` (another account is never substituted), takes its Minecraft token from the vault through the normal refresh path, and sends it only to `api.minecraftservices.com`, never following a redirect and never retrying a change. The token stays in Rust: the frontend sends an account id, a model, PNG bytes or an owned cape id. The backend validates the PNG (size bound, signature, dimensions) and re-reads the profile to confirm the account owns a cape before equipping it; after a change the cached avatar is refreshed (best effort, a failed texture download never undoes an accepted change). Mojang publishes no formal specification for these endpoints, so they are kept behind the `ProfileService` trait. This is verified with test doubles only; it has not yet been exercised against a real account.

## 11. The encrypted credential vault

Refresh tokens and the Minecraft access token are stored only in `data/vault.bin`. Clade never uses the OS credential store, because that is bound to one machine and user and would break portability.

- A master passphrase is stretched with **Argon2id** (64 MiB memory, 3 passes) into a 32-byte key. Floors are enforced on read, and ceilings protect against hostile files.
- Secrets are encrypted with **XChaCha20-Poly1305** using a fresh random nonce per save, with the whole header authenticated.
- The passphrase is never stored. A wrong passphrase and a tampered file are indistinguishable by design. Keys and secrets are zeroized on lock.
- Writes are atomic and keep one `.bak` generation.
- First sign-in asks for a new passphrase (eight characters or more, repeated, with a clear "cannot be recovered" warning). Later sessions unlock when a sign-in or refresh is needed. The vault is unlocked **before** the browser opens, so a finished sign-in can always be saved. The passphrase is sent to Rust once and never kept in the frontend.
- `accounts.json` holds metadata only (internal id, Minecraft UUID, profile name, status, timestamps), never tokens. It stays readable while the vault is locked.
- **Forgotten passphrase: "Start fresh"** (`vault_recovery.rs`, the quiet "Forgot passphrase?" link in the sign-in unlock step, the launch "unlock to play" dialog and the appearance unlock). The passphrase cannot be recovered, so this archives the old vault instead of opening it. It is allowed only while the vault is locked, missing or unreadable, never while it is unlocked, and never while a sign-in or a skin or cape change is under way or a game is _starting_ (a game that is already running, and a profile being prepared, do not block it). It uses the existing gates in the existing order: vault, then accounts. Steps: (1) `accounts.json` must be readable, a damaged file stops everything before any file moves; (2) `vault.bin.bak`, any leftover `vault.bin.<12 hex digits>.tmp` (exactly the name an interrupted save leaves, nothing broader) and then `vault.bin` are **renamed** (never deleted) into one new folder `data/retired-vaults/<unix-time>-<random>/` on the same drive; if a rename fails, the files already moved are renamed back, the active vault stays usable and the error is `reset_failed`; no folder is made when no credential file exists; (3) every account in `accounts.json` is marked `NeedsReauth(Unknown)` with the normal atomic write, keeping its id, UUID, name, active flag and other metadata. It is not one atomic step, so it is built to be repeated: each step skips what is already done, and if step 3 fails the error is `accounts_not_updated` and running it again only repeats step 3. No new vault is created: the next sign-in asks for a new passphrase. Profiles, worlds, playtime, settings, Minecraft files and the skin cache are never touched. The archived vault still opens with its old passphrase; Microsoft sessions are **not** revoked remotely; archived vaults in `data/retired-vaults/` can be deleted explicitly from Settings -> Support (with confirmation, shown only when archived vaults exist; `support_cmd::clear_retired_vaults` deletes only verified `<unix-time>-<hex>/` vault archives under the vault lock and never touches the active `vault.bin`). An unlock or create that was already in flight cannot install its vault afterwards either (an epoch counter, checked under the vault lock). A refresh that was already under way cannot write into the new vault: `ensure_fresh` remembers the vault's generation (its random salt) before the network steps and refuses to write if it changed or the vault was locked meanwhile. A vault file that is simply missing is reported as `vault_missing`, and Start fresh repairs that state (it only marks the accounts, and archives a stale `vault.bin.bak`).
- Removing an account while the vault is locked leaves its secrets until the next unlock, which then removes the entries that belong to no account. That cleanup deletes credentials, so it only runs against an account list that is known to be authoritative: `accounts.json` must exist and parse. A missing file (a copied `vault.bin`, a restored backup) or a damaged one leaves the vault exactly as it is; a file that parses and really lists no accounts is a genuine empty list.
- Locks are always taken in one order: the vault first, then the accounts file gate. Removing an account never holds the gate while it touches the vault.

Limit: a stolen copy of the folder allows unlimited offline guessing. Argon2id slows that down but cannot protect a weak passphrase.

## 12. Accounts versus profiles

An account is **never** bound to a profile. Profiles define the version, the loader and the files; accounts define the Microsoft/Minecraft identity. Neither a profile manifest, any profile file nor playtime references an account, and selecting or removing an account changes none of them (tests assert this byte for byte). Only the selected account is persisted, as a convenience, in `accounts.json`.

The same Minecraft UUID never creates a second entry. The newest sign-in becomes the selected account. Removing an account also removes its vault secrets and its cached avatar.

**Avatar.** The head shown for an account comes from its own current skin. The profile Minecraft services return at sign-in and at every credential refresh lists the ACTIVE skin; its public address is checked (exactly `https://textures.minecraft.net/texture/<hex id>`, the one Mojang texture host, no query or credentials) and the PNG is downloaded through the normal allow-listed client and stored in `cache/skins/` (`<uuid>.want` holds only the validated texture id; `<uuid>.png` and `<uuid>.have` the texture and the id it came from: public ids only, no address, no token). No third-party skin service and no web page is used. The backend sends the texture to the interface as an inline PNG; the interface cuts the 8x8 face and the 8x8 outer hat layer out of it (scaled for high-resolution skins) and shows them nearest-neighbour, never smoothed. A changed skin replaces the cache atomically; a download that fails keeps the previous file and is retried. The account list never waits for the network: `list_accounts` answers from local files at once and, when a skin is waiting, fetches all waiting skins together in the background (one round at a time, one overall time limit however many accounts there are) and then sends `accounts-changed`, on which the window lists again. Sign-in and a credential refresh wait for it at most about three seconds. The profile (and so a new skin) is read only when credentials are obtained: at sign-in, when a launch has to renew them, or on an explicit refresh; the avatar then updates at the end of that launch or at the next start, whichever comes first, without signing in again. A missing, offline, malformed or unsupported texture never affects sign-in or the account menu: the neutral face is drawn instead.

**Account required for game files.** Anything that downloads or re-checks game files, namely setting a profile up (Minecraft, libraries, assets, managed Java and loader files), repairing it and Verify files, and launching, needs the ACTIVE account to be `Ready` (not missing, signed out or waiting for a new sign-in; another ready account is never used instead). The backend enforces it: `prepare_profile` answers `account_required` before it claims the profile or touches the network (launching keeps its own `no_account` and `needs_sign_in` reasons). Looking up versions and loaders for the Create profile dialog is only metadata and stays available, as do onboarding, Settings and creating, selecting, editing and deleting profiles. In the dock, the primary button reads "Sign in to continue" instead of Set up or Play while the active account cannot play and opens the normal sign-in (the same account for "sign in again"); Verify files is unavailable then. It updates as soon as a sign-in succeeds.

## 13. Launching the game

**Before launch** (`plan_launch`, which starts nothing and is fully unit-tested):

1. the profile must be ready;
2. a selected, launch-ready account must exist;
3. the lightweight integrity check must pass;
4. the profile's own verified Java must be present;
5. fresh credentials are obtained. An expired token that cannot be refreshed stops the launch.

There is **no offline, guest or cracked mode**, and none must be added.

**Command construction** (`launch_args`). The version chain (`inheritsFrom`) is resolved from the saved metadata: loader libraries first, duplicates keeping the loader's copy, the base's arguments followed by the loader's, the loader's main class, and the base Minecraft jar closing the classpath. Modern argument lists use Mojang's rules; older versions use `minecraftArguments` plus the default JVM arguments the official launcher always added; the oldest use positional arguments. Placeholders are filled from the account, the profile and the stores; unknown placeholders become empty. A command longer than the Windows limit is refused.

**JVM defaults.** `-Xmx` is a quarter of the RAM (at least 2 GiB, at most 8 GiB, never more than half the machine); a profile can override memory and add its own validated arguments.

**Isolation.** The game runs with its working directory and game directory set to the profile's `game/` folder. Its environment is cleared and rebuilt: the Windows system directory, `TEMP`/`TMP` pointing into `cache/game-tmp/<id>`, and `USERPROFILE`/`HOME`/`APPDATA`/`LOCALAPPDATA` pointing into the profile's `home/`. Nothing a game or a mod writes by default lands on the machine. `JAVA_HOME`, `CLASSPATH` and similar variables are not passed. The game is started directly, with no shell, using the verified `java.exe` and no console window.

**Secrets.** The access token exists only inside a `Secret` and the argument vector. The command's `Debug` output and its redacted form hide the token and show the Clade folder as `<clade>`; every session-log line also passes a redactor built from the session's secrets.

**Lifecycle.** The first Play click reserves the profile, so duplicate clicks and a second launch of the same profile are refused, while other profiles are independent. The button goes Play, Launching, Playing (once the process has been alive for a few seconds; one that dies sooner counts as failed to start), then back to Play. The supervisor is an independent task that does not depend on the window.

**Closing Clade during a game** hides the window instead of closing it (there is no tray icon or service). The game and its playtime tracking continue, and Clade exits by itself when the game ends. Starting Clade again wakes the hidden window. If Clade itself is killed, the game keeps running and the next start banks the journalled playtime.

**A failure is always shown.** The "When Minecraft closes" setting only applies to a normal exit. If the game could not be started or ends abnormally, Clade's window always comes back (shown, restored if minimized, focused) so the existing launch-failure or "Minecraft stopped" dialog can be read, even with _Hide Clade_ on start and _Stay hidden_ on close; a preference never hides an error. The one exception is unchanged: if the person closed the window during the game, Clade finishes quitting when the game ends.

**Logs and exit summaries.** Each session writes `logs/games/<profile>/<time>.log` (redacted header, all output, a footer with exit code and duration; size-capped, newest ten kept). The exit is classified from the last lines into a short reason code (wrong Java version, out of memory, native library failure, mod loader error and so on). The backend sends only the code, and the UI words it in the selected language.

## 14. Playtime tracking

Playtime counts only the lifetime of a launched game process, never launcher idle time, downloads or browsing.

- Stored per profile in `instances/<id>/playtime.json` (so it travels with a copied profile), as a lifetime total in `data/playtime.json`, and as a crash journal in `data/playtime-active/`.
- Durations use a monotonic clock, so changing the system clock cannot change playtime. A 60-second heartbeat ticks the session; a tick arriving after more than 180 seconds (the PC slept) counts one interval, not the gap.
- Recovery runs at every start: any session left in the journal is banked up to its last persisted heartbeat, and unreadable entries are set aside (`.corrupt`, never overwriting an earlier backup, never deleted). A damaged per-profile or lifetime total is set aside the same way and a fresh total starts, and one session that cannot be banked does not stop the others. Banking is idempotent per session.
- **What recovery can and cannot recover.** If Clade dies while the game keeps running, only the time persisted before Clade stopped can be banked. Clade is what observes the game process, so time played while Clade is not running is not recorded; it is not bounded by the heartbeat interval and can be a whole session if Clade stopped before its first heartbeat. Playtime is not redesigned around this (no watchdog or sidecar process).
- The files hold only numbers, ids and timestamps: no paths and no secrets.

## 15. Localization

Clade's own interface ships in exactly two languages, **English (`en`) and Russian (`ru`)**. There is no third language and no machine translation. Developer communication, logs, code and comments stay in English.

- **One catalog layer.** `src/locales/en.ts` is canonical (its keys define the `MessageKey` type) and `ru.ts` is typed against it, so a missing or extra key fails the typecheck. `src/lib/i18n/` holds the engine: templates, plural rules (`Intl.PluralRules`, with Russian one/few/many), fallback and first-run resolution. No user-facing string may be hardcoded in a component.
- **Complete templates, never fragments.** A message is a whole sentence with named parameters; translated pieces are never concatenated. English is the canonical fallback.
- **Language setting.** `ui.language` in `data/settings.json` is `en`, `ru` or absent. While absent, the UI follows the Windows display language; an explicit choice is never overridden automatically, and switching applies at once without a restart.
- **Backend errors are codes, not prose.** Rust returns a stable `code` plus named `params` and never user-facing text; its English message is for logs. The frontend words each code per domain (launch, prepare, sign-in, vault, account, profile, catalog), falling back to common codes and then to a generic message. Params are facts (a profile name, byte counts), never secrets. A test cross-checks the codes in the Rust sources against the catalogs.
- **Native start-up dialogs.** Anything shown before the React interface exists (folder in use, folder not writable, folder too new, WebView2 missing and similar) goes through one function with a fixed set of message ids, each with an English and a Russian text in a small native table. The language is resolved without the frontend or the main settings system (explicit setting, then Windows language, then English), so a broken settings file cannot block error reporting.
- **Enforced by tests.** A test fails on a key present in one language only, duplicates, Russian text left in English, or different parameter sets between languages. Another parses every UI source file and fails on hardcoded text or user-facing props outside the catalogs.

Not translated: Minecraft version ids, loader names, UUIDs, paths, technical log content and the brand names Clade and Clade Launcher.

## 16. Security and portability invariants

These hold for all new code. Reporting vulnerabilities is covered in [SECURITY.md](../SECURITY.md).

- Tokens live only in the encrypted vault: never in plain files, logs, frontend state or the OS credential store.
- Never log tokens, authorization headers or full launch command lines.
- Every download is verified against a hash or size before use and written through a temporary file.
- HTTPS only, with an allow-list of hosts (Mojang, Microsoft sign-in, official loader hosts); redirects are followed only to allowed hosts.
- Archive extraction guards against traversal, absolute paths, links and decompression bombs.
- The game and loader installers are started with `Command::new(java).args([...])`, never through a shell, with a cleared environment.
- User-supplied JVM arguments and all downloaded metadata are untrusted: strictly parsed, bounded in size and validated.
- Tauri capabilities are least-privilege, with no wildcard filesystem or shell scopes and no remote content in the WebView. The frontend is granted no opener permission at all: opening the Clade folder, a profile's `game/` folder, the logs and the sign-in page is done by Rust with fixed, validated locations (the opener plugin is registered only so Rust can use it), and `tests/window_rules.rs` fails if an opener permission or the opener's JavaScript package returns.
- The WebView runs under a strict Content Security Policy (`default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src ipc: http://ipc.localhost`): no remote origins, no `unsafe-eval`, no inline or remote scripts. Inline styles are allowed because the interface sets some. A test pins the policy.
- At startup Clade removes inherited `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` and `WEBVIEW2_USER_DATA_FOLDER`, so the environment cannot move the web engine or its profile out of the folder. Release builds also remove `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`; a development build keeps that one so DevTools can be attached.
- Scratch files go to `cache/`, never the system TEMP; a child process never inherits the user's environment.
- Self-update, when it arrives, will be a custom signed-archive updater that verifies a detached signature with a key compiled into the binary. `tauri-plugin-updater` is intentionally not used because on Windows it launches installers.

## 17. Automated guards and tests

- **`src-tauri/tests/portability_rules.rs`** scans all production Rust and fails on ambient environment lookups, the system TEMP, Tauri AppData paths, `dirs`/`directories`, bare process spawning, `.minecraft` path literals and hard-coded absolute Windows paths. Each genuine exception needs a `portability-allow:` comment and is pinned in the test.
- **`src-tauri/tests/product_rules.rs`** fails if a content-platform or content-browser name appears in source, configuration or dependencies, if the download allow-list differs from the approved set, or if an API-key setting appears.
- **`src-tauri/tests/icon_rules.rs`** checks that the build notices icon changes, that every configured icon exists and that the Windows icon keeps its small sizes.
- **Frontend unit tests** cover the localization rules above, the ban on browser storage for launcher state, a guard that keeps test fixtures out of production code (`src/lib/e2eIsolation.test.ts`), and the interface components (Vitest and Testing Library).
- **Browser end-to-end tests** (`e2e/`, Playwright with Chromium) check what a person sees and does: the home screen and headline, English/Russian switching, Settings, the profile selector and Create profile (Vanilla and Modded), accounts, preparation progress and failures, the launch lifecycle, and keyboard and focus behaviour. They use accessible roles and labels rather than CSS selectors, wait on real application state rather than timers, and fail on any console error or uncaught exception.
- Rust unit tests cover version-rule evaluation, command construction, the download pipeline, the vault, the sign-in flow against a scripted transport, playtime recovery and more, using temporary folders and fake network layers. Tests never contact live Mojang endpoints.

**How the end-to-end fixtures work.** The tests run the unmodified production frontend. Playwright injects a stand-in for the Tauri backend (`e2e/support/fakeBackend.ts`) before the page loads: it implements the same commands and events the frontend uses (`src/lib/*/api.ts`), keeps its state only in the page, and holds long operations such as a preparation open until the test advances them, so every intermediate state can be asserted deterministically. The data (`e2e/support/fixtures.ts`) is obviously synthetic: accounts named `FixtureAlex` and `FixtureBlake` with all-zero UUIDs, a short invented version catalog, and no tokens of any kind. The fake never starts a Microsoft sign-in (the command is refused), never touches the network or disk, and lives only under `e2e/`, so a release build contains no fake backend or fake account; `src/lib/e2eIsolation.test.ts` fails if production code ever defines the Tauri bridge, imports test support or carries a fixture account. The real Rust command, process and launch logic is covered by the Rust tests.

If a rule has to change, change the documentation and the guard deliberately, with the reason, rather than weakening the guard to make a change pass.

## 18. Release packaging

The operating procedure is in [RELEASE.md](RELEASE.md); this section is the summary of how the pieces fit.

- **Build.** The distributable `Clade.exe` is built with `pnpm tauri build --no-bundle --target x86_64-pc-windows-msvc` (never plain `cargo build --release`, which would not embed the interface and the production protocol), by `scripts/package-portable.ps1`, with the pinned toolchain (`rust-toolchain.toml`), `--locked`, `pnpm install --frozen-lockfile`, a statically linked C runtime, and build-machine paths remapped out of the binary (Rust `--remap-path-prefix`, C `/d1trimfile`). Installers are disabled (`bundle.active` is false) because a portable build must never run one.
- **Package.** `scripts/release/package.mjs` stages only the named inputs in a clean folder (`Clade.exe`, `README.txt`, `LICENSE`, `THIRD-PARTY-NOTICES.txt`, the WebView2 licence, the WebView2 runtime) and writes a deterministic `Clade-Launcher-Portable.zip` and its `.sha256`. `scripts/release/validate-package.mjs` checks the staged names before the ZIP is written and the finished ZIP afterwards against an explicit allowlist (fail closed): no user data, no development files, required files present, no developer paths or secrets in the shipped text, none of the build machine's paths in the executable, the expected version and the release CSP embedded.
- **Versions.** `scripts/release/check-versions.mjs` requires one version in `package.json`, `Cargo.toml`, `tauri.conf.json` and `Cargo.lock`, a valid release tag that agrees with it, and a `CHANGELOG.md` entry.
- **WebView2.** The runtime is one pinned Fixed Version CAB whose version, Microsoft download URL, SHA-256 and redistribution approval metadata (`approvedBy`, `approvedOn`) live in `scripts/release/webview2-runtime.json`. The hash is verified before extraction, the executable must carry a valid Microsoft signature and the pinned version, and the runtime's licence text ships with it (`scripts/licenses/`).
- **Notices.** `scripts/release/generate-notices.mjs` writes `THIRD-PARTY-NOTICES.txt` from `cargo metadata` (the crates linked into the Windows binary) and from the installed packages that the interface source imports, including both fonts; it states what it could not cover.
- **Smoke test.** `scripts/smoke-test-package.ps1` validates, extracts and starts the package and checks the portable folders, the bundled runtime and that nothing contacts the development server.
- **Workflow.** `.github/workflows/release.yml` runs only for pushed `v*` tags, with no default permissions; the build job is read-only and only the draft-release job can write. It creates a draft (and a pre-release for previews), never a published release.

The runtime's redistribution terms, the unsigned executable, the lack of automatic runtime updates and the remaining manual validation are listed in [RELEASE.md](RELEASE.md#10-known-limits).

## 19. Known limitations and current status

- **Authentication is verified on a development PC only.** Mojang has approved the Microsoft application ID. The whole chain (Microsoft sign-in, Xbox Live, XSTS, Minecraft login, entitlements, profile), persistence through the vault after a restart, and launches of modern Vanilla, 1.12.2, Fabric, Forge, NeoForge and Quilt were verified there with a real account and a real GPU. That machine has a system WebView2 and a real `.minecraft`, so it is not a clean-machine certification. The automated tests still use scripted transports and development fixtures only.
- Microsoft may ask for a new sign-in on any PC (an expired or revoked session, a password change, a new-device check). Portability cannot prevent that.
- The bundled WebView2 runtime is not updated automatically; security fixes require re-packaging with a newer runtime.
- A clean-machine test without any WebView2 installed has not been done yet.
- `Clade.exe` is not code-signed and signing is not implemented yet (the provider is still to be chosen). Windows Smart App Control blocks an unsigned `Clade.exe` in Windows Sandbox, SmartScreen may warn, and antivirus software may block it. Smart App Control also evaluates the third-party DLLs a game loads, which signing Clade cannot change.
- The filesystem should be NTFS or exFAT; FAT32 cannot hold files over 4 GB and has no atomic-rename guarantee. On Windows 10, FAT32 and exFAT do not support persistent ACLs (`FILE_PERSISTENT_ACLS`), so the AppContainer permissions needed by the bundled Fixed Version WebView2 runtime cannot be stored on those volumes; Clade falls back to the PC's installed Evergreen WebView2 runtime when one is present, and NTFS is recommended for removable drives meant for clean Windows 10 PCs without WebView2. Also on Windows 10, where Windows 11's DWM Mica backdrop (22H2+) is unavailable, Clade uses its warm opaque paper window surface.
- The launcher is not fully hermetic: Java may touch the registry for its preferences, and mods can write wherever they choose.
- A folder under `instances/` whose `instance.json` is damaged or unreadable is left untouched and omitted from the playable profile list; healthy profiles are unaffected. The problem is written to the log, included in the diagnostics summary, and surfaced as a warning in the interface so the folder never disappears silently.
- "Verify files" can only check a Java runtime against a list that Mojang's manifest provides: the list recorded at install time, or the current manifest when it describes the same version. An older runtime whose version Mojang no longer lists is covered only by its `java.exe` hash.
- Self-update is not implemented yet. Skin and cape changes are implemented but have not yet been exercised against a real account.

## 20. Settings, first-run introduction and diagnostics

Settings are deliberately short and start in **Simple** mode (`ui.ui_mode = "simple"`), keeping the main Settings dialog focused on language, sound effects, volume, opening the Clade folder, and resetting settings (while profile dialogs stick to name, version, files, duplication, and removal). Switching the mode toggle in Settings to **Advanced** (`"advanced"`) reveals reduced motion, transparency effects, global game memory, launch/exit behavior, logs and diagnostics export, snapshot/historical version filters, manual loader version picking, and per-profile memory, window size, and allowlisted JVM arguments. Global Settings still has no theme, Java-path, network, update, or telemetry options.

### The settings file

Everything is stored by the Rust settings service in `data/settings.json` and nowhere else (no WebView storage). Every field has a safe default, old files load unchanged, damaged values are repaired rather than fatal, and the file is written atomically with a stable field order. Values are repaired one at a time: in a file that is valid JSON, an unsupported or unreadable value (an unknown mode, an out-of-range number, text where a number belongs, a path where a profile id belongs, a whole section of the wrong shape) is reset to its default and every other setting is kept; the file itself stays in place and the log names only the settings that were reset. A file that is not a settings document at all (empty, truncated, malformed JSON, or not a JSON object) never stops Clade from starting either: it is moved aside as `settings.json.corrupt` (or `.corrupt-<time>` if that exists, so an earlier backup is never overwritten), the defaults are used, and one English line in the log says so without quoting the file. A missing file simply means defaults. A file written by a newer Clade, and real filesystem failures (permissions, a locked file, failing to move the damaged file aside) are still reported as startup errors.

```json
{
  "layout_version": 1,
  "last_instance": "survival",
  "max_concurrent_downloads": 8,
  "ui": {
    "sound": { "enabled": true, "volume": 0.5 },
    "last_phrase": "welcome-back",
    "language": "ru",
    "ui_mode": "simple",
    "reduced_motion": "system",
    "transparency": "system",
    "onboarding_done": true
  },
  "minecraft": { "memory_mb": null },
  "behavior": { "on_launch": "embed", "on_exit": "show" }
}
```

| Field                 | Values                       | Default            | Notes                                                                                                                                                                       |
| --------------------- | ---------------------------- | ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ui.ui_mode`          | `simple`, `advanced`         | `simple`           | Controls whether tuning and diagnostic controls are shown; anything else is repaired to `simple`.                                                                           |
| `ui.reduced_motion`   | `system`, `on`, `off`        | `system`           | `system` follows the Windows preference; anything else is repaired to `system`.                                                                                             |
| `ui.transparency`     | `system`, `on`, `off`        | `system`           | `system` follows Windows Transparency effects (disabled under High Contrast); anything else is repaired to `system`.                                                        |
| `ui.onboarding_done`  | `true`, `false`              | `false`            | Set once the first-run introduction is finished or skipped. It lives in the portable folder, so moving the folder keeps it.                                                 |
| `minecraft.memory_mb` | a number or `null`           | `null` (automatic) | The global game memory in MiB. Stored values stay portable (`1024..=32768` in 256 MiB steps) and are clamped at launch time to this machine's safe physical/commit ceiling. |
| `behavior.on_launch`  | `embed`, `keep_open`, `hide` | `embed`            | `embed` frames Minecraft inside Clade's window; `hide` hides Clade and suspends WebView2 memory while the game runs; unknown values are repaired.                           |
| `behavior.on_exit`    | `show`, `stay_hidden`        | `show`             | Unknown values are repaired.                                                                                                                                                |

The frontend reads them once at start (`get_ui_settings`, which also reports the memory limits for this computer, what "Automatic" means here and the application version from `Cargo.toml`) and writes through small commands: `set_sound_settings`, `set_language`, `set_ui_mode`, `set_reduced_motion`, `set_transparency`, `set_memory`, `set_behavior`, `complete_onboarding` and `reset_settings`. Each change is announced on the `ui-settings-changed` event, so the UI stays in sync. Refusals are coded errors (`invalid_memory` with the allowed range, `unsupported_value`, `storage`) that the interface words in the current language.

**Reset settings** puts the preferences above back to their defaults (sound, language, `ui_mode` back to `simple`, reduced motion, transparency, memory, behavior). It deliberately keeps `ui.onboarding_done`, the selected profile, the last headline and the download concurrency, and it cannot touch profiles, accounts, the vault, Minecraft files or playtime, because none of them is stored in this file.

### Reduced motion

One module (`src/lib/motion.ts`) decides whether decorative motion is reduced. The Settings choice wins; `system` defers to the operating system. It sets `data-motion` (`reduce`, `full` or `system`) on the root element, which the stylesheet follows, and scripted animations ask `prefersReducedMotion()`. Necessary state changes and focus feedback are kept; ambient background motion, entrance animations, burst particles and long fades are removed or shortened.

### Window backdrop (glass)

When transparency is enabled (`ui.transparency` set to `on`, or `system` while Windows "Transparency effects" are on and High Contrast is off), Windows 11 (22H2+) uses the DWM Mica backdrop (`DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW` in light mode) behind translucent interface surfaces. On Windows 10, where DWM Mica does not exist, or whenever transparency is off or unavailable, Clade automatically stays on its warm opaque paper window look.

### Game memory

The memory used for a launch is the profile's own setting if it has one, else the global value from Settings, else the automatic policy (a quarter of the RAM, between 2 and 8 GiB, never more than half the machine). The result is always kept within sane bounds.

### Window behavior around a game

By default (`behavior.on_launch = "embed"`), **Inside Clade** keeps Clade's frameless host window open as the outer frame: once Minecraft creates its native window, Rust reparents it directly into Clade's stage rectangle via Win32 `SetParent`, stripped frame styles, `SetWindowPos`, and WebView cutout regions (`SetWindowRgn` / `CreateRectRgnIndirect` with `CombineRgn` `RGN_DIFF` so the WebView never occludes the child game HWND). While a game is framed inside Clade, the main menu hides and only the top header strip (product title plus Minimize, Maximize/Restore, and Close) and warm background margins remain visible; Maximize/Restore expands the host to the monitor work area in Rust (`toggle_game_maximize`) without triggering DWM's duplicate native caption buttons, and exiting the game restores Clade's pre-attach window geometry. Under **Keep Clade open**, Minecraft opens in a standalone window; under **Hide Clade**, the window hides while the game runs. When the last running game ends, **Show Clade again** shows and focuses a hidden window; **Stay hidden** leaves it hidden, and because Clade has no tray icon or background service, a hidden Clade then quietly ends together with the game (starting Clade again brings it up). A window that is already visible is left alone. In `keep_open`/`hide`, closing Clade while the game runs hides Clade and exits once the game ends; in `embed`, closing asks the embedded game window to close cleanly before quitting once it exits.

### First-run introduction

When `ui.onboarding_done` is `false`, a three-page introduction opens in the shared modal (Welcome, Portable by design, Ready to start). It needs no network, no account and no profile. **Skip** and **Finish** call `complete_onboarding`, which marks it done permanently; Escape counts as Skip; the last page can also lead straight into the normal Create profile or Add account dialogs (which keep their own behavior, including the approval-pending message). It follows the normal language rules. If the backend cannot be read, the introduction is treated as seen so it can never trap anyone.

### Diagnostics export

**Export diagnostics** creates `logs/diagnostics/Clade-Diagnostics-<UTC date>-<time>.zip` inside the Clade folder (the newest five are kept) and shows the file in its folder. The archive is built from a fixed allowlist and nothing else:

| Entry                       | Content                                                                                                                                  |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| `launcher-version.txt`      | Clade version, folder layout version, export time                                                                                        |
| `system-info.txt`           | OS and architecture, Windows display language, RAM, free space on the Clade drive, WebView2 source and version                           |
| `profile-summary.txt`       | per profile: id, Minecraft version, vanilla or loader, set-up state, memory override, number of extra JVM arguments (never their values) |
| `settings-redacted.json`    | the settings file                                                                                                                        |
| `logs/clade.log`            | the end of the launcher log (at most 512 KiB)                                                                                            |
| `logs/minecraft-latest.log` | the end of Minecraft's `latest.log` for the profile played last (at most 512 KiB)                                                        |

It never reads the vault, tokens, worlds, mods, resource packs or any file the frontend names (the frontend names nothing). The only account data it touches is the metadata-only `accounts.json`, and only to learn the known Minecraft profile names and UUIDs so they can be removed from the exported logs. Text from logs is redacted before it is written: the Clade folder becomes `<clade>`, the Windows user folder becomes `<user>`, e-mail addresses become `<email>`, the known account profile names become `<mc-name>` (whole words only) and their UUIDs, dashed or not, become `<mc-uuid>` (the local game logs on disk are not rewritten), and values after `access_token`, `refresh_token`, `password`, `passphrase`, `session`, `Authorization`/`Bearer`, `--accessToken` and similar keys, JWT-shaped strings and Microsoft refresh tokens become `<redacted>`. The unit tests check the allowlist, that planted secrets and paths never reach the ZIP, and that a failure (for example an unwritable folder) is reported as the stable code `diagnostics_failed`.
