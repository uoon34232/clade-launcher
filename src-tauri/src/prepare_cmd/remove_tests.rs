#[cfg(test)]
use super::*;
use crate::instances::{NewInstance, RETAINED_DIR};
use crate::settings::LauncherSettings;
use crate::ui_settings::SettingsState;

fn world(names: &[&str]) -> (tempfile::TempDir, PrepareState, Vec<InstanceId>) {
    let d = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(d.path());
    root.ensure_layout().unwrap();
    let no = || Err("tests do not use the network".to_owned());
    let state = PrepareState::new(root.clone(), no(), no(), no(), no());
    let store = InstanceStore::new(&root);
    let ids = names
        .iter()
        .enumerate()
        .map(|(n, name)| {
            let inst = store
                .create(NewInstance::vanilla(name, "1.21.4").unwrap(), n as i64)
                .unwrap();
            let save = store
                .dirs(&inst.id)
                .saves_dir()
                .join("World")
                .join("level.dat");
            std::fs::create_dir_all(save.parent().unwrap()).unwrap();
            std::fs::write(save, format!("world of {name}")).unwrap();
            inst.id
        })
        .collect();
    (d, state, ids)
}

fn code<T>(r: Result<T, CodedError>) -> Option<&'static str> {
    r.err().map(|e| e.code)
}

fn level(state: &PrepareState, id: &InstanceId) -> std::path::PathBuf {
    InstanceStore::new(&state.root)
        .dirs(id)
        .saves_dir()
        .join("World")
        .join("level.dat")
}

#[test]
fn removing_keeps_the_worlds_and_leaves_the_other_profile_alone() {
    let (_d, state, ids) = world(&["One", "Two"]);
    let retained_world = remove_from_clade(&state, ids[0].as_str());
    assert!(retained_world.is_ok());
    // Gone from the list, kept on disk with its world.
    let listing = InstanceStore::new(&state.root).list().unwrap();
    assert_eq!(listing.instances.len(), 1);
    assert_eq!(listing.instances[0].id, ids[1]);
    let kept: Vec<_> = std::fs::read_dir(state.root.instances_dir().join(RETAINED_DIR))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        std::fs::read_to_string(
            kept[0]
                .join("game")
                .join("saves")
                .join("World")
                .join("level.dat")
        )
        .unwrap(),
        "world of One"
    );
    assert!(level(&state, &ids[1]).is_file());
    // The claim taken for the move was released: the profile id is free again.
    assert!(state.locks.current(ids[0].as_str()).is_none());
}

#[test]
fn removing_clears_recorded_prepare_failure_for_that_profile_only() {
    let (_d, state, ids) = world(&["One", "Two"]);
    lock(&state.failed).insert(
        ids[0].as_str().to_owned(),
        (&PrepareError::Offline("x".into())).into(),
    );
    lock(&state.failed).insert(
        ids[1].as_str().to_owned(),
        (&PrepareError::Network("y".into())).into(),
    );
    assert_eq!(state.status(ids[0].as_str()).state, "failed");
    remove_from_clade(&state, ids[0].as_str()).unwrap();
    assert_eq!(state.status(ids[0].as_str()).state, "idle");
    assert_eq!(state.status(ids[1].as_str()).state, "failed");
}

#[test]
fn bad_ids_unknown_profiles_and_busy_profiles_are_rejected_before_anything_moves() {
    let (_d, state, ids) = world(&["One", "Two"]);
    for bad in ["", "../escape", ".removed", "UPPER", "a/b", "nope"] {
        assert_eq!(
            code(remove_from_clade(&state, bad)),
            Some("not_found"),
            "{bad}"
        );
    }
    // Its game is starting or running.
    state
        .locks
        .claim(ids[0].as_str(), Activity::Launching)
        .unwrap();
    assert_eq!(
        code(remove_from_clade(&state, ids[0].as_str())),
        Some("already_running")
    );
    state.locks.release(ids[0].as_str());
    // It is being prepared or verified.
    state
        .locks
        .claim(ids[1].as_str(), Activity::Preparing)
        .unwrap();
    assert_eq!(
        code(remove_from_clade(&state, ids[1].as_str())),
        Some("busy")
    );
    state.locks.release(ids[1].as_str());
    // Nothing moved, and a refusal never keeps the profile claimed.
    assert_eq!(
        InstanceStore::new(&state.root)
            .list()
            .unwrap()
            .instances
            .len(),
        2
    );
    assert!(!state.root.instances_dir().join(RETAINED_DIR).exists());
    assert!(level(&state, &ids[0]).is_file() && level(&state, &ids[1]).is_file());
    assert!(remove_from_clade(&state, ids[0].as_str()).is_ok());
}

#[test]
fn retire_failures_release_lock_preserve_failed_status_and_leave_profile_untouched() {
    // 1. `InstanceError::Io` -> `remove_failed` (RETAINED_DIR blocked by a plain file).
    let (_d1, state1, ids1) = world(&["One", "Two"]);
    lock(&state1.failed).insert(
        ids1[0].as_str().to_owned(),
        (&PrepareError::Offline("x".into())).into(),
    );
    let retained_blocker = state1.root.instances_dir().join(RETAINED_DIR);
    std::fs::write(&retained_blocker, b"not a directory").unwrap();

    assert_eq!(
        code(remove_from_clade(&state1, ids1[0].as_str())),
        Some("remove_failed")
    );
    // `Held` was acquired before `retire`; dropping on error must release the claim automatically.
    assert!(state1.locks.current(ids1[0].as_str()).is_none());
    // Failed status is only cleared after `retire` succeeds.
    assert_eq!(state1.status(ids1[0].as_str()).state, "failed");
    assert!(level(&state1, &ids1[0]).is_file());
    assert!(InstanceStore::new(&state1.root).get(&ids1[0]).is_ok());

    // 2. `InstanceError::Json` -> `remove_refused` (corrupt `instance.json`).
    let (_d2, state2, ids2) = world(&["One", "Two"]);
    lock(&state2.failed).insert(
        ids2[0].as_str().to_owned(),
        (&PrepareError::Network("y".into())).into(),
    );
    let manifest = InstanceStore::new(&state2.root)
        .dirs(&ids2[0])
        .manifest_file();
    std::fs::write(&manifest, b"{ not json").unwrap();

    assert_eq!(
        code(remove_from_clade(&state2, ids2[0].as_str())),
        Some("remove_refused")
    );
    assert!(state2.locks.current(ids2[0].as_str()).is_none());
    assert_eq!(state2.status(ids2[0].as_str()).state, "failed");
    assert!(level(&state2, &ids2[0]).is_file());
    assert_eq!(std::fs::read(&manifest).unwrap(), b"{ not json");
    assert!(!state2.root.instances_dir().join(RETAINED_DIR).exists());
    // The healthy sibling profile is unaffected.
    assert!(InstanceStore::new(&state2.root).get(&ids2[1]).is_ok());

    // 3. `InstanceError::Link` -> `remove_refused` (junction at `RETAINED_DIR`).
    #[cfg(windows)]
    {
        let (_d3, state3, ids3) = world(&["One"]);
        let outside = tempfile::tempdir().unwrap();
        lock(&state3.failed).insert(
            ids3[0].as_str().to_owned(),
            (&PrepareError::Offline("z".into())).into(),
        );
        let retained_link = state3.root.instances_dir().join(RETAINED_DIR);
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&retained_link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        assert_eq!(
            code(remove_from_clade(&state3, ids3[0].as_str())),
            Some("remove_refused")
        );
        assert!(state3.locks.current(ids3[0].as_str()).is_none());
        assert_eq!(state3.status(ids3[0].as_str()).state, "failed");
        assert!(level(&state3, &ids3[0]).is_file());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        std::fs::remove_dir(&retained_link).unwrap();
    }
}

fn remove_and_update_selection(
    state: &PrepareState,
    settings: &SettingsState,
    id: &str,
) -> Result<ProfilesDto, CodedError> {
    remove_from_clade(state, id)?;
    let selected = settings.last_instance().filter(|s| s.as_str() != id);
    let dto = snapshot(&state.root, selected).map_err(CodedError::storage)?;
    let shown = dto
        .selected
        .as_deref()
        .and_then(|s| InstanceId::new(s).ok());
    let _ = settings.set_last_instance(shown);
    Ok(dto)
}

#[test]
fn the_selection_stays_sane_and_the_last_profile_gives_the_empty_state() {
    let (_d, state, ids) = world(&["One", "Two", "Three"]);
    let settings = SettingsState::new(state.root.settings_file(), LauncherSettings::default());
    settings.set_last_instance(Some(ids[1].clone())).unwrap();

    // Removing a non-selected profile preserves the active selection in memory and on disk.
    let after_non_selected =
        remove_and_update_selection(&state, &settings, ids[0].as_str()).unwrap();
    assert_eq!(after_non_selected.profiles.len(), 2);
    assert_eq!(
        after_non_selected.selected.as_deref(),
        Some(ids[1].as_str()),
        "removing a non-selected profile must keep the current selection"
    );
    assert_eq!(settings.last_instance(), Some(ids[1].clone()));
    assert_eq!(
        LauncherSettings::load_or_default(&state.root.settings_file())
            .unwrap()
            .last_instance,
        Some(ids[1].clone())
    );

    // A rejected removal leaves `SettingsState` untouched.
    assert_eq!(
        code(remove_and_update_selection(&state, &settings, "nope")),
        Some("not_found")
    );
    assert_eq!(settings.last_instance(), Some(ids[1].clone()));

    // Removing the active profile filters it out and persists the fallback selection.
    let after_active = remove_and_update_selection(&state, &settings, ids[1].as_str()).unwrap();
    assert_eq!(after_active.profiles.len(), 1);
    assert_eq!(
        after_active.selected.as_deref(),
        Some(ids[2].as_str()),
        "removing the selected profile falls back to a remaining profile"
    );
    assert_eq!(settings.last_instance(), Some(ids[2].clone()));
    assert_eq!(
        LauncherSettings::load_or_default(&state.root.settings_file())
            .unwrap()
            .last_instance,
        Some(ids[2].clone())
    );

    // Removing the last profile leaves the empty state and clears persisted `last_instance`.
    let empty = remove_and_update_selection(&state, &settings, ids[2].as_str()).unwrap();
    assert!(empty.profiles.is_empty());
    assert_eq!(empty.selected, None);
    assert_eq!(settings.last_instance(), None);
    assert_eq!(
        LauncherSettings::load_or_default(&state.root.settings_file())
            .unwrap()
            .last_instance,
        None
    );

    // All three are retained, none were erased.
    assert_eq!(
        std::fs::read_dir(state.root.instances_dir().join(RETAINED_DIR))
            .unwrap()
            .count(),
        3
    );
}

#[test]
fn remove_profile_command_runs_on_blocking_pool_and_never_erases_files() {
    let cmd = include_str!("../prepare_cmd.rs");
    let fn_start = cmd.find("pub async fn remove_profile").unwrap();
    let fn_end = cmd.find("pub fn cancel_prepare").unwrap();
    let body = &cmd[fn_start..fn_end];
    assert!(
        body.contains("spawn_blocking("),
        "remove_profile must offload filesystem retire and snapshot walks to spawn_blocking"
    );
    assert!(
        !body.contains("remove_dir_all")
            && !body.contains("remove_dir")
            && !body.contains("remove_file")
            && !body.contains(".delete("),
        "remove_profile must never erase files or directories"
    );
    // Keep `remove_and_update_selection` aligned with the command body's selection filtering & persistence.
    assert!(body.contains("remove_from_clade(&state, &id)?"));
    assert!(body.contains("settings.last_instance().filter(|s| s.as_str() != id)"));
    assert!(body.contains("snapshot(&state.root, selected)"));
    assert!(body.contains("settings.set_last_instance(shown)"));
}

#[test]
fn no_screen_or_command_can_erase_a_profile() {
    // The destructive store method exists only for tests (it does not even compile into the app).
    let src = include_str!("../instances.rs");
    let at = src.find("pub fn delete(").unwrap();
    assert!(src[..at].trim_end().ends_with("#[cfg(test)]"));
    // The profile commands never name it, and the removal code has no file/directory erase of its own.
    let cmd = include_str!("../prepare_cmd.rs");
    let from = cmd.find("pub(crate) fn remove_from_clade").unwrap();
    let to = cmd.find("pub fn cancel_prepare").unwrap();
    let removal = &cmd[from..to];
    assert!(
        !removal.contains("remove_dir_all")
            && !removal.contains("remove_dir")
            && !removal.contains("remove_file")
            && !removal.contains(".delete(")
    );
    let profiles = include_str!("../profiles.rs");
    let production = profiles.split("#[cfg(test)]").next().unwrap();
    assert!(
        !production.contains(".delete(")
            && !production.contains("remove_dir_all")
            && !production.contains("remove_dir")
            && !production.contains("remove_file")
    );
    for module in [
        include_str!("../launch_cmd.rs"),
        include_str!("../support_cmd.rs"),
    ] {
        let prod = module.split("#[cfg(test)]").next().unwrap();
        assert!(!prod.contains(".delete("));
    }
}
