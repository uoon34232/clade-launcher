#[cfg(test)]
use super::*;
#[cfg(test)]
use crate::instances::{InstanceStore, NewInstance};
#[cfg(test)]
use std::process::Stdio;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
fn clock() -> i64 {
    1_700_000_000
}

#[cfg(test)]
struct Env {
    _dir: tempfile::TempDir,
    root: PortableRoot,
    id: InstanceId,
}

#[cfg(test)]
fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = PortableRoot::at(dir.path());
    root.ensure_layout().unwrap();
    fs::create_dir_all(root.data_dir().join("playtime-active")).unwrap();
    let id = InstanceStore::new(&root)
        .create(NewInstance::vanilla("Survival", "1.21.4").unwrap(), 1)
        .unwrap()
        .id;
    Env {
        _dir: dir,
        root,
        id,
    }
}

/// A controlled stand-in for the game: an ordinary Windows process running a tiny script.
#[cfg(test)]
fn child(script: &str) -> Child {
    // Test code only: production spawns go through `GameCommand::to_process`.
    tokio::process::Command::new("cmd")
        .args(["/D", "/C", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[cfg(test)]
fn watch<'a>(e: &'a Env, secrets: &'a [String], tracker: Option<SessionTracker>) -> Watch<'a> {
    Watch {
        root: &e.root,
        instance: &e.id,
        secrets,
        header: "java -Xmx2048M --accessToken <redacted> net.minecraft.client.main.Main",
        tracker,
        heartbeat: Duration::from_millis(300),
        grace: Duration::from_millis(400),
        now_unix: clock,
    }
}

#[cfg(test)]
fn session(e: &Env, pid: u32) -> SessionTracker {
    PlaytimeStore::new(&e.root)
        .begin_session(&e.id, pid, clock(), Instant::now())
        .unwrap()
}

#[tokio::test]
async fn a_normal_exit_is_logged_summarised_and_counted_as_played_time() {
    let e = env();
    let c = child("echo Setting user: Steve & echo Reached the main menu & ping -n 3 127.0.0.1 >nul & exit /b 0");
    let tracker = session(&e, c.id().unwrap());
    let playing = Arc::new(AtomicUsize::new(0));
    let p2 = playing.clone();
    let out = supervise(c, watch(&e, &[], Some(tracker)), move || {
        p2.fetch_add(1, Ordering::SeqCst);
    })
    .await;

    assert_eq!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.code, Some(0));
    assert_eq!(
        playing.load(Ordering::SeqCst),
        1,
        "Playing fires once, after the grace period"
    );

    // Playtime: only the time the process was alive (about two seconds), per instance and in total.
    let secs = out.banked_seconds.expect("a session was banked");
    assert!((1..=5).contains(&secs), "{secs}");
    let store = PlaytimeStore::new(&e.root);
    let inst = store.instance_total(&e.id).unwrap();
    let life = store.lifetime_total().unwrap();
    assert_eq!((inst.total_seconds, inst.sessions), (secs, 1));
    assert_eq!((life.total_seconds, life.sessions), (secs, 1));
    assert_eq!(
        fs::read_dir(e.root.data_dir().join("playtime-active"))
            .unwrap()
            .count(),
        0,
        "the journal entry is closed"
    );

    // The log has the header, the game's output and the footer.
    let log = fs::read_to_string(out.log_path.unwrap()).unwrap();
    assert!(log.contains("Command: java -Xmx2048M --accessToken <redacted>"));
    assert!(log.contains("Setting user: Steve") && log.contains("Reached the main menu"));
    assert!(log.contains("Exited with code 0"));
    assert!(log.starts_with("Clade game session"));
}

#[tokio::test]
async fn a_crash_still_counts_the_time_played_and_is_explained_briefly() {
    let e = env();
    let c = child("echo Exception in thread \"main\" java.lang.UnsatisfiedLinkError: no lwjgl in java.library.path 1>&2 & exit /b 1");
    let tracker = session(&e, c.id().unwrap());
    let out = supervise(c, watch(&e, &[], Some(tracker)), || {}).await;
    assert_eq!(out.exit.code, Some(1));
    assert_eq!(
        out.exit.kind,
        ExitKind::FailedToStart,
        "it died almost at once"
    );
    assert!(
        out.exit.message.contains("native library"),
        "{}",
        out.exit.message
    );
    assert!(
        out.banked_seconds.is_some(),
        "a crash exit is still a finished session"
    );
    assert_eq!(
        PlaytimeStore::new(&e.root)
            .instance_total(&e.id)
            .unwrap()
            .sessions,
        1
    );
    let log = fs::read_to_string(out.log_path.unwrap()).unwrap();
    assert!(log.contains("UnsatisfiedLinkError") && log.contains("Exited with code 1"));
}

#[tokio::test]
async fn the_session_secrets_never_reach_the_log_or_the_summary() {
    let e = env();
    let token = "SECRET-ACCESS-TOKEN-0123456789".to_owned();
    let c = child(&format!(
        "echo launching with --accessToken {token} 1>&2 & echo token:{token}:uuid & exit /b 2"
    ));
    let secrets = [token.clone()];
    let out = supervise(c, watch(&e, &secrets, None), || {}).await;
    let log = fs::read_to_string(out.log_path.unwrap()).unwrap();
    assert!(!log.contains(&token), "{log}");
    assert!(log.contains("--accessToken <redacted>") && log.contains("token:<redacted>:uuid"));
    assert!(!out.exit.message.contains(&token));
    assert!(out.banked_seconds.is_none(), "no tracker was given");
}

#[tokio::test]
async fn a_process_that_dies_inside_the_grace_period_never_becomes_playing() {
    let e = env();
    let called = Arc::new(AtomicUsize::new(0));
    let c2 = called.clone();
    let mut w = watch(&e, &[], None);
    w.grace = Duration::from_secs(30);
    supervise(child("exit /b 1"), w, move || {
        c2.fetch_add(1, Ordering::SeqCst);
    })
    .await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_supervisor_keeps_working_when_its_starter_is_gone() {
    // Closing Clade's window must not end the watch: it is an independent task that owns the child.
    let e = env();
    let c = child("ping -n 3 127.0.0.1 >nul & exit /b 0");
    let tracker = session(&e, c.id().unwrap());
    let (root, id) = (e.root.clone(), e.id.clone());
    let handle = tokio::spawn(async move {
        let w = Watch {
            root: &root,
            instance: &id,
            secrets: &[],
            header: "controlled test process",
            tracker: Some(tracker),
            heartbeat: Duration::from_millis(300),
            grace: Duration::from_millis(300),
            now_unix: clock,
        };
        supervise(c, w, || {}).await.banked_seconds
    });
    // The "window" and everything that started the game is dropped; the game goes on.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let secs = handle.await.unwrap().expect("banked");
    assert!((1..=5).contains(&secs));
}

#[test]
fn exits_are_classified_into_short_human_messages() {
    use std::os::windows::process::ExitStatusExt;
    let code = |c: u32| Some(ExitStatus::from_raw(c));
    let lines = |s: &str| vec![s.to_owned()];
    let long = Duration::from_secs(300);
    let short = Duration::from_secs(3);

    assert_eq!(summarize(code(0), long, &[]).kind, ExitKind::Normal);
    let cases = [
        ("Exception in thread main java.lang.UnsupportedClassVersionError: class file version 65.0", "different Java"),
        ("Error: Could not find or load main class net.minecraft.client.main.Main", "files are incomplete"),
        ("java.lang.OutOfMemoryError: Java heap space", "ran out of memory"),
        ("Error occurred during initialization of VM\nCould not reserve enough space for object heap", "free memory"),
        ("java.lang.NoClassDefFoundError: com/example/Thing", "library or mod"),
        ("# A fatal error has been detected by the Java Runtime Environment: EXCEPTION_ACCESS_VIOLATION", "graphics driver"),
        ("net.minecraftforge.fml.ModLoadingException: Mod X requires Y", "mod loader"),
    ];
    for (line, want) in cases {
        let s = summarize(code(1), long, &lines(line));
        assert!(s.message.contains(want), "{line} -> {}", s.message);
        assert_eq!(
            s.kind,
            ExitKind::Crashed,
            "after running a while it is a crash"
        );
        assert_eq!(
            summarize(code(1), short, &lines(line)).kind,
            ExitKind::FailedToStart
        );
    }
    let plain = summarize(code(137), long, &lines("nothing recognisable"));
    assert!(plain.message.contains("exit code 137"));
    assert_eq!(plain.reason, "unexpected");
    assert_eq!(plain.seconds, 300);
    assert!(summarize(None, long, &[]).message.contains("stopped"));
}

#[test]
fn every_exit_carries_a_stable_reason_code_for_the_ui_to_word() {
    use std::os::windows::process::ExitStatusExt;
    let code = |c: u32| Some(ExitStatus::from_raw(c));
    let lines = |s: &str| vec![s.to_owned()];
    let long = Duration::from_secs(300);
    let short = Duration::from_secs(1);
    let cases = [
        ("java.lang.UnsupportedClassVersionError: x", "wrong_java"),
        (
            "Could not find or load main class a.b.C",
            "incomplete_files",
        ),
        (
            "Could not reserve enough space for object heap",
            "no_memory",
        ),
        (
            "java.lang.OutOfMemoryError: Java heap space",
            "out_of_memory",
        ),
        ("java.lang.UnsatisfiedLinkError: lwjgl", "native_library"),
        ("EXCEPTION_ACCESS_VIOLATION", "java_crash"),
        ("ModLoadingException: x", "mod_loader"),
        ("Mixin apply failed x", "mixin"),
        ("Incompatible mod set", "incompatible_mods"),
        ("NoClassDefFoundError: x", "missing_class"),
        (
            "Error: Could not create the Java Virtual Machine.",
            "jvm_start",
        ),
    ];
    for (line, reason) in cases {
        assert_eq!(
            summarize(code(1), long, &lines(line)).reason,
            reason,
            "{line}"
        );
    }
    assert_eq!(summarize(code(0), long, &[]).reason, "closed");
    assert_eq!(
        summarize(code(3), short, &lines("nothing")).reason,
        "early_exit"
    );
    assert_eq!(
        summarize(code(3), long, &lines("nothing")).reason,
        "unexpected"
    );
    assert_eq!(summarize(None, long, &[]).reason, "stopped");
}

#[test]
fn only_the_newest_logs_are_kept_and_the_redactor_ignores_trivial_secrets() {
    let e = env();
    let dir = logs_dir(&e.root, &e.id);
    fs::create_dir_all(&dir).unwrap();
    for i in 0..15 {
        fs::write(dir.join(format!("{:010}.log", 1000 + i)), "x").unwrap();
    }
    prune(&dir);
    let names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), KEEP_LOGS);
    assert!(
        names.contains(&"0000001014.log".to_owned())
            && !names.contains(&"0000001000.log".to_owned())
    );

    let r = Redactor::new(&["abc".to_owned(), "LONGER-SECRET-VALUE".to_owned()]);
    assert_eq!(
        r.apply("x abc y LONGER-SECRET-VALUE z"),
        "x abc y <redacted> z",
        "very short values are not worth (or safe) to replace"
    );
}
