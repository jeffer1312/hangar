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
    /// Gravado agora, ou trocado pelo que já havia com o mesmo `batch_id` (retrato da hora, envio final
    /// ou repetição de quem não viu a resposta).
    Accepted,
    /// `day` mais de `FUTURE_SLACK_DAYS` à frente de hoje em UTC.
    DayAhead,
}

/// Folga para relógio torto na máquina que manda.
const FUTURE_SLACK_DAYS: i32 = 2;

/// Grava o pacote já conferido por `Batch::validate`, que limita cada `count` a `MAX_COUNT`.
/// O mesmo `batch_id` de novo troca tudo o que ele tinha, na mesma transação: os retratos da hora e o
/// envio final nunca somam duas vezes. Não troca se a máquina ou o dia mudaram, nem o final por um retrato.
pub async fn insert(pool: &PgPool, batch: &Batch) -> Result<Insert, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let env = &batch.env;
    let inserted = sqlx::query(
        r#"INSERT INTO batches (batch_id, machine_id, owner_id, day, version, "commit", os, arch, channel, partial)
           SELECT $1, $2, $3, $4::date, $5, $6, $7, $8, $9, $11
           WHERE $4::date <= (now() AT TIME ZONE 'utc')::date + $10
           ON CONFLICT (batch_id) DO UPDATE SET owner_id = EXCLUDED.owner_id, version = EXCLUDED.version,
               "commit" = EXCLUDED."commit", os = EXCLUDED.os, arch = EXCLUDED.arch, channel = EXCLUDED.channel,
               partial = EXCLUDED.partial, received_at = now()
           WHERE batches.machine_id = EXCLUDED.machine_id AND batches.day = EXCLUDED.day
             AND (batches.partial OR NOT EXCLUDED.partial)"#,
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
    .bind(batch.partial)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        // Nada gravado: ou o batch_id já existe e não pode ser trocado, ou o dia está à frente demais.
        let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM batches WHERE batch_id = $1)")
            .bind(&batch.batch_id)
            .fetch_one(&mut *tx)
            .await?;
        return Ok(if known { Insert::Accepted } else { Insert::DayAhead });
    }
    sqlx::query(
        "WITH c AS (DELETE FROM counters WHERE batch_id = $1), l AS (DELETE FROM logs WHERE batch_id = $1),
              r AS (DELETE FROM records WHERE batch_id = $1)
         DELETE FROM trail_steps WHERE batch_id = $1",
    )
    .bind(&batch.batch_id)
    .execute(&mut *tx)
    .await?;
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
    if !batch.logs.is_empty() {
        let logs = &batch.logs;
        let levels: Vec<&str> = logs.iter().map(|l| l.level.as_str()).collect();
        let targets: Vec<&str> = logs.iter().map(|l| l.target.as_str()).collect();
        let messages: Vec<&str> = logs.iter().map(|l| l.message.as_str()).collect();
        let counts: Vec<i64> = logs.iter().map(|l| l.count as i64).collect();
        sqlx::query(
            "INSERT INTO logs (batch_id, level, target, message, count)
             SELECT $1, level, target, message, count
             FROM UNNEST($2::text[], $3::text[], $4::text[], $5::bigint[]) AS l(level, target, message, count)",
        )
        .bind(&batch.batch_id)
        .bind(levels)
        .bind(targets)
        .bind(messages)
        .bind(counts)
        .execute(&mut *tx)
        .await?;
    }
    if !batch.records.is_empty() {
        let records = &batch.records;
        let levels: Vec<&str> = records.iter().map(|r| r.level.as_str()).collect();
        let events: Vec<&str> = records.iter().map(|r| r.event.as_str()).collect();
        let counts: Vec<i64> = records.iter().map(|r| r.count as i64).collect();
        let firsts: Vec<&str> = records.iter().map(|r| r.first.as_str()).collect();
        let lasts: Vec<&str> = records.iter().map(|r| r.last.as_str()).collect();
        let fields: Vec<_> = records.iter().map(|r| Json(&r.fields)).collect();
        let trails: Vec<_> = records.iter().map(|r| Json(&r.trail)).collect();
        sqlx::query(
            "INSERT INTO records (batch_id, level, event, count, first_at, last_at, fields, trail)
             SELECT $1, level, event, count, first_at::time, last_at::time, fields, trail
             FROM UNNEST($2::text[], $3::text[], $4::bigint[], $5::text[], $6::text[], $7::jsonb[], $8::jsonb[])
                  AS r(level, event, count, first_at, last_at, fields, trail)",
        )
        .bind(&batch.batch_id)
        .bind(levels)
        .bind(events)
        .bind(counts)
        .bind(firsts)
        .bind(lasts)
        .bind(fields)
        .bind(trails)
        .execute(&mut *tx)
        .await?;
    }
    if !batch.trail.is_empty() {
        let steps = &batch.trail;
        let times: Vec<&str> = steps.iter().map(|s| s.t.as_str()).collect();
        let events: Vec<&str> = steps.iter().map(|s| s.event.as_str()).collect();
        let levels: Vec<&str> = steps.iter().map(|s| s.level.as_str()).collect();
        let screens: Vec<Option<&str>> = steps.iter().map(|s| s.screen.as_deref()).collect();
        let counts: Vec<i64> = steps.iter().map(|s| s.count as i64).collect();
        sqlx::query(
            "INSERT INTO trail_steps (batch_id, seq, t, event, level, screen, count)
             SELECT $1, s.seq::int, s.t::time, s.event, s.level, s.screen, s.count
             FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::bigint[]) WITH ORDINALITY
                  AS s(t, event, level, screen, count, seq)",
        )
        .bind(&batch.batch_id)
        .bind(times)
        .bind(events)
        .bind(levels)
        .bind(screens)
        .bind(counts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Insert::Accepted)
}

/// Valor que o Postgres recusou por conteúdo (classe 22, como uma data que não existe no calendário no `day`): erro de quem mandou.
pub fn is_bad_value(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().is_some_and(|code| code.starts_with("22")))
}

/// Assinatura de falha agregada: nível, evento, codigo, etapa, erro_tipo, vezes, máquinas, versões,
/// sistemas, primeira e última vez, campos e passos da amostra, total de assinaturas antes do corte.
type RecordRow = (
    String, String, Option<String>, Option<String>, Option<String>, i64, i64, Vec<String>, Vec<String>, String, String,
    Json<serde_json::Value>, Json<serde_json::Value>, i64,
);

/// Hoje e os `days - 1` dias antes dele, em UTC; dia à frente (relógio adiantado) só conta quando chegar.
/// `concat!` porque o sqlx só aceita SQL estático.
macro_rules! in_period {
    ($day:literal) => {
        concat!($day, " BETWEEN (now() AT TIME ZONE 'utc')::date - $1::int + 1 AND (now() AT TIME ZONE 'utc')::date")
    };
}

/// Resumo do painel dos últimos `days` dias (UTC), tudo calculado no banco. Cada soma para no maior
/// `bigint`: o `validate` limita cada pacote, mas pacote antigo no banco não pode derrubar o resumo.
pub async fn summary(pool: &PgPool, days: i32) -> Result<serde_json::Value, sqlx::Error> {
    // Uma foto só do banco: um pacote que chega no meio não descasa os números entre si.
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *tx).await?;
    let pool = &mut *tx;
    let active: (i64, i64, i64, i64) = sqlx::query_as("SELECT machines_7d, machines_30d, owners_7d, owners_30d FROM v_active")
        .fetch_one(&mut *pool)
        .await?;
    let (batches, machines): (i64, i64) =
        sqlx::query_as(concat!("SELECT count(*), count(DISTINCT machine_id) FROM batches WHERE ", in_period!("day")))
            .bind(days)
            .fetch_one(&mut *pool)
            .await?;
    let daily: Vec<(String, i64)> = sqlx::query_as(concat!(
        "SELECT to_char(day, 'YYYY-MM-DD'), count(DISTINCT machine_id) FROM batches WHERE ", in_period!("day"), " GROUP BY day ORDER BY day"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Versão, sistema e canal de cada máquina pelo último pacote dela no período.
    let env: Vec<(String, String, i64)> = sqlx::query_as(concat!(
        "WITH last AS (SELECT DISTINCT ON (machine_id) version, os, channel FROM batches WHERE ", in_period!("day"), "
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
        "SELECT c.event, c.dims, c.bucket, LEAST(sum(c.count), 9223372036854775807)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE ", in_period!("b.day"), "
         GROUP BY c.event, c.dims, c.bucket ORDER BY c.event, 4 DESC"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Máquinas distintas por combinação e por faixa: somar as linhas contaria a mesma máquina de novo.
    let by_dims: Vec<(String, Json<serde_json::Value>, i64, i64)> = sqlx::query_as(concat!(
        "SELECT c.event, c.dims, LEAST(sum(c.count), 9223372036854775807)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE ", in_period!("b.day"), "
         GROUP BY c.event, c.dims ORDER BY c.event, 3 DESC"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    let by_bucket: Vec<(String, String, i64, i64)> = sqlx::query_as(concat!(
        "SELECT c.event, c.bucket, LEAST(sum(c.count), 9223372036854775807)::bigint, count(DISTINCT b.machine_id)
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE ", in_period!("b.day"), " AND c.bucket IS NOT NULL
         GROUP BY c.event, c.bucket ORDER BY c.event"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    let logs: Vec<(String, String, String, i64, i64, String, String, Vec<String>)> = sqlx::query_as(concat!(
        "SELECT l.level, l.target, l.message, LEAST(sum(l.count), 9223372036854775807)::bigint, count(DISTINCT b.machine_id),
                to_char(min(b.day), 'YYYY-MM-DD'), to_char(max(b.day), 'YYYY-MM-DD'), array_agg(DISTINCT b.version ORDER BY b.version)
         FROM logs l JOIN batches b ON b.batch_id = l.batch_id
         WHERE ", in_period!("b.day"), "
         GROUP BY l.level, l.target, l.message ORDER BY 5 DESC, 4 DESC LIMIT 300"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Total de cada evento por dia: as minilinhas dos cartões e os gráficos de uso por dia.
    // Com as dimensões: a falha por dia inclui a linha que diz erro (ferramenta com erro, passo que falhou).
    let trend: Vec<(String, String, Json<serde_json::Value>, i64)> = sqlx::query_as(concat!(
        "SELECT to_char(b.day, 'YYYY-MM-DD'), c.event, c.dims, LEAST(sum(c.count), 9223372036854775807)::bigint
         FROM counters c JOIN batches b ON b.batch_id = c.batch_id
         WHERE ", in_period!("b.day"), "
         GROUP BY b.day, c.event, c.dims ORDER BY b.day"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Falhas do diário de todas as máquinas, uma linha por assinatura; a amostra é a vez mais recente.
    let records: Vec<RecordRow> = sqlx::query_as(concat!(
        "WITH r AS (
             SELECT r.level, r.event, r.count, r.fields, r.trail, b.machine_id, b.version, b.os,
                    b.day + r.first_at AS first_ts, b.day + r.last_at AS last_ts
             FROM records r JOIN batches b ON b.batch_id = r.batch_id
             WHERE ", in_period!("b.day"), ")
         SELECT CASE WHEN bool_or(level = 'error') THEN 'error' ELSE 'warn' END, event,
                fields->>'codigo', fields->>'etapa', fields->>'erro_tipo',
                LEAST(sum(count), 9223372036854775807)::bigint, count(DISTINCT machine_id),
                array_agg(DISTINCT version ORDER BY version), array_agg(DISTINCT os ORDER BY os),
                to_char(min(first_ts), 'YYYY-MM-DD HH24:MI:SS'), to_char(max(last_ts), 'YYYY-MM-DD HH24:MI:SS'),
                (array_agg(fields ORDER BY last_ts DESC))[1], (array_agg(trail ORDER BY last_ts DESC))[1],
                count(*) OVER ()
         FROM r GROUP BY event, fields->>'codigo', fields->>'etapa', fields->>'erro_tipo'
         ORDER BY 7 DESC, 6 DESC, event LIMIT 300"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    let records_total = records.first().map_or(0, |r| r.13);
    // Passagens de um passo ao seguinte dentro do dia de cada máquina.
    let transitions: Vec<(String, String, i64, i64)> = sqlx::query_as(concat!(
        "WITH s AS (
             SELECT b.machine_id, t.event, lag(t.event) OVER (PARTITION BY t.batch_id ORDER BY t.seq) AS prev
             FROM trail_steps t JOIN batches b ON b.batch_id = t.batch_id
             WHERE ", in_period!("b.day"), ")
         SELECT prev, event, count(*), count(DISTINCT machine_id) FROM s
         WHERE prev IS NOT NULL AND prev <> event
         GROUP BY prev, event ORDER BY 3 DESC, 4 DESC, prev, event LIMIT 50"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    // Os dias de cada máquina que têm caminho, para o painel escolher um e pedir `trail`.
    let trail_days: Vec<(String, String, String, String, i64, i64)> = sqlx::query_as(concat!(
        "SELECT b.machine_id, to_char(b.day, 'YYYY-MM-DD'), (array_agg(b.os ORDER BY b.received_at DESC))[1],
                (array_agg(b.version ORDER BY b.received_at DESC))[1], count(*), count(*) FILTER (WHERE t.level <> 'ok')
         FROM trail_steps t JOIN batches b ON b.batch_id = t.batch_id
         WHERE ", in_period!("b.day"), "
         GROUP BY b.machine_id, b.day ORDER BY b.day DESC, 5 DESC LIMIT 500"
    ))
    .bind(days)
    .fetch_all(&mut *pool)
    .await?;
    tx.commit().await?;
    use serde_json::json;
    Ok(json!({
        "records": records.into_iter().map(|r| json!({
            "level": r.0, "event": r.1, "codigo": r.2, "etapa": r.3, "erro_tipo": r.4, "count": r.5, "machines": r.6,
            "versions": r.7, "os": r.8, "first": r.9, "last": r.10, "fields": r.11.0, "trail": r.12.0,
        })).collect::<Vec<_>>(),
        "records_total": records_total,
        "transitions": transitions.into_iter().map(|(from, to, count, machines)|
            json!({"from": from, "to": to, "count": count, "machines": machines})).collect::<Vec<_>>(),
        "trail_days": trail_days.into_iter().map(|(machine, day, os, version, steps, problems)| json!({
            "machine": machine, "label": machine.get(..8).unwrap_or(&machine).to_owned(), "day": day, "os": os, "version": version,
            "steps": steps, "problems": problems,
        })).collect::<Vec<_>>(),
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
        "logs": logs.into_iter().map(|(level, target, message, count, machines, first, last, versions)| json!({
            "level": level, "target": target, "message": message, "count": count, "machines": machines,
            "first_day": first, "last_day": last, "versions": versions,
        })).collect::<Vec<_>>(),
        "trend": trend.into_iter().map(|(day, event, dims, count)| json!({"day": day, "event": event, "dims": dims.0, "count": count})).collect::<Vec<_>>(),
        "counters": counters.into_iter().map(|(event, dims, bucket, count, machines)|
            json!({"event": event, "dims": dims.0, "bucket": bucket, "count": count, "machines": machines})).collect::<Vec<_>>(),
    }))
}

/// Os passos de uma máquina num dia, na ordem; dia sem caminho é lista vazia.
pub async fn trail(pool: &PgPool, machine: &str, day: &str) -> Result<serde_json::Value, sqlx::Error> {
    let steps: Vec<(String, String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT to_char(t.t, 'HH24:MI:SS'), t.event, t.level, t.screen, t.count
         FROM trail_steps t JOIN batches b ON b.batch_id = t.batch_id
         WHERE b.machine_id = $1 AND b.day = $2::date
         ORDER BY t.t, b.batch_id, t.seq",
    )
    .bind(machine)
    .bind(day)
    .fetch_all(pool)
    .await?;
    Ok(serde_json::json!({
        "machine": machine, "day": day,
        "steps": steps.into_iter().map(|(t, event, level, screen, count)|
            serde_json::json!({"t": t, "event": event, "level": level, "screen": screen, "count": count})).collect::<Vec<_>>(),
    }))
}

pub async fn prune(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM batches WHERE day < (now() AT TIME ZONE 'utc')::date - $1")
        .bind(RETENTION_DAYS)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
