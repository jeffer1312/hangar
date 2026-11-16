use hangar_boot::life::{begin, end, last_errors, reduce_error, unclean_exits, FILE};
use hangar_boot::notify::Notifier;
use proptest::prelude::*;

#[test]
fn b24_clean_stop_is_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let first = begin(dir.path(), 1, "2026-11-16T10:00:00-03:00").unwrap();
    assert!(!first.unclean && first.unclean_exits == 0 && !first.reset);
    end(dir.path()).unwrap();
    assert!(!begin(dir.path(), 2, "2026-11-16T10:01:00-03:00").unwrap().unclean);
    assert_eq!(unclean_exits(dir.path()).unwrap(), 0);
}

#[test]
fn b24_life_without_clean_stop_counts_on_the_next_boot() {
    let dir = tempfile::tempdir().unwrap();
    begin(dir.path(), 1, "2026-11-16T10:00:00-03:00").unwrap();
    let second = begin(dir.path(), 2, "2026-11-16T10:05:00-03:00").unwrap();
    assert!(second.unclean);
    assert_eq!(second.unclean_exits, 1);
    let third = begin(dir.path(), 3, "2026-11-16T10:06:00-03:00").unwrap();
    assert_eq!(third.unclean_exits, 2);
    end(dir.path()).unwrap();
    // A contagem sobrevive à parada limpa: é o histórico para a tela de status.
    assert_eq!(unclean_exits(dir.path()).unwrap(), 2);
    let life: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(life["last_unclean"], "2026-11-16T10:06:00-03:00");
    assert_eq!(life["running"], false);
}

#[test]
fn b24_unreadable_life_file_restarts_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"{torto").unwrap();
    let got = begin(dir.path(), 1, "2026-11-16T10:00:00-03:00").unwrap();
    assert!(got.reset && !got.unclean && got.unclean_exits == 0);
}

#[test]
fn b24_crash_output_keeps_codes_and_drops_the_message() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("hangar-server.log");
    std::fs::write(&log, "2026-11-03T10:55:00.123Z  INFO de pé\n\
        2026-11-03T10:55:01.123Z  WARN hangar_server::loops: claude -p falhou: conversa secreta code=loop_refine_failed\n\
        2026-11-03T10:55:02.123Z ERROR alvo: caiu detail=\"conversa secreta\" panic=true\n").unwrap();
    let saida = last_errors(&log, 3);
    assert_eq!(saida, "10:55:01 code=loop_refine_failed | 10:55:02 panic=true");
    assert!(!saida.contains("conversa"));
    assert!(last_errors(&dir.path().join("nada.log"), 3).starts_with("log ilegível"));
}

proptest! {
    #[test]
    fn reduce_never_panics(line in "\\PC*") {
        let _ = reduce_error(&line);
    }
}

#[test]
fn p2_watchdog_interval_is_half_and_only_for_this_pid() {
    let env = |pairs: &'static [(&'static str, &'static str)]| move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string());
    let ours = Notifier::from_lookup(env(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "60000000"), ("WATCHDOG_PID", "42")]), 42);
    assert_eq!(ours.watchdog, Some(std::time::Duration::from_secs(30)));
    let other = Notifier::from_lookup(env(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "60000000"), ("WATCHDOG_PID", "7")]), 42);
    assert_eq!(other.watchdog, None);
    let outside = Notifier::from_lookup(env(&[]), 42);
    assert_eq!(outside.watchdog, None);
    outside.send("READY=1").unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn p2_notify_reaches_the_systemd_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notify.sock");
    let socket = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
    let path_text = path.to_string_lossy().into_owned();
    let notifier = Notifier::from_lookup(move |key| (key == "NOTIFY_SOCKET").then(|| path_text.clone()), 1);
    notifier.send("READY=1").unwrap();
    let mut buffer = [0u8; 64];
    let read = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..read], b"READY=1");
}
