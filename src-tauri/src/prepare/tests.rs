#[cfg(test)]
use super::*;
#[cfg(test)]
use crate::instances::{LoaderVersionSpec, NewInstance};
#[cfg(test)]
use crate::java_install::RUNTIME_INDEX_URL;
#[cfg(test)]
use crate::loader_install::InstallerCommand;
#[cfg(test)]
use crate::loader_prepare::{InstallerRunner, RunResult};
#[cfg(test)]
use crate::mc_catalog::MANIFEST_URL;
#[cfg(test)]
use crate::net::testing::FakeFetcher;
#[cfg(test)]
use crate::net::testing_stream::FakeStreamer;
#[cfg(test)]
use serde_json::{json, Value};
#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::io::Write;

#[cfg(test)]
pub(crate) fn sha1_hex(b: &[u8]) -> String {
    hex::encode(Sha1::digest(b))
}

#[cfg(test)]
pub(crate) fn sha256_of(b: &[u8]) -> String {
    use sha2::Sha256;
    hex::encode(Sha256::digest(b))
}

#[cfg(test)]
pub(crate) fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[cfg(test)]
pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Scripts what "running the installer" does, and records the command it was given.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeRunner {
    #[allow(clippy::type_complexity)]
    pub(crate) on_run:
        Mutex<Option<Box<dyn Fn(&InstallerCommand) -> io::Result<RunResult> + Send + Sync>>>,
    pub(crate) seen: Mutex<Vec<InstallerCommand>>,
}

#[cfg(test)]
impl InstallerRunner for FakeRunner {
    fn run<'a>(
        &'a self,
        cmd: &'a InstallerCommand,
        cancel: &'a Cancel,
    ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a {
        self.seen.lock().unwrap().push(cmd.clone());
        let result = if cancel.is_cancelled() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        } else {
            match &*self.on_run.lock().unwrap() {
                Some(f) => f(cmd),
                None => Err(io::Error::other("no installer scripted")),
            }
        };
        let cancelled_during = cancel.is_cancelled();
        async move {
            if cancelled_during {
                Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
            } else {
                result
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Era {
    /// Separate `natives-windows` libraries, hash-named assets, the delta runtime.
    Modern,
    /// The historical `natives` map, pre-1.6 assets mapped into the game folder, the legacy runtime.
    Pre16,
    /// The 1.6-1.7 layout: assets copied to `assets/virtual/legacy`.
    Virtual,
}

#[cfg(test)]
pub(crate) struct World {
    pub(crate) _dir: tempfile::TempDir,
    pub(crate) root: PortableRoot,
    pub(crate) fetcher: FakeFetcher,
    pub(crate) streamer: FakeStreamer,
    pub(crate) runner: FakeRunner,
    pub(crate) events: Arc<Mutex<Vec<PrepareProgress>>>,
    pub(crate) tracker: Tracker,
    pub(crate) cancel: Arc<Cancel>,
    pub(crate) mc: String,
    /// Every URL the streamer serves for this version, by purpose.
    pub(crate) lib_url: String,
    pub(crate) client_url: String,
    pub(crate) asset_urls: Vec<String>,
    #[allow(dead_code)]
    pub(crate) java_files: Vec<String>,
}

#[cfg(test)]
pub(crate) const LIBS: &str = "https://libraries.minecraft.net";
#[cfg(test)]
pub(crate) const DATA: &str = "https://piston-data.mojang.com/v1/objects";
#[cfg(test)]
pub(crate) const CDN: &str = "https://resources.download.minecraft.net";

#[cfg(test)]
pub(crate) fn add_runtime(f: &FakeFetcher, s: &FakeStreamer, components: &[&str]) -> Vec<String> {
    let mut index = serde_json::Map::new();
    let mut urls = Vec::new();
    for (n, comp) in components.iter().enumerate() {
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("bin/java.exe", bytes(300, 1 + n as u8)),
            ("bin/javaw.exe", bytes(200, 2 + n as u8)),
            ("lib/modules", bytes(2000, 3 + n as u8)),
        ];
        let mut entries = serde_json::Map::new();
        entries.insert("bin".into(), json!({"type": "directory"}));
        entries.insert("lib".into(), json!({"type": "directory"}));
        for (path, data) in &files {
            let url = format!("{DATA}/{}/{comp}/{path}", sha1_hex(data));
            s.add(&url, data);
            urls.push(url.clone());
            entries.insert((*path).into(), json!({"type": "file", "downloads": {"raw": {"sha1": sha1_hex(data), "size": data.len(), "url": url}}}));
        }
        let manifest = json!({"files": entries}).to_string();
        let manifest_url =
            format!("https://piston-meta.mojang.com/v1/packages/{comp}/manifest.json");
        f.ok(&manifest_url, &manifest);
        index.insert(
            (*comp).into(),
            json!([{"availability": {"group": 1, "progress": 100}, "manifest": {"sha1": sha1_hex(manifest.as_bytes()), "size": manifest.len(), "url": manifest_url}, "version": {"name": "9.9.9", "released": "2025-01-01T00:00:00+00:00"}}]),
        );
    }
    f.ok(RUNTIME_INDEX_URL, json!({"windows-x64": index}).to_string());
    urls
}

#[cfg(test)]
pub(crate) fn world(mc: &str, era: Era) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(dir.path());
    root.ensure_layout().unwrap();
    let fetcher = FakeFetcher::new();
    let streamer = FakeStreamer::new();

    // library jars
    let lib_a = bytes(700, 9);
    let lib_path = "com/example/alpha/1.0/alpha-1.0.jar";
    let lib_url = format!("{LIBS}/{lib_path}");
    streamer.add(&lib_url, &lib_a);
    let dll_jar = zip_of(&[
        ("windows/x64/org/lwjgl/lwjgl.dll", b"dll-bytes"),
        ("lwjgl.dll", b"old-style-dll"),
        ("META-INF/MANIFEST.MF", b"x"),
    ]);
    let (native_path, libraries) = match era {
        Era::Modern => {
            let np = "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar";
            streamer.add(&format!("{LIBS}/{np}"), &dll_jar);
            (
                np,
                json!([
                    {"name": "com.example:alpha:1.0", "downloads": {"artifact": {"path": lib_path, "sha1": sha1_hex(&lib_a), "size": lib_a.len(), "url": lib_url}}},
                    {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows", "rules": [{"action": "allow", "os": {"name": "windows"}}],
                     "downloads": {"artifact": {"path": np, "sha1": sha1_hex(&dll_jar), "size": dll_jar.len(), "url": format!("{LIBS}/{np}")}}},
                    {"name": "org.lwjgl:lwjgl:3.3.3:natives-linux", "rules": [{"action": "allow", "os": {"name": "linux"}}],
                     "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar", "sha1": sha1_hex(b"never fetched"), "size": 13, "url": format!("{LIBS}/never-fetched.jar")}}},
                ]),
            )
        }
        Era::Pre16 | Era::Virtual => {
            let np =
                "org/lwjgl/lwjgl/lwjgl-platform/2.9.4/lwjgl-platform-2.9.4-natives-windows.jar";
            streamer.add(&format!("{LIBS}/{np}"), &dll_jar);
            (
                np,
                json!([
                    {"name": "com.example:alpha:1.0", "downloads": {"artifact": {"path": lib_path, "sha1": sha1_hex(&lib_a), "size": lib_a.len(), "url": lib_url}}},
                    {"name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4", "natives": {"windows": "natives-windows", "linux": "natives-linux"},
                     "extract": {"exclude": ["META-INF/"]},
                     "downloads": {"classifiers": {
                        "natives-windows": {"path": np, "sha1": sha1_hex(&dll_jar), "size": dll_jar.len(), "url": format!("{LIBS}/{np}")},
                        "natives-linux": {"path": "x/linux.jar", "sha1": sha1_hex(b"l"), "size": 1, "url": format!("{LIBS}/never-fetched.jar")}}}},
                ]),
            )
        }
    };
    let _ = native_path;

    // client, logging, assets
    let client = bytes(3000, 5);
    let client_url = format!("{DATA}/{}/client.jar", sha1_hex(&client));
    streamer.add(&client_url, &client);
    let log = b"<Configuration/>".to_vec();
    let log_url = format!("{DATA}/{}/client-log.xml", sha1_hex(&log));
    streamer.add(&log_url, &log);
    let objs = [
        ("icons/icon_16x16.png", bytes(400, 11)),
        ("minecraft/sounds.json", bytes(900, 12)),
        ("copy/of_icon.png", bytes(400, 11)),
    ];
    let mut object_json = serde_json::Map::new();
    let mut asset_urls = Vec::new();
    for (name, data) in &objs {
        let h = sha1_hex(data);
        let url = format!("{CDN}/{}/{h}", &h[..2]);
        if !asset_urls.contains(&url) {
            streamer.add(&url, data);
            asset_urls.push(url);
        }
        object_json.insert((*name).into(), json!({"hash": h, "size": data.len()}));
    }
    let mut index = json!({"objects": object_json});
    let assets_name = match era {
        Era::Modern => "19",
        Era::Pre16 => {
            index["map_to_resources"] = json!(true);
            "pre-1.6"
        }
        Era::Virtual => {
            index["virtual"] = json!(true);
            "legacy"
        }
    };
    let index_bytes = index.to_string().into_bytes();
    let index_url = format!(
        "https://piston-meta.mojang.com/v1/packages/{}/{assets_name}.json",
        sha1_hex(&index_bytes)
    );
    streamer.add(&index_url, &index_bytes);

    let component = if era == Era::Modern {
        "java-runtime-delta"
    } else {
        "jre-legacy"
    };
    let mut version = json!({
        "id": mc, "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": assets_name,
        "assetIndex": {"id": assets_name, "sha1": sha1_hex(&index_bytes), "size": index_bytes.len(), "totalSize": 99999, "url": index_url},
        "downloads": {"client": {"sha1": sha1_hex(&client), "size": client.len(), "url": client_url}},
        "javaVersion": {"component": component, "majorVersion": if era == Era::Modern { 21 } else { 8 }},
        "libraries": libraries,
    });
    if era == Era::Modern {
        version["arguments"] = json!({
            "game": ["--username", "${auth_player_name}", "--version", "${version_name}", "--gameDir", "${game_directory}",
                "--assetsDir", "${assets_root}", "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}",
                "--accessToken", "${auth_access_token}", "--userType", "${user_type}", "--versionType", "${version_type}"],
            "jvm": ["-Djava.library.path=${natives_directory}", "-cp", "${classpath}"]
        });
    } else {
        version["minecraftArguments"] = json!("--username ${auth_player_name} --session ${auth_session} --gameDir ${game_directory} --assetsDir ${game_assets}");
    }
    if era == Era::Modern {
        version["logging"] = json!({"client": {"argument": "-Dlog4j.configurationFile=${path}", "type": "log4j2-xml",
            "file": {"id": "client-1.21.2.xml", "sha1": sha1_hex(&log), "size": log.len(), "url": log_url}}});
    }
    let version_bytes = version.to_string().into_bytes();
    let version_url = format!(
        "https://piston-meta.mojang.com/v1/packages/{}/{mc}.json",
        sha1_hex(&version_bytes)
    );
    fetcher.ok(&version_url, &version_bytes);
    fetcher.ok(
        MANIFEST_URL,
        json!({"latest": {"release": mc, "snapshot": mc}, "versions": [
            {"id": mc, "type": "release", "url": version_url, "time": "t", "releaseTime": "2024-12-03T10:12:57+00:00", "sha1": sha1_hex(&version_bytes), "complianceLevel": 1}
        ]})
        .to_string(),
    );
    let java_files = add_runtime(&fetcher, &streamer, &[component]);

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let progress = Progress::new();
    let tracker = Tracker::new(
        &InstanceId::new("t").unwrap(),
        progress,
        Box::new(move |p| sink_events.lock().unwrap().push(p)),
    );
    World {
        _dir: dir,
        root,
        fetcher,
        streamer,
        runner: FakeRunner::default(),
        events,
        tracker,
        cancel: Cancel::new(),
        mc: mc.into(),
        lib_url,
        client_url,
        asset_urls,
        java_files,
    }
}

#[cfg(test)]
pub(crate) fn ctx(w: &World) -> Ctx<'_, FakeFetcher, FakeStreamer, FakeRunner> {
    Ctx {
        root: &w.root,
        meta: &w.fetcher,
        loader_meta: &w.fetcher,
        streamer: &w.streamer,
        loader_streamer: &w.streamer,
        runner: &w.runner,
        sys: SystemEnv::from_process(),
        concurrency: 4,
        now: 1_000,
        backoff: Duration::ZERO,
        cancel: w.cancel.clone(),
        tracker: &w.tracker,
        verify_all: false,
        free_space: Arc::new(|_| None),
    }
}

#[cfg(test)]
pub(crate) fn create(w: &World, new: NewInstance) -> InstanceId {
    InstanceStore::new(&w.root).create(new, 1).unwrap().id
}

#[cfg(test)]
pub(crate) fn vanilla(w: &World) -> InstanceId {
    create(w, NewInstance::vanilla("Survival", &w.mc).unwrap())
}

#[cfg(test)]
pub(crate) fn setup_of(w: &World, id: &InstanceId) -> SetupState {
    InstanceStore::new(&w.root).get(id).unwrap().setup
}

#[cfg(test)]
pub(crate) fn stages(w: &World) -> Vec<Stage> {
    let mut out: Vec<Stage> = Vec::new();
    for e in w.events.lock().unwrap().iter() {
        if out.last() != Some(&e.stage) {
            out.push(e.stage);
        }
    }
    out
}

// ================================================================================ vanilla

#[tokio::test]
async fn a_modern_vanilla_profile_is_fully_prepared_and_marked_ready() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();

    let SetupState::Ready {
        version_id,
        loader_version,
    } = &inst.setup
    else {
        panic!("not ready")
    };
    assert_eq!(version_id.as_str(), "1.21.4");
    assert!(loader_version.is_none());

    let r = &w.root;
    assert!(version_json_path(r, "1.21.4").is_file());
    assert_eq!(fs::read(client_jar_path(r, "1.21.4")).unwrap().len(), 3000);
    assert!(r
        .libraries_dir()
        .join("com/example/alpha/1.0/alpha-1.0.jar")
        .is_file());
    assert!(r
        .libraries_dir()
        .join("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")
        .is_file());
    assert!(log_config_path(r, "client-1.21.2.xml").is_file());
    assert!(asset_index_path(r, "19").is_file());
    for u in &w.asset_urls {
        let name = u.rsplit('/').next().unwrap();
        assert!(object_path(r, &format!("{}/{name}", &name[..2])).is_file());
    }
    // Other platforms' natives were never fetched.
    assert!(w
        .streamer
        .calls()
        .iter()
        .all(|(u, _)| !u.contains("never-fetched")));
    // Natives live only in the instance.
    let natives = InstanceStore::new(r).dirs(&id).natives_dir();
    assert_eq!(
        fs::read(natives.join("windows/x64/org/lwjgl/lwjgl.dll")).unwrap(),
        b"dll-bytes"
    );
    assert!(!natives.join("META-INF").exists());
    // The delta runtime for Java 21 is installed under runtimes/java.
    let java = java_install::find_installed(r, "java-runtime-delta").unwrap();
    assert!(java.home.starts_with(r.java_runtimes_dir()));
    assert!(check_installed(r, &inst).is_ok());

    // Vanilla stays clean: only the manifest, natives and (empty) game folder exist.
    let dirs = InstanceStore::new(r).dirs(&id);
    assert!(!dirs.mods_dir().exists() && !dirs.saves_dir().exists());
    assert!(
        !dirs.game_dir().join("resources").exists(),
        "only pre-1.6 versions get resources"
    );

    // Real, ordered progress ending in "Ready to play."
    assert_eq!(
        stages(&w),
        [
            Stage::Planning,
            Stage::Java,
            Stage::Minecraft,
            Stage::Libraries,
            Stage::Assets,
            Stage::Natives,
            Stage::Verifying,
            Stage::Done
        ]
    );
    let events = w.events.lock().unwrap();
    let assets = events.iter().find(|e| e.stage == Stage::Assets).unwrap();
    assert_eq!(assets.label, "Downloading assets");
    assert_eq!(
        assets.items_total,
        Some(2),
        "the duplicate hash is fetched once"
    );
    assert!(assets.overall.is_some_and(|o| (0.0..=1.0).contains(&o)));
    let planning = events.iter().find(|e| e.stage == Stage::Planning).unwrap();
    assert!(
        planning.overall.is_none() && planning.bytes_total.is_none(),
        "no percentage before the totals are known"
    );
    let done = events.last().unwrap();
    assert_eq!(
        (done.stage, done.label.as_str()),
        (Stage::Done, "Ready to play.")
    );
    let snap = w.tracker.progress.snapshot();
    assert_eq!(
        snap.bytes_done, snap.bytes_total,
        "everything planned was accounted for"
    );
}

#[tokio::test]
async fn a_ready_profile_needs_no_network_to_stay_ready() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let requests = w.streamer.total_calls();
    // The network disappears completely.
    w.fetcher.fail_all();
    w.events.lock().unwrap().clear();
    let again = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(again.is_ready());
    assert_eq!(
        w.streamer.total_calls(),
        requests,
        "no file was requested again"
    );
    assert_eq!(
        stages(&w),
        [Stage::Planning, Stage::Done],
        "already-ready fast path must short-circuit before running preparation stages"
    );
}

#[tokio::test]
async fn an_interrupted_preparation_stays_pending_keeps_verified_files_and_resumes() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    let broken = w.asset_urls[1].clone();
    w.streamer.answer_status(&broken, 404);
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Network(_)), "{err:?}");
    assert!(
        matches!(setup_of(&w, &id), SetupState::Pending),
        "never Ready on a failure"
    );
    assert!(
        client_jar_path(&w.root, "1.21.4").is_file(),
        "verified files are kept"
    );

    w.streamer.clear_status(&broken);
    let jar_requests = w.streamer.count(&w.client_url);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(matches!(setup_of(&w, &id), SetupState::Ready { .. }));
    assert_eq!(
        w.streamer.count(&w.client_url),
        jar_requests,
        "the verified client jar was not downloaded again"
    );
    assert_eq!(w.streamer.count(&w.lib_url), 1);
}

#[tokio::test]
async fn a_download_cut_off_part_way_is_resumed_not_restarted() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    w.streamer.break_once_after(&w.client_url, 1000);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let calls: Vec<_> = w
        .streamer
        .calls()
        .into_iter()
        .filter(|c| c.0 == w.client_url)
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1].1 >= 1000,
        "second request starts where the first stopped: {calls:?}"
    );
    assert!(matches!(setup_of(&w, &id), SetupState::Ready { .. }));
}

#[tokio::test]
async fn a_truncated_or_corrupted_file_is_detected_and_repaired() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let lib = w
        .root
        .libraries_dir()
        .join("com/example/alpha/1.0/alpha-1.0.jar");
    let good = fs::read(&lib).unwrap();

    // Truncated: the offline check notices at once; preparing repairs it.
    fs::write(&lib, &good[..100]).unwrap();
    let inst = InstanceStore::new(&w.root).get(&id).unwrap();
    assert!(check_installed(&w.root, &inst).is_err());
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(fs::read(&lib).unwrap(), good);

    // Same size, different bytes: only a verification run (hashing) can see it, and it fixes it.
    let mut flipped = good.clone();
    flipped[10] ^= 0xFF;
    fs::write(&lib, &flipped).unwrap();
    let before = w.streamer.count(&w.lib_url);
    prepare_instance(&ctx(&w), &id).await.unwrap(); // quick check passes: sizes are right
    assert_eq!(w.streamer.count(&w.lib_url), before);
    let mut deep = ctx(&w);
    deep.verify_all = true;
    prepare_instance(&deep, &id).await.unwrap();
    assert_eq!(fs::read(&lib).unwrap(), good);
    assert_eq!(w.streamer.count(&w.lib_url), before + 1);
}

#[tokio::test]
async fn verify_files_also_checks_and_repairs_the_managed_java_runtime() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let java = java_install::find_installed(&w.root, "java-runtime-delta").unwrap();
    let modules = java.home.join("lib").join("modules");
    let good = fs::read(&modules).unwrap();

    // same size, different bytes (a damaged jvm/module file): launcher exe is intact, so the runtime still
    // resolves, and neither the quick check nor an ordinary preparation can see it
    let mut damaged = good.clone();
    damaged[7] ^= 0xFF;
    fs::write(&modules, &damaged).unwrap();
    assert!(java_install::find_installed(&w.root, "java-runtime-delta").is_some());
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(fs::read(&modules).unwrap(), damaged);

    // the explicit full verification finds it and replaces it, using the runtime's own manifest
    let mut deep = ctx(&w);
    deep.verify_all = true;
    prepare_instance(&deep, &id).await.unwrap();
    assert_eq!(fs::read(&modules).unwrap(), good);
    // nothing but the managed runtime was involved: it is still the one under runtimes/java
    let again = java_install::find_installed(&w.root, "java-runtime-delta").unwrap();
    assert!(again.exe.starts_with(w.root.java_runtimes_dir()));

    // and a missing file is restored too
    fs::remove_file(java.home.join("bin").join("java.exe")).unwrap();
    prepare_instance(&deep, &id).await.unwrap();
    assert!(java.home.join("bin").join("java.exe").is_file());
    fs::remove_file(java.home.join("bin").join("javaw.exe")).unwrap();
    prepare_instance(&deep, &id).await.unwrap();
    assert!(java.home.join("bin").join("javaw.exe").is_file());
}

#[tokio::test]
async fn cancelling_leaves_the_profile_pending() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    w.cancel.cancel();
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Cancelled), "{err:?}");
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));
}

#[tokio::test]
async fn offline_with_nothing_saved_fails_clearly_and_an_unknown_version_is_refused() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    w.fetcher.fail_all();
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Offline(_)), "{err:?}");
    assert_eq!(err.code(), "offline");

    let other = world("1.21.4", Era::Modern);
    let ghost = create(&other, NewInstance::vanilla("Ghost", "9.9.9").unwrap());
    let err = prepare_instance(&ctx(&other), &ghost).await.unwrap_err();
    assert!(matches!(err, PrepareError::Metadata(_)), "{err:?}");
}

#[tokio::test]
async fn metadata_that_fails_its_hash_is_never_used() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    // Replace the served version JSON by something else (same URL): the catalog's SHA-1 no longer matches.
    let manifest: Value = serde_json::from_slice(&w.fetcher_get(MANIFEST_URL)).unwrap();
    let url = manifest["versions"][0]["url"].as_str().unwrap().to_owned();
    w.fetcher.ok(&url, r#"{"id":"1.21.4","libraries":[]}"#);
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Verification(_)), "{err:?}");
    assert!(!version_json_path(&w.root, "1.21.4").exists());
}

#[cfg(test)]
impl World {
    fn fetcher_get(&self, url: &str) -> Vec<u8> {
        use futures_util::FutureExt;
        self.fetcher
            .get(&Url::parse(url).unwrap(), 1 << 20)
            .now_or_never()
            .unwrap()
            .unwrap()
    }
}

// ================================================================================ historical versions

#[tokio::test]
async fn a_pre_1_6_version_uses_the_historical_natives_map_and_resource_layout() {
    let w = world("b1.7.3", Era::Pre16);
    let id = create(&w, NewInstance::vanilla("Old", "b1.7.3").unwrap());
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(inst.is_ready());
    let dirs = InstanceStore::new(&w.root).dirs(&id);
    // The historical classifier jar was extracted (META-INF excluded), not placed on a classpath.
    assert_eq!(
        fs::read(dirs.natives_dir().join("lwjgl.dll")).unwrap(),
        b"old-style-dll"
    );
    assert!(!dirs.natives_dir().join("META-INF").exists());
    // Assets are mapped into this instance's game folder, by name.
    assert_eq!(
        fs::read(dirs.game_dir().join("resources/icons/icon_16x16.png"))
            .unwrap()
            .len(),
        400
    );
    assert_eq!(
        fs::read(dirs.game_dir().join("resources/minecraft/sounds.json"))
            .unwrap()
            .len(),
        900
    );
    // It asked for the legacy Java runtime, not the modern one.
    assert!(java_install::find_installed(&w.root, "jre-legacy").is_some());
    assert!(java_install::find_installed(&w.root, "java-runtime-delta").is_none());
    assert!(w
        .streamer
        .calls()
        .iter()
        .all(|(u, _)| !u.contains("never-fetched")));
}

#[tokio::test]
async fn the_1_6_virtual_asset_layout_is_materialised_in_the_shared_store() {
    let w = world("1.6.4", Era::Virtual);
    let id = create(&w, NewInstance::vanilla("Six", "1.6.4").unwrap());
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let virt = w.root.assets_dir().join("virtual").join("legacy");
    assert_eq!(
        fs::read(virt.join("icons/icon_16x16.png")).unwrap().len(),
        400
    );
    assert_eq!(fs::read(virt.join("copy/of_icon.png")).unwrap().len(), 400);
    let dirs = InstanceStore::new(&w.root).dirs(&id);
    assert!(
        !dirs.game_dir().join("resources").exists(),
        "a virtual layout writes nothing per instance"
    );
}

// ================================================================================ loaders

#[cfg(test)]
pub(crate) const FABRIC_ID: &str = "fabric-loader-0.16.9-1.21.4";

#[cfg(test)]
pub(crate) fn fabric_profile(w: &World, inherits: &str) -> (Vec<u8>, String) {
    let jar = bytes(1234, 77);
    let path = "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar";
    w.streamer
        .add(&format!("https://maven.fabricmc.net/{path}"), &jar);
    let profile = json!({
        "id": FABRIC_ID, "inheritsFrom": inherits, "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "libraries": [{"name": "net.fabricmc:fabric-loader:0.16.9", "url": "https://maven.fabricmc.net/", "sha1": sha1_hex(&jar), "size": jar.len()}]
    })
    .to_string();
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4/0.16.9/profile/json",
        &profile,
    );
    (jar, path.into())
}

#[tokio::test]
async fn fabric_is_installed_on_top_of_vanilla_and_its_profile_id_is_recorded() {
    let w = world("1.21.4", Era::Modern);
    let (jar, path) = fabric_profile(&w, "1.21.4");
    let lv = LoaderVersion::new("0.16.9").unwrap();
    let id = create(
        &w,
        NewInstance::modded(
            "Fab",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: lv.clone(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let SetupState::Ready {
        version_id,
        loader_version,
    } = &inst.setup
    else {
        panic!()
    };
    assert_eq!(version_id.as_str(), FABRIC_ID);
    assert_eq!(loader_version.as_ref(), Some(&lv));
    assert_eq!(fs::read(w.root.libraries_dir().join(&path)).unwrap(), jar);
    assert!(version_json_path(&w.root, FABRIC_ID).is_file());
    assert!(
        version_json_path(&w.root, "1.21.4").is_file(),
        "the vanilla version is there too"
    );
    // Modded instances get the loader's folders only from the user: nothing is created in game/.
    let dirs = InstanceStore::new(&w.root).dirs(&id);
    assert!(!dirs.mods_dir().join("anything").exists());
    assert!(
        w.runner.seen.lock().unwrap().is_empty(),
        "Fabric runs no installer"
    );
    assert!(stages(&w).contains(&Stage::Loader));
    // Offline afterwards.
    w.fetcher.fail_all();
    assert!(prepare_instance(&ctx(&w), &id).await.unwrap().is_ready());
}

#[tokio::test]
async fn a_loader_profile_for_another_minecraft_version_is_refused() {
    let w = world("1.21.4", Era::Modern);
    fabric_profile(&w, "1.20.1");
    let id = create(
        &w,
        NewInstance::modded(
            "Fab",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.16.9").unwrap(),
            },
        )
        .unwrap(),
    );
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Loader(_)), "{err:?}");
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));
}

#[tokio::test]
async fn an_automatic_loader_version_is_chosen_from_the_official_catalog() {
    let w = world("1.21.4", Era::Modern);
    fabric_profile(&w, "1.21.4");
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4",
        r#"[{"loader":{"version":"0.16.9","stable":true}},{"loader":{"version":"0.16.8","stable":true}}]"#,
    );
    let id = create(
        &w,
        NewInstance::modded("Fab", "1.21.4", LoaderKind::Fabric, LoaderVersionSpec::Auto).unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let SetupState::Ready { loader_version, .. } = &inst.setup else {
        panic!()
    };
    assert_eq!(loader_version.as_ref().unwrap().as_str(), "0.16.9");
}

#[cfg(test)]
pub(crate) const FORGE_ID: &str = "1.21.4-forge-54.1.14";
#[cfg(test)]
const FORGE_JAR_URL: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-installer.jar";

/// A Forge installer whose "run" writes what the real one writes, into the sandbox it is given.
#[cfg(test)]
pub(crate) fn forge_world() -> (World, Vec<u8>) {
    let w = world("1.21.4", Era::Modern);
    let installer = bytes(2048, 99);
    w.streamer.add(FORGE_JAR_URL, &installer);
    w.fetcher.ok(
        &format!("{FORGE_JAR_URL}.sha256"),
        format!("{}\n", sha256_of(&installer)),
    );
    let universal = bytes(600, 55);
    w.streamer.add("https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar", &universal);
    let universal_copy = universal.clone();
    *w.runner.on_run.lock().unwrap() = Some(Box::new(move |cmd| {
        let mc = PathBuf::from(cmd.args.last().unwrap());
        let vdir = mc.join("versions").join(FORGE_ID);
        fs::create_dir_all(&vdir)?;
        let profile = json!({
            "id": FORGE_ID, "inheritsFrom": "1.21.4", "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                {"name": "net.minecraftforge:forge:1.21.4-54.1.14:universal", "downloads": {"artifact": {
                    "path": "net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar",
                    "url": "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar",
                    "sha1": sha1_hex(&universal_copy), "size": universal_copy.len()}}},
                {"name": "net.minecraftforge:forge:1.21.4-54.1.14:client", "downloads": {"artifact": {
                    "path": "net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar", "url": "", "sha1": sha1_hex(b"patched"), "size": 7}}}
            ]
        });
        fs::write(vdir.join(format!("{FORGE_ID}.json")), profile.to_string())?;
        let gen = mc.join("libraries/net/minecraftforge/forge/1.21.4-54.1.14");
        fs::create_dir_all(&gen)?;
        fs::write(gen.join("forge-1.21.4-54.1.14-client.jar"), b"patched")?;
        // The installer also writes junk the launcher must never import.
        fs::write(mc.join("launcher_profiles.json"), b"{}")?;
        fs::write(cmd.cwd.join("tmp").join("installer.log"), b"log")?;
        Ok(RunResult {
            success: true,
            tail: String::new(),
        })
    }));
    (w, universal)
}

#[tokio::test]
async fn forge_runs_its_installer_only_inside_the_sandbox_and_the_result_is_merged() {
    let (w, universal) = forge_world();
    let lv = LoaderVersion::new("54.1.14").unwrap();
    let id = create(
        &w,
        NewInstance::modded(
            "Forge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: lv.clone(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let SetupState::Ready {
        version_id,
        loader_version,
    } = &inst.setup
    else {
        panic!()
    };
    assert_eq!(version_id.as_str(), FORGE_ID);
    assert_eq!(loader_version.as_ref(), Some(&lv));

    // The installer ran once, with the managed Java and a scrubbed, root-confined environment.
    {
        let seen = w.runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let cmd = &seen[0];
        assert!(cmd.program.starts_with(w.root.java_runtimes_dir()));
        assert!(cmd.cwd.starts_with(w.root.cache_dir()));
        let confined = [
            "TEMP",
            "TMP",
            "USERPROFILE",
            "HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ];
        for k in confined {
            assert!(
                Path::new(&cmd.env[k]).starts_with(w.root.cache_dir()),
                "{k} must point inside the Clade folder"
            );
        }
        assert!(!cmd.env.contains_key("JAVA_HOME") && !cmd.env.contains_key("CLASSPATH"));
        assert!(cmd.args.iter().any(|a| a == "--installClient"));
    }

    // Only libraries/ and versions/ were taken over, and the staging tree + installer jar are gone.
    assert!(version_json_path(&w.root, FORGE_ID).is_file());
    assert_eq!(
        fs::read(
            w.root
                .libraries_dir()
                .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar")
        )
        .unwrap(),
        universal
    );
    assert!(w
        .root
        .libraries_dir()
        .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar")
        .is_file());
    assert!(
        !w.root
            .downloads_dir()
            .join("forge-1.21.4-54.1.14-installer.jar")
            .exists(),
        "the temporary installer jar in cache/downloads is deleted after installation"
    );
    assert!(!w
        .root
        .minecraft_dir()
        .join("launcher_profiles.json")
        .exists());
    assert!(
        !w.root.cache_dir().join("loader-install").exists()
            || fs::read_dir(w.root.cache_dir().join("loader-install"))
                .unwrap()
                .count()
                == 0
    );

    // No percentage was invented while the installer ran.
    {
        let events = w.events.lock().unwrap();
        let running = events
            .iter()
            .find(|e| e.label == "Installing Forge")
            .unwrap();
        assert_eq!(
            (running.overall, running.bytes_total, running.bytes_done),
            (None, None, None)
        );
    }

    // And it stays playable offline.
    w.fetcher.fail_all();
    assert!(prepare_instance(&ctx(&w), &id).await.unwrap().is_ready());
}

#[tokio::test]
async fn an_installer_that_does_not_match_its_published_checksum_is_never_run() {
    let (w, _) = forge_world();
    w.fetcher
        .ok(&format!("{FORGE_JAR_URL}.sha256"), "0".repeat(64));
    let id = create(
        &w,
        NewInstance::modded(
            "Forge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("54.1.14").unwrap(),
            },
        )
        .unwrap(),
    );
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Verification(_)), "{err:?}");
    assert!(
        w.runner.seen.lock().unwrap().is_empty(),
        "an unverified executable is never executed"
    );
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));
    assert!(!w
        .root
        .downloads_dir()
        .join("forge-1.21.4-54.1.14-installer.jar")
        .exists());
}

#[tokio::test]
async fn a_failing_installer_leaves_the_profile_pending_and_the_sandbox_cleaned_up() {
    let (w, _) = forge_world();
    *w.runner.on_run.lock().unwrap() = Some(Box::new(|_| {
        Ok(RunResult {
            success: false,
            tail: "boom".into(),
        })
    }));
    let id = create(
        &w,
        NewInstance::modded(
            "Forge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("54.1.14").unwrap(),
            },
        )
        .unwrap(),
    );
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(
        matches!(&err, PrepareError::Loader(m) if m.contains("boom")),
        "{err:?}"
    );
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));
    let stage_dir = w.root.cache_dir().join("loader-install");
    assert!(!stage_dir.exists() || fs::read_dir(stage_dir).unwrap().count() == 0);
    // Vanilla was fully prepared before the loader step and is kept for the retry.
    assert!(client_jar_path(&w.root, "1.21.4").is_file());
}

#[tokio::test]
async fn a_generated_file_the_installer_failed_to_produce_blocks_ready() {
    let (w, _) = forge_world();
    let original = w.runner.on_run.lock().unwrap().take().unwrap();
    *w.runner.on_run.lock().unwrap() = Some(Box::new(move |cmd| {
        let r = original(cmd)?;
        let mc = PathBuf::from(cmd.args.last().unwrap());
        fs::remove_file(mc.join(
            "libraries/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar",
        ))?;
        Ok(r)
    }));
    let id = create(
        &w,
        NewInstance::modded(
            "Forge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("54.1.14").unwrap(),
            },
        )
        .unwrap(),
    );
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Loader(_)), "{err:?}");
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));
}

#[tokio::test]
async fn neoforge_and_quilt_use_the_same_paths() {
    // NeoForge: installer flow with its own Maven layout.
    let w = world("1.21.4", Era::Modern);
    let jar = bytes(1500, 42);
    let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.4.158/neoforge-21.4.158-installer.jar";
    w.streamer.add(url, &jar);
    w.fetcher.ok(&format!("{url}.sha256"), sha256_of(&jar));
    *w.runner.on_run.lock().unwrap() = Some(Box::new(|cmd| {
        let mc = PathBuf::from(cmd.args.last().unwrap());
        let vdir = mc.join("versions/neoforge-21.4.158");
        fs::create_dir_all(&vdir)?;
        fs::write(
            vdir.join("neoforge-21.4.158.json"),
            json!({"id": "neoforge-21.4.158", "inheritsFrom": "1.21.4", "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher", "libraries": []})
                .to_string(),
        )?;
        Ok(RunResult {
            success: true,
            tail: String::new(),
        })
    }));
    let id = create(
        &w,
        NewInstance::modded(
            "Neo",
            "1.21.4",
            LoaderKind::NeoForge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("21.4.158").unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let SetupState::Ready { version_id, .. } = &inst.setup else {
        panic!()
    };
    assert_eq!(version_id.as_str(), "neoforge-21.4.158");

    // Quilt: profile flow from its own host.
    let w = world("1.21.4", Era::Modern);
    let lib = bytes(500, 7);
    w.streamer.add("https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-loader/0.27.0/quilt-loader-0.27.0.jar", &lib);
    w.fetcher.ok(
        "https://meta.quiltmc.org/v3/versions/loader/1.21.4/0.27.0/profile/json",
        json!({"id": "quilt-loader-0.27.0-1.21.4", "inheritsFrom": "1.21.4", "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient", "libraries": [
            {"name": "org.quiltmc:quilt-loader:0.27.0", "url": "https://maven.quiltmc.org/repository/release/", "sha1": sha1_hex(&lib), "size": lib.len()}]})
        .to_string(),
    );
    let id = create(
        &w,
        NewInstance::modded(
            "Quilt",
            "1.21.4",
            LoaderKind::Quilt,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.27.0").unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let SetupState::Ready { version_id, .. } = &inst.setup else {
        panic!()
    };
    assert_eq!(version_id.as_str(), "quilt-loader-0.27.0-1.21.4");
}

// ================================================================================ portability

#[cfg(test)]
pub(crate) fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

#[cfg(test)]
pub(crate) fn text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            text_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
}

#[tokio::test]
async fn a_prepared_profile_survives_moving_the_whole_folder_and_stores_no_absolute_paths() {
    let (w, _) = forge_world();
    let id = create(
        &w,
        NewInstance::modded(
            "Forge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("54.1.14").unwrap(),
            },
        )
        .unwrap(),
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let vid = id.clone();
    let vanilla_id = create(&w, NewInstance::vanilla("Plain", "1.21.4").unwrap());
    prepare_instance(&ctx(&w), &vanilla_id).await.unwrap();

    // No file the launcher wrote mentions where the folder lives.
    let here = w
        .root
        .path()
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let mut jsons = Vec::new();
    text_files(w.root.path(), &mut jsons);
    assert!(!jsons.is_empty());
    for f in &jsons {
        let text = fs::read_to_string(f)
            .unwrap()
            .replace("\\\\", "/")
            .replace('\\', "/")
            .to_lowercase();
        assert!(
            !text.contains(&here),
            "{} stores an absolute path",
            f.display()
        );
    }

    // Move everything to a different place (another "drive"): still Ready, still checks out.
    let elsewhere = tempfile::tempdir().unwrap();
    let new_root = elsewhere.path().join("E_drive").join("Clade Launcher");
    copy_tree(w.root.path(), &new_root);
    let moved = PortableRoot::at(&new_root);
    let store = InstanceStore::new(&moved);
    for i in [&vid, &vanilla_id] {
        let inst = store.get(i).unwrap();
        assert!(inst.is_ready());
        assert!(
            check_installed(&moved, &inst).is_ok(),
            "{i} must still check out after the move"
        );
    }
}

#[tokio::test]
async fn everything_preparation_writes_stays_inside_the_folder() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    // The scratch locations are left empty, and every top-level folder is one of the layout's.
    assert_eq!(fs::read_dir(w.root.prepare_tmp_dir()).unwrap().count(), 0);
    let mut found: Vec<String> = fs::read_dir(w.root.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    let allowed = [
        "assets",
        "cache",
        "data",
        "instances",
        "libraries",
        "logs",
        "minecraft",
        "runtimes",
    ];
    for f in &found {
        assert!(
            allowed.contains(&f.as_str()),
            "unexpected top-level folder {f}"
        );
    }
}

// ================================================================================ disk space and verification runs

#[tokio::test]
async fn too_little_free_space_stops_before_anything_big_is_downloaded() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    let mut tight = ctx(&w);
    tight.free_space = Arc::new(|_| Some(10 * 1024 * 1024));
    let err = prepare_instance(&tight, &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::DiskSpace { .. }), "{err:?}");
    assert_eq!(err.code(), "disk_space");
    assert!(err.to_string().contains("free space") && err.to_string().contains("available"));
    assert_eq!(
        w.streamer.count(&w.client_url),
        0,
        "the client jar was never requested"
    );
    assert!(matches!(setup_of(&w, &id), SetupState::Pending));

    // With room it goes through.
    let mut roomy = ctx(&w);
    roomy.free_space = Arc::new(|_| Some(500 * 1024 * 1024 * 1024));
    prepare_instance(&roomy, &id).await.unwrap();

    // Once everything is installed there is nothing left to download, so a full drive is no obstacle
    // to confirming (or verifying) what is already there.
    let mut full = ctx(&w);
    full.free_space = Arc::new(|_| Some(1));
    full.verify_all = true;
    prepare_instance(&full, &id).await.unwrap();
}

#[tokio::test]
async fn a_verification_run_is_labelled_as_one_and_rehashes_every_file() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    prepare_instance(&ctx(&w), &id).await.unwrap();
    w.events.lock().unwrap().clear();
    let mut deep = ctx(&w);
    deep.verify_all = true;
    prepare_instance(&deep, &id).await.unwrap();
    let labels: Vec<String> = w
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.label.clone())
        .collect();
    for want in [
        "Verifying Minecraft",
        "Verifying libraries",
        "Verifying assets",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
    assert!(
        !labels.iter().any(|l| l.starts_with("Downloading")),
        "nothing needed downloading"
    );
}

#[tokio::test]
async fn libraries_with_neither_size_nor_hash_are_still_checked_through_the_size_recorded_at_install(
) {
    // Quilt (and Fabric's own jars) publish only a name and a base URL; the `.sha1` beside the file
    // is the only verification, once. A later truncation must still be noticed offline and repaired.
    let w = world("1.21.4", Era::Modern);
    let jar = bytes(900, 3);
    let path = "org/quiltmc/quilt-loader/0.27.0/quilt-loader-0.27.0.jar";
    let url = format!("https://maven.quiltmc.org/repository/release/{path}");
    w.streamer.add(&url, &jar);
    w.fetcher.ok(&format!("{url}.sha1"), sha1_hex(&jar));
    w.fetcher.ok(
        "https://meta.quiltmc.org/v3/versions/loader/1.21.4/0.27.0/profile/json",
        json!({"id": "quilt-loader-0.27.0-1.21.4", "inheritsFrom": "1.21.4", "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
               "libraries": [{"name": "org.quiltmc:quilt-loader:0.27.0", "url": "https://maven.quiltmc.org/repository/release/"}]})
        .to_string(),
    );
    let id = create(
        &w,
        NewInstance::modded(
            "Quilt",
            "1.21.4",
            LoaderKind::Quilt,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.27.0").unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(
        loader_prepare::read_sizes(&w.root, "quilt-loader-0.27.0-1.21.4").get(path),
        Some(&900)
    );
    assert!(check_installed(&w.root, &inst).is_ok());

    let file = w.root.libraries_dir().join(path);
    fs::write(&file, &jar[..100]).unwrap();
    assert!(
        check_installed(&w.root, &inst).is_err(),
        "the truncated jar is noticed with no network"
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(
        fs::read(&file).unwrap(),
        jar,
        "and repaired from the sidecar-verified download"
    );
    assert!(check_installed(&w.root, &InstanceStore::new(&w.root).get(&id).unwrap()).is_ok());
}

// ================================================================================ regressions & edge cases

#[tokio::test]
async fn unhashed_library_with_missing_sizes_record_or_zero_bytes_is_never_trusted_and_verify_all_rechecks_sidecar(
) {
    let w = world("1.21.4", Era::Modern);
    let jar = bytes(900, 31);
    let path = "org/quiltmc/quilt-loader/0.27.0/quilt-loader-0.27.0.jar";
    let url = format!("https://maven.quiltmc.org/repository/release/{path}");
    let quilt_id = "quilt-loader-0.27.0-1.21.4";
    w.streamer.add(&url, &jar);
    w.fetcher.ok(&format!("{url}.sha1"), sha1_hex(&jar));
    w.fetcher.ok(
        "https://meta.quiltmc.org/v3/versions/loader/1.21.4/0.27.0/profile/json",
        json!({
            "id": quilt_id,
            "inheritsFrom": "1.21.4",
            "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{"name": "org.quiltmc:quilt-loader:0.27.0", "url": "https://maven.quiltmc.org/repository/release/"}]
        })
        .to_string(),
    );
    let id = create(
        &w,
        NewInstance::modded(
            "Quilt",
            "1.21.4",
            LoaderKind::Quilt,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.27.0").unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    let file = w.root.libraries_dir().join(path);
    let sizes_file = w
        .root
        .minecraft_dir()
        .join("versions")
        .join(quilt_id)
        .join(format!("{quilt_id}.sizes.json"));

    // 1. Delete `.sizes.json` AND replace the jar with unverified bytes: must NOT be trusted!
    fs::remove_file(&sizes_file).unwrap();
    fs::write(&file, b"unverified-garbage").unwrap();
    assert!(
        check_installed(&w.root, &inst).is_err(),
        "missing .sizes.json for an unhashed library must fail offline check"
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(
        fs::read(&file).unwrap(),
        jar,
        "missing .sizes.json forced sidecar re-verification and repaired the jar"
    );
    assert!(sizes_file.is_file());

    // 2. A 0-byte jar (even if `.sizes.json` was forged to say 0) is rejected and repaired.
    fs::write(&file, b"").unwrap();
    fs::write(&sizes_file, json!({ path: 0 }).to_string()).unwrap();
    assert!(
        check_installed(&w.root, &inst).is_err(),
        "0-byte unhashed library must never pass offline check"
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(fs::read(&file).unwrap(), jar);

    // 3. Same-size byte corruption: `verify_all` fetches `.sha1` sidecar and repairs it.
    let mut flipped = jar.clone();
    flipped[5] ^= 0xAA;
    fs::write(&file, &flipped).unwrap();
    let before = w.streamer.count(&url);
    let mut deep = ctx(&w);
    deep.verify_all = true;
    prepare_instance(&deep, &id).await.unwrap();
    assert_eq!(fs::read(&file).unwrap(), jar);
    assert_eq!(w.streamer.count(&url), before + 1);
}

#[tokio::test]
async fn verify_all_detects_and_repairs_tampered_vanilla_and_loader_profile_jsons() {
    let w = world("1.21.4", Era::Modern);
    fabric_profile(&w, "1.21.4");
    let id = create(
        &w,
        NewInstance::modded(
            "Fab",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.16.9").unwrap(),
            },
        )
        .unwrap(),
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();

    let vanilla_json_file = version_json_path(&w.root, "1.21.4");
    let fabric_json_file = version_json_path(&w.root, FABRIC_ID);
    let good_vanilla = fs::read(&vanilla_json_file).unwrap();
    let good_fabric = fs::read(&fabric_json_file).unwrap();

    // Tamper both JSON files with syntactically valid metadata whose `mainClass` was swapped.
    let mut tampered_vanilla: Value = serde_json::from_slice(&good_vanilla).unwrap();
    tampered_vanilla["mainClass"] = json!("tampered.VanillaMain");
    fs::write(&vanilla_json_file, tampered_vanilla.to_string()).unwrap();

    let mut tampered_fabric: Value = serde_json::from_slice(&good_fabric).unwrap();
    tampered_fabric["mainClass"] = json!("tampered.FabricMain");
    fs::write(&fabric_json_file, tampered_fabric.to_string()).unwrap();

    // Ordinary run skips re-fetching valid-looking local JSONs, whereas `verify_all` checks
    // Mojang's SHA-1 and refreshes the official loader profile.
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_ne!(
        fs::read(&vanilla_json_file).unwrap(),
        good_vanilla,
        "ordinary run must leave valid-looking tampered <mc>.json untouched"
    );
    assert_ne!(
        fs::read(&fabric_json_file).unwrap(),
        good_fabric,
        "ordinary run must leave valid-looking tampered loader profile untouched"
    );

    let mut deep = ctx(&w);
    deep.verify_all = true;
    prepare_instance(&deep, &id).await.unwrap();

    assert_eq!(
        fs::read(&vanilla_json_file).unwrap(),
        good_vanilla,
        "verify_all must detect SHA-1 mismatch on <mc>.json and restore Mojang's copy"
    );
    let restored_fabric: Value =
        serde_json::from_slice(&fs::read(&fabric_json_file).unwrap()).unwrap();
    assert_eq!(
        restored_fabric["mainClass"],
        json!("net.fabricmc.loader.impl.launch.knot.KnotClient"),
        "verify_all must restore tampered Fabric profile JSON"
    );
}

#[tokio::test]
async fn tampered_generated_forge_jar_fails_verification_and_valid_forge_is_reused_offline() {
    // 1. An installer that writes corrupt bytes to its locally generated JAR (`url: ""`) is caught.
    {
        let (w, _) = forge_world();
        let original = w.runner.on_run.lock().unwrap().take().unwrap();
        *w.runner.on_run.lock().unwrap() = Some(Box::new(move |cmd| {
            let r = original(cmd)?;
            let mc = PathBuf::from(cmd.args.last().unwrap());
            fs::write(
                mc.join(
                    "libraries/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar",
                ),
                b"tamperd", // same 7-byte length, wrong SHA-1
            )?;
            Ok(r)
        }));
        let bad_id = create(
            &w,
            NewInstance::modded(
                "BadForge",
                "1.21.4",
                LoaderKind::Forge,
                LoaderVersionSpec::Pinned {
                    version: LoaderVersion::new("54.1.14").unwrap(),
                },
            )
            .unwrap(),
        );
        let err = prepare_instance(&ctx(&w), &bad_id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Verification(_)), "{err:?}");
        assert!(matches!(setup_of(&w, &bad_id), SetupState::Pending));
    }

    // 2. Once a valid Forge version is installed, a second Forge profile for the same MC + loader
    //    succeeds completely offline without running the installer again.
    let (w, _) = forge_world();
    let lv = LoaderVersion::new("54.1.14").unwrap();
    let first = create(
        &w,
        NewInstance::modded(
            "ForgeOne",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: lv.clone(),
            },
        )
        .unwrap(),
    );
    prepare_instance(&ctx(&w), &first).await.unwrap();
    assert_eq!(w.runner.seen.lock().unwrap().len(), 1);

    w.fetcher.fail_all();
    let downloads_before = w.streamer.total_calls();
    let second = create(
        &w,
        NewInstance::modded(
            "ForgeTwo",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: lv.clone(),
            },
        )
        .unwrap(),
    );
    let second_inst = prepare_instance(&ctx(&w), &second).await.unwrap();
    assert!(second_inst.is_ready());
    assert_eq!(
        second_inst.setup,
        SetupState::Ready {
            version_id: VersionId::new(FORGE_ID).unwrap(),
            loader_version: Some(lv.clone()),
        }
    );
    assert_eq!(
        w.runner.seen.lock().unwrap().len(),
        1,
        "reused Forge install must not run the installer JVM again"
    );
    assert_eq!(w.streamer.total_calls(), downloads_before);

    // 3. Corrupting the generated jar on disk is caught by `verify_generated_file` and prevents
    //    offline reuse by a third profile.
    let gen_jar = w
        .root
        .libraries_dir()
        .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar");
    fs::write(&gen_jar, b"corrupt").unwrap();
    let gf = GeneratedFile {
        dest: gen_jar.clone(),
        expected: Some(Expected::Sha1(sha1_hex(b"patched"))),
        size: Some(7),
    };
    assert!(matches!(
        verify_generated_file(&gf, &w.root),
        Err(PrepareError::Verification(_))
    ));
    let third = create(
        &w,
        NewInstance::modded(
            "ForgeThree",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned { version: lv },
        )
        .unwrap(),
    );
    assert!(
        prepare_instance(&ctx(&w), &third).await.is_err(),
        "corrupted generated jar must never be reused offline"
    );
}

#[tokio::test]
async fn forge_on_pre_1_17_minecraft_runs_installer_with_java_21_while_keeping_jre_legacy_for_game()
{
    let w = world("1.12.2", Era::Pre16);
    // Serve both the legacy Java 8 runtime (for MC 1.12.2) and modern Java 21 (for the installer).
    add_runtime(
        &w.fetcher,
        &w.streamer,
        &["jre-legacy", "java-runtime-delta"],
    );

    let forge_ver = "14.23.5.2860";
    let forge_id = "1.12.2-forge-14.23.5.2860";
    let installer_url = format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/1.12.2-{forge_ver}/forge-1.12.2-{forge_ver}-installer.jar"
    );
    let universal_url = format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/1.12.2-{forge_ver}/forge-1.12.2-{forge_ver}-universal.jar"
    );
    let installer = bytes(1800, 21);
    let universal = bytes(650, 22);
    w.streamer.add(&installer_url, &installer);
    w.fetcher
        .ok(&format!("{installer_url}.sha256"), sha256_of(&installer));
    w.streamer.add(&universal_url, &universal);

    let universal_copy = universal.clone();
    let universal_url_copy = universal_url.clone();
    *w.runner.on_run.lock().unwrap() = Some(Box::new(move |cmd| {
        let mc = PathBuf::from(cmd.args.last().unwrap());
        let vdir = mc.join("versions").join(forge_id);
        fs::create_dir_all(&vdir)?;
        let profile = json!({
            "id": forge_id,
            "inheritsFrom": "1.12.2",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${game_assets} --tweakClass net.minecraftforge.fml.common.launcher.FMLTweaker",
            "libraries": [
                {"name": format!("net.minecraftforge:forge:1.12.2-{forge_ver}:universal"), "downloads": {"artifact": {
                    "path": format!("net/minecraftforge/forge/1.12.2-{forge_ver}/forge-1.12.2-{forge_ver}-universal.jar"),
                    "url": universal_url_copy,
                    "sha1": sha1_hex(&universal_copy),
                    "size": universal_copy.len()
                }}}
            ]
        });
        fs::write(vdir.join(format!("{forge_id}.json")), profile.to_string())?;
        Ok(RunResult {
            success: true,
            tail: String::new(),
        })
    }));

    let id = create(
        &w,
        NewInstance::modded(
            "LegacyForge",
            "1.12.2",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new(forge_ver).unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(inst.is_ready());

    let legacy = java_install::find_installed(&w.root, "jre-legacy")
        .expect("jre-legacy must be installed for launching 1.12.2");
    let delta = java_install::find_installed(&w.root, "java-runtime-delta")
        .expect("java-runtime-delta must be installed for the Forge installer");
    assert_eq!(legacy.java_major, 8);
    assert_eq!(delta.java_major, 21);

    let seen = w.runner.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].program.starts_with(&delta.home),
        "installer JVM must use modern java-runtime-delta, got {}",
        seen[0].program.display()
    );

    let resolved = crate::launch_args::resolve(&w.root, forge_id).unwrap();
    assert_eq!(
        (resolved.java.component.as_str(), resolved.java.major),
        ("jre-legacy", 8),
        "launching 1.12.2 Forge still uses jre-legacy Java 8"
    );
}

#[tokio::test]
async fn loader_natives_override_and_supplement_vanilla_natives_because_extraction_runs_after_loader(
) {
    let w = world("1.21.4", Era::Modern);
    let loader_jar = bytes(512, 61);
    let loader_path = "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar";
    w.streamer.add(
        &format!("https://maven.fabricmc.net/{loader_path}"),
        &loader_jar,
    );

    // Override vanilla's `org.lwjgl:lwjgl:natives-windows` AND add a supplemental native library.
    let upgraded_lwjgl = zip_of(&[("windows/x64/org/lwjgl/lwjgl.dll", b"loader-upgraded-dll")]);
    let upgraded_path = "org/lwjgl/lwjgl/3.3.4/lwjgl-3.3.4-natives-windows.jar";
    w.streamer
        .add(&format!("{LIBS}/{upgraded_path}"), &upgraded_lwjgl);

    let extra_native = zip_of(&[("fabric_extra.dll", b"supplemental-dll")]);
    let extra_path = "net/fabricmc/extra-native/1.0/extra-native-1.0-natives-windows.jar";
    w.streamer.add(
        &format!("https://maven.fabricmc.net/{extra_path}"),
        &extra_native,
    );

    let profile = json!({
        "id": FABRIC_ID,
        "inheritsFrom": "1.21.4",
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "libraries": [
            {"name": "net.fabricmc:fabric-loader:0.16.9", "url": "https://maven.fabricmc.net/", "sha1": sha1_hex(&loader_jar), "size": loader_jar.len()},
            {"name": "org.lwjgl:lwjgl:3.3.4:natives-windows", "downloads": {"artifact": {
                "path": upgraded_path, "sha1": sha1_hex(&upgraded_lwjgl), "size": upgraded_lwjgl.len(), "url": format!("{LIBS}/{upgraded_path}")
            }}},
            {"name": "net.fabricmc:extra-native:1.0:natives-windows", "downloads": {"artifact": {
                "path": extra_path, "sha1": sha1_hex(&extra_native), "size": extra_native.len(), "url": format!("https://maven.fabricmc.net/{extra_path}")
            }}}
        ]
    })
    .to_string();
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4/0.16.9/profile/json",
        &profile,
    );

    let id = create(
        &w,
        NewInstance::modded(
            "FabNatives",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.16.9").unwrap(),
            },
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(inst.is_ready());

    let seen_stages = stages(&w);
    let loader_pos = seen_stages
        .iter()
        .position(|s| *s == Stage::Loader)
        .unwrap();
    let natives_pos = seen_stages
        .iter()
        .position(|s| *s == Stage::Natives)
        .unwrap();
    assert!(
        loader_pos < natives_pos,
        "Stage::Loader must run before Stage::Natives: {seen_stages:?}"
    );

    let natives = InstanceStore::new(&w.root).dirs(&id).natives_dir();
    assert_eq!(
        fs::read(natives.join("windows/x64/org/lwjgl/lwjgl.dll")).unwrap(),
        b"loader-upgraded-dll",
        "loader's native jar must override vanilla's"
    );
    assert_eq!(
        fs::read(natives.join("fabric_extra.dll")).unwrap(),
        b"supplemental-dll",
        "loader's supplemental native jar must also be extracted"
    );

    // Even if vanilla had no natives of its own, `check_files` enforces `natives/` when the loader has natives.
    let vpath = version_json_path(&w.root, "1.21.4");
    let mut vjson: Value = serde_json::from_slice(&fs::read(&vpath).unwrap()).unwrap();
    vjson["libraries"] = json!([vjson["libraries"][0].clone()]);
    fs::write(&vpath, vjson.to_string()).unwrap();
    assert!(check_installed(&w.root, &inst).is_ok());
    fs::remove_dir_all(&natives).unwrap();
    assert_eq!(
        check_installed(&w.root, &inst),
        Err("the extracted native libraries".into())
    );
}

#[tokio::test]
async fn missing_or_corrupted_virtual_and_mapped_resource_assets_are_detected_and_repaired() {
    // 1. Virtual layout (`assets/virtual/legacy/`).
    let w_virt = world("1.6.4", Era::Virtual);
    let id_virt = create(&w_virt, NewInstance::vanilla("Six", "1.6.4").unwrap());
    let inst_virt = prepare_instance(&ctx(&w_virt), &id_virt).await.unwrap();
    let virt_icon = w_virt
        .root
        .assets_dir()
        .join("virtual/legacy/icons/icon_16x16.png");
    let good_icon = fs::read(&virt_icon).unwrap();

    fs::remove_file(&virt_icon).unwrap();
    assert_eq!(
        check_installed(&w_virt.root, &inst_virt),
        Err("a game asset".into()),
        "missing virtual asset copy must fail check_installed"
    );
    w_virt.fetcher.fail_all(); // repaired locally from assets/objects/ with no network
    prepare_instance(&ctx(&w_virt), &id_virt).await.unwrap();
    assert_eq!(fs::read(&virt_icon).unwrap(), good_icon);

    // Same-size byte corruption in virtual asset is repaired by `verify_all`.
    let w_virt2 = world("1.6.4", Era::Virtual);
    let id_virt2 = create(&w_virt2, NewInstance::vanilla("Six2", "1.6.4").unwrap());
    prepare_instance(&ctx(&w_virt2), &id_virt2).await.unwrap();
    let virt_icon2 = w_virt2
        .root
        .assets_dir()
        .join("virtual/legacy/icons/icon_16x16.png");
    let mut bad_icon = good_icon.clone();
    bad_icon[0] ^= 0xFF;
    fs::write(&virt_icon2, &bad_icon).unwrap();
    let mut deep_virt = ctx(&w_virt2);
    deep_virt.verify_all = true;
    prepare_instance(&deep_virt, &id_virt2).await.unwrap();
    assert_eq!(fs::read(&virt_icon2).unwrap(), good_icon);

    // 2. Pre-1.6 mapped resource layout (`<game_dir>/resources/`).
    let w_res = world("b1.7.3", Era::Pre16);
    let id_res = create(&w_res, NewInstance::vanilla("Beta", "b1.7.3").unwrap());
    let inst_res = prepare_instance(&ctx(&w_res), &id_res).await.unwrap();
    let sounds = InstanceStore::new(&w_res.root)
        .dirs(&id_res)
        .game_dir()
        .join("resources/minecraft/sounds.json");
    let good_sounds = fs::read(&sounds).unwrap();

    fs::remove_file(&sounds).unwrap();
    assert_eq!(
        check_installed(&w_res.root, &inst_res),
        Err("a game asset".into()),
        "missing mapped resource asset must fail check_installed"
    );
    w_res.fetcher.fail_all();
    prepare_instance(&ctx(&w_res), &id_res).await.unwrap();
    assert_eq!(fs::read(&sounds).unwrap(), good_sounds);
}

#[test]
fn copy_named_preserves_stems_with_copy_tmp_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(dir.path());
    root.ensure_layout().unwrap();

    let src_png = root.assets_dir().join("objects/aa/src_png");
    let src_meta = root.assets_dir().join("objects/bb/src_meta");
    fs::create_dir_all(src_png.parent().unwrap()).unwrap();
    fs::create_dir_all(src_meta.parent().unwrap()).unwrap();
    let png_bytes = bytes(128, 41);
    let meta_bytes = bytes(96, 42);
    fs::write(&src_png, &png_bytes).unwrap();
    fs::write(&src_meta, &meta_bytes).unwrap();

    let out_dir = root.assets_dir().join("virtual/legacy");
    fs::create_dir_all(&out_dir).unwrap();
    // Sentinel proving `with_extension("copy.tmp")` (`pack.copy.tmp`) is NOT used.
    let sentinel = out_dir.join("pack.copy.tmp");
    fs::write(&sentinel, b"keep-me").unwrap();

    let dst_png = out_dir.join("pack.png");
    let dst_meta = out_dir.join("pack.mcmeta");
    let mut checked = HashSet::new();
    copy_named(
        &src_png,
        &dst_png,
        png_bytes.len() as u64,
        Some(&Expected::Sha1(sha1_hex(&png_bytes))),
        &mut checked,
        root.path(),
    )
    .unwrap();
    copy_named(
        &src_meta,
        &dst_meta,
        meta_bytes.len() as u64,
        Some(&Expected::Sha1(sha1_hex(&meta_bytes))),
        &mut checked,
        root.path(),
    )
    .unwrap();

    assert_eq!(fs::read(&dst_png).unwrap(), png_bytes);
    assert_eq!(fs::read(&dst_meta).unwrap(), meta_bytes);
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"keep-me",
        "copy_named must use <filename>.copy.tmp, never <stem>.copy.tmp"
    );
    assert!(!out_dir.join("pack.png.copy.tmp").exists());
    assert!(!out_dir.join("pack.mcmeta.copy.tmp").exists());
}

#[tokio::test]
async fn bytes_to_download_rounds_up_to_cluster_slack_counts_legacy_asset_copies_and_is_zero_when_present(
) {
    assert_eq!(round_up_cluster(0, 4096), 0);
    assert_eq!(round_up_cluster(1, 4096), 4096);
    assert_eq!(round_up_cluster(4096, 4096), 4096);
    assert_eq!(round_up_cluster(4097, 4096), 8192);
    assert_eq!(round_up_cluster(u64::MAX - 10, 4096), u64::MAX);

    let w = world("b1.7.3", Era::Pre16);
    let id = create(&w, NewInstance::vanilla("BetaSpace", "b1.7.3").unwrap());
    let game_dir = InstanceStore::new(&w.root).dirs(&id).game_dir();

    let c = ctx(&w);
    let vjson = ensure_version_json(&c, &McVersion::new("b1.7.3").unwrap())
        .await
        .unwrap();
    let plan = parse_version(&vjson, &WINDOWS_X64).unwrap();
    let dl = c.downloader();
    let (assets_name, index) = ensure_asset_index(&c, &dl, &plan).await.unwrap().unwrap();
    let rp = java_install::plan_runtime(&w.fetcher, &plan.java.component)
        .await
        .unwrap();

    let raw_sum = planned_bytes(&plan, Some(&index), Some(&rp));
    let before = bytes_to_download(
        &w.root,
        "b1.7.3",
        &[&plan],
        Some((&assets_name, &index)),
        &game_dir,
        &[&rp],
    );
    let cluster = fsutil::cluster_size_bytes(w.root.path()).max(1);
    assert_eq!(
        before % cluster,
        0,
        "footprint is a multiple of cluster size"
    );
    assert!(
        before > raw_sum,
        "cluster slack + mapped resource copies exceed raw download sum ({before} vs {raw_sum})"
    );

    prepare_instance(&ctx(&w), &id).await.unwrap();
    let after = bytes_to_download(
        &w.root,
        "b1.7.3",
        &[&plan],
        Some((&assets_name, &index)),
        &game_dir,
        &[],
    );
    assert_eq!(after, 0, "all files present on disk require 0 extra bytes");
}

// ================================================================================ JavaChoice::Runtime, failure paths & cross-file contracts

#[cfg(test)]
fn write_custom_runtime(
    root: &PortableRoot,
    id: &str,
    java_major: u32,
    with_cli_exe: bool,
) -> ManagedJava {
    let dir = crate::runtimes::runtime_dir(root, id).unwrap();
    fs::create_dir_all(dir.join("bin")).unwrap();
    let javaw_bytes = b"MZ custom javaw";
    fs::write(dir.join("bin/javaw.exe"), javaw_bytes).unwrap();
    let cli_bytes = b"MZ custom java cli";
    if with_cli_exe {
        fs::write(dir.join("bin/java.exe"), cli_bytes).unwrap();
    }
    let manifest = crate::runtimes::RuntimeManifest {
        schema: 1,
        id: id.to_owned(),
        vendor: "temurin".to_owned(),
        java_major,
        arch: "x64".to_owned(),
        java_exe: RelPath::new("bin/javaw.exe").unwrap(),
        java_exe_sha256: sha256_of(javaw_bytes),
        java_cli_exe: with_cli_exe.then(|| RelPath::new("bin/java.exe").unwrap()),
        java_cli_exe_sha256: with_cli_exe.then(|| sha256_of(cli_bytes)),
    };
    crate::runtimes::write_manifest(root, &manifest).unwrap();
    resolve_java(root, id).unwrap()
}

#[cfg(all(test, windows))]
fn create_junction(link: &Path, target: &Path) {
    let link_win = link.to_string_lossy().replace('/', "\\");
    let target_win = target.to_string_lossy().replace('/', "\\");
    let out = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link_win)
        .arg(target_win)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn java_choice_runtime_resolves_verifies_and_reports_missing_runtime() {
    // 1. Valid custom runtime (no runtime-files.json): skips Mojang's runtime index and succeeds.
    let w = world("1.21.4", Era::Modern);
    write_custom_runtime(&w.root, "custom-temurin-21", 21, true);
    w.fetcher.fail(RUNTIME_INDEX_URL);

    let mut new = NewInstance::vanilla("CustomJava", &w.mc).unwrap();
    new.java = JavaChoice::Runtime {
        id: "custom-temurin-21".into(),
    };
    let id = create(&w, new);
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(inst.is_ready());
    assert!(check_installed(&w.root, &inst).is_ok());
    assert!(
        java_install::find_installed(&w.root, "java-runtime-delta").is_none(),
        "explicit JavaChoice::Runtime must not install Mojang's default runtime"
    );
    assert!(
        !w.fetcher.calls().iter().any(|u| u == RUNTIME_INDEX_URL),
        "JavaChoice::Runtime must skip plan_runtime"
    );
    let snap = w.tracker.progress.snapshot();
    assert_eq!(snap.bytes_done, snap.bytes_total);

    // 2. `verify_all = true` passes `component = None` to `verification_files` and succeeds without
    //    consulting Mojang's runtime index when the chosen runtime has no `runtime-files.json`.
    let events_verify = Arc::new(Mutex::new(Vec::new()));
    let sink_verify = events_verify.clone();
    let tracker_verify = Tracker::new(
        &id,
        Progress::new(),
        Box::new(move |p| sink_verify.lock().unwrap().push(p)),
    );
    let mut deep = ctx(&w);
    deep.verify_all = true;
    deep.tracker = &tracker_verify;
    prepare_instance(&deep, &id).await.unwrap();
    assert!(
        !w.fetcher.calls().iter().any(|u| u == RUNTIME_INDEX_URL),
        "verify_all on JavaChoice::Runtime must not fall back to Mojang's runtime index"
    );
    let snap_verify = tracker_verify.progress.snapshot();
    assert_eq!(snap_verify.bytes_done, snap_verify.bytes_total);

    // 3. If the chosen runtime WAS installed by Mojang (has `runtime-files.json`), `verify_all`
    //    still checks and repairs damaged files locally even though `component = None`.
    let w2 = world("1.21.4", Era::Modern);
    let auto_id = vanilla(&w2);
    prepare_instance(&ctx(&w2), &auto_id).await.unwrap();
    let delta = java_install::find_installed(&w2.root, "java-runtime-delta").unwrap();
    let mut new_rt = NewInstance::vanilla("PinnedMojangJava", &w2.mc).unwrap();
    new_rt.java = JavaChoice::Runtime {
        id: delta.id.clone(),
    };
    let rt_id = create(&w2, new_rt);
    prepare_instance(&ctx(&w2), &rt_id).await.unwrap();

    let modules = delta.home.join("lib/modules");
    let good_modules = fs::read(&modules).unwrap();
    let mut bad_modules = good_modules.clone();
    bad_modules[3] ^= 0x5A;
    fs::write(&modules, &bad_modules).unwrap();
    w2.fetcher.fail(RUNTIME_INDEX_URL);
    let mut deep2 = ctx(&w2);
    deep2.verify_all = true;
    prepare_instance(&deep2, &rt_id).await.unwrap();
    assert_eq!(
        fs::read(&modules).unwrap(),
        good_modules,
        "verify_all must repair runtime-files.json entries for JavaChoice::Runtime even with component = None"
    );

    // 4. Missing chosen runtime fails `prepare_instance` with `PrepareError::Java` and fails
    //    `check_installed` when removed after setup.
    let mut missing_new = NewInstance::vanilla("MissingJava", &w.mc).unwrap();
    missing_new.java = JavaChoice::Runtime {
        id: "missing-jdk-21".into(),
    };
    let missing_id = create(&w, missing_new);
    let err = prepare_instance(&ctx(&w), &missing_id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Java(_)), "{err:?}");
    assert!(matches!(setup_of(&w, &missing_id), SetupState::Pending));

    fs::remove_file(
        crate::runtimes::runtime_dir(&w.root, "custom-temurin-21")
            .unwrap()
            .join("runtime.json"),
    )
    .unwrap();
    let err_check = check_installed(&w.root, &inst).unwrap_err();
    assert!(
        err_check.contains("chosen Java runtime"),
        "check_installed must report missing chosen runtime: {err_check}"
    );
}

#[tokio::test]
async fn java_choice_runtime_below_17_uses_java_21_for_forge_installer_and_keeps_chosen_runtime() {
    let (w, _) = forge_world();
    write_custom_runtime(&w.root, "custom-java-8", 8, true);

    let mut new = NewInstance::modded(
        "ForgeRuntime8",
        "1.21.4",
        LoaderKind::Forge,
        LoaderVersionSpec::Pinned {
            version: LoaderVersion::new("54.1.14").unwrap(),
        },
    )
    .unwrap();
    new.java = JavaChoice::Runtime {
        id: "custom-java-8".into(),
    };
    let id = create(&w, new);

    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(inst.is_ready());
    assert_eq!(
        inst.java,
        JavaChoice::Runtime {
            id: "custom-java-8".into()
        }
    );
    assert!(check_installed(&w.root, &inst).is_ok());

    let delta = java_install::find_installed(&w.root, "java-runtime-delta")
        .expect("java-runtime-delta must be installed for the Forge installer");
    let seen = w.runner.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].program.starts_with(&delta.home),
        "installer JVM must use Java 21 when JavaChoice::Runtime has java_major < 17, got {}",
        seen[0].program.display()
    );
}

#[tokio::test]
async fn unhashed_library_sidecar_rejects_missing_malformed_and_mismatched_sha1() {
    struct Status404SidecarFetcher<'a> {
        inner: &'a FakeFetcher,
    }
    impl Fetcher for Status404SidecarFetcher<'_> {
        fn get(
            &self,
            url: &Url,
            max_bytes: usize,
        ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send {
            let is_sha1 = url.as_str().ends_with(".sha1");
            let inner = self.inner;
            let url = url.clone();
            async move {
                if is_sha1 {
                    Err(FetchError::Status(404))
                } else {
                    inner.get(&url, max_bytes).await
                }
            }
        }
    }

    let jar = bytes(900, 17);
    let path = "org/quiltmc/quilt-loader/0.27.0/quilt-loader-0.27.0.jar";
    let url = format!("https://maven.quiltmc.org/repository/release/{path}");
    let setup_quilt = |w: &World| {
        w.streamer.add(&url, &jar);
        w.fetcher.ok(
            "https://meta.quiltmc.org/v3/versions/loader/1.21.4/0.27.0/profile/json",
            json!({
                "id": "quilt-loader-0.27.0-1.21.4",
                "inheritsFrom": "1.21.4",
                "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
                "libraries": [{"name": "org.quiltmc:quilt-loader:0.27.0", "url": "https://maven.quiltmc.org/repository/release/"}]
            })
            .to_string(),
        );
        create(
            w,
            NewInstance::modded(
                "Quilt",
                "1.21.4",
                LoaderKind::Quilt,
                LoaderVersionSpec::Pinned {
                    version: LoaderVersion::new("0.27.0").unwrap(),
                },
            )
            .unwrap(),
        )
    };

    // 1. Missing `.sha1` sidecar (404 HTTP status and offline route failure).
    {
        let w = world("1.21.4", Era::Modern);
        let id = setup_quilt(&w);
        let err_offline = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(
            matches!(err_offline, PrepareError::Offline(_)),
            "{err_offline:?}"
        );
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));

        let status_fetcher = Status404SidecarFetcher { inner: &w.fetcher };
        let c404 = Ctx {
            root: &w.root,
            meta: &status_fetcher,
            loader_meta: &status_fetcher,
            streamer: &w.streamer,
            loader_streamer: &w.streamer,
            runner: &w.runner,
            sys: SystemEnv::from_process(),
            concurrency: 4,
            now: 1_000,
            backoff: Duration::ZERO,
            cancel: w.cancel.clone(),
            tracker: &w.tracker,
            verify_all: false,
            free_space: Arc::new(|_| None),
        };
        let err_404 = prepare_instance(&c404, &id).await.unwrap_err();
        assert!(matches!(err_404, PrepareError::Network(_)), "{err_404:?}");
        assert!(!w.root.libraries_dir().join(path).exists());
    }

    // 2. Malformed `.sha1` sidecar.
    {
        let w = world("1.21.4", Era::Modern);
        let id = setup_quilt(&w);
        w.fetcher.ok(&format!("{url}.sha1"), "not-a-valid-sha1");
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Unsupported(_)), "{err:?}");
        assert!(!w.root.libraries_dir().join(path).exists());
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));
    }

    // 3. Mismatched `.sha1` sidecar.
    {
        let w = world("1.21.4", Era::Modern);
        let id = setup_quilt(&w);
        w.fetcher.ok(&format!("{url}.sha1"), "0".repeat(40));
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Verification(_)), "{err:?}");
        assert!(!w.root.libraries_dir().join(path).exists());
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));
    }
}

#[tokio::test]
async fn mid_flight_cancellation_during_download_and_installer_leaves_profile_pending() {
    use crate::net::{StreamHead, StreamSink};

    // 1. Mid-flight cancellation during client.jar download chunking leaves `.part` and resumes.
    struct CancellingStreamer<'a> {
        inner: &'a FakeStreamer,
        target_url: String,
        cancel: Arc<Cancel>,
    }
    struct CancelAfterFirstChunk<'a> {
        inner: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
        trigger: bool,
        fired: bool,
    }
    impl StreamSink for CancelAfterFirstChunk<'_> {
        fn begin(&mut self, head: &StreamHead) -> io::Result<()> {
            self.inner.begin(head)
        }
        fn chunk(&mut self, data: &[u8]) -> io::Result<()> {
            self.inner.chunk(data)?;
            if self.trigger && !self.fired {
                self.fired = true;
                self.cancel.cancel();
            }
            Ok(())
        }
    }
    impl Streamer for CancellingStreamer<'_> {
        fn stream<'a>(
            &'a self,
            url: &'a Url,
            from: u64,
            sink: &'a mut dyn StreamSink,
        ) -> impl Future<Output = Result<(), FetchError>> + Send + 'a {
            let trigger = url.as_str() == self.target_url;
            let mut wrapped = CancelAfterFirstChunk {
                inner: sink,
                cancel: &self.cancel,
                trigger,
                fired: false,
            };
            async move { self.inner.stream(url, from, &mut wrapped).await }
        }
    }

    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        let cancelling = CancellingStreamer {
            inner: &w.streamer,
            target_url: w.client_url.clone(),
            cancel: w.cancel.clone(),
        };
        let c = Ctx {
            root: &w.root,
            meta: &w.fetcher,
            loader_meta: &w.fetcher,
            streamer: &cancelling,
            loader_streamer: &cancelling,
            runner: &w.runner,
            sys: SystemEnv::from_process(),
            concurrency: 1,
            now: 1_000,
            backoff: Duration::ZERO,
            cancel: w.cancel.clone(),
            tracker: &w.tracker,
            verify_all: false,
            free_space: Arc::new(|_| None),
        };
        let err = prepare_instance(&c, &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Cancelled), "{err:?}");
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));
        let jar_dest = client_jar_path(&w.root, "1.21.4");
        assert!(!jar_dest.exists());
        assert!(
            crate::download::part_path(&jar_dest).is_file(),
            "mid-flight cancelled download keeps partial `.part` file for resume"
        );

        let mut resumed_ctx = ctx(&w);
        resumed_ctx.cancel = Cancel::new();
        let inst = prepare_instance(&resumed_ctx, &id).await.unwrap();
        assert!(inst.is_ready());
    }

    // 2. Mid-flight cancellation at Stage::Assets preserves verified client.jar.
    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        let cancel_on_assets = Cancel::new();
        let cancel_hook = cancel_on_assets.clone();
        let tracker = Tracker::new(
            &id,
            Progress::new(),
            Box::new(move |p| {
                if p.stage == Stage::Assets {
                    cancel_hook.cancel();
                }
            }),
        );
        let mut c = ctx(&w);
        c.cancel = cancel_on_assets;
        c.tracker = &tracker;
        let err = prepare_instance(&c, &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Cancelled), "{err:?}");
        assert!(client_jar_path(&w.root, "1.21.4").is_file());
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));
    }

    // 3. Mid-flight cancellation during FakeRunner::run cleans up staging and leaves Pending.
    {
        let (w, _) = forge_world();
        let cancel_in_runner = w.cancel.clone();
        let original = w.runner.on_run.lock().unwrap().take().unwrap();
        *w.runner.on_run.lock().unwrap() = Some(Box::new(move |cmd| {
            let res = original(cmd)?;
            cancel_in_runner.cancel();
            Ok(res)
        }));
        let id = create(
            &w,
            NewInstance::modded(
                "ForgeCancel",
                "1.21.4",
                LoaderKind::Forge,
                LoaderVersionSpec::Pinned {
                    version: LoaderVersion::new("54.1.14").unwrap(),
                },
            )
            .unwrap(),
        );
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Cancelled), "{err:?}");
        assert!(matches!(setup_of(&w, &id), SetupState::Pending));
        let stage_dir = w.root.cache_dir().join("loader-install");
        assert!(!stage_dir.exists() || fs::read_dir(stage_dir).unwrap().count() == 0);
    }
}

#[tokio::test]
async fn link_and_unsafe_path_guards_block_generated_files_asset_copies_and_unsafe_asset_names() {
    #[cfg(windows)]
    {
        // 1. `verify_generated_file` rejects an ancestor junction.
        let d = tempfile::tempdir().unwrap();
        let root = PortableRoot::at(d.path().join("root"));
        root.ensure_layout().unwrap();
        let outside = d.path().join("outside");
        fs::create_dir_all(outside.join("forge/1.21.4")).unwrap();
        fs::write(outside.join("forge/1.21.4/client.jar"), b"patched").unwrap();
        let net_dir = root.libraries_dir().join("net");
        fs::create_dir_all(&net_dir).unwrap();
        create_junction(&net_dir.join("minecraftforge"), &outside);

        let gf = GeneratedFile {
            dest: root
                .libraries_dir()
                .join("net/minecraftforge/forge/1.21.4/client.jar"),
            expected: Some(Expected::Sha1(sha1_hex(b"patched"))),
            size: Some(7),
        };
        assert!(matches!(
            verify_generated_file(&gf, &root),
            Err(PrepareError::Storage(_))
        ));

        // 2. `copy_named` rejects an ancestor junction on destination and a link at `.copy.tmp`.
        let src = root.assets_dir().join("objects/aa/aabbcc");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"icon-bytes").unwrap();
        let out_outside = d.path().join("out_outside");
        fs::create_dir_all(&out_outside).unwrap();
        let virt = root.assets_dir().join("virtual/legacy");
        fs::create_dir_all(&virt).unwrap();
        create_junction(&virt.join("linked_dir"), &out_outside);

        let mut checked = HashSet::new();
        assert!(copy_named(
            &src,
            &virt.join("linked_dir/icon.png"),
            10,
            None,
            &mut checked,
            root.path(),
        )
        .is_err());
        assert!(!out_outside.join("icon.png").exists());

        create_junction(&virt.join("real.png.copy.tmp"), &out_outside);
        assert!(copy_named(
            &src,
            &virt.join("real.png"),
            10,
            None,
            &mut checked,
            root.path(),
        )
        .is_err());
    }

    // 3. `ensure_assets` rejects an unsafe `RelPath` asset name in virtual/mapped layouts.
    let w = world("1.6.4", Era::Virtual);
    let data = b"evil-asset".to_vec();
    let hash = sha1_hex(&data);
    let obj_url = format!("{CDN}/{}/{hash}", &hash[..2]);
    w.streamer.add(&obj_url, &data);
    let bad_index = json!({
        "virtual": true,
        "objects": {
            "../escape.txt": {"hash": hash, "size": data.len()}
        }
    })
    .to_string()
    .into_bytes();
    let bad_index_url = format!(
        "https://piston-meta.mojang.com/v1/packages/{}/legacy-bad.json",
        sha1_hex(&bad_index)
    );
    w.streamer.add(&bad_index_url, &bad_index);

    let mut vjson: Value = serde_json::from_slice(
        &w.fetcher_get(
            serde_json::from_slice::<Value>(&w.fetcher_get(MANIFEST_URL)).unwrap()["versions"][0]
                ["url"]
                .as_str()
                .unwrap(),
        ),
    )
    .unwrap();
    vjson["assetIndex"] = json!({
        "id": "legacy-bad",
        "sha1": sha1_hex(&bad_index),
        "size": bad_index.len(),
        "totalSize": data.len(),
        "url": bad_index_url
    });
    let vbytes = vjson.to_string().into_bytes();
    let vurl = format!(
        "https://piston-meta.mojang.com/v1/packages/{}/1.6.4.json",
        sha1_hex(&vbytes)
    );
    w.fetcher.ok(&vurl, &vbytes);
    w.fetcher.ok(
        MANIFEST_URL,
        json!({"latest": {"release": "1.6.4", "snapshot": "1.6.4"}, "versions": [
            {"id": "1.6.4", "type": "release", "url": vurl, "time": "t", "releaseTime": "2024-12-03T10:12:57+00:00", "sha1": sha1_hex(&vbytes), "complianceLevel": 1}
        ]})
        .to_string(),
    );

    let id = create(&w, NewInstance::vanilla("UnsafeAsset", "1.6.4").unwrap());
    let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
    assert!(matches!(err, PrepareError::Unsupported(_)), "{err:?}");
    assert!(!w.root.assets_dir().join("virtual/escape.txt").exists());
}

#[tokio::test]
async fn version_json_recovery_rejection_missing_client_unhosted_lib_and_unknown_instance() {
    let serve_modified_version = |w: &World, mutate: &dyn Fn(&mut Value)| {
        let manifest: Value = serde_json::from_slice(&w.fetcher_get(MANIFEST_URL)).unwrap();
        let orig_url = manifest["versions"][0]["url"].as_str().unwrap().to_owned();
        let mut vjson: Value = serde_json::from_slice(&w.fetcher_get(&orig_url)).unwrap();
        mutate(&mut vjson);
        let bytes = vjson.to_string().into_bytes();
        let url = format!(
            "https://piston-meta.mojang.com/v1/packages/{}/{}.json",
            sha1_hex(&bytes),
            w.mc
        );
        w.fetcher.ok(&url, &bytes);
        w.fetcher.ok(
            MANIFEST_URL,
            json!({"latest": {"release": w.mc, "snapshot": w.mc}, "versions": [
                {"id": w.mc, "type": "release", "url": url, "time": "t", "releaseTime": "2024-12-03T10:12:57+00:00", "sha1": sha1_hex(&bytes), "complianceLevel": 1}
            ]})
            .to_string(),
        );
    };

    // 1. Corrupt local `<mc>.json` and mismatched-id local `<mc>.json` recover from network.
    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        let vpath = version_json_path(&w.root, "1.21.4");
        fs::create_dir_all(vpath.parent().unwrap()).unwrap();
        fs::write(&vpath, b"{not valid json").unwrap();
        let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
        assert!(inst.is_ready());

        // Mismatched id in local JSON forces re-fetch on a fresh Pending profile.
        let w2 = world("1.21.4", Era::Modern);
        let id2 = vanilla(&w2);
        let vpath2 = version_json_path(&w2.root, "1.21.4");
        let manifest: Value = serde_json::from_slice(&w2.fetcher_get(MANIFEST_URL)).unwrap();
        let orig_url = manifest["versions"][0]["url"].as_str().unwrap().to_owned();
        let mut wrong_local: Value = serde_json::from_slice(&w2.fetcher_get(&orig_url)).unwrap();
        wrong_local["id"] = json!("1.20.1");
        fs::create_dir_all(vpath2.parent().unwrap()).unwrap();
        fs::write(&vpath2, wrong_local.to_string()).unwrap();
        let inst2 = prepare_instance(&ctx(&w2), &id2).await.unwrap();
        assert!(inst2.is_ready());
        let saved: Value = serde_json::from_slice(&fs::read(&vpath2).unwrap()).unwrap();
        assert_eq!(saved["id"], json!("1.21.4"));
    }

    // 2. Remote `<mc>.json` with `parsed.id != mc` (even with matching manifest SHA-1) is rejected.
    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        serve_modified_version(&w, &|v| {
            v["id"] = json!("1.20.1");
        });
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Metadata(_)), "{err:?}");
        assert!(!version_json_path(&w.root, "1.21.4").exists());
    }

    // 3. Missing `client` download in version JSON returns `PrepareError::Unsupported`.
    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        serve_modified_version(&w, &|v| {
            v["downloads"] = json!({});
        });
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Unsupported(_)), "{err:?}");
    }

    // 4. Vanilla library with `url: None` (`url: ""`) returns `PrepareError::Unsupported`.
    {
        let w = world("1.21.4", Era::Modern);
        let id = vanilla(&w);
        serve_modified_version(&w, &|v| {
            v["libraries"].as_array_mut().unwrap().push(json!({
                "name": "com.example:local-only:1.0",
                "downloads": {"artifact": {"path": "com/example/local-only/1.0/local-only-1.0.jar", "url": "", "sha1": sha1_hex(b"x"), "size": 1}}
            }));
        });
        let err = prepare_instance(&ctx(&w), &id).await.unwrap_err();
        assert!(matches!(err, PrepareError::Unsupported(_)), "{err:?}");
    }

    // 5. Unknown `InstanceId` returns `PrepareError::NotFound`.
    {
        let w = world("1.21.4", Era::Modern);
        let ghost = InstanceId::new("unknown-profile").unwrap();
        let err = prepare_instance(&ctx(&w), &ghost).await.unwrap_err();
        assert!(matches!(err, PrepareError::NotFound), "{err:?}");
        assert_eq!(err.code(), "not_found");
    }
}

#[tokio::test]
async fn verify_all_on_installed_forge_reuses_installer_and_repairs_libraries() {
    let (w, universal) = forge_world();
    let lv = LoaderVersion::new("54.1.14").unwrap();
    let id = create(
        &w,
        NewInstance::modded(
            "ForgeVerify",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Pinned { version: lv },
        )
        .unwrap(),
    );
    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(w.runner.seen.lock().unwrap().len(), 1);

    // Same-size corruption on a downloaded Forge library: `verify_all = true` must reuse the
    // installed Forge profile (`PlannedLoader::ReusedInstaller`) and repair the jar without
    // spawning the installer again.
    let uni_path = w
        .root
        .libraries_dir()
        .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar");
    let mut damaged = universal.clone();
    damaged[12] ^= 0xFF;
    fs::write(&uni_path, &damaged).unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let tracker = Tracker::new(
        &id,
        Progress::new(),
        Box::new(move |p| sink.lock().unwrap().push(p)),
    );
    let mut deep = ctx(&w);
    deep.verify_all = true;
    deep.tracker = &tracker;
    let inst = prepare_instance(&deep, &id).await.unwrap();
    assert!(inst.is_ready());
    assert_eq!(fs::read(&uni_path).unwrap(), universal);
    assert_eq!(
        w.runner.seen.lock().unwrap().len(),
        1,
        "verify_all on intact Forge generated files must reuse installer output without re-running JVM"
    );
    let labels: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.label.clone())
        .collect();
    assert!(
        labels.iter().any(|l| l == "Verifying Forge"),
        "expected Verifying Forge stage label in {labels:?}"
    );
    let snap = tracker.progress.snapshot();
    assert_eq!(snap.bytes_done, snap.bytes_total);
}

#[tokio::test]
async fn auto_loader_pins_installed_version_during_repair_and_verify_even_when_catalog_updates() {
    // 1. Fabric Auto: pins the installed version during offline/repair AND verify_all even when
    //    the catalog updates to advertise a newer version.
    let w = world("1.21.4", Era::Modern);
    let jar_old = bytes(1100, 13);
    let path_old = "net/fabricmc/fabric-loader/0.16.8/fabric-loader-0.16.8.jar";
    let id_old = "fabric-loader-0.16.8-1.21.4";
    w.streamer
        .add(&format!("https://maven.fabricmc.net/{path_old}"), &jar_old);
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4/0.16.8/profile/json",
        json!({
            "id": id_old,
            "inheritsFrom": "1.21.4",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{"name": "net.fabricmc:fabric-loader:0.16.8", "url": "https://maven.fabricmc.net/", "sha1": sha1_hex(&jar_old), "size": jar_old.len()}]
        })
        .to_string(),
    );
    fabric_profile(&w, "1.21.4"); // registers 0.16.9 profile too, to catch any unintended switch
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4",
        r#"[{"loader":{"version":"0.16.8","stable":true}}]"#,
    );

    let id = create(
        &w,
        NewInstance::modded(
            "AutoFab",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Auto,
        )
        .unwrap(),
    );
    let inst = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(
        inst.setup,
        SetupState::Ready {
            version_id: VersionId::new(id_old).unwrap(),
            loader_version: Some(LoaderVersion::new("0.16.8").unwrap()),
        }
    );

    // Catalog now promotes 0.16.9 as the sole/newest stable release.
    w.fetcher.ok(
        "https://meta.fabricmc.net/v2/versions/loader/1.21.4",
        r#"[{"loader":{"version":"0.16.9","stable":true}}]"#,
    );

    // Delete the 0.16.8 loader jar so `already_ready` is false and full repair runs.
    let jar_old_path = w.root.libraries_dir().join(path_old);
    fs::remove_file(&jar_old_path).unwrap();
    let repaired = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert_eq!(
        repaired.setup, inst.setup,
        "repairing an Auto instance must keep the already-installed loader version pinned"
    );
    assert_eq!(fs::read(&jar_old_path).unwrap(), jar_old);

    // Full verification (`verify_all = true`) also preserves the pinned version.
    let mut deep = ctx(&w);
    deep.verify_all = true;
    let verified = prepare_instance(&deep, &id).await.unwrap();
    assert_eq!(
        verified.setup, inst.setup,
        "verifying an Auto instance must not switch loader versions"
    );
    assert!(
        !w.fetcher
            .calls()
            .iter()
            .any(|u| u.contains("/0.16.9/profile/json")),
        "0.16.9 profile must never be fetched while 0.16.8 is pinned"
    );

    // 2. Forge Auto: pins installed 54.1.14 across repair/verify even when promotions_slim.json
    //    advances to 54.1.18.
    let (wf, universal) = forge_world();
    wf.fetcher.ok(
        "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
        json!({"promos": {"1.21.4-recommended": "54.1.14", "1.21.4-latest": "54.1.14"}})
            .to_string(),
    );
    let forge_id = create(
        &wf,
        NewInstance::modded(
            "AutoForge",
            "1.21.4",
            LoaderKind::Forge,
            LoaderVersionSpec::Auto,
        )
        .unwrap(),
    );
    let forge_inst = prepare_instance(&ctx(&wf), &forge_id).await.unwrap();
    assert_eq!(
        forge_inst.setup,
        SetupState::Ready {
            version_id: VersionId::new(FORGE_ID).unwrap(),
            loader_version: Some(LoaderVersion::new("54.1.14").unwrap()),
        }
    );

    wf.fetcher.ok(
        "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
        json!({"promos": {"1.21.4-recommended": "54.1.18", "1.21.4-latest": "54.1.18"}})
            .to_string(),
    );
    let uni_path = wf
        .root
        .libraries_dir()
        .join("net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-universal.jar");
    let mut bad_uni = universal.clone();
    bad_uni[0] ^= 0xFF;
    fs::write(&uni_path, &bad_uni).unwrap();

    let mut deep_forge = ctx(&wf);
    deep_forge.verify_all = true;
    let verified_forge = prepare_instance(&deep_forge, &forge_id).await.unwrap();
    assert_eq!(verified_forge.setup, forge_inst.setup);
    assert_eq!(fs::read(&uni_path).unwrap(), universal);
    assert_eq!(
        wf.runner.seen.lock().unwrap().len(),
        1,
        "Forge Auto verify must reuse 54.1.14 without spawning an installer for 54.1.18"
    );
}

#[tokio::test]
async fn progress_plan_bytes_is_not_double_counted_across_fresh_and_verify_runs() {
    let w = world("1.21.4", Era::Modern);
    let (fabric_jar, _) = fabric_profile(&w, "1.21.4");
    let id = create(
        &w,
        NewInstance::modded(
            "PlanBytes",
            "1.21.4",
            LoaderKind::Fabric,
            LoaderVersionSpec::Pinned {
                version: LoaderVersion::new("0.16.9").unwrap(),
            },
        )
        .unwrap(),
    );

    // Compute exact expected bytes independently from served metadata.
    let manifest: Value = serde_json::from_slice(&w.fetcher_get(MANIFEST_URL)).unwrap();
    let vurl = manifest["versions"][0]["url"].as_str().unwrap().to_owned();
    let vplan = parse_version(&w.fetcher_get(&vurl), &WINDOWS_X64).unwrap();
    let asset_index_size = vplan.asset_index.as_ref().unwrap().download.size;
    let client_size = vplan.client.as_ref().unwrap().size;
    let logging_size = vplan.logging.as_ref().unwrap().download.size;
    let vanilla_libs_size: u64 = vplan.libraries.iter().filter_map(|l| l.artifact.size).sum();
    // `world()` serves 3 object entries where 2 share the same 400-byte hash + 1 is 900 bytes.
    let unique_assets_size = 400u64 + 900u64;
    // `add_runtime()` serves 300 + 200 + 2000 = 2500 bytes.
    let java_runtime_size = 2500u64;
    let expected_total = asset_index_size
        + client_size
        + logging_size
        + vanilla_libs_size
        + unique_assets_size
        + java_runtime_size
        + fabric_jar.len() as u64;

    // 1. Fresh run: every planned byte is counted once and matches bytes_done at completion.
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let snap_fresh = w.tracker.progress.snapshot();
    assert_eq!(
        snap_fresh.bytes_total, expected_total,
        "fresh run must not double-count planned bytes"
    );
    assert_eq!(snap_fresh.bytes_done, snap_fresh.bytes_total);

    // 2. Full verification run on a fresh Progress/Tracker: java_verify + asset index + vanilla +
    //    assets + loader libraries are each planned once and match bytes_done.
    let tracker_verify = Tracker::new(&id, Progress::new(), Box::new(|_| {}));
    let mut deep = ctx(&w);
    deep.verify_all = true;
    deep.tracker = &tracker_verify;
    prepare_instance(&deep, &id).await.unwrap();
    let snap_verify = tracker_verify.progress.snapshot();
    assert_eq!(
        snap_verify.bytes_total, expected_total,
        "verify_all run must not double-count planned bytes"
    );
    assert_eq!(snap_verify.bytes_done, snap_verify.bytes_total);
}
