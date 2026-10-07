# Changelog

All notable changes to Clade Launcher will be documented in this file.

The project is currently pre-release. The first public preview is 0.1.0; its entry below is the text of its release notes.

## Release policy

Clade follows semantic versioning where practical.

The first public build is planned as a preview release rather than `1.0.0`. Breaking changes may occur during the preview period.

## [Unreleased]

## [0.1.0]

Before you run it: the Preview targets **Windows 11 x64** only. Windows 10 is not supported by this Preview. Clade is not code-signed, so Windows SmartScreen may warn about `Clade.exe`, and Windows 11 Smart App Control in enforcement mode may block it completely, with no "Run anyway" option. Please do not disable or bypass Windows security features to run Clade. Report bugs in [GitHub Issues](https://github.com/uoon34232/clade-launcher/issues/new/choose) and never post passwords, sign-in codes, tokens or vault files.

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
- Changing your Minecraft skin and capes from the account dialog: upload a 64x64 or 64x32 PNG as Classic or Slim, restore the default skin, and equip or hide the capes the account owns. The change is made on the Minecraft account itself, so it shows in the game and other launchers.
- Preparing, verifying and launching a profile require a ready active account; without one the main button reads "Sign in to continue". Creating and browsing profiles still works.
- "Remove from Clade" takes a profile out of the list and keeps its whole folder, worlds included, under `instances/.removed/`. Nothing is deleted.
- The dock's profile button shows only the profile's name, and the profile and account menus are denser.
- Exported diagnostics redact the known Minecraft profile names and UUIDs.
- Windows QA tooling: a Windows Sandbox harness (offline, online, online with GPU), an environment report and a before/after portability audit.
- Launching has been validated on a development PC with a real GPU for modern Vanilla, Vanilla 1.12.2 (Java 8), Fabric, Forge, NeoForge and Quilt, including duplicate-launch protection, hiding and showing the window, surviving a closed launcher, and recovery after a crashed game.

### Known limitations

- Windows 11 x64 only. Windows 10 and other platforms are not supported by this Preview.
- Not code-signed. SmartScreen may warn, and Smart App Control in enforcement mode may block `Clade.exe` outright.
- No self-update yet: download a new release to update.
- Skin and cape changes are new and have not yet been tried against a real Minecraft account, so report anything odd.
- Launching needs a network connection once the saved Minecraft sign-in has expired. There is no offline mode.
