//! Gravação no Postgres: um pacote por transação, `batch_id` como chave da repetição.
use hangar_telemetry::wire::Batch;
use sqlx::PgPool;
use sqlx::types::Json;

/// Dias guardados; o mais antigo sai na tarefa diária.
const RETENTION_DAYS: i32 = 400;

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub enum Insert {
    /// Gravado agora, ou já gravado antes com o mesmo `batch_id` (o servidor repete quando não viu a resposta).
    Accepted,
    /// `day` mais de `FUTURE_SLACK_DAYS` à frente de hoje em UTC.
    DayAhead,
}

/// Folga para relógio torto na máquina que manda.
const FUTURE_SLACK_DAYS: i32 = 2;

/// Grava o pacote já conferido por `Batch::validate`; `count` acima de `i64::MAX` é recusado antes.
pub async fn insert(pool: &PgPool, batch: &Batch) -> Result<Insert, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let env = &batch.env;
    let inserted = sqlx::query(
        r#"INSERT INTO batches (batch_id, machine_id, owner_id, day, version, "commit", os, arch, channel)
           SELECT $1, $2, $3, $4::date, $5, $6, $7, $8, $9
           WHERE $4::date <= (now() AT TIME ZONE 'utc')::date + $10
           ON CONFLICT (batch_id) DO NOTHING"#,
    )
    .bind(&batch.batch_id)
    .bind(&batch.machine_id)
    .bind(&batch.owner_id)
    .bind(&batch.day)
    .bind(&env.version)
    .bind(&env.commit)
    .bind(&env.os)
    .bind(&env.arch)
    .bind(&env.channel)
    .bind(FUTURE_SLACK_DAYS)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        // Nada gravado: ou o batch_id já existe (repetição), ou o dia está à frente demais.
        let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM batches WHERE batch_id = $1)")
            .bind(&batch.batch_id)
            .fetch_one(&mut *tx)
            .await?;
        return Ok(if known { Insert::Accepted } else { Insert::DayAhead });
    }
    let counters = &batch.counters;
    let events: Vec<&str> = counters.iter().map(|c| c.event.as_str()).collect();
    let dims: Vec<_> = counters.iter().map(|c| Json(&c.dims)).collect();
    let buckets: Vec<Option<&str>> = counters.iter().map(|c| c.bucket.as_deref()).collect();
    let counts: Vec<i64> = counters.iter().map(|c| c.count as i64).collect();
    sqlx::query(
        "INSERT INTO counters (batch_id, event, dims, bucket, count)
         SELECT $1, event, dims, bucket, count
         FROM UNNEST($2::text[], $3::jsonb[], $4::text[], $5::bigint[]) AS c(event, dims, bucket, count)",
    )
    .bind(&batch.batch_id)
    .bind(events)
    .bind(dims)
    .bind(buckets)
    .bind(counts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Insert::Accepted)
}

/// Valor que o Postgres recusou por conteúdo (classe 22, como `2026-02-31` no `day`): erro de quem mandou.
pub fn is_bad_value(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().is_some_and(|code| code.starts_with("22")))
}

pub async fn prune(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM batches WHERE day < (now() AT TIME ZONE 'utc')::date - $1")
        .bind(RETENTION_DAYS)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
