//! Repasse, autenticação e saúde do hangar-server contra um Python falso.
use crate::fake;

use std::time::Duration;

use fake::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn health_answers_without_token_and_with_cors() {
    let (_fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/__hangar_server/health"))
        .header("origin", "http://outra.maquina")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    assert!(r.headers()["access-control-expose-headers"].to_str().unwrap().split(", ").any(|header| header == "ETag"));
    let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    // `protocol` é o contrato com o Python (RUST_SERVER_PROTOCOL na Task 13): mudar exige os dois.
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["protocol"], hangar_server::INTERNAL_PROTOCOL);
    // A saúde anuncia também a posse dos três provedores migrados com terminal.
    assert_eq!(v["owns"], serde_json::json!([
        {"provider": "claude", "headless": true}, {"provider": "claude", "headless": false},
        {"provider": "codex", "headless": true}, {"provider": "codex", "headless": false},
        {"provider": "pi", "headless": false}, {"provider": "omp", "headless": false},
        {"provider": "kimi", "headless": false}]));
    // O endereço da ponte de terminal é anunciado aqui, mas só pode ser de loopback.
    let terminal = v["terminal_address"].as_str().expect("terminal_address");
    assert!(terminal.starts_with("127.0.0.1:"), "{terminal}");
    // Grupos: o Python só passa a pedir ao Rust quando a saúde diz que ele os atende.
    assert!(v["groups"].is_boolean(), "{v}");
}

#[tokio::test]
async fn closed_stdin_stops_the_server() {
    let (_fake, up) = spawn_fake().await;
    // Porta pública nova a cada tentativa: em paralelo, outro teste pode estar na porta da ponte do
    // plugin (127.0.0.1 na mesma porta), e aí o servidor sai com `AddrInUse` antes de atender.
    let mut started = None;
    for _ in 0..5 {
        let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // O duplex faz o papel do stdin: soltar a ponta de escrita é o Python morrendo.
        let (parent, child_stdin) = tokio::io::duplex(64);
        let server = tokio::spawn(hangar_server::serve_until(
            listener,
            config(up, "127.0.0.1"),
            hangar_server::parent_gone(child_stdin),
        ));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let health = loop {
            if server.is_finished() {
                break None;
            }
            if let Ok(r) = client().get(format!("http://{addr}/__hangar_server/health")).send().await {
                break Some(r);
            }
            assert!(tokio::time::Instant::now() < deadline, "a saúde pública não respondeu em 5 s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        match health {
            Some(r) => {
                assert_eq!(r.status(), 200);
                started = Some((addr, parent, server));
                break;
            }
            None => {
                let error = server.await.unwrap().expect_err("o servidor saiu sem erro antes da saúde");
                assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
            }
        }
    }
    let (addr, parent, server) = started.expect("nenhuma porta livre para a ponte");
    let plugin = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, addr.port()));
    assert_eq!(client().get(format!("http://{plugin}/api/sessions")).send().await.unwrap().status(), 404);
    drop(parent);
    let ended = tokio::time::timeout(Duration::from_secs(5), server).await.expect("parou em 5 s");
    assert!(ended.unwrap().is_ok(), "fim pelo cano é saída limpa");
    crate::assert_closed(addr, "a porta pública").await;
    crate::assert_closed(plugin, "a ponte do plugin").await;
}

#[tokio::test]
async fn proxy_streams_body_without_buffering() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let mut r = client().get(format!("http://{srv}/api/stream")).send().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), r.chunk()).await.unwrap().unwrap().unwrap();
    assert_eq!(&first[..], b"um");
    fake.release.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(5), r.chunk()).await.unwrap().unwrap().unwrap();
    assert_eq!(&second[..], b"dois");
}

#[tokio::test]
async fn proxy_never_follows_redirects() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client().get(format!("http://{srv}/api/redirect")).send().await.unwrap();
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers()["location"], "/api/outro");
    assert_eq!(fake.hits_to("/api/outro"), 0);
}

#[tokio::test]
async fn proxy_passes_websocket_upgrade_both_ways() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "10.9.9.9")).await;
    let mut s = tokio::net::TcpStream::connect(srv).await.unwrap();
    s.write_all(
        b"GET /api/ws HTTP/1.1\r\nHost: hangar\r\nConnection: Upgrade\r\nUpgrade: eco\r\n\
          X-Forwarded-For: 203.0.113.9\r\nX-Hangar-Internal: segredo-interno\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut byte)).await.unwrap().unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s.write_all(b"ola").await.unwrap();
    let mut got = [0u8; 3];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut got)).await.unwrap().unwrap();
    assert_eq!(&got, b"ola");
    // O aperto de mão também leva o cliente real e nunca o segredo vindo de fora.
    let (_, h) = fake.last_hit();
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
    assert!(!h.contains_key("x-hangar-internal"));
}

#[tokio::test]
async fn unknown_token_gets_401_from_rust() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .bearer_auth("token-de-convidado")
        .send()
        .await
        .unwrap();
    // R5 (decisão 5): token que nenhum porteiro reconhece leva o 401 do Rust, sem chegar ao Python.
    assert_eq!(r.status().as_u16(), 401);
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0);
    assert_eq!(fake.info_calls(), 0, "convidado nunca chega às rotas internas");

    // Controle: o mesmo pedido com o token do dono passa pelo atalho e consulta a rota interna.
    client().get(format!("http://{srv}/api/sessions/s/history")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(fake.info_calls(), 1);
}

#[tokio::test]
async fn blocked_origin_never_gets_the_owner_shortcut() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let ip = "198.51.100.7";
    for _ in 0..8 {
        let r = client()
            .get(format!("http://{srv}/api/probe"))
            .header("x-forwarded-for", ip)
            .bearer_auth("errado")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    // `test_backoff_barra_apos_max_fails`: bloqueada, nem a senha certa passa; o 429 agora é do Rust.
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .header("x-forwarded-for", ip)
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    assert_eq!(r.headers()["retry-after"], "30");
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0);
    assert_eq!(fake.info_calls(), 0, "bloqueado: o token do dono nem é avaliado");

    // Controle: outra origem com o mesmo token segue pelo atalho.
    client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .header("x-forwarded-for", "198.51.100.8")
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(fake.info_calls(), 1);
}

#[tokio::test]
async fn origin_limited_by_python_gets_owner_requests_proxied() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let ip = "198.51.100.9";
    let r = client()
        .get(format!("http://{srv}/api/limited"))
        .header("x-forwarded-for", ip)
        .bearer_auth("errado")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    // A origem que o Python saturou fica bloqueada também no Rust: o dono dali recebe o 429 dele.
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .header("x-forwarded-for", ip)
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    // `/events` não existe mais no Python: a origem bloqueada recebe o 429 do próprio Rust.
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/events"))
        .header("x-forwarded-for", ip)
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    assert_eq!(r.headers()["retry-after"], "30");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["detail"]["code"], "erro_muitas_tentativas");
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 0);
    assert_eq!(fake.hits_to("/api/sessions/s/events"), 0);
    assert_eq!(fake.info_calls(), 0, "o 429 do Python desliga o atalho dessa origem");
}

#[tokio::test]
async fn pooled_connection_is_dropped_before_uvicorn_closes_it() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Upstream que nunca fecha a conexão e conta quantas recebeu.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let Ok(n) = s.read(&mut chunk).await else { return };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    while let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        buf.drain(..i + 4);
                        if s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok").await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    let http = hangar_server::proxy::client();
    let get = || async {
        let req = axum::http::Request::get(format!("http://{up}/x")).body(axum::body::Body::empty()).unwrap();
        let resp = http.request(req).await.unwrap();
        http_body_util::BodyExt::collect(resp.into_body()).await.unwrap();
    };
    get().await;
    get().await;
    // O pool às vezes abre uma segunda conexão em paralelo enquanto espera a primeira voltar; a
    // conta vale depois da espera. Com o padrão de 90 s, o pedido seguinte reaproveitaria uma.
    tokio::time::sleep(hangar_server::proxy::POOL_IDLE + Duration::from_millis(300)).await;
    let before = accepted.load(Ordering::SeqCst);
    get().await;
    assert_eq!(accepted.load(Ordering::SeqCst), before + 1, "ociosa além do prazo, abre outra");
}
