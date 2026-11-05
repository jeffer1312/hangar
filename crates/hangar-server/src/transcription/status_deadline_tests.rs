use super::*;
use crate::{
    auth::TrustedHosts,
    config::Config,
    routes::{AppState, router},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[tokio::test]
async fn public_status_deadline_includes_waiting_for_shared_configuration() {
    const CHILD: &str = "HANGAR_STATUS_DEADLINE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let isolated = tempfile::tempdir().unwrap();
        let proof = isolated.path().join("status-deadline-proof");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "transcription::service::public_status_deadline_tests::public_status_deadline_includes_waiting_for_shared_configuration",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD, "1")
            .env("HANGAR_STATUS_DEADLINE_PROOF", &proof)
            .env("HOME", isolated.path())
            .env("USERPROFILE", isolated.path())
            .env("APPDATA", isolated.path())
            .env("LOCALAPPDATA", isolated.path())
            .env("XDG_CONFIG_HOME", isolated.path())
            .env("XDG_CACHE_HOME", isolated.path())
            .env("TMPDIR", isolated.path())
            .env("TEMP", isolated.path())
            .env("TMP", isolated.path());
        if let Some(root) = std::env::var_os("SystemRoot") {
            child.env("SystemRoot", root);
        }
        let result = child.output().unwrap();
        assert!(
            result.status.success(),
            "teste isolado falhou: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            proof.is_file(),
            "o processo filho não executou a prova de status"
        );
        return;
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = upstream.local_addr().unwrap();
    let source = tokio::spawn(async move {
        axum::serve(
            upstream,
            axum::Router::new().route(
                "/internal/transcription/config",
                axum::routing::get(move || {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async {
                        (
                            [("content-type", "application/json")],
                            json!({"providers":[]}).to_string(),
                        )
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let public = listener.local_addr().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config {
        listen: public,
        upstream: address,
        auth_token: "owner".into(),
        internal_secret: "private".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.hangar_dir = temporary.path().to_owned();
    let state = Arc::new(state);
    let app = router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let guard = state.transcription.snapshot.read().await;
    assert!(state.transcription.snapshot.try_read().is_ok());
    let request = tokio::spawn(async move {
        reqwest::Client::new()
            .get(format!(
                "http://{public}/api/transcription/providers/status"
            ))
            .bearer_auth("owner")
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        // O escritor na fila comprova que o corpo HTTP já foi consumido; contar a chamada não comprova.
        while calls.load(Ordering::SeqCst) == 0
            || state.transcription.snapshot.try_read().is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    let response = tokio::time::timeout(Duration::from_secs(2), request).await;
    tokio::time::resume();
    drop(guard);
    let response = response
        .expect("o status ultrapassou o teto público de15s esperando configuração")
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"detail": {"code": "transcription_rust_unavailable", "params": {}, "msg": "Não foi possível confirmar a transcrição no Rust. O áudio foi preservado para tentar novamente."}})
    );
    assert_eq!(state.transcription_slots.available_permits(), 4);
    assert_eq!(
        reqwest::Client::new()
            .get(format!(
                "http://{public}/api/transcription/providers/status"
            ))
            .bearer_auth("owner")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.abort();
    source.abort();
    std::fs::write(
        std::env::var_os("HANGAR_STATUS_DEADLINE_PROOF").unwrap(),
        b"ok",
    )
    .unwrap();
}
