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
        _cancel: &'a Cancel,
    ) -> impl Future<Output = io::Result<RunResult>> + Send + 'a {
        self.seen.lock().unwrap().push(cmd.clone());
        let result = match &*self.on_run.lock().unwrap() {
            Some(f) => f(cmd),
            None => Err(io::Error::other("no installer scripted")),
        };
        async move { result }
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
    let again = prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(again.is_ready());
    assert_eq!(
        w.streamer.total_calls(),
        requests,
        "no file was requested again"
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

    // same size, different bytes (a damaged jvm/module file): java.exe is intact, so the runtime still
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
        futures::executor::block_on(self.fetcher.get(&Url::parse(url).unwrap(), 1 << 20)).unwrap()
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
        let mc = cmd.cwd.join("mc");
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

    // Only libraries/ and versions/ were taken over, and the staging tree is gone.
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
        fs::remove_file(cmd.cwd.join(
            "mc/libraries/net/minecraftforge/forge/1.21.4-54.1.14/forge-1.21.4-54.1.14-client.jar",
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
    assert!(prepare_instance(&ctx(&w), &id).await.is_err());
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
        let vdir = cmd.cwd.join("mc/versions/neoforge-21.4.158");
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
    assert_eq!(fs::read_dir(w.root.tmp_dir()).unwrap().count(), 0);
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
        "downloads",
        "instances",
        "libraries",
        "logs",
        "minecraft",
        "runtimes",
        "updates",
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
