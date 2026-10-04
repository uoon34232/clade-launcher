# Clade Launcher

> **Simple. Portable. Yours.**

Clade Launcher is an independent, minimal and fully portable launcher for **Minecraft: Java Edition** on Windows.

Clade is built around a simple idea: the launcher should stay out of the way and let you manage and play Minecraft without turning into a content marketplace.

## Highlights

- Fully portable — extract, run, and move the folder
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
- English and Russian UI planned for the first public preview
- No built-in mod, modpack, shader, or resource-pack marketplace

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

Authentication uses Microsoft's official sign-in flow. Clade does **not** ask for or store your Microsoft password.

Sensitive launcher credentials are stored locally in an encrypted portable vault. Account identity is kept independent from Minecraft profiles, so switching accounts does not alter a profile's saves, mods, settings, or playtime.

There is no cracked, guest, or offline-account bypass mode.

## Minecraft Services

Clade uses official Microsoft, Xbox, Mojang, and Minecraft services where required to:

- authenticate the user;
- obtain the Minecraft access token;
- retrieve the Java profile;
- verify the account state required for launching;
- support official Java profile features such as skins and owned capes when those controls are enabled.

Clade's Microsoft application ID is currently awaiting Minecraft Services approval. The launch pipeline has been implemented and tested with controlled development fixtures, but the final end-to-end authenticated launch test will only be performed after that approval.

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

## Languages

Clade is being prepared with exactly two interface languages for the first public preview:

- English
- Русский

The language can be changed from launcher settings without redesigning the rest of the interface.

## Project status

Clade Launcher is in active pre-release development.

The core portable architecture, profile preparation, loader support, repair pipeline, managed Java runtime handling, process lifecycle, and launch command construction are implemented and under test.

Before the first public preview, the project is still completing:

- English / Russian localization;
- public source-code audit and publication;
- real Microsoft / Minecraft end-to-end launch verification after AppID approval;
- release packaging and clean-machine verification;
- release automation and updater work.

No stable public release has been published yet.

## First public preview

The first public preview is expected to include:

- portable Windows build;
- Vanilla and modded profiles;
- multiple Minecraft versions;
- Microsoft account support;
- Fabric / Forge / NeoForge / Quilt;
- automatic Java runtime management;
- file verification and repair;
- playtime tracking;
- English and Russian UI;
- public source code.

The initial release is planned as a preview rather than a `1.0` release.

## Technology

Clade is built with:

- Tauri 2
- Rust
- React
- TypeScript
- Vite

Build instructions will be documented when the source tree is published for the first public preview.

## Security

Security and responsible-disclosure guidance is available in [SECURITY.md](SECURITY.md).

## Contributing

Contribution guidance is available in [CONTRIBUTING.md](CONTRIBUTING.md).

## Changelog

Development changes are tracked in [CHANGELOG.md](CHANGELOG.md).

## Disclaimer

Clade Launcher is an independent project and is not affiliated with, endorsed by, or sponsored by Mojang Studios or Microsoft.

Minecraft and related marks belong to their respective owners.
