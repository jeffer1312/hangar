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
fn work_actions_capability_is_explicit_and_versioned() {
    let mut value = valid();
    value["capabilities"] = json!({"work_tracking": 1, "work_actions": 1});
    let parsed = manifest(value.clone()).expect("a capacidade de ações deve ser reconhecida");
    assert_eq!(parsed.capabilities.get("work_actions"), Some(&1));
    value["capabilities"]["work_actions"] = json!(2);
    assert_eq!(manifest(value).unwrap_err(), "unsupported_version");
    let mut value = valid();
    value["capabilities"] = json!({"work_actions": 1});
    assert_eq!(manifest(value).unwrap_err(), "unsupported_capability");
}

#[test]
fn work_actions_catalog_refuses_ambiguous_or_unbounded_descriptors() {
    use hangar_connectors::work_actions::Catalog;
    let action = json!({"id": "start-pm", "label": "Iniciar PM", "prompt_template": "/iniciar {key}", "stage": "planejamento"});
    let mut invalid = Vec::new();
    invalid.push(json!({"actions": [action.clone(), action.clone()]}));
    for (field, value) in [("id", json!("../comando")), ("label", json!(" ")), ("prompt_template", json!("x\u{0000}y")),
        ("prompt_template", json!("x".repeat(4097))), ("description", json!("x".repeat(1001))), ("stage", json!("x".repeat(33)))] {
        let mut item = action.clone();
        item[field] = value;
        invalid.push(json!({"actions": [item]}));
    }
    invalid.push(json!({"actions": (0..65).map(|i| json!({"id": format!("action-{i}"), "label": "Ação", "prompt_template": "Executar"})).collect::<Vec<_>>()}));
    for payload in invalid {
        let error = Catalog::from_connector(payload).expect_err("o catálogo precisa recusar descritores inválidos");
        assert_eq!(error.code, ErrorCode::ProtocolError);
    }
}

#[test]
fn work_actions_rendering_is_one_pass_and_caps_the_expanded_message() {
    use hangar_connectors::work_actions::Catalog;
    let mut ticket = summary("t-1");
    ticket["key"] = json!("{url}");
    ticket["description_markdown"] = json!("");
    let detail = TicketDetail::from_connector(ticket.clone()).unwrap();
    let catalog = || Catalog::from_connector(json!({"actions": [{"id": "start", "label": "Iniciar", "prompt_template": "{key} {url}"}]})).unwrap();
    let action = catalog().actions.remove(0).resolve(&detail.summary).unwrap();
    assert_eq!(action.prompt, "{url} https://exemplo.invalid/t/7");
    let mut changed = detail.summary.clone();
    changed.key = "Outra chave".into();
    assert_ne!(catalog().actions.remove(0).resolve(&changed).unwrap().fingerprint, action.fingerprint);
    ticket["key"] = json!("k".repeat(1024));
    let detail = TicketDetail::from_connector(ticket).unwrap();
    let mut catalog = Catalog::from_connector(json!({"actions": [{"id": "start", "label": "Iniciar", "prompt_template": "{key}".repeat(800)}]})).unwrap();
    assert!(matches!(catalog.actions.remove(0).resolve(&detail.summary), Err(error) if error.code == ErrorCode::ProtocolError));
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
    assert_eq!(ListParams::from_value(json!({"limit": 5, "filter": {"state": "open"}})).unwrap().filter.state.as_deref(), Some("open"));
    let error = ListParams::from_pairs([("state", "aberto")]).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams, "só open ou done");
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

#[test]
fn start_prompt_fills_ticket_fields_once_and_keeps_unknown_braces() {
    let mut value = valid();
    value["start_prompt"] = json!("/iniciar {key} {title} {nada} {url}");
    let parsed = manifest(value).unwrap();
    let mut ticket = summary("t-1");
    ticket["title"] = json!("{key} no título");
    let ticket: hangar_connectors::work_tracking::TicketSummary = serde_json::from_value(ticket).unwrap();
    assert_eq!(
        parsed.start_prompt_for(&ticket).unwrap().as_deref(),
        Some("/iniciar OPS·7 {key} no título {nada} https://exemplo.invalid/t/7"),
    );
    assert_eq!(manifest(valid()).unwrap().start_prompt_for(&ticket).unwrap(), None, "sem modelo, sem sugestão");
    let mut empty = valid();
    empty["start_prompt"] = json!("  ");
    assert_eq!(manifest(empty).unwrap_err(), "manifest_invalid");
}
