//! Tudo que entra pelo Connect veio da internet, embora o socket diga loopback (o Caddy é local).
use std::net::{IpAddr, SocketAddr};

use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::extract::Request;
use axum::serve::IncomingStream;
use tokio::net::TcpListener;

use crate::devices::CONNECT_PEER;

/// Servir com `into_make_service_with_connect_info::<ConnectConn>()` num `TcpListener` puro. O socket
/// real nunca vira `ConnectInfo<SocketAddr>`: rota fora de `rewrite_peer` falha, em vez de ver um
/// cliente local.
#[derive(Clone, Copy, Debug)]
pub struct ConnectConn;

impl Connected<IncomingStream<'_, TcpListener>> for ConnectConn {
    fn connect_info(_: IncomingStream<'_, TcpListener>) -> Self { Self }
}

/// Põe o par do Connect e apaga os `X-Forwarded-*`, que aqui ninguém de confiança escreveu.
pub async fn rewrite_peer(mut req: Request) -> Request {
    let forged: Vec<_> = req.headers().keys().filter(|name| name.as_str().starts_with("x-forwarded-")).cloned().collect();
    for name in forged {
        req.headers_mut().remove(name);
    }
    let peer: IpAddr = CONNECT_PEER.parse().expect("CONNECT_PEER é um endereço");
    req.extensions_mut().insert(ConnectInfo(SocketAddr::new(peer, 0)));
    req
}
