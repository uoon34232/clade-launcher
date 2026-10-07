# Security Policy

Clade Launcher is still in pre-release development. Security issues are taken seriously, especially those involving authentication, credentials, portable storage, downloads, archive extraction, process launching, or update integrity.

## Supported versions

There is no stable public release yet.

Until the first preview is published, security fixes apply to the current development version only. After releases begin, this document will be updated with an explicit supported-version policy.

## Reporting a vulnerability

Ordinary bugs belong in [GitHub Issues](https://github.com/uoon34232/clade-launcher/issues/new/choose). Use the steps below only for a security-sensitive problem.

Please do **not** publish secrets, access tokens, refresh tokens, vault contents, personal account data, or exploit details in a public issue.

If GitHub private vulnerability reporting is enabled for this repository, use the repository's **Security** tab to report the issue privately.

If no private reporting channel is available yet, open a minimal public issue requesting a private security contact **without including sensitive details**.

Useful information includes:

- affected Clade version or commit;
- Windows version;
- a concise description of the issue;
- reproduction steps that do not expose real credentials;
- expected and actual behavior;
- whether the issue can write outside the portable Clade directory;
- whether authentication material, account data, or downloaded files are affected.

## Scope

Security-sensitive areas include:

- Microsoft / Xbox / Minecraft authentication;
- the encrypted portable credential vault;
- token refresh and account removal;
- portable filesystem boundaries;
- downloader integrity and checksum verification;
- archive and native extraction;
- Java runtime management;
- Forge / NeoForge installer sandboxing;
- Minecraft process construction and environment isolation;
- log redaction;
- future self-update verification.

## Secrets and logs

Never attach real authentication tokens or vault files to an issue.

When sharing logs, inspect them first for:

- account identifiers you consider private;
- local filesystem paths;
- access or refresh tokens;
- passphrases;
- other personal information.

Clade is designed to redact authentication secrets from its own logs, but reports should still be reviewed before publication.

## Security design goals

Clade aims to:

- use official authentication and service endpoints;
- avoid cracked or bypass authentication modes;
- keep launcher-controlled persistent data inside the portable directory;
- avoid shell-based game launching;
- verify downloaded artifacts before use;
- reject unsafe archive paths;
- isolate third-party loader installation where practical;
- keep secrets out of ordinary settings, account metadata, and logs.

These goals are not a guarantee that the software is free of vulnerabilities.

## Disclosure

Please allow reasonable time for investigation and remediation before publishing exploit details.
