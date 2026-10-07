# Contributing to Clade Launcher

Clade Launcher is in pre-release development and its source code is public. Contributions are welcome when they fit the product direction below.

## Ways to help

- report reproducible launcher bugs;
- point out documentation problems;
- test public preview builds when they become available;
- propose focused improvements that fit Clade's product direction.

Please avoid opening requests for a built-in mod marketplace, advertising feed, account bypass, cracked/offline authentication, or unrelated platform expansion. Those conflict with the current project scope.

## Building and checking

The prerequisites and build steps are in the [technical documentation](docs/TECHNICAL.md#2-building-from-source), which also describes the architecture and the portability, security and product rules. Please read the relevant sections before touching storage, accounts, downloads, extraction, process launch or the interface text.

Run these before opening a pull request, and make sure they pass. The same checks run automatically in CI on every pull request.

```bash
pnpm check                              # typecheck + eslint + vitest
pnpm build                              # production build of the frontend
pnpm test:e2e                           # browser end-to-end tests
cd src-tauri
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The end-to-end tests need Playwright's Chromium. Install it once with `pnpm exec playwright install chromium`. They use a stand-in backend with synthetic data, so they never need a Microsoft account or a network connection ([how they work](docs/TECHNICAL.md#17-automated-guards-and-tests)).

The guard tests in `src-tauri/tests/` and the frontend scans enforce the product, portability and localization rules. Do not weaken them to make a change pass; if a rule has to change, change it deliberately and explain why.

## Product direction

Clade is intentionally:

- Windows-focused;
- portable;
- Vanilla-first;
- minimal;
- based on legitimate Microsoft / Minecraft accounts;
- free of built-in modpack, shader, resource-pack, and content marketplaces.

Optional modded profiles are supported through Fabric, Forge, NeoForge, and Quilt, but users provide their own content.

## Development principles

Changes should preserve these constraints:

- launcher-controlled persistent data remains inside the portable Clade folder;
- no dependency on system Java, `JAVA_HOME`, the Windows registry, or a global `.minecraft`;
- account state remains independent from Minecraft profile state;
- secrets never belong in ordinary JSON settings, logs, screenshots, fixtures, or commits;
- downloads and extracted paths must be validated;
- production authentication must use the official Microsoft / Xbox / Minecraft flow;
- there is no guest, cracked, or authentication-bypass mode;
- every user-facing string lives in the localization catalogs (`src/locales/en.ts` and `ru.ts`) in both English and Russian, and Clade ships exactly those two languages;
- internal developer logs and code comments stay in English;
- UI changes should preserve Clade's restrained visual language rather than introducing a generic gaming dashboard.

## Issues

Report bugs through [GitHub Issues](https://github.com/uoon34232/clade-launcher/issues/new/choose); the bug form lists what to include. When reporting a bug, include:

- what you were trying to do;
- exact steps to reproduce it;
- what you expected;
- what happened instead;
- Windows version;
- Clade version or commit when known;
- relevant redacted logs.

Do not post authentication tokens, vault contents, passphrases, or other credentials.

For security issues, follow [SECURITY.md](SECURITY.md).

## Pull requests

Pull requests should:

- stay focused on one problem;
- include tests for behavioral changes;
- preserve portability and security guards;
- pass formatting, linting, TypeScript, frontend tests, Rust tests, Clippy, and the project guard tests;
- avoid unrelated redesign or refactoring.

Visual changes should keep the launcher calm and minimal; attach a screenshot taken from the real application, without personal data.

## Licensing

Clade Launcher is licensed under the [Apache License 2.0](LICENSE). By submitting a contribution you agree that it is provided under the same license.
