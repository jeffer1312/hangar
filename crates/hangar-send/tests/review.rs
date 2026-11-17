//! O que a revisão da R3 pediu: redirecionamento não seguido, prazos do ambiente e o da varredura,
//! teto do corpo, `PWD` que não é a pasta real, fim de linha CRLF e a conversão `/c/` do Windows.

mod common;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Answer, FakeHost, FakeServer, Home, Reply, Route, plain_host};

/// Servidor que responde 301 para outro endereço a toda conexão.
fn redirecting(to: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = [0u8; 8192];
            let _ = stream.read(&mut buffer);
            let _ = write!(stream, "HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:{to}/api/sessions/alvo/input\r\n\
                                   Content-Length: 4\r\nConnection: close\r\n\r\nfoi!");
        }
    });
    port
}

fn peer_host(id: &str, base: &str) -> FakeHost {
    let answer = |stdout: String| Answer { stdout, ..Answer::default() };
    let server = BTreeMap::from([
        (format!("peers config {id}"), answer(format!("{base}\ttok-{id}\n"))),
        ("peers path".to_owned(), answer("/nao/existe/peers.json\n".into())),
    ]);
    let tmux = BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() })]);
    FakeHost::new(Some(server), tmux)
}

/// O curl não seguia redirecionamento: o 301 da outra máquina é a resposta, e o token não vai a outro lugar.
#[test]
fn redirect_from_a_peer_is_reported_not_followed() {
    let elsewhere = FakeServer::start(vec![Route::new("*", "", 200, r#"{"delivered": true}"#)]);
    let peer = redirecting(elsewhere.port);
    let local = FakeServer::start(vec![Route::new("GET", "/api/external-pairs", 200, "[]")]);
    let home = Home::new(local.port, "CP_SERVER_ID=casa\n");
    let done = home.run(&["a::alvo", "oi"], &[], &peer_host("a", &format!("http://127.0.0.1:{peer}")));
    assert_eq!((done.code, done.stderr.as_str()), (1, "erro HTTP 301: foi!\n"));
    assert!(elsewhere.requests().is_empty(), "o redirecionamento foi seguido");
}

/// `API_TIMEOUT` encurta o prazo de cada pedido; sem resposta no prazo é o "inacessível" com 2.
#[test]
fn api_timeout_from_the_environment_cuts_the_request() {
    let slow = Route { method: "*".into(), prefix: String::new(), replies: vec![Reply { status: 200, body: "{}".into(), delay: 3.0 }] };
    let server = FakeServer::start(vec![slow]);
    let home = Home::new(server.port, "");
    let started = Instant::now();
    let done = home.run(&["alvo", "oi"], &[("API_TIMEOUT", "0.3")], &plain_host());
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(done.code, 2);
    assert_eq!(done.stderr, format!("erro: backend inacessível em http://127.0.0.1:{} (hangar rodando?)\n", server.port));
}

/// `API_CONNECT_TIMEOUT` vale para a conexão: um endereço que não responde desiste no prazo.
#[test]
fn api_connect_timeout_from_the_environment_bounds_the_connection() {
    let local = FakeServer::start(vec![Route::new("GET", "/api/external-pairs", 200, "[]")]);
    let home = Home::new(local.port, "CP_SERVER_ID=casa\n");
    let started = Instant::now();
    // 10.255.255.1 não é roteável: a conexão pendura até o prazo (ou falha antes, sem rota).
    let done = home.run(&["a::alvo", "oi"], &[("API_CONNECT_TIMEOUT", "0.3")], &peer_host("a", "http://10.255.255.1:9"));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(done.code, 2, "{}", done.stderr);
}

/// A varredura do `--list` desiste de cada máquina em 8 s, mesmo com `API_TIMEOUT` maior.
#[test]
fn peer_scan_keeps_its_eight_second_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let peers = dir.path().join("peers.json");
    std::fs::write(&peers, "{}").unwrap();
    let slow = Route { method: "*".into(), prefix: String::new(), replies: vec![Reply { status: 200, body: "[]".into(), delay: 12.0 }] };
    let peer = FakeServer::start(vec![slow]);
    let local = FakeServer::start(vec![Route::new("GET", "/api/sessions", 200, "[]"), Route::new("GET", "/api/external-pairs", 200, "[]")]);
    let home = Home::new(local.port, "");
    let answer = |stdout: String| Answer { stdout, ..Answer::default() };
    let server = BTreeMap::from([
        ("peers path".to_owned(), answer(format!("{}\n", peers.display()))),
        ("peers tsv".to_owned(), answer(format!("lenta\thttp://127.0.0.1:{}\ttok\n", peer.port))),
    ]);
    let host = FakeHost::new(Some(server), BTreeMap::from([("*".to_owned(), Answer { rc: 1, ..Answer::default() })]));
    let started = Instant::now();
    let done = home.run(&["--list"], &[("API_TIMEOUT", "60")], &host);
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(7) && elapsed < Duration::from_secs(11), "{elapsed:?}");
    assert_eq!(done.code, 0);
    assert_eq!(done.stderr, format!("⚠ servidor 'lenta' fora do ar (http://127.0.0.1:{})\n", peer.port));
}

/// Corpo acima do teto não é lido inteiro: vira `erro:` com o endereço.
#[test]
fn response_above_the_limit_is_an_error() {
    let big = "x".repeat(hangar_send::RESPONSE_LIMIT + 1);
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, &big)]);
    let home = Home::new(server.port, "");
    let done = home.run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 1);
    assert_eq!(done.stderr, format!("erro: a resposta de http://127.0.0.1:{}/api/sessions/alvo/input passou de 16 MiB\n", server.port));
}

/// Token com caractere que não cabe em cabeçalho não vira "inacessível": a frase diz o porquê.
#[test]
fn token_that_cannot_be_a_header_has_its_own_error() {
    let server = FakeServer::start(vec![Route::new("*", "", 200, "{}")]);
    let home = Home::new(server.port, "");
    std::fs::write(home.home().join(".hangar/config/.env"), format!("CP_AUTH_TOKEN=a\u{1}b\nCP_PORT={}\n", server.port)).unwrap();
    let done = home.run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 1);
    assert_eq!(done.stderr, "erro: o token do .env tem um caractere que não cabe num cabeçalho HTTP\n");
    assert!(server.requests().is_empty());
}

/// `.env` em CRLF: o `\r` não entra no token.
#[test]
fn crlf_env_gives_the_bare_token() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, "{}")]);
    let home = Home::new(server.port, "");
    std::fs::write(home.home().join(".hangar/config/.env"), format!("CP_AUTH_TOKEN=tok\r\nCP_PORT={}\r\n", server.port)).unwrap();
    let done = home.run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(server.requests()[0].header("Authorization"), Some("Bearer tok"));
}

fn contract_body(server: &FakeServer) -> String {
    let request = server.requests().into_iter().find(|request| request.method == "PUT").expect("PUT do contrato");
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    body["content"].as_str().unwrap().to_owned()
}

fn run_in(home: &Home, args: &[&str], cwd: &Path, pwd: &Path) -> common::Outcome {
    let pwd = pwd.to_string_lossy().into_owned();
    let invocation = hangar_send::Invocation {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        root: Some(home.root()),
        home: home.home(),
        vars: home.vars(&[("PWD", pwd.as_str())]),
        cwd: cwd.to_path_buf(),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = hangar_send::run(&invocation, &plain_host(), &mut out, &mut err);
    common::Outcome { code, stdout: String::from_utf8_lossy(&out).into_owned(), stderr: String::from_utf8_lossy(&err).into_owned() }
}

/// `--gravar` relativo: um `PWD` herdado de outra pasta não escolhe o arquivo; vale a pasta real.
#[test]
fn relative_contract_file_ignores_a_stale_pwd() {
    let server = FakeServer::start(vec![Route::new("PUT", "/api/sessions/", 200, r#"{"mtime": 1}"#)]);
    let home = Home::new(server.port, "");
    let (real, stale) = (home.dir.path().join("real"), home.dir.path().join("outra"));
    for (dir, text) in [(&real, "da pasta real\r\nfim"), (&stale, "do PWD velho")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("c.md"), text).unwrap();
    }
    let done = run_in(&home, &["--contrato", "--gravar", "c.md"], &real, &stale);
    assert_eq!(done.code, 0, "{}", done.stderr);
    // O CRLF vira LF no conteúdo, como o `open()` do Python.
    assert_eq!(contract_body(&server), "da pasta real\nfim");
}

/// Com o `PWD` sendo a pasta real (por um link, inclusive), ele vale.
#[cfg(unix)]
#[test]
fn relative_contract_file_uses_the_logical_pwd_when_it_is_the_real_dir() {
    let server = FakeServer::start(vec![Route::new("PUT", "/api/sessions/", 200, r#"{"mtime": 1}"#)]);
    let home = Home::new(server.port, "");
    let real = home.dir.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("c.md"), "pelo link").unwrap();
    let link = home.dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let done = run_in(&home, &["--contrato", "--gravar", "c.md"], &real, &link);
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(contract_body(&server), "pelo link");
}

/// No Windows, o `/c/...` do Git Bash chega como `C:/...` (a conversão que o Python recebia).
#[cfg(windows)]
#[test]
fn msys_path_of_the_contract_file_is_converted_on_windows() {
    assert_eq!(hangar_send::msys_drive_path("/c/Users/x/c.md", cfg!(windows)), "C:/Users/x/c.md");
    let server = FakeServer::start(vec![Route::new("PUT", "/api/sessions/", 200, r#"{"mtime": 1}"#)]);
    let home = Home::new(server.port, "");
    let file = home.dir.path().join("c.md");
    std::fs::write(&file, "convertido").unwrap();
    let native = file.to_string_lossy().replace('\\', "/");
    let (drive, rest) = native.split_once(':').unwrap();
    let msys = format!("/{}{rest}", drive.to_ascii_lowercase());
    let done = home.run(&["--contrato", "--gravar", &msys], &[], &plain_host());
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(contract_body(&server), "convertido");
}
