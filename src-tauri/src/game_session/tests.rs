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
fn controlled_child(pre_script: &str) -> (Child, tokio::process::ChildStdin) {
    let script = if pre_script.is_empty() {
        "set /p _clade_wait= & exit /b 0".to_owned()
    } else {
        format!("{pre_script} & set /p _clade_wait= & exit /b 0")
    };
    // Test code only: production spawns go through `GameCommand::to_process`.
    let mut c = tokio::process::Command::new("cmd")
        .args(["/D", "/C", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = c.stdin.take().unwrap();
    (c, stdin)
}

#[cfg(test)]
struct KillOnDrop(std::process::Child);

#[cfg(test)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
async fn wait_for_heartbeat_ms(root: &PortableRoot, min_ms: u64) -> u64 {
    let active_dir = root.data_dir().join("playtime-active");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(entries) = fs::read_dir(&active_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(bytes) = fs::read(&p) {
                        if let Ok(active) =
                            serde_json::from_slice::<crate::playtime::ActiveSession>(&bytes)
                        {
                            if active.accumulated_ms >= min_ms {
                                return active.accumulated_ms;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for playtime-active heartbeat >= {min_ms} ms"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
fn watch<'a>(e: &'a Env, secrets: &'a [String], tracker: Option<SessionTracker>) -> Watch<'a> {
    Watch {
        root: &e.root,
        instance: &e.id,
        secrets,
        header: "java -Xmx2048M --accessToken <redacted> net.minecraft.client.main.Main",
        output: None,
        tracker,
        heartbeat: Duration::from_millis(300),
        grace: Duration::from_millis(400),
        now_unix: clock,
    }
}

#[cfg(test)]
fn session_for(root: &PortableRoot, id: &InstanceId, pid: u32) -> SessionTracker {
    PlaytimeStore::new(root)
        .begin_session(id, pid, clock(), Instant::now())
        .unwrap()
        .with_unbiased_clock()
}

#[cfg(test)]
fn session(e: &Env, pid: u32) -> SessionTracker {
    session_for(&e.root, &e.id, pid)
}

#[tokio::test]
async fn a_normal_exit_is_logged_summarised_and_counted_as_played_time() {
    let e = env();
    let (c, stdin) = controlled_child("echo Setting user: Steve & echo Reached the main menu");
    let tracker = session(&e, c.id().unwrap());
    let playing = Arc::new(AtomicUsize::new(0));
    let p2 = playing.clone();
    let (root, id) = (e.root.clone(), e.id.clone());
    let handle = tokio::spawn(async move {
        let w = Watch {
            root: &root,
            instance: &id,
            secrets: &[],
            header: "java -Xmx2048M --accessToken <redacted> net.minecraft.client.main.Main",
            output: None,
            tracker: Some(tracker),
            heartbeat: Duration::from_millis(100),
            grace: Duration::from_millis(150),
            now_unix: clock,
        };
        supervise(c, w, move || {
            p2.fetch_add(1, Ordering::SeqCst);
        })
        .await
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while playing.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "on_playing never fired"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let heartbeat_ms = wait_for_heartbeat_ms(&e.root, 600).await;
    assert!(
        heartbeat_ms >= 600,
        "heartbeat did not persist non-zero accumulated_ms before exit"
    );
    assert_eq!(
        fs::read_dir(e.root.data_dir().join("playtime-active"))
            .unwrap()
            .count(),
        1,
        "active journal entry exists while running"
    );

    drop(stdin);
    let out = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("supervisor did not finish after stdin closed")
        .unwrap();

    assert_eq!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.code, Some(0));
    assert_eq!(
        playing.load(Ordering::SeqCst),
        1,
        "Playing fires once, after the grace period"
    );

    // Playtime: only the time the process was alive, per instance and in total.
    let secs = out.banked_seconds.expect("a session was banked");
    assert!((1..=30).contains(&secs), "{secs}");
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
    let raw_header = format!("java -Xmx2048M --accessToken {token} net.minecraft.client.main.Main");
    let mut w = watch(&e, &secrets, None);
    w.header = &raw_header;
    let out = supervise(c, w, || {}).await;
    let log = fs::read_to_string(out.log_path.unwrap()).unwrap();
    assert!(!log.contains(&token), "{log}");
    assert!(log.contains(
        "Command: java -Xmx2048M --accessToken <redacted> net.minecraft.client.main.Main"
    ));
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
    let out = supervise(child("exit /b 1"), w, move || {
        c2.fetch_add(1, Ordering::SeqCst);
    })
    .await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert_eq!(out.exit.kind, ExitKind::FailedToStart);
    assert_eq!(out.exit.code, Some(1));
    assert!(out.banked_seconds.is_none());
    assert!(out.log_path.is_some());
}

#[tokio::test]
async fn the_supervisor_keeps_working_when_its_starter_is_gone() {
    // Closing Clade's window must not end the watch: it is an independent task that owns the child.
    let Env { _dir, root, id } = env();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let playing = Arc::new(AtomicUsize::new(0));
    let stdin = {
        let starter_root = root.clone();
        let starter_id = id.clone();
        let (c, stdin) = controlled_child("");
        let tracker = session_for(&starter_root, &starter_id, c.id().unwrap());
        let p2 = playing.clone();
        let join = tokio::spawn(async move {
            let w = Watch {
                root: &starter_root,
                instance: &starter_id,
                secrets: &[],
                header: "controlled test process",
                output: None,
                tracker: Some(tracker),
                heartbeat: Duration::from_millis(100),
                grace: Duration::from_millis(150),
                now_unix: clock,
            };
            let out = supervise(c, w, move || {
                p2.fetch_add(1, Ordering::SeqCst);
            })
            .await;
            let _ = done_tx.send(out);
        });
        // The starter's JoinHandle and cloned state are explicitly dropped; the game goes on.
        drop(join);
        stdin
    };

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while playing.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "on_playing never fired"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let heartbeat_ms = wait_for_heartbeat_ms(&root, 600).await;
    assert!(
        heartbeat_ms >= 600,
        "detached supervisor did not persist heartbeat: {heartbeat_ms}"
    );

    drop(stdin);
    let out = tokio::time::timeout(Duration::from_secs(10), done_rx)
        .await
        .expect("supervisor did not finish after stdin closed")
        .expect("supervisor task dropped sender");
    assert_eq!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.code, Some(0));
    let secs = out.banked_seconds.expect("banked");
    assert!((1..=30).contains(&secs), "{secs}");
    assert_eq!(
        PlaytimeStore::new(&root)
            .instance_total(&id)
            .unwrap()
            .total_seconds,
        secs
    );
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
    assert!(
        matches!(r.apply("x abc y plain line z"), Cow::Borrowed(_)),
        "lines without secrets borrow in place with zero allocations"
    );
    let redacted = r.apply("x abc y LONGER-SECRET-VALUE z");
    assert!(matches!(redacted, Cow::Owned(_)));
    assert_eq!(
        redacted, "x abc y <redacted> z",
        "very short values are not worth (or safe) to replace"
    );
}

/// A child whose stdout and stderr are the launcher-owned output file, exactly like a real game.
#[cfg(test)]
fn child_with_output_file(script: &str, e: &Env) -> (Child, PathBuf) {
    let (file, path) = open_output(&e.root, &e.id).unwrap();
    let other = file.try_clone().unwrap();
    // Test code only: production spawns go through `GameCommand::to_process`.
    let c = tokio::process::Command::new("cmd")
        .args(["/D", "/C", script])
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(other))
        .spawn()
        .unwrap();
    (c, path)
}

#[tokio::test]
async fn output_written_to_the_launcher_owned_file_reaches_the_redacted_session_log() {
    let e = env();
    let token = "SECRET-ACCESS-TOKEN-0123456789".to_owned();
    let (c, path) = child_with_output_file(
        &format!(
            "echo Setting user: Steve & echo token:{token}:uuid 1>&2 & ping -n 2 127.0.0.1 >nul & echo last line without newline<nul & exit /b 0"
        ),
        &e,
    );
    let secrets = [token.clone()];
    let mut w = watch(&e, &secrets, Some(session(&e, c.id().unwrap())));
    w.output = Some(path.as_path());
    let out = supervise(c, w, || {}).await;

    assert_eq!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.code, Some(0));
    let log = fs::read_to_string(out.log_path.unwrap()).unwrap();
    assert!(log.contains("Setting user: Steve"), "{log}");
    assert!(
        log.contains("token:<redacted>:uuid") && !log.contains(&token),
        "{log}"
    );
    assert!(log.contains("last line without newline"), "{log}");
    assert!(out.banked_seconds.is_some(), "playtime is still banked");
}

#[tokio::test]
async fn a_crash_is_still_detected_and_explained_when_the_output_is_a_file() {
    let e = env();
    let (c, path) = child_with_output_file(
        "echo Exception in thread main java.lang.OutOfMemoryError: Java heap space 1>&2 & ping -n 2 127.0.0.1 >nul & exit /b 1",
        &e,
    );
    let mut w = watch(&e, &[], None);
    w.output = Some(path.as_path());
    let out = supervise(c, w, || {}).await;
    assert_eq!(out.exit.code, Some(1));
    assert_ne!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.reason, "out_of_memory");
}

#[tokio::test]
async fn a_game_whose_output_is_a_file_keeps_running_and_keeps_writing_with_nobody_reading() {
    // The F2 regression: with a pipe, the game's writes start failing once its reader (Clade) is gone.
    // With a file there is no reader to lose: nothing watches this child at all and it still finishes
    // normally and all of its output is there.
    let e = env();
    let (mut c, path) = child_with_output_file(
        "echo before & ping -n 3 127.0.0.1 >nul & echo after the reader would have gone & exit /b 0",
        &e,
    );
    let status = c.wait().await.unwrap();
    assert_eq!(status.code(), Some(0));
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("before") && text.contains("after the reader would have gone"));
}

#[test]
fn output_files_are_launcher_owned_portable_emptied_by_the_startup_sweep_and_start_empty() {
    use std::io::Write;
    let e = env();
    let (mut game, path) = open_output(&e.root, &e.id).unwrap();
    assert!(
        path.starts_with(e.root.cache_dir()),
        "inside the Clade folder: {path:?}"
    );
    assert_eq!(path.parent(), Some(output_dir(&e.root).as_path()));

    // an orphaned game is still appending to its file when the next Clade starts
    game.write_all(
        b"old output
",
    )
    .unwrap();
    assert!(fs::metadata(&path).unwrap().len() > 0);
    sweep_output(&e.root);
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        0,
        "stale output is emptied"
    );
    // ... and its next write lands at the start of the emptied file, with no zero-filled gap
    game.write_all(
        b"new
",
    )
    .unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"new
"
    );
    sweep_output(&e.root);
    sweep_output(&e.root); // nothing to do twice: harmless

    // a new game for the same instance starts from an empty file even if old output was left there
    game.write_all(
        b"leftover
",
    )
    .unwrap();
    let (_second, again) = open_output(&e.root, &e.id).unwrap();
    assert_eq!(again, path);
    assert_eq!(fs::metadata(&again).unwrap().len(), 0);
}

#[tokio::test]
async fn the_output_file_stays_bounded_however_much_the_game_prints() {
    use std::io::Write;
    let e = env();
    let (mut game, path) = open_output(&e.root, &e.id).unwrap();
    let (tx, mut rx) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(tail_file(path.clone(), tx, done_rx));
    let consumer = tokio::spawn(async move {
        let (mut lines, mut odd) = (0usize, 0usize);
        while let Some(l) = rx.recv().await {
            lines += 1;
            if l.len() != 60 {
                odd += 1;
            }
        }
        (lines, odd)
    });

    // a noisy mod: about four times the bound, in 1 MiB bursts of numbered lines
    let burst: String = (0..17_000)
        .map(|i| {
            format!(
                "{i:060}
"
            )
        })
        .collect();
    assert!(burst.len() >= 1_000_000);
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut biggest = 0u64;
        for _ in 0..34 {
            // A noisy game is not slowed down by Clade, but this test must not depend on how busy the
            // machine is: it only writes while the file is below 1.5 times the bound, and fails if the
            // file is never emptied (then it would just keep growing).
            let started = std::time::Instant::now();
            while fs::metadata(&writer_path).unwrap().len() > OUTPUT_LIMIT_BYTES * 3 / 2 {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "the output file was never emptied"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            game.write_all(burst.as_bytes()).unwrap();
            biggest = biggest.max(fs::metadata(&writer_path).unwrap().len());
            std::thread::sleep(Duration::from_millis(5));
        }
        biggest
    });
    let biggest = tokio::task::spawn_blocking(move || writer.join().unwrap())
        .await
        .unwrap();
    let _ = done_tx.send(true);
    reader.await.unwrap();
    let (lines, odd) = consumer.await.unwrap();

    let written: u64 = 34 * 1_037_000;
    assert!(
        written > 3 * OUTPUT_LIMIT_BYTES,
        "the test really writes far more than the bound"
    );
    assert!(
        biggest <= OUTPUT_LIMIT_BYTES * 3 / 2 + 1_100_000,
        "the file reached {biggest} bytes (bound {OUTPUT_LIMIT_BYTES})"
    );
    assert!(
        lines >= 500_000,
        "almost all of the output arrived: {lines} lines"
    );
    assert!(
        odd <= 8,
        "{odd} cut lines (at most one or two per emptying)"
    );
}

#[tokio::test]
async fn lines_are_cut_on_newlines_and_a_partial_line_waits_for_the_rest() {
    let (tx, mut rx) = mpsc::channel(LINE_CHANNEL_CAPACITY);
    let mut pending = b"one\r\ntwo\npart".to_vec();
    drain_lines(&mut pending, &tx).await;
    assert_eq!(rx.try_recv().unwrap(), "one");
    assert_eq!(rx.try_recv().unwrap(), "two");
    assert!(rx.try_recv().is_err());
    assert_eq!(pending, b"part");
    pending.extend_from_slice(b"ial\n");
    drain_lines(&mut pending, &tx).await;
    assert_eq!(rx.try_recv().unwrap(), "partial");
    // a runaway line without a newline is cut instead of growing without bound
    let mut huge = vec![b'x'; MAX_PENDING_BYTES + 1];
    drain_lines(&mut huge, &tx).await;
    assert_eq!(rx.try_recv().unwrap().len(), MAX_PENDING_BYTES + 1);
    assert!(huge.is_empty());
}

#[tokio::test]
async fn rapid_restarts_within_the_same_second_write_distinct_pid_stamped_logs() {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let e = env();
    let (c1, stdin1) = controlled_child("echo first session");
    let pid1 = c1.id().unwrap();
    // Hold a kernel process handle to `pid1` until `c2` is spawned so Windows cannot reuse `pid1`.
    // SAFETY: `OpenProcess` returns null on failure and a fresh handle owned by `pin_pid1` on success.
    let raw1 = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid1) };
    assert!(!raw1.is_null(), "failed to pin pid {pid1}");
    let pin_pid1 = unsafe { OwnedHandle::from_raw_handle(raw1.cast()) };
    drop(stdin1);
    let out1 = supervise(c1, watch(&e, &[], None), || {}).await;

    let c2 = child("echo second session & exit /b 0");
    let pid2 = c2.id().unwrap();
    drop(pin_pid1);
    let out2 = supervise(c2, watch(&e, &[], None), || {}).await;

    assert_ne!(pid1, pid2);
    let p1 = out1.log_path.unwrap();
    let p2 = out2.log_path.unwrap();
    assert_eq!(
        p1.file_name().unwrap().to_str().unwrap(),
        format!("{}-{pid1}.log", clock())
    );
    assert_eq!(
        p2.file_name().unwrap().to_str().unwrap(),
        format!("{}-{pid2}.log", clock())
    );
    assert_ne!(p1, p2);
    assert!(fs::read_to_string(&p1).unwrap().contains("first session"));
    assert!(fs::read_to_string(&p2).unwrap().contains("second session"));
}

#[test]
fn sweep_output_preserves_live_game_output_while_emptying_stale_files() {
    use std::io::Write;
    let e = env();
    let dead_id = InstanceStore::new(&e.root)
        .create(NewInstance::vanilla("Dead", "1.21.4").unwrap(), 2)
        .unwrap()
        .id;
    let (mut live_out, live_path) = open_output(&e.root, &e.id).unwrap();
    let (mut dead_out, dead_path) = open_output(&e.root, &dead_id).unwrap();
    live_out.write_all(b"still playing\n").unwrap();
    dead_out.write_all(b"already exited\n").unwrap();

    let mut proc = KillOnDrop(
        std::process::Command::new("cmd")
            .args(["/D", "/C", "set /p _clade_wait= & exit /b 0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdin = proc.0.stdin.take().unwrap();
    let started = crate::profiles::now_unix();
    let _tracker = PlaytimeStore::new(&e.root)
        .begin_session(&e.id, proc.0.id(), started, Instant::now())
        .unwrap();

    sweep_output(&e.root);
    assert_eq!(
        fs::read(&live_path).unwrap(),
        b"still playing\n",
        "live game's .out must not be truncated by sweep_output"
    );
    assert_eq!(
        fs::metadata(&dead_path).unwrap().len(),
        0,
        "dead game's .out is emptied"
    );

    drop(stdin);
    let _ = proc.0.wait().unwrap();
    sweep_output(&e.root);
    assert_eq!(
        fs::metadata(&live_path).unwrap().len(),
        0,
        "once the process exits, sweep_output empties its .out too"
    );
}

#[test]
fn redactor_sorts_overlapping_secrets_longest_first() {
    // Shorter secret provided first must not leave a trailing/leading fragment of the longer one.
    let r = Redactor::new(&[
        "short".to_owned(), // < 6 chars: ignored
        "TOKEN-KEY".to_owned(),
        "TOKEN-KEY-LONGER".to_owned(),
        "PREFIX-TOKEN-KEY".to_owned(),
    ]);
    let out = r.apply("short TOKEN-KEY-LONGER and PREFIX-TOKEN-KEY and TOKEN-KEY end");
    assert_eq!(out, "short <redacted> and <redacted> and <redacted> end");
}

#[test]
fn session_log_clips_utf8_lines_and_caps_at_max_log_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, path) = SessionLog::open(dir.path(), "cap-test.log").unwrap();

    // Multi-byte UTF-8 (> MAX_LINE_CHARS bytes, <= MAX_LINE_CHARS chars): kept in full.
    let under_char_limit = "é".repeat(MAX_LINE_CHARS - 10);
    assert!(under_char_limit.len() > MAX_LINE_CHARS);
    log.line(&under_char_limit);

    // Multi-byte UTF-8 (> MAX_LINE_CHARS chars): clipped at exact MAX_LINE_CHARS char boundary.
    let over_char_limit = "é".repeat(MAX_LINE_CHARS + 5);
    log.line(&over_char_limit);
    log.flush();

    let first_pass = fs::read_to_string(&path).unwrap();
    let mut lines = first_pass.lines();
    assert_eq!(lines.next().unwrap().chars().count(), MAX_LINE_CHARS - 10);
    assert_eq!(lines.next().unwrap().chars().count(), MAX_LINE_CHARS);
    assert!(lines.next().is_none());

    // Fill up past MAX_LOG_BYTES and confirm truncation marker + hard stop.
    let chunk = "a".repeat(MAX_LINE_CHARS);
    let max_iterations = (MAX_LOG_BYTES / MAX_LINE_CHARS as u64) as usize + 16;
    for _ in 0..max_iterations {
        log.line(&chunk);
    }
    log.line("must not appear after cap");
    log.flush();

    let full = fs::read_to_string(&path).unwrap();
    assert_eq!(
        full.matches("[log truncated: size limit reached]").count(),
        1
    );
    assert!(!full.contains("must not appear after cap"));
    assert!(
        fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES + 64,
        "log grew past cap"
    );
}

#[tokio::test]
async fn supervise_continues_when_session_log_cannot_be_opened() {
    let e = env();
    // Place a regular file where `logs_dir` wants a directory so `SessionLog::open` fails.
    let games_logs_parent = e.root.logs_dir().join("games");
    fs::create_dir_all(&games_logs_parent).unwrap();
    fs::write(logs_dir(&e.root, &e.id), b"not a directory").unwrap();

    let c = child("echo running without log & exit /b 0");
    let tracker = session(&e, c.id().unwrap());
    let mut w = watch(&e, &[], Some(tracker));
    w.grace = Duration::from_millis(0);
    let out = supervise(c, w, || {}).await;

    assert!(out.log_path.is_none());
    assert_eq!(out.exit.kind, ExitKind::Normal);
    assert_eq!(out.exit.code, Some(0));
    assert!(out.banked_seconds.is_some());
}

#[tokio::test]
async fn tail_file_handles_missing_file_and_external_truncation() {
    use std::io::Write;
    let e = env();

    // Missing output file: returns promptly and closes the channel.
    let (tx_missing, mut rx_missing) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let (_done_tx_missing, done_rx_missing) = tokio::sync::watch::channel(false);
    tail_file(
        e.root.cache_dir().join("missing-game.out"),
        tx_missing,
        done_rx_missing,
    )
    .await;
    assert!(rx_missing.recv().await.is_none());

    // External truncation (len < read_total): rewinds to 0 and delivers subsequent lines.
    let (mut game, path) = open_output(&e.root, &e.id).unwrap();
    game.write_all(b"before external truncation\n").unwrap();
    let (tx, mut rx) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(tail_file(path.clone(), tx, done_rx));

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("before external truncation")
    );

    empty_file(&path).unwrap();
    // Write fewer bytes than `read_total` so `len < read_total` holds regardless of poll timing.
    game.write_all(b"after\n").unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .as_deref(),
        Some("after")
    );

    let _ = done_tx.send(true);
    reader.await.unwrap();
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn tail_file_truncates_output_when_runaway_writer_exceeds_double_bound() {
    use std::io::Write;
    let e = env();
    let (mut game, path) = open_output(&e.root, &e.id).unwrap();

    // Pre-populate past 2 * OUTPUT_LIMIT_BYTES so when `tail_file` crosses OUTPUT_LIMIT_BYTES,
    // `len != read_total` and `len >= OUTPUT_LIMIT_BYTES * 2` forces the runaway-writer reset.
    let line = format!("{}\n", "r".repeat(63));
    let target_bytes = OUTPUT_LIMIT_BYTES * 2 + 128 * 1024;
    let initial_lines = (target_bytes / line.len() as u64) as usize + 1;
    let block = line.repeat(1024);
    let mut written_lines = 0usize;
    while written_lines < initial_lines {
        let take = (initial_lines - written_lines).min(1024);
        if take == 1024 {
            game.write_all(block.as_bytes()).unwrap();
        } else {
            game.write_all(line.repeat(take).as_bytes()).unwrap();
        }
        written_lines += take;
    }
    game.flush().unwrap();
    assert!(fs::metadata(&path).unwrap().len() >= OUTPUT_LIMIT_BYTES * 2);

    let (tx, mut rx) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(tail_file(path.clone(), tx, done_rx));
    let consumer = tokio::spawn(async move {
        let mut count = 0usize;
        let mut saw_marker = false;
        while let Some(l) = rx.recv().await {
            count += 1;
            if l == "after-runaway-cap" {
                saw_marker = true;
            }
        }
        (count, saw_marker)
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while fs::metadata(&path).unwrap().len() != 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "tail_file never emptied runaway output file"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    game.write_all(b"after-runaway-cap\n").unwrap();
    game.flush().unwrap();
    let _ = done_tx.send(true);
    reader.await.unwrap();
    let (count, saw_marker) = consumer.await.unwrap();

    assert!(
        saw_marker,
        "tail_file kept reading after runaway truncation"
    );
    assert!(
        count < initial_lines,
        "runaway truncation skipped unread excess ({count} < {initial_lines})"
    );
}

#[tokio::test]
async fn pump_decodes_non_utf8_pipe_output_across_chunks() {
    use tokio::io::AsyncWriteExt;

    let (mut writer, reader) = tokio::io::duplex(64);
    let (tx, mut rx) = mpsc::channel::<String>(LINE_CHANNEL_CAPACITY);
    let pump_task = tokio::spawn(pump(reader, tx));

    writer.write_all(b"hello \xff\xfe").await.unwrap();
    writer.flush().await.unwrap();
    writer.write_all(b" world\r").await.unwrap();
    writer.flush().await.unwrap();
    writer.write_all(b"\n\xf0\x9f").await.unwrap();
    writer.flush().await.unwrap();
    writer.write_all(b"\xa6\x80 crab\nend").await.unwrap();
    drop(writer);

    let mut lines = Vec::new();
    while let Some(line) = rx.recv().await {
        lines.push(line);
    }
    pump_task.await.unwrap();

    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], "hello \u{FFFD}\u{FFFD} world");
    assert_eq!(lines[1], "\u{1F980} crab");
    assert_eq!(lines[2], "end");
}
