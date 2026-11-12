use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Instant;

enum Reply { Answer(&'static str), Silent, Drip }

/// Servidor de sessão falso: confere a chave e responde ao `list-panes -s -F` como o psmux.
fn server(key: &'static str, reply: Reply) -> (u16, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut auth, mut cmd) = (String::new(), String::new());
        reader.read_line(&mut auth).unwrap();
        let mut out = stream;
        match reply {
            Reply::Silent => { std::thread::sleep(Duration::from_millis(400)); return vec![auth]; }
            _ if auth.trim() != format!("AUTH {key}") => { out.write_all(b"ERROR: Invalid session key\n").unwrap(); return vec![auth]; }
            _ => {}
        }
        out.write_all(b"OK\n").unwrap();
        reader.read_line(&mut cmd).unwrap();
        match reply {
            Reply::Answer(text) => out.write_all(text.as_bytes()).unwrap(),
            // Um byte por vez, sem fim de linha, cada um antes do prazo de leitura vencer.
            _ => for _ in 0..40 { if out.write_all(b"x").is_err() { break; } std::thread::sleep(Duration::from_millis(30)); },
        }
        vec![auth, cmd]
    });
    (port, handle)
}

fn register(dir: &Path, base: &str, port: u16, key: &str) {
    std::fs::write(dir.join(format!("{base}.port")), format!("{port}\n")).unwrap();
    std::fs::write(dir.join(format!("{base}.key")), format!("{key}\n")).unwrap();
}

fn soon(ms: u64) -> Instant { Instant::now() + Duration::from_millis(ms) }

#[test]
fn reads_default_namespace_servers_like_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k1", Reply::Answer("main\t1\t42\tC:\\a b\t%1\n\n"));
    register(dir.path(), "main", port, "k1");
    // Outro `-L` e a sessão pré-aberta não são do espaço padrão.
    register(dir.path(), "ns__other", 1, "x");
    register(dir.path(), "__warm__", 1, "x");
    let listing = list_panes(dir.path(), "#{session_name}\t#{pane_active}", soon(2000)).unwrap();
    assert_eq!(listing.text, "main\t1\t42\tC:\\a b\t%1\n");
    assert_eq!(listing.servers.len(), 1);
    let seen = seen.join().unwrap();
    assert_eq!(seen[0], "AUTH k1\n");
    assert_eq!(seen[1], "list-panes -s -F \"#{session_name}\t#{pane_active}\"\n");
}

#[test]
fn the_server_identity_changes_with_port_or_key() {
    let dir = tempfile::tempdir().unwrap();
    let (port, first) = server("k1", Reply::Answer("main\n"));
    register(dir.path(), "main", port, "k1");
    let before = list_panes(dir.path(), "#{session_name}", soon(2000)).unwrap().servers;
    first.join().unwrap();
    let (port, second) = server("k2", Reply::Answer("main\n"));
    register(dir.path(), "main", port, "k2");
    let after = list_panes(dir.path(), "#{session_name}", soon(2000)).unwrap().servers;
    second.join().unwrap();
    assert_ne!(before, after, "sessão recriada com o mesmo nome é outro servidor");
    assert!(!format!("{after:?}").contains("k2"), "a chave não aparece na identidade");
}

#[test]
fn refused_key_is_an_error_not_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("certa", Reply::Answer(""));
    register(dir.path(), "main", port, "errada");
    assert_eq!(list_panes(dir.path(), "#{session_name}", soon(2000)), Err(Failure { code: "psmux_direct_auth" }));
    seen.join().unwrap();
}

#[test]
fn ok_then_close_without_panes_is_truncated_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k", Reply::Answer(""));
    register(dir.path(), "main", port, "k");
    assert_eq!(list_panes(dir.path(), "#{session_name}", soon(2000)), Err(Failure { code: "psmux_direct_empty" }));
    seen.join().unwrap();
}

#[test]
fn silent_server_is_a_timeout_error() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k", Reply::Silent);
    register(dir.path(), "main", port, "k");
    assert_eq!(list_panes(dir.path(), "#{session_name}", soon(100)), Err(Failure { code: "psmux_direct_timeout" }));
    seen.join().unwrap();
}

#[test]
fn a_dripping_reply_cannot_outlive_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k", Reply::Drip);
    register(dir.path(), "main", port, "k");
    let started = Instant::now();
    assert_eq!(list_panes(dir.path(), "#{session_name}", soon(200)), Err(Failure { code: "psmux_direct_timeout" }));
    assert!(started.elapsed() < Duration::from_millis(600), "levou {:?}", started.elapsed());
    seen.join().unwrap();
}

/// No Windows, o servidor que aceita a conexão enquanto abre um processo filho deixa o filho com uma
/// cópia dela, e o fechamento chega como reset. A segunda tentativa encontra a conexão limpa.
#[test]
fn a_reset_connection_is_tried_once_more() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        // Fechar sem ler o pedido que já chegou faz o sistema mandar reset ao cliente.
        let (first, _) = listener.accept().unwrap();
        // Espera o pedido chegar sem consumi-lo: fechar com ele pendente é o que provoca o reset.
        first.peek(&mut [0u8; 1]).unwrap();
        drop(first);
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut auth, mut cmd) = (String::new(), String::new());
        reader.read_line(&mut auth).unwrap();
        reader.read_line(&mut cmd).unwrap();
        let mut out = stream;
        out.write_all(b"OK\nmain\n").unwrap();
    });
    register(dir.path(), "main", port, "k");
    assert_eq!(list_panes(dir.path(), "#{session_name}", soon(2000)).map(|l| l.text), Ok("main\n".to_owned()));
    handle.join().unwrap();
}

#[test]
fn data_dir_follows_the_cli_resolution() {
    let base = std::env::temp_dir();
    let data = base.join("dados");
    let with_slash = format!("{}{}", data.display(), std::path::MAIN_SEPARATOR);
    let vars = |set: Vec<(&'static str, String)>| move |name: &str| set.iter().find(|(k, _)| *k == name).map(|(_, v)| v.into());
    let home = base.display().to_string();
    assert_eq!(data_dir(vars(vec![("PSMUX_DATA_DIR", with_slash), ("USERPROFILE", home.clone())])), Some(data));
    assert_eq!(data_dir(vars(vec![("USERPROFILE", home.clone())])), Some(base.join(".psmux")));
    // O psmux recusa pasta relativa; aqui ela fica no CLI.
    assert_eq!(data_dir(vars(vec![("PSMUX_DATA_DIR", "relativa".into()), ("USERPROFILE", home)])), None);
    assert_eq!(data_dir(vars(vec![])), None);
}

#[test]
fn a_command_reaches_its_session_quoted_and_returns_the_cli_output() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k", Reply::Answer("linha\n\n\nfim\n"));
    register(dir.path(), "main", port, "k");
    let args: Vec<String> = ["capture-pane", "-p", "-t", "=main:0.0", "-S", "-200"].map(String::from).to_vec();
    assert_eq!(command(dir.path(), "main", &args, soon(2000)), Ok(b"linha\n\n\nfim\n".to_vec()), "linhas em branco ficam, como no CLI");
    let seen = seen.join().unwrap();
    assert_eq!(seen[1], "\"capture-pane\" \"-p\" \"-t\" \"=main:0.0\" \"-S\" \"-200\"\n");
}

#[test]
fn a_refused_key_never_ran_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("certa", Reply::Answer(""));
    register(dir.path(), "main", port, "errada");
    let args = vec!["send-keys".to_owned(), "-t".into(), "=main:0.0".into(), "Enter".into()];
    assert_eq!(command(dir.path(), "main", &args, soon(2000)), Err(CommandFailure { code: "psmux_direct_auth", written: false }));
    seen.join().unwrap();
}

#[test]
fn a_silent_server_may_have_run_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = server("k", Reply::Silent);
    register(dir.path(), "main", port, "k");
    let args = vec!["send-keys".to_owned(), "-t".into(), "=main:0.0".into(), "Enter".into()];
    assert_eq!(command(dir.path(), "main", &args, soon(100)), Err(CommandFailure { code: "psmux_direct_timeout", written: true }));
    seen.join().unwrap();
}

#[test]
fn a_session_outside_the_registry_is_not_tried() {
    let dir = tempfile::tempdir().unwrap();
    let args = vec!["send-keys".to_owned()];
    for base in ["", "ns__other", "..\\x", "nada"] {
        assert_eq!(command(dir.path(), base, &args, soon(500)), Err(CommandFailure { code: "psmux_direct_registry", written: false }), "{base}");
    }
}
