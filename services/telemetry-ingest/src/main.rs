//! Receptor da telemetria: `POST /v1/batch` grava o pacote diário de cada máquina no Postgres.
//! Não lê IP nem cabeçalho de origem; o limite de pedidos fica com a Cloudflare.
mod store;

use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hangar_telemetry::wire::Batch;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::signal::unix::{SignalKind, signal};

const MAX_BODY: usize = 64 * 1024;

fn app(pool: PgPool) -> Router {
    Router::new()
        .route("/v1/batch", post(post_batch))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(pool)
}

fn reply(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn refused(status: StatusCode, field: &str) -> Response {
    reply(status, format!(r#"{{"ok":false,"error":"{field}"}}"#))
}

/// O corpo vem como bytes para que pacote fora da forma seja 400, e não o 422 do `Json` do axum.
async fn post_batch(State(pool): State<PgPool>, body: Bytes) -> Response {
    let Ok(batch) = serde_json::from_slice::<Batch>(&body) else {
        return refused(StatusCode::BAD_REQUEST, "json");
    };
    if let Err(field) = batch.validate() {
        return refused(StatusCode::BAD_REQUEST, field);
    }
    if batch.counters.iter().any(|c| i64::try_from(c.count).is_err()) {
        return refused(StatusCode::BAD_REQUEST, "count");
    }
    match store::insert(&pool, &batch).await {
        Ok(store::Insert::Accepted) => reply(StatusCode::OK, r#"{"ok":true}"#.to_owned()),
        Ok(store::Insert::DayAhead) => refused(StatusCode::BAD_REQUEST, "day"),
        Err(error) if store::is_bad_value(&error) => refused(StatusCode::BAD_REQUEST, "value"),
        Err(error) => {
            eprintln!("telemetry-ingest: insert: {error}");
            refused(StatusCode::SERVICE_UNAVAILABLE, "store")
        }
    }
}

async fn retention(pool: PgPool) {
    let mut daily = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    loop {
        daily.tick().await;
        match store::prune(&pool).await {
            Ok(removed) => eprintln!("telemetry-ingest: retention removed {removed} batches"),
            Err(error) => eprintln!("telemetry-ingest: retention: {error}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let listen = std::env::var("LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let pool = PgPoolOptions::new().max_connections(5).connect(&database_url).await?;
    store::migrate(&pool).await?;
    tokio::spawn(retention(pool.clone()));
    // Instalado antes de ouvir: falha aqui derruba a subida, não o desligamento.
    let mut terminate = signal(SignalKind::terminate())?;
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("telemetry-ingest: listening on {listen}");
    // O pedido em curso termina de gravar antes de o processo sair.
    let stop = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    };
    axum::serve(listener, app(pool)).with_graceful_shutdown(stop).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::hash::{BuildHasher, RandomState};

    use axum::body::Body;
    use axum::http::Request;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    /// Pool que nunca conecta: os pedidos recusados antes do banco não precisam dele.
    fn offline() -> PgPool {
        PgPoolOptions::new().connect_lazy("postgres://telemetry@127.0.0.1:1/unused").unwrap()
    }

    async fn online() -> PgPool {
        let url = std::env::var("TELEMETRY_TEST_DATABASE_URL").expect("TELEMETRY_TEST_DATABASE_URL");
        let pool = PgPoolOptions::new().connect(&url).await.unwrap();
        store::migrate(&pool).await.unwrap();
        pool
    }

    /// Hoje em UTC mais `offset` dias, pelo relógio do banco que confere o dia.
    async fn utc_day(pool: &PgPool, offset: i32) -> String {
        sqlx::query_scalar("SELECT to_char((now() AT TIME ZONE 'utc')::date + $1, 'YYYY-MM-DD')")
            .bind(offset)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Id aleatório por chamada, para os testes dividirem o mesmo banco entre rodadas sem colidir.
    fn fresh_id() -> String {
        let random = || RandomState::new().hash_one(0u8);
        format!("{:016x}{:016x}", random(), random())
    }

    fn batch_body(batch_id: &str, day: &str) -> Value {
        json!({
            "schema": 1,
            "batch_id": batch_id,
            "machine_id": fresh_id(),
            "owner_id": fresh_id(),
            "day": day,
            "env": {"version": "0.42.0", "commit": "dev", "os": "linux", "arch": "x86_64", "channel": "latest"},
            "counters": [
                {"event": "future_event", "dims": {"new_dim": "new_value"}, "bucket": "lt3s", "count": 7},
                {"event": "peers", "count": 2}
            ]
        })
    }

    async fn send(pool: PgPool, body: impl Into<Body>) -> (StatusCode, String) {
        let request = Request::post("/v1/batch").header(header::CONTENT_TYPE, "application/json").body(body.into()).unwrap();
        let response = app(pool).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn post_json(pool: PgPool, body: &Value) -> (StatusCode, String) {
        send(pool, serde_json::to_vec(body).unwrap()).await
    }

    #[tokio::test]
    async fn body_over_64_kb_is_413() {
        let (status, _) = send(offline(), vec![b' '; MAX_BODY + 1]).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        // No limite ainda passa pelo leitor: espaço só não é JSON.
        let (status, _) = send(offline(), vec![b' '; MAX_BODY]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_machine_id_is_400() {
        let mut body = batch_body(&fresh_id(), "2026-11-26");
        body.as_object_mut().unwrap().remove("machine_id");
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(text, r#"{"ok":false,"error":"json"}"#);
    }

    #[tokio::test]
    async fn malformed_fields_are_400() {
        let mut body = batch_body("NOT-HEX", "2026-11-26");
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"batch_id"}"#));
        body["batch_id"] = json!(fresh_id());
        body["counters"][0]["count"] = json!(u64::MAX);
        let (status, text) = post_json(offline(), &body).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"count"}"#));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn unknown_event_is_stored_as_sent() {
        let pool = online().await;
        let id = fresh_id();
        let (status, text) = post_json(pool.clone(), &batch_body(&id, &utc_day(&pool, -1).await)).await;
        assert_eq!((status, text.as_str()), (StatusCode::OK, r#"{"ok":true}"#));
        let rows: Vec<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT event, dims->>'new_dim', bucket, count FROM counters WHERE batch_id = $1 ORDER BY event",
        )
        .bind(&id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![
            ("future_event".to_owned(), Some("new_value".to_owned()), Some("lt3s".to_owned()), 7),
            ("peers".to_owned(), None, None, 2),
        ]);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn repeated_batch_id_is_200_and_not_stored_twice() {
        let pool = online().await;
        let id = fresh_id();
        let body = batch_body(&id, &utc_day(&pool, -1).await);
        for _ in 0..2 {
            let (status, text) = post_json(pool.clone(), &body).await;
            assert_eq!((status, text.as_str()), (StatusCode::OK, r#"{"ok":true}"#));
        }
        let batches: i64 = sqlx::query_scalar("SELECT count(*) FROM batches WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        let counters: i64 = sqlx::query_scalar("SELECT count(*) FROM counters WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!((batches, counters), (1, 2));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn day_too_far_ahead_is_400() {
        let pool = online().await;
        let (status, _) = post_json(pool.clone(), &batch_body(&fresh_id(), &utc_day(&pool, 2).await)).await;
        assert_eq!(status, StatusCode::OK);
        let id = fresh_id();
        let (status, text) = post_json(pool.clone(), &batch_body(&id, &utc_day(&pool, 3).await)).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"day"}"#));
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM batches WHERE batch_id = $1").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(stored, 0);
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn impossible_calendar_day_is_400() {
        let (status, text) = post_json(online().await, &batch_body(&fresh_id(), "2026-02-31")).await;
        assert_eq!((status, text.as_str()), (StatusCode::BAD_REQUEST, r#"{"ok":false,"error":"value"}"#));
    }

    #[tokio::test]
    #[ignore = "precisa de TELEMETRY_TEST_DATABASE_URL"]
    async fn retention_drops_batches_older_than_400_days() {
        let pool = online().await;
        let yesterday = utc_day(&pool, -1).await;
        let (old, recent) = (fresh_id(), fresh_id());
        for (id, day) in [(&old, "2000-01-01"), (&recent, yesterday.as_str())] {
            assert_eq!(post_json(pool.clone(), &batch_body(id, day)).await.0, StatusCode::OK);
        }
        store::prune(&pool).await.unwrap();
        let left: Vec<String> = sqlx::query_scalar("SELECT batch_id FROM batches WHERE batch_id = $1 OR batch_id = $2")
            .bind(&old)
            .bind(&recent)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, vec![recent]);
        let orphans: i64 = sqlx::query_scalar("SELECT count(*) FROM counters WHERE batch_id = $1").bind(&old).fetch_one(&pool).await.unwrap();
        assert_eq!(orphans, 0);
    }
}
