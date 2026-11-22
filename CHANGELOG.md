# Changelog

All notable changes to Clade Launcher will be documented in this file.

The project is currently in pre-release (`0.2.0` follows the initial `0.1.0` preview); each version section below serves as its release notes.

## Release policy

Clade follows semantic versioning where practical.

The initial `0.x` releases are preview builds rather than `1.0.0`. Breaking changes may occur during the preview period.

## [Unreleased]

## [0.2.0]

Before you run it: the Preview targets **Windows 10 (64-bit) and Windows 11 (64-bit) (`x64`)**. Clade is not code-signed, so Windows SmartScreen may warn about `Clade.exe`, and Windows 11 Smart App Control in enforcement mode may block it completely, with no "Run anyway" option. Please do not disable or bypass Windows security features to run Clade. Report bugs in [GitHub Issues](https://github.com/uoon34232/clade-launcher/issues/new/choose) and never post passwords, sign-in codes, tokens or vault files.

### Added

- Warm editorial Dark theme (**"Graphite Paper"**) with a `Theme` selector in Settings (`Follow Windows`, `Light`, `Dark`), synchronized Windows 11 dark Mica backdrop (`DWMWA_USE_IMMERSIVE_DARK_MODE`), opaque dark paper fallback (`#1C1917`), and a theme-matched reveal veil for framed game mode.
- Verified in-app portable self-updater in Settings (`Check for updates` -> `Update` -> `Restart to update`): downloads `Clade-Launcher-Portable.zip` and `.sha256` exclusively from the isolated `UPDATE_DOWNLOAD_HOSTS` allowlist, checks free disk space, verifies both `.sha256` and any GitHub API digest via streamed SHA-256 hashing, unpacks through `safe_archive` against a fail-closed portable package allowlist, performs lock-aware staged replacement (`--apply-update`) with automatic rollback on error, and cleans up old staged/backup directories at startup.
- Bundled Cyrillic variable fonts (`Literata` and `Onest`) alongside `Fraunces` and `Instrument Sans` so Russian typography renders consistently offline.
- Animated framed game launch, native window reveal, and return-to-launcher close transitions.

### Changed

- Overhauled framed game mode (**Inside Clade**) to host Minecraft inside an owned borderless `WS_POPUP` surface instead of `WS_CHILD` reparenting, adding STA subclass safety, cursor-capture compatibility (`Back to Game` and raw mouse focus), `DwmFlush`-paced transitions, and exact stage-resolution matching on launch.
- Added frosted backdrop blur treatment for translucent glass modal dialogs and overlay backdrops when transparency effects are enabled.
- Polished English and Russian copy and compact `960x600` dialog layouts, including text wrapping/truncation fixes and a pinned `+ Add account` footer in `AccountMenu`.
- Expanded forbidden JVM argument exclusions to block additional heap, archive, flight-recorder, and diagnostic output flags (`-XX:Flags`, `-XX:MaxHeapSize`, `-XX:InitialHeapSize`, `-XX:StartFlightRecording`, `-XX:FlightRecorderOptions`, `-XX:DumpLoadedClassList`, `-XX:CompileCommandFile`, `-XX:SharedArchiveConfigFile`, `-XX:PerfDataSaveFile`, `-XX:ReplayDataFile`).

### Fixed

- Consolidated Windows appearance tracking onto a single permanent `glass-watch` WinRT `UISettings` MTA observer thread without calling `RoUninitialize`, eliminating WinRT activation factory unload access violations while tracking Light/Dark and transparency changes live.
- Eliminated white flash frames and layering artifacts during framed game open, reveal veil handoff, and close transitions, and ensured launcher controls and focus recover cleanly when the game closes.
- Fixed WCAG AA/AAA contrast and control theming across Light, Dark, Glass, and Windows Forced Colors modes, including dark-themed native range sliders and `Select` popovers, clear hover feedback pills on `Segmented` toggles and quiet/soft modal buttons in Light and Dark, and readable `Equipped` cape badges.
- Improved keyboard accessibility and modal state safety: 2D arrow-key navigation in `AccountMenu`, accessible alert/form error links, focus recovery on dialog close, in-flight/double-submit guards across dialogs, and a CSS minification fix (`transform: none !important`) keeping `.settings-gear` stationary under Reduced Motion.
- Hardened backend network, storage, auth, and lifecycle paths: propagated HTTP redirect policy rejection errors, switched encrypted vault `.bak` writes to atomic writes with link checks and error propagation on stale backup cleanup failures, left explicit AppContainer NULL DACLs untouched during Windows 10 WebView2 ACL setup, rejected non-origin-form OAuth loopback targets and dead IPv6 listener loops while moving browser launches to `spawn_blocking`, enforced `reject_links_between` checks when opening support folders, latched window-closing guards before game spawn, prioritized download cancellation during retries, aligned runtime preview version reporting (`CLADE_RELEASE_TAG` / `CARGO_PKG_VERSION`), and fixed UTF-8-safe diagnostics truncation.

### Known limitations

- 64-bit Windows 10 and Windows 11 (`x64`) only. On Windows 10, FAT32 and exFAT drives do not support persistent ACLs, so Clade cannot grant the bundled WebView2 runtime's AppContainer permissions on those drives and falls back to the PC's installed Evergreen WebView2 runtime; prefer NTFS if a clean Windows 10 PC may not have WebView2 installed.
- Not code-signed. SmartScreen may warn, and Smart App Control in enforcement mode may block `Clade.exe` outright.
- No background auto-update: checking for and installing updates runs strictly on demand in Settings.
- Skin and cape changes have not yet been exercised against a live Minecraft account, so report anything odd.
- Launching needs a network connection once the saved Minecraft sign-in has expired. There is no offline mode.

## [0.1.0]

Before you run it: the Preview targets **Windows 10 (64-bit) and Windows 11 (64-bit) (`x64`)**. Clade is not code-signed, so Windows SmartScreen may warn about `Clade.exe`, and Windows 11 Smart App Control in enforcement mode may block it completely, with no "Run anyway" option. Please do not disable or bypass Windows security features to run Clade. Report bugs in [GitHub Issues](https://github.com/uoon34232/clade-launcher/issues/new/choose) and never post passwords, sign-in codes, tokens or vault files.

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
- "Forgot passphrase?" with "Start fresh" on every passphrase unlock: the old encrypted sign-in store is archived under `data/retired-vaults/` (nothing is deleted), every account signs in again with a new passphrase, and profiles, worlds, playtime and settings are kept. It does not sign you out at Microsoft.
- "Remove from Clade" takes a profile out of the list and keeps its whole folder, worlds included, under `instances/.removed/`. Nothing is deleted.
- Per-profile settings and folder actions: rename any profile without moving or renaming its folder, set a per-profile game memory override in MiB (or follow the global / automatic setting), open the profile's `game/` folder directly from Clade, and surface unreadable profile folders in `instances/` as warnings without touching their files.
- Windows 10 (64-bit) (`x64`) support alongside Windows 11 (64-bit) (`x64`): automatic Win32 AppContainer read-and-execute ACL setup (`S-1-15-2-1` and `S-1-15-2-2`) on `runtimes/webview2/` for NTFS folders without administrator rights, Evergreen WebView2 fallback on non-ACL `FAT32`/`exFAT` volumes, and automatic fallback to the warm opaque paper window surface where Windows 11's DWM Mica backdrop is unavailable.
- The dock's profile button shows only the profile's name, and the profile and account menus are denser.
- Exported diagnostics redact the known Minecraft profile names and UUIDs.
- Windows QA tooling: a Windows Sandbox harness (offline, online, online with GPU), an environment report and a before/after portability audit.
- Launching has been validated on a development PC with a real GPU for modern Vanilla, Vanilla 1.12.2 (Java 8), Fabric, Forge, NeoForge and Quilt, including duplicate-launch protection, hiding and showing the window, surviving a closed launcher, and recovery after a crashed game.
- Framed game window mode (**Inside Clade**): by default, Clade reparents the native Minecraft window directly into Clade's frameless host window (`SetParent` and Win32 region cutouts so WebView2 never occludes the game, Minimize / Maximize-Restore / Close header strip, automatic window geometry restore on exit), alongside **Keep Clade open** and **Hide Clade**.
- **Simple** and **Advanced** interface modes: Clade opens in a calm Simple mode by default; switching to Advanced mode in Settings unlocks snapshot and historical version filters, manual loader version choice, reduced-motion and Windows 11 Mica transparency controls, launch/exit window behavior, diagnostics export, and per-profile memory, initial window size (`320×240`–`7680×4320`), and allowlisted JVM arguments.
- In-place version switching and profile duplication: switch a profile to another Minecraft or compatible loader version while keeping its worlds and settings, or duplicate any profile into an independent copy before testing changes.
- Restoring removed profiles: restore any profile kept under `instances/.removed/` in one click (with automatic name and folder collision resolution) from the profile menu or empty-state banner, or open `instances/.removed/` in File Explorer while keeping Clade's rule that no command ever deletes profile files.
- Clearing archived sign-in vaults: explicitly delete old archived vaults under `data/retired-vaults/` from Settings -> Support (with confirmation, shown only when archived vaults exist) without touching the active `vault.bin`.
- Manual update check in Settings: **Check for updates** queries GitHub Releases strictly on click (zero background requests, isolated `api.github.com` policy) and opens the canonical release page when a newer version is available.
- Smaller release binary and WebView2 disk footprint: code-split modal surfaces, WebView2 cache and V8 heap limits, and startup cleanup of disposable Chromium component-updater bloat in `data/webview2`.

### Known limitations

- 64-bit Windows 10 and Windows 11 (`x64`) only. On Windows 10, FAT32 and exFAT drives do not support persistent ACLs, so Clade cannot grant the bundled WebView2 runtime's AppContainer permissions on those drives and falls back to the PC's installed Evergreen WebView2 runtime; prefer NTFS if a clean Windows 10 PC may not have WebView2 installed.
- Not code-signed. SmartScreen may warn, and Smart App Control in enforcement mode may block `Clade.exe` outright.
- No automatic self-update yet: use **Check for updates** in Settings and extract the new portable ZIP.
- Skin and cape changes are new and have not yet been tried against a real Minecraft account, so report anything odd.
- Launching needs a network connection once the saved Minecraft sign-in has expired. There is no offline mode.
