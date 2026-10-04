<p align="center">
  <img src="docs/media/banner-final.png" alt="Clade Launcher. Simple. Portable. Yours." width="100%">
</p>

# Clade Launcher

> **Simple. Portable. Yours.**

Clade Launcher is an independent, minimal and fully portable launcher for **Minecraft: Java Edition** on Windows.

Clade is built around a simple idea: the launcher should stay out of the way and let you manage and play Minecraft without turning into a content marketplace.

## Preview

### A quiet launcher that stays out of the way

<p align="center"><img src="docs/media/hero-interaction.gif" alt="Clicking the headline on the Clade home screen changes the phrase" width="720"></p>

### Accounts stay independent from your profiles

<p align="center"><img src="docs/media/account-menu.gif" alt="Switching accounts and opening an account card" width="720"></p>

### Create Vanilla or modded profiles

<p align="center"><img src="docs/media/create-profile.gif" alt="Creating a profile: Vanilla or Modded, the version list and the loader choice" width="720"></p>

### English and Русский

<p align="center"><img src="docs/media/settings-language.gif" alt="Switching the launcher between English and Russian in Settings" width="720"></p>

The recordings show the real application with development fixture accounts, not a mock-up.

## Highlights

- Fully portable: extract, run, and move the folder
- No installer required
- Launcher-managed Java runtimes
- Bundled portable WebView2 runtime
- Multiple isolated Minecraft profiles
- Vanilla-first design
- Optional Fabric, Forge, NeoForge, and Quilt profiles
- Multiple Microsoft / Minecraft accounts
- Automatic Minecraft and Java preparation
- File verification and repair
- Reuse of already prepared profiles while offline
- Per-profile and lifetime playtime tracking
- English and Russian interface, including the native start-up dialogs
- No built-in mod, modpack, shader, or resource-pack marketplace

## Project status

Clade Launcher is in active pre-release development. No stable public release has been published yet.

Implemented and covered by automated tests:

- the portable architecture, encrypted credential vault and isolated profiles;
- profile creation against the official Minecraft and loader catalogs;
- preparation of Minecraft, managed Java and the selected loader, with verification and repair;
- the Microsoft / Xbox / Minecraft sign-in flow, up to Mojang's approval step;
- the launch core: command construction for modern, legacy and loader-based versions, an isolated process environment, lifecycle tracking, crash summaries and playtime;
- complete English and Russian localization, including the native dialogs shown before the interface exists.

Still pending before the first preview:

- **Real authenticated Minecraft launch.** Clade's Microsoft application ID is awaiting Minecraft Services approval. Until Mojang approves it, sign-in stops at that step, so the final end-to-end launch with a real account has **not** been verified. The launch pipeline has only been exercised with controlled development fixtures and placeholder identities; nothing is claimed beyond that.
- Skin and cape management (it needs a real signed-in account first).
- Release packaging, clean-machine verification, release automation and a signed self-updater.

## Vanilla-first

Clade keeps Vanilla Minecraft clean by default.

Modded profiles are optional. When you create one, Clade prepares Minecraft and the selected loader, while your own content stays inside that profile.

Typical per-profile folders include:

```text
game/
├─ mods/
├─ config/
├─ resourcepacks/
├─ shaderpacks/
└─ saves/
```

Profiles are isolated from one another, so a modded setup does not contaminate a Vanilla profile.

## Portable by design

Launcher-controlled persistent data is kept inside the Clade directory.

A prepared Clade folder is designed to be movable to another location, drive, USB device, or compatible Windows PC without reinstalling the launcher or relying on a global `.minecraft` directory.

Typical layout:

```text
Clade-Launcher/
├─ Clade.exe
├─ data/
├─ instances/
├─ minecraft/
├─ libraries/
├─ assets/
├─ runtimes/
├─ cache/
└─ logs/
```

Windows itself may still create normal operating-system artifacts such as recent-item history or event records. Clade does not intentionally use those locations for launcher-owned persistent data.

## Accounts and authentication

Clade is designed for legitimate Microsoft / Minecraft accounts only.

Authentication uses Microsoft's official sign-in flow in your system browser. Clade does **not** ask for or store your Microsoft password.

Sensitive launcher credentials are stored locally in an encrypted portable vault protected by a master passphrase. Account identity is kept independent from Minecraft profiles, so switching accounts does not alter a profile's saves, mods, settings, or playtime.

There is no cracked, guest, or offline-account bypass mode.

## Minecraft Services

Clade uses official Microsoft, Xbox, Mojang, and Minecraft services where required to:

- authenticate the user;
- obtain the Minecraft access token;
- retrieve the Java profile;
- verify the account state required for launching;
- support official Java profile features such as skins and owned capes when those controls are enabled.

Clade's Microsoft application ID is currently awaiting Minecraft Services approval (see [Project status](#project-status)).

## Profiles and versions

Clade supports isolated profiles across multiple Minecraft generations, including:

- current releases;
- snapshots;
- historical alpha and beta versions;
- Fabric;
- Forge;
- NeoForge;
- Quilt.

Version and loader metadata come from their official project sources.

## Integrity and recovery

Clade verifies downloaded files before considering a profile ready.

It can detect and repair problems such as:

- missing or truncated libraries;
- damaged assets;
- interrupted downloads;
- missing or corrupted managed Java runtimes;
- same-size file corruption through full verification.

A lightweight integrity check runs before launch, while a full **Verify files** action is available for deeper validation.

## Privacy

Clade is designed as a local desktop application.

Launcher-controlled profiles, settings, account metadata, caches, logs, runtimes, and game data stay inside the portable Clade directory.

Clade does not require an account with a third-party mod distribution platform and does not contain a built-in content marketplace.

## Building from source

Clade is a Tauri 2 application: a Rust backend in `src-tauri/` and a React + TypeScript + Vite frontend in `src/`. It targets 64-bit Windows 10/11.

You need:

- [Rust](https://rustup.rs/) (stable, MSVC toolchain) and the Visual Studio C++ build tools;
- [Node.js](https://nodejs.org/) and [pnpm](https://pnpm.io/) (the exact version is pinned in `package.json`);
- the Microsoft Edge WebView2 Runtime (already present on current Windows 10/11).

```bash
pnpm install        # downloads the frontend dependencies
pnpm tauri dev      # builds the Rust side (first build downloads crates) and starts Clade
```

Quality checks, all of which should pass before a change is proposed:

```bash
pnpm check                              # typecheck + eslint + vitest
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

End-to-end layout tests use Playwright and need a browser once: `pnpm exec playwright install chromium`, then `pnpm test:e2e`.

In development Clade keeps its data in `src-tauri/target/debug/clade-root`, never in your user profile.

### What is, and is not, in this repository

In Git: the source, tests, lockfiles (`pnpm-lock.yaml`, `src-tauri/Cargo.lock`), configuration, localization catalogs, the application icons, the packaging script and this documentation.

Not in Git, and fetched or generated when you need it:

- `node_modules/` and `src-tauri/target/` (installed and built locally);
- Minecraft versions, libraries, assets, Java runtimes and mod loaders, which Clade downloads at run time from the official Mojang and loader hosts into the Clade folder;
- the WebView2 Fixed Version Runtime, which you download from Microsoft only when building a release (`scripts/package-portable.ps1`);
- any Clade data folder, accounts, vault, logs or portable release archives.

Architecture notes, product rules and the portability and security requirements are in [CLAUDE.md](CLAUDE.md).

## Technology

Clade is built with Tauri 2, Rust, React, TypeScript, Vite and Tailwind CSS. The interface uses the Fraunces and Instrument Sans typefaces (both open-licensed, installed from npm).

## License

Clade Launcher is licensed under the [Apache License 2.0](LICENSE).

## Security

Security and responsible-disclosure guidance is available in [SECURITY.md](SECURITY.md).

## Contributing

Contribution guidance is available in [CONTRIBUTING.md](CONTRIBUTING.md).

## Changelog

Development changes are tracked in [CHANGELOG.md](CHANGELOG.md).

## Disclaimer

Clade Launcher is an independent project and is not affiliated with, endorsed by, or sponsored by Mojang Studios or Microsoft.

Minecraft and related marks belong to their respective owners.
