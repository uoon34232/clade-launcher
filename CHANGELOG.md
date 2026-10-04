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

### In progress

- Complete English and Russian localization.
- Minecraft Services AppID approval and final authenticated end-to-end launch verification.
- Public source-code audit and publication.
- Final portable release packaging and clean-machine verification.
- GitHub Actions release automation.
- Portable self-update design and implementation.
- Official skin and owned-cape management after real authenticated account verification.

## Release policy

Clade follows semantic versioning where practical.

The first public build is planned as a preview release rather than `1.0.0`. Breaking changes may occur during the preview period.
