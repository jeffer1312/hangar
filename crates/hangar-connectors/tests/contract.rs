//! Contrato público: manifesto e `work_tracking/v1`, pela API da biblioteca.
use hangar_connectors::manifest::Manifest;
use hangar_connectors::protocol::ErrorCode;
use hangar_connectors::work_tracking::{ListParams, ListResult, StatusCategory, TicketDetail};
use serde_json::json;
use std::path::Path;

fn manifest(value: serde_json::Value) -> Result<Manifest, String> {
    Manifest::parse(value.to_string().as_bytes(), Path::new("/srv/conectores/help"))
        .map_err(|error| error.code().to_owned())
}

fn valid() -> serde_json::Value {
    json!({
        "manifest_version": 1, "id": "help-test", "name": "Help de teste", "version": "0.1.0",
        "protocol_version": 1, "capabilities": {"work_tracking": 1},
        "command": ["./bin/hangar-help-test-connector", "--modo", "leitura"]
    })
}

#[test]
fn manifest_resolves_relative_executable_against_its_folder_and_keeps_arguments() {
    let parsed = manifest(valid()).unwrap();
    assert_eq!(parsed.id, "help-test");
    assert_eq!(Path::new(&parsed.command[0]), Path::new("/srv/conectores/help").join("./bin/hangar-help-test-connector"));
    assert_eq!(&parsed.command[1..], ["--modo", "leitura"]);
    assert_eq!(parsed.cwd, Path::new("/srv/conectores/help"));
}

#[test]
fn manifest_keeps_bare_program_name_for_path_lookup() {
    let mut value = valid();
    value["command"] = json!(["node", "./work_tracking.mjs"]);
    assert_eq!(manifest(value).unwrap().command, ["node", "./work_tracking.mjs"]);
}

#[test]
fn manifest_refuses_unknown_fields_such_as_secrets() {
    let mut value = valid();
    value["token"] = json!("segredo");
    assert_eq!(manifest(value).unwrap_err(), "manifest_invalid");
}

#[test]
fn manifest_refuses_unsupported_versions_and_capabilities() {
    let mut value = valid();
    value["manifest_version"] = json!(2);
    assert_eq!(manifest(value).unwrap_err(), "unsupported_version");
    let mut value = valid();
    value["protocol_version"] = json!(2);
    assert_eq!(manifest(value).unwrap_err(), "unsupported_version");
    let mut value = valid();
    value["capabilities"] = json!({"work_tracking": 2});
    assert_eq!(manifest(value).unwrap_err(), "unsupported_version");
    let mut value = valid();
    value["capabilities"] = json!({"ticket_actions": 1});
    assert_eq!(manifest(value).unwrap_err(), "unsupported_capability");
    let mut value = valid();
    value["capabilities"] = json!({});
    assert_eq!(manifest(value).unwrap_err(), "manifest_invalid");
}

#[test]
fn manifest_refuses_invalid_ids_and_empty_command() {
    for id in ["Help", "1help", "help_test", "", &"a".repeat(65)] {
        let mut value = valid();
        value["id"] = json!(id);
        assert_eq!(manifest(value).unwrap_err(), "manifest_invalid", "id {id:?}");
    }
    let mut value = valid();
    value["command"] = json!([]);
    assert_eq!(manifest(value).unwrap_err(), "manifest_invalid");
    let mut value = valid();
    value["command"] = json!(["prog", ""]);
    assert_eq!(manifest(value).unwrap_err(), "manifest_invalid");
}

#[test]
fn list_params_check_limit_and_refuse_unknown_filters() {
    let ok = ListParams::from_value(json!({"filter": {"query": "impressão", "assignee": "me"}, "limit": 20, "cursor": null})).unwrap();
    assert_eq!(ok.limit, 20);
    for limit in [0, 101] {
        let error = ListParams::from_value(json!({"limit": limit})).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams, "limite {limit}");
    }
    let error = ListParams::from_value(json!({"limit": 5, "filter": {"jql": "project = X"}})).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams);
}

fn summary(id: &str) -> serde_json::Value {
    json!({
        "id": id, "key": "OPS·7", "title": "Trocar impressora", "url": "https://exemplo.invalid/t/7",
        "status": {"id": "s-9", "name": "Em revisão", "category": "in_progress"},
        "assignee": null, "project": {"id": "p1", "name": "Infra"}, "updated_at": "2026-11-12T10:00:00-03:00"
    })
}

#[test]
fn list_result_accepts_opaque_ids_that_do_not_look_like_pm() {
    let result = ListResult::from_connector(json!({"items": [summary("7f3c"), summary("ticket/ação 2")], "next_cursor": "c2", "total": null}), 2).unwrap();
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[1].id, "ticket/ação 2");
    assert_eq!(result.items[0].status.category, StatusCategory::InProgress);
    assert_eq!(result.total, None);
    assert_eq!(result.next_cursor.as_deref(), Some("c2"));
}

#[test]
fn list_result_refuses_malformed_connector_output() {
    // Mais itens que o limite pedido.
    let error = ListResult::from_connector(json!({"items": [summary("a"), summary("b")], "next_cursor": null, "total": 2}), 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolError);
    let mut relative = summary("a");
    relative["url"] = json!("/t/7");
    let mut naive = summary("b");
    naive["updated_at"] = json!("2026-11-12T10:00:00");
    let mut category = summary("c");
    category["status"]["category"] = json!("blocked");
    let mut duplicated = vec![summary("d"), summary("d")];
    for items in [vec![relative], vec![naive], vec![category], std::mem::take(&mut duplicated)] {
        let error = ListResult::from_connector(json!({"items": items, "next_cursor": null, "total": null}), 10).unwrap_err();
        assert_eq!(error.code, ErrorCode::ProtocolError);
    }
}

#[test]
fn ticket_detail_requires_markdown_description() {
    let mut detail = summary("x1");
    assert_eq!(TicketDetail::from_connector(detail.clone()).unwrap_err().code, ErrorCode::ProtocolError);
    detail["description_markdown"] = json!("## Passos\n- um");
    let parsed = TicketDetail::from_connector(detail).unwrap();
    assert_eq!(parsed.description_markdown, "## Passos\n- um");
    assert_eq!(parsed.summary.key, "OPS·7");
}
