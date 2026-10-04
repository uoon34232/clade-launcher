# Contributing to Clade Launcher

Clade Launcher is currently in pre-release development. The full source tree has not yet been published to this repository, so contribution workflows are intentionally lightweight for now.

## Before the source publication

You can help by:

- reporting reproducible launcher bugs;
- pointing out documentation problems;
- testing public preview builds when they become available;
- suggesting focused improvements that fit Clade's product direction.

Please avoid opening requests for a built-in mod marketplace, advertising feed, account bypass, cracked/offline authentication, or unrelated platform expansion. Those conflict with the current project scope.

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
- user-facing strings must exist in both English and Russian once localization lands;
- internal developer logs may remain in English;
- UI changes should preserve Clade's restrained visual language rather than introducing a generic gaming dashboard.

## Issues

When reporting a bug, include:

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

A full code-contribution workflow will be documented when the source tree is published.

Once source contributions are enabled, pull requests should:

- stay focused on one problem;
- include tests for behavioral changes;
- preserve portability and security guards;
- pass formatting, linting, TypeScript, frontend tests, Rust tests, Clippy, and project guard tests;
- avoid unrelated redesign or refactoring.

## Licensing

The repository license will be selected and published before accepting general source-code contributions.

Until then, do not assume that repository contents are licensed for redistribution or derivative works.
