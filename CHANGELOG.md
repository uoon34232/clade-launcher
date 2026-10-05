# Changelog

All notable changes to Clade Launcher will be documented in this file.

The project is currently pre-release. Versioned entries will begin with the first public preview.

## [Unreleased]

### Added

- Portable Windows launcher architecture based on Tauri 2 and Rust.
- Isolated Minecraft profiles and game directories.
- Official Mojang version catalog support for releases, snapshots, and historical versions.
- Vanilla, Fabric, Forge, NeoForge, and Quilt profile preparation.
- Launcher-managed Java runtimes stored inside the portable directory.
- Verified downloads, interrupted-download resume, repair, and full file verification.
- Portable sandboxing for Forge and NeoForge installer execution.
- Microsoft authentication flow using authorization code + PKCE.
- Encrypted portable credential vault.
- Multiple-account architecture independent from Minecraft profiles.
- Real Minecraft launch-command construction across modern, legacy, and loader-based profiles.
- Process lifecycle tracking, crash handling, duplicate-launch prevention, and portable game logs.
- Real playtime tracking based on Minecraft process lifetime.
- Portable WebView2 Fixed Version Runtime support.
- Minimal Clade UI with profile creation, account management, preparation progress, and launcher settings.
- Interactive rotating home-screen phrases, restrained particles, and procedural UI sounds.
- Full-file verification command and pre-launch integrity repair path.
- Disk-space checks before large preparation or repair work.
- Complete English and Russian localization of the interface, with a first-run choice that follows the Windows display language and a Language setting.
- Localized native start-up dialogs (folder in use, WebView2 missing and similar) shown before the interface exists.
- Backend errors reported as stable codes and worded by the interface in the selected language.
- Final Clade branding: logo mark, application and Windows icons, README banner, social preview and interface recordings.
- Apache License 2.0.
- Public source tree, build instructions and automated guard tests for portability, product rules and the application icon.

### In progress

- Minecraft Services AppID approval. Until Mojang approves Clade's Microsoft application ID, sign-in stops at that step, so the final authenticated end-to-end launch with a real account has **not** been verified. The launch pipeline has only been exercised with development fixtures.
- Final portable release packaging and clean-machine verification.
- GitHub Actions release automation: the tag-driven draft-release workflow, the packaging tools and the validation exist (see `docs/RELEASE.md`); the approved WebView2 runtime pin and the first real run are still to come.
- Portable self-update design and implementation.
- Official skin and owned-cape management after real authenticated account verification.

## Release policy

Clade follows semantic versioning where practical.

The first public build is planned as a preview release rather than `1.0.0`. Breaking changes may occur during the preview period.
