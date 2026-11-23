//! Túnel de porta: cada WebSocket é uma conexão TCP com o loopback desta máquina, bytes crus nos
//! dois sentidos. Autenticação e portas reservadas ficam com quem monta a rota.
use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, MissedTickBehavior, interval_at};

pub const PING_EVERY: Duration = Duration::from_secs(25);
/// Teto de uma mensagem do cliente: ele manda pedaços de leitura TCP, nunca perto disso.
pub const MAX_MESSAGE: usize = 1024 * 1024;
const CHUNK: usize = 64 * 1024;

/// `port` da query, 1..=65535; o resto é `None`.
pub fn port_of(query: Option<&str>) -> Option<u16> {
    hangar_core::auth::query_param(query, "port")?.parse::<u16>().ok().filter(|port| *port != 0)
}

pub async fn connect(port: u16) -> io::Result<TcpStream> {
    let stream = hangar_core::net::connect_loopback(port).await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Copia até um dos lados fechar ou `ended` resolver (acesso revogado). Cada sentido corre sozinho:
/// num laço único, alvo que só lê depois de escrever travaria os dois lados.
pub async fn relay(socket: WebSocket, tcp: TcpStream, ended: impl Future<Output = ()>) {
    let (mut from_tcp, mut to_tcp) = tcp.into_split();
    let (mut sink, mut stream) = socket.split();
    let start = Instant::now();
    let heard = AtomicU64::new(0);
    let up = async {
        while let Some(Ok(message)) = stream.next().await {
            heard.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
            match message {
                // Binário vazio = o cliente parou de mandar mas espera a resposta (EOF de escrita,
                // como o `nc`). O Close não serve: depois dele o servidor não pode mandar mais nada.
                Message::Binary(bytes) if bytes.is_empty() => { let _ = to_tcp.shutdown().await; }
                Message::Binary(bytes) => if to_tcp.write_all(&bytes).await.is_err() { return },
                Message::Close(_) => return,
                // Texto não é do protocolo; ping e pong o axum já trata.
                _ => {}
            }
        }
    };
    let down = async {
        // Sem tráfego, o Caddy e os NATs do caminho derrubariam a conexão parada.
        let mut ping = interval_at(Instant::now() + PING_EVERY, PING_EVERY);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut buf = vec![0u8; CHUNK];
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    // Notebook que dormiu ou trocou de rede some sem FIN: sem pong, a conexão cai aqui.
                    let silent = start.elapsed().saturating_sub(Duration::from_millis(heard.load(Ordering::Relaxed)));
                    if silent > PING_EVERY * 3 || sink.send(Message::Ping(Bytes::new())).await.is_err() { return }
                }
                read = from_tcp.read(&mut buf) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => if sink.send(Message::Binary(Bytes::copy_from_slice(&buf[..n]))).await.is_err() { return },
                },
            }
        }
        let _ = sink.send(Message::Close(Some(CloseFrame { code: 1000, reason: "".into() }))).await;
    };
    // Revogado: o socket cai sem despedida; o cliente vê a conexão fechar.
    tokio::select! {
        () = up => {}
        () = down => {}
        () = ended => {}
    }
}

#[cfg(test)]
mod tests;
