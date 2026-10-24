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
    assert_eq!(r.headers()["access-control-expose-headers"], "ETag");
    let v: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    // `protocol` é o contrato com o Python (RUST_SERVER_PROTOCOL na Task 13): mudar exige os dois.
    assert_eq!(v["ok"], true);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["protocol"], hangar_server::INTERNAL_PROTOCOL);
    // Derivado da tabela de escritas: depois do corte, Claude e Codex com e sem terminal.
    assert_eq!(v["owns"], serde_json::json!([
        {"provider": "claude", "headless": true}, {"provider": "claude", "headless": false},
        {"provider": "codex", "headless": true}, {"provider": "codex", "headless": false}]));
    // O endereço da ponte de terminal é anunciado aqui, mas só pode ser de loopback.
    let terminal = v["terminal_address"].as_str().expect("terminal_address");
    assert!(terminal.starts_with("127.0.0.1:"), "{terminal}");
    // Grupos: o Python só passa a pedir ao Rust quando a saúde diz que ele os atende.
    assert!(v["groups"].is_boolean(), "{v}");
    assert_eq!(hangar_server::INTERNAL_PROTOCOL, 90);
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
async fn proxy_sets_forwarded_headers_and_drops_internal_secret() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/api/qualquer?x=1"))
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .header("x-hangar-internal", SECRET)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    let (path, h) = fake.last_hit();
    assert_eq!(path, "/api/qualquer?x=1");
    assert_eq!(h["x-forwarded-for"], "203.0.113.9");
    assert_eq!(h["x-forwarded-proto"], "https");
    assert!(!h.contains_key("x-hangar-internal"));
}

#[tokio::test]
async fn untrusted_peer_cannot_rewrite_client_or_scheme() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "10.9.9.9")).await;
    client()
        .get(format!("http://{srv}/api/qualquer"))
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    let (_, h) = fake.last_hit();
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
    assert_eq!(h["x-forwarded-proto"], "http");
}

#[tokio::test]
async fn unparseable_forwarded_for_from_trusted_peer_uses_the_tcp_peer() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    client()
        .get(format!("http://{srv}/api/qualquer"))
        .header("x-forwarded-for", "nao-e-ip")
        .send()
        .await
        .unwrap();
    let (_, h) = fake.last_hit();
    assert_eq!(h["x-forwarded-for"], "127.0.0.1");
}

#[tokio::test]
async fn proxy_streams_body_without_buffering() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let mut r = client().get(format!("http://{srv}/stream")).send().await.unwrap();
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
    let r = client().get(format!("http://{srv}/redirect")).send().await.unwrap();
    assert_eq!(r.status(), 302);
    assert_eq!(r.headers()["location"], "/outro");
    assert_eq!(fake.hits_to("/outro"), 0);
}

#[tokio::test]
async fn proxy_passes_websocket_upgrade_both_ways() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "10.9.9.9")).await;
    let mut s = tokio::net::TcpStream::connect(srv).await.unwrap();
    s.write_all(
        b"GET /ws HTTP/1.1\r\nHost: hangar\r\nConnection: Upgrade\r\nUpgrade: eco\r\n\
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
async fn guest_token_is_always_passed_to_python() {
    let (fake, up) = spawn_fake().await;
    let srv = spawn_server(config(up, "127.0.0.1")).await;
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .bearer_auth("token-de-convidado")
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
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
            .get(format!("http://{srv}/probe"))
            .header("x-forwarded-for", ip)
            .bearer_auth("errado")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let r = client()
        .get(format!("http://{srv}/api/sessions/s/history"))
        .header("x-forwarded-for", ip)
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
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
        .get(format!("http://{srv}/limited"))
        .header("x-forwarded-for", ip)
        .bearer_auth("errado")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    for path in ["history", "events"] {
        let r = client()
            .get(format!("http://{srv}/api/sessions/s/{path}"))
            .header("x-forwarded-for", ip)
            .bearer_auth(OWNER)
            .send()
            .await
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "from-python");
    }
    assert_eq!(fake.hits_to("/api/sessions/s/history"), 1);
    assert_eq!(fake.hits_to("/api/sessions/s/events"), 1);
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

/// A primeira parte só libera o restante por sinal, mantendo os dois sockets do repasse vivos.
async fn spawn_chunked_upstream(
    pieces: usize,
    size: usize,
) -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::Receiver<std::net::SocketAddr>,
    std::sync::Arc<tokio::sync::Notify>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap();
    let (connected, peer) = tokio::sync::mpsc::channel(1);
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let released = release.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, peer) = listener.accept().await.unwrap();
            stream.set_nodelay(true).unwrap();
            let connected = connected.clone();
            let release = released.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read_exact(&mut byte).await.is_err() {
                        return;
                    }
                    head.push(byte[0]);
                }
                // As tarefas auxiliares do servidor podem consultar fatos antes da prova.
                if !head.starts_with(b"GET /x ") {
                    let _=stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                    return;
                }
                connected.send(peer).await.unwrap();
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
                    .await
                    .unwrap();
                let mut piece = format!("{size:x}\r\n").into_bytes();
                piece.extend(std::iter::repeat_n(b'x', size));
                piece.extend_from_slice(b"\r\n");
                stream.write_all(&piece).await.unwrap();
                release.notified().await;
                for _ in 1..pieces {
                    stream.write_all(&piece).await.unwrap();
                    tokio::task::yield_now().await;
                }
                stream.write_all(b"0\r\n\r\n").await.unwrap();
            });
        }
    });
    (up, peer, release)
}

async fn chunk(reader: &mut tokio::io::BufReader<tokio::net::TcpStream>) -> Option<Vec<u8>> {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let size = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes).await.unwrap();
    let mut end = [0; 2];
    reader.read_exact(&mut end).await.unwrap();
    assert_eq!(&end, b"\r\n");
    (size > 0).then_some(bytes)
}

/// Duplica só a referência ao socket: a conexão sinalizada continua aberta e impede reuso da tupla.
#[cfg(target_os = "linux")]
fn live_socket(local: std::net::SocketAddr, peer: std::net::SocketAddr) -> std::os::fd::OwnedFd {
    use std::os::fd::FromRawFd;
    for entry in std::fs::read_dir("/proc/self/fd").unwrap() {
        let entry = entry.unwrap();
        let Some(fd) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        // O descritor pode ser de outro teste ou fechar durante a enumeração; só a duplicata é nossa.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            continue;
        }
        let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(duplicate) };
        let socket = socket2::SockRef::from(&owned);
        if socket.local_addr().ok().and_then(|a| a.as_socket()) == Some(local)
            && socket.peer_addr().ok().and_then(|a| a.as_socket()) == Some(peer)
        {
            return owned;
        }
    }
    panic!("socket da prova não encontrado: {local} -> {peer}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_chunked_body_is_not_held_by_nagle() {
    use tokio::io::AsyncBufReadExt;
    let (pieces, size) = (300, 1000);
    let (up, mut peers, release) = spawn_chunked_upstream(pieces, size).await;
    let home = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(config(up, "127.0.0.1"));
    state.origins_home = home.path().into();
    let srv = spawn_state(state).await;
    tokio::time::timeout(Duration::from_secs(5),async {
        let mut stream=tokio::net::TcpStream::connect(srv).await.unwrap();
        stream.set_nodelay(true).unwrap();
        #[cfg(target_os="linux")]
        let client=stream.local_addr().unwrap();
        stream.write_all(format!("GET /x HTTP/1.1\r\nHost: h\r\nAuthorization: Bearer {OWNER}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut reader=tokio::io::BufReader::new(stream);
        let mut head=String::new();
        loop {
            let mut line=String::new(); reader.read_line(&mut line).await.unwrap();
            assert!(!line.is_empty(),"resposta fechou antes do cabeçalho");
            head.push_str(&line); if line=="\r\n" {break;}
        }
        assert!(head.starts_with("HTTP/1.1 200"));
        assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"));
        let mut body=chunk(&mut reader).await.unwrap();
        assert_eq!(body,vec![b'x';size]);
        let upstream_peer=peers.recv().await.unwrap();
        // A mesma pausa que invalidava a mediana não muda a configuração nem o conteúdo entregue.
        std::thread::sleep(Duration::from_millis(80));
        // A inspeção de sockets do processo usa /proc no Linux; o contrato HTTP completo roda também no Windows.
        #[cfg(target_os="linux")]
        for (label,local,peer) in [("porta pública",srv,client),("conector ao upstream",upstream_peer,up)] {
            let owned=live_socket(local,peer); let socket=socket2::SockRef::from(&owned);
            assert!(socket.tcp_nodelay().unwrap(),"TCP_NODELAY desligado: {label}");
            // Controle negativo: a sonda detecta Nagle no mesmo socket, sem modificar outra conexão.
            socket.set_tcp_nodelay(false).unwrap();
            assert!(!socket.tcp_nodelay().unwrap(),"controle negativo: {label}");
            socket.set_tcp_nodelay(true).unwrap();
        }
        #[cfg(not(target_os="linux"))]
        let _=upstream_peer;
        release.notify_one();
        while let Some(bytes)=chunk(&mut reader).await {body.extend(bytes);}
        assert_eq!(body,vec![b'x';pieces*size]);
    }).await.expect("repasse chunked não concluiu a prova em 5 s");
}
