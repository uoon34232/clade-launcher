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
        fs::create_dir_all(&outside).unwrap();
        let linked = root.instances_dir().join("linked-retire");
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(matches!(
            store.retire(&InstanceId::new("linked-retire").unwrap()),
            Err(InstanceError::Link(_))
        ));
        fs::remove_dir(&linked).unwrap();
    }
    let _ = &d;
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
    assert!(store.list_retained().unwrap().is_empty());
    assert!(!root.instances_dir().join(RETAINED_DIR).exists());
    assert_eq!(fs::read(&save_file).unwrap(), b"world-bytes");
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

    let restored = store.restore(&folder, "Survival (2)").unwrap();
    assert_ne!(restored.id, first.id);
    assert_eq!(restored.name, "Survival (2)");
    assert_eq!(store.list().unwrap().instances.len(), 2);

    // Traversal and bad names are refused.
    assert!(matches!(
        store.read_retained("../survival"),
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
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&linked)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(store.list_retained().unwrap().is_empty());
    assert!(matches!(
        store.read_retained("linked-entry"),
        Err(InstanceError::Link(_))
    ));
    fs::remove_dir(&linked).unwrap();
}
