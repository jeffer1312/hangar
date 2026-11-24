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
        "command": ["./bin/tickets-connector", "--modo", "leitura"]
    })
}

#[test]
fn manifest_resolves_relative_executable_against_its_folder_and_keeps_arguments() {
    let parsed = manifest(valid()).unwrap();
    assert_eq!(parsed.id, "help-test");
    assert_eq!(Path::new(&parsed.command[0]), Path::new("/srv/conectores/help").join("./bin/tickets-connector"));
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

#[test]
fn work_review_requires_work_tracking_and_is_versioned() {
    let mut value = valid();
    value["capabilities"] = json!({"work_tracking": 1, "work_review": 1});
    assert_eq!(manifest(value.clone()).unwrap().capabilities.get("work_review"), Some(&1));
    value["capabilities"]["work_review"] = json!(2);
    assert_eq!(manifest(value).unwrap_err(), "unsupported_version");
    let mut value = valid();
    value["capabilities"] = json!({"work_review": 1});
    assert_eq!(manifest(value).unwrap_err(), "unsupported_capability");
}

fn side(asset: &str) -> serde_json::Value {
    json!({"label": "Mock", "value": "~1110 px", "asset": asset, "width": 1600, "height": 900, "box": [385, 263, 830, 372]})
}

fn waiting_review() -> serde_json::Value {
    json!({"review": {
        "id": "rodada:2026-11-18.1", "title": "Comparação com o mock", "subject": "Controle Bancário",
        "updated_at": "2026-11-18T10:00:00-03:00", "state": "waiting",
        "steps": {"labels": ["Prints", "Comparação", "Sua decisão"], "current": 2},
        "headline": {"value": "2", "text": "diferenças esperam a sua decisão", "detail": "15 corrigidas"},
        "badge": {"text": "mock: 2 para você", "tone": "warning"},
        "decisions": [
            {"id": "47", "tag": "Diferença 47", "title": "Janela menor que no mock", "reason": "padrão do kit",
             "reason_label": "Por que o agente não igualou", "left": side("a1"), "right": side("a2"), "extra": "fora",
             "choices": [
                {"id": "igualar", "label": "Igualar ao mock", "hint": "o agente corrige a tela"},
                {"id": "manter", "label": "Manter", "requires_reason": true, "default_reason": "padrão do kit",
                 "reason_prompt": "Por que manter?"}]},
            {"id": "48", "title": "Só um lado", "left": side("a3"), "choices": [{"id": "ok", "label": "Aceitar"}]}],
        "resolved": {"label": "Corrigidas", "count": 15, "items": [{"title": "Cabeçalho", "before": "a3", "after": "a4"}]},
        "kept": {"label": "Mantidas", "count": 3, "items": [{"title": "Selo", "reason": "sem fonte no backend"}]},
        "links": [{"label": "Relatório", "url": "http://127.0.0.1:47613/c/x/", "local": true}],
        "segredo": "não atravessa"
    }})
}

#[test]
fn work_review_get_keeps_only_contract_fields_with_contract_names() {
    use hangar_connectors::work_review::{Review, ReviewState};
    let review = Review::from_connector(waiting_review()).unwrap().expect("revisão presente");
    assert_eq!(review.state, ReviewState::Waiting);
    assert_eq!(review.decisions[1].right, None);
    let out = json!(review);
    assert!(out.get("segredo").is_none());
    assert!(out["decisions"][0].get("extra").is_none());
    assert_eq!(out["state"], "waiting");
    assert_eq!(out["decisions"][0]["left"]["box"], json!([385, 263, 830, 372]));
    assert_eq!(out["decisions"][0]["choices"][1]["requires_reason"], true);
    assert_eq!(out["decisions"][0]["reason_label"], "Por que o agente não igualou");
    assert_eq!(out["kept"]["count"], 3);
    assert_eq!(out["badge"]["tone"], "warning");
    assert_eq!(Review::from_connector(json!({"review": null})).unwrap(), None);
    assert_eq!(Review::from_connector(json!({})).unwrap_err().code, ErrorCode::ProtocolError);
}

#[test]
fn work_review_get_accepts_every_step_done_and_running_progress() {
    use hangar_connectors::work_review::Review;
    let mut value = waiting_review();
    value["review"]["steps"]["current"] = json!(3);
    Review::from_connector(value.clone()).expect("current igual ao total vale como tudo feito");
    value["review"]["state"] = json!("running");
    value["review"]["decisions"] = json!([]);
    value["review"]["progress"] = json!({"label": "Capturando", "done": 9, "total": 16});
    Review::from_connector(value).expect("progress no estado running");
}

#[test]
fn invalid_decision_from_the_connector_is_invalid_params_with_its_sentence() {
    use hangar_connectors::protocol::ConnectorError;
    let error = ConnectorError::from_rpc(&json!({"code": -32000, "message": "a decisão 47 precisa do motivo", "data": {"code": "invalid_decision"}}));
    assert_eq!((error.code, error.message.as_str()), (ErrorCode::InvalidParams, "a decisão 47 precisa do motivo"));
}

#[test]
fn work_review_get_refuses_a_result_above_3_mib_even_with_every_field_in_bounds() {
    use hangar_connectors::work_review::Review;
    let text = |n: usize| json!("é".repeat(n));
    let choice = |id: &str| json!({"id": id, "label": "x", "hint": text(2000), "default_reason": text(2000), "reason_prompt": text(2000)});
    let mut value = waiting_review();
    let big_side = |asset: &str| { let mut s = side(asset); s["value"] = text(2000); s };
    value["review"]["decisions"] = json!((0..50).map(|i| json!({"id": format!("d{i}"), "title": "x", "reason": text(2000),
        "left": big_side("a1"), "right": big_side("a2"), "choices": [choice("a"), choice("b"), choice("c"), choice("d")]})).collect::<Vec<_>>());
    value["review"]["kept"] = json!({"label": "Mantidas", "items": vec![json!({"title": "x", "reason": text(2000)}); 200]});
    let size = serde_json::to_vec(&value).unwrap().len();
    assert!(size > 3 * 1024 * 1024 && size < 4 * 1024 * 1024, "o caso cabe no transporte e passa de 3 MiB: {size}");
    assert_eq!(Review::from_connector(value).unwrap_err().code, ErrorCode::ProtocolError);
}

#[test]
fn work_review_decision_without_images_is_valid_but_right_requires_left() {
    use hangar_connectors::work_review::Review;
    let mut value = waiting_review();
    value["review"]["decisions"][1].as_object_mut().unwrap().remove("left");
    let review = Review::from_connector(value.clone()).expect("decisão sem imagens mostra só título, motivo e escolhas").unwrap();
    assert_eq!((review.decisions[1].left.as_ref(), review.decisions[1].right.as_ref()), (None, None));
    assert!(json!(review)["decisions"][1].get("left").is_none(), "lado ausente não viaja como null");
    value["review"]["decisions"][1]["right"] = side("a2");
    assert_eq!(Review::from_connector(value).unwrap_err().code, ErrorCode::ProtocolError, "right sem left");
}

#[test]
fn work_review_get_refuses_every_rule_of_the_contract() {
    use hangar_connectors::work_review::Review;
    type Change = fn(&mut serde_json::Value);
    let long = |n: usize| json!("x".repeat(n));
    let cases: Vec<(&str, Change)> = vec![
        ("estado desconhecido", |r| r["state"] = json!("blocked")),
        ("progress fora de running", |r| r["progress"] = json!({"label": "x", "done": 1, "total": 2})),
        ("progress além do total", |r| { r["state"] = json!("running"); r["decisions"] = json!([]); r["progress"] = json!({"label": "x", "done": 3, "total": 2}); }),
        ("waiting sem decisões", |r| r["decisions"] = json!([])),
        ("decisões em done", |r| r["state"] = json!("done")),
        ("current além das etapas", |r| r["steps"]["current"] = json!(4)),
        ("sem etapas", |r| r["steps"]["labels"] = json!([])),
        ("13 etapas", |r| r["steps"]["labels"] = json!(vec!["e"; 13])),
        ("box além da largura", |r| r["decisions"][0]["left"]["box"] = json!([800, 0, 801, 10])),
        ("box além da altura", |r| r["decisions"][0]["right"]["box"] = json!([0, 500, 10, 401])),
        ("box negativo", |r| r["decisions"][0]["left"]["box"] = json!([-1, 0, 10, 10])),
        ("box com 3 números", |r| r["decisions"][0]["left"]["box"] = json!([0, 0, 10])),
        ("largura zero", |r| r["decisions"][0]["left"]["width"] = json!(0)),
        ("largura acima de 16384", |r| r["decisions"][0]["left"]["width"] = json!(16385)),
        ("decisão repetida", |r| r["decisions"][1]["id"] = json!("47")),
        ("escolha repetida", |r| r["decisions"][0]["choices"][1]["id"] = json!("igualar")),
        ("sem escolhas", |r| r["decisions"][1]["choices"] = json!([])),
        ("5 escolhas", |r| r["decisions"][1]["choices"] = json!((0..5).map(|i| json!({"id": format!("c{i}"), "label": "x"})).collect::<Vec<_>>())),
        ("count abaixo dos itens", |r| r["resolved"]["count"] = json!(0)),
        ("kept.count abaixo dos itens", |r| r["kept"]["count"] = json!(0)),
        ("link relativo", |r| r["links"][0]["url"] = json!("/c/x")),
        ("link ftp", |r| r["links"][0]["url"] = json!("ftp://exemplo.invalid/x")),
        ("updated_at sem fuso", |r| r["updated_at"] = json!("2026-11-18T10:00:00")),
        ("id da revisão com espaço", |r| r["id"] = json!("rodada 2")),
        ("id do asset acima de 128", |r| r["decisions"][0]["left"]["asset"] = json!("a".repeat(129))),
        ("id da escolha maiúsculo", |r| r["decisions"][0]["choices"][0]["id"] = json!("Igualar")),
        ("asset do antes inválido", |r| r["resolved"]["items"][0]["before"] = json!("a/b")),
        ("NUL no texto", |r| r["decisions"][0]["reason"] = json!("a\u{0000}b")),
        ("título vazio", |r| r["title"] = json!(" ")),
        ("50 decisões e mais uma", |r| r["decisions"] = json!((0..51).map(|i| json!({"id": format!("d{i}"), "title": "x", "left": side("a1"), "choices": [{"id": "ok", "label": "x"}]})).collect::<Vec<_>>())),
        ("9 links", |r| r["links"] = json!(vec![json!({"label": "x", "url": "https://exemplo.invalid"}); 9])),
        ("201 corrigidas", |r| { r["resolved"]["items"] = json!(vec![json!({"title": "x"}); 201]); r["resolved"]["count"] = json!(201); }),
    ];
    for (name, change) in cases {
        let mut value = waiting_review();
        change(&mut value["review"]);
        let error = Review::from_connector(value).expect_err(name);
        assert_eq!(error.code, ErrorCode::ProtocolError, "{name}");
    }
    for (pointer, max) in [("/title", 200), ("/subject", 200), ("/badge/text", 200), ("/headline/value", 16), ("/headline/text", 2000),
        ("/decisions/0/choices/0/label", 200), ("/decisions/0/tag", 200), ("/decisions/0/reason", 2000), ("/kept/items/0/reason", 2000)] {
        let mut value = waiting_review();
        *value["review"].pointer_mut(pointer).unwrap() = json!("é".repeat(max));
        Review::from_connector(value.clone()).unwrap_or_else(|error| panic!("{pointer} no teto: {error}"));
        *value["review"].pointer_mut(pointer).unwrap() = long(max + 1);
        assert_eq!(Review::from_connector(value).unwrap_err().code, ErrorCode::ProtocolError, "{pointer} acima do teto");
    }
}

fn encoded(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0\0\x10JFIF";
const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";

#[test]
fn work_review_asset_checks_the_bytes_against_the_mime() {
    use hangar_connectors::work_review::{Asset, MAX_ASSET_BYTES};
    for (mime, bytes) in [("image/png", PNG), ("image/jpeg", JPEG), ("image/webp", WEBP)] {
        let asset = Asset::from_connector(json!({"mime": mime, "data": encoded(bytes)})).unwrap();
        assert_eq!((asset.mime.as_str(), asset.bytes.as_slice()), (mime, bytes));
    }
    let mut big = PNG.to_vec();
    big.resize(MAX_ASSET_BYTES + 1, 0);
    for (name, payload) in [
        ("png com bytes de jpeg", json!({"mime": "image/png", "data": encoded(JPEG)})),
        ("webp sem a marca", json!({"mime": "image/webp", "data": encoded(b"RIFF\x24\0\0\0WAVEfmt ")})),
        ("gif", json!({"mime": "image/gif", "data": encoded(b"GIF89a")})),
        ("vazio", json!({"mime": "image/png", "data": ""})),
        ("base64 inválido", json!({"mime": "image/png", "data": "não é base64"})),
        ("acima de 2 MiB", json!({"mime": "image/png", "data": encoded(&big)})),
        ("sem data", json!({"mime": "image/png"})),
    ] {
        assert_eq!(Asset::from_connector(payload).expect_err(name).code, ErrorCode::ProtocolError, "{name}");
    }
}

#[test]
fn work_review_asset_params_follow_the_id_formats() {
    use hangar_connectors::work_review::AssetParams;
    AssetParams::new("cartão:1/2", "r-1", "a1").unwrap();
    for (ticket, review, asset) in [("", "r-1", "a1"), ("t", "r 1", "a1"), ("t", "r-1", ""), ("t", "r-1", &"a".repeat(129))] {
        assert_eq!(AssetParams::new(ticket, review, asset).unwrap_err().code, ErrorCode::InvalidParams, "{ticket:?} {review:?} {asset:?}");
    }
}

#[test]
fn work_review_decide_checks_the_request_and_requires_ok_true() {
    use hangar_connectors::work_review::{DecideParams, decided};
    let ok = json!({"ticket_id": "t-1", "review_id": "r-1", "decisions": [{"id": "47", "choice": "manter", "reason": "é".repeat(2000)}, {"id": "48", "choice": "ok"}]});
    let params = DecideParams::from_value(ok.clone()).unwrap();
    assert!(json!(params)["decisions"][1].get("reason").is_none(), "motivo ausente não viaja como null");
    let mut cases = Vec::new();
    for (pointer, value) in [("/decisions/0/reason", json!("x".repeat(2001))), ("/decisions/0/reason", json!("a\u{0000}b")),
        ("/decisions/1/id", json!("47")), ("/decisions/0/choice", json!("Manter")), ("/review_id", json!("r 1")),
        ("/ticket_id", json!("")), ("/decisions", json!([]))] {
        let mut value_ = ok.clone();
        *value_.pointer_mut(pointer).unwrap() = value;
        cases.push((pointer, value_));
    }
    let mut extra = ok.clone();
    extra["decisions"][0]["outro"] = json!(1);
    cases.push(("campo desconhecido", extra));
    for (name, payload) in cases {
        assert_eq!(DecideParams::from_value(payload).expect_err(name).code, ErrorCode::InvalidParams, "{name}");
    }
    decided(json!({"ok": true})).unwrap();
    for payload in [json!({"ok": false}), json!({}), json!(true), json!({"ok": "true"})] {
        assert_eq!(decided(payload.clone()).unwrap_err().code, ErrorCode::ProtocolError, "{payload}");
    }
}
