//! Projeção fechada do diário: texto livre e linhas sem origem e período comprovados não entram.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{CLIENT_CODES, CONTEXT, Code, FEATURE_ROUTES, HEALTH, MCP_CAUSES, closed};
use crate::state::Collection;
use crate::wire::{MAX_COUNT, MAX_RECORD_FIELDS, MAX_RECORD_TRAIL, MAX_RECORDS, MAX_TRAIL_STEPS, Record, Step};

const SCREEN_EVENTS: &[&str] = &[
    "app.abriu", "app.crash", "app.oculto", "app.visivel", "api.sem_rede", "api.voltou", "app.pronto", "chat.opened", "chat.sincronia_falhou",
    "dictation.recording_failed", "dictation.upload_failed", "js.fatal", "lista.falhou", "lista.retentativa", "lista.voltou",
    "login.falhou", "machines.request_failed", "rota.escolhida", "rota.falhou", "servidor.parado", "servidor.voltou",
    "sse.abrir", "sse.caiu", "sse.reconectando", "sse.retentativa", "sse.voltou", "tela.ver", "update.arrived_failed",
];
const SCREENS: &[&str] = &["chat", "lista", "sessions", "settings", "machines", "servers", "login", "board", "canvas", "archive", "search", "status", "mobile", "desktop", "celular"];
/// O mesmo passo que deu certo até este tempo depois da última falha conta como recuperação.
const RECOVERY: TimeDelta = TimeDelta::minutes(2);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayDetail {
    pub records: Vec<Record>,
    pub trail: Vec<Step>,
    pub dropped_records: u64,
    pub dropped_steps: u64,
}

impl DayDetail {
    pub(crate) fn is_empty(&self) -> bool { self.records.is_empty() && self.trail.is_empty() && self.dropped_records == 0 && self.dropped_steps == 0 }
}

/// Lê, projeta e agrupa o dia UTC de um binário (`build`) fora do laço de rede; a função é bloqueante.
pub type DetailSource = Arc<dyn Fn(&str, &str, &Collection, DateTime<Utc>) -> io::Result<DayDetail> + Send + Sync>;

/// Só carrega valores fechados e números, nunca a linha original.
pub struct Entry {
    at: DateTime<Utc>,
    event: &'static str,
    level: &'static str,
    screen: Option<&'static str>,
    action: Option<&'static str>,
    code: &'static str,
    stage: &'static str,
    description: &'static str,
    ms: Option<u64>,
    cause: Option<&'static str>,
    ctx: BTreeMap<String, String>,
    /// Aparelho que mandou a linha de tela; só serve para medir a volta da suspensão e não sai.
    device: Option<String>,
    /// Falha que o servidor percebeu sozinho: não é passo do dono e fica fora da trilha.
    health: bool,
}

/// A marca só é escrita por produtores autenticados; `origem` legado não prova autoria. Só entram linhas
/// carimbadas pelo binário `build`: o lote leva a versão em que cada coisa aconteceu.
pub fn project(line: &Value, day: &str, build: &str, period: &Collection, now: DateTime<Utc>, tools: &[&'static str]) -> Option<Entry> {
    let text = |key: &str| line.get(key).and_then(Value::as_str).unwrap_or_default();
    if text("telemetry_collection") != period.id || text("telemetry_build") != build { return None; }
    let time_field = if line.get("telemetry_at").is_some() { "telemetry_at" } else { "ts" };
    let at = DateTime::parse_from_rfc3339(text(time_field)).ok()?.with_timezone(&Utc);
    if at < period.since || at > now || crate::aggregate::day_of(at) != day { return None; }
    let level = match text("nivel") { "erro" => "error", "aviso" => "warn", "ok" => "ok", _ => return None };
    let mut ctx = context(line);
    let mut cause = None;
    let mut device = None;
    let mut health = false;
    let (event, screen, action, stage, code, description) = match text("telemetry_origin") {
        "owner_screen" => {
            if matches!(text("evento"), "settings.snapshot" | "machines.request_failed" | "app.visible_time" | "dictation.client_timing") { return None; }
            let event = Code::known(text("evento"), SCREEN_EVENTS).as_str();
            let screen = ["tela", "vista"].iter().map(|key| text(key)).find(|value| !value.is_empty())
                .map(|value| Code::known(value, SCREENS).as_str());
            let stage = match event {
                "login.falhou" => "authenticate", "rota.falhou" | "api.sem_rede" => "connect",
                "sse.caiu" | "chat.sincronia_falhou" => "stream", "dictation.recording_failed" => "record",
                "dictation.upload_failed" => "upload", "lista.falhou" => "request",
                "app.crash" | "js.fatal" => "runtime", _ => "other",
            };
            let description = match event {
                "api.sem_rede" | "rota.falhou" => "O aplicativo não conseguiu conectar ao servidor.",
                "sse.caiu" => "O fluxo de eventos do servidor foi interrompido.",
                "chat.sincronia_falhou" => "O aplicativo não conseguiu sincronizar a conversa com o servidor.",
                "app.crash" | "js.fatal" => "O aplicativo foi interrompido por um erro de execução.",
                "lista.falhou" => "O aplicativo não conseguiu carregar a lista de sessões.",
                "login.falhou" => "O aplicativo não conseguiu concluir a autenticação.",
                "dictation.recording_failed" => "O aplicativo não conseguiu gravar o áudio do ditado.",
                "dictation.upload_failed" => "O aplicativo não conseguiu enviar o áudio do ditado.",
                _ => "",
            };
            for (key, field) in [("client", "cliente"), ("client_version", "versao")] {
                if let Some(value) = closed(CONTEXT, key, text(field)).filter(|_| !text(field).is_empty()) { ctx.insert(key.into(), value); }
            }
            device = Some(text("cli")).filter(|cli| !cli.is_empty()).map(str::to_owned);
            (event, screen, None, stage, client_code(text("codigo")), description)
        }
        "owner_http" if text("evento") == "http.action" => {
            let action = FEATURE_ROUTES.iter().find(|(_, _, name)| *name == text("action")).map_or("other", |(_, _, name)| *name);
            let (code, description) = server_failure(text("codigo"));
            ("http.action", None, Some(action), "request", code, description)
        }
        "owner_mcp" if text("evento") == "mcp.tool" => {
            let stage = match text("etapa") { "arguments" => "arguments", "cancelled" => "cancelled", _ => "dispatch" };
            let (code, description) = server_failure(text("codigo"));
            if code == "tool_failed" { cause = Some(Code::known(text("causa"), MCP_CAUSES).as_str()); }
            ("mcp.tool", None, Some(Code::known(text("action"), tools).as_str()), stage, code, description)
        }
        "server_health" => {
            let known = HEALTH.iter().find(|h| h.code == text("codigo") && h.event == text("evento"))?;
            if level == "ok" { return None; }
            health = true;
            cause = Some(Code::known(text("causa"), known.causes).as_str());
            (known.event, None, None, "health", known.code, known.description)
        }
        _ => return None,
    };
    Some(Entry { at, event, level, screen, action, code, stage, description, ms: line["ms"].as_u64().map(|ms| ms.min(3_600_000)),
        cause, ctx, device, health })
}

/// O retrato que o servidor gravou na linha (`ctx`), só com chaves e valores da tabela `CONTEXT`.
fn context(line: &Value) -> BTreeMap<String, String> {
    let Some(raw) = line.get("ctx").and_then(Value::as_object) else { return BTreeMap::new() };
    raw.iter().filter_map(|(key, value)| Some((key.clone(), closed(CONTEXT, key, value.as_str()?)?)))
        .take(MAX_RECORD_FIELDS).collect()
}

fn client_code(code: &str) -> &'static str {
    match code {
        "route_timeout" | "TimeoutError" | "timeouterror" => "route_timeout",
        "AbortError" | "aborterror" => "aborted", "TypeError" | "typeerror" => "type_error",
        status if status.len() == 3 && status.starts_with('4') && status.bytes().all(|b| b.is_ascii_digit()) => "http_4xx",
        status if status.len() == 3 && status.starts_with('5') && status.bytes().all(|b| b.is_ascii_digit()) => "http_5xx",
        other => Code::known(other, CLIENT_CODES).as_str(),
    }
}

/// Códigos diagnósticos do servidor com significado estável no pacote.
pub fn known_failure(code: &str) -> bool { server_failure(code).0 != "other" }

fn server_failure(code: &str) -> (&'static str, &'static str) {
    match code {
        "erro_sessao_inexistente" => ("erro_sessao_inexistente", "A sessão pedida não existe mais."),
        "locate_timeout" => ("locate_timeout", "A descoberta da sessão não respondeu a tempo."),
        "runtime_unhealthy" => ("runtime_unhealthy", "O processo responsável pela sessão não aceita comandos."),
        "erro_corpo_invalido" => ("erro_corpo_invalido", "O pedido não corresponde ao formato esperado."),
        "erro_interacao_desatualizada" => ("erro_interacao_desatualizada", "A interação indicada já mudou na sessão."),
        "http_400" => ("http_400", "O servidor rejeitou o formato ou os parâmetros do pedido."),
        "http_403" => ("http_403", "A operação não foi autorizada pelo servidor."),
        "http_404" => ("http_404", "O servidor não encontrou o recurso pedido."),
        "http_409" => ("http_409", "O estado atual do recurso impediu a operação."),
        "http_422" => ("http_422", "O servidor não aceitou os valores do pedido."),
        "http_429" => ("http_429", "O servidor limitou novas tentativas."),
        "http_500" => ("http_500", "O servidor encontrou uma falha interna ao executar o pedido."),
        "http_502" => ("http_502", "O serviço necessário retornou uma resposta inválida ou indisponível."),
        "http_503" => ("http_503", "O serviço necessário não estava disponível."),
        "http_504" => ("http_504", "O serviço necessário não respondeu a tempo."),
        "invalid_arguments" => ("invalid_arguments", "Os argumentos da ferramenta não correspondem ao formato esperado."),
        "unknown_tool" => ("unknown_tool", "O servidor MCP não reconheceu a ferramenta solicitada."),
        "tool_failed" => ("tool_failed", "A ferramenta informou que não concluiu a operação."),
        "tool_cancelled" => ("tool_cancelled", "A chamada da ferramenta foi cancelada antes de retornar."),
        _ => ("other", ""),
    }
}

/// Agrupa só depois da projeção. Cada falha traz os passos antes da primeira e da última vez, o retrato
/// das duas e o que aconteceu depois da última.
pub fn build(mut entries: Vec<Entry>) -> DayDetail {
    entries.sort_by_key(|entry| entry.at);
    let mut detail = DayDetail::default();
    let mut signatures = HashMap::<_, usize>::new();
    // O texto do passo só é montado quando uma falha o copia.
    let mut recent = VecDeque::<(&str, Option<&str>)>::with_capacity(MAX_RECORD_TRAIL);
    let mut trail = VecDeque::<Step>::new();
    // Passos do dono na ordem, e de cada registro o índice em `steps` da última vez.
    let mut steps = Vec::<(DateTime<Utc>, &str, &str)>::new();
    let mut last_step = Vec::<Option<usize>>::new();
    let mut visible = HashMap::<String, DateTime<Utc>>::new();
    for entry in entries {
        let Entry { at, event, level, screen, action, code, stage, description, ms, cause, mut ctx, device, health } = entry;
        let time = at.format("%H:%M:%S").to_string();
        let step = action.unwrap_or(event);
        if let Some(device) = &device {
            match event {
                "app.visivel" => { visible.insert(device.clone(), at); }
                "app.oculto" => { visible.remove(device); }
                _ if level != "ok" => {
                    let since = visible.get(device).map(|seen| at - *seen);
                    let resumed = match since { Some(d) if d < TimeDelta::seconds(5) => "lt5s", Some(d) if d < TimeDelta::seconds(30) => "lt30s", _ => "no" };
                    ctx.insert("resumed".into(), resumed.into());
                }
                _ => {}
            }
        }
        if !health {
            match trail.back_mut() {
                Some(last) if last.event == step && last.level == level && last.screen.as_deref() == screen => {
                    last.count = (last.count + 1).min(MAX_COUNT);
                    last.capped |= last.count == MAX_COUNT;
                }
                _ => {
                    if trail.len() == MAX_TRAIL_STEPS { trail.pop_front(); detail.dropped_steps += 1; }
                    trail.push_back(Step { t: time.clone(), event: step.into(), level: level.into(), screen: screen.map(str::to_owned), count: 1, capped: false });
                }
            }
            steps.push((at, step, level));
        }
        if level != "ok" {
            let signature = (event, action, level, code, stage, cause);
            let before: Vec<String> = recent.iter()
                .map(|(step, screen)| screen.map_or_else(|| (*step).to_owned(), |screen| format!("{step}:{screen}"))).collect();
            let at_step = (!health).then(|| steps.len() - 1);
            match signatures.get(&signature) {
                Some(&index) => {
                    let record = &mut detail.records[index];
                    record.count = (record.count + 1).min(MAX_COUNT);
                    record.capped |= record.count == MAX_COUNT;
                    record.last = time;
                    record.ctx_last = ctx;
                    record.trail_last = before;
                    last_step[index] = at_step;
                }
                None if detail.records.len() < MAX_RECORDS => {
                    signatures.insert(signature, detail.records.len());
                    let mut fields = BTreeMap::from([("codigo".into(), code.into()), ("etapa".into(), stage.into())]);
                    if !description.is_empty() { fields.insert("detalhe".into(), description.into()); }
                    if let Some(action) = action { fields.insert("action".into(), action.into()); }
                    if let Some(cause) = cause { fields.insert("causa".into(), cause.into()); }
                    if let Some(ms) = ms { fields.insert("ms".into(), ms.to_string()); }
                    if let Some(screen) = screen { fields.insert("tela".into(), screen.into()); }
                    detail.records.push(Record { level: level.into(), event: event.into(), count: 1, first: time.clone(), last: time,
                        fields, trail: before.clone(), trail_last: before, ctx_first: ctx.clone(), ctx_last: ctx, after: None, capped: false });
                    last_step.push(at_step);
                }
                None => detail.dropped_records += 1,
            }
        }
        if !health {
            if recent.len() == MAX_RECORD_TRAIL { recent.pop_front(); }
            recent.push_back((step, screen));
        }
    }
    for (record, last) in detail.records.iter_mut().zip(last_step) {
        let Some(last) = last else { continue };
        let (at, step, _) = steps[last];
        let later = &steps[last + 1..];
        record.after = Some(match later.iter().find(|(_, s, level)| *s == step && *level == "ok") {
            Some((when, _, _)) if *when - at <= RECOVERY => "recovered",
            _ if later.is_empty() => "none",
            _ => "other_action",
        }.into());
    }
    detail.trail = trail.into();
    detail
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// O passo que falha de novo por outra causa e depois dá certo em até 2 minutos recuperou.
    #[test]
    fn telemetry_recovery_looks_past_a_failure_of_the_same_step_with_another_cause() {
        let start: DateTime<Utc> = "2026-11-27T03:00:00Z".parse().unwrap();
        let entry = |secs: i64, level, cause| Entry { at: start + TimeDelta::seconds(secs), event: "mcp.tool", level, screen: None,
            action: Some("browser"), code: if level == "ok" { "other" } else { "tool_failed" }, stage: "dispatch", description: "", ms: None,
            cause, ctx: BTreeMap::new(), device: None, health: false };
        let detail = build(vec![entry(0, "error", Some("desktop_app_closed")), entry(30, "error", Some("cdp_closed")), entry(60, "ok", None)]);
        assert_eq!(detail.records[0].after.as_deref(), Some("recovered"));
        let late = build(vec![entry(0, "error", Some("desktop_app_closed")), entry(30, "error", Some("cdp_closed")), entry(200, "ok", None)]);
        assert_eq!(late.records[0].after.as_deref(), Some("other_action"), "sucesso depois de 2 minutos não é recuperação");
    }

    /// O retrato só tem chaves de `CONTEXT` e as `sessions.<agente>.<superfície>`: todas juntas cabem no teto do registro.
    #[test]
    fn telemetry_every_context_key_fits_the_record_limit() {
        use crate::events::{Provider, Surface};
        assert!(CONTEXT.len() + Provider::ALL.len() * Surface::ALL.len() <= MAX_RECORD_FIELDS);
    }

    #[test]
    fn telemetry_keeps_only_the_recent_trail_and_the_ten_steps_before_a_failure() {
        let at = "2026-11-27T03:00:00Z".parse().unwrap();
        let entry = |event, level| Entry { at, event, level, screen: None, action: None, code: "other",
            stage: "other", description: "", ms: None, cause: None, ctx: BTreeMap::new(), device: None, health: false };
        let mut entries: Vec<_> = (0..5002).map(|i| entry(if i % 2 == 0 { "app.visivel" } else { "app.oculto" }, "ok")).collect();
        entries.push(entry("sse.caiu", "error"));
        let detail = build(entries);
        assert_eq!((detail.trail.len(), detail.dropped_steps), (5000, 3));
        assert_eq!(detail.trail.last().unwrap().event, "sse.caiu");
        assert_eq!(detail.records[0].trail.len(), 10);
        assert_eq!(detail.records[0].trail.last().unwrap(), "app.oculto");
    }

    #[test]
    fn telemetry_mcp_and_http_project_only_catalogued_codes_not_raw_results() {
        let now = "2026-11-27T01:00:00Z".parse().unwrap();
        let period = Collection { id: "current".into(), since: "2026-11-27T00:00:00Z".parse().unwrap(), sealed_through: None };
        let row = |origin, event, action, code| json!({"ts": now, "telemetry_origin": origin, "telemetry_collection": "current",
            "nivel": "erro", "evento": event, "action": action, "codigo": code, "etapa": "dispatch", "ms": 73,
            "telemetry_build": "0.42.0-dev", "req": "private_fixture", "arguments": ["private_fixture"], "stdout": "private_fixture", "url": "private_fixture"});
        let rows = [row("owner_mcp", "mcp.tool", "send", "tool_failed"), row("owner_http", "http.action", "session.browser", "http_503")];
        let detail = build(rows.iter().filter_map(|row| project(row, "2026-11-27", "0.42.0-dev", &period, now, &["send"])).collect());
        assert_eq!((detail.records[0].fields["action"].as_str(), detail.records[0].fields["codigo"].as_str()), ("send", "tool_failed"));
        assert_eq!((detail.records[1].fields["etapa"].as_str(), detail.records[1].fields["ms"].as_str()), ("request", "73"));
        assert_eq!(detail.records[1].fields["detalhe"], "O serviço necessário não estava disponível.");
        assert!(!serde_json::to_string(&detail).unwrap().contains("private_fixture"));
    }

    #[test]
    fn telemetry_projection_uses_full_utc_time_and_never_groups_by_private_values() {
        let since = "2026-11-27T00:00:00.050Z".parse().unwrap();
        let period = Collection { id: "current".into(), since, sealed_through: None };
        let now = "2026-11-27T01:00:00Z".parse().unwrap();
        let row = |ts, collection, source, secret| json!({"telemetry_at": ts, "telemetry_origin": source, "telemetry_collection": collection,
            "nivel": "erro", "evento": "unknown_event", "tela": secret, "codigo": secret, "etapa": secret, "telemetry_build": "b",
            "detalhe": secret, "sessao": secret, "pilha": secret, "saida": secret});
        let rows = [
            row("2026-11-26T21:00:00.051-03:00", "current", "owner_screen", "private_value_a"),
            row("2026-11-27T00:00:01.100Z", "current", "owner_screen", "private_value_b"),
            row("2026-11-26T21:00:00.049-03:00", "current", "owner_screen", "before"),
            row("2026-11-27T00:00:00.100Z", "old", "owner_screen", "previous_period"),
            row("2026-11-27T00:00:00.100Z", "current", "tela", "unknown_origin"),
            row("2026-11-27T23:30:00-03:00", "current", "owner_screen", "other_utc_day"),
        ];
        let detail = build(rows.iter().filter_map(|row| project(row, "2026-11-27", "b", &period, now, &[])).collect());
        assert_eq!(detail.records.len(), 1);
        assert_eq!((detail.records[0].event.as_str(), detail.records[0].count, detail.records[0].last.as_str()), ("other", 2, "00:00:01"));
        assert_eq!(detail.trail[0].screen.as_deref(), Some("other"));
        let body = serde_json::to_string(&detail).unwrap();
        assert!(!body.contains("private_value"));
    }

    fn period() -> Collection { Collection { id: "current".into(), since: "2026-11-27T00:00:00Z".parse().unwrap(), sealed_through: None } }

    fn line(at: &str, origin: &str, event: &str, level: &str, extra: Value) -> Value {
        let mut row = json!({"ts": at, "telemetry_at": at, "telemetry_origin": origin, "telemetry_collection": "current",
            "telemetry_build": "0.42.0-abc1234", "evento": event, "nivel": level});
        row.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        row
    }

    fn detail_of(rows: &[Value]) -> DayDetail {
        let now = "2026-11-27T23:00:00Z".parse().unwrap();
        build(rows.iter().filter_map(|row| project(row, "2026-11-27", "0.42.0-abc1234", &period(), now, &["browser"])).collect())
    }

    #[test]
    fn telemetry_keeps_only_the_lines_of_its_own_binary() {
        let mine = line("2026-11-27T10:00:00Z", "owner_screen", "sse.caiu", "aviso", json!({}));
        let mut older = mine.clone();
        older["telemetry_build"] = json!("0.41.0-abc1234");
        let mut unstamped = mine.clone();
        unstamped.as_object_mut().unwrap().remove("telemetry_build");
        let detail = detail_of(&[mine, older, unstamped]);
        assert_eq!(detail.records[0].count, 1, "linha de outro binário ou sem carimbo fica no lote dela");
    }

    #[test]
    fn telemetry_health_failures_carry_cause_and_context_but_stay_out_of_the_owner_trail() {
        let rows = [
            line("2026-11-27T10:00:00Z", "owner_http", "http.action", "ok", json!({"action": "session.browser"})),
            line("2026-11-27T10:00:05Z", "server_health", "health.state", "aviso", json!({"codigo": "terminal_observacao_falhou",
                "causa": "capture_timeout", "ctx": {"mem_free": "lt5", "provider": "claude", "nome": "minha-sessao", "model": "Meu Motor"}})),
            line("2026-11-27T10:01:00Z", "server_health", "health.state", "aviso", json!({"codigo": "terminal_observacao_falhou",
                "causa": "texto livre", "ctx": {"mem_free": "lt20"}})),
            line("2026-11-27T10:02:00Z", "server_health", "health.state", "aviso", json!({"codigo": "inventado", "causa": "x"})),
        ];
        let detail = detail_of(&rows);
        assert_eq!(detail.trail.len(), 1, "saúde do servidor não é passo do dono");
        assert_eq!(detail.records.len(), 2, "causa fora da lista vira outro registro, código fora da lista fica fora");
        let first = &detail.records[0];
        assert_eq!((first.fields["codigo"].as_str(), first.fields["causa"].as_str()), ("terminal_observacao_falhou", "capture_timeout"));
        assert_eq!(first.ctx_first, BTreeMap::from([("mem_free".into(), "lt5".into()), ("model".into(), "other".into()), ("provider".into(), "claude".into())]));
        assert_eq!(first.trail, ["session.browser"]);
        assert_eq!(first.after, None);
        assert_eq!(detail.records[1].fields["causa"], "other");
        assert!(!serde_json::to_string(&detail).unwrap().contains("minha-sessao"));
    }

    #[test]
    fn telemetry_record_keeps_both_trails_both_contexts_and_what_came_after() {
        let fail = |at: &str, mem: &str| line(at, "owner_mcp", "mcp.tool", "erro", json!({"action": "browser", "codigo": "tool_failed",
            "causa": "desktop_app_closed", "ctx": {"mem_free": mem}}));
        let ok = |at: &str, action: &str| line(at, "owner_mcp", "mcp.tool", "ok", json!({"action": action}));
        let rows = [
            line("2026-11-27T09:59:00Z", "owner_screen", "tela.ver", "ok", json!({"tela": "chat"})),
            fail("2026-11-27T10:00:00Z", "ge40"),
            ok("2026-11-27T10:05:00Z", "send"),
            fail("2026-11-27T10:06:00Z", "lt5"),
            ok("2026-11-27T10:07:00Z", "browser"),
        ];
        let record = &detail_of(&rows).records[0];
        assert_eq!(record.count, 2);
        assert_eq!(record.fields["causa"], "desktop_app_closed");
        assert_eq!(record.trail, ["tela.ver:chat"]);
        assert_eq!(record.trail_last, ["tela.ver:chat", "browser", "other"]);
        assert_eq!((record.ctx_first["mem_free"].as_str(), record.ctx_last["mem_free"].as_str()), ("ge40", "lt5"));
        assert_eq!(record.after.as_deref(), Some("recovered"));

        let late = [fail("2026-11-27T10:00:00Z", "lt5"), ok("2026-11-27T10:03:00Z", "browser")];
        assert_eq!(detail_of(&late).records[0].after.as_deref(), Some("other_action"), "depois de 2 min não é recuperação");
        assert_eq!(detail_of(&late[..1]).records[0].after.as_deref(), Some("none"));
    }

    #[test]
    fn telemetry_screen_failures_say_which_app_version_and_whether_it_just_came_back() {
        let screen = |at: &str, event: &str, level: &str, extra: Value| line(at, "owner_screen", event, level, extra);
        let rows = [
            screen("2026-11-27T10:00:00Z", "app.visivel", "ok", json!({"cli": "aparelho-1"})),
            screen("2026-11-27T10:00:02Z", "api.sem_rede", "erro", json!({"cli": "aparelho-1", "cliente": "ios", "versao": "1.4.2", "codigo": "503"})),
            screen("2026-11-27T10:05:00Z", "sse.caiu", "aviso", json!({"cli": "aparelho-1", "cliente": "Meu App", "codigo": "silencio"})),
        ];
        let detail = detail_of(&rows);
        let net = &detail.records[0];
        assert_eq!(net.fields["codigo"], "http_5xx");
        assert_eq!((net.ctx_first["client"].as_str(), net.ctx_first["client_version"].as_str(), net.ctx_first["resumed"].as_str()), ("ios", "1.4.2", "lt5s"));
        let sse = &detail.records[1];
        assert_eq!((sse.fields["codigo"].as_str(), sse.ctx_first["client"].as_str(), sse.ctx_first["resumed"].as_str()), ("silencio", "other", "no"));
        assert!(!serde_json::to_string(&detail).unwrap().contains("aparelho-1"));
    }
}
