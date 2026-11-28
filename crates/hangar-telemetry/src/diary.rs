//! Projeção fechada do diário: texto livre e linhas sem origem e período comprovados não entram.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{Code, FEATURE_ROUTES};
use crate::state::Collection;
use crate::wire::{MAX_COUNT, MAX_RECORD_TRAIL, MAX_RECORDS, MAX_TRAIL_STEPS, Record, Step};

const SCREEN_EVENTS: &[&str] = &[
    "app.abriu", "app.crash", "app.oculto", "app.visivel", "api.sem_rede", "api.voltou", "app.pronto", "chat.opened", "chat.sincronia_falhou",
    "dictation.recording_failed", "dictation.upload_failed", "js.fatal", "lista.falhou", "lista.retentativa", "lista.voltou",
    "login.falhou", "machines.request_failed", "rota.escolhida", "rota.falhou", "servidor.parado", "servidor.voltou",
    "sse.abrir", "sse.caiu", "sse.reconectando", "sse.retentativa", "sse.voltou", "tela.ver", "update.arrived_failed",
];
const SCREENS: &[&str] = &["chat", "lista", "sessions", "settings", "machines", "servers", "login", "board", "canvas", "archive", "search", "status", "mobile", "desktop", "celular"];

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

/// Lê, projeta e agrupa o dia UTC fora do laço de rede; a função é bloqueante.
pub type DetailSource = Arc<dyn Fn(&str, &Collection, DateTime<Utc>) -> io::Result<DayDetail> + Send + Sync>;

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
}

/// A marca só é escrita por produtores autenticados; `origem` legado não prova autoria.
pub fn project(line: &Value, day: &str, period: &Collection, now: DateTime<Utc>, tools: &[&'static str]) -> Option<Entry> {
    let text = |key: &str| line.get(key).and_then(Value::as_str).unwrap_or_default();
    if text("telemetry_collection") != period.id { return None; }
    let time_field = if text("telemetry_origin") == "owner_screen" { "telemetry_at" } else { "ts" };
    let at = DateTime::parse_from_rfc3339(text(time_field)).ok()?.with_timezone(&Utc);
    if at < period.since || at > now || crate::aggregate::day_of(at) != day { return None; }
    let level = match text("nivel") { "erro" => "error", "aviso" => "warn", "ok" => "ok", _ => return None };
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
            let code = match text("codigo") {
                "route_timeout" | "TimeoutError" | "timeouterror" => "route_timeout",
                "AbortError" | "aborterror" => "aborted", "TypeError" | "typeerror" => "type_error", "panic" => "panic",
                _ => "other",
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
            (event, screen, None, stage, code, description)
        }
        "owner_http" if text("evento") == "http.action" => {
            let action = FEATURE_ROUTES.iter().find(|(_, _, name)| *name == text("action")).map_or("other", |(_, _, name)| *name);
            let (code, description) = server_failure(text("codigo"));
            ("http.action", None, Some(action), "request", code, description)
        }
        "owner_mcp" if text("evento") == "mcp.tool" => {
            let stage = match text("etapa") { "arguments" => "arguments", "cancelled" => "cancelled", _ => "dispatch" };
            let (code, description) = server_failure(text("codigo"));
            ("mcp.tool", None, Some(Code::known(text("action"), tools).as_str()), stage, code, description)
        }
        _ => return None,
    };
    Some(Entry { at, event, level, screen, action, code, stage, description, ms: line["ms"].as_u64().map(|ms| ms.min(3_600_000)) })
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

/// Agrupa só depois da projeção. A amostra de cada falha traz os dez passos anteriores.
pub fn build(mut entries: Vec<Entry>) -> DayDetail {
    entries.sort_by_key(|entry| entry.at);
    let mut detail = DayDetail::default();
    let mut signatures = HashMap::<_, usize>::new();
    let mut recent = VecDeque::<&str>::with_capacity(MAX_RECORD_TRAIL);
    let mut trail = VecDeque::<Step>::new();
    for entry in entries {
        let Entry { at, event, level, screen, action, code, stage, description, ms } = entry;
        let time = at.format("%H:%M:%S").to_string();
        let step = action.unwrap_or(event);
        match trail.back_mut() {
            Some(last) if last.event == step && last.level == level && last.screen.as_deref() == screen => last.count = (last.count + 1).min(MAX_COUNT),
            _ => {
                if trail.len() == MAX_TRAIL_STEPS { trail.pop_front(); detail.dropped_steps += 1; }
                trail.push_back(Step { t: time.clone(), event: step.into(), level: level.into(), screen: screen.map(str::to_owned), count: 1 });
            }
        }
        if level != "ok" {
            let signature = (event, action, level, code, stage);
            match signatures.get(&signature) {
                Some(&index) => {
                    let record = &mut detail.records[index];
                    record.count = (record.count + 1).min(MAX_COUNT);
                    record.last = time;
                }
                None if detail.records.len() < MAX_RECORDS => {
                    signatures.insert(signature, detail.records.len());
                    let mut fields = BTreeMap::from([("codigo".into(), code.into()), ("etapa".into(), stage.into())]);
                    if !description.is_empty() { fields.insert("detalhe".into(), description.into()); }
                    if let Some(action) = action { fields.insert("action".into(), action.into()); }
                    if let Some(ms) = ms { fields.insert("ms".into(), ms.to_string()); }
                    if let Some(screen) = screen { fields.insert("tela".into(), screen.into()); }
                    detail.records.push(Record { level: level.into(), event: event.into(), count: 1, first: time.clone(), last: time,
                        fields, trail: recent.iter().map(|s| (*s).into()).collect() });
                }
                None => detail.dropped_records += 1,
            }
        }
        if recent.len() == MAX_RECORD_TRAIL { recent.pop_front(); }
        recent.push_back(step);
    }
    detail.trail = trail.into();
    detail
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn telemetry_keeps_only_the_recent_trail_and_the_ten_steps_before_a_failure() {
        let at = "2026-11-27T03:00:00Z".parse().unwrap();
        let entry = |event, level| Entry { at, event, level, screen: None, action: None, code: "other",
            stage: "other", description: "", ms: None };
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
            "req": "private_fixture", "arguments": ["private_fixture"], "stdout": "private_fixture", "url": "private_fixture"});
        let rows = [row("owner_mcp", "mcp.tool", "send", "tool_failed"), row("owner_http", "http.action", "session.browser", "http_503")];
        let detail = build(rows.iter().filter_map(|row| project(row, "2026-11-27", &period, now, &["send"])).collect());
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
            "nivel": "erro", "evento": "unknown_event", "tela": secret, "codigo": secret, "etapa": secret,
            "detalhe": secret, "sessao": secret, "pilha": secret, "saida": secret});
        let rows = [
            row("2026-11-26T21:00:00.051-03:00", "current", "owner_screen", "private_value_a"),
            row("2026-11-27T00:00:01.100Z", "current", "owner_screen", "private_value_b"),
            row("2026-11-26T21:00:00.049-03:00", "current", "owner_screen", "before"),
            row("2026-11-27T00:00:00.100Z", "old", "owner_screen", "previous_period"),
            row("2026-11-27T00:00:00.100Z", "current", "tela", "unknown_origin"),
            row("2026-11-27T23:30:00-03:00", "current", "owner_screen", "other_utc_day"),
        ];
        let detail = build(rows.iter().filter_map(|row| project(row, "2026-11-27", &period, now, &[])).collect());
        assert_eq!(detail.records.len(), 1);
        assert_eq!((detail.records[0].event.as_str(), detail.records[0].count, detail.records[0].last.as_str()), ("other", 2, "00:00:01"));
        assert_eq!(detail.trail[0].screen.as_deref(), Some("other"));
        let body = serde_json::to_string(&detail).unwrap();
        assert!(!body.contains("private_value"));
    }
}
