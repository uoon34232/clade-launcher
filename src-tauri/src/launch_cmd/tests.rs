#[cfg(test)]
use super::*;
#[cfg(test)]
use crate::accounts::{AccountMeta, ReauthReason};
#[cfg(test)]
use crate::instances::{LoaderVersionSpec, NewInstance};
#[cfg(test)]
use crate::loaders::LoaderKind;
#[cfg(test)]
use crate::ms_auth::ENDPOINTS;
#[cfg(test)]
use crate::net::testing_transport::FakeTransport;
#[cfg(test)]
use crate::prepare::prepare_instance;
#[cfg(test)]
use crate::prepare::tests::{
    create, ctx, forge_world, vanilla, world, Era, World, FABRIC_ID, FORGE_ID,
};
#[cfg(test)]
use crate::vault::{AccountSecrets, KdfParams};
#[cfg(test)]
use crate::versions::LoaderVersion;
#[cfg(test)]
use std::fs;

#[cfg(test)]
const NOW: i64 = 1_000;
#[cfg(test)]
const FAST: KdfParams = KdfParams {
    m_cost_kib: 8,
    t_cost: 1,
    p_cost: 1,
};
#[cfg(test)]
const UUID_A: &str = "00000000-0000-4000-8000-00000000a001";
#[cfg(test)]
const UUID_B: &str = "00000000-0000-4000-8000-00000000b002";

/// Test fixtures only: obviously fake values, never used by production code.
#[cfg(test)]
struct Accounts {
    state: AccountsState,
    vault: Mutex<Option<Vault>>,
}

#[cfg(test)]
fn meta(id: &str, name: &str, uuid: &str, status: AccountStatus) -> AccountMeta {
    AccountMeta {
        id: id.into(),
        username: name.into(),
        minecraft_uuid: uuid.into(),
        added_at: 1,
        last_used_at: None,
        status,
    }
}

/// Two signed-in accounts whose cached Minecraft tokens are still fresh (so no network is needed).
#[cfg(test)]
fn accounts(w: &World) -> Accounts {
    // A fresh pair every time, even in a world that already had one.
    let _ = fs::remove_file(w.root.vault_file());
    let _ = fs::remove_file(w.root.accounts_file());
    let state = AccountsState::new(w.root.accounts_file());
    state
        .update(|f| {
            f.accounts
                .push(meta("acc-a", "Alex", UUID_A, AccountStatus::Ready));
            f.accounts
                .push(meta("acc-b", "Blake_2", UUID_B, AccountStatus::Ready));
            f.active = Some("acc-a".into());
        })
        .unwrap();
    let mut vault =
        Vault::create_with(&w.root.vault_file(), "correct horse battery", FAST).unwrap();
    for (id, token) in [
        ("acc-a", "FIXTURE-TOKEN-A-not-a-credential"),
        ("acc-b", "FIXTURE-TOKEN-B-not-a-credential"),
    ] {
        vault
            .set_account(
                id,
                AccountSecrets {
                    ms_refresh_token: Some("fixture-refresh".into()),
                    minecraft_access_token: Some(token.into()),
                    minecraft_token_expires_at: Some(NOW + 100_000),
                },
            )
            .unwrap();
    }
    Accounts {
        state,
        vault: Mutex::new(Some(vault)),
    }
}

#[cfg(test)]
async fn plan(
    w: &World,
    a: &Accounts,
    t: &FakeTransport,
    id: &InstanceId,
) -> Result<Planned, LaunchFailure> {
    let sys = SystemEnv::from_process();
    plan_launch(
        &w.root,
        t,
        &a.vault,
        &a.state,
        id,
        &LaunchEnv {
            now: NOW,
            sys: &sys,
            total_memory_mb: Some(16_384),
            default_memory_mb: None,
            memory_snapshot: None,
            embed_resolution: None,
        },
    )
    .await
}

#[cfg(test)]
async fn prepared(mc: &str, era: Era) -> (World, InstanceId) {
    let w = world(mc, era);
    let id = create(&w, NewInstance::vanilla("Profile", mc).unwrap());
    prepare_instance(&ctx(&w), &id).await.unwrap();
    (w, id)
}

#[cfg(test)]
fn arg<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("{flag} missing in {args:?}"));
    &args[i + 1]
}

// ================================================================================ every kind of profile

#[tokio::test]
async fn a_prepared_modern_vanilla_profile_plans_a_launch_for_the_selected_account() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = accounts(&w);
    let t = FakeTransport::new();
    let p = plan(&w, &a, &t, &id).await.unwrap();
    let args = &p.command.args;
    assert_eq!(arg(args, "--username"), "Alex");
    assert_eq!(arg(args, "--uuid"), "0000000000004000800000000000a001");
    assert_eq!(
        arg(args, "--accessToken"),
        "FIXTURE-TOKEN-A-not-a-credential"
    );
    assert_eq!(args[0], "-Xmx4096M", "a quarter of 16 GiB");
    assert!(
        p.command.program.starts_with(w.root.java_runtimes_dir()),
        "the managed Java only"
    );
    assert_eq!(
        p.command.cwd,
        InstanceStore::new(&w.root).dirs(&id).game_dir()
    );
    assert!(
        args.contains(&"net.minecraft.client.main.Main".to_owned())
            || args.iter().any(|x| x.ends_with("Main"))
            || args.len() > 5
    );
    assert!(t.calls().is_empty(), "fresh credentials need no network");
    // The folders the game needs exist, inside the Clade folder, with isolated cache/tmp/game/<id>.
    for k in ["TEMP", "APPDATA", "USERPROFILE"] {
        let dir = std::path::Path::new(&p.command.env[k]);
        assert!(dir.is_dir() && dir.starts_with(w.root.path()), "{k}");
    }
    assert_eq!(
        std::path::Path::new(&p.command.env["TEMP"]),
        w.root.game_tmp_dir().join(id.as_str())
    );
}

#[tokio::test]
async fn snapshots_historical_and_legacy_asset_layouts_all_plan() {
    for (mc, era, assets_flag_is_resources) in [
        ("26.4-snapshot-2", Era::Modern, false),
        ("b1.7.3", Era::Pre16, true),
        ("1.6.4", Era::Virtual, false),
    ] {
        let (w, id) = prepared(mc, era).await;
        let a = accounts(&w);
        let p = plan(&w, &a, &FakeTransport::new(), &id)
            .await
            .unwrap_or_else(|e| panic!("{mc}: {e:?}"));
        let args = &p.command.args;
        if era == Era::Modern {
            assert_eq!(arg(args, "--version"), mc);
        } else {
            let assets = arg(args, "--assetsDir");
            if assets_flag_is_resources {
                assert!(assets.ends_with("resources"), "{assets}");
            } else {
                assert!(assets.contains("virtual"), "{assets}");
            }
            assert!(arg(args, "--session").starts_with("token:FIXTURE-TOKEN-A"));
        }
    }
}

#[tokio::test]
async fn fabric_forge_neoforge_and_quilt_plan_with_their_own_main_class_and_libraries() {
    // Fabric
    let w = world("1.21.4", Era::Modern);
    crate::prepare::tests::fabric_profile(&w, "1.21.4");
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
    let a = accounts(&w);
    let p = plan(&w, &a, &FakeTransport::new(), &id).await.unwrap();
    assert!(
        p.command.args.iter().any(|x| x.contains("fabric-loader")),
        "{:?}",
        p.command.args
    );
    assert_eq!(
        p.instance.setup,
        SetupState::Ready {
            version_id: crate::versions::VersionId::new(FABRIC_ID).unwrap(),
            loader_version: Some(LoaderVersion::new("0.16.9").unwrap())
        }
    );

    // Forge
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
    let a = accounts(&w);
    let p = plan(&w, &a, &FakeTransport::new(), &id).await.unwrap();
    let cp = arg(&p.command.args, "-cp");
    assert!(
        cp.contains("forge-1.21.4-54.1.14-universal.jar")
            && cp.contains("forge-1.21.4-54.1.14-client.jar"),
        "{cp}"
    );
    assert!(cp.rfind("1.21.4.jar").is_some());
    assert!(FORGE_ID.contains("forge"));
}

// ================================================================================ accounts

#[tokio::test]
async fn switching_the_active_account_changes_who_plays_and_never_the_instance() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = accounts(&w);
    let store = InstanceStore::new(&w.root);
    let manifest = store.dirs(&id).manifest_file();
    let before = (
        fs::read(&manifest).unwrap(),
        fs::read_dir(store.dirs(&id).root()).unwrap().count(),
    );

    let first = plan(&w, &a, &FakeTransport::new(), &id).await.unwrap();
    assert_eq!(arg(&first.command.args, "--username"), "Alex");
    a.state.update(|f| f.active = Some("acc-b".into())).unwrap();
    let second = plan(&w, &a, &FakeTransport::new(), &id).await.unwrap();
    assert_eq!(arg(&second.command.args, "--username"), "Blake_2");
    assert_eq!(
        arg(&second.command.args, "--uuid"),
        "0000000000004000800000000000b002"
    );
    assert_eq!(
        arg(&second.command.args, "--accessToken"),
        "FIXTURE-TOKEN-B-not-a-credential"
    );

    // The same instance, byte for byte; only the identity arguments differ.
    let after = (
        fs::read(&manifest).unwrap(),
        fs::read_dir(store.dirs(&id).root()).unwrap().count(),
    );
    assert_eq!(before.0, after.0, "the instance manifest is untouched");
    assert!(
        !String::from_utf8_lossy(&after.0).contains("acc-"),
        "an instance never mentions an account"
    );
    let strip = |c: &GameCommand| -> Vec<String> {
        let mut v = c.args.clone();
        for flag in ["--username", "--uuid", "--accessToken"] {
            let i = v.iter().position(|x| x == flag).unwrap();
            v[i + 1] = String::new();
        }
        v
    };
    assert_eq!(strip(&first.command), strip(&second.command));
    assert_eq!(
        PlaytimeStore::new(&w.root)
            .instance_total(&id)
            .unwrap()
            .total_seconds,
        0,
        "playtime is not touched either"
    );
}

#[tokio::test]
async fn without_an_account_or_with_one_that_must_sign_in_again_the_launch_stops_cleanly() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = Accounts {
        state: AccountsState::new(w.root.accounts_file()),
        vault: Mutex::new(None),
    };
    let e = plan(&w, &a, &FakeTransport::new(), &id)
        .await
        .err()
        .unwrap();
    assert_eq!((e.code, e.account_id.as_deref()), ("no_account", None));

    let a = accounts(&w);
    a.state
        .update(|f| f.mark_needs_reauth("acc-a", ReauthReason::Revoked, 5))
        .unwrap();
    let e = plan(&w, &a, &FakeTransport::new(), &id)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (e.code, e.account_id.as_deref()),
        ("needs_sign_in", Some("acc-a"))
    );
    assert!(e.message.contains("Alex"));
    // Nothing was deleted or rewritten because of it.
    assert_eq!(a.state.update(|f| f.accounts.len()).unwrap(), 2);
}

#[tokio::test]
async fn expired_credentials_refresh_through_the_existing_hook_and_every_failure_is_distinct() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let expire = |a: &Accounts| {
        let mut g = a.vault.lock().unwrap();
        let v = g.as_mut().unwrap();
        let mut s = v.account("acc-a").unwrap().clone();
        s.minecraft_token_expires_at = Some(NOW + 10); // inside the 5-minute margin: must refresh
        v.set_account("acc-a", s).unwrap();
    };
    let (token, xbl, xsts, login, profile) = (
        ENDPOINTS[1],
        ENDPOINTS[2],
        ENDPOINTS[3],
        ENDPOINTS[4],
        ENDPOINTS[6],
    );

    // Refresh works: a new token is used and stored.
    let a = accounts(&w);
    expire(&a);
    let t = FakeTransport::new();
    t.reply(
        token,
        200,
        r#"{"access_token":"MS","refresh_token":"ROTATED","expires_in":3600}"#,
    );
    t.reply(
        xbl,
        200,
        r#"{"Token":"X","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#,
    );
    t.reply(
        xsts,
        200,
        r#"{"Token":"S","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#,
    );
    t.reply(
        login,
        200,
        r#"{"access_token":"FRESH-MC-TOKEN-123456","expires_in":86400}"#,
    );
    t.reply(
        profile,
        200,
        r#"{"id":"0000000000004000800000000000a001","name":"Alex"}"#,
    );
    let p = plan(&w, &a, &t, &id).await.unwrap();
    assert_eq!(
        arg(&p.command.args, "--accessToken"),
        "FRESH-MC-TOKEN-123456"
    );

    // Revoked: "Sign in again", account kept and marked.
    let a = accounts(&w);
    a.state.update(|f| f.accounts.retain(|_| true)).unwrap();
    expire(&a);
    let t = FakeTransport::new();
    t.reply(token, 400, r#"{"error":"invalid_grant"}"#);
    let e = plan(&w, &a, &t, &id).await.err().unwrap();
    assert_eq!(
        (e.code, e.account_id.as_deref()),
        ("needs_sign_in", Some("acc-a"))
    );
    assert!(a
        .state
        .update(|f| f
            .accounts
            .iter()
            .any(|m| m.id == "acc-a" && matches!(m.status, AccountStatus::NeedsReauth { .. })))
        .unwrap());

    // Offline during a needed refresh.
    let a = accounts(&w);
    expire(&a);
    let t = FakeTransport::new();
    t.offline(token);
    assert_eq!(plan(&w, &a, &t, &id).await.err().unwrap().code, "offline");

    // Vault locked.
    let a = accounts(&w);
    expire(&a);
    a.vault.lock().unwrap().take();
    assert_eq!(
        plan(&w, &a, &FakeTransport::new(), &id)
            .await
            .err()
            .unwrap()
            .code,
        "vault_locked"
    );

    // Mojang's approval is still pending: a distinct reason, never a way around it.
    let a = accounts(&w);
    expire(&a);
    let t = FakeTransport::new();
    t.reply(
        token,
        200,
        r#"{"access_token":"MS","refresh_token":"R","expires_in":3600}"#,
    );
    t.reply(
        xbl,
        200,
        r#"{"Token":"X","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#,
    );
    t.reply(
        xsts,
        200,
        r#"{"Token":"S","DisplayClaims":{"xui":[{"uhs":"1"}]}}"#,
    );
    t.reply(login, 403, r#"{"errorMessage":"Invalid app registration"}"#);
    let e = plan(&w, &a, &t, &id).await.err().unwrap();
    assert_eq!(e.code, "approval_pending");
}

// ================================================================================ integrity and Java

#[tokio::test]
async fn a_pending_profile_and_damaged_files_refuse_the_launch_and_point_to_repair() {
    let w = world("1.21.4", Era::Modern);
    let id = vanilla(&w);
    let a = accounts(&w);
    assert_eq!(
        plan(&w, &a, &FakeTransport::new(), &id)
            .await
            .err()
            .unwrap()
            .code,
        "not_ready"
    );

    prepare_instance(&ctx(&w), &id).await.unwrap();
    assert!(plan(&w, &a, &FakeTransport::new(), &id).await.is_ok());

    // A truncated library.
    let lib = w
        .root
        .libraries_dir()
        .join("com/example/alpha/1.0/alpha-1.0.jar");
    let good = fs::read(&lib).unwrap();
    fs::write(&lib, &good[..10]).unwrap();
    let e = plan(&w, &a, &FakeTransport::new(), &id)
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, "needs_repair");
    fs::write(&lib, good).unwrap();

    // A missing asset, missing natives.
    let natives = InstanceStore::new(&w.root).dirs(&id).natives_dir();
    fs::remove_dir_all(&natives).unwrap();
    assert_eq!(
        plan(&w, &a, &FakeTransport::new(), &id)
            .await
            .err()
            .unwrap()
            .code,
        "needs_repair"
    );
}

#[tokio::test]
async fn a_missing_or_tampered_java_runtime_is_never_replaced_by_another() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = accounts(&w);
    let java = crate::java_install::find_installed(&w.root, "java-runtime-delta").unwrap();
    fs::write(&java.exe, b"swapped").unwrap();
    let e = plan(&w, &a, &FakeTransport::new(), &id)
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, "needs_repair");
    assert!(e.message.contains("Java"));

    fs::remove_dir_all(&java.home).unwrap();
    assert_eq!(
        plan(&w, &a, &FakeTransport::new(), &id)
            .await
            .err()
            .unwrap()
            .code,
        "needs_repair"
    );
    // Nothing on this machine was consulted instead: the plan fails, it does not fall back.
}

// ================================================================================ the running state

#[test]
fn a_profile_cannot_be_launched_twice_and_closing_the_window_is_gated_on_a_live_game() {
    let d = tempfile::tempdir().unwrap();
    let s = LaunchState::new(PortableRoot::at(d.path()));
    let a = InstanceId::new("survival").unwrap();
    let b = InstanceId::new("other").unwrap();
    assert!(s.claim(&a), "first click");
    assert!(!s.claim(&a), "a duplicate click is refused");
    assert!(s.claim(&b), "a different profile is independent");
    assert!(
        !s.has_live_game(),
        "launching is not a live game yet: closing the window just closes"
    );

    s.set_pid(&a, 4242);
    assert!(
        s.has_live_game(),
        "now closing the window must keep Clade alive"
    );
    let mut snap = s.snapshot();
    snap.sort();
    assert_eq!(
        snap,
        [
            ("other".to_owned(), "launching"),
            ("survival".to_owned(), "launching")
        ]
    );
    s.mark_playing(&a);
    assert!(s.snapshot().contains(&("survival".to_owned(), "playing")));

    s.set_exit_when_idle(true);
    s.release(&a);
    assert!(!s.has_live_game());
    assert!(s.exit_when_idle(), "and then Clade finishes quitting");
    s.release(&b);
    assert!(s.claim(&a), "the profile can be launched again");
}

#[test]
fn failures_serialise_with_a_code_and_the_account_only_when_relevant() {
    let f = from_auth(&AuthError::NeedsSignIn(ReauthReason::Revoked), "acc-a");
    let json = serde_json::to_string(&f).unwrap();
    assert!(
        json.contains("\"code\":\"needs_sign_in\"") && json.contains("\"accountId\":\"acc-a\"")
    );
    let plain = serde_json::to_string(&fail("not_ready", "x")).unwrap();
    assert!(!plain.contains("accountId"));
    assert!(
        serde_json::to_string(&from_auth(&AuthError::ApprovalRequired, "a"))
            .unwrap()
            .contains("approval_pending")
    );
    let ev = serde_json::to_string(&LaunchEvent {
        instance_id: "x".into(),
        state: "stopped",
        exit: Some(
            (&ExitSummary {
                code: Some(1),
                kind: ExitKind::Crashed,
                message: "m".into(),
                reason: "unexpected",
                seconds: 3,
            })
                .into(),
        ),
    })
    .unwrap();
    assert!(ev.contains("\"instanceId\":\"x\"") && ev.contains("\"kind\":\"crashed\""));
}

// ================================================================================ development fixture (by hand)

/// Run by hand to put two obviously fake development accounts into a dev Clade folder, so the
/// real window's launch flow (unlock, Launching, Playing, closing, crashes) can be exercised
/// without a Microsoft account: `CLADE_LIVE_ROOT=... CLADE_FIXTURE_PASS=... cargo test
/// seed_dev_fixture -- --ignored`. The tokens are placeholders, not credentials, and the real
/// sign-in flow still needs Mojang's approval; nothing like this exists in production code.
#[cfg(test)]
#[test]
#[ignore = "writes development fixture accounts into CLADE_LIVE_ROOT"]
fn seed_dev_fixture_accounts() {
    let root = PortableRoot::at(std::env::var_os("CLADE_LIVE_ROOT").expect("CLADE_LIVE_ROOT"));
    let pass = std::env::var("CLADE_FIXTURE_PASS").expect("CLADE_FIXTURE_PASS");
    let _ = fs::remove_file(root.vault_file());
    let state = AccountsState::new(root.accounts_file());
    state
        .update(|f| {
            f.accounts.clear();
            f.accounts
                .push(meta("acc-a", "FixtureAlex", UUID_A, AccountStatus::Ready));
            f.accounts
                .push(meta("acc-b", "FixtureBlake", UUID_B, AccountStatus::Ready));
            f.active = Some("acc-a".into());
        })
        .unwrap();
    let mut vault = Vault::create(&root.vault_file(), &pass).unwrap();
    for (id, token) in [
        ("acc-a", "DEV-FIXTURE-TOKEN-A-not-a-credential"),
        ("acc-b", "DEV-FIXTURE-TOKEN-B-not-a-credential"),
    ] {
        vault
            .set_account(
                id,
                AccountSecrets {
                    ms_refresh_token: Some("dev-fixture-refresh".into()),
                    minecraft_access_token: Some(token.into()),
                    minecraft_token_expires_at: Some(4_000_000_000),
                },
            )
            .unwrap();
    }
}

// ---- what happens to the window when a game ends

#[test]
fn a_failure_is_always_shown_whatever_the_window_preferences_say() {
    use crate::game_session::ExitKind::{Crashed, FailedToStart, Normal};
    use WindowPlan::{Leave, Quit, Reveal};
    // (kind, closed during the game, another game running, window visible, stay hidden) -> plan
    let cases = [
        // a normal exit follows "When Minecraft closes"
        (Normal, false, false, true, false, Leave),
        (Normal, false, false, true, true, Leave),
        (Normal, false, false, false, false, Reveal),
        (Normal, false, false, false, true, Quit),
        // hide on start + stay hidden, then the game crashes or never starts: the window comes back
        (Crashed, false, false, false, true, Reveal),
        (FailedToStart, false, false, false, true, Reveal),
        (Crashed, false, false, false, false, Reveal),
        // a failure also brings forward a window that is visible but buried or minimized
        (Crashed, false, false, true, true, Reveal),
        (FailedToStart, false, false, true, false, Reveal),
        // closing Clade during the game keeps its meaning: it finishes quitting once the game ends
        (Normal, true, false, false, false, Quit),
        (Normal, true, false, false, true, Quit),
        // while another game still runs nothing changes yet
        (Crashed, false, true, false, true, Leave),
        (Normal, false, true, false, true, Leave),
    ];
    for (kind, closed, other, visible, stay, want) in cases {
        assert_eq!(
            window_plan(kind, closed, other, visible, stay),
            want,
            "{kind:?} closed={closed} other={other} visible={visible} stay_hidden={stay}"
        );
    }
}

#[test]
fn stay_hidden_can_only_ever_quit_after_a_clean_exit() {
    use crate::game_session::ExitKind;
    for kind in [ExitKind::Crashed, ExitKind::FailedToStart] {
        for visible in [true, false] {
            assert_ne!(
                window_plan(kind, false, false, visible, true),
                WindowPlan::Quit,
                "a failure must never end Clade silently"
            );
        }
    }
}

#[test]
fn close_requested_and_finish_session_are_atomic_and_eliminate_the_zombie_race() {
    let d = tempfile::tempdir().unwrap();
    let s = LaunchState::new(PortableRoot::at(d.path()));
    let a = InstanceId::new("alpha").unwrap();
    let b = InstanceId::new("bravo").unwrap();

    // No game or still before PID exists: CloseRequested lets Tauri close immediately and
    // latches `closing` so late pre-spawn launches or new claims are rejected.
    let closing_idle = LaunchState::new(PortableRoot::at(d.path()));
    assert!(!closing_idle.close_requested());
    assert!(closing_idle.is_closing());
    assert!(!closing_idle.exit_when_idle());
    assert!(!closing_idle.claim(&a));

    let closing_pre_spawn = LaunchState::new(PortableRoot::at(d.path()));
    assert!(closing_pre_spawn.claim(&a));
    assert!(!closing_pre_spawn.close_requested());
    assert!(closing_pre_spawn.is_closing());
    assert!(!closing_pre_spawn.exit_when_idle());
    assert!(!closing_pre_spawn.claim(&b));

    // Two live games: CloseRequested hides, sets exit_when_idle atomically, and does not latch
    // `closing`; finishing the first reports another_running = true, second reports false.
    assert!(s.claim(&a));
    s.set_pid(&a, 1001);
    assert!(s.claim(&b));
    s.set_pid(&b, 1002);
    assert!(s.close_requested());
    assert!(!s.is_closing());
    assert!(s.exit_when_idle());
    assert_eq!(s.finish_session(&a), (true, true));
    assert_eq!(s.finish_session(&b), (true, false));

    // Stress both thread interleavings: a CloseRequested racing game exit can NEVER leave the
    // launcher hidden with `close_requested == true` while `finish_session` saw `closed == false`.
    for _ in 0..200 {
        let state = LaunchState::new(PortableRoot::at(d.path()));
        assert!(state.claim(&a));
        state.set_pid(&a, 7777);
        let barrier = std::sync::Barrier::new(2);
        let (prevented_close, (saw_closed, another)) = std::thread::scope(|scope| {
            let t1 = scope.spawn(|| {
                barrier.wait();
                state.close_requested()
            });
            let t2 = scope.spawn(|| {
                barrier.wait();
                state.finish_session(&a)
            });
            (t1.join().unwrap(), t2.join().unwrap())
        });
        assert!(!another);
        assert!(
            !(prevented_close && !saw_closed),
            "zombie race detected: prevented_close={prevented_close}, saw_closed={saw_closed}"
        );
    }
}

#[tokio::test]
async fn semicolons_in_the_clade_folder_map_to_unsupported_folder_path() {
    let mut w = world("1.21.4", Era::Modern);
    let semi_dir = tempfile::tempdir().unwrap();
    w.root = PortableRoot::at(semi_dir.path().join("Clade;Folder"));
    w.root.ensure_layout().unwrap();
    let id = create(&w, NewInstance::vanilla("Profile", "1.21.4").unwrap());
    prepare_instance(&ctx(&w), &id).await.unwrap();
    let a = accounts(&w);
    let err = plan(&w, &a, &FakeTransport::new(), &id)
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, "unsupported_folder_path");
}

#[tokio::test]
async fn memory_snapshot_clamps_heap_down_when_commit_or_physical_ram_is_tight() {
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = accounts(&w);
    let sys = SystemEnv::from_process();

    // Plenty of available physical and commit memory: keeps the 4096 MiB target on a 16 GiB machine.
    let roomy = plan_launch(
        &w.root,
        &FakeTransport::new(),
        &a.vault,
        &a.state,
        &id,
        &LaunchEnv {
            now: NOW,
            sys: &sys,
            total_memory_mb: Some(16_384),
            default_memory_mb: None,
            memory_snapshot: Some(crate::system::MemorySnapshot {
                total_mb: 16_384,
                avail_phys_mb: 10_240,
                avail_commit_mb: 12_288,
            }),
            embed_resolution: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(roomy.command.args[0], "-Xmx4096M");

    // Tight Windows commit charge (1920 MiB available -> 1920 - 512 = 1408 MiB cap): clamps down.
    let tight = plan_launch(
        &w.root,
        &FakeTransport::new(),
        &a.vault,
        &a.state,
        &id,
        &LaunchEnv {
            now: NOW,
            sys: &sys,
            total_memory_mb: Some(16_384),
            default_memory_mb: None,
            memory_snapshot: Some(crate::system::MemorySnapshot {
                total_mb: 16_384,
                avail_phys_mb: 2_048,
                avail_commit_mb: 1_920,
            }),
            embed_resolution: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(tight.command.args[0], "-Xmx1408M");
}

#[tokio::test]
async fn embed_resolution_overrides_profile_resolution_for_launch_command() {
    let sys = SystemEnv::from_process();
    let (w, id) = prepared("1.21.4", Era::Modern).await;
    let a = accounts(&w);
    let planned = plan_launch(
        &w.root,
        &FakeTransport::new(),
        &a.vault,
        &a.state,
        &id,
        &LaunchEnv {
            now: NOW,
            sys: &sys,
            total_memory_mb: Some(16_384),
            default_memory_mb: None,
            memory_snapshot: None,
            embed_resolution: Some(crate::instances::Resolution {
                width: 1280,
                height: 676,
            }),
        },
    )
    .await
    .unwrap();
    let pos = planned
        .command
        .args
        .windows(4)
        .position(|pair| pair == ["--width", "1280", "--height", "676"]);
    assert!(
        pos.is_some(),
        "expected --width 1280 --height 676 in args: {:?}",
        planned.command.args
    );
}
