//! Linhas que a telemetria lê: carimbo do binário, retrato da hora e falha de saúde do servidor.
use chrono::{DateTime, FixedOffset};
use hangar_usage_diary::{OwnerAction, Severity, record_health, record_owner_action, record_screen_with_owner, set_build};
use serde_json::{Value, json};

fn now() -> DateTime<FixedOffset> { DateTime::parse_from_rfc3339("2026-11-16T10:00:00-03:00").unwrap() }

fn rows(base: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(base.join("uso-2026-11-16.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn telemetry_lines_carry_the_binary_the_context_and_the_cause() {
    set_build("0.42.0-abc1234");
    let dir = tempfile::tempdir().unwrap();
    let ctx = [("mem_free", "lt10".to_owned()), ("provider", "claude".to_owned())];
    record_owner_action(dir.path(), OwnerAction::Mcp, "c1", now(), "browser", "dispatch", Some("tool_failed"), 12,
        Some("desktop_app_closed"), &ctx).unwrap();
    record_owner_action(dir.path(), OwnerAction::Http, "c1", now(), "session.rename", "request", None, 3, None, &ctx).unwrap();
    let at = DateTime::parse_from_rfc3339("2026-11-16T09:00:00-03:00").unwrap();
    record_health(dir.path(), "c1", now(), Some(at), "health.server_exit", "server_exit", "core_dump", Severity::Error, &ctx).unwrap();
    let body = json!([{"evento": "sse.caiu", "nivel": "aviso", "ts": "2026-11-16T12:59:00Z", "cliente": "ios", "cli": "a1"},
                      {"evento": "tela.ver", "nivel": "ok", "ts": "2026-11-16T12:59:01Z"},
                      {"evento": "api.sem_rede", "nivel": "erro", "ts": "2026-11-16T12:00:00Z"}]);
    let started = DateTime::parse_from_rfc3339("2026-11-16T09:30:00-03:00").unwrap();
    let screen_ctx = [("mem_free", "lt10".to_owned()), ("up", "lt1h".to_owned())];
    record_screen_with_owner(dir.path(), body.to_string().as_bytes(), now(), Some(("c1", &screen_ctx[..], started))).unwrap();
    let rows = rows(dir.path());
    assert!(rows.iter().all(|r| r["telemetry_build"] == "0.42.0-abc1234"));
    assert_eq!((rows[0]["causa"].as_str(), rows[0]["ctx"]["mem_free"].as_str()), (Some("desktop_app_closed"), Some("lt10")));
    assert!(rows[1].get("ctx").is_none() && rows[1].get("causa").is_none(), "sucesso não leva retrato");
    let health = &rows[2];
    assert_eq!((health["telemetry_origin"].as_str(), health["evento"].as_str(), health["codigo"].as_str()),
        (Some("server_health"), Some("health.server_exit"), Some("server_exit")));
    assert_eq!((health["nivel"].as_str(), health["telemetry_at"].as_str()), (Some("erro"), Some("2026-11-16T09:00:00-03:00")));
    assert_eq!(rows[3]["cliente"], "ios");
    assert_eq!(rows[3]["ctx"], json!({"mem_free": "lt10", "up": "lt1h"}));
    assert!(rows[4].get("ctx").is_none());
    assert_eq!(rows[5]["ctx"]["up"], "down", "falha de antes da subida do servidor");
}
