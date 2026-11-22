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
    let diag_dir = r.logs_dir().join("diagnostics");
    fs::create_dir_all(&diag_dir).unwrap();
    // Unrelated files in logs/diagnostics/ must be left untouched by prune().
    fs::write(diag_dir.join("notes.txt"), b"keep me").unwrap();
    fs::write(diag_dir.join("other.zip"), b"unrelated zip").unwrap();

    let settings = LauncherSettings::default();
    for i in 0..(KEEP as i64 + 3) {
        export(&r, &settings, &facts(1_700_000_000 + i * 60)).unwrap();
    }

    let mut remaining: Vec<String> = fs::read_dir(&diag_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    remaining.sort();

    let mut expected: Vec<String> = (3..(KEEP as i64 + 3))
        .map(|i| {
            format!(
                "Clade-Diagnostics-{}.zip",
                timestamp(1_700_000_000 + i * 60)
            )
        })
        .collect();
    expected.push("notes.txt".to_owned());
    expected.push("other.zip".to_owned());
    expected.sort();

    assert_eq!(remaining, expected);
    assert_eq!(fs::read(diag_dir.join("notes.txt")).unwrap(), b"keep me");
    assert_eq!(
        fs::read(diag_dir.join("other.zip")).unwrap(),
        b"unrelated zip"
    );
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

// ---- credentials that are not behind a "key=" (F1: a token echoed inside an error message)

/// A development fixture token and invented, obviously fake credential shapes. None is real.
const FAKE_JWT: &str =
    "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJmYWtlLXVzZXIifQ.ZmFrZXNpZ25hdHVyZS1ub3QtcmVhbA";
const FAKE_DOTTED: &str = "abcdEFGH12345678.ijklMNOP12345678.qrstUVWX12345678";
const FAKE_MS_ACCESS: &str = "EwBYA8l6BAAUfakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefakefake1234";
const FAKE_FIXTURE: &str = "DEV-FIXTURE-TOKEN-A-not-a-credential";

#[test]
fn a_jwt_like_token_is_redacted_wherever_it_stands() {
    let d = tempfile::tempdir().unwrap();
    for line in [
        format!("Failed to parse into SignedJWT: {FAKE_JWT}"),
        format!("x=hash123;{FAKE_JWT}"),
        format!("(token {FAKE_JWT}) rejected"),
        format!("saw {FAKE_DOTTED} in a header"),
        format!("Authorization: Bearer {FAKE_JWT}"),
        format!("XBL3.0 x=1234567890;{FAKE_JWT}"),
        format!("ticket {FAKE_MS_ACCESS} end"),
    ] {
        let out = redact(d.path(), &line);
        for secret in [
            FAKE_JWT,
            FAKE_DOTTED,
            FAKE_MS_ACCESS,
            "ZmFrZXNpZ25hdHVyZS1ub3QtcmVhbA",
        ] {
            assert!(
                !out.contains(secret),
                "{secret} survived in {out:?} (from {line:?})"
            );
        }
        assert!(out.contains("<redacted>"), "{out}");
    }
}

#[test]
fn bearer_access_and_refresh_token_forms_are_redacted() {
    let d = tempfile::tempdir().unwrap();
    for (line, secret) in [
        (
            "Authorization: Bearer opaque-bearer-VALUE-1",
            "opaque-bearer-VALUE-1",
        ),
        (
            "bearer opaque-bearer-VALUE-2 was used",
            "opaque-bearer-VALUE-2",
        ),
        (
            "access_token=opaque-access-VALUE-3&state=x",
            "opaque-access-VALUE-3",
        ),
        (
            r#"{"access_token":"opaque-access-VALUE-4"}"#,
            "opaque-access-VALUE-4",
        ),
        (
            r#"{"refresh_token":"M.C500_BAY.0.U.opaque-refresh-VALUE-5"}"#,
            "opaque-refresh-VALUE-5",
        ),
        (
            "refresh_token: opaque-refresh-VALUE-6",
            "opaque-refresh-VALUE-6",
        ),
        (
            "minecraft token=opaque-token-VALUE-7",
            "opaque-token-VALUE-7",
        ),
        ("RpsTicket d=opaque-rps-VALUE-8", "opaque-rps-VALUE-8"),
        (
            "client_secret=opaque-client-VALUE-9",
            "opaque-client-VALUE-9",
        ),
        (
            "--accessToken opaque-launch-VALUE-10 --uuid 1",
            "opaque-launch-VALUE-10",
        ),
        (
            "code=opaque-oauth-CODE-0123456789abcdef",
            "opaque-oauth-CODE-0123456789abcdef",
        ),
        (
            r#"passphrase="correct horse battery" next=1"#,
            "correct horse battery",
        ),
        ("secret_key: 'alpha beta gamma' done=1", "alpha beta gamma"),
    ] {
        let out = redact(d.path(), line);
        assert!(!out.contains(secret), "{secret} survived in {out:?}");
    }
}

#[test]
fn the_development_fixture_token_is_redacted_even_inside_an_error_message() {
    let d = tempfile::tempdir().unwrap();
    let line = format!(
        "[Realms Notification Availability checker #1/INFO]: Could not authorize you against Realms server: java.lang.RuntimeException: Failed to parse into SignedJWT: {FAKE_FIXTURE}"
    );
    let out = redact(d.path(), &line);
    assert!(
        !out.contains("DEV-FIXTURE") && !out.contains("not-a-credential"),
        "{out}"
    );
    assert!(
        out.contains("Could not authorize you against Realms server"),
        "the rest stays: {out}"
    );
}

#[test]
fn harmless_values_stay_readable() {
    let d = tempfile::tempdir().unwrap();
    for line in [
        "launch did not start code=vault_locked",
        "launch did not start code=needs_repair",
        "game process ended code=Some(-1) kind=Crashed",
        "game process ended code=Some(0) kind=Normal",
        "profile preparation did not complete code=cancelled",
        "exit code=1 after 12 s",
        "profile 3f2a9c1e-5b7d-4c88-9a10-0123456789ab uses net.minecraft.client.main.Main 1.21.4",
        r"loading C:\Clade\libraries\org\lwjgl\lwjgl\3.3.3\lwjgl-3.3.3.jar and fabric-loader-0.19.5-1.21.4",
        "Java 21.0.7 started in 3 s with -Xmx7936M",
        "sha1 of the asset is 0123456789abcdef0123456789abcdef01234567",
    ] {
        assert_eq!(redact(d.path(), line), line, "over-redacted: {line:?}");
    }
}

#[test]
fn a_secret_that_only_looks_like_a_result_code_is_still_redacted() {
    let d = tempfile::tempdir().unwrap();
    // an OAuth authorization code is a long mixed token, not a lower-case word or a number
    for secret in [
        "M.C500_BAY.2.U.fakeCodeValue12345",
        "0.AXEAfakefakefake12345",
        "AbC123xyz789AbC123xyz789",
    ] {
        let out = redact(d.path(), &format!("redirect with code={secret} done"));
        assert!(!out.contains(secret), "{secret} survived in {out:?}");
    }
}

#[test]
fn the_exported_zip_contains_no_original_secret_material_but_keeps_useful_lines() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    fs::write(
        r.logs_dir().join("clade.log"),
        format!(
            "launch did not start code=vault_locked\ngame process ended code=Some(-1) kind=Crashed\nlaunch failed with {FAKE_JWT}\n"
        ),
    )
    .unwrap();
    let game_logs = r.instances_dir().join("p").join("game").join("logs");
    fs::create_dir_all(&game_logs).unwrap();
    fs::write(
        game_logs.join("latest.log"),
        format!(
            "[Client thread/INFO]: Setting user: Steve\n[Realms/INFO]: Failed to parse into SignedJWT: {FAKE_FIXTURE}\nAuthorization: Bearer {FAKE_JWT}\n[Render thread/INFO]: Created: 1024x512 textures-atlas\n"
        ),
    )
    .unwrap();
    let settings = LauncherSettings {
        last_instance: Some(crate::instances::InstanceId::new("p").unwrap()),
        ..LauncherSettings::default()
    };
    let (_, path) = export(&r, &settings, &facts(1_700_000_000)).unwrap();

    // look at the actual ZIP bytes' contents, entry by entry
    let files = read_zip(&path);
    let all: String = files
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for secret in [
        FAKE_JWT,
        FAKE_FIXTURE,
        "DEV-FIXTURE",
        "ZmFrZXNpZ25hdHVyZS1ub3QtcmVhbA",
        "eyJhbGci",
    ] {
        assert!(!all.contains(secret), "{secret} is in the diagnostics ZIP");
    }
    // the diagnostic value is still there
    assert!(all.contains("code=vault_locked"), "{all}");
    assert!(all.contains("code=Some(-1)"), "{all}");
    assert!(
        all.contains("Setting user: Steve") && all.contains("Created: 1024x512 textures-atlas")
    );
    assert!(
        all.contains("Failed to parse into SignedJWT:"),
        "the message stays, the token goes"
    );
}

#[test]
fn key_and_secret_style_names_are_recognised_by_shape() {
    let d = tempfile::tempdir().unwrap();
    // The literal spelling of one of these is banned from the source tree by product_rules, so the
    // names are assembled here; the redactor must classify them by shape, not by a listed spelling.
    let names = [
        ["api", "_key"].concat(),
        ["api", "-key"].concat(),
        ["Api", "Key"].concat(),
        ["API", "_", "KEY"].concat(),
        ["x-api", "-key"].concat(),
        "secret_key".to_owned(),
        "private-key".to_owned(),
        "AWS_SECRET_ACCESS_KEY".to_owned(),
        "stripeSecret".to_owned(),
        "auth_token".to_owned(),
        "sessionToken".to_owned(),
        "db_password".to_owned(),
    ];
    for name in &names {
        for line in [
            format!("config {name}=opaque-VALUE-12345 next=1"),
            format!("header {name}: opaque-VALUE-12345"),
            format!(r#"{{"{name}":"opaque-VALUE-12345","a":1}}"#),
        ] {
            let out = redact(d.path(), &line);
            assert!(
                !out.contains("opaque-VALUE-12345"),
                "{name}: survived in {out:?}"
            );
        }
    }
    // names that merely contain such words are not secrets
    for line in [
        "key=minecraft.key.jump and keybinding=ctrl",
        "monkey=1 turkey=2 hotkey=F5",
        "token_count=5 secretary=Ada primary_key=7",
    ] {
        assert_eq!(redact(d.path(), line), line, "over-redacted: {line:?}");
    }
}

#[test]
fn known_minecraft_names_and_uuids_are_redacted_from_exported_logs_only() {
    use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let uuid = "00000000-0000-4000-8000-0000000000aa";
    let mut accounts = AccountsFile::default();
    accounts.accounts.push(AccountMeta {
        id: "acc1".into(),
        username: "FixtureAlex".into(),
        minecraft_uuid: uuid.into(),
        added_at: 0,
        last_used_at: None,
        status: AccountStatus::Ready,
    });
    accounts.save(&r.accounts_file()).unwrap();

    let line = format!(
        "[main] Setting user: FixtureAlex\n[main] profile {uuid} / {} owner fixturealex!\n\
         [main] FixtureAlexander and Alex_Fixture and Steve are other words\n",
        uuid.replace('-', "")
    );
    fs::write(r.logs_dir().join("clade.log"), &line).unwrap();
    let game_logs = r.instances_dir().join("p").join("game").join("logs");
    fs::create_dir_all(&game_logs).unwrap();
    fs::write(game_logs.join("latest.log"), &line).unwrap();
    let settings = LauncherSettings {
        last_instance: Some(crate::instances::InstanceId::new("p").unwrap()),
        ..LauncherSettings::default()
    };
    let (_, path) = export(&r, &settings, &facts(1_700_000_000)).unwrap();
    let files = read_zip(&path);
    let all: String = files
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for known in [
        "FixtureAlex\n",
        uuid,
        &uuid.replace('-', ""),
        "fixturealex!",
    ] {
        assert!(!all.contains(known), "{known} leaked into the export");
    }
    assert!(!all.to_lowercase().contains("setting user: fixturealex\n"));
    // Context stays useful, placeholders mark the removals, and other words are untouched.
    let mc = &files
        .iter()
        .find(|(n, _)| n == "logs/minecraft-latest.log")
        .unwrap()
        .1;
    assert!(mc.contains("Setting user: <mc-name>"));
    assert!(mc.contains("profile <mc-uuid> / <mc-uuid> owner <mc-name>!"));
    assert!(mc.contains("FixtureAlexander and Alex_Fixture and Steve are other words"));
    // The local game log itself is not rewritten.
    assert_eq!(
        fs::read_to_string(game_logs.join("latest.log")).unwrap(),
        line
    );
}

#[test]
fn identity_redaction_needs_known_values_and_handles_odd_input() {
    assert_eq!(redact_identities("user Steve here", &[]), "user Steve here");
    assert_eq!(
        redact_identities("user Steve here", &["Steve".to_owned()]),
        "user <mc-name> here"
    );
    assert_eq!(
        redact_identities("Steve's world, Steve2, xSteve", &["Steve".to_owned()]),
        "<mc-name>'s world, Steve2, xSteve"
    );
    assert_eq!(redact_identities("a", &[String::new()]), "a");
    // A missing accounts.json redacts nothing and does not fail the export.
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    assert!(known_identities(&r).is_empty());
}

#[test]
fn archived_vaults_are_never_candidates_for_the_export() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let retired = r
        .data_dir()
        .join("retired-vaults")
        .join("1700000000-0123456789ab");
    fs::create_dir_all(&retired).unwrap();
    fs::write(retired.join("vault.bin"), b"RETIRED-VAULT-SECRET-BYTES").unwrap();
    fs::write(
        retired.join("vault.bin.bak"),
        b"RETIRED-BACKUP-SECRET-BYTES",
    )
    .unwrap();
    let (_, path) = export(&r, &LauncherSettings::default(), &facts(1_700_000_000)).unwrap();
    let entries = read_zip(&path);
    for (name, content) in &entries {
        assert!(ENTRIES.contains(&name.as_str()), "{name}");
        assert!(
            !name.contains("retired") && !name.contains("vault"),
            "{name}"
        );
        assert!(!content.contains("RETIRED-"), "{name}");
        assert!(!content.contains("retired-vaults"), "{name}");
    }
}

#[test]
fn json_escaped_and_url_user_folders_are_redacted() {
    let d = tempfile::tempdir().unwrap();
    let cases = [
        (
            r"C:\Users\Alice\AppData\Roaming",
            r"C:\Users\<user>\AppData\Roaming",
        ),
        (
            r#"{"path":"C:\\Users\\Alice\\AppData\\Local"}"#,
            r#"{"path":"C:\\Users\\<user>\\AppData\\Local"}"#,
        ),
        (
            r"escaped C:\\\\Users\\\\Alice\\\\AppData\\\\Local",
            r"escaped C:\\\\Users\\\\<user>\\\\AppData\\\\Local",
        ),
        ("/Users/bob/x", "/Users/<user>/x"),
        (
            "file:///C:/Users/Alice%20Smith/x",
            "file:///C:/Users/<user>/x",
        ),
    ];
    for (input, expected) in cases {
        let out = redact(d.path(), input);
        assert_eq!(out, expected, "input={input}");
        assert!(!out.contains("Alice") && !out.contains("bob"), "{out}");
    }
}

#[test]
fn json_escaped_and_url_forms_of_root_with_spaces_become_placeholders_and_keep_newlines() {
    let r = Path::new(r"D:\Games\Clade Launcher");
    let text = concat!(
        "plain D:\\Games\\Clade Launcher\\logs\n",
        "json D:\\\\Games\\\\Clade Launcher\\\\logs\n",
        "quad D:\\\\\\\\Games\\\\\\\\Clade Launcher\\\\\\\\logs\n",
        "slash D:/Games/Clade Launcher/logs\n",
        "pct D:/Games/Clade%20Launcher/logs\n",
        "url file:///D:/Games/Clade%20Launcher/logs/clade.log\n",
    );
    let out = redact(r, text);
    assert!(out.ends_with('\n'), "trailing newline must be preserved");
    assert!(
        !out.contains("Clade Launcher") && !out.contains("Clade%20Launcher"),
        "{out}"
    );
    assert!(out.contains("plain <clade>\\logs\n"), "{out}");
    assert!(out.contains("json <clade>\\\\logs\n"), "{out}");
    assert!(out.contains("quad <clade>\\\\\\\\logs\n"), "{out}");
    assert!(out.contains("slash <clade>/logs\n"), "{out}");
    assert!(out.contains("pct <clade>/logs\n"), "{out}");
    assert!(
        out.contains("url file:///<clade>/logs/clade.log\n"),
        "{out}"
    );

    // Non-drive relative paths with percent-encoded spaces are also redacted.
    let rel = Path::new("portable/Clade Launcher");
    let rel_out = redact(rel, "see portable/Clade%20Launcher/logs");
    assert_eq!(rel_out, "see <clade>/logs");
}

#[test]
fn non_ascii_characters_whose_lowercase_changes_byte_length_never_bypass_name_redaction() {
    let known = vec!["FixtureAlex".to_owned()];
    // 'İ' (U+0130, 2 UTF-8 bytes) lowercases in Unicode to "i\u{307}" (3 UTF-8 bytes).
    let log = "[Chat] İstanbul server — player FixtureAlex (fixturealex) joined 🚀 Привет!\n";
    let out = redact_identities(log, &known);
    assert!(
        !out.to_ascii_lowercase().contains("fixturealex"),
        "username bypassed redaction when 'İ' was present: {out:?}"
    );
    assert_eq!(
        out,
        "[Chat] İstanbul server — player <mc-name> (<mc-name>) joined 🚀 Привет!\n"
    );
}

#[test]
fn msa_tokens_with_base64_and_rps_punctuation_jwe_and_xbl3_opaque_tokens_are_redacted() {
    let d = tempfile::tempdir().unwrap();
    let msa_ew = concat!(
        "EwBYA8l6BAAUfake+part/with=pad==!bang*star$dollar",
        "0123456789abcdef0123456789abcdefTAILSECRET",
    );
    let msa_m = "M.C507_BAY.0.U.-CiFake+Refresh/Val=1!bang*star$TAILSECRET";
    let msa_0a = "0.AXEAfakeRefresh+Value/With=Pad!bang*star$TAILSECRET";
    let jwe_dir = concat!(
        "ewogImFsZyI6ICJkaXIiIH0..iv_part_01234567.",
        "ciphertext_0123456789.authtag_0123456789",
    );
    let jwe_kw = concat!(
        "ewogImFsZyI6ICJSU0EtT0FFUCIgfQ.encryptedKeyPart1234.",
        "iv_part_01234567.ciphertext_0123456789.authtag_0123456789",
    );
    let xbl_opaque = "OPAQUE_XSTS_SECRET_AFTER_SEMICOLON";

    for line in [
        format!("rps ticket d={msa_ew} failed"),
        format!("bare refresh ({msa_m}) rejected"),
        format!("bare oauth {msa_0a}&state=ok"),
        format!("decrypt error on {jwe_dir} in auth"),
        format!("decrypt error on {jwe_kw} in auth"),
        format!("header XBL3.0 x=1234567890;{xbl_opaque} end"),
        format!(r#"{{"identityToken":"XBL3.0 x=1234567890;{xbl_opaque}"}}"#),
    ] {
        let out = redact(d.path(), &line);
        for leaked in [
            "TAILSECRET",
            "bang",
            "star",
            "dollar",
            "ciphertext_0123456789",
            "authtag_0123456789",
            "encryptedKeyPart1234",
            xbl_opaque,
        ] {
            assert!(
                !out.contains(leaked),
                "{leaked} survived in {out:?} (from {line:?})"
            );
        }
        assert!(out.contains("<redacted>"), "{out}");
    }
}

#[test]
fn profile_summary_redacts_known_identities_and_personal_paths() {
    use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let uuid = "00000000-0000-4000-8000-0000000000bb";
    let mut accounts = AccountsFile::default();
    accounts.accounts.push(AccountMeta {
        id: "acc1".into(),
        username: "FixtureAlex".into(),
        minecraft_uuid: uuid.into(),
        added_at: 0,
        last_used_at: None,
        status: AccountStatus::Ready,
    });
    accounts.save(&r.accounts_file()).unwrap();

    // A profile whose id/folder names the player, and a broken profile whose manifest error
    // echoes a personal user path and the Clade root path in `listing.problems`.
    let broken_name = format!("FixtureAlex-{uuid}");
    let broken = r.instances_dir().join(&broken_name);
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("instance.json"), b"{ not json").unwrap();

    let broken_path = r.instances_dir().join("broken-path");
    fs::create_dir_all(&broken_path).unwrap();
    let root_str = r.path().display().to_string();
    let root_escaped = root_str.replace('\\', "\\\\");
    // `InstanceId::deserialize` echoes the invalid `id` string in `listing.problems`.
    let payload = serde_json::json!({
        "id": format!("C:\\Users\\Alice\\x and {root_str}")
    });
    fs::write(broken_path.join("instance.json"), payload.to_string()).unwrap();

    let raw_summary = profile_summary(&r);
    assert!(
        raw_summary.contains("Alice") && raw_summary.contains(&root_escaped),
        "expected unredacted path/user in raw problem summary: {raw_summary}"
    );

    let collected = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    let summary = &collected
        .iter()
        .find(|(n, _)| *n == "profile-summary.txt")
        .unwrap()
        .1;
    assert!(
        !summary.to_ascii_lowercase().contains("fixturealex"),
        "known username leaked in profile-summary.txt: {summary}"
    );
    assert!(
        !summary.contains(uuid),
        "known UUID leaked in profile-summary.txt: {summary}"
    );
    assert!(
        summary.contains("<mc-name>-<mc-uuid>"),
        "expected redacted placeholders in {summary}"
    );
    assert!(
        !summary.contains("Alice"),
        "personal user folder leaked in profile-summary.txt: {summary}"
    );
    assert!(
        !summary.contains(&root_str) && !summary.contains(&root_escaped),
        "portable root path leaked in profile-summary.txt: {summary}"
    );
    assert!(
        summary.contains("<user>") && summary.contains("<clade>"),
        "expected <user> and <clade> placeholders in {summary}"
    );
}

#[test]
fn clade_old_log_tail_is_prepended_only_when_current_log_is_below_budget() {
    use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let mut accounts = AccountsFile::default();
    accounts.accounts.push(AccountMeta {
        id: "acc1".into(),
        username: "FixtureAlex".into(),
        minecraft_uuid: "00000000-0000-4000-8000-0000000000cc".into(),
        added_at: 0,
        last_used_at: None,
        status: AccountStatus::Ready,
    });
    accounts.save(&r.accounts_file()).unwrap();

    // 1. Small clade.log -> clade.old.log is prepended (with newline boundary) and redacted.
    fs::write(
        r.logs_dir().join("clade.old.log"),
        "old-entry user FixtureAlex token=SECRET_OLD_TOKEN",
    )
    .unwrap();
    fs::write(
        r.logs_dir().join("clade.log"),
        "current-entry user FixtureAlex\n",
    )
    .unwrap();
    let collected = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    let log = &collected
        .iter()
        .find(|(n, _)| *n == "logs/clade.log")
        .unwrap()
        .1;
    assert!(
        log.contains("old-entry user <mc-name> token=<redacted>\ncurrent-entry user <mc-name>\n"),
        "unexpected combined log: {log:?}"
    );
    assert!(!log.contains("SECRET_OLD_TOKEN") && !log.contains("FixtureAlex"));

    // 2. clade.log >= LOG_TAIL_BYTES -> clade.old.log is NOT prepended.
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abc\n";
    let repeats = (LOG_TAIL_BYTES as usize / line.len()) + 32;
    fs::write(r.logs_dir().join("clade.log"), line.repeat(repeats)).unwrap();
    let collected_full = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    let log_full = &collected_full
        .iter()
        .find(|(n, _)| *n == "logs/clade.log")
        .unwrap()
        .1;
    assert!(
        !log_full.contains("old-entry"),
        "clade.old.log must not be prepended when clade.log >= LOG_TAIL_BYTES"
    );

    // 3. clade.log slightly below LOG_TAIL_BYTES -> clade.old.log is tailed to rem = LOG_TAIL_BYTES - cur_len on a line boundary.
    let cur_line = "current-budget-line-0123456789abcdef0123456789abcdef0123456789a\n";
    let cur_repeats = (LOG_TAIL_BYTES as usize - 1024) / cur_line.len();
    fs::write(r.logs_dir().join("clade.log"), cur_line.repeat(cur_repeats)).unwrap();
    let old_line = "old-line-0123456789abcdef0123456789abcdef0123456789abcdef012345\n";
    fs::write(
        r.logs_dir().join("clade.old.log"),
        format!("OLD-HEAD-DROPPED\n{}old-tail-last\n", old_line.repeat(64)),
    )
    .unwrap();
    let collected_trimmed = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    let log_trimmed = &collected_trimmed
        .iter()
        .find(|(n, _)| *n == "logs/clade.log")
        .unwrap()
        .1;
    assert!(log_trimmed.len() as u64 <= LOG_TAIL_BYTES);
    assert!(
        !log_trimmed.contains("OLD-HEAD-DROPPED"),
        "older head of clade.old.log should be trimmed away"
    );
    assert!(
        log_trimmed.starts_with("old-line-"),
        "trimmed clade.old.log tail must begin on a line boundary: {:?}",
        &log_trimmed[..log_trimmed.len().min(80)]
    );
    assert!(
        log_trimmed.contains("old-tail-last\ncurrent-budget-line-"),
        "expected trimmed old tail followed by current log"
    );

    // 4. Only clade.old.log exists (clade.log absent) -> clade.old.log tail is still collected.
    fs::remove_file(r.logs_dir().join("clade.log")).unwrap();
    fs::write(
        r.logs_dir().join("clade.old.log"),
        "only-old-entry user FixtureAlex token=SECRET_OLD_ONLY\n",
    )
    .unwrap();
    let collected_old_only = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    let log_old_only = &collected_old_only
        .iter()
        .find(|(n, _)| *n == "logs/clade.log")
        .unwrap()
        .1;
    assert_eq!(
        log_old_only,
        "only-old-entry user <mc-name> token=<redacted>\n"
    );
}

#[test]
fn redact_emails_handles_multibyte_utf8_delimiters_without_panicking() {
    let d = tempfile::tempdir().unwrap();
    for line in [
        "§7player@server.net connected",
        "Настройки аккаунта «user@example.com» обновлены",
        "Ошибка —admin@test.org после запуска",
        "Alert 🔥user@example.com here",
        "user+tag@sub.example.com §aok",
    ] {
        let out = redact(d.path(), line);
        assert!(
            out.contains("<email>"),
            "expected email redacted in {out:?}"
        );
        assert!(!out.contains('@'), "raw email leaked in {out:?}");
    }
}

#[test]
fn settings_redacted_json_redacts_last_instance_identity() {
    use crate::accounts::{AccountMeta, AccountStatus, AccountsFile};
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let uuid = "00000000-0000-4000-8000-0000000000ee";
    let mut accounts = AccountsFile::default();
    accounts.accounts.push(AccountMeta {
        id: "acc1".into(),
        username: "FixtureAlex".into(),
        minecraft_uuid: uuid.into(),
        added_at: 0,
        last_used_at: None,
        status: AccountStatus::Ready,
    });
    accounts.save(&r.accounts_file()).unwrap();

    let id = crate::instances::InstanceId::new(&format!("fixturealex-{uuid}")).unwrap();
    let settings = LauncherSettings {
        last_instance: Some(id),
        ..LauncherSettings::default()
    };
    let collected = collect(&r, &settings, &facts(1_700_000_000));
    let settings_json = &collected
        .iter()
        .find(|(n, _)| *n == "settings-redacted.json")
        .unwrap()
        .1;
    assert!(
        !settings_json.to_ascii_lowercase().contains("fixturealex"),
        "known username leaked in settings-redacted.json: {settings_json}"
    );
    assert!(
        !settings_json.contains(uuid),
        "known UUID leaked in settings-redacted.json: {settings_json}"
    );
    assert!(
        settings_json.contains("<mc-name>-<mc-uuid>"),
        "expected redacted identity placeholder in settings-redacted.json: {settings_json}"
    );
}

#[test]
fn minecraft_latest_log_falls_back_to_most_recent_instance_across_profiles() {
    use crate::instances::NewInstance;
    use std::time::{Duration, SystemTime};

    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let store = InstanceStore::new(&r);

    let alpha = store
        .create(NewInstance::vanilla("Alpha", "1.21.4").unwrap(), 100)
        .unwrap();
    let beta = store
        .create(NewInstance::vanilla("Beta", "1.21.4").unwrap(), 200)
        .unwrap();
    let gamma = store
        .create(NewInstance::vanilla("Gamma", "1.21.4").unwrap(), 300)
        .unwrap();

    let alpha_logs = store.dirs(&alpha.id).game_dir().join("logs");
    let beta_logs = store.dirs(&beta.id).game_dir().join("logs");
    fs::create_dir_all(&alpha_logs).unwrap();
    fs::create_dir_all(&beta_logs).unwrap();

    let alpha_log = alpha_logs.join("latest.log");
    let beta_log = beta_logs.join("latest.log");
    fs::write(&alpha_log, "alpha older log\n").unwrap();
    fs::write(&beta_log, "beta newest log\n").unwrap();

    let t_old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let t_new = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_600);
    fs::OpenOptions::new()
        .write(true)
        .open(&alpha_log)
        .unwrap()
        .set_modified(t_old)
        .unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&beta_log)
        .unwrap()
        .set_modified(t_new)
        .unwrap();

    // 1. last_instance is None -> selects the most recently modified instance log (Beta).
    let none_settings = LauncherSettings::default();
    let collected = collect(&r, &none_settings, &facts(1_700_000_000));
    let mc_log = &collected
        .iter()
        .find(|(n, _)| *n == "logs/minecraft-latest.log")
        .unwrap()
        .1;
    assert_eq!(mc_log, "beta newest log\n");

    // 2. last_instance points to an instance without latest.log -> falls back to Beta.
    let missing_settings = LauncherSettings {
        last_instance: Some(gamma.id),
        ..LauncherSettings::default()
    };
    let collected_missing = collect(&r, &missing_settings, &facts(1_700_000_000));
    let mc_log_missing = &collected_missing
        .iter()
        .find(|(n, _)| *n == "logs/minecraft-latest.log")
        .unwrap()
        .1;
    assert_eq!(mc_log_missing, "beta newest log\n");
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let ok = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(ok, "could not create directory junction for test");
}

#[cfg(windows)]
#[test]
fn diagnostics_export_skips_junction_redirected_game_logs() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let outside = d.path().join("outside-secrets");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("latest.log"), "SENSITIVE_OUTSIDE_DATA").unwrap();

    let game_dir = r.instances_dir().join("p").join("game");
    fs::create_dir_all(&game_dir).unwrap();
    let linked_logs = game_dir.join("logs");
    create_junction(&linked_logs, &outside);

    let settings = LauncherSettings {
        last_instance: Some(crate::instances::InstanceId::new("p").unwrap()),
        ..LauncherSettings::default()
    };
    let collected = collect(&r, &settings, &facts(1_700_000_000));
    assert!(
        !collected
            .iter()
            .any(|(name, _)| *name == "logs/minecraft-latest.log"),
        "junction-redirected game log must not be included"
    );
    let _ = fs::remove_dir(&linked_logs);
}

#[cfg(windows)]
#[test]
fn minecraft_latest_log_fallback_skips_junction_redirected_last_instance() {
    use crate::instances::NewInstance;
    use std::time::{Duration, SystemTime};

    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let outside = d.path().join("outside-gamma");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("latest.log"), "OUTSIDE_GAMMA_SECRET\n").unwrap();

    let store = InstanceStore::new(&r);
    let alpha = store
        .create(NewInstance::vanilla("Alpha", "1.21.4").unwrap(), 100)
        .unwrap();
    let beta = store
        .create(NewInstance::vanilla("Beta", "1.21.4").unwrap(), 200)
        .unwrap();
    let gamma = store
        .create(NewInstance::vanilla("Gamma", "1.21.4").unwrap(), 300)
        .unwrap();

    let alpha_logs = store.dirs(&alpha.id).game_dir().join("logs");
    let beta_logs = store.dirs(&beta.id).game_dir().join("logs");
    fs::create_dir_all(&alpha_logs).unwrap();
    fs::create_dir_all(&beta_logs).unwrap();
    let alpha_log = alpha_logs.join("latest.log");
    let beta_log = beta_logs.join("latest.log");
    fs::write(&alpha_log, "alpha older log\n").unwrap();
    fs::write(&beta_log, "beta newest log\n").unwrap();

    fs::OpenOptions::new()
        .write(true)
        .open(&alpha_log)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&beta_log)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_600))
        .unwrap();

    let linked_gamma_logs = store.dirs(&gamma.id).game_dir().join("logs");
    create_junction(&linked_gamma_logs, &outside);

    let settings = LauncherSettings {
        last_instance: Some(gamma.id),
        ..LauncherSettings::default()
    };
    let collected = collect(&r, &settings, &facts(1_700_000_000));
    let mc_log = &collected
        .iter()
        .find(|(n, _)| *n == "logs/minecraft-latest.log")
        .unwrap()
        .1;
    assert_eq!(mc_log, "beta newest log\n");
    assert!(!mc_log.contains("OUTSIDE_GAMMA_SECRET"));
    let _ = fs::remove_dir(&linked_gamma_logs);
}

#[cfg(windows)]
#[test]
fn export_rejects_junction_at_logs_or_diagnostics_dir() {
    // 1. Junction at logs/diagnostics/
    let d1 = tempfile::tempdir().unwrap();
    let r1 = root(d1.path());
    let outside_diag = d1.path().join("outside-diag");
    fs::create_dir_all(&outside_diag).unwrap();
    let linked_diag = r1.logs_dir().join("diagnostics");
    create_junction(&linked_diag, &outside_diag);

    let err1 = export(&r1, &LauncherSettings::default(), &facts(1_700_000_000)).unwrap_err();
    let coded1: CodedError = err1.into();
    assert_eq!(coded1.code, "diagnostics_failed");
    assert_eq!(fs::read_dir(&outside_diag).unwrap().count(), 0);
    let _ = fs::remove_dir(&linked_diag);

    // 2. Junction at logs/
    let d2 = tempfile::tempdir().unwrap();
    let r2 = root(d2.path());
    let outside_logs = d2.path().join("outside-logs");
    fs::create_dir_all(&outside_logs).unwrap();
    fs::remove_dir(r2.logs_dir()).unwrap();
    create_junction(&r2.logs_dir(), &outside_logs);

    let err2 = export(&r2, &LauncherSettings::default(), &facts(1_700_000_000)).unwrap_err();
    let coded2: CodedError = err2.into();
    assert_eq!(coded2.code, "diagnostics_failed");
    assert_eq!(fs::read_dir(&outside_logs).unwrap().count(), 0);
    let _ = fs::remove_dir(r2.logs_dir());
}

#[cfg(windows)]
#[test]
fn clade_log_tail_skips_junction_at_clade_log_and_old_log() {
    let d = tempfile::tempdir().unwrap();
    let r = root(d.path());
    let outside = d.path().join("outside-clade-log");
    fs::create_dir_all(&outside).unwrap();

    let cur_path = r.logs_dir().join("clade.log");
    let old_path = r.logs_dir().join("clade.old.log");

    // 1. clade.log is a junction, clade.old.log is a regular file -> only clade.old.log is returned.
    create_junction(&cur_path, &outside);
    fs::write(&old_path, "from-old-log\n").unwrap();
    assert_eq!(clade_log_tail(&r).as_deref(), Some("from-old-log\n"));
    let _ = fs::remove_dir(&cur_path);

    // 2. clade.old.log is a junction, clade.log is a regular file -> only clade.log is returned.
    fs::remove_file(&old_path).unwrap();
    create_junction(&old_path, &outside);
    fs::write(&cur_path, "from-cur-log\n").unwrap();
    assert_eq!(clade_log_tail(&r).as_deref(), Some("from-cur-log\n"));

    // 3. Both clade.log and clade.old.log are junctions -> omitted completely.
    fs::remove_file(&cur_path).unwrap();
    create_junction(&cur_path, &outside);
    assert!(clade_log_tail(&r).is_none());
    let collected = collect(&r, &LauncherSettings::default(), &facts(1_700_000_000));
    assert!(!collected.iter().any(|(n, _)| *n == "logs/clade.log"));

    let _ = fs::remove_dir(&cur_path);
    let _ = fs::remove_dir(&old_path);
}
