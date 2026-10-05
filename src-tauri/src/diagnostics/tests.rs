#[cfg(test)]
use super::*;

use zip::ZipArchive;

fn facts(now: i64) -> Facts<'static> {
    Facts {
        version: "0.1.0",
        layout_version: 1,
        webview: "Bundled",
        webview_version: "154.0",
        system_locale: Some("ru-RU"),
        total_memory_mb: Some(16_384),
        free_disk_bytes: Some(50 * 1024 * 1024 * 1024),
        now_unix: now,
    }
}

fn root(dir: &Path) -> PortableRoot {
    let r = PortableRoot::at(dir);
    r.ensure_layout().unwrap();
    r
}

fn read_zip(path: &Path) -> Vec<(String, String)> {
    let mut z = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            (f.name().to_owned(), s)
        })
        .collect()
}

#[test]
fn timestamps_are_utc_and_sortable() {
    assert_eq!(timestamp(0), "19700101-000000");
    assert_eq!(timestamp(1_700_000_000), "20231114-221320");
    assert!(timestamp(1_700_000_000) < timestamp(1_700_000_001));
}

#[test]
fn secrets_are_removed_from_log_text() {
    let d = tempfile::tempdir().unwrap();
    let cases = [
        "Authorization: Bearer abc123SECRETvalue",
        "POST /x?access_token=SECRETTOKEN123&x=1",
        r#"{"refresh_token":"M.R3_BAY.SECRETREFRESH","a":1}"#,
        "launching with --accessToken SECRETARGUMENT --uuid 0000",
        "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.signaturepartXYZ end",
        "password=hunter2hunter2 passphrase: correct horse",
        "contact someone.name+tag@example.com please",
    ];
    for c in cases {
        let out = redact(d.path(), c);
        for secret in [
            "abc123SECRETvalue",
            "SECRETTOKEN123",
            "SECRETREFRESH",
            "SECRETARGUMENT",
            "signaturepartXYZ",
            "hunter2hunter2",
            "correct",
            "someone.name",
        ] {
            assert!(!out.contains(secret), "{secret} survived in {out:?}");
        }
    }
    // ordinary words that merely contain a key are untouched
    assert_eq!(
        redact(d.path(), "barcode encoded sessions"),
        "barcode encoded sessions"
    );
    assert_eq!(
        redact(d.path(), "Java 21 started in 3 s"),
        "Java 21 started in 3 s"
    );
}

#[test]
fn personal_paths_become_placeholders() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("Clade-Launcher");
    let text = format!(
        "root {} and {} user C:\\Users\\Alice Smith\\AppData\\Roaming and /Users/bob/x",
        r.display(),
        r.display().to_string().replace('\\', "/")
    );
    let out = redact(&r, &text);
    assert!(!out.contains("Clade-Launcher"), "{out}");
    assert!(out.contains("<clade>"));
    assert!(!out.contains("Alice") && !out.contains("bob"), "{out}");
    assert!(out.contains("C:\\Users\\<user>\\AppData") && out.contains("/Users/<user>/x"));
}

#[test]
fn only_allowlisted_entries_and_no_secret_files_reach_the_zip() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    // Things that must never be exported.
    fs::write(r.vault_file(), b"VAULT-BYTES-SECRET").unwrap();
    fs::write(
        r.accounts_file(),
        br#"{"accounts":[{"username":"PrivateName"}]}"#,
    )
    .unwrap();
    let world = r
        .instances_dir()
        .join("p")
        .join("game")
        .join("saves")
        .join("w");
    fs::create_dir_all(&world).unwrap();
    fs::write(world.join("level.dat"), b"WORLD-SAVE-DATA").unwrap();
    fs::create_dir_all(r.instances_dir().join("p").join("game").join("mods")).unwrap();
    fs::write(
        r.instances_dir()
            .join("p")
            .join("game")
            .join("mods")
            .join("m.jar"),
        b"MOD",
    )
    .unwrap();
    // Logs that do get exported, with secrets inside.
    fs::write(
        r.logs_dir().join("clade.log"),
        format!(
            "start in {}\nrefresh_token=M.R3_SECRETLOG access_token: SECRETLOG2\nowner a@b.example\n",
            r.path().display()
        ),
    )
    .unwrap();
    let game_logs = r.instances_dir().join("p").join("game").join("logs");
    fs::create_dir_all(&game_logs).unwrap();
    fs::write(
        game_logs.join("latest.log"),
        "[main] Setting user: Steve\nsession=SECRETSESSION\n",
    )
    .unwrap();

    let settings = LauncherSettings {
        last_instance: Some(crate::instances::InstanceId::new("p").unwrap()),
        ..LauncherSettings::default()
    };
    let (exported, path) = export(&r, &settings, &facts(1_700_000_000)).unwrap();
    assert_eq!(exported.name, "Clade-Diagnostics-20231114-221320.zip");
    assert!(
        path.starts_with(r.logs_dir()),
        "the ZIP stays inside the Clade folder"
    );

    let files = read_zip(&path);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    for n in &names {
        assert!(ENTRIES.contains(n), "{n} is not allowlisted");
    }
    for expected in ENTRIES {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    let all: String = files
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for secret in [
        "VAULT-BYTES-SECRET",
        "PrivateName",
        "WORLD-SAVE-DATA",
        "SECRETLOG",
        "SECRETLOG2",
        "SECRETSESSION",
        "a@b.example",
    ] {
        assert!(!all.contains(secret), "{secret} leaked into the export");
    }
    assert!(
        !all.contains(&r.path().display().to_string()),
        "the folder path leaked"
    );
    assert!(all.contains("<clade>"));
    assert!(all.contains("Clade Launcher 0.1.0"));
    assert!(files
        .iter()
        .any(|(n, t)| n == "settings-redacted.json" && t.contains("\"language\"")));
}

#[test]
fn a_long_log_keeps_only_its_tail_from_a_line_start() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("big.log");
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef\n";
    let n = (LOG_TAIL_BYTES as usize / line.len()) + 50;
    fs::write(&p, line.repeat(n)).unwrap();
    let t = tail(&p).unwrap();
    assert!(t.len() as u64 <= LOG_TAIL_BYTES);
    assert!(
        t.starts_with("0123456789abcdef"),
        "starts at a line boundary"
    );
    assert!(tail(&d.path().join("missing.log")).is_none());
}

#[test]
fn only_the_newest_exports_are_kept() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let settings = LauncherSettings::default();
    for i in 0..(KEEP as i64 + 3) {
        export(&r, &settings, &facts(1_700_000_000 + i * 60)).unwrap();
    }
    let zips = fs::read_dir(r.logs_dir().join("diagnostics"))
        .unwrap()
        .count();
    assert_eq!(zips, KEEP);
}

#[test]
fn a_failure_is_reported_as_a_stable_code() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    // `logs/diagnostics` cannot be created: a file is in the way.
    fs::write(r.logs_dir().join("diagnostics"), b"in the way").unwrap();
    let err = export(&r, &LauncherSettings::default(), &facts(1_700_000_000)).unwrap_err();
    let coded: CodedError = err.into();
    assert_eq!(coded.code, "diagnostics_failed");
}

#[test]
fn a_profile_that_cannot_be_read_is_named_in_the_summary_and_left_untouched() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let broken = r.instances_dir().join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("instance.json"), b"{ not json").unwrap();
    let summary = profile_summary(&r);
    assert!(
        summary.contains("Profiles that could not be loaded: 1"),
        "{summary}"
    );
    assert!(summary.contains("- broken:"), "{summary}");
    assert_eq!(
        fs::read(broken.join("instance.json")).unwrap(),
        b"{ not json"
    );
}
