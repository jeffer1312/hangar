//! Servidor de teste da voz: home temporário e Python falso atrás.
use std::{net::SocketAddr, path::Path};

pub async fn server(home: &Path) -> SocketAddr {
    let (_python, upstream) = crate::fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(crate::fake::config(upstream, ""));
    state.voice = std::sync::Arc::new(hangar_server::voice::hub::VoiceHub::for_test(home.to_path_buf(), home.join(".claude"), Some(vec!["default".into()])));
    crate::fake::spawn_state(state).await
}
