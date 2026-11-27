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

/// Valor que o Postgres recusou por conteúdo (classe 22, como uma data que não existe no calendário no `day`): erro de quem mandou.
pub fn is_bad_value(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().is_some_and(|code| code.starts_with("22")))
}

/// Resumo do painel dos últimos `days` dias (UTC), tudo calculado no banco.
/// Primeiro dia do período; `concat!` porque o sqlx só aceita SQL estático.
macro_rules! since {
    // Hoje e os `days - 1` dias antes dele: "7 dias" são 7 dias UTC.
    () => { "(now() AT TIME ZONE 'utc')::date - $1::int + 1" };
}

pub async fn summary(pool: &PgPool, days: i32) -> Result<serde_json::Value, sqlx::Error> {
    // Uma foto só do banco: um pacote que chega no meio não descasa os números entre si.
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *tx).await?;
    let pool = &mut *tx;
    let active: (i64, i64, i64, i64) = sqlx::query_as("SELECT machines_7d, machines_30d, owners_7d, owners_30d FROM v_active")
        .fetch_one(&mut *pool)
        .await?;
    let (batches, machines): (i64, i64) =
        sqlx::query_as(concat!("SELECT count(*), count(DISTINCT machine_id) FROM batches WHERE day >= ", since!()))
            .bind(days)
            .fetch_one(&mut *pool)
            .await?;
    let daily: Vec<(String, i64)> = sqlx::query_as(concat!(
        "SELECT to_char(day, 'YYYY-MM-DD'), count(DISTINCT machine_id) FROM batches WHERE day >= ", since!(), " GROUP BY day ORDER BY day"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Versão, sistema e canal de cada máquina pelo último pacote dela no período.
    let env: Vec<(String, String, i64)> = sqlx::query_as(concat!(
        "WITH last AS (SELECT DISTINCT ON (machine_id) version, os, channel FROM batches WHERE day >= ", since!(), "
                       ORDER BY machine_id, day DESC, received_at DESC)
         SELECT k, v, count(*) FROM last, LATERAL (VALUES ('version', version), ('os', os), ('channel', channel)) AS x(k, v)
         GROUP BY k, v ORDER BY k, 3 DESC, v"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    let per_owner: Vec<(i64, i64)> =
        sqlx::query_as("SELECT machines, count(*) FROM v_machines_per_owner GROUP BY machines ORDER BY machines").fetch_all(&mut *pool).await?;
    let counters: Vec<(String, Json<serde_json::Value>, Option<String>, i64, i64)> = sqlx::query_as(concat!(
        "SELECT c.event, c.dims, c.bucket, sum(c.count)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE b.day >= ", since!(), "
         GROUP BY c.event, c.dims, c.bucket ORDER BY c.event, 4 DESC"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Máquinas distintas por combinação e por faixa: somar as linhas contaria a mesma máquina de novo.
    let by_dims: Vec<(String, Json<serde_json::Value>, i64, i64)> = sqlx::query_as(concat!(
        "SELECT c.event, c.dims, sum(c.count)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE b.day >= ", since!(), "
         GROUP BY c.event, c.dims ORDER BY c.event, 3 DESC"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    let by_bucket: Vec<(String, String, i64, i64)> = sqlx::query_as(concat!(
        "SELECT c.event, c.bucket, sum(c.count)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE b.day >= ", since!(), " AND c.bucket IS NOT NULL
         GROUP BY c.event, c.bucket ORDER BY c.event"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    tx.commit().await?;
    use serde_json::json;
    Ok(json!({
        "days": days,
        "active": {"machines_7d": active.0, "machines_30d": active.1, "owners_7d": active.2, "owners_30d": active.3},
        "period": {"batches": batches, "machines": machines},
        "daily": daily.into_iter().map(|(day, machines)| json!({"day": day, "machines": machines})).collect::<Vec<_>>(),
        "env": env.into_iter().map(|(key, value, machines)| json!({"key": key, "value": value, "machines": machines})).collect::<Vec<_>>(),
        "per_owner": per_owner.into_iter().map(|(machines, owners)| json!({"machines": machines, "owners": owners})).collect::<Vec<_>>(),
        "by_dims": by_dims.into_iter().map(|(event, dims, count, machines)|
            json!({"event": event, "dims": dims.0, "count": count, "machines": machines})).collect::<Vec<_>>(),
        "by_bucket": by_bucket.into_iter().map(|(event, bucket, count, machines)|
            json!({"event": event, "bucket": bucket, "count": count, "machines": machines})).collect::<Vec<_>>(),
        "counters": counters.into_iter().map(|(event, dims, bucket, count, machines)|
            json!({"event": event, "dims": dims.0, "bucket": bucket, "count": count, "machines": machines})).collect::<Vec<_>>(),
    }))
}

pub async fn prune(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM batches WHERE day < (now() AT TIME ZONE 'utc')::date - $1")
        .bind(RETENTION_DAYS)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
