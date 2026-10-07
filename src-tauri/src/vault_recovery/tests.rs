#[cfg(test)]
use super::*;
use crate::accounts::{AccountMeta, AccountStatus};
use crate::instances::InstanceId;
use crate::portable::root::PortableRoot;
use crate::profile_lock::{Activity, ProfileLocks};
use crate::vault::{AccountSecrets, KdfParams, Vault};
use std::collections::BTreeMap;

const FAST: KdfParams = KdfParams {
    m_cost_kib: 8,
    t_cost: 1,
    p_cost: 1,
};
const PASS: &str = "correct horse battery";
const NEW_PASS: &str = "a brand new passphrase";
const NOW: i64 = 1_700_000_000;

struct World {
    dir: tempfile::TempDir,
    auth: AuthState,
    accounts: AccountsState,
    locks: Arc<ProfileLocks>,
    launch: LaunchState,
}

impl World {
    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }
    fn vault(&self) -> PathBuf {
        self.data().join("vault.bin")
    }
    fn bak(&self) -> PathBuf {
        self.data().join("vault.bin.bak")
    }
    fn accounts_file(&self) -> PathBuf {
        self.data().join("accounts.json")
    }
    fn retired(&self) -> Vec<PathBuf> {
        match fs::read_dir(self.data().join(RETIRED_DIR)) {
            Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }
    fn list(&self) -> AccountsFile {
        AccountsFile::load(&self.accounts_file()).unwrap()
    }
    fn reset(&self) -> Result<(), ResetError> {
        reset(
            &self.auth,
            &self.accounts,
            &|| self.launch.any_starting(),
            NOW,
        )
    }
    /// Everything that must never change: every file outside the credential files and the account list.
    fn user_data(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        walk(self.dir.path(), self.dir.path(), &mut out);
        out.retain(|k, _| {
            !(k == "data/accounts.json"
                || k.starts_with("data/vault")
                || k.starts_with(&format!("data/{RETIRED_DIR}/")))
        });
        out
    }
}

fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(base, &p, out);
        } else {
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, fs::read(&p).unwrap());
        }
    }
}

fn meta(id: &str, name: &str, uuid: &str, status: AccountStatus) -> AccountMeta {
    AccountMeta {
        id: id.into(),
        username: name.into(),
        minecraft_uuid: uuid.into(),
        added_at: 5,
        last_used_at: Some(9),
        status,
    }
}

fn is_unknown_reauth(a: &AccountMeta) -> bool {
    matches!(
        a.status,
        AccountStatus::NeedsReauth {
            reason: ReauthReason::Unknown,
            ..
        }
    )
}

/// A Clade folder with two accounts, a locked vault with a backup generation, and user data of
/// every kind next to it.
fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let files: [(&str, &[u8]); 12] = [
        ("instances/vanilla/instance.json", b"{\"id\":\"vanilla\"}"),
        (
            "instances/vanilla/game/saves/World/level.dat",
            &[0x1f, 0x8b, 1, 2, 3],
        ),
        ("instances/vanilla/game/mods/m.jar", b"PK"),
        ("instances/vanilla/playtime.json", b"{\"schema\":1}"),
        ("data/settings.json", b"{\"language\":\"en\"}"),
        ("data/playtime.json", b"{\"total\":5}"),
        (
            "cache/skins/00000000-0000-4000-8000-00000000a001.png",
            b"png",
        ),
        (
            "cache/skins/00000000-0000-4000-8000-00000000a001.want",
            b"abcdef",
        ),
        ("runtimes/java/x/bin/java.exe", b"MZ"),
        ("libraries/a.jar", b"lib"),
        ("assets/objects/ab", b"asset"),
        ("minecraft/versions/1.21.4.json", b"{}"),
    ];
    for (rel, bytes) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    AccountsFile {
        schema: 1,
        active: Some("b".into()),
        accounts: vec![
            meta(
                "a",
                "Alex_1",
                "00000000-0000-4000-8000-00000000a001",
                AccountStatus::Ready,
            ),
            meta(
                "b",
                "Blake_2",
                "00000000-0000-4000-8000-00000000b002",
                AccountStatus::NeedsReauth {
                    reason: ReauthReason::Revoked,
                    since: 3,
                },
            ),
        ],
    }
    .save(&data.join("accounts.json"))
    .unwrap();
    // each save keeps the previous generation as vault.bin.bak
    let mut v = Vault::create_with(&data.join("vault.bin"), PASS, FAST).unwrap();
    for id in ["a", "b"] {
        v.set_account(
            id,
            AccountSecrets {
                ms_refresh_token: Some(format!("REFRESH-{id}")),
                minecraft_access_token: Some(format!("TOKEN-{id}")),
                minecraft_token_expires_at: Some(NOW + 9999),
            },
        )
        .unwrap();
    }
    drop(v); // locked
    let locks = Arc::new(ProfileLocks::default());
    World {
        auth: AuthState::new(data.join("vault.bin"), Err("no network in tests".into())),
        accounts: AccountsState::new(data.join("accounts.json")),
        launch: LaunchState::new(PortableRoot::at(root)).with_locks(locks.clone()),
        locks,
        dir,
    }
}

#[test]
fn the_old_vault_and_its_backup_are_retired_byte_for_byte_and_still_open() {
    let w = world();
    assert!(w.bak().is_file(), "the fixture has a backup generation");
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    w.reset().unwrap();
    // neither old credential file is left in the active place
    assert!(!w.vault().exists() && !w.bak().exists());
    let retired = w.retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(fs::read(retired[0].join("vault.bin")).unwrap(), vault);
    assert_eq!(fs::read(retired[0].join("vault.bin.bak")).unwrap(), bak);
    // nothing was destroyed: the retired vault opens with its old passphrase
    let old = Vault::unlock(&retired[0].join("vault.bin"), PASS).unwrap();
    assert_eq!(
        old.account("a").and_then(|s| s.ms_refresh_token.clone()),
        Some("REFRESH-a".to_owned())
    );
    // the folder is named <unix time>-<12 hex digits>, under data/retired-vaults
    let name = retired[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let prefix = format!("{NOW}-");
    assert!(name.starts_with(&prefix) && name.len() == prefix.len() + 12);
}

#[test]
fn every_account_signs_in_again_and_keeps_its_identity() {
    let w = world();
    let before = w.list();
    w.reset().unwrap();
    let after = w.list();
    assert_eq!(after.active, before.active);
    assert_eq!(after.accounts.len(), 2);
    for (b, a) in before.accounts.iter().zip(&after.accounts) {
        assert_eq!(
            (&a.id, &a.minecraft_uuid, &a.username),
            (&b.id, &b.minecraft_uuid, &b.username)
        );
        assert_eq!((a.added_at, a.last_used_at), (b.added_at, b.last_used_at));
        assert_eq!(
            a.status,
            AccountStatus::NeedsReauth {
                reason: ReauthReason::Unknown,
                since: NOW
            }
        );
    }
}

#[test]
fn profiles_worlds_playtime_settings_runtimes_and_the_skin_cache_are_byte_identical() {
    let w = world();
    let before = w.user_data();
    assert!(before.len() >= 12);
    w.reset().unwrap();
    assert_eq!(w.user_data(), before);
}

#[test]
fn a_second_reset_changes_nothing_more() {
    let w = world();
    w.reset().unwrap();
    let (list, retired) = (w.list(), w.retired());
    w.reset().unwrap();
    assert_eq!(w.list().accounts.len(), list.accounts.len());
    assert_eq!(w.retired(), retired, "no second retirement folder");
}

#[test]
fn a_missing_vault_is_repaired_without_an_empty_retirement_folder() {
    let w = world();
    fs::remove_file(w.vault()).unwrap();
    fs::remove_file(w.bak()).unwrap();
    w.reset().unwrap();
    assert!(w.retired().is_empty());
    assert!(!w.data().join(RETIRED_DIR).exists());
    assert!(w.list().accounts.iter().all(is_unknown_reauth));
}

#[test]
fn a_missing_vault_with_a_stale_backup_retires_the_backup() {
    let w = world();
    let bak = fs::read(w.bak()).unwrap();
    fs::remove_file(w.vault()).unwrap();
    w.reset().unwrap();
    assert!(
        !w.bak().exists(),
        "no stale backup stays in the active place"
    );
    let retired = w.retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(fs::read(retired[0].join("vault.bin.bak")).unwrap(), bak);
    assert!(!retired[0].join("vault.bin").exists());
}

#[test]
fn leftover_temporary_vault_files_are_retired_too() {
    let w = world();
    let tmp = w.data().join("vault.bin.0123456789ab.tmp");
    fs::write(&tmp, b"half written").unwrap();
    w.reset().unwrap();
    assert!(!tmp.exists());
    assert_eq!(
        fs::read(w.retired()[0].join("vault.bin.0123456789ab.tmp")).unwrap(),
        b"half written"
    );
}

#[test]
fn a_damaged_vault_can_be_started_fresh_too() {
    let w = world();
    fs::write(w.vault(), b"not a vault at all").unwrap();
    w.reset().unwrap();
    assert!(!w.vault().exists());
    assert_eq!(
        fs::read(w.retired()[0].join("vault.bin")).unwrap(),
        b"not a vault at all"
    );
}

#[test]
fn a_damaged_account_list_stops_everything_before_any_file_moves() {
    let w = world();
    fs::write(w.accounts_file(), b"{ not json").unwrap();
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    assert_eq!(w.reset(), Err(ResetError::Storage));
    assert_eq!(
        (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap()),
        (vault, bak)
    );
    assert!(!w.data().join(RETIRED_DIR).exists());
    assert_eq!(
        fs::read(w.accounts_file()).unwrap(),
        b"{ not json",
        "it is not repaired either"
    );
}

#[test]
fn a_missing_account_list_is_an_empty_one() {
    let w = world();
    fs::remove_file(w.accounts_file()).unwrap();
    w.reset().unwrap();
    assert!(!w.accounts_file().exists(), "no accounts file is invented");
    assert_eq!(w.retired().len(), 1);
}

#[test]
fn it_is_refused_while_the_vault_is_unlocked_and_changes_nothing() {
    let w = world();
    *lock(&w.auth.vault) = Some(Vault::unlock(&w.vault(), PASS).unwrap());
    let before = (
        fs::read(w.vault()).unwrap(),
        fs::read(w.accounts_file()).unwrap(),
    );
    assert_eq!(w.reset(), Err(ResetError::VaultUnlocked));
    assert_eq!(
        (
            fs::read(w.vault()).unwrap(),
            fs::read(w.accounts_file()).unwrap()
        ),
        before
    );
    assert!(lock(&w.auth.vault).is_some(), "the open vault stays open");
}

#[test]
fn it_is_refused_during_a_sign_in_a_skin_change_and_a_game_start_and_releases_its_claims() {
    let w = world();
    // a sign-in is running
    *lock(&w.auth.running) = Some(Arc::new(Notify::new()));
    assert_eq!(w.reset(), Err(ResetError::SignInInProgress));
    assert!(
        lock(&w.auth.running).is_some(),
        "the sign-in's own claim is left alone"
    );
    *lock(&w.auth.running) = None;
    // a skin or cape change is running
    let change = w.accounts.begin_appearance_change().unwrap();
    assert_eq!(w.reset(), Err(ResetError::AppearanceBusy));
    drop(change);
    // a game is starting
    let id = InstanceId::new("vanilla").unwrap();
    w.launch.claim_checked(&id).unwrap();
    assert_eq!(w.reset(), Err(ResetError::GameStarting));
    assert!(w.vault().is_file(), "every refusal left the vault in place");
    w.launch.release(&id);
    // a profile being prepared does not use credentials, so it does not stop it
    w.locks.claim("other", Activity::Preparing).unwrap();
    w.reset().unwrap();
    // and every temporary claim was released
    assert!(lock(&w.auth.running).is_none());
    assert!(w.accounts.begin_appearance_change().is_some());
}

#[cfg(windows)]
#[test]
fn a_failed_rename_leaves_the_active_vault_usable_and_puts_the_backup_back() {
    use std::os::windows::fs::OpenOptionsExt;
    let w = world();
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    // another program has the vault open without sharing: it cannot be renamed
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(w.vault())
        .unwrap();
    assert_eq!(w.reset(), Err(ResetError::ResetFailed));
    drop(held);
    assert_eq!(
        (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap()),
        (vault, bak)
    );
    assert!(
        w.retired().is_empty(),
        "the empty retirement folder is gone again"
    );
    assert!(
        w.list()
            .accounts
            .iter()
            .any(|a| matches!(a.status, AccountStatus::Ready)),
        "the accounts were not touched"
    );
    assert!(Vault::unlock(&w.vault(), PASS).is_ok());
}

#[test]
fn a_retirement_folder_that_cannot_be_made_leaves_everything_as_it_was() {
    let w = world();
    fs::write(w.data().join(RETIRED_DIR), b"a file in the way").unwrap();
    let before = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    assert_eq!(w.reset(), Err(ResetError::ResetFailed));
    assert_eq!(
        (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap()),
        before
    );
}

#[test]
fn a_credential_path_that_is_a_folder_is_never_touched() {
    let w = world();
    fs::remove_file(w.bak()).unwrap();
    fs::create_dir(w.bak()).unwrap();
    fs::write(w.bak().join("precious"), b"x").unwrap();
    assert_eq!(w.reset(), Err(ResetError::ResetFailed));
    assert_eq!(fs::read(w.bak().join("precious")).unwrap(), b"x");
    assert!(w.vault().is_file());
}

#[cfg(windows)]
#[test]
fn when_the_account_list_cannot_be_written_the_vault_stays_archived_and_a_retry_finishes() {
    let w = world();
    let mut perms = fs::metadata(w.accounts_file()).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(w.accounts_file(), perms).unwrap();
    assert_eq!(w.reset(), Err(ResetError::AccountsNotUpdated));
    // the vault is archived, and the accounts are as they were
    assert!(!w.vault().exists() && !w.bak().exists());
    assert_eq!(w.retired().len(), 1);
    assert!(w
        .list()
        .accounts
        .iter()
        .any(|a| matches!(a.status, AccountStatus::Ready)));
    // the retry only repeats the account step: no second archive, no loss
    let mut perms = fs::metadata(w.accounts_file()).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(w.accounts_file(), perms).unwrap();
    w.reset().unwrap();
    assert_eq!(w.retired().len(), 1);
    assert!(w.list().accounts.iter().all(is_unknown_reauth));
    assert!(w.retired()[0].join("vault.bin").is_file());
}

#[test]
fn after_recovery_a_new_vault_can_be_created_and_is_not_the_old_one() {
    let w = world();
    let old = Vault::unlock(&w.vault(), PASS).unwrap().generation();
    w.reset().unwrap();
    assert!(lock(&w.auth.vault).is_none(), "the vault stays locked");
    let fresh = Vault::create_with(&w.vault(), NEW_PASS, FAST).unwrap();
    assert_ne!(fresh.generation(), old);
    assert!(
        fresh.account("a").is_none(),
        "none of the old sign-ins came along"
    );
    drop(fresh);
    // the old passphrase does not open the new vault
    assert!(Vault::unlock(&w.vault(), PASS).is_err());
    assert!(Vault::unlock(&w.vault(), NEW_PASS).is_ok());
}

#[test]
fn unlocking_a_missing_vault_says_so_instead_of_looping() {
    let w = world();
    w.reset().unwrap();
    assert!(matches!(
        Vault::unlock(&w.vault(), PASS),
        Err(crate::vault::VaultError::Missing)
    ));
}

#[test]
fn the_recovery_never_deletes_a_file_or_a_folder_tree_and_uses_no_other_store() {
    let src = include_str!("../vault_recovery.rs");
    let production = src.split("#[cfg(test)]").next().unwrap();
    // only the empty retirement folder it just made may be removed, and only with `remove_dir`
    assert!(!production.contains("remove_file(") && !production.contains("remove_dir_all"));
    assert_eq!(production.matches("fs::remove_dir(").count(), 1);
    for forbidden in [
        "CredWrite",
        "CryptProtectData",
        "keyring",
        "APPDATA",
        "LOCALAPPDATA",
        "temp_dir",
        "localStorage",
    ] {
        assert!(!production.contains(forbidden), "{forbidden}");
    }
    // everything it logs carries only the operation and a result code
    for line in production.lines().filter(|l| l.contains("tracing::")) {
        assert!(line.contains("op = \"vault_reset\""), "{line}");
    }
}

#[test]
fn the_error_codes_are_stable_and_say_nothing_private() {
    for (e, code) in [
        (ResetError::VaultUnlocked, "vault_unlocked"),
        (ResetError::SignInInProgress, "sign_in_in_progress"),
        (ResetError::AppearanceBusy, "appearance_busy"),
        (ResetError::GameStarting, "game_starting"),
        (ResetError::ResetFailed, "reset_failed"),
        (ResetError::AccountsNotUpdated, "accounts_not_updated"),
        (ResetError::Storage, "storage"),
    ] {
        assert_eq!(e.code(), code);
        assert_eq!(e.coded().code, code);
        assert!(!e.to_string().contains('\\') && !e.to_string().contains('/'));
    }
}

#[test]
fn only_the_exact_temporary_name_of_an_interrupted_save_is_retired() {
    let w = world();
    let tmp = w.data().join("vault.bin.0123456789ab.tmp");
    fs::write(&tmp, b"real leftover").unwrap();
    // files that merely look similar are not credential files and are never moved (an upper-case twin
    // is left to the matcher test below: Windows treats it as the same file)
    let bystanders = [
        "vault.bin.notes.tmp",
        "vault.bin.0123456789a.tmp",
        "vault.bin.0123456789abc.tmp",
        "vault.bin.0123456789gz.tmp",
        "vault.bin.0123456789ab.tmp.txt",
        "vault.bin.bak.0123456789ab.tmp",
        "vault.binx.0123456789ab.tmp",
        "vault.bin..tmp",
        "vault.bin.tmp",
        "vault.bin.txt",
    ];
    for name in bystanders {
        fs::write(w.data().join(name), name.as_bytes()).unwrap();
    }
    w.reset().unwrap();
    assert!(!tmp.exists());
    assert_eq!(
        fs::read(w.retired()[0].join("vault.bin.0123456789ab.tmp")).unwrap(),
        b"real leftover"
    );
    for name in bystanders {
        assert_eq!(
            fs::read(w.data().join(name)).unwrap(),
            name.as_bytes(),
            "{name}"
        );
    }
    // and nothing else was retired
    assert_eq!(fs::read_dir(&w.retired()[0]).unwrap().count(), 3);
}

#[test]
fn the_temporary_name_matcher_accepts_exactly_what_the_vault_save_writes() {
    // `atomic_write` appends "." + 12 lowercase hex digits + ".tmp" to the file name
    assert!(is_save_temp("vault.bin.0123456789ab.tmp", "vault.bin"));
    assert!(is_save_temp("vault.bin.ffffffffffff.tmp", "vault.bin"));
    assert!(!is_save_temp("vault.bin.0123456789AB.tmp", "vault.bin"));
    assert!(!is_save_temp("x/vault.bin.0123456789ab.tmp", "vault.bin"));
    assert!(!is_save_temp("vault.bin.0123456789ab.tmp", "other.bin"));
}

#[test]
fn a_crash_after_the_backup_moved_but_before_the_vault_loses_nothing_and_a_retry_finishes() {
    let w = world();
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    // what a crash leaves: the backup is already in a retirement folder, the vault is still active
    let first = w.data().join(RETIRED_DIR).join("1699999999-aaaaaaaaaaaa");
    fs::create_dir_all(&first).unwrap();
    fs::rename(w.bak(), first.join("vault.bin.bak")).unwrap();
    assert!(Vault::unlock(&w.vault(), PASS)
        .unwrap()
        .account("a")
        .is_some());
    w.reset().unwrap();
    let all = w.retired();
    assert_eq!(all.len(), 2, "the retry used its own folder");
    // every credential byte is somewhere, and nothing is in the active place
    assert_eq!(fs::read(first.join("vault.bin.bak")).unwrap(), bak);
    let second = all.iter().find(|p| **p != first).unwrap();
    assert_eq!(fs::read(second.join("vault.bin")).unwrap(), vault);
    assert!(!w.vault().exists() && !w.bak().exists());
    assert!(w.list().accounts.iter().all(is_unknown_reauth));
}

#[test]
fn a_crash_after_the_vault_moved_but_before_the_accounts_leaves_a_missing_vault_that_start_fresh_repairs(
) {
    let w = world();
    // what a crash leaves: everything is archived, the account list still says "signed in"
    let dir = w.data().join(RETIRED_DIR).join("1699999999-bbbbbbbbbbbb");
    fs::create_dir_all(&dir).unwrap();
    fs::rename(w.bak(), dir.join("vault.bin.bak")).unwrap();
    fs::rename(w.vault(), dir.join("vault.bin")).unwrap();
    assert!(w
        .list()
        .accounts
        .iter()
        .any(|a| matches!(a.status, AccountStatus::Ready)));
    // the unlock does not loop: it says the vault is missing, and nothing refreshes or repairs by itself
    assert!(matches!(
        Vault::unlock(&w.vault(), PASS),
        Err(crate::vault::VaultError::Missing)
    ));
    assert_eq!(
        crate::auth::vault_error(&crate::vault::VaultError::Missing).code,
        "vault_missing"
    );
    // Start fresh finishes it: no new archive, accounts marked
    w.reset().unwrap();
    assert_eq!(w.retired().len(), 1);
    assert!(w.list().accounts.iter().all(is_unknown_reauth));
    assert!(Vault::unlock(&dir.join("vault.bin"), PASS).is_ok());
}

#[test]
fn a_rollback_that_cannot_put_a_file_back_deletes_nothing_and_still_reports_failure() {
    let w = world();
    let dir = w.data().join(RETIRED_DIR).join("1699999999-cccccccccccc");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("vault.bin.bak"), b"stays in the archive").unwrap();
    // the original place of the file is gone and the archived copy is the only one: put it "back" to
    // a path whose folder does not exist, which fails
    let src = w.data().join("no-such-folder").join("vault.bin.bak");
    let moved = vec![(src, dir.join("vault.bin.bak"))];
    assert_eq!(
        rollback(&moved, &dir, &|from, to| fs::rename(from, to)),
        Err(ResetError::ResetFailed)
    );
    assert_eq!(
        fs::read(dir.join("vault.bin.bak")).unwrap(),
        b"stays in the archive"
    );
    assert!(dir.is_dir(), "a folder that still holds a file is kept");
}

#[test]
fn only_a_game_that_is_starting_blocks_it_not_a_running_one_or_a_profile_being_prepared() {
    let w = world();
    let id = InstanceId::new("vanilla").unwrap();
    assert!(!w.launch.any_starting());
    w.launch.claim_checked(&id).unwrap();
    assert!(w.launch.any_starting());
    assert_eq!(w.reset(), Err(ResetError::GameStarting));
    // the game came up: it is running now, and no longer needs the credentials
    w.launch.mark_playing(&id);
    assert!(!w.launch.any_starting());
    w.locks.claim("other", Activity::Preparing).unwrap();
    w.reset().unwrap();
}

#[test]
fn an_unlock_or_create_that_began_before_start_fresh_cannot_install_its_vault_afterwards() {
    let w = world();
    let started = w.auth.epoch.load(Ordering::SeqCst);
    let opened = Vault::unlock(&w.vault(), PASS).unwrap(); // in flight: its file is about to be archived
    w.reset().unwrap();
    let err = w
        .auth
        .install_vault(opened, started, |_| panic!("must not run"))
        .unwrap_err();
    assert_eq!(err.code, "vault_missing");
    assert!(lock(&w.auth.vault).is_none(), "nothing was installed");
    assert!(
        !w.vault().exists(),
        "and the archived vault was not written back"
    );
    // an operation that starts after it installs normally
    let now = w.auth.epoch.load(Ordering::SeqCst);
    let fresh = Vault::create_with(&w.vault(), NEW_PASS, FAST).unwrap();
    w.auth.install_vault(fresh, now, |_| ()).unwrap();
    assert!(lock(&w.auth.vault).is_some());
}

/// A rename that fails on chosen calls (counted from 1) and otherwise really renames.
fn failing_on(calls: &'static [usize]) -> impl Fn(&Path, &Path) -> io::Result<()> {
    let n = std::cell::Cell::new(0usize);
    move |from, to| {
        n.set(n.get() + 1);
        if calls.contains(&n.get()) {
            Err(io::Error::other("injected failure"))
        } else {
            fs::rename(from, to)
        }
    }
}

#[test]
fn a_failed_vault_rename_puts_the_backup_back_and_reports_failure_on_any_system() {
    let w = world();
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    let files = credential_files(&w.vault()).unwrap();
    assert_eq!(files.len(), 2, "the backup, then the vault");
    // call 1 moves the backup, call 2 (the vault) fails, call 3 puts the backup back
    let result = retire_with(&files, &w.data().join(RETIRED_DIR), NOW, &failing_on(&[2]));
    assert_eq!(result, Err(ResetError::ResetFailed));
    assert_eq!(
        (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap()),
        (vault, bak)
    );
    assert!(
        w.retired().is_empty(),
        "the folder it made is empty again, so it is gone"
    );
    assert!(
        Vault::unlock(&w.vault(), PASS).is_ok(),
        "the active vault still opens"
    );
}

#[test]
fn when_the_rollback_fails_too_the_credentials_are_kept_in_the_archive_and_a_retry_finishes() {
    let w = world();
    let (vault, bak) = (fs::read(w.vault()).unwrap(), fs::read(w.bak()).unwrap());
    let files = credential_files(&w.vault()).unwrap();
    // call 1 moves the backup, call 2 (the vault) fails, call 3 (putting the backup back) fails too
    let result = retire_with(
        &files,
        &w.data().join(RETIRED_DIR),
        NOW,
        &failing_on(&[2, 3]),
    );
    // never a false success
    assert_eq!(result, Err(ResetError::ResetFailed));
    // the active vault is untouched, and the backup's bytes are safe in the archive folder
    assert_eq!(fs::read(w.vault()).unwrap(), vault);
    assert!(!w.bak().exists());
    let archived = w.retired();
    assert_eq!(
        archived.len(),
        1,
        "the folder that still holds a file is kept"
    );
    assert_eq!(fs::read(archived[0].join("vault.bin.bak")).unwrap(), bak);
    assert!(w
        .list()
        .accounts
        .iter()
        .any(|a| matches!(a.status, AccountStatus::Ready)));
    assert!(Vault::unlock(&w.vault(), PASS).is_ok());
    // a retry (with a working file system) retires the vault and marks the accounts: nothing is lost
    w.reset().unwrap();
    let all = w.retired();
    assert_eq!(all.len(), 2);
    let second = all.iter().find(|p| **p != archived[0]).unwrap();
    assert_eq!(fs::read(second.join("vault.bin")).unwrap(), vault);
    assert_eq!(fs::read(archived[0].join("vault.bin.bak")).unwrap(), bak);
    assert!(!w.vault().exists());
    assert!(w.list().accounts.iter().all(is_unknown_reauth));
}

#[test]
fn a_create_that_began_before_start_fresh_cannot_install_its_vault_afterwards() {
    let w = world();
    let started = w.auth.epoch.load(Ordering::SeqCst);
    // an unlock first: Start fresh archives the vault, then a create begins and completes before the
    // epoch the *first* operation saw is compared
    w.reset().unwrap();
    let created = Vault::create_with(&w.vault(), NEW_PASS, FAST).unwrap();
    let err = w.auth.install_vault(created, started, |_| ()).unwrap_err();
    assert_eq!(err.code, "vault_missing");
    assert!(lock(&w.auth.vault).is_none(), "nothing was installed");
    // and a create that saw the current epoch is installed
    let current = w.auth.epoch.load(Ordering::SeqCst);
    let again = Vault::unlock(&w.vault(), NEW_PASS).unwrap();
    w.auth.install_vault(again, current, |_| ()).unwrap();
    assert!(lock(&w.auth.vault).is_some());
}

#[test]
fn the_orphan_purge_runs_only_after_the_epoch_is_verified_and_never_rewrites_an_archived_vault() {
    use crate::auth::purge_orphans;
    let w = world();
    // a vault with an entry that no account owns: a purge would remove it, and so would SAVE the vault
    let mut v = Vault::unlock(&w.vault(), PASS).unwrap();
    v.set_account("ghost", AccountSecrets::default()).unwrap();
    let started = w.auth.epoch.load(Ordering::SeqCst);
    w.reset().unwrap();
    assert!(!w.vault().exists());
    let ran = std::cell::Cell::new(false);
    let err = w
        .auth
        .install_vault(v, started, |v| {
            ran.set(true);
            purge_orphans(v, w.accounts.path());
        })
        .unwrap_err();
    assert_eq!(err.code, "vault_missing");
    assert!(!ran.get(), "the purge never ran");
    assert!(
        !w.vault().exists(),
        "so the archived vault was not written back to the active place"
    );
    // with the epoch current, the purge does run, under the vault lock, and the vault is installed
    let fresh = Vault::create_with(&w.vault(), NEW_PASS, FAST).unwrap();
    let mut fresh = fresh;
    fresh
        .set_account("ghost", AccountSecrets::default())
        .unwrap();
    let now = w.auth.epoch.load(Ordering::SeqCst);
    w.auth
        .install_vault(fresh, now, |v| {
            ran.set(true);
            assert_eq!(purge_orphans(v, w.accounts.path()), 1);
        })
        .unwrap();
    assert!(ran.get());
    assert!(lock(&w.auth.vault)
        .as_ref()
        .unwrap()
        .account("ghost")
        .is_none());
}
