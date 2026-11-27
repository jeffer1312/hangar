//! Um teste por defeito do `hangar-send` antigo que a migração corrigiu. Cada um
//! falha no comportamento de antes, dito no comentário.

mod common;

use common::{FakeServer, Home, Outcome, Route, peer_host, plain_host};

/// Antes: `hangar-send --foo x` mandava o recado para a sessão "--foo" e saía 0.
#[test]
fn unknown_flag_is_refused_before_any_request() {
    let server = FakeServer::start(vec![Route::new("*", "", 200, "{}")]);
    let home = Home::new(server.port, "");
    let done = home.run(&["--foo", "x"], &[], &plain_host());
    assert_eq!(done.code, 2, "{}", done.stderr);
    assert_eq!(done.stderr, "erro: opção desconhecida '--foo' (veja hangar-send --help)\n");
    assert!(server.requests().is_empty());
}

/// Antes: o nome ia cru no caminho e `a#b` virava `POST /api/sessions/a` (o `#` cortava a URL).
#[test]
fn session_name_goes_encoded_in_the_path() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, r#"{"delivered": true}"#)]);
    let home = Home::new(server.port, "");
    let done = home.run(&["a#b c/d", "oi"], &[], &plain_host());
    assert_eq!(done.code, 0, "{}", done.stderr);
    let paths: Vec<String> = server.requests().into_iter().map(|request| request.path).collect();
    assert_eq!(paths, ["/api/sessions/a%23b%20c%2Fd/input"]);
}

/// Antes: servidor fora no `--list` saía 1 com o traceback do `json.load` depois da frase.
#[test]
fn list_with_the_server_down_is_an_error_with_code_two() {
    let port = common::dead_port();
    let home = Home::new(port, "");
    let done = home.run(&["--list"], &[], &plain_host());
    assert_eq!(done.code, 2);
    assert_eq!(done.stderr, format!("erro: backend inacessível em http://127.0.0.1:{port} (hangar rodando?)\n"));
}

/// Antes: `a::b::c` ia para o servidor `a`, sessão `b::c`.
#[test]
fn three_part_address_is_refused() {
    let server = FakeServer::start(vec![Route::new("*", "", 200, "[]")]);
    let home = Home::new(server.port, "CP_SERVER_ID=casa\n");
    let done = home.run(&["a::b::c", "oi"], &[], &plain_host());
    assert_eq!(done.code, 2);
    assert_eq!(done.stderr, "erro: nome inválido 'a::b::c': use servidor::sessao (um '::' só)\n");
    assert!(server.requests().is_empty());
}

/// Antes: resposta sem `session` dava `KeyError` em traceback depois de o par já existir.
#[test]
fn missing_field_in_the_answer_is_an_erro_line() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, r#"{"owner": "Ana"}"#)]);
    let home = Home::new(server.port, "");
    let done = home.run(&["--aceitar-par", "https://x--guest.y.hangar.dev.br/par/z"], &[], &plain_host());
    assert_eq!(done.code, 1);
    assert_eq!(done.stderr, "erro: resposta de /api/sessions/cli/pair-accept sem o campo 'session'\n");
    assert!(!done.stderr.contains("Traceback"));
}

/// Antes: resposta que não é JSON imprimia "na fila" sem saber se o recado ficou na fila.
#[test]
fn unreadable_send_answer_is_not_reported_as_queued() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, "ok")]);
    let home = Home::new(server.port, "");
    let done = home.run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 1);
    assert!(!done.stdout.contains("na fila"), "{}", done.stdout);
    assert!(done.stderr.starts_with("erro: resposta ilegível do envio para alvo"), "{}", done.stderr);
}

/// Antes: o `${2:?}` do bash saía com o código dele e o prefixo `script: line N:`.
#[test]
fn usage_error_exits_two_without_the_bash_prefix() {
    let home = Home::new(common::dead_port(), "");
    let done = home.run(&["--close"], &[], &plain_host());
    assert_eq!(done.code, 2);
    assert_eq!(done.stderr, "uso: hangar-send --close <sessao|servidor::sessao>\n");
}

/// Antes: o `curl` mandava só a primeira linha do token repetido (`Bearer um`); vale a última.
#[test]
fn repeated_token_uses_the_last_line() {
    let server = FakeServer::start(vec![Route::new("POST", "/api/sessions/", 200, "{}")]);
    let home = Home::new(server.port, "CP_AUTH_TOKEN=dois\n");
    let done = home.run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 0, "{}", done.stderr);
    let requests = server.requests();
    assert_eq!(requests[0].header("authorization"), Some("Bearer dois"));
}

/// Antes: sem `CP_AUTH_TOKEN` o `pipefail` saía 1 calado.
#[test]
fn missing_token_says_where_it_looked() {
    let home = Home::new(common::dead_port(), "");
    // Um componente por vez: no Windows o caminho dito usa `\` em todas as partes.
    let env = home.home().join(".hangar").join("config").join(".env");
    std::fs::write(&env, "CP_PORT=1\n").unwrap();
    let done = home.run(&["--whoami"], &[], &plain_host());
    assert_eq!(done.code, 1);
    assert_eq!(done.stderr, format!("erro: CP_AUTH_TOKEN ausente em {}\n", env.display()));
}

/// O servidor recebeu o recado e não respondeu no prazo, e a conversa não pôde ser lida: a frase diz
/// que pode ter chegado (4), porque "backend inacessível" levaria a reenviar e duplicar o recado.
/// Conexão recusada continua "backend inacessível".
#[test]
fn a_send_without_answer_says_the_message_may_have_arrived() {
    let mut route = Route::new("POST", "/api/sessions/", 200, r#"{"delivered": true}"#);
    route.replies[0].delay = 3.0;
    let server = FakeServer::start(vec![route]);
    let home = Home::new(server.port, "");
    let done = home.run(&["alvo", "oi"], &[("API_TIMEOUT", "1"), ("HANGAR_SEND_CHECK_SECONDS", "1")], &plain_host());
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert_eq!(
        done.stderr,
        format!("erro: sem resposta de http://127.0.0.1:{} no prazo — o recado pode ter chegado; confira com hangar-send --list antes de reenviar\n", server.port)
    );
    assert_eq!(posts(&server), 1, "o pedido chegou ao servidor e não foi repetido");

    let port = common::dead_port();
    let done = Home::new(port, "").run(&["alvo", "oi"], &[], &plain_host());
    assert_eq!(done.code, 2, "{}", done.stderr);
    assert_eq!(done.stderr, format!("erro: backend inacessível em http://127.0.0.1:{port} (hangar rodando?)\n"));
}

/// Horários fixos, sem relógio: um bem depois de qualquer envio e um bem antes.
const LATER: f64 = 4_102_444_800.0;
const LONG_AGO: f64 = 1_000_000_000.0;

fn posts(server: &FakeServer) -> usize {
    server.requests().iter().filter(|request| request.method == "POST").count()
}

/// O POST para o recado e não responde antes do `API_TIMEOUT`; a conversa responde como `history`.
fn cut_send(history: Route) -> (FakeServer, Outcome) {
    let mut post = Route::new("POST", "/api/sessions/", 200, r#"{"delivered": true}"#);
    post.replies[0].delay = 2.0;
    let server = FakeServer::start(vec![post, history]);
    let home = Home::new(server.port, "");
    let done = home.run(&["alvo", "oi"], &[("API_TIMEOUT", "0.5"), ("HANGAR_SEND_CHECK_SECONDS", "1")], &plain_host());
    (server, done)
}

fn history(status: u16, body: &str) -> Route {
    Route::new("GET", "/api/sessions/alvo/history", status, body)
}

/// Antes: o corte saía 2 sem olhar a conversa, e quem chamava reenviava um recado que já tinha chegado.
#[test]
fn a_cut_send_found_in_the_conversation_says_it_arrived() {
    let line = format!(r#"[{{"kind": "user_msg", "id": "queued-x", "text": "[de: cli] oi", "ts": {LATER}}}]"#);
    let (server, done) = cut_send(history(200, &line));
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(done.stdout, "chegou -> alvo (conferido na conversa depois do corte)\n");
    assert_eq!(done.stderr, "");
    assert_eq!(posts(&server), 1, "nunca um segundo POST");
    let paths: Vec<String> = server.requests().into_iter().filter(|request| request.method == "GET").map(|request| request.path).collect();
    assert_eq!(paths.first().map(String::as_str), Some("/api/sessions/alvo/history?limit=40"));
}

/// A conversa foi lida e o recado não está nela: o servidor ainda pode gravá-lo, então fica incerto (4),
/// nunca "não chegou", que levaria a reenviar e duplicar.
#[test]
fn a_cut_send_missing_from_the_conversation_stays_uncertain() {
    let lines = format!(
        r#"[{{"kind": "user_msg", "id": "u1", "text": "[de: cli] outra coisa", "ts": {LATER}}}, {{"kind": "assistant_msg", "id": "a1", "text": "oi", "ts": {LATER}}}]"#
    );
    let (server, done) = cut_send(history(200, &lines));
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert_eq!(done.stdout, "");
    assert_eq!(
        done.stderr,
        format!("erro: sem resposta de http://127.0.0.1:{} no prazo e o recado não apareceu na conversa em 1 s; o servidor pode ainda estar processando — confira com hangar-send --list antes de reenviar\n", server.port)
    );
    assert_eq!(posts(&server), 1);
}

/// O mesmo texto de muito antes do envio é outro recado: não prova a chegada deste.
#[test]
fn an_old_identical_message_does_not_prove_arrival() {
    let line = format!(r#"[{{"kind": "user_msg", "id": "u1", "text": "[de: cli] oi", "ts": {LONG_AGO}}}]"#);
    let (server, done) = cut_send(history(200, &line));
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert_eq!(done.stdout, "");
    assert_eq!(posts(&server), 1);
}

/// Fala igual sem horário pode ser um recado de antes: não prova a chegada, nem a nega (4).
#[test]
fn an_untimed_identical_message_leaves_the_send_uncertain() {
    let line = r#"[{"kind": "user_msg", "id": "u1", "text": "[de: cli] oi"}]"#;
    let (server, done) = cut_send(history(200, line));
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert_eq!(done.stdout, "");
    assert_eq!(
        done.stderr,
        format!("erro: sem resposta de http://127.0.0.1:{} no prazo — o recado pode ter chegado; confira com hangar-send --list antes de reenviar\n", server.port)
    );
    assert_eq!(posts(&server), 1);
}

/// Sem leitura boa da conversa não se afirma nada: segue a frase de "pode ter chegado" (4).
#[test]
fn a_cut_send_whose_history_cannot_be_read_stays_uncertain() {
    let (server, done) = cut_send(history(404, r#"{"detail": "erro_sessao_inexistente"}"#));
    assert_eq!(done.code, 4, "{}", done.stderr);
    assert_eq!(done.stdout, "");
    assert_eq!(
        done.stderr,
        format!("erro: sem resposta de http://127.0.0.1:{} no prazo — o recado pode ter chegado; confira com hangar-send --list antes de reenviar\n", server.port)
    );
    assert_eq!(posts(&server), 1);
}

/// O recado a outra máquina é conferido nela, com o token dela, nunca na conversa desta.
#[test]
fn a_cut_send_to_another_server_checks_that_server() {
    let mut post = Route::new("POST", "/api/sessions/", 200, r#"{"delivered": true}"#);
    post.replies[0].delay = 2.0;
    let line = format!(r#"[{{"kind": "user_msg", "id": "queued-x", "text": "[de: casa::cli] oi", "ts": {LATER}}}]"#);
    let peer = FakeServer::start(vec![post, history(200, &line)]);
    let local = FakeServer::start(vec![Route::new("GET", "/api/external-pairs", 200, "[]")]);
    let home = Home::new(local.port, "CP_SERVER_ID=casa\n");
    let host = peer_host("a", &format!("http://127.0.0.1:{}", peer.port));
    let done = home.run(&["a::alvo", "oi"], &[("API_TIMEOUT", "0.5"), ("HANGAR_SEND_CHECK_SECONDS", "1")], &host);
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert_eq!(done.stdout, "chegou -> a::alvo (conferido na conversa depois do corte)\n");
    assert_eq!(posts(&peer), 1);
    let reads: Vec<_> = peer.requests().into_iter().filter(|request| request.method == "GET").collect();
    assert!(!reads.is_empty(), "a conversa da outra máquina não foi lida");
    assert!(reads.iter().all(|request| request.header("authorization") == Some("Bearer tok-a")), "{reads:?}");
    assert!(local.requests().iter().all(|request| !request.path.contains("/history")), "a conferência foi à máquina errada");
}
