// Facts shown in the Settings footer that are not translated. The version itself is not kept here:
// it comes from the backend (Cargo.toml is the single source of truth).

export const COPYRIGHT_LINE = "© 2026";

/** "v0.1.0", or nothing while the version is unknown. */
export const versionLabel = (version: string): string => (version ? `v${version}` : "");
