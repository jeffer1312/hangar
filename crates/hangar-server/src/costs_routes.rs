//! Custos e cotação para o dono. Falha do Rust é 503 com código, nunca repasse ao Python.

use crate::costs::collect::{CollectError, Ready};
use crate::costs::index::IndexError;
use crate::costs::py::LocalTs;
use crate::costs::{CacheKey, codex, report_costs, report_uso, session_cost};
use crate::costs_failure::FailureReason;
use crate::routes::{AppState, cors, gate, maybe_gzip, pass, route_failed};
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

pub(crate) fn warming(read: usize, total: usize) -> Response {
    (
        StatusCode::ACCEPTED,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"aquecendo":true, "lidos":read, "total":total}).to_string(),
    )
        .into_response()
}

fn response(headers: &HeaderMap, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::empty());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let body = maybe_gzip(headers, response.headers_mut(), body);
    *response.body_mut() = Body::from(body);
    cors(headers, response.headers_mut());
    response
}

fn dimension_finite(bucket: &report_costs::DimBucket) -> bool {
    [
        bucket.cost,
        bucket.cost_input,
        bucket.cost_output,
        bucket.cost_cache_write,
        bucket.cost_cache_read,
        bucket.custo_regravado,
    ]
    .iter()
    .all(|value| value.is_finite())
}

fn costs_finite(report: &report_costs::CostReport) -> bool {
    dimension_finite(&report.totals)
        && [
            &report.by_day,
            &report.by_provider,
            &report.by_source,
            &report.by_project,
            &report.by_model,
        ]
        .iter()
        .all(|buckets| buckets.iter().all(dimension_finite))
        && report.anterior.as_ref().is_none_or(dimension_finite)
        && report.by_kind.iter().all(|bucket| bucket.cost.is_finite())
        && report.rates.iter().all(|rate| {
            [rate.input, rate.output, rate.cache_read, rate.cache_write]
                .iter()
                .all(|value| value.is_finite())
        })
        && report.combos.iter().all(|row| {
            [
                row.custo_sem_cache,
                row.equivalente_cobrado,
                row.cost,
                row.cost_input,
                row.cost_output,
                row.cost_cache_write,
                row.cost_cache_read,
                row.custo_regravado,
            ]
            .iter()
            .all(|value| value.is_finite())
        })
        && report
            .sessoes
            .iter()
            .all(|row| row.cost.is_finite() && row.custo_regravado.is_finite())
        && report.custo_sem_cache.is_finite()
        && report.usd_brl.is_none_or(f64::is_finite)
}

fn summary_finite(report: &report_costs::SummaryReport) -> bool {
    dimension_finite(&report.totals)
        && [&report.by_day, &report.by_model].iter().all(|buckets| buckets.iter().all(dimension_finite))
        && report.usd_brl.is_none_or(f64::is_finite)
}

fn usage_finite(report: &report_uso::UsoReport) -> bool {
    let valid = |bucket: &report_uso::UsoBucket| {
        [
            bucket.cost,
            bucket.cost_input,
            bucket.cost_output,
            bucket.cost_cache_write,
            bucket.cost_cache_read,
        ]
        .iter()
        .all(|value| value.is_finite())
    };
    valid(&report.totals)
        && [
            &report.by_skill,
            &report.by_tool,
            &report.by_bash,
            &report.by_mcp,
            &report.by_agente,
            &report.by_contexto,
            &report.by_plugin,
            &report.by_imagem,
            &report.by_area,
            &report.by_area_dia,
            &report.by_conta,
            &report.by_projeto,
            &report.by_modelo,
            &report.by_day,
        ]
        .iter()
        .all(|buckets| buckets.iter().all(valid))
        && report.usd_brl.is_none_or(f64::is_finite)
}

fn error_reason(error: &CollectError) -> FailureReason {
    match error {
        CollectError::NoScopes => FailureReason::NoScopes,
        CollectError::Index(IndexError::NoDisk) => FailureReason::NoDisk,
        CollectError::Index(IndexError::ReaderPanic) => FailureReason::ReaderPanic,
        CollectError::Index(IndexError::Sqlite(_)) => FailureReason::Sqlite,
    }
}

fn fresh(value: Option<String>) -> Option<bool> {
    match value {
        None => Some(false),
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" | "t" | "y" => Some(true),
            "0" | "false" | "off" | "no" | "f" | "n" => Some(false),
            _ => None,
        },
    }
}

enum Prepared {
    Warming(usize, usize),
    Body(Vec<u8>),
    Missing(&'static str),
}

/// Uma tentativa só: quem repete é o usuário (o coletor refaz a varredura que falhou atrás).
async fn finish(
    state: &AppState,
    request: Request,
    // Nome da sessão no custo avulso; nas outras, a rota: o diário separa uma da outra.
    session: &str,
    result: Result<Prepared, FailureReason>,
) -> Response {
    match result {
        Ok(Prepared::Body(body)) => response(request.headers(), body),
        Ok(Prepared::Warming(read, total)) => {
            let mut result = warming(read, total);
            cors(request.headers(), result.headers_mut());
            result
        }
        Ok(Prepared::Missing(message)) => session_not_found(request.headers(), message),
        Err(reason) => {
            tracing::warn!(route = %request.uri().path(), code = reason.code(), "custos no Rust falharam");
            route_failed(state, request.headers(), "rust.costs_failed", session, reason.code(), reason.message())
        }
    }
}

/// A última varredura pulou uma pasta que não leu (linhas dela mantidas): a resposta sai, e o
/// diário fica sabendo, com a causa no código.
fn report_unread(state: &AppState, route: &str) {
    if let Some(code) = state.costs.unread_issue() {
        state.diag.report("rust.costs_dir_unread", route, &code, "pasta ilegível na leitura de custos; linhas anteriores mantidas");
    }
}

async fn blocking<F>(work: F) -> Result<Prepared, FailureReason>
where
    F: FnOnce() -> Result<Prepared, FailureReason> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| FailureReason::WorkerJoin)?
}

pub async fn costs(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || request.method() != Method::GET {
        return pass(&state, request, &forward).await;
    }
    // Valor fora do bool do FastAPI: o 422 é dele, não uma falha do Rust.
    let Some(fresh) = fresh(crate::auth::query_param(request.uri().query(), "fresco")) else {
        return pass(&state, request, &forward).await;
    };
    let period = crate::auth::query_param(request.uri().query(), "period")
        .filter(|p| p == "all" || report_costs::PERIODS.iter().any(|(key, _)| p == key))
        .unwrap_or_else(|| "all".into());
    // Outro valor (ou nenhum) é o relatório inteiro: cliente antigo e servidor antigo seguem iguais.
    let summary = crate::auth::query_param(request.uri().query(), "view").as_deref() == Some("summary");
    let worker = state.clone();
    let result = blocking(move || {
        match worker.costs.prepare_blocking(fresh).map_err(|error| error_reason(&error))? {
            Ready::Warming { read, total } => return Ok(Prepared::Warming(read, total)),
            Ready::Go => {}
        }
        let now = LocalTs(chrono::Utc::now().timestamp_micros());
        let labels = worker.costs.labels_key();
        let key = CacheKey {
            data_version: worker.costs.data_version(),
            pricing_generation: worker.costs.pricing().generation(),
            area_signature: worker.costs.areas().signature().into(),
            labels: labels.clone(),
            route: vec![if summary { "costs-summary" } else { "costs" }.into(), period.clone(), now.day()],
        };
        if summary {
            let report = match worker.reports.get::<report_costs::SummaryReport>(&key) {
                Some(report) => report,
                None => {
                    let rows = worker
                        .costs
                        .read_costs(report_costs::since(&period, now).as_deref())
                        .map_err(|error| error_reason(&error))?;
                    let report = Arc::new(report_costs::build_summary(rows, &period, now, &worker.costs.pricing()));
                    worker.reports.insert(key, report.clone());
                    report
                }
            };
            let mut report = (*report).clone();
            report.usd_brl = worker.fx.usd_brl();
            if !summary_finite(&report) {
                return Err(FailureReason::NonFinite);
            }
            return serde_json::to_vec(&report).map(Prepared::Body).map_err(|_| FailureReason::Json);
        }
        let report = match worker.reports.get::<report_costs::CostReport>(&key) {
            Some(report) => report,
            None => {
                // Ler usa Pricing internamente: só adquirir a tarifa depois da leitura.
                let rows = worker
                    .costs
                    .read_costs(report_costs::since(&period, now).as_deref())
                    .map_err(|error| error_reason(&error))?;
                let report = Arc::new(report_costs::build(rows, &period, now, &worker.costs.pricing(), &|key| {
                    labels.iter().find(|(name, _)| name == key).map(|(_, label)| label.clone())
                }));
                worker.reports.insert(key, report.clone());
                report
            }
        };
        let mut report = (*report).clone();
        report.usd_brl = worker.fx.usd_brl();
        if !costs_finite(&report) {
            return Err(FailureReason::NonFinite);
        }
        serde_json::to_vec(&report).map(Prepared::Body).map_err(|_| FailureReason::Json)
    })
    .await;
    report_unread(&state, "costs");
    finish(&state, request, "costs", result).await
}

pub async fn cotacao(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || request.method() != Method::GET {
        return pass(&state, request, &forward).await;
    }
    let fx = state.fx.clone();
    let result = blocking(move || {
        let rate = fx.usd_brl();
        if rate.is_some_and(|value| !value.is_finite()) {
            return Err(FailureReason::NonFinite);
        }
        Ok(Prepared::Body(serde_json::json!({"usd_brl":rate}).to_string().into_bytes()))
    })
    .await;
    finish(&state, request, "cotacao", result).await
}

fn session_not_found(headers: &HeaderMap, message: &'static str) -> Response {
    let body = serde_json::json!({"detail":{"code":"erro_sessao_inexistente", "params":{}, "msg":message}});
    let mut response = response(headers, body.to_string().into_bytes());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

pub async fn session_cost(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    Path(name): Path<String>,
    request: Request,
) -> Response {
    let (forward, who) = match crate::routes::who(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if request.method() != Method::GET || !who.is_some_and(|c| c.can(&name, request.method())) {
        return pass(&state, request, &forward).await;
    }
    let result = match state.info(&name).await {
        Err(_) => Err(FailureReason::InfoUnavailable),
        Ok(info) => match info
            .filter(|info| info.provider == "codex")
            .and_then(|info| info.jsonl)
            .filter(|path| !path.as_os_str().is_empty())
        {
            None => Ok(Prepared::Missing("Codex session not found")),
            Some(rollout) => {
                let worker = state.clone();
                blocking(move || {
                    let path = match std::fs::canonicalize(rollout) {
                        Ok(path) => path,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(Prepared::Missing("Codex rollout not found"));
                        }
                        Err(_) => return Err(FailureReason::Io),
                    };
                    let index = worker.costs.index().map_err(|error| error_reason(&error))?;
                    let rows = codex::try_session_rows(index, &path, worker.costs.areas())
                        .map_err(|error| error_reason(&CollectError::Index(error)))?;
                    // A sessão avulsa não inicia a coleta global; reler tarifas só depois do índice.
                    let mut pricing = worker.costs.pricing();
                    pricing.reload_if_changed();
                    let cost = session_cost::estimate(rows, &pricing);
                    if cost.cost_usd.is_some_and(|value| !value.is_finite()) {
                        return Err(FailureReason::NonFinite);
                    }
                    serde_json::to_vec(&cost).map(Prepared::Body).map_err(|_| FailureReason::Json)
                })
                .await
            }
        },
    };
    finish(&state, request, &name, result).await
}

fn usage_filters(query: Option<&str>) -> report_uso::UsoFilters {
    let mut filters = report_uso::UsoFilters::default();
    for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let values = match key.as_ref() {
            "conta" => &mut filters.conta,
            "projeto" => &mut filters.projeto,
            "modelo" => &mut filters.modelo,
            "plugin" => &mut filters.plugin,
            _ => continue,
        };
        if !value.is_empty() {
            values.push(value.into_owned());
        }
    }
    filters.foco = crate::auth::query_param(query, "foco").filter(|f| !f.is_empty());
    filters
}

pub async fn usage(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = match gate(&state, peer, &request).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || request.method() != Method::GET {
        return pass(&state, request, &forward).await;
    }
    let Some(fresh) = fresh(crate::auth::query_param(request.uri().query(), "fresco")) else {
        return pass(&state, request, &forward).await;
    };
    let period = crate::auth::query_param(request.uri().query(), "period")
        .filter(|p| p == "all" || report_costs::PERIODS.iter().any(|(key, _)| p == key))
        .unwrap_or_else(|| "all".into());
    let filters = usage_filters(request.uri().query());
    let worker = state.clone();
    let result = blocking(move || {
        match worker.costs.prepare_blocking(fresh).map_err(|error| error_reason(&error))? {
            Ready::Warming { read, total } => return Ok(Prepared::Warming(read, total)),
            Ready::Go => {}
        }
        let now = LocalTs(chrono::Utc::now().timestamp_micros());
        let repo = worker.costs.repo().filter(|p| !p.as_os_str().is_empty()).ok_or(FailureReason::NoScopes)?;
        let labels = worker.costs.labels_key();
        let data_version = worker.costs.data_version();
        let origins = worker.skill_origins(&repo);
        let (origins_generation, origins_map) = origins.recent();
        let since = report_costs::PERIODS.iter().find(|(p, _)| *p == period)
            .map(|(_, n)| LocalTs(now.0 - (n - 1) * 86_400_000_000).day());
        let filter_key = serde_json::to_string(&serde_json::json!([
            ["conta", filters.conta], ["foco", filters.foco], ["modelo", filters.modelo],
            ["plugin", filters.plugin], ["projeto", filters.projeto],
        ])).map_err(|_| FailureReason::Json)?;
        let route = vec!["uso".into(), period.clone(), now.day(),
            serde_json::to_string(&repo).map_err(|_| FailureReason::Json)?, origins_generation.to_string(), filter_key];
        let key = |pricing_generation| CacheKey {
            data_version, pricing_generation, area_signature: worker.costs.areas().signature().into(),
            labels: labels.clone(), route: route.clone(),
        };
        // A leitura trava Pricing: segurar a tarifa aqui travaria o próprio worker.
        let generation = worker.costs.pricing().generation();
        let report = match worker.reports.get::<report_uso::UsoReport>(&key(generation)) {
            Some(report) => report,
            None => {
                // A chave usa a geração da mesma tarifa que montou o relatório.
                let (generation, builder) = worker.costs.fold_usage(since.as_deref(),
                    |tokens, pricing| (pricing.generation(), report_uso::UsoBuilder::new(tokens, &period, now, &filters, Some(&origins_map), pricing)),
                    &mut |(_, builder): &mut (u64, report_uso::UsoBuilder), row, account, pricing| builder.push(row, account, pricing),
                ).map_err(|error| error_reason(&error))?;
                if worker.costs.repo().as_ref() != Some(&repo) { return Err(FailureReason::NoScopes); }
                let report = Arc::new(builder.finish(&|key| {
                    labels.iter().find(|(name, _)| name == key).map(|(_, label)| label.clone())
                }));
                worker.reports.insert(key(generation), report.clone()); report
            }
        };
        if worker.costs.repo().as_ref() != Some(&repo) { return Err(FailureReason::NoScopes); }
        let mut report = (*report).clone(); report.usd_brl = worker.fx.usd_brl();
        if !usage_finite(&report) { return Err(FailureReason::NonFinite); }
        serde_json::to_vec(&report).map(Prepared::Body).map_err(|_| FailureReason::Json)
    })
    .await;
    report_unread(&state, "uso");
    finish(&state, request, "uso", result).await
}
