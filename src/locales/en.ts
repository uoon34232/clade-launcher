// English: the canonical catalog. Every key here must also exist in `ru.ts` (a test and the
// compiler both enforce it). Add new user-visible text here and in Russian in the same change.
//
// Conventions: sentence case, short and plain; complete sentences with named `{params}` (never
// glue fragments together); counted nouns use plural forms (`by` names the numeric parameter).
// Product identifiers (Minecraft versions, Fabric, Forge, NeoForge, Quilt, UUIDs, paths) and the
// brand names Clade / Clade Launcher are never translated and do not appear here as copy.

import type { Message } from "@/lib/i18n/core";

export const en = {
  // ---- common
  "common.cancel": "Cancel",
  "common.close": "Close",
  "common.done": "Done",
  "common.tryAgain": "Try again",
  "common.working": "Working…",
  "common.continue": "Continue",
  "common.unlock": "Unlock",
  "common.passphrase": "Passphrase",
  "common.signInAgain": "Sign in again",
  "common.openLogs": "Open logs",

  // ---- window chrome
  "window.minimize": "Minimize",
  "window.close": "Close",
  "window.wordmark": "Show the Clade tagline",

  // ---- home
  "hero.subtitle": "Choose a build and enter your world.",
  "hero.shuffleHint": "Shows a different message",
  "hero.played": "Played {time}",
  "phrase.ready-when-you-are": "Ready when you are.",
  "phrase.your-world-is-waiting": "Your world is waiting.",
  "phrase.welcome-back": "Welcome back.",
  "phrase.pick-up-where-you-left-off": "Pick up where you left off.",
  "phrase.where-to-today": "Where to today?",
  "phrase.one-more-world": "One more world?",
  "phrase.take-your-time": "Take your time.",
  "phrase.choose-your-world": "Choose your world.",

  // ---- durations and sizes
  "playtime.zero": "0m",
  "playtime.lessThanMinute": "<1m",
  "playtime.minutes": "{minutes}m",
  "playtime.hoursMinutes": "{hours}h {minutes}m",
  "size.kb": "{value} KB",
  "size.mb": "{value} MB",
  "size.gb": "{value} GB",

  // ---- dock: play button
  "play.play": "Play",
  "play.setup": "Set up",
  "play.launching": "Launching…",
  "play.playing": "Playing",
  "play.repairing": "Repairing…",
  "play.signIn": "Sign in to continue",

  // ---- dock: settings button
  "dock.settings": "Settings",

  // ---- dock: account menu
  "account.signIn": "Sign in",
  "account.trigger": "Account: {name}. Switch account",
  "account.triggerNone": "Add an account",
  "account.needsSignIn": "Sign in again",
  "account.manage": "Manage {name}",
  "account.add": "+ Add account",

  // ---- dock: profile selector
  "profile.createFirst": "Create a profile",
  "profile.trigger": "Profile: {label}. Change profile",
  "profile.create": "+ Create profile",
  "profile.settings": "Profile settings…",
  "profile.openFolder": "Open profile folder",
  "profile.openInstancesFolder": "Open profiles folder",
  "profile.unreadableNotice": {
    by: "count",
    one: "1 profile folder couldn’t be read ({folders}). Its files were left untouched.",
    other: "{count} profile folders couldn’t be read ({folders}). Their files were left untouched.",
  } satisfies Message,
  "profile.verify": "Verify files",
  "profile.remove": "Remove from Clade…",
  "removeProfile.title": "Remove “{profile}” from Clade?",
  "removeProfile.body":
    "The profile will disappear from Clade, but its worlds, mods, settings, screenshots and other files will be kept inside the Clade folder. Nothing will be permanently deleted.",
  "removeProfile.confirm": "Remove from Clade",
  "removeProfile.working": "Removing…",
  "profile.vanilla": "Vanilla",
  "profile.modded": "Modded",

  // ---- profile settings
  "profileModal.details": "Profile",
  "profileModal.saveName": "Save",
  "profileModal.saving": "Saving…",
  "profileModal.status": "Status",
  "profileModal.statusReady": "Ready",
  "profileModal.statusPending": "Needs setup",
  "profileModal.playtime": "Playtime",
  "profileModal.changeVersion": "Change version…",
  "profileModal.saveVersion": "Save version",
  "profileModal.loaderVersionFor": "{loader} version",
  "profileModal.versionKeepNote":
    "Worlds and options stay in place. Clade will set up the new version before you play.",
  "profileModal.versionModdedNote":
    "Worlds and options stay in place. Check that any mods in your mods folder match the new version.",
  "profileModal.duplicate": "Duplicate",
  "profileModal.duplicating": "Duplicating…",
  "profileModal.memoryDefault": "Launcher default",
  "profileModal.memoryDefaultNote": "Follows Settings ({value} on this computer).",
  "profileModal.files": "Files",
  "profileModal.filesNoteVanilla":
    "Worlds, resource packs, screenshots and options live in this profile’s folder.",
  "profileModal.filesNoteModded":
    "Place compatible mods in the mods folder inside this profile’s folder.",

  // ---- create profile
  "create.title": "New profile",
  "create.description": "Each profile keeps its own files, separate from the rest.",
  "create.name": "Name",
  "create.namePlaceholder": "Survival",
  "create.mcVersion": "Minecraft version",
  "create.filterLabel": "Which versions to list",
  "create.filter.releases": "Releases",
  "create.filter.snapshots": "Snapshots",
  "create.filter.historical": "Historical",
  "create.choose": "Choose a version",
  "create.loading": "Loading versions…",
  "create.unavailable": "Versions unavailable",
  "create.noVersions": "No versions in this list.",
  "create.type": "Type",
  "create.typeLabel": "Profile type",
  "create.vanillaNote": "Plain Minecraft, nothing added.",
  "create.moddedNote": "Minecraft with a mod loader. You add your own mods.",
  "create.loader": "Loader",
  "create.loaderLabel": "Mod loader",
  "create.loaderCheckFailed": "Could not check which loaders fit. Check your connection.",
  "create.loaderChecking": "Checking compatible loaders…",
  "create.loaderNone": "{loader} has no version for {minecraft}",
  "create.loaderNoneAny": "{loader} has no version for this version",
  "create.loaderUnchecked": "{loader} could not be checked",
  "create.loaderVersion": "Loader version",
  "create.versionRecommended": "{version} (recommended)",
  "create.channel.recommended": "recommended",
  "create.channel.latest": "latest",
  "create.channel.prerelease": "beta",
  "create.channel.stable": "",
  "create.checking": "Checking…",
  "create.noCompatible": "No compatible version",
  "create.staleLoaders": "Offline: using a saved list of versions.",
  "create.creating": "Creating…",
  "create.submit": "Create profile",
  "validate.nameEmpty": "Give the profile a name.",
  "validate.nameLong": {
    by: "max",
    other: "Keep the name under {max} characters.",
  } satisfies Message,
  "validate.nameControl": "The name can't contain control characters.",
  "validate.nameDuplicate": "You already have a profile with this name.",
  "catalog.stale": "Offline: using the version list from {date}.",
  "catalog.staleUnknown": "Offline: using a saved version list.",

  // ---- preparing a profile
  "prepare.stage.planning": "Preparing Minecraft",
  "prepare.stage.java": "Preparing Java",
  "prepare.stage.java.verify": "Verifying Java",
  "prepare.stage.minecraft": "Downloading Minecraft",
  "prepare.stage.minecraft.verify": "Verifying Minecraft",
  "prepare.stage.libraries": "Downloading libraries",
  "prepare.stage.libraries.verify": "Verifying libraries",
  "prepare.stage.assets": "Downloading assets",
  "prepare.stage.assets.verify": "Verifying assets",
  "prepare.stage.natives": "Extracting natives",
  "prepare.stage.loader": "Installing {loader}",
  "prepare.stage.verifying": "Verifying files",
  "prepare.stage.done": "Ready to play.",
  "prepare.ready": "Ready to play.",
  "prepare.readyDescription": "{name} is installed on this computer.",
  "prepare.progress": "Setup progress",
  "prepare.files": {
    by: "total",
    other: "{done} of {total} files",
  } satisfies Message,
  "prepare.bytes": "{done} of {total}",

  // ---- when the game could not start or ended badly
  "problem.endedTitle": "Minecraft stopped",
  "problem.title.no_account": "Add an account",
  "problem.title.needs_sign_in": "Sign in again",
  "problem.title.needs_repair": "Repair needed",
  "problem.title.approval_pending": "Sign-in not available yet",
  "problem.title.offline": "No connection",
  "problem.title.other": "Can't start",
  "problem.repair": "Repair",
  "problem.addAccount": "Add account",
  "problem.unlockTitle": "Unlock your accounts",
  "problem.unlockDescription": "Enter your passphrase to play. It stays on this computer.",
  "exit.wrong_java":
    "Minecraft or a mod needs a different Java version than the one Clade prepared. Repairing the profile may help.",
  "exit.incomplete_files":
    "Minecraft's files are incomplete. Repair the profile to download them again.",
  "exit.no_memory":
    "There isn't enough free memory to start Minecraft. Close other programs and try again.",
  "exit.out_of_memory": "Minecraft ran out of memory.",
  "exit.native_library": "A native library (graphics or sound) could not be loaded.",
  "exit.java_crash": "The Java runtime crashed, often because of a graphics driver.",
  "exit.mod_loader": "The mod loader reported an error while starting.",
  "exit.mixin": "A mod could not be applied. Check the mods in this profile.",
  "exit.incompatible_mods": "The mods in this profile don't work together.",
  "exit.missing_class": "A library or mod could not be loaded.",
  "exit.jvm_start": "Java could not start with the chosen settings.",
  "exit.early_exit": "Minecraft stopped right after starting (exit code {code}).",
  "exit.unexpected": "Minecraft stopped unexpectedly (exit code {code}).",
  "exit.stopped": "Minecraft was stopped.",
  "exit.generic": "Minecraft stopped unexpectedly.",

  // ---- sign-in and the account vault
  "signin.addTitle": "Add account",
  "signin.againTitle": "Sign in again",
  "signin.addDescription": "Sign in with your Microsoft account in your browser.",
  "signin.againDescription": "Sign in as {name} with Microsoft.",
  "signin.stage.opening": "Opening Microsoft sign-in…",
  "signin.stage.waiting": "Waiting for sign-in…",
  "signin.stage.finishing": "Finishing setup…",
  "signin.createTitle": "Protect your accounts",
  "signin.createDescription":
    "Sign-in details are encrypted in this folder with a passphrase you choose. It can't be recovered, so keep it safe.",
  "signin.unlockTitle": "Unlock your accounts",
  "recovery.link": "Forgot passphrase?",
  "recovery.title": "Start fresh?",
  "recovery.body":
    "Clade can’t recover a forgotten passphrase. Starting fresh archives the old encrypted sign-in store and stops using those saved sign-ins in this Clade folder. You’ll sign in to your Microsoft accounts again. Your profiles, worlds, playtime and settings are kept.",
  "recovery.keep": "Keep trying",
  "recovery.confirm": "Start fresh",
  "signin.unlockDescription": "Enter your passphrase to sign in. It stays on this computer.",
  "signin.passphraseNote": {
    other: "At least {count} characters. A few words work well.",
  } satisfies Message,
  "signin.repeat": "Repeat passphrase",
  "signin.mismatch": "The passphrases don’t match.",
  "signin.approvalNote":
    "Nothing was saved. This resolves once Mojang approves the launcher’s registration.",

  // ---- account management
  "accountModal.description": "Minecraft Java account",
  "accountModal.profile": "Profile",
  "accountModal.skin": "Skin",
  "accountModal.cape": "Cape",
  "accountModal.name": "Name",
  "accountModal.uuid": "UUID",
  "accountModal.status": "Status",
  "accountModal.statusNeedsSignIn": "Needs to sign in again",
  "accountModal.statusSignedIn": "Signed in",
  "accountModal.skinModel": "Skin model",
  "accountModal.skinClassic": "Classic",
  "accountModal.skinSlim": "Slim",
  "accountModal.choosePng": "Choose PNG…",
  "accountModal.restoreDefault": "Restore default",
  "accountModal.uploading": "Uploading…",
  "accountModal.resetConfirm":
    "Restore the default skin? Your current custom skin will be removed from this Minecraft account.",
  "accountModal.keepSkin": "Keep current skin",
  "accountModal.restoring": "Restoring…",
  "accountModal.skinModelNote": "The model applies to the PNG you choose next.",
  "accountModal.capeAvailable": "Choose one of the capes this account owns, or turn the cape off.",
  "appearance.loading": "Loading…",
  "appearance.noCapes": "No capes available for this account.",
  "appearance.capeUnnamed": "Cape {n}",
  "appearance.equip": "Equip",
  "appearance.equipped": "Equipped",
  "appearance.equipNamed": "Equip {name}",
  "appearance.hideCape": "Hide cape",
  "appearance.needsSignIn": "Sign in again to change your Minecraft appearance.",
  "accountModal.confirmRemove":
    "Remove {name}? Your profiles, worlds and playtime stay as they are.",
  "accountModal.keep": "Keep account",
  "accountModal.remove": "Remove account",
  "accountModal.removing": "Removing…",
  "accountModal.signOutRemove": "Sign out and remove…",

  // ---- settings
  "settings.title": "Settings",
  "settings.sound": "Sound effects",
  "settings.volume": "Volume",
  "settings.language": "Language",
  "settings.interface": "Interface",
  "settings.minecraft": "Minecraft",
  "settings.behavior": "Behavior",
  "settings.support": "Support",
  "settings.motion": "Reduced motion",
  "settings.motion.system": "Follow Windows",
  "settings.motion.on": "On",
  "settings.motion.off": "Off",
  "settings.transparency": "Transparency effects",
  "settings.transparency.system": "Follow Windows",
  "settings.transparency.on": "On",
  "settings.transparency.off": "Off",
  "settings.memory": "Memory",
  "settings.memory.auto": "Automatic (recommended)",
  "settings.memory.custom": "Custom",
  "settings.memory.autoNote": "Chosen automatically for this computer: {value}.",
  "settings.memory.slider": "Memory for Minecraft",
  "settings.memory.gb": "{value} GB",
  "settings.onLaunch": "When Minecraft starts",
  "settings.onLaunch.keep": "Keep Clade open",
  "settings.onLaunch.hide": "Hide Clade",
  "settings.onExit": "When Minecraft closes",
  "settings.onExit.show": "Show Clade again",
  "settings.onExit.hidden": "Stay hidden",
  "settings.onExit.hiddenNote":
    "A hidden Clade closes quietly with the game. Start it again any time.",
  "settings.openFolder": "Open Clade folder",
  "settings.exportDiagnostics": "Export diagnostics",
  "settings.exporting": "Exporting…",
  "settings.diagnosticsNote": "Diagnostics include no passwords, tokens or worlds.",
  "settings.diagnosticsDone": "Saved {name}.",
  "settings.reset": "Reset settings",
  "settings.resetConfirm":
    "Reset settings to their defaults? Your profiles, worlds, accounts and playtime stay as they are.",
  "settings.resetKeep": "Keep settings",
  "settings.resetDo": "Reset",
  "settings.cat": "Cat",
  "onboarding.step": "Step {current} of {total}",
  "onboarding.skip": "Skip",
  "onboarding.back": "Back",
  "onboarding.continue": "Continue",
  "onboarding.welcomeTitle": "Welcome to Clade",
  "onboarding.tagline": "Simple. Portable. Yours.",
  "onboarding.welcomeText": "A clean, portable launcher for Minecraft: Java Edition.",
  "onboarding.portableTitle": "Portable by design",
  "onboarding.portableText":
    "Everything Clade manages stays inside the Clade folder, so you can move it to another drive or compatible Windows PC.",
  "onboarding.readyTitle": "Ready to start",
  "onboarding.readyText": "Create a profile, add your Microsoft account, or just look around.",
  "onboarding.createProfile": "Create profile",
  "onboarding.addAccount": "Add Microsoft account",
  "onboarding.finish": "Finish",
  "error.settings.unknown": "That didn't work.",
  "error.settings.diagnostics_failed":
    "The diagnostics file couldn't be created. Check that the drive has space and isn't read-only.",
  "error.settings.invalid_memory":
    "That amount of memory isn't safe for this computer. Choose between {min} and {max}.",
  "error.settings.unsupported_value": "That setting isn't supported.",
  "error.settings.unsupported_language": "That language isn't supported.",
  "error.settings.open_failed": "Windows couldn't open that folder.",

  // ---- errors from the launcher's backend, worded by code (see lib/i18n/errors.ts)
  "error.generic": "Something went wrong.",
  "error.common.storage":
    "Clade couldn't read or write the files it needs. Check that its folder is writable.",
  "error.common.not_found": "That profile no longer exists.",
  "error.common.network_unavailable": "Networking is unavailable.",
  "error.common.backend_unavailable": "The launcher backend is not available here.",
  "error.common.unsupported_folder_path":
    "Clade can't run from this folder path. Move the Clade folder to a local drive path without special device prefixes and try again.",

  "error.launch.unknown": "Minecraft couldn't be started.",
  "error.launch.no_account": "Sign in with a Microsoft account to play.",
  "error.launch.needs_sign_in": "This account needs to sign in again before it can play.",
  "error.launch.needs_repair":
    "Part of this profile is missing or damaged. It needs to be repaired before it can start.",
  "error.launch.vault_locked": "Unlock your accounts to play.",
  "error.launch.offline":
    "Clade can't refresh your Minecraft sign-in without a network. Connect and try again.",
  "error.launch.approval_pending":
    "Mojang has not approved this launcher for Minecraft sign-in yet, so a fresh sign-in can't be completed.",
  "error.launch.no_java_profile": "This Microsoft account has no Minecraft: Java Edition profile.",
  "error.launch.auth": "Your Minecraft sign-in could not be refreshed.",
  "error.launch.not_ready": "This profile has to be set up before it can start.",
  "error.launch.unsupported": "This profile can't be started.",
  "error.launch.already_running": "This profile is already starting or running.",
  "error.launch.already_preparing": "This profile is being set up. Wait for it to finish.",
  "error.launch.spawn_failed": "Java could not be started.",

  "error.prepare.unknown": "Setup failed.",
  "error.prepare.offline":
    "Can't reach the network, and some files this profile needs aren't saved on this computer yet.",
  "error.prepare.network": "A download failed. Check your connection and try again.",
  "error.prepare.verification":
    "A downloaded file failed verification, so it was discarded. Try again.",
  "error.prepare.metadata": "Mojang's information for this version isn't available.",
  "error.prepare.unsupported": "This version can't be prepared.",
  "error.prepare.java": "Java couldn't be set up.",
  "error.prepare.loader": "The mod loader couldn't be installed.",
  "error.prepare.storage":
    "Files couldn't be saved. Check that the drive has space and isn't read-only.",
  "error.prepare.disk_space":
    "There isn't enough free space on the drive that holds Clade: about {required} is needed and {available} is available. Free some space and try again.",
  "error.prepare.account_required":
    "Sign in with a Minecraft account to download or verify game files. Your profiles stay as they are.",
  "error.prepare.busy": "Another profile is being set up. Wait for it to finish.",
  "error.prepare.already_running":
    "This profile is starting or running. Close the game before setting it up or verifying it.",

  "error.signIn.unknown": "Sign-in failed.",
  "error.signIn.timed_out": "Sign-in took too long and was stopped. Try again when you are ready.",
  "error.signIn.browser_unavailable": "Clade could not open your browser.",
  "error.signIn.in_progress": "A sign-in is already in progress.",
  "error.signIn.offline": "Could not reach Microsoft. Check your connection and try again.",
  "error.signIn.oauth": "Microsoft did not accept the sign-in.",
  "error.signIn.needs_sign_in": "This account needs to sign in again.",
  "error.signIn.no_xbox_account":
    "This Microsoft account has no Xbox account. Create one at xbox.com, then try again.",
  "error.signIn.xbox_region": "Xbox Live is not available in this account's country or region.",
  "error.signIn.adult_verification":
    "This account needs adult verification before it can sign in. Finish it on xbox.com.",
  "error.signIn.child_account":
    "This is a child account. An adult must add it to a Microsoft family before it can sign in.",
  "error.signIn.xbox_banned": "This Xbox account is banned.",
  "error.signIn.xsts": "Xbox sign-in failed (code {xstsCode}).",
  "error.signIn.approval_required":
    "Microsoft sign-in worked, but Mojang has not approved this launcher for Minecraft sign-in yet.",
  "error.signIn.no_java_profile": "This Microsoft account does not own Minecraft: Java Edition.",
  "error.signIn.no_java_profile_owned":
    "This account owns Minecraft but has no Java profile yet. Open the official Minecraft Launcher once to create it, then try again.",
  "error.signIn.bad_response": "Minecraft sign-in returned something unexpected.",
  "error.signIn.vault_locked": "Unlock your accounts first.",
  "error.signIn.storage": "Your accounts could not be saved.",
  "error.signIn.wrong_account":
    "That is a different Minecraft account from the one being signed in again.",
  "error.signIn.unknown_account": "That account no longer exists.",
  "error.signIn.sign_in_unavailable": "Microsoft sign-in isn't available in this build yet.",

  "error.vault.unknown": "That didn't work.",
  "error.vault.passphrase_mismatch": "That passphrase doesn't match.",
  "error.vault.passphrase_too_short": "That passphrase is too short.",
  "error.vault.passphrase_too_long": "That passphrase is too long.",
  "error.vault.vault_exists": "Accounts are already protected by a passphrase.",
  "error.vault.vault_unreadable": "The encrypted account store could not be opened.",
  "error.vault.vault_missing":
    "The encrypted account store is missing from this folder. Start fresh to sign in again.",
  "error.vault.sign_in_in_progress": "A sign-in is in progress. Finish or cancel it first.",
  "error.vault.appearance_busy": "A skin or cape change is still running. Try again in a moment.",
  "error.vault.game_starting": "A game is starting. Try again once it has started.",
  "error.vault.vault_unlocked": "Your accounts are already unlocked.",
  "error.vault.reset_failed":
    "Clade couldn’t archive the old sign-in store, so nothing was changed. Close anything using the Clade folder and try again.",
  "error.vault.accounts_not_updated":
    "The old sign-in store was archived, but the account list couldn’t be updated. Try Start fresh again.",

  "error.account.unknown": "Could not remove the account.",
  "error.account.unknown_account": "That account no longer exists.",

  "error.appearance.unknown": "That change didn’t go through.",
  "error.appearance.invalid_skin": "Choose a 64×64 or 64×32 PNG skin.",
  "error.appearance.needs_sign_in": "Sign in again to change your Minecraft appearance.",
  "error.appearance.vault_locked": "Unlock your accounts to change your Minecraft appearance.",
  "error.appearance.offline": "Connect to the internet to change your Minecraft appearance.",
  "error.appearance.rate_limited": "Too many skin changes. Try again later.",
  "error.appearance.unauthorized":
    "Minecraft didn’t accept your sign-in. Sign in again, then retry.",
  "error.appearance.forbidden": "Minecraft refused this change for this account.",
  "error.appearance.cape_not_owned": "This account doesn’t own that cape.",
  "error.appearance.service_unavailable":
    "Minecraft services aren’t available right now. Try again later.",
  "error.appearance.no_profile": "This account has no Minecraft: Java Edition profile.",
  "error.appearance.bad_response": "Minecraft sent an unexpected answer. Try again later.",
  "error.appearance.storage": "Your accounts could not be read.",
  "error.appearance.busy": "Another change is still running.",
  "error.appearance.unknown_account": "That account no longer exists.",

  "error.profile.unknown": "Could not create the profile.",
  "error.profile.duplicate_name": "A profile named “{name}” already exists.",
  "error.profile.unknown_minecraft":
    "Minecraft {minecraft} is not in Mojang’s official version list.",
  "error.profile.cannot_verify":
    "Can't check Minecraft versions right now. Connect once and try again.",
  "error.profile.cannot_verify_loader":
    "Can't check which {loader} versions fit Minecraft {minecraft} right now. Connect once and try again.",
  "error.profile.no_compatible_loader": "{loader} has no version for Minecraft {minecraft}.",
  "error.profile.loader_version_not_offered":
    "{loader} {version} is not a compatible version for Minecraft {minecraft}.",
  "error.profile.invalid_name": "That name can't be used.",
  "error.profile.storage": "Could not save the profile.",
  "error.profile.remove_failed":
    "The profile could not be moved. Close anything that uses its folder (Explorer, an editor) and try again. Nothing was changed.",
  "error.profile.remove_refused": "This profile can't be removed from here. Nothing was changed.",
  "error.profile.already_running": "This profile is starting or running. Close the game first.",
  "error.profile.busy": "This profile is being set up. Wait for it to finish.",
  "error.profile.invalid_memory":
    "That amount of memory isn't safe for this computer. Choose between {min} and {max}.",
  "error.profile.not_found": "That profile no longer exists.",
  "error.profile.duplicate_failed":
    "Could not duplicate the profile. Check available disk space and close anything using its folder.",
  "error.profile.open_failed": "Windows couldn't open that folder.",

  "error.catalog.unknown": "Could not load the version list.",
  "error.catalog.catalog_unavailable":
    "Could not load the version list. Check your connection and try again.",
} satisfies Record<string, Message>;
