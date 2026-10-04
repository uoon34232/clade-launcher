# Clade Launcher: technical documentation

This document explains how Clade Launcher is built and which rules its code follows. It is written for contributors and for anyone who wants to check the claims made in the [README](../README.md). For reporting vulnerabilities see [SECURITY.md](../SECURITY.md); for contribution rules see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Pre-release status.** Clade Launcher is currently pre-release. The launch pipeline is implemented, but a real authenticated Minecraft launch has not yet been verified because Minecraft Services AppID approval is pending.

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

## 1. Overview

Clade is a Windows desktop launcher for Minecraft: Java Edition. It is built with **Tauri 2**: a Rust backend does all privileged work, and a **React + TypeScript + Vite** frontend (styled with Tailwind CSS) is a thin view layer on top of it.

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

| Area | Modules |
|---|---|
| Storage and safety | `portable/` (root discovery, validated relative paths, atomic writes, hashing, folder lock), `safe_archive`, `download_policy`, `net`, `coded_error` |
| Settings and state | `settings`, `ui_settings`, `startup`, `startup_text`, `webview` |
| Profiles | `instances`, `versions`, `profiles`, `mc_catalog`, `loader_options`, `loaders` |
| Preparation | `prepare`, `prepare_cmd`, `download`, `mc_meta`, `java_install`, `runtimes`, `natives`, `loader_prepare`, `loader_install` |
| Accounts | `vault`, `accounts`, `accounts_api`, `auth`, `ms_auth`, `profile_service` |
| Launching | `launch_args`, `launch_cmd`, `game_session`, `system`, `playtime` |

**Frontend rules.** The frontend calls typed Tauri commands and listens to events. It contains no business logic and no secrets, and it never talks to Mojang or Microsoft itself. All native calls go through one typed boundary (`src/lib/ipc.ts`, with validating bridges per feature under `src/lib/`). Everything crossing that boundary is validated at the edge, and there are no fake profiles or accounts in production code.

**Commands and events.** Commands are small, validated and return a serializable stable-code error. Long tasks (downloads, preparation, launch) run as tokio tasks, report progress through events, and are cancellable.

## 4. Portable filesystem architecture

The **root** is the folder containing `Clade.exe`, found at run time from the executable's path. Debug builds use `target/debug/clade-root` so development never pollutes the repository.

```
Clade-Launcher/
├─ Clade.exe
├─ data/        settings.json, accounts.json, vault.bin, clade.lock, webview2/ profile
├─ instances/   one folder per profile
├─ minecraft/   shared version files
├─ libraries/   shared libraries
├─ assets/      shared game assets
├─ runtimes/    java/ and webview2/
├─ cache/       scratch and catalog caches
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

**WebView2.** Release builds ship the Fixed Version runtime in `runtimes/webview2/` and point WebView2 at it before any window exists. If neither the bundled nor an installed runtime loads, Clade shows a native message and exits; it never downloads or runs an installer.

## 5. Profiles and isolation

A profile (internally an *instance*) is created with two required answers: **Vanilla or Modded**, and **which Minecraft version**. There is no default for either. A profile's type is fixed at creation.

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
- **Isolation is absolute.** Everything a player or a mod can modify lives under that profile's `game/`. There is intentionally no shared mods, config, saves, resource-pack or shader folder, and no links or junctions between profiles.
- **What may be shared** (`minecraft/`, `libraries/`, `assets/`, `runtimes/`): only immutable, hash-verified files published by Mojang or the loader project.
- Minecraft versions are treated as opaque, path-safe ids. Nothing in the launcher's own logic hard-codes or special-cases a Minecraft version.
- Deleting a profile removes only its own folder.
- JVM arguments and memory are user input: validated, shown before launch and passed as separate arguments, never through a shell.

## 6. Version catalog and profile creation

The frontend never fetches Minecraft or loader metadata. Rust does, through the commands `list_profiles`, `select_profile`, `get_version_catalog`, `get_loader_options` and `create_profile`.

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

**Java.** Mojang's runtime index (`windows-x64`) lists a component manifest, which is checked against its SHA-1. Every file is downloaded and verified into a staging folder, then the folder is renamed into `runtimes/java/` in one step. A runtime is reused while `bin/java.exe` still matches its recorded SHA-256. A version without a stated Java requirement uses the legacy runtime.

**Natives** are unpacked by the safe archive extractor into scratch space, filtered, and swapped into `instances/<id>/natives/` in one rename.

**Progress** is reported as stages with real file and byte counts, or as indeterminate while nothing can be measured (for example while a Forge installer runs).

## 8. Loaders: Fabric, Forge, NeoForge, Quilt

Clade may download and install **only the loader the user selected, from that loader's official source**. A loader is infrastructure needed to run a modded profile, not content.

- **Official sources only.** Each URL is built from validated tokens and checked against a loader-specific host list that is stricter than the global allow-list: Fabric and Quilt use their meta services and Maven hosts; Forge and NeoForge use their official file and Maven hosts. No mirrors and no third-party download sites.
- **Compatible versions.** The catalog yields versions compatible with the chosen Minecraft version, newest first, labelled Recommended, Stable, Latest or Prerelease. An empty list means "no compatible loader"; the UI says so and Clade never guesses. `Auto` picks recommended, then stable, then latest, then prerelease.
- **Fabric and Quilt** need no installer process. The official profile JSON is saved and every library is verified with the hash and size inside it. Library coordinates go through a strict Maven-path builder that can only produce paths under `libraries/`.
- **Forge and NeoForge** use the official installer jar. It is verified against the checksum published beside it and only then run, with the managed Java, inside a sandbox (below). Residual trust: that checksum comes from the same host as the jar, so it catches corruption and tampering but not a compromised loader project, and the installer makes its own library downloads, which Clade cannot restrict.
- **Loader output** (profile JSON, libraries) is immutable and goes into the shared stores. A loader is never installed into a profile's `game/` folder.

**Installer sandbox.** The Forge and NeoForge installers would normally write to `%APPDATA%\.minecraft`. Instead, each installation gets a fresh staging area under `cache/loader-install/<id>/` with its own target folder, a stand-in user profile and a stand-in TEMP directory. The installer process runs with a **completely cleared environment** plus a few explicit variables pointing into the stage, never as administrator and never with a mirror option. The command line is verified twice (also right before spawning) and refused if any program, argument or environment value is an absolute path outside the root. A tripwire watches the real `.minecraft` directories and fails the installation loudly if anything changes. Only `libraries/` and `versions/` are then merged into the shared stores, with links refused and an existing file never overwritten with different content.

## 9. Downloads, verification and repair

- **HTTPS only**, through the allow-list in `download_policy`: Mojang, Microsoft sign-in, and loader hosts. Content platforms are never on it, and tests assert that.
- Every job carries a mandatory SHA-1 or SHA-256 and usually a size. Existing verified files are reused with no network.
- Bytes stream into a `.part` file and resume with `Range` where the server supports it. A part is never trusted until it verifies in full. Finalisation is an atomic rename.
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

**Token lifecycle.** A cached Minecraft token is used while it has more than five minutes left. Otherwise the Microsoft token is refreshed and the chain repeated, and the rotated refresh token is stored. Revoked or expired credentials mark the account as *needs sign-in again* and keep the account and its data; network failures change nothing. Accounts are never deleted because credentials expired. Signing in again must return the same Minecraft UUID or it is refused.

**Error codes.** The UI distinguishes `cancelled`, `timed_out`, `browser_unavailable`, `offline`, `oauth`, `needs_sign_in`, `no_xbox_account`, `xbox_region`, `adult_verification`, `child_account`, `xbox_banned`, `xsts`, `approval_required`, `no_java_profile`, `bad_response`, `vault_locked`, `storage` and `wrong_account`.

**Skin and cape changes** are not built yet. When they are, they will use only Mojang's authenticated Minecraft services, so one profile looks the same in the game, the official launcher and account pages. There will be no launcher-local cosmetic overrides and no third-party skin services.

## 11. The encrypted credential vault

Refresh tokens and the Minecraft access token are stored only in `data/vault.bin`. Clade never uses the OS credential store, because that is bound to one machine and user and would break portability.

- A master passphrase is stretched with **Argon2id** (64 MiB memory, 3 passes) into a 32-byte key. Floors are enforced on read, and ceilings protect against hostile files.
- Secrets are encrypted with **XChaCha20-Poly1305** using a fresh random nonce per save, with the whole header authenticated.
- The passphrase is never stored. A wrong passphrase and a tampered file are indistinguishable by design. Keys and secrets are zeroized on lock.
- Writes are atomic and keep one `.bak` generation.
- First sign-in asks for a new passphrase (eight characters or more, repeated, with a clear "cannot be recovered" warning). Later sessions unlock when a sign-in or refresh is needed. The vault is unlocked **before** the browser opens, so a finished sign-in can always be saved. The passphrase is sent to Rust once and never kept in the frontend.
- `accounts.json` holds metadata only (internal id, Minecraft UUID, profile name, status, timestamps), never tokens. It stays readable while the vault is locked.

Limit: a stolen copy of the folder allows unlimited offline guessing. Argon2id slows that down but cannot protect a weak passphrase.

## 12. Accounts versus profiles

An account is **never** bound to a profile. Profiles define the version, the loader and the files; accounts define the Microsoft/Minecraft identity. Neither a profile manifest, any profile file nor playtime references an account, and selecting or removing an account changes none of them (tests assert this byte for byte). Only the selected account is persisted, as a convenience, in `accounts.json`.

The same Minecraft UUID never creates a second entry. The newest sign-in becomes the selected account. Removing an account also removes its vault secrets.

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

**Logs and exit summaries.** Each session writes `logs/games/<profile>/<time>.log` (redacted header, all output, a footer with exit code and duration; size-capped, newest ten kept). The exit is classified from the last lines into a short reason code (wrong Java version, out of memory, native library failure, mod loader error and so on). The backend sends only the code, and the UI words it in the selected language.

## 14. Playtime tracking

Playtime counts only the lifetime of a launched game process, never launcher idle time, downloads or browsing.

- Stored per profile in `instances/<id>/playtime.json` (so it travels with a copied profile), as a lifetime total in `data/playtime.json`, and as a crash journal in `data/playtime-active/`.
- Durations use a monotonic clock, so changing the system clock cannot change playtime. A 60-second heartbeat ticks the session; a tick arriving after more than 180 seconds (the PC slept) counts one interval, not the gap.
- Recovery runs at every start: any session left in the journal is banked up to its last heartbeat (at most about a minute is lost), and unreadable entries are set aside. Banking is idempotent per session.
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
- Tauri capabilities are least-privilege, with no wildcard filesystem or shell scopes and no remote content in the WebView.
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

`scripts/package-portable.ps1` builds the portable archive: it extracts the WebView2 Fixed Version Runtime from a CAB obtained directly from Microsoft, **refuses to continue unless `msedgewebview2.exe` has a valid Microsoft signature**, builds with a static C runtime, and assembles `Clade-Launcher/` and `Clade-Launcher-Portable.zip` with a checksum. Installers are disabled (`bundle.active` is false) because a portable build must never run one. The runtime's licence text ships inside the archive (`scripts/licenses/`). A `-WithoutWebView2` switch exists for development only and produces a differently named archive.

The application icon is `src-tauri/icons/icon.ico`. `build.rs` tells Cargo to watch it, so a changed icon is rebuilt into the executable.

## 19. Known limitations and current status

- **Authenticated launch is unverified.** Clade's Microsoft application ID is awaiting Minecraft Services approval. Until Mojang approves it, `login_with_xbox` answers with an "invalid app registration" error and Clade shows a clear message and saves nothing. This is by design and is not bypassed. Everything after that step (entitlements, profile, persistence, refresh and the real launch with a real account) has only been exercised against scripted transports and development fixtures.
- Microsoft may ask for a new sign-in on any PC (an expired or revoked session, a password change, a new-device check). Portability cannot prevent that.
- The bundled WebView2 runtime is not updated automatically; security fixes require re-packaging with a newer runtime.
- A clean-machine test without any WebView2 installed has not been done yet.
- Windows may warn about an unsigned `Clade.exe` (SmartScreen, Smart App Control), and antivirus software may block it. Code signing is not implemented.
- The filesystem must be NTFS or exFAT; FAT32 cannot hold files over 4 GB and has no atomic-rename guarantee.
- The launcher is not fully hermetic: Java may touch the registry for its preferences, and mods can write wherever they choose.
- Self-update and skin/cape management are not implemented yet.
