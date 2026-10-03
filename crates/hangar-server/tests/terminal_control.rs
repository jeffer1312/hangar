use hangar_server::terminal_control::{CaptureRequest, ControlEvent, ControlParser, FrameIdentity, TerminalPool, Limits};
use std::{path::PathBuf, process::Command, time::Duration};

fn request(consumer: &str) -> CaptureRequest {
    CaptureRequest { consumer: consumer.into(), name: "fixture".into(), provider: "claude".into(),
        binding: "conversation-a".into(), target: "=fixture:".into(), started: 42.5,
        lines: 200, colors: false, join: false }
}

#[test]
fn fragmented_frames_preserve_percent_body_and_exact_blank_lines() {
    let mut parser = ControlParser::default();
    let mut events = Vec::new();
    for chunk in b"%begin 1 7 0\n%begin exemplo literal\n%end 1 8 0\n\na\n%end 1 7 0\n".chunks(2) {
        events.extend(parser.push(chunk).unwrap());
    }
    assert_eq!(events, vec![ControlEvent::Frame { identity: FrameIdentity { timestamp: 1, command: 7, flags: 0 }, text: "%begin exemplo literal\n%end 1 8 0\n\na\n".into(), error: false }]);
}

#[test]
fn output_notifications_are_ignored_without_consuming_frame_body() {
    let mut parser = ControlParser::default();
    assert_eq!(parser.push(b"%output %3 ol\\303\\241\\134\n%begin 2 8 0\n%output %4 literal\n%error 2 8 0\n").unwrap(), vec![
        ControlEvent::Frame { identity: FrameIdentity { timestamp: 2, command: 8, flags: 0 }, text: "%output %4 literal\n".into(), error: true },
    ]);
}

#[test]
fn invalid_utf8_and_oversized_or_unfinished_frames_are_errors() {
    let mut parser = ControlParser::default();
    assert!(parser.push(b"%begin 1 1 0\n\xff\n%end 1 1 0\n").is_err());
    let mut parser = ControlParser::default();
    assert!(parser.push(&vec![b'a'; 1024 * 1024 + 1]).is_err());
    let mut parser = ControlParser::default();
    parser.push(b"%begin 1 1 0\nbody\n").unwrap();
    assert!(parser.finish().is_err());
}

#[cfg(unix)]
struct IsolatedTmux { _dir: tempfile::TempDir, socket: PathBuf }
#[cfg(unix)]
impl IsolatedTmux {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tmux.sock");
        let server = Self { _dir: dir, socket };
        server.run(&["-f", "/dev/null", "new-session", "-d", "-s", "fixture", "-x", "120", "-y", "30", "-e", "HANGAR_PROBE=kept", "cat"]);
        server
    }
    fn run(&self, args: &[&str]) -> String {
        let output = Command::new("tmux").arg("-u").arg("-S").arg(&self.socket).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }
    fn clients(&self) -> String { self.run(&["list-clients", "-t", "=fixture", "-F", "#{client_pid}\t#{client_control_mode}\t#{client_tty}"]) }
    fn pool(&self) -> TerminalPool { TerminalPool::with_program("tmux", Some(self.socket.clone()), Limits::default()) }
    async fn await_text(&self, pool: &TerminalPool, r: &CaptureRequest, text: &str) -> String {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let value = pool.capture(r.clone()).await.unwrap().text;
                if value.contains(text) { break value; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap()
    }
}
#[cfg(unix)]
impl Drop for IsolatedTmux { fn drop(&mut self) { let _ = Command::new("tmux").arg("-S").arg(&self.socket).arg("kill-server").output(); } }

#[cfg(unix)]
#[tokio::test]
async fn isolated_clients_share_pid_and_last_release_reaps_only_observer() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    let before = server.run(&["display-message", "-p", "-t", "=fixture:", "#{pane_width}x#{pane_height}"]);
    let env = server.run(&["show-environment", "-t", "=fixture", "HANGAR_PROBE"]);
    let r = request("state");
    let captured = pool.capture(r.clone()).await.unwrap();
    assert_eq!(captured.binding, "conversation-a");
    assert_eq!(captured.started, 42.5);
    let clients = server.clients();
    assert_eq!(clients.lines().count(), 1);
    assert!(clients.ends_with("\t1\t\n"), "{clients}");
    pool.acquire(request("preview")).await.unwrap();
    pool.capture(r).await.unwrap();
    assert_eq!(server.clients(), clients);
    pool.release("state").await.unwrap();
    assert_eq!(server.clients(), clients);
    pool.release("preview").await.unwrap();
    assert_eq!(server.clients(), "");
    assert_eq!(server.run(&["display-message", "-p", "-t", "=fixture:", "#{pane_width}x#{pane_height}"]), before);
    assert_eq!(server.run(&["show-environment", "-t", "=fixture", "HANGAR_PROBE"]), env);
    pool.release("unknown").await.unwrap();
    assert_eq!(server.clients(), "");
}

#[cfg(unix)]
#[tokio::test]
async fn canonical_capture_matches_tmux_history_ansi_join_blanks_and_literal_percent() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    server.run(&["respawn-pane", "-k", "-t", "=fixture:", "python3 -u -c 'import sys,time; print(\"\\n\".join(\"history-%d\"%i for i in range(40))); print(\"%begin exemplo literal\"); print(\"\\x1b[31molá\\x1b[0m\"); print(\"wrap-\"+\"x\"*250); print(\"\\nblank\\n\"); time.sleep(30)'"]);
    let mut r = request("state");
    server.await_text(&pool, &r, "blank").await;
    for (colors, join) in [(false, false), (true, false), (false, true), (true, true)] {
        r.colors = colors; r.join = join;
        let result = pool.capture(r.clone()).await.unwrap();
        let mut args = vec!["capture-pane", "-p", "-t", "=fixture:", "-S", "-200"];
        if colors { args.push("-e"); }
        if join { args.push("-J"); }
        assert_eq!(result.text, server.run(&args));
        assert!(result.text.contains("%begin exemplo literal"));
        assert!(result.text.contains("history-0"));
    }
    server.run(&["resize-window", "-t", "=fixture:", "-x", "90", "-y", "22"]);
    assert_eq!(pool.capture(r).await.unwrap().text, server.run(&["capture-pane", "-p", "-e", "-J", "-t", "=fixture:", "-S", "-200"]));
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_requests_and_exact_target_changes_never_reuse_old_capture() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    let mut r = request("state");
    let original = server.run(&["display-message", "-p", "-t", "=fixture:", "#{pane_id}"]).trim().to_owned();
    r.target = original.clone();
    pool.capture(r.clone()).await.unwrap();
    server.run(&["split-window", "-d", "-t", "=fixture:", "cat"]);
    server.run(&["kill-pane", "-t", &original]);
    assert!(pool.capture(r.clone()).await.is_err());
    for invalid in ["=fixture:\nkill-server", "$(kill-server)", "=fixture:;kill-server"] {
        r.target = invalid.into();
        assert!(pool.capture(r.clone()).await.is_err());
    }
    r.target = "=fixture:".into(); r.started = f64::NAN;
    assert!(pool.capture(r).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn idle_expiry_is_not_postponed_by_continuous_output() {
    let server = IsolatedTmux::new();
    server.run(&["respawn-pane", "-k", "-t", "=fixture:", "python3 -u -c 'import time; exec(\"while True:\\n print(\\\"stream\\\"); time.sleep(.01)\")'"]);
    let limits = Limits { lease: Duration::from_millis(200), ..Limits::default() };
    let pool = TerminalPool::with_program("tmux", Some(server.socket.clone()), limits);
    pool.acquire(request("state")).await.unwrap();
    assert_eq!(server.clients().lines().count(), 1);
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert_eq!(server.clients(), "");
    assert!(server.run(&["display-message", "-p", "-t", "=fixture:", "#{pane_pid}"]).trim().parse::<u32>().is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn child_startup_timeout_and_eof_are_errors_and_reaped() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake");
    for body in ["#!/bin/sh\nexec sleep 30\n", "#!/bin/sh\nexit 0\n"] {
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let limits = Limits { startup: Duration::from_millis(100), command: Duration::from_millis(100), ..Limits::default() };
        let pool = TerminalPool::with_program(script.clone(), None, limits);
        assert!(tokio::time::timeout(Duration::from_secs(2), pool.capture(request("state"))).await.unwrap().is_err());
    }
}

#[cfg(unix)]
fn fake_observer(mode: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("observer");
    let pid = dir.path().join("pid");
    let log = dir.path().join("commands");
    std::fs::write(dir.path().join("mode"), mode).unwrap();
    // O executável fica só para leitura; criá-lo durante outros spawns pode causar ETXTBSY.
    std::os::unix::fs::symlink(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/terminal_observer.py"), &program).unwrap();
    (dir, program, pid, log)
}

#[cfg(unix)]
fn process_exists(pid: &str) -> bool {
    Command::new("kill").args(["-0", pid]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap().success()
}

#[cfg(unix)]
#[tokio::test]
async fn command_timeout_active_eof_protocol_error_and_bad_utf8_discard_observer() {
    for mode in ["timeout", "eof", "utf8", "error"] {
        let (_dir, program, pid_file, _) = fake_observer(mode);
        let limits = Limits { startup: Duration::from_millis(500), command: Duration::from_millis(150), ..Limits::default() };
        let pool = TerminalPool::with_program(program, None, limits);
        pool.acquire(request("state")).await.unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        assert!(process_exists(&pid));
        let error = pool.capture(request("state")).await.unwrap_err();
        assert!(matches!(error.0, "terminal command timeout" | "terminal observer EOF" | "invalid frame UTF-8" | "tmux command failed"), "{mode}: {error}");
        tokio::time::timeout(Duration::from_secs(2), async {
            while process_exists(&pid) { tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ansi_queries_do_not_write_to_terminal_and_final_release_reaps_child() {
    let (_dir, program, pid_file, log) = fake_observer("normal");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    assert_eq!(pool.capture(request("state")).await.unwrap().text, "ready\n\n\n\n");
    let pid = std::fs::read_to_string(pid_file).unwrap();
    pool.release("state").await.unwrap();
    assert!(!process_exists(&pid));
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(commands.lines().flat_map(|line| line.split(" ; ")).all(|command| command.starts_with("display-message -p -t ") || command.starts_with("capture-pane -p ") || command.starts_with("display-message -p HG_")));
    assert!(!commands.contains('\x1b'));
}

#[cfg(unix)]
#[tokio::test]
async fn persistent_observer_failures_do_not_spawn_on_every_capture_or_acquire() {
    for mode in ["static-error", "eof", "timeout"] {
        let (dir, program, _, _) = fake_observer(mode);
        let limits = Limits { startup: Duration::from_millis(500), command: Duration::from_millis(80), ..Limits::default() };
        let pool = TerminalPool::with_program(program, None, limits);
        assert!(pool.capture(request("state")).await.is_err(), "{mode}");
        for _ in 0..4 {
            assert!(pool.capture(request("state")).await.is_err());
            assert!(pool.acquire(request("preview")).await.is_err());
        }
        assert_eq!(std::fs::read_to_string(dir.path().join("spawns")).unwrap().lines().count(), 1, "{mode}: falha repetida abriu outro processo");
        let mut another = request("other"); another.name = "another".into(); another.target = "=another:".into();
        assert!(pool.capture(another).await.is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("spawns")).unwrap().lines().count(), 2, "{mode}: outra sessão deve ter tentativa independente");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn literal_command_markers_work_without_tmux_34_flag() {
    let (_dir, program, _, log) = fake_observer("legacy");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    assert_eq!(pool.capture(request("state")).await.unwrap().text, "ready\n\n\n\n");
    let commands = std::fs::read_to_string(log).unwrap();
    assert!(!commands.contains("display-message -p -l "));
    assert!(commands.contains("HG_START_") && commands.contains("HG_END_"));
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn observer_retry_pause_grows_expires_and_resets_after_success() {
    let (dir, program, _, _) = fake_observer("static-error");
    let limits = Limits { retry: Duration::from_millis(100), ..Limits::default() };
    let pool = TerminalPool::with_program(program, None, limits);
    let count = || std::fs::read_to_string(dir.path().join("spawns")).unwrap().lines().count();
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 1);
    tokio::time::sleep(Duration::from_millis(130)).await;
    assert!(pool.acquire(request("preview")).await.is_err());
    assert_eq!(count(), 2, "prazo expirado permite nova tentativa");
    tokio::time::sleep(Duration::from_millis(130)).await;
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 2, "segunda falha deve esperar o dobro");
    tokio::time::sleep(Duration::from_millis(130)).await;
    std::fs::write(dir.path().join("mode"), "normal").unwrap();
    assert_eq!(pool.capture(request("state")).await.unwrap().text, "ready\n\n\n\n");
    assert_eq!(count(), 3);
    pool.release("state").await.unwrap();
    std::fs::write(dir.path().join("mode"), "static-error").unwrap();
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 4);
    tokio::time::sleep(Duration::from_millis(130)).await;
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 5, "sucesso deve repor a pausa inicial");
}

#[cfg(unix)]
#[tokio::test]
async fn successful_acquire_preserves_capture_failure_backoff_until_valid_capture() {
    let (dir, program, _, _) = fake_observer("error");
    let limits = Limits { retry: Duration::from_millis(250), ..Limits::default() };
    let pool = TerminalPool::with_program(program, None, limits);
    let count = || std::fs::read_to_string(dir.path().join("spawns")).unwrap().lines().count();
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    pool.acquire(request("preview")).await.unwrap();
    assert_eq!(count(), 2);
    assert!(pool.capture(request("state")).await.is_err());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(pool.acquire(request("preview")).await.is_err());
    assert_eq!(count(), 2, "acquire bem-sucedido não deve repor a pausa inicial");
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(dir.path().join("mode"), "normal").unwrap();
    assert_eq!(pool.capture(request("state")).await.unwrap().text, "ready\n\n\n\n");
    assert_eq!(count(), 3);
    pool.release("state").await.unwrap();
    std::fs::write(dir.path().join("mode"), "error").unwrap();
    assert!(pool.capture(request("state")).await.is_err());
    assert_eq!(count(), 4);
    tokio::time::sleep(Duration::from_millis(300)).await;
    pool.acquire(request("preview")).await.unwrap();
    assert_eq!(count(), 5, "captura válida deve repor a pausa inicial");
    pool.release("preview").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn consumer_rebind_waits_for_old_release_without_blocking_another_session() {
    let (dir, program, _, _) = fake_observer("hold");
    let limits = Limits { startup: Duration::from_millis(500), command: Duration::from_millis(700), ..Limits::default() };
    let pool = TerminalPool::with_program(program, None, limits);
    pool.acquire(request("state")).await.unwrap();
    let old = pool.clone();
    let capture = tokio::spawn(async move { old.capture(request("state")).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dir.path().join("blocked").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.unwrap();
    let migrating = pool.clone();
    let migration = tokio::spawn(async move {
        let mut r = request("state"); r.binding = "conversation-b".into();
        migrating.acquire(r).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let releasing = pool.clone();
    let release = tokio::spawn(async move { releasing.release("state").await });
    let mut other = request("other"); other.name = "another".into(); other.target = "=another:".into();
    tokio::time::timeout(Duration::from_millis(250), async {
        pool.acquire(other.clone()).await.unwrap();
        assert_eq!(pool.capture(other).await.unwrap().text, "ready\n\n\n\n");
        pool.release("other").await.unwrap();
    }).await.expect("release da sessão A não pode prender acquire/capture/release da B");
    std::fs::write(dir.path().join("resume"), "yes").unwrap();
    capture.await.unwrap().unwrap();
    migration.await.unwrap().unwrap();
    release.await.unwrap().unwrap();
    let pids = std::fs::read_to_string(dir.path().join("spawns")).unwrap();
    for pid in pids.lines() { assert!(!process_exists(pid), "release concorrente deve alcançar o actor novo: {pid}"); }
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_consumer_locks_are_bounded_and_reusable_after_release() {
    let (_dir, program, pid_file, _) = fake_observer("normal");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    for n in 0..256 { pool.acquire(request(&format!("consumer-{n}"))).await.unwrap(); }
    pool.release("not-active").await.unwrap();
    assert_eq!(pool.acquire(request("overflow")).await.unwrap_err().0, "too many terminal consumers");
    pool.release("consumer-0").await.unwrap();
    pool.acquire(request("overflow")).await.unwrap();
    for n in 1..256 { pool.release(&format!("consumer-{n}")).await.unwrap(); }
    pool.release("overflow").await.unwrap();
    assert!(!process_exists(&std::fs::read_to_string(pid_file).unwrap()));
    for n in 0..300 { pool.release(&format!("absent-{n}")).await.unwrap(); }
    pool.acquire(request("fresh")).await.unwrap();
    pool.release("fresh").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_actor_queue_is_bounded_while_capture_is_blocked() {
    let (dir, program, _, _) = fake_observer("hold");
    let limits = Limits { command: Duration::from_secs(2), ..Limits::default() };
    let pool = TerminalPool::with_program(program, None, limits);
    pool.acquire(request("state")).await.unwrap();
    let old = pool.clone();
    let capture = tokio::spawn(async move { old.capture(request("state")).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !dir.path().join("blocked").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.unwrap();
    let (completed, mut results) = tokio::sync::mpsc::channel(64);
    let mut tasks = Vec::new();
    for n in 0..64 {
        let (queued, completed) = (pool.clone(), completed.clone());
        tasks.push(tokio::spawn(async move {
            completed.send(queued.capture(request(&format!("queued-{n}"))).await).await.unwrap();
        }));
    }
    tokio::time::timeout(Duration::from_millis(250), async {
        for _ in 0..32 { assert_eq!(results.recv().await.unwrap().unwrap_err().0, "terminal queue full"); }
    }).await.expect("fila cheia deve recusar sem aguardar o comando preso");
    std::fs::write(dir.path().join("resume"), "yes").unwrap();
    capture.await.unwrap().unwrap();
    for _ in 0..32 { results.recv().await.unwrap().unwrap(); }
    for task in tasks { task.await.unwrap(); }
    for n in 0..64 { pool.release(&format!("queued-{n}")).await.unwrap(); }
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn unsolicited_queued_frames_never_supply_a_successful_capture() {
    let (_dir, program, _, _) = fake_observer("prefill");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    assert!(pool.capture(request("state")).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn late_fragmented_unsolicited_frame_never_serves_the_next_request() {
    let (_dir, program, _, _) = fake_observer("late");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    assert!(pool.capture(request("state")).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn capture_waits_for_end_marker_and_rejects_missing_or_wrong_end() {
    for mode in ["missing-end", "wrong-end", "late-end"] {
        let (dir, program, _, _) = fake_observer(mode);
        let limits = Limits { command: Duration::from_millis(250), ..Limits::default() };
        let pool = TerminalPool::with_program(program, None, limits);
        let result = pool.capture(request("state")).await;
        if mode == "late-end" {
            assert_eq!(result.unwrap().text, "ready\n\n\n\n");
            assert!(dir.path().join("end-sent").exists());
            pool.release("state").await.unwrap();
        } else {
            assert!(matches!(result.unwrap_err().0, "terminal command timeout" | "terminal command marker mismatch"));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn exact_full_line_without_newline_preserves_pending_wrap_cursor() {
    let server = IsolatedTmux::new();
    server.run(&["resize-window", "-t", "=fixture:", "-x", "80", "-y", "30"]);
    server.run(&["respawn-pane", "-k", "-t", "=fixture:", "python3 -u -c 'import sys,time; sys.stdout.write(\"x\"*80); sys.stdout.flush(); time.sleep(30)'"]);
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.run(&["display-message", "-p", "-t", "=fixture:", "#{pane_width}\t#{cursor_x}"]) != "80\t80\n" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let pool = server.pool();
    let text = pool.capture(request("state")).await.unwrap().text;
    assert_eq!(text, server.run(&["capture-pane", "-p", "-t", "=fixture:", "-S", "-200"]));
    assert!(text.starts_with(&format!("{}\n", "x".repeat(80))));
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn consumer_binding_change_reaps_old_actor_before_new_capture() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    let mut r = request("state");
    pool.capture(r.clone()).await.unwrap();
    let old_pid = server.clients().split('\t').next().unwrap().to_owned();
    r.binding = "conversation-b".into();
    let result = pool.capture(r).await.unwrap();
    assert_eq!(result.binding, "conversation-b");
    let new_pid = server.clients().split('\t').next().unwrap().to_owned();
    assert_ne!(old_pid, new_pid);
    #[cfg(unix)]
    assert!(!process_exists(&old_pid));
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn another_session_pane_and_changed_active_target_fail_without_substitution() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    let r = request("state");
    pool.capture(r.clone()).await.unwrap();
    server.run(&["split-window", "-t", "=fixture:", "cat"]);
    assert_eq!(pool.capture(r).await.unwrap_err().0, "terminal target changed");
    server.run(&["new-session", "-d", "-s", "other", "cat"]);
    let id = server.run(&["display-message", "-p", "-t", "=other:", "#{pane_id}"]).trim().to_owned();
    let mut r = request("other"); r.target = id;
    assert_eq!(pool.capture(r).await.unwrap_err().0, "invalid terminal metadata");
}

#[test]
fn invalid_notification_utf8_is_error() {
    let mut parser = ControlParser::default();
    assert!(parser.push(b"%notice \xff\n").is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_never_launches_tmux_or_psmux() {
    let pool = TerminalPool::with_program("does-not-exist", None, Limits::default());
    assert_eq!(pool.capture(request("state")).await.unwrap_err().0, "terminal control unavailable");
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn actual_alternate_screen_capture_and_numeric_session_keep_exact_target() {
    let server = IsolatedTmux::new();
    server.run(&["respawn-pane", "-k", "-t", "=fixture:", "python3 -u -c 'import sys,time; sys.stdout.write(\"main\\x1b[?1049h\\x1b[2J\\x1b[HALTERNATE\"); sys.stdout.flush(); time.sleep(30)'"]);
    let pool = server.pool();
    let r = request("state");
    server.await_text(&pool, &r, "ALTERNATE").await;
    assert_eq!(server.run(&["display-message", "-p", "-t", "=fixture:", "#{alternate_on}"]), "1\n");
    assert_eq!(pool.capture(r).await.unwrap().text, server.run(&["capture-pane", "-p", "-t", "=fixture:", "-S", "-200"]));
    server.run(&["new-session", "-d", "-s", "0", "python3 -u -c 'import time; print(\"NUMERIC_SESSION\"); time.sleep(30)'"]);
    let mut numeric = request("numeric"); numeric.name = "0".into(); numeric.target = "=0:".into();
    assert!(server.await_text(&pool, &numeric, "NUMERIC_SESSION").await.contains("NUMERIC_SESSION"));
    pool.release("numeric").await.unwrap();
    pool.release("state").await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn each_round_has_one_canonical_capture_and_scope_proof() {
    let (_dir, program, _, log) = fake_observer("normal");
    let pool = TerminalPool::with_program(program, None, Limits::default());
    pool.acquire(request("state")).await.unwrap();
    let before = std::fs::read_to_string(&log).unwrap();
    let captured = pool.capture(request("state")).await.unwrap();
    assert_eq!(captured.text, "ready\n\n\n\n");
    let after = std::fs::read_to_string(&log).unwrap();
    let round = &after[before.len()..];
    assert_eq!(round.lines().count(), 2, "scope proof and one canonical capture, without grade checkpoints");
    let commands: Vec<_> = round.lines().flat_map(|line| line.split(" ; ")).collect();
    assert_eq!(commands.len(), 6);
    assert_eq!(commands.iter().filter(|cmd| cmd.starts_with("capture-pane ")).count(), 1);
    assert!(!commands.iter().any(|cmd| cmd.contains("-S 0")));
    pool.release("state").await.unwrap();
}

#[test]
fn terminal_capture_does_not_compile_a_second_terminal_grid() {
    assert!(!include_str!("../Cargo.toml").contains("alacritty_terminal"));
}

#[cfg(unix)]
#[tokio::test]
async fn read_only_control_observer_never_subscribes_to_pane_output() {
    let server = IsolatedTmux::new();
    let pool = server.pool();
    pool.acquire(request("state")).await.unwrap();
    let flags = server.run(&["list-clients", "-t", "=fixture", "-F", "#{client_flags}"]);
    assert!(flags.contains("read-only") && flags.contains("ignore-size") && flags.contains("no-output"), "{flags}");
    pool.release("state").await.unwrap();
}
