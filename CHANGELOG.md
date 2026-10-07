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
- Real Microsoft, Xbox and Minecraft sign-in, approved by Mojang and verified with a real account; the account avatar is the head (with the hat layer) of the account's current Minecraft skin, cached inside the Clade folder.
- Preparing, verifying and launching a profile require a ready active account; without one the main button reads "Sign in to continue". Creating and browsing profiles still works.
- "Remove from Clade" takes a profile out of the list and keeps its whole folder, worlds included, under `instances/.removed/`. Nothing is deleted.
- The dock's profile button shows only the profile's name, and the profile and account menus are denser.
- Exported diagnostics redact the known Minecraft profile names and UUIDs.
- Windows QA tooling: a Windows Sandbox harness (offline, online, online with GPU), an environment report and a before/after portability audit.
- Launching has been validated on a development PC with a real GPU for modern Vanilla, Vanilla 1.12.2 (Java 8), Fabric, Forge, NeoForge and Quilt, including duplicate-launch protection, hiding and showing the window, surviving a closed launcher, and recovery after a crashed game.

### In progress

- Windows code signing. The executable is not signed; Windows Smart App Control currently blocks an unsigned `Clade.exe` in Windows Sandbox, and SmartScreen may warn. Signing is not implemented and the provider is not chosen yet.
- Final portable release packaging and clean-machine verification of the release package, including the bundled WebView2 runtime (its redistribution approval is still pending).
- GitHub Actions release automation: the tag-driven draft-release workflow, the packaging tools and the validation exist (see `docs/RELEASE.md`); the first real run is still to come.
- Portable self-update design and implementation.
- Changing the skin and managing owned capes (the account's current skin is only displayed for now).

## Release policy

Clade follows semantic versioning where practical.

The first public build is planned as a preview release rather than `1.0.0`. Breaking changes may occur during the preview period.
