use hangar_boot::life::{begin, end, end_with, health, last_errors, mark_running, reduce_error, running_pid, unclean_exits, Ended, Mark, Previous, Stop, FILE};
use hangar_boot::notify::Notifier;
use proptest::prelude::*;

const T: &str = "2026-11-16T10:00:00-03:00";
/// O instante das paradas, em segundos desde a época.
const AT: u64 = 1_800_000_000;

/// Nenhum outro servidor de pé: a marca que ficou é sempre de uma vida que acabou.
fn dead(_: u32, _: f64) -> bool { false }

/// Uma vida inteira: a conferência da subida e a marca de pé quando pronta. O nascimento é o pid.
fn boot(dir: &std::path::Path, pid: u32, now: &str) -> hangar_boot::life::Begin {
    let begin = begin(dir, now, dead).unwrap();
    assert_eq!(mark_running(dir, pid, Some(f64::from(pid)), None, now, dead).unwrap(), Mark::Marked);
    begin
}

fn life(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap()
}

#[test]
fn b24_clean_stop_is_not_counted() {
    let dir = tempfile::tempdir().unwrap();
    let first = boot(dir.path(), 1, T);
    assert!(!first.unclean && first.unclean_exits == 0 && !first.reset);
    assert_eq!(end(dir.path(), 1, AT).unwrap(), Ended::Cleared);
    assert!(!boot(dir.path(), 2, T).unclean);
    assert_eq!(unclean_exits(dir.path()).unwrap(), 0);
}

#[test]
fn b24_life_without_clean_stop_counts_on_the_next_boot() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    let second = boot(dir.path(), 2, "2026-11-16T10:05:00-03:00");
    assert!(second.unclean);
    assert_eq!(second.unclean_exits, 1);
    let third = boot(dir.path(), 3, "2026-11-16T10:06:00-03:00");
    assert_eq!(third.unclean_exits, 2);
    assert_eq!(end(dir.path(), 3, AT).unwrap(), Ended::Cleared);
    // A contagem sobrevive à parada limpa: é o histórico para a tela de status.
    assert_eq!(unclean_exits(dir.path()).unwrap(), 2);
    assert_eq!(life(dir.path())["last_unclean"], "2026-11-16T10:06:00-03:00");
    assert_eq!(life(dir.path())["running"], false);
}

#[test]
fn b24_a_boot_that_never_got_ready_is_not_a_fall() {
    // Porta tomada, identidade indisponível: a vida sai antes de se marcar de pé, e o systemd tenta de novo.
    let dir = tempfile::tempdir().unwrap();
    for _ in 0..3 { assert!(!begin(dir.path(), T, dead).unwrap().unclean); }
    assert_eq!(unclean_exits(dir.path()).unwrap(), 0);
}

#[test]
fn b24_end_only_clears_its_own_life() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    // Outra vida parando não apaga a marca de pé desta.
    assert_eq!(end(dir.path(), 2, AT).unwrap(), Ended::OtherLife);
    assert_eq!(life(dir.path())["running"], true);
    assert!(boot(dir.path(), 3, T).unclean);
}

/// Segundo servidor no mesmo HOME (binário de branch em teste) com o primeiro de pé: não conta
/// queda, não toma a marca e, ao parar, não apaga a do outro. O mesmo pid com outro nascimento é
/// outra vida: aí a marca que ficou é queda.
#[test]
fn b24_a_live_server_keeps_its_mark_from_a_second_one() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    let first_alive = |pid: u32, birth: f64| pid == 1 && birth == 1.0;
    let second = begin(dir.path(), T, first_alive).unwrap();
    assert_eq!((second.unclean, second.unclean_exits, second.held_by), (false, 0, Some(1)));
    assert_eq!(mark_running(dir.path(), 2, Some(2.0), None, T, first_alive).unwrap(), Mark::Held(1));
    assert_eq!(end(dir.path(), 2, AT).unwrap(), Ended::OtherLife);
    assert_eq!((life(dir.path())["pid"].as_u64(), life(dir.path())["running"].as_bool()), (Some(1), Some(true)));
    let reborn = begin(dir.path(), T, |pid, birth| pid == 1 && birth == 9.0).unwrap();
    assert!(reborn.unclean && reborn.held_by.is_none());
    assert_eq!(reborn.unclean_exits, 1);
}

/// Marca antiga, sem nascimento: o pid sozinho não prova que é o mesmo processo, e a queda conta.
#[test]
fn b24_a_mark_without_birth_is_counted_even_with_the_pid_alive() {
    let dir = tempfile::tempdir().unwrap();
    begin(dir.path(), T, dead).unwrap();
    mark_running(dir.path(), 1, None, None, T, dead).unwrap();
    assert!(life(dir.path()).get("birth").is_none());
    assert!(begin(dir.path(), T, |_, _| true).unwrap().unclean);
}

/// O lançador do Windows só confere a saúde do servidor que ele abriu, de pé e com o endereço
/// gravado: a versão que não grava o endereço, outro filho ou a vida parada não são conferidos.
#[test]
fn the_launcher_reads_the_health_address_only_of_its_own_running_server() {
    let dir = tempfile::tempdir().unwrap();
    let address: std::net::SocketAddr = "127.0.0.1:8765".parse().unwrap();
    assert_eq!(health(dir.path(), 1), None, "sem arquivo");
    boot(dir.path(), 1, T);
    assert_eq!(health(dir.path(), 1), None, "versão que não grava o endereço");
    assert_eq!(mark_running(dir.path(), 1, Some(1.0), Some(address), T, dead).unwrap(), Mark::Marked);
    assert_eq!(health(dir.path(), 1), Some(address));
    assert_eq!(health(dir.path(), 2), None, "outro filho");
    assert_eq!(end(dir.path(), 1, AT).unwrap(), Ended::Cleared);
    assert_eq!(health(dir.path(), 1), None, "vida parada");
    boot(dir.path(), 3, T);
    assert_eq!(health(dir.path(), 3), None, "a vida nova não herda o endereço da anterior");
}

#[test]
fn b24_unreadable_life_file_restarts_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"{torto").unwrap();
    let got = begin(dir.path(), T, dead).unwrap();
    assert!(got.reset && !got.unclean && got.unclean_exits == 0);
    std::fs::write(dir.path().join(FILE), b"{torto").unwrap();
    assert_eq!(end(dir.path(), 1, AT).unwrap(), Ended::Reset);
    assert_eq!(life(dir.path())["running"], false, "arquivo ilegível na parada vira parado, nunca fica de pé");
}

/// A parada grava a causa, a falha e o instante; a subida seguinte as entrega uma vez só.
#[test]
fn exit_stop_cause_reaches_the_next_boot_once() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    assert_eq!(running_pid(dir.path()).unwrap(), Some(1));
    assert_eq!(end_with(dir.path(), 1, Some("sigterm"), false, AT).unwrap(), Ended::Cleared);
    assert_eq!(running_pid(dir.path()).unwrap(), None);
    let next = begin(dir.path(), T, dead).unwrap();
    assert_eq!(next.pid, Some(1));
    assert_eq!(next.previous, Some(Previous { pid: Some(1), started: Some(T.into()), unclean: false,
        stop: Some(Stop { cause: Some("sigterm".into()), failed: false, at: Some(AT) }) }));
    assert!(life(dir.path()).get("stop").is_none(), "a subida consome a parada");
    // Subida que não ficou pronta não relê a mesma vida.
    assert_eq!(begin(dir.path(), T, dead).unwrap().previous, None);
}

/// `serve_failed` grava a parada como falha, sem contar queda pela marca.
#[test]
fn exit_failed_stop_is_kept_as_failure() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    end_with(dir.path(), 1, Some("serve_failed"), true, AT).unwrap();
    let next = boot(dir.path(), 2, T);
    assert!(!next.unclean);
    assert_eq!(next.previous.unwrap().stop, Some(Stop { cause: Some("serve_failed".into()), failed: true, at: Some(AT) }));
    assert!(life(dir.path()).get("stop").is_none(), "a marca de pé nova não carrega a parada da anterior");
}

/// O lançador fechando depois não apaga a causa que o servidor gravou; fechando sozinho, a parada fica sem causa.
#[test]
fn exit_launcher_end_keeps_the_server_cause() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 1, T);
    end_with(dir.path(), 1, Some("ctrl_close"), false, AT).unwrap();
    assert_eq!(end(dir.path(), 1, AT + 5).unwrap(), Ended::Cleared);
    let server = begin(dir.path(), T, dead).unwrap().previous.unwrap().stop.unwrap();
    assert_eq!((server.cause.as_deref(), server.at), (Some("ctrl_close"), Some(AT)), "a parada do servidor fica inteira");
    boot(dir.path(), 2, T);
    end(dir.path(), 2, AT + 60).unwrap();
    assert_eq!(begin(dir.path(), T, dead).unwrap().previous.unwrap().stop, Some(Stop { cause: None, failed: false, at: Some(AT + 60) }));
}

/// Parada gravada por versão antiga, sem o instante: lida sem ele.
#[test]
fn exit_stop_from_an_older_version_has_no_instant() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), r#"{"running":false,"pid":3,"stop":{"pid":3,"cause":"sigterm","failed":false}}"#).unwrap();
    assert_eq!(begin(dir.path(), T, dead).unwrap().previous.unwrap().stop, Some(Stop { cause: Some("sigterm".into()), failed: false, at: None }));
}

/// Parada de versão que não gravava a causa: sem vida anterior a classificar, mas com o pid dela
/// para a marca de intenção conferir.
#[test]
fn exit_life_without_stop_still_gives_its_pid() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), r#"{"running":false,"unclean_exits":0,"pid":9}"#).unwrap();
    let next = begin(dir.path(), T, dead).unwrap();
    assert_eq!((next.previous, next.pid), (None, Some(9)));
}

/// Queda: a vida anterior vem sem parada e com o pid dela, que a marca de intenção confere.
#[test]
fn exit_unclean_life_comes_with_its_pid() {
    let dir = tempfile::tempdir().unwrap();
    boot(dir.path(), 7, T);
    let next = begin(dir.path(), T, dead).unwrap();
    assert_eq!(next.previous, Some(Previous { pid: Some(7), started: Some(T.into()), unclean: true, stop: None }));
    // Parada de outra vida (versão antiga que não a apagou) não vale.
    std::fs::write(dir.path().join(FILE), r#"{"running":false,"pid":3,"stop":{"pid":2,"cause":"x","failed":false}}"#).unwrap();
    assert_eq!(begin(dir.path(), T, dead).unwrap().previous, None);
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
    assert_eq!(ours.rejected, None);
    let other = Notifier::from_lookup(env(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "60000000"), ("WATCHDOG_PID", "7")]), 42);
    assert_eq!((other.watchdog, other.rejected), (None, Some("watchdog_pid_other")));
    let outside = Notifier::from_lookup(env(&[]), 42);
    assert_eq!((outside.watchdog, outside.rejected), (None, None));
    outside.send("READY=1").unwrap();
}

/// Watchdog pedido e recusado diz o código: sem ele, o systemd mata pelo prazo sem causa no log.
#[test]
fn p2_a_rejected_watchdog_says_why() {
    let env = |pairs: &'static [(&'static str, &'static str)]| move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string());
    let rejected = |pairs: &'static [(&'static str, &'static str)]| Notifier::from_lookup(env(pairs), 42).rejected;
    assert_eq!(rejected(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "trinta")]), Some("watchdog_usec_invalid"));
    assert_eq!(rejected(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "60000000"), ("WATCHDOG_PID", "x")]), Some("watchdog_pid_invalid"));
    assert_eq!(rejected(&[("WATCHDOG_USEC", "60000000")]), Some("watchdog_without_socket"));
    assert_eq!(rejected(&[("NOTIFY_SOCKET", "/run/x"), ("WATCHDOG_USEC", "0")]), None, "zero é watchdog desligado");
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
