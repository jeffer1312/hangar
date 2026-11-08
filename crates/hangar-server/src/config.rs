//! A configuração mora no `hangar-core`; aqui ficam só as portas do Connect, que são do `hangar-connect`.
use std::net::SocketAddr;

pub use hangar_core::config::*;

/// Entrada do Connect, sempre no loopback. Com a porta principal na 8768, ela fica com o cliente
/// real e o Connect não liga.
pub fn connect_listen(listen: SocketAddr) -> Option<SocketAddr> {
    if listen.port() == crate::connect::CONNECT_PORT {
        tracing::warn!(code = "connect_port_is_main", "CP_PORT é a porta do Connect: a entrada do Connect fica fechada");
        return None;
    }
    Some(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, crate::connect::CONNECT_PORT)))
}

/// Porta da prévia do Connect, sempre no loopback. Com a porta principal nela, a prévia não liga.
pub fn preview_listen(listen: SocketAddr) -> Option<SocketAddr> {
    let port = crate::connect::preview::PREVIEW_PORT;
    (listen.port() != port).then(|| SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)))
}
