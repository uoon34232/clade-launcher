#[cfg(test)]
use super::*;
#[cfg(test)]
use crate::instances::{InstanceStore, NewInstance};
#[cfg(test)]
use serde_json::{json, Value};

#[cfg(test)]
const H: &str = "0123456789abcdef0123456789abcdef01234567";
#[cfg(test)]
const UUID: &str = "00000000-0000-4000-8000-00000000a001";
#[cfg(test)]
const TOKEN: &str = "TEST-ONLY-TOKEN-do-not-use-0123456789";

#[cfg(test)]
struct Env {
    _dir: tempfile::TempDir,
    root: PortableRoot,
}

#[cfg(test)]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(dir.path().join("Clade Folder")); // a path with a space on purpose
    root.ensure_layout().unwrap();
    Env { _dir: dir, root }
}

#[cfg(test)]
fn touch(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

#[cfg(test)]
fn write_version(root: &PortableRoot, id: &str, v: &Value) {
    let p = version_file(root, id);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, v.to_string()).unwrap();
}

#[cfg(test)]
fn lib(name: &str, path: &str) -> Value {
    json!({"name": name, "downloads": {"artifact": {"path": path, "sha1": H, "size": 1, "url": format!("https://libraries.minecraft.net/{path}")}}})
}

/// Creates the on-disk files a prepared version has (jar, libraries, asset index, logging config).
#[cfg(test)]
fn install(root: &PortableRoot, id: &str, v: &Value, libs: &[&str], assets: Option<(&str, Value)>) {
    write_version(root, id, v);
    if v.pointer("/downloads/client").is_some() {
        touch(
            &root
                .minecraft_dir()
                .join("versions")
                .join(id)
                .join(format!("{id}.jar")),
        );
    }
    for l in libs {
        touch(&root.libraries_dir().join(l));
    }
    if let Some((name, idx)) = assets {
        let p = root
            .assets_dir()
            .join("indexes")
            .join(format!("{name}.json"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, idx.to_string()).unwrap();
    }
    if let Some(f) = v.pointer("/logging/client/file/id").and_then(Value::as_str) {
        touch(&root.assets_dir().join("log_configs").join(f));
    }
}

#[cfg(test)]
fn client() -> Value {
    json!({"sha1": H, "size": 1, "url": "https://piston-data.mojang.com/c.jar"})
}

#[cfg(test)]
fn asset_ref(id: &str) -> Value {
    json!({"id": id, "sha1": H, "size": 1, "totalSize": 1, "url": format!("https://piston-meta.mojang.com/{id}.json")})
}

#[cfg(test)]
fn modern_vanilla() -> Value {
    json!({
        "id": "1.21.4", "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": "19",
        "assetIndex": asset_ref("19"), "downloads": {"client": client()},
        "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
        "logging": {"client": {"argument": "-Dlog4j.configurationFile=${path}", "type": "log4j2-xml",
            "file": {"id": "client-1.21.2.xml", "sha1": H, "size": 1, "url": "https://piston-data.mojang.com/x.xml"}}},
        "libraries": [
            lib("com.google.code.gson:gson:2.11.0", "com/google/code/gson/gson/2.11.0/gson-2.11.0.jar"),
            {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows", "rules": [{"action": "allow", "os": {"name": "windows"}}],
             "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar", "sha1": H, "size": 1, "url": "https://libraries.minecraft.net/x.jar"}}},
            {"name": "org.lwjgl:lwjgl:3.3.3:natives-linux", "rules": [{"action": "allow", "os": {"name": "linux"}}],
             "downloads": {"artifact": {"path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar", "sha1": H, "size": 1, "url": "https://libraries.minecraft.net/y.jar"}}},
        ],
        "arguments": {
            "game": ["--username", "${auth_player_name}", "--version", "${version_name}", "--gameDir", "${game_directory}",
                "--assetsDir", "${assets_root}", "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}",
                "--accessToken", "${auth_access_token}", "--clientId", "${clientid}", "--xuid", "${auth_xuid}",
                "--userType", "${user_type}", "--versionType", "${version_type}",
                {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
                {"rules": [{"action": "allow", "features": {"has_custom_resolution": true}}], "value": ["--width", "${resolution_width}"]}],
            "jvm": [
                {"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]},
                {"rules": [{"action": "allow", "os": {"name": "windows"}}], "value": "-XX:HeapDumpPath=MojangTricks.heapdump"},
                {"rules": [{"action": "allow", "os": {"arch": "x86"}}], "value": "-Xss1M"},
                "-Djava.library.path=${natives_directory}", "-Dminecraft.launcher.brand=${launcher_name}",
                "-Dminecraft.launcher.version=${launcher_version}", "-cp", "${classpath}"]
        }
    })
}

#[cfg(test)]
const VANILLA_LIBS: [&str; 2] = [
    "com/google/code/gson/gson/2.11.0/gson-2.11.0.jar",
    "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar",
];

#[cfg(test)]
fn modern_index() -> Value {
    json!({"objects": {"a": {"hash": H, "size": 1}}})
}

#[cfg(test)]
fn java_for(root: &PortableRoot, major: u32) -> ManagedJava {
    let home = root
        .java_runtimes_dir()
        .join("java-runtime-delta--21.0.7--windows-x64");
    ManagedJava {
        id: "java-runtime-delta--21.0.7--windows-x64".into(),
        java_major: major,
        exe: home.join("bin").join("java.exe"),
        home,
    }
}

#[cfg(test)]
fn identity() -> Identity {
    Identity {
        uuid: UUID.into(),
        name: "Steve_01".into(),
        access_token: Secret::new(TOKEN.into()),
    }
}

#[cfg(test)]
fn build_for(
    e: &Env,
    launch_id: &str,
    major: u32,
    memory: u32,
    extra: &[String],
) -> Result<GameCommand, LaunchError> {
    let store = InstanceStore::new(&e.root);
    let inst = store
        .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 1)
        .unwrap();
    let dirs = store.dirs(&inst.id);
    let resolved = resolve(&e.root, launch_id)?;
    let java = java_for(&e.root, major);
    let sys = SystemEnv::from_process();
    let temp = e.root.cache_dir().join("game-tmp").join(inst.id.as_str());
    build(&BuildInput {
        root: &e.root,
        dirs: &dirs,
        resolved: &resolved,
        java: &java,
        identity: &identity(),
        memory_mb: memory,
        extra_jvm_args: extra,
        sys: &sys,
        temp_dir: &temp,
    })
}

#[cfg(test)]
fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} missing in {args:?}"));
    &args[i + 1]
}

#[cfg(test)]
fn classpath(args: &[String]) -> Vec<String> {
    after(args, "-cp").split(';').map(str::to_owned).collect()
}

// ================================================================================ modern Vanilla

#[test]
fn modern_vanilla_gets_the_metadata_arguments_in_the_right_order() {
    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    let cmd = build_for(&e, "1.21.4", 21, 4096, &[]).unwrap();
    let a = &cmd.args;

    assert!(cmd.program.ends_with("bin/java.exe") || cmd.program.ends_with("bin\\java.exe"));
    assert!(
        cmd.program.starts_with(e.root.java_runtimes_dir()),
        "the managed runtime, never a system one"
    );
    assert_eq!(a[0], "-Xmx4096M");
    let main = a
        .iter()
        .position(|x| x == "net.minecraft.client.main.Main")
        .unwrap();
    assert!(
        a[..main].iter().all(|x| x != "--username"),
        "JVM arguments come before the main class"
    );
    assert!(
        a[main + 1..].iter().all(|x| x != "-cp"),
        "game arguments come after it"
    );

    // Rules: Windows x64 only; no macOS flag, no 32-bit flag, no demo mode, no custom resolution.
    assert!(a.contains(&"-XX:HeapDumpPath=MojangTricks.heapdump".to_string()));
    for never in ["-XstartOnFirstThread", "-Xss1M", "--demo", "--width"] {
        assert!(!a.iter().any(|x| x == never), "{never}");
    }
    assert!(a.contains(&"-Dminecraft.launcher.brand=Clade".to_string()));
    assert!(a.contains(&format!(
        "-Dminecraft.launcher.version={}",
        env!("CARGO_PKG_VERSION")
    )));

    // The classpath: libraries applicable to Windows, then the client jar.
    let cp = classpath(a);
    assert_eq!(
        cp.len(),
        3,
        "gson, lwjgl natives (windows) and the client jar; not the Linux natives: {cp:?}"
    );
    assert!(
        cp[0].contains("gson-2.11.0.jar")
            && cp[1].contains("natives-windows")
            && cp[2].ends_with("1.21.4.jar")
    );
    assert!(!cp.iter().any(|p| p.contains("natives-linux")));

    // Identity and isolation.
    let game_dir = InstanceStore::new(&e.root)
        .dirs(&InstanceStore::new(&e.root).list().unwrap().instances[0].id)
        .game_dir();
    assert_eq!(after(a, "--gameDir"), game_dir.to_str().unwrap());
    assert_eq!(
        cmd.cwd, game_dir,
        "the game runs in the instance's own folder"
    );
    assert_eq!(after(a, "--username"), "Steve_01");
    assert_eq!(
        after(a, "--uuid"),
        "0000000000004000800000000000a001",
        "undashed, like the official launcher"
    );
    assert_eq!(after(a, "--accessToken"), TOKEN);
    assert_eq!(after(a, "--userType"), "msa");
    assert_eq!(after(a, "--versionType"), "release");
    assert_eq!(after(a, "--version"), "1.21.4");
    assert_eq!(after(a, "--assetIndex"), "19");
    assert_eq!(
        after(a, "--assetsDir"),
        e.root.assets_dir().to_str().unwrap()
    );
    assert_eq!(after(a, "--clientId"), "");
    assert_eq!(after(a, "--xuid"), "");
    assert!(a
        .iter()
        .any(|x| x.starts_with("-Djava.library.path=") && x.contains("natives")));
    let log_cfg = e
        .root
        .assets_dir()
        .join("log_configs")
        .join("client-1.21.2.xml");
    assert!(a.contains(&format!(
        "-Dlog4j.configurationFile={}",
        log_cfg.to_str().unwrap()
    )));
    assert!(a.contains(&format!(
        "-Duser.home={}",
        InstanceStore::new(&e.root)
            .dirs(&InstanceStore::new(&e.root).list().unwrap().instances[0].id)
            .home_dir()
            .to_str()
            .unwrap()
    )));

    // No placeholder is left unresolved, and no argument is a shell string.
    assert!(!a.iter().any(|x| x.contains("${")), "{a:?}");
    assert!(!a.iter().any(|x| x == "cmd" || x == "/c" || x == "-c"));
}

#[test]
fn memory_and_extra_jvm_arguments_come_from_the_profile_and_system() {
    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    let cmd = build_for(&e, "1.21.4", 21, 6144, &["-Dmy.flag=1".into()]).unwrap();
    assert_eq!(cmd.args[0], "-Xmx6144M");
    let main = cmd
        .args
        .iter()
        .position(|x| x == "net.minecraft.client.main.Main")
        .unwrap();
    assert_eq!(
        cmd.args[main - 1],
        "-Dmy.flag=1",
        "user arguments sit last among the JVM arguments"
    );
    // Java 8 gets the collector Java 17+ already defaults to; newer Java gets nothing extra.
    assert!(!cmd.args.contains(&"-XX:+UseG1GC".to_string()));
    let old = build_for(&e, "1.21.4", 8, 2048, &[]).unwrap();
    assert!(old.args.contains(&"-XX:+UseG1GC".to_string()));
}

// ================================================================================ historical versions

#[test]
fn a_pre_1_13_version_uses_minecraft_arguments_and_the_default_jvm_arguments() {
    let e = env();
    let v = json!({
        "id": "1.12.2", "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": "1.12",
        "assetIndex": asset_ref("1.12"), "downloads": {"client": client()},
        "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userType ${user_type} --versionType ${version_type}",
        "libraries": [lib("com.mojang:authlib:1.5.25", "com/mojang/authlib/1.5.25/authlib-1.5.25.jar"), lib("com.mojang:netty:1.8.8", "com/mojang/netty/1.8.8/netty-1.8.8.jar")]
    });
    install(
        &e.root,
        "1.12.2",
        &v,
        &[
            "com/mojang/authlib/1.5.25/authlib-1.5.25.jar",
            "com/mojang/netty/1.8.8/netty-1.8.8.jar",
        ],
        Some(("1.12", json!({"objects": {}}))),
    );
    let a = build_for(&e, "1.12.2", 8, 2048, &[]).unwrap().args;
    for needed in ["-Djava.library.path", "-Dminecraft.launcher.brand=Clade"] {
        assert!(
            a.iter().any(|x| x.starts_with(needed)),
            "default JVM argument {needed}"
        );
    }
    assert_eq!(classpath(&a).len(), 3);
    assert_eq!(after(&a, "--username"), "Steve_01");
    assert_eq!(after(&a, "--accessToken"), TOKEN);
    assert_eq!(after(&a, "--assetIndex"), "1.12");
}

#[test]
fn the_pre_1_6_layout_uses_positional_arguments_and_resources_in_the_game_folder() {
    let e = env();
    let v = json!({
        "id": "b1.7.3", "type": "old_beta", "mainClass": "net.minecraft.launchwrapper.Launch", "assets": "pre-1.6",
        "assetIndex": asset_ref("pre-1.6"), "downloads": {"client": client()},
        "minecraftArguments": "${auth_player_name} ${auth_session} --gameDir ${game_directory} --assetsDir ${game_assets}",
        "libraries": [lib("net.minecraft:launchwrapper:1.5", "net/minecraft/launchwrapper/1.5/launchwrapper-1.5.jar")]
    });
    install(
        &e.root,
        "b1.7.3",
        &v,
        &["net/minecraft/launchwrapper/1.5/launchwrapper-1.5.jar"],
        Some(("pre-1.6", json!({"map_to_resources": true, "objects": {}}))),
    );
    let a = build_for(&e, "b1.7.3", 8, 2048, &[]).unwrap().args;
    let main = a
        .iter()
        .position(|x| x == "net.minecraft.launchwrapper.Launch")
        .unwrap();
    assert_eq!(a[main + 1], "Steve_01");
    assert_eq!(
        a[main + 2],
        format!("token:{TOKEN}:0000000000004000800000000000a001"),
        "the legacy session string"
    );
    let game = after(&a, "--gameDir").to_owned();
    assert_eq!(
        after(&a, "--assetsDir"),
        format!("{game}\\resources").replace('\\', std::path::MAIN_SEPARATOR_STR)
    );
}

#[test]
fn the_1_6_virtual_layout_points_at_the_shared_virtual_assets() {
    let e = env();
    let v = json!({
        "id": "1.6.4", "type": "release", "mainClass": "net.minecraft.client.main.Main", "assets": "legacy",
        "assetIndex": asset_ref("legacy"), "downloads": {"client": client()},
        "minecraftArguments": "--username ${auth_player_name} --session ${auth_session} --version ${version_name} --gameDir ${game_directory} --assetsDir ${game_assets}",
        "libraries": []
    });
    install(
        &e.root,
        "1.6.4",
        &v,
        &[],
        Some(("legacy", json!({"virtual": true, "objects": {}}))),
    );
    let a = build_for(&e, "1.6.4", 8, 2048, &[]).unwrap().args;
    assert_eq!(
        after(&a, "--assetsDir"),
        e.root
            .assets_dir()
            .join("virtual")
            .join("legacy")
            .to_str()
            .unwrap()
    );
    assert_eq!(
        after(&a, "--session"),
        format!("token:{TOKEN}:0000000000004000800000000000a001")
    );
}

// ================================================================================ loaders

#[cfg(test)]
fn with_vanilla(e: &Env) {
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
}

#[test]
fn fabric_and_quilt_inherit_from_vanilla_with_their_libraries_first() {
    let e = env();
    with_vanilla(&e);
    let fabric = json!({
        "id": "fabric-loader-0.16.9-1.21.4", "inheritsFrom": "1.21.4", "type": "release",
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "arguments": {"game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "]},
        "libraries": [
            {"name": "net.fabricmc:fabric-loader:0.16.9", "url": "https://maven.fabricmc.net/", "sha1": H},
            {"name": "com.google.code.gson:gson:2.9.0", "url": "https://maven.fabricmc.net/", "sha1": H}]
    });
    install(
        &e.root,
        "fabric-loader-0.16.9-1.21.4",
        &fabric,
        &[
            "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar",
            "com/google/code/gson/gson/2.9.0/gson-2.9.0.jar",
        ],
        None,
    );
    let a = build_for(&e, "fabric-loader-0.16.9-1.21.4", 21, 2048, &[])
        .unwrap()
        .args;
    assert!(a
        .iter()
        .any(|x| x == "net.fabricmc.loader.impl.launch.knot.KnotClient"));
    assert!(
        !a.iter().any(|x| x == "net.minecraft.client.main.Main"),
        "only the loader's main class"
    );
    assert!(
        a.contains(&"-DFabricMcEmu= net.minecraft.client.main.Main ".to_string()),
        "a JVM argument with spaces stays one argument"
    );
    let cp = classpath(&a);
    assert!(
        cp[0].contains("fabric-loader-0.16.9.jar"),
        "the loader's libraries come first: {cp:?}"
    );
    assert_eq!(
        cp.iter().filter(|p| p.contains("gson")).count(),
        1,
        "the duplicate library keeps the loader's copy"
    );
    assert!(
        cp.iter().any(|p| p.contains("gson-2.9.0"))
            && !cp.iter().any(|p| p.contains("gson-2.11.0"))
    );
    assert!(
        cp.last().unwrap().ends_with("1.21.4.jar"),
        "the Vanilla jar closes the classpath"
    );
    assert_eq!(
        after(&a, "--version"),
        "1.21.4",
        "the base Minecraft version"
    );
    assert_eq!(
        after(&a, "--username"),
        "Steve_01",
        "Vanilla's game arguments still apply"
    );
}

#[test]
fn neoforge_style_module_path_arguments_expand_library_directory_and_separator() {
    let e = env();
    with_vanilla(&e);
    let neo = json!({
        "id": "neoforge-21.4.158", "inheritsFrom": "1.21.4", "type": "release",
        "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
        "arguments": {
            "jvm": ["-DignoreList=client-extra,${version_name}.jar", "-DlibraryDirectory=${library_directory}",
                "-p", "${library_directory}/cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar${classpath_separator}${library_directory}/cpw/mods/securejarhandler/3.0.8/securejarhandler-3.0.8.jar",
                "--add-modules", "ALL-MODULE-PATH"],
            "game": ["--fml.mcVersion", "1.21.4", "--launchTarget", "neoforgeclient"]
        },
        "libraries": [lib("cpw.mods:bootstraplauncher:2.0.2", "cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar")]
    });
    install(
        &e.root,
        "neoforge-21.4.158",
        &neo,
        &["cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar"],
        None,
    );
    let a = build_for(&e, "neoforge-21.4.158", 21, 2048, &[])
        .unwrap()
        .args;
    assert!(
        a.contains(&"-DignoreList=client-extra,1.21.4.jar".to_string()),
        "matches the Vanilla jar's name"
    );
    let lib = e.root.libraries_dir().to_str().unwrap().to_owned();
    assert!(a.contains(&format!("-DlibraryDirectory={lib}")));
    let p = after(&a, "-p");
    assert!(
        p.starts_with(&lib) && p.contains(";") && p.contains("securejarhandler-3.0.8.jar"),
        "{p}"
    );
    assert!(!p.contains("${"));
    // Vanilla's own game arguments plus the loader's, in that order.
    let launch = a.iter().position(|x| x == "--launchTarget").unwrap();
    assert!(a.iter().position(|x| x == "--username").unwrap() < launch);
    assert_eq!(a[launch + 1], "neoforgeclient");
    assert!(a
        .iter()
        .any(|x| x == "cpw.mods.bootstraplauncher.BootstrapLauncher"));
}

#[test]
fn a_loop_in_the_inheritance_chain_and_a_missing_parent_are_refused() {
    let e = env();
    write_version(
        &e.root,
        "a",
        &json!({"id": "a", "inheritsFrom": "b", "libraries": []}),
    );
    write_version(
        &e.root,
        "b",
        &json!({"id": "b", "inheritsFrom": "a", "libraries": []}),
    );
    assert!(matches!(
        resolve(&e.root, "a"),
        Err(LaunchError::BadVersion(_))
    ));
    write_version(
        &e.root,
        "c",
        &json!({"id": "c", "inheritsFrom": "nowhere", "libraries": []}),
    );
    assert!(matches!(resolve(&e.root, "c"), Err(LaunchError::MissingVersion(m)) if m == "nowhere"));
}

// ================================================================================ safety

#[test]
fn missing_files_stop_the_launch_with_a_clear_reason() {
    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS[..1],
        Some(("19", modern_index())),
    );
    let err = build_for(&e, "1.21.4", 21, 2048, &[]).unwrap_err();
    assert!(
        matches!(&err, LaunchError::MissingFile(m) if m.contains("lwjgl")),
        "{err:?}"
    );

    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    fs::remove_file(e.root.minecraft_dir().join("versions/1.21.4/1.21.4.jar")).unwrap();
    assert!(
        matches!(build_for(&e, "1.21.4", 21, 2048, &[]), Err(LaunchError::MissingFile(m)) if m.contains("Minecraft 1.21.4"))
    );

    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    fs::remove_file(e.root.assets_dir().join("log_configs/client-1.21.2.xml")).unwrap();
    assert!(matches!(
        build_for(&e, "1.21.4", 21, 2048, &[]),
        Err(LaunchError::MissingFile(_))
    ));
}

#[test]
fn the_token_never_appears_in_the_loggable_forms_of_the_command() {
    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    let cmd = build_for(&e, "1.21.4", 21, 2048, &[]).unwrap();
    assert!(
        cmd.args.iter().any(|a| a == TOKEN),
        "the real argument carries it"
    );
    let shown = cmd.redacted(&e.root);
    assert!(!shown.contains(TOKEN), "{shown}");
    assert!(shown.contains("--accessToken <redacted>"));
    assert!(
        shown.contains("<clade>"),
        "paths are shown relative to the Clade folder"
    );
    assert!(!shown.contains(e.root.path().to_str().unwrap()));
    assert!(!format!("{cmd:?}").contains(TOKEN));
    assert!(!format!("{:?}", identity()).contains(TOKEN));
    assert_eq!(cmd.secrets(), [TOKEN]);

    // A legacy session string embeds the token too, and is hidden as well.
    let e = env();
    let v = json!({"id": "1.6.4", "type": "release", "mainClass": "m.Main", "assets": "legacy", "assetIndex": asset_ref("legacy"),
        "downloads": {"client": client()}, "minecraftArguments": "--session ${auth_session} ${auth_player_name}", "libraries": []});
    install(
        &e.root,
        "1.6.4",
        &v,
        &[],
        Some(("legacy", json!({"virtual": true, "objects": {}}))),
    );
    let cmd = build_for(&e, "1.6.4", 8, 2048, &[]).unwrap();
    assert!(!cmd.redacted(&e.root).contains(TOKEN));
}

#[test]
fn the_process_environment_is_clean_and_confined_to_the_clade_folder() {
    let e = env();
    install(
        &e.root,
        "1.21.4",
        &modern_vanilla(),
        &VANILLA_LIBS,
        Some(("19", modern_index())),
    );
    let cmd = build_for(&e, "1.21.4", 21, 2048, &[]).unwrap();
    for k in [
        "JAVA_HOME",
        "CLASSPATH",
        "JAVA_TOOL_OPTIONS",
        "_JAVA_OPTIONS",
        "JDK_JAVA_OPTIONS",
    ] {
        assert!(!cmd.env.contains_key(k), "{k}");
    }
    for k in [
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        assert!(
            Path::new(&cmd.env[k]).starts_with(e.root.path()),
            "{k} must point inside the Clade folder"
        );
    }
    // Only the read-only Windows directory is allowed outside the folder.
    let outside: Vec<_> = cmd
        .env
        .iter()
        .filter(|(k, v)| {
            !Path::new(v.as_str()).starts_with(e.root.path())
                && !matches!(k.as_str(), "SystemRoot" | "windir" | "PATH")
        })
        .collect();
    assert!(outside.is_empty(), "{outside:?}");
    let process = cmd.to_process();
    assert_eq!(process.get_program(), cmd.program.as_os_str());
    assert_eq!(process.get_current_dir(), Some(cmd.cwd.as_path()));
}

#[test]
fn unknown_placeholders_become_empty_instead_of_leaking_through() {
    let mut unknown = Vec::new();
    let vars = BTreeMap::from([("a", "1".to_string())]);
    assert_eq!(
        substitute("x=${a} y=${nope} z", &vars, &mut unknown),
        "x=1 y= z"
    );
    assert_eq!(unknown, ["nope"]);
    assert_eq!(substitute("broken ${a", &vars, &mut unknown), "broken ${a");
}

// ================================================================================ the real thing (by hand)

/// Run by hand against a Clade folder whose profiles were prepared by the real app:
/// `cargo test live_ -- --ignored --nocapture`. For every prepared profile it builds the real
/// command and starts the REAL Java with it, only to prove the command is right (classpath, natives,
/// loader bootstrap): it stops the game as soon as Minecraft reports who is playing, or after a few
/// seconds for versions that log nothing. The identity is an obviously fake development placeholder
/// (this is a wiring check, not a way to play: production code only ever builds an identity from a
/// real signed-in account), so a real game never gets as far as joining anything.
#[cfg(test)]
#[tokio::test]
#[ignore = "starts the real Java and Minecraft; needs a prepared Clade folder"]
async fn live_real_profiles_build_commands_and_the_real_jvm_accepts_them() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let root = PortableRoot::at(
        std::env::var_os("CLADE_LIVE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("target")
                    .join("debug")
                    .join("clade-root")
            }),
    );
    let store = InstanceStore::new(&root);
    let sys = SystemEnv::from_process();
    let mut failures = Vec::new();
    for inst in store.list().unwrap().instances {
        let crate::instances::SetupState::Ready { version_id, .. } = &inst.setup else {
            continue;
        };
        let resolved =
            resolve(&root, version_id.as_str()).unwrap_or_else(|e| panic!("{}: {e}", inst.name));
        let java = crate::java_install::find_installed(&root, &resolved.java.component)
            .unwrap_or_else(|| panic!("{}: no Java", inst.name));
        let dirs = store.dirs(&inst.id);
        let temp = root.cache_dir().join("game-tmp").join(inst.id.as_str());
        for d in [
            dirs.game_dir(),
            temp.clone(),
            dirs.home_dir().join("AppData").join("Roaming"),
            dirs.home_dir().join("AppData").join("Local"),
        ] {
            fs::create_dir_all(d).unwrap();
        }
        let cmd = build(&BuildInput {
            root: &root,
            dirs: &dirs,
            resolved: &resolved,
            java: &java,
            identity: &Identity {
                uuid: UUID.into(),
                name: "CladeWiringCheck".into(),
                access_token: Secret::new("WIRING-CHECK-NOT-A-CREDENTIAL".into()),
            },
            memory_mb: 2048,
            extra_jvm_args: &[],
            sys: &sys,
            temp_dir: &temp,
        })
        .unwrap_or_else(|e| panic!("{}: {e}", inst.name));
        println!(
            "== {} ({}) java {} cp entries {}",
            inst.name,
            resolved.launch_id,
            java.id,
            classpath(&cmd.args).len()
        );
        println!(
            "   {}",
            &cmd.redacted(&root)[..cmd.redacted(&root).len().min(260)]
        );

        let mut child = tokio::process::Command::from(cmd.to_process());
        child
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = child.spawn().expect("java starts");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        for pipe in [
            child
                .stdout
                .take()
                .map(|s| Box::pin(s) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>),
            child.stderr.take().map(|s| Box::pin(s) as _),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    let _ = tx.send(l);
                }
            });
        }
        drop(tx);
        let started = std::time::Instant::now();
        let mut seen: Vec<String> = Vec::new();
        let mut verdict = None;
        while started.elapsed() < std::time::Duration::from_secs(40) {
            tokio::select! {
                line = rx.recv() => match line {
                    Some(l) => {
                        let ok = l.contains("Setting user") || l.contains("Backend library");
                        seen.push(l);
                        if ok { verdict = Some("minecraft reached its main startup"); break; }
                    }
                    None => break,
                },
                status = child.wait() => { verdict = Some(if status.map(|s| s.success()).unwrap_or(false) { "exited cleanly" } else { "EXITED EARLY" }); break; }
                () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                    // Versions that print nothing (the oldest) just have to stay alive for a while.
                    if resolved.logging.is_none() && started.elapsed() > std::time::Duration::from_secs(8) && child.try_wait().ok().flatten().is_none() { verdict = Some("alive after 8 s (no log markers in this version)"); break; }
                }
            }
        }
        let verdict = verdict.unwrap_or("timed out waiting for startup");
        let _ = child.kill().await;
        let _ = child.wait().await;
        println!(
            "   -> {verdict} after {:.1}s",
            started.elapsed().as_secs_f32()
        );
        if verdict.starts_with("EXITED") || verdict.starts_with("timed out") {
            for l in seen.iter().rev().take(12).rev() {
                println!("      | {l}");
            }
            failures.push(inst.name.clone());
        }
        let _ = fs::remove_dir_all(&temp);
    }
    assert!(
        failures.is_empty(),
        "profiles that did not start: {failures:?}"
    );
}
