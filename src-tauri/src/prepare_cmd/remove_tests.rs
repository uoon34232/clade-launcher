#[cfg(test)]
use super::*;
use crate::instances::{NewInstance, RETAINED_DIR};

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

fn code(r: Result<(), CodedError>) -> Option<&'static str> {
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
fn the_selection_stays_sane_and_the_last_profile_gives_the_empty_state() {
    let (_d, state, ids) = world(&["One", "Two", "Three"]);
    let shown = |removed: &InstanceId| {
        // What the command does: the old selection is dropped when it was the removed profile.
        crate::profiles::snapshot(&state.root, Some(removed.clone())).unwrap()
    };
    remove_from_clade(&state, ids[0].as_str()).unwrap();
    let after_first = shown(&ids[0]);
    assert_eq!(after_first.profiles.len(), 2);
    assert_eq!(
        after_first.selected.as_deref(),
        Some(ids[1].as_str()),
        "falls back to a remaining profile"
    );
    remove_from_clade(&state, ids[1].as_str()).unwrap();
    assert_eq!(shown(&ids[1]).selected.as_deref(), Some(ids[2].as_str()));
    remove_from_clade(&state, ids[2].as_str()).unwrap();
    let empty = shown(&ids[2]);
    assert!(empty.profiles.is_empty());
    assert_eq!(empty.selected, None);
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
}

#[test]
fn no_screen_or_command_can_erase_a_profile() {
    // The destructive store method exists only for tests (it does not even compile into the app).
    let src = include_str!("../instances.rs");
    let at = src.find("pub fn delete(").unwrap();
    assert!(src[..at].trim_end().ends_with("#[cfg(test)]"));
    // The profile commands never name it, and the removal code has no recursive erase of its own.
    let cmd = include_str!("../prepare_cmd.rs");
    let from = cmd.find("pub(crate) fn remove_from_clade").unwrap();
    let to = cmd.find("pub fn cancel_prepare").unwrap();
    let removal = &cmd[from..to];
    assert!(!removal.contains("remove_dir_all") && !removal.contains(".delete("));
    let profiles = include_str!("../profiles.rs");
    let production = profiles.split("#[cfg(test)]").next().unwrap();
    assert!(!production.contains(".delete(") && !production.contains("remove_dir_all"));
}
