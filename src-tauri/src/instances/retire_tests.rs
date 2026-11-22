#[cfg(test)]
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn setup() -> (tempfile::TempDir, PortableRoot) {
    let d = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(d.path());
    root.ensure_layout().unwrap();
    (d, root)
}

fn vanilla(name: &str) -> NewInstance {
    NewInstance::vanilla(name, "1.21.4").unwrap()
}

fn create_junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");
}

fn sample_manifest_bytes(id: &str, name: &str) -> Vec<u8> {
    let inst = Instance {
        schema: SCHEMA,
        id: InstanceId::new(id).unwrap(),
        name: name.to_owned(),
        minecraft: McVersion::new("1.21.4").unwrap(),
        profile: Profile::Vanilla,
        setup: SetupState::Pending,
        java: JavaChoice::Auto,
        memory_mb: None,
        resolution: None,
        extra_jvm_args: Vec::new(),
        created_at: 1,
    };
    serde_json::to_vec_pretty(&inst).unwrap()
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

/// Every file below `dir` with its bytes.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn retiring_a_profile_moves_every_file_unchanged_and_nothing_lists_it_again() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    let keep = store.create(vanilla("Keep"), 1).unwrap();
    let gone = store
        .create(
            NewInstance::modded(
                "Gone",
                "1.21.4",
                LoaderKind::Fabric,
                LoaderVersionSpec::Auto,
            )
            .unwrap(),
            2,
        )
        .unwrap();
    let dirs = store.dirs(&gone.id);
    // User content of every kind, including files nobody knows about.
    let content: [(&str, &[u8]); 10] = [
        ("game/saves/World One/level.dat", &[0x1f, 0x8b, 1, 2, 3]),
        (
            "game/saves/World One/region/r.0.0.mca",
            &[0, 1, 2, 255, 254],
        ),
        ("game/mods/some-mod.jar", b"PK-mod"),
        ("game/config/some-mod.toml", b"enabled = true\n"),
        ("game/screenshots/2026-01-01.png", b"png"),
        ("game/options.txt", b"fov:90.0\n"),
        ("game/logs/latest.log", b"log"),
        ("game/unknown-by-minecraft/x.bin", b"?"),
        ("home/AppData/Roaming/thing.dat", b"home"),
        ("playtime.json", b"{\"schema\":1}"),
    ];
    for (rel, bytes) in content {
        let p = dirs.root().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    let before = tree(dirs.root());
    assert!(before.contains_key("instance.json"));
    let other_before = tree(store.dirs(&keep.id).root());

    let kept_at = store.retire(&gone.id).unwrap();

    // The entire content, byte for byte, now sits inside the instances folder's retained area.
    assert_eq!(tree(&kept_at), before);
    assert!(kept_at.starts_with(root.instances_dir().join(RETAINED_DIR)));
    assert!(!dirs.root().exists(), "it left the active profile folder");
    // It is not an active profile, not reported as a problem, and not even a valid instance id.
    let listing = store.list().unwrap();
    assert_eq!(
        listing
            .instances
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        vec![keep.id.as_str()]
    );
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);
    assert!(InstanceId::new(RETAINED_DIR).is_err());
    assert!(store.get(&gone.id).is_err());
    // The other profile is untouched.
    assert_eq!(tree(store.dirs(&keep.id).root()), other_before);
}

#[test]
fn every_removal_gets_its_own_folder_and_nothing_is_overwritten() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    let mut places = BTreeSet::new();
    for n in 0..3 {
        // The same name, so the same id, three times over.
        let inst = store.create(vanilla("Again"), n).unwrap();
        fs::write(
            store.dirs(&inst.id).game_dir().join(format!("f{n}.txt")),
            b"x",
        )
        .unwrap();
        places.insert(store.retire(&inst.id).unwrap());
    }
    assert_eq!(places.len(), 3);
    for (n, p) in places.iter().enumerate() {
        assert!(p.join("instance.json").is_file(), "{n}");
    }
    let files: BTreeSet<String> = places
        .iter()
        .flat_map(|p| tree(p).into_keys().filter(|k| k.starts_with("game/f")))
        .collect();
    assert_eq!(files.len(), 3, "each removed copy kept its own file");
    assert!(store.list().unwrap().instances.is_empty());
}

#[test]
fn retiring_refuses_what_is_not_a_clade_profile_and_changes_nothing() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    // unknown id, a folder without a manifest, a damaged manifest
    assert!(store.retire(&InstanceId::new("nope").unwrap()).is_err());
    fs::create_dir(root.instances_dir().join("not-ours")).unwrap();
    fs::write(
        root.instances_dir().join("not-ours").join("precious.txt"),
        b"x",
    )
    .unwrap();
    assert!(store.retire(&InstanceId::new("not-ours").unwrap()).is_err());
    let inst = store.create(vanilla("Broken"), 1).unwrap();
    fs::write(store.dirs(&inst.id).manifest_file(), b"{ not json").unwrap();
    assert!(store.retire(&inst.id).is_err());
    assert!(root
        .instances_dir()
        .join("not-ours")
        .join("precious.txt")
        .exists());
    assert!(store.dirs(&inst.id).manifest_file().exists());
    assert!(!root
        .instances_dir()
        .join(RETAINED_DIR)
        .join("not-ours")
        .exists());
}

#[test]
fn a_failed_move_leaves_the_profile_exactly_where_it_was() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    let inst = store.create(vanilla("Stay"), 1).unwrap();
    fs::write(store.dirs(&inst.id).game_dir().join("save.dat"), b"data").unwrap();
    let before = tree(store.dirs(&inst.id).root());
    // The retained folder cannot be made: a plain file sits where it must be.
    fs::write(root.instances_dir().join(RETAINED_DIR), b"in the way").unwrap();
    assert!(store.retire(&inst.id).is_err());
    assert_eq!(tree(store.dirs(&inst.id).root()), before);
    assert_eq!(store.list().unwrap().instances.len(), 1);
}

#[test]
fn retiring_cleans_disposable_natives_while_preserving_all_user_files_byte_for_byte() {
    let (d, root) = setup();
    let store = InstanceStore::new(&root);
    let inst = store.create(vanilla("WithNatives"), 1).unwrap();
    let dirs = store.dirs(&inst.id);

    // User files across game/, home/, playtime.json, and unknown custom files.
    let kept_files: [(&str, &[u8]); 5] = [
        ("game/saves/World/level.dat", &[1, 2, 3, 4]),
        ("game/options.txt", b"fov:85.0\n"),
        ("home/AppData/Roaming/state.bin", b"user-state"),
        ("playtime.json", br#"{"schema":1}"#),
        ("custom-note.txt", b"do not touch"),
    ];
    for (rel, bytes) in kept_files {
        let p = dirs.root().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    let expected = tree(dirs.root());

    // Disposable extracted native DLLs under natives/.
    fs::create_dir_all(dirs.natives_dir().join("x64")).unwrap();
    fs::write(dirs.natives_dir().join("lwjgl.dll"), vec![0xAA; 4096]).unwrap();
    fs::write(dirs.natives_dir().join("x64/openal.dll"), vec![0xBB; 4096]).unwrap();
    assert!(dirs.natives_dir().is_dir());

    let retired_at = store.retire(&inst.id).unwrap();
    assert!(!retired_at.join("natives").exists());
    assert_eq!(tree(&retired_at), expected);

    {
        let outside = d.path().join("outside-profile");
        fs::create_dir_all(outside.join("game/saves")).unwrap();
        fs::write(
            outside.join("instance.json"),
            sample_manifest_bytes("linked-retire", "Linked"),
        )
        .unwrap();
        fs::write(outside.join("game/saves/sentinel.txt"), b"untouched").unwrap();
        let outside_before = tree(&outside);

        let linked = root.instances_dir().join("linked-retire");
        create_junction(&linked, &outside);
        assert!(matches!(
            store.retire(&InstanceId::new("linked-retire").unwrap()),
            Err(InstanceError::Link(_))
        ));
        assert!(is_link(&linked).unwrap());
        assert_eq!(tree(&outside), outside_before);
        fs::remove_dir(&linked).unwrap();
    }
    let _ = &d;
}

#[test]
fn retiring_and_reading_reject_links_on_removed_dir_natives_and_retained_game() {
    let (d, root) = setup();
    let store = InstanceStore::new(&root);

    // 1. Junction at `instances/.removed` is refused by retire, list_retained, read_retained, and restore.
    {
        let inst = store.create(vanilla("StayPut"), 10).unwrap();
        let sentinel = store.dirs(&inst.id).game_dir().join("save.dat");
        fs::write(&sentinel, b"keep-me").unwrap();
        let inst_before = tree(store.dirs(&inst.id).root());

        let outside_removed = d.path().join("outside-removed");
        fs::create_dir_all(&outside_removed).unwrap();
        let retained_dir = root.instances_dir().join(RETAINED_DIR);
        create_junction(&retained_dir, &outside_removed);

        assert!(matches!(
            store.retire(&inst.id),
            Err(InstanceError::Link(_))
        ));
        assert_eq!(tree(store.dirs(&inst.id).root()), inst_before);
        assert!(fs::read_dir(&outside_removed).unwrap().next().is_none());
        assert!(store.list_retained().is_err());
        assert!(matches!(
            store.read_retained("some-folder"),
            Err(InstanceError::Link(_))
        ));
        assert!(matches!(
            store.restore("some-folder", "Restored"),
            Err(InstanceError::Link(_))
        ));
        fs::remove_dir(&retained_dir).unwrap();
        store.delete(&inst.id).unwrap();
    }

    // 2. Junction at `<instance>/natives` is never followed or deleted during retire cleanup.
    {
        let inst = store.create(vanilla("LinkedNatives"), 20).unwrap();
        let dirs = store.dirs(&inst.id);
        fs::write(dirs.game_dir().join("options.txt"), b"fov:80.0\n").unwrap();

        let outside_natives = d.path().join("outside-natives");
        fs::create_dir_all(&outside_natives).unwrap();
        fs::write(outside_natives.join("precious.txt"), b"do-not-delete").unwrap();
        create_junction(&dirs.natives_dir(), &outside_natives);

        let retired_at = store.retire(&inst.id).unwrap();
        let retired_natives = retired_at.join("natives");
        assert!(is_link(&retired_natives).unwrap());
        assert_eq!(
            fs::read(outside_natives.join("precious.txt")).unwrap(),
            b"do-not-delete"
        );
        fs::remove_dir(&retired_natives).unwrap();
        remove_dir_all_with_retry(&retired_at).unwrap();
    }

    // 3. Junction at `<retained>/game` is skipped by list_retained and refused by read_retained/restore.
    {
        let inst = store.create(vanilla("LinkedGame"), 30).unwrap();
        let retired_at = store.retire(&inst.id).unwrap();
        let folder = retired_at.file_name().unwrap().to_str().unwrap().to_owned();
        let retired_game = retired_at.join("game");
        fs::remove_dir_all(&retired_game).unwrap();

        let outside_game = d.path().join("outside-game");
        fs::create_dir_all(&outside_game).unwrap();
        fs::write(outside_game.join("level.dat"), b"outside-world").unwrap();
        create_junction(&retired_game, &outside_game);

        assert!(store.list_retained().unwrap().is_empty());
        assert!(matches!(
            store.read_retained(&folder),
            Err(InstanceError::Link(_))
        ));
        assert!(matches!(
            store.restore(&folder, "Restored"),
            Err(InstanceError::Link(_))
        ));
        assert!(retired_at.exists());
        assert_eq!(
            fs::read(outside_game.join("level.dat")).unwrap(),
            b"outside-world"
        );
        fs::remove_dir(&retired_game).unwrap();
    }
}

#[test]
fn retained_profiles_can_be_listed_and_restored_with_all_files_intact() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    assert!(store.list_retained().unwrap().is_empty());

    let inst = store.create(vanilla("Survival"), 100).unwrap();
    let orig_dirs = store.dirs(&inst.id);
    let save_file = orig_dirs.game_dir().join("saves/World/level.dat");
    fs::create_dir_all(save_file.parent().unwrap()).unwrap();
    fs::write(&save_file, b"world-bytes").unwrap();
    let home_file = orig_dirs.home_dir().join("AppData/Roaming/state.bin");
    fs::create_dir_all(home_file.parent().unwrap()).unwrap();
    fs::write(&home_file, b"home-bytes").unwrap();
    fs::write(
        orig_dirs.root().join("playtime.json"),
        br#"{"schema":1,"total_seconds":180,"sessions":2,"last_played_at":null,"applied":[]}"#,
    )
    .unwrap();

    let retired_path = store.retire(&inst.id).unwrap();
    let folder = retired_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let retained = store.list_retained().unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].folder, folder);
    assert_eq!(retained[0].instance.name, "Survival");
    assert!(retained[0].removed_at.is_some());

    let restored = store.restore(&folder, "Survival").unwrap();
    assert_eq!(restored.id, inst.id);
    assert_eq!(restored.name, "Survival");
    assert_eq!(store.get(&restored.id).unwrap(), restored);
    let listing = store.list().unwrap();
    assert_eq!(listing.instances, vec![restored.clone()]);
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);
    assert!(store.list_retained().unwrap().is_empty());
    assert!(!root.instances_dir().join(RETAINED_DIR).exists());
    assert_eq!(fs::read(&save_file).unwrap(), b"world-bytes");
    assert_eq!(fs::read(&home_file).unwrap(), b"home-bytes");
    assert_eq!(
        crate::playtime::PlaytimeStore::new(&root)
            .instance_total(&restored.id)
            .unwrap()
            .total_seconds,
        180
    );
}

#[test]
fn restore_resolves_folder_collisions_and_rejects_traversal_and_links() {
    let (d, root) = setup();
    let store = InstanceStore::new(&root);

    let first = store.create(vanilla("Survival"), 100).unwrap();
    let first_dirs = store.dirs(&first.id);
    fs::create_dir_all(first_dirs.game_dir().join("saves/World")).unwrap();
    fs::write(
        first_dirs.game_dir().join("saves/World/level.dat"),
        b"first-world",
    )
    .unwrap();
    fs::create_dir_all(first_dirs.home_dir()).unwrap();
    fs::write(first_dirs.home_dir().join("state.bin"), b"first-home").unwrap();
    fs::write(
        first_dirs.playtime_file(),
        br#"{"schema":1,"total_seconds":90,"sessions":1,"last_played_at":null,"applied":[]}"#,
    )
    .unwrap();
    let retired_path = store.retire(&first.id).unwrap();
    let folder = retired_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Another active profile now occupies `instances/survival`.
    let replacement = store.create(vanilla("Survival"), 200).unwrap();
    assert_eq!(replacement.id, first.id);
    fs::write(
        store.dirs(&replacement.id).game_dir().join("options.txt"),
        b"replacement\n",
    )
    .unwrap();
    let replacement_before = tree(store.dirs(&replacement.id).root());

    // Restoring with the same name collides on `instances/survival` and picks `survival-2`.
    let restored = store.restore(&folder, "Survival").unwrap();
    assert_ne!(restored.id, first.id);
    assert_eq!(restored.id.as_str(), "survival-2");
    assert_eq!(restored.name, "Survival");
    assert_eq!(store.get(&restored.id).unwrap(), restored);
    let restored_dirs = store.dirs(&restored.id);
    assert_eq!(
        fs::read(restored_dirs.game_dir().join("saves/World/level.dat")).unwrap(),
        b"first-world"
    );
    assert_eq!(
        fs::read(restored_dirs.home_dir().join("state.bin")).unwrap(),
        b"first-home"
    );
    assert_eq!(
        crate::playtime::PlaytimeStore::new(&root)
            .instance_total(&restored.id)
            .unwrap()
            .total_seconds,
        90
    );
    assert_eq!(tree(store.dirs(&replacement.id).root()), replacement_before);
    let listing = store.list().unwrap();
    assert_eq!(listing.instances.len(), 2);
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);

    // Restoring as "Survival (2)" while `instances/survival-2` is occupied collides and picks `survival-2-2`.
    let second = store.create(vanilla("Other"), 300).unwrap();
    fs::write(
        store.dirs(&second.id).game_dir().join("note.txt"),
        b"second",
    )
    .unwrap();
    let folder2 = store
        .retire(&second.id)
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let restored2 = store.restore(&folder2, "Survival (2)").unwrap();
    assert_eq!(restored2.id.as_str(), "survival-2-2");
    assert_eq!(restored2.name, "Survival (2)");
    assert_eq!(store.get(&restored2.id).unwrap(), restored2);
    assert_eq!(
        fs::read(store.dirs(&restored2.id).game_dir().join("note.txt")).unwrap(),
        b"second"
    );
    assert_eq!(tree(store.dirs(&replacement.id).root()), replacement_before);
    let listing = store.list().unwrap();
    assert_eq!(listing.instances.len(), 3);
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);

    // Traversal and missing folders are refused.
    assert!(matches!(
        store.read_retained("../survival"),
        Err(InstanceError::BadId(_))
    ));
    assert!(matches!(
        store.restore("../survival", "X"),
        Err(InstanceError::BadId(_))
    ));
    assert!(matches!(
        store.restore("missing-folder", "X"),
        Err(InstanceError::NotFound(_))
    ));

    // A symlink/junction inside .removed is skipped by list_retained and refused by read_retained/restore.
    let outside = d.path().join("outside-retained");
    fs::create_dir_all(&outside).unwrap();
    let retained_dir = root.instances_dir().join(RETAINED_DIR);
    fs::create_dir_all(&retained_dir).unwrap();
    let linked = retained_dir.join("linked-entry");
    create_junction(&linked, &outside);
    assert!(store.list_retained().unwrap().is_empty());
    assert!(matches!(
        store.read_retained("linked-entry"),
        Err(InstanceError::Link(_))
    ));
    assert!(matches!(
        store.restore("linked-entry", "X"),
        Err(InstanceError::Link(_))
    ));
    fs::remove_dir(&linked).unwrap();
}

#[test]
fn list_retained_sorts_newest_first_uses_mtime_fallback_and_skips_bad_entries() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    let retained_dir = root.instances_dir().join(RETAINED_DIR);
    fs::create_dir_all(&retained_dir).unwrap();

    // Valid entries with explicit timestamps, equal-timestamp tie-break, and mtime fallback.
    for (folder, id, name) in [
        ("alpha-100-00000001", "alpha", "Alpha"),
        ("beta-300-00000002", "beta", "Beta"),
        ("gamma-200-00000003", "gamma", "Gamma"),
        ("delta-200-00000004", "delta", "Delta"),
        ("manual-backup", "manual", "Manual"),
    ] {
        let dir = retained_dir.join(folder);
        fs::create_dir_all(dir.join("game")).unwrap();
        fs::write(dir.join("instance.json"), sample_manifest_bytes(id, name)).unwrap();
    }

    // Unexpected / corrupt entries that must be silently skipped without failing list_retained().
    fs::write(retained_dir.join("stray-file.txt"), b"ignore").unwrap();
    fs::create_dir_all(retained_dir.join(".hidden")).unwrap();
    fs::create_dir_all(retained_dir.join("empty-dir")).unwrap();

    let corrupt_json = retained_dir.join("corrupt-json");
    fs::create_dir_all(&corrupt_json).unwrap();
    fs::write(corrupt_json.join("instance.json"), b"{not valid json").unwrap();

    let too_new = retained_dir.join("too-new");
    fs::create_dir_all(&too_new).unwrap();
    fs::write(
        too_new.join("instance.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": SCHEMA + 1,
            "id": "future",
            "name": "Future",
            "minecraft": "1.21.4",
            "profile": { "type": "vanilla" },
            "setup": { "state": "pending" },
            "java": { "type": "auto" },
            "memory_mb": null,
            "extra_jvm_args": [],
            "created_at": 1
        }))
        .unwrap(),
    )
    .unwrap();

    let bad_name = retained_dir.join("bad-name");
    fs::create_dir_all(&bad_name).unwrap();
    let mut bad_manifest: serde_json::Value =
        serde_json::from_slice(&sample_manifest_bytes("bad-name", "Valid")).unwrap();
    bad_manifest["name"] = serde_json::json!("   ");
    fs::write(
        bad_name.join("instance.json"),
        serde_json::to_vec(&bad_manifest).unwrap(),
    )
    .unwrap();

    let listed = store.list_retained().unwrap();
    let folders: Vec<&str> = listed.iter().map(|r| r.folder.as_str()).collect();
    assert_eq!(
        folders,
        vec![
            "manual-backup",
            "beta-300-00000002",
            "delta-200-00000004",
            "gamma-200-00000003",
            "alpha-100-00000001",
        ]
    );
    assert!(listed[0].removed_at.is_some_and(|ts| ts > 300));
    assert_eq!(listed[1].removed_at, Some(300));
    assert_eq!(listed[2].removed_at, Some(200));
    assert_eq!(listed[3].removed_at, Some(200));
    assert_eq!(listed[4].removed_at, Some(100));
}

#[test]
fn restore_rejects_invalid_final_names() {
    let (_d, root) = setup();
    let store = InstanceStore::new(&root);
    let inst = store.create(vanilla("Survival"), 100).unwrap();
    let folder = store
        .retire(&inst.id)
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let long_name = "a".repeat(65);
    for bad in ["", "   ", "bad\nname", "bad\0name", long_name.as_str()] {
        assert!(
            matches!(store.restore(&folder, bad), Err(InstanceError::BadName(_))),
            "{bad:?} should be rejected"
        );
    }

    let retained = store.list_retained().unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].folder, folder);
    assert!(store.list().unwrap().instances.is_empty());
}
