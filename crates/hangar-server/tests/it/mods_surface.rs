use crate::mods_support;

use hangar_server::mods::model::*;
use hangar_server::mods::surface::Surface;
use hangar_server::runtime::protocol::RequestId;
use mods_support::*;
use serde_json::{Value, json};

pub fn writes(out: &[SurfaceEffect]) -> Vec<Value> {
    out.iter().filter_map(|effect| match effect { SurfaceEffect::Write { frame, .. } => Some(frame.clone()), _ => None }).collect()
}
pub fn request(out: &[SurfaceEffect], subtype: &str) -> Value {
    writes(out).into_iter().find(|frame| frame["request"]["subtype"] == subtype).unwrap_or_else(|| panic!("sem {subtype}"))
}
pub fn ok(surface: &mut Surface, frame: &Value, body: Value, now: f64) -> Vec<SurfaceEffect> {
    let id = RequestId::String(frame["request_id"].as_str().unwrap().into());
    surface.on_response(&id, &json!({"subtype": "success", "request_id": frame["request_id"], "response": body}), now)
}
/// Pedido de app com o prazo de quem acabou de chegar à rota (7 s no ator).
pub fn app(surface: &mut Surface, token: u64, call: ModsCall, now: f64) -> Vec<SurfaceEffect> {
    surface.call(token, call, now, now + 7.0)
}
pub fn panes(list: Value, shown: &str) -> Value {
    json!({"type": "system", "subtype": "ui_panes", "panes": list, "shown_id": shown, "focused_id": null, "focus_requested_id": null})
}
/// Superfície ligada, com a faixa `band` desenhada; devolve os pedidos de desenho e de rol pendentes.
pub fn ready(band: Value) -> Surface {
    let mut surface = Surface::new("ui:t".into());
    let attach = request(&surface.start(0.0), "ui_attach");
    let out = ok(&mut surface, &attach, json!({"surfaces": ["desktop"]}), 0.0);
    let render = request(&out, "ui_render");
    ok(&mut surface, &render, json!({"tree": band, "hooked": true}), 0.0);
    surface
}

#[test]
fn attach_then_band_and_panes_with_full_props() {
    let mut surface = Surface::new("ui:t".into());
    let attach = request(&surface.start(0.0), "ui_attach");
    assert!(attach["request_id"].as_str().unwrap().starts_with("ui:t:"));
    assert_eq!(attach["request"], json!({"subtype": "ui_attach", "surface": "desktop", "client_id": "hangar",
        "viewport": {"columns": 120, "rows": 40, "isFullscreen": true}, "answers": ["ui_copy"]}));
    let out = ok(&mut surface, &attach, json!({"surfaces": ["desktop"]}), 0.0);
    let render = request(&out, "ui_render");
    assert_eq!(render["request"]["component"], "AbovePrompt");
    assert_eq!(render["request"]["instance_id"], "above-prompt");
    assert_eq!(render["request"]["client_id"], "hangar");
    assert_eq!(render["request"]["props"], json!({"hasSurvey": false, "isWorking": false, "bodyColumns": 110, "maxRows": 30,
        "scroll": {"offset": 0, "bodyRows": 30}, "view": {}}));
    assert_eq!(request(&out, "ui_panes")["request"]["client_id"], "hangar");
    assert!(surface.is_ready());
}

#[test]
fn recorded_vitrine_band_is_published() {
    let drive = Drive::start("vitrine");
    let view = drive.view();
    assert_eq!((view["source"].as_str(), view["columns"].as_u64()), (Some("surface"), Some(110)));
    assert!(view["shown_id"].is_null() && view["panes"] == json!([]));
    assert!(texts(&view["above"]).contains("superfície desktop"), "V53 lido pelo mod");
}

#[test]
fn band_without_mods_is_null() {
    let drive = Drive::start("sem-plugins");
    assert!(drive.view()["above"].is_null());
}

#[test]
fn new_pane_is_drawn_with_its_columns() {
    let mut surface = ready(json!({"type": "engine", "ref": 1}));
    let out = surface.on_notice(&panes(json!([{"id": "p", "title": "Painel", "plugin": "m", "columns": 70}]), "p"), 1.0);
    let render = request(&out, "ui_render");
    assert_eq!(render["request"]["component"], "Pane");
    assert_eq!(render["request"]["instance_id"], "p");
    assert_eq!(render["request"]["props"], json!({"title": "Painel", "isFocused": false, "bodyColumns": 68, "placement": "dock",
        "scroll": {"offset": 0, "bodyRows": 40}, "view": {}}));
    let tree = json!({"type": "Text", "children": ["oi"]});
    let view = published(&ok(&mut surface, &render, json!({"tree": tree, "hooked": true}), 1.1)).unwrap();
    // O `columns` publicado é o `bodyColumns` que o mod recebeu (70 - 2), como o contrato dos apps pede (A3).
    assert_eq!(view["panes"], json!([{"id": "p", "title": "Painel", "placement": "dock", "columns": 68, "tree": tree}]));
    assert_eq!(view["shown_id"], "p");
    assert!(view["above"].is_null(), "faixa só com o nó engine sai como vazia");
}

#[test]
fn invalidate_batches_in_100ms_and_ignores_closed_instances() {
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 1.0);
    ok(&mut surface, &request(&out, "ui_render"), json!({"tree": {"type": "Text"}}), 1.0);
    let changed = json!({"type": "system", "subtype": "ui_invalidate", "event": "ui.render", "instances": [
        {"surface": "desktop", "component": "Pane", "instance_id": "p"},
        {"surface": "desktop", "component": "Pane", "instance_id": "fechado"}]});
    assert!(writes(&surface.on_notice(&changed, 2.0)).is_empty(), "espera a janela");
    assert!(writes(&surface.on_notice(&changed, 2.05)).is_empty());
    assert!(writes(&surface.tick(2.05)).is_empty());
    let out = surface.tick(2.1);
    let renders: Vec<Value> = writes(&out).into_iter().filter(|frame| frame["request"]["subtype"] == "ui_render").collect();
    assert_eq!(renders.len(), 1, "um pedido por instância montada");
    assert_eq!(renders[0]["request"]["instance_id"], "p");
    // Responde antes da próxima rodada; o invalidate com o desenho em voo tem teste próprio.
    ok(&mut surface, &renders[0], json!({"tree": {"type": "Text"}}), 2.2);
    let all = json!({"type": "system", "subtype": "ui_invalidate", "event": "ui.render"});
    surface.on_notice(&all, 3.0);
    let ids: Vec<Value> = writes(&surface.tick(3.1)).into_iter().map(|frame| frame["request"]["instance_id"].clone()).collect();
    assert!(ids.contains(&json!("above-prompt")) && ids.contains(&json!("p")), "sem instances é tudo o que está montado");
}

#[test]
fn toast_becomes_effect_and_status_is_ignored() {
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_notice(&json!({"type": "system", "subtype": "ui_toast", "plugin": "vitrine", "text": "linha 1\nlinha 2", "timeout_ms": 8000}), 1.0);
    assert_eq!(out, vec![SurfaceEffect::Toast { plugin: "vitrine".into(), text: "linha 1\nlinha 2".into(), timeout_ms: 8000 }]);
    assert!(surface.on_notice(&json!({"type": "system", "subtype": "ui_status", "plugin": "vitrine", "text": "x"}), 1.0).is_empty());
}

#[test]
fn copy_is_answered_true_and_handed_over() {
    let mut idle = Surface::new("ui:t".into());
    let out = idle.on_copy(&json!("uuid-1"), &json!({"subtype": "ui_copy", "plugin": "vitrine", "text": "antes"}));
    assert_eq!(writes(&out)[0]["response"]["response"], json!({"copied": true}));
    assert_eq!(out.len(), 1, "antes de ligar, só a resposta (pedido velho do snapshot do cano)");
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_copy(&json!("uuid-2"), &json!({"subtype": "ui_copy", "surface": "desktop", "client_id": "hangar", "plugin": "vitrine", "text": "Texto"}));
    assert_eq!(writes(&out)[0], json!({"type": "control_response", "response": {"subtype": "success", "request_id": "uuid-2", "response": {"copied": true}}}));
    assert!(out.contains(&SurfaceEffect::Copied { plugin: "vitrine".into(), text: "Texto".into() }));
}

#[test]
fn responses_from_another_life_are_ignored() {
    let mut surface = Surface::new("ui:a".into());
    surface.start(0.0);
    let foreign = RequestId::String("ui:b:1".into());
    assert!(!surface.owns(&foreign));
    assert!(surface.owns(&RequestId::String("ui:a:1".into())));
    assert!(!surface.owns(&RequestId::String("ui:ab:1".into())), "prefixo inteiro, não começo de outro");
    assert!(surface.on_response(&foreign, &json!({"subtype": "success", "response": {"surfaces": ["desktop"]}}), 0.1).is_empty());
    assert!(!surface.is_ready());
}

#[test]
fn refused_attach_turns_off_and_clears() {
    let mut surface = Surface::new("ui:t".into());
    let attach = request(&surface.start(0.0), "ui_attach");
    let id = RequestId::String(attach["request_id"].as_str().unwrap().into());
    // O rol chega durante a ligação e já pede o desenho do painel.
    let out = surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 0.05);
    let render = request(&out, "ui_render");
    assert_eq!(published(&out).unwrap()["panes"][0]["id"], "p");
    let out = surface.on_response(&id, &json!({"subtype": "error", "request_id": attach["request_id"], "error": "desconhecido"}), 0.1);
    assert!(!surface.is_ready());
    let view = published(&out).unwrap();
    assert_eq!((view["panes"].clone(), view["shown_id"].clone()), (json!([]), Value::Null));
    assert!(ok(&mut surface, &render, json!({"tree": {"type": "Text"}}), 0.2).is_empty(), "desenho em voo não volta depois de desligar");
    assert_eq!(surface.deadline(), None);
}

#[test]
fn silent_attach_retries_then_turns_off_and_clears() {
    let mut surface = Surface::new("ui:u".into());
    surface.start(0.0);
    assert_eq!(surface.deadline(), Some(15.0));
    let out = surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 6.0);
    let render = request(&out, "ui_render");
    assert_eq!(published(&out).unwrap()["panes"][0]["id"], "p");
    // Sem resposta em 15 s: liga de novo depois de 1, 2 e 4 s, e só então desliga.
    let (mut attaches, mut off) = (Vec::new(), None);
    while let Some(at) = surface.deadline() {
        let out = surface.tick(at);
        if writes(&out).iter().any(|frame| frame["request"]["subtype"] == "ui_attach") { attaches.push(at); }
        if let Some(view) = published(&out) { off = Some((at, view)); break; }
    }
    assert_eq!(attaches, vec![16.0, 33.0, 52.0]);
    let (at, view) = off.expect("esgotadas as tentativas, desliga");
    assert_eq!(at, 67.0);
    assert!(!surface.is_ready());
    assert_eq!((view["panes"].clone(), view["shown_id"].clone()), (json!([]), Value::Null));
    assert!(ok(&mut surface, &render, json!({"tree": {"type": "Text"}}), 67.1).is_empty(), "desenho em voo não volta depois de desligar");
    assert_eq!(surface.deadline(), None);
}

#[test]
fn attach_retry_that_answers_turns_on() {
    let mut surface = Surface::new("ui:u".into());
    let first = request(&surface.start(0.0), "ui_attach");
    assert!(writes(&surface.tick(15.0)).is_empty(), "espera 1 s antes de ligar de novo");
    assert!(surface.deadline() == Some(16.0) && !surface.is_ready());
    let again = request(&surface.tick(16.0), "ui_attach");
    assert_ne!(again["request_id"], first["request_id"]);
    let out = ok(&mut surface, &again, json!({"surfaces": ["desktop"]}), 16.5);
    assert!(surface.is_ready());
    assert_eq!(request(&out, "ui_render")["request"]["instance_id"], BAND_SITE);
}

#[test]
fn silent_render_is_asked_again() {
    let mut surface = ready(json!({"type": "Text"}));
    let first = request(&surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 1.0), "ui_render");
    // O rol pedido na ligação (prazo 10 s) também vence sem resposta; ele é pedido de novo 1 s depois.
    assert!(writes(&surface.tick(10.0)).is_empty());
    assert!(writes(&surface.tick(11.0)).iter().all(|frame| frame["request"]["subtype"] == "ui_panes"),
        "o desenho vencido só volta a ficar sujo");
    assert_eq!(surface.deadline(), Some(11.1));
    let again = request(&surface.tick(11.1), "ui_render");
    assert_eq!(again["request"]["instance_id"], "p");
    assert_ne!(again["request_id"], first["request_id"]);
}

#[test]
fn invalidate_during_render_waits_for_the_answer() {
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 1.0);
    let first = request(&out, "ui_render");
    let changed = json!({"type": "system", "subtype": "ui_invalidate", "event": "ui.render", "instances": [
        {"surface": "desktop", "component": "Pane", "instance_id": "p"}]});
    assert!(writes(&surface.on_notice(&changed, 2.0)).is_empty());
    // Sem janela de 100 ms enquanto o desenho está em voo: o próximo prazo é o do rol (10 s), não 2,1.
    assert_eq!(surface.deadline(), Some(10.0));
    assert!(writes(&surface.tick(2.1)).is_empty(), "nada sai com o desenho em voo");
    assert!(writes(&surface.tick(3.0)).is_empty());
    let out = ok(&mut surface, &first, json!({"tree": {"type": "Text"}}), 3.5);
    assert!(writes(&out).is_empty(), "a resposta só arma a janela");
    assert_eq!(surface.deadline(), Some(3.6));
    let renders: Vec<Value> = writes(&surface.tick(3.6)).into_iter().filter(|frame| frame["request"]["subtype"] == "ui_render").collect();
    assert_eq!(renders.len(), 1, "um pedido só depois da resposta");
    assert_eq!(renders[0]["request"]["instance_id"], "p");
}

#[test]
fn invalidate_during_click_refresh_waits_for_it() {
    let mut surface = ready(json!({"type": "Text"}));
    // Botão fora do desenho guardado: pede o desenho de novo antes de tentar (S4).
    let refresh = request(&app(&mut surface, 1, ModsCall::Press { site: BAND_SITE.into(), plugin: "vitrine".into(), key: "x".into() }, 1.0), "ui_render");
    let all = json!({"type": "system", "subtype": "ui_invalidate", "event": "ui.render"});
    assert!(writes(&surface.on_notice(&all, 1.5)).is_empty());
    assert!(writes(&surface.tick(1.6)).is_empty(), "a nova tentativa conta como desenho em voo");
    let out = ok(&mut surface, &refresh, json!({"tree": {"type": "Text"}}), 2.0);
    assert_eq!(reply_of(&out, 1), Some(Err(stale())));
    assert!(writes(&out).is_empty());
    let renders: Vec<Value> = writes(&surface.tick(2.1)).into_iter().filter(|frame| frame["request"]["subtype"] == "ui_render").collect();
    assert_eq!(renders.len(), 1);
    assert_eq!(renders[0]["request"]["instance_id"], "above-prompt");
}

#[test]
fn invalid_tree_keeps_the_pane_listed() {
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_notice(&panes(json!([{"id": "quebrado", "title": "Quebrado", "plugin": "m"}]), "quebrado"), 1.0);
    let view = published(&ok(&mut surface, &request(&out, "ui_render"), json!({"tree": {"type": "engine", "ref": 0}}), 1.1)).unwrap();
    assert_eq!(view["panes"][0]["tree"], json!({"type": "engine", "ref": 0}), "S8: fica na lista, desenhado vazio pelo app");
}

fn button(key: &str, handle: i64) -> Value {
    json!({"type": "Box", "children": [{"type": "Button", "props": {"key": key, "label": "OK"}, "press": {"plugin": "m", "handle": handle}}]})
}
fn press(site: &str, key: &str) -> ModsCall { ModsCall::Press { site: site.into(), plugin: "m".into(), key: key.into() } }
/// Clique nos desenhos gravados da vitrine, que é o mod deles.
fn recorded_press(site: &str, key: &str) -> ModsCall { ModsCall::Press { site: site.into(), plugin: "vitrine".into(), key: key.into() } }
fn code(result: Option<Result<Value, ModsError>>) -> String { result.unwrap().unwrap_err().code }

#[test]
fn recorded_vitrine_click_counts_on_its_pane() {
    let mut drive = Drive::start("vitrine");
    drive.call(1, recorded_press("above-prompt", "abrir-vitrine-botoes"));
    assert_eq!(drive.reply(1), Some(Ok(json!({"element": "abrir-vitrine-botoes"}))));
    assert_eq!(drive.view()["shown_id"], "vitrine-botoes");
    drive.advance(0.2);
    assert!(drive.pane_text("vitrine-botoes").contains("V15-comum: 0"));
    drive.call(2, recorded_press("vitrine-botoes", "V15-comum"));
    assert_eq!(drive.reply(2), Some(Ok(json!({"element": "V15-comum"}))));
    drive.advance(0.2);
    assert!(drive.pane_text("vitrine-botoes").contains("V15-comum: 1"), "o ui_invalidate com instances redesenhou o painel");
}

#[test]
fn recorded_three_panes_follow_the_last_opened() {
    let mut drive = Drive::start("vitrine");
    drive.call(1, recorded_press("above-prompt", "abrir-abas"));
    assert!(drive.reply(1).unwrap().is_ok());
    let view = drive.view();
    let ids: Vec<&str> = view["panes"].as_array().unwrap().iter().map(|pane| pane["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["vitrine-texto", "vitrine-botoes", "vitrine-hover"]);
    assert_eq!(view["shown_id"], "vitrine-hover", "P11: o último aberto fica na frente");
}

#[test]
fn recorded_invalid_and_big_trees_pass() {
    let mut drive = Drive::start("vitrine");
    drive.call(1, recorded_press("above-prompt", "abrir-invalida"));
    assert!(drive.reply(1).unwrap().is_ok());
    let quebrado = drive.view()["panes"].as_array().unwrap().iter().find(|pane| pane["id"] == "vitrine-quebrado").cloned().unwrap();
    assert_eq!(quebrado["tree"], nth_render("vitrine", "vitrine-quebrado", 0));
    drive.call(2, recorded_press("above-prompt", "abrir-grande"));
    drive.advance(0.2);
    assert!(drive.reply(2).unwrap().is_ok(), "o segundo clique também é respondido (A18)");
    let grande = drive.view()["panes"].as_array().unwrap().iter().find(|pane| pane["id"] == "vitrine-quebrado").cloned().unwrap();
    assert_eq!(grande["tree"], nth_render("vitrine", "vitrine-quebrado", 1), "V55: a árvore grande gravada (~398 KB) passa inteira");
}

#[test]
fn stale_handle_redraws_and_tries_once() {
    let mut surface = ready(button("ok", 1));
    let first = request(&app(&mut surface, 7, press("above-prompt", "ok"), 0.1), "ui_press");
    assert_eq!((first["request"]["handle"].as_i64(), first["request"]["key"].as_str()), (Some(1), Some("ok")));
    assert_eq!(first["request"]["surface"], "desktop");
    let redraw = request(&ok(&mut surface, &first, json!({"handled": false}), 0.2), "ui_render");
    assert_eq!(redraw["request"]["instance_id"], "above-prompt");
    let again = request(&ok(&mut surface, &redraw, json!({"tree": button("ok", 2), "hooked": true}), 0.3), "ui_press");
    assert_eq!(again["request"]["handle"], 2);
    let done = ok(&mut surface, &again, json!({"handled": true, "element": "ok"}), 0.4);
    assert_eq!(reply_of(&done, 7), Some(Ok(json!({"element": "ok"}))));
}

#[test]
fn second_refusal_or_missing_key_is_desenho_vencido() {
    let mut surface = ready(button("ok", 1));
    let first = request(&app(&mut surface, 1, press("above-prompt", "ok"), 0.1), "ui_press");
    let redraw = request(&ok(&mut surface, &first, json!({"handled": false}), 0.2), "ui_render");
    let again = request(&ok(&mut surface, &redraw, json!({"tree": button("ok", 2)}), 0.3), "ui_press");
    assert_eq!(code(reply_of(&ok(&mut surface, &again, json!({"handled": false}), 0.4), 1)), "erro_mod_desenho_vencido");

    let mut surface = ready(button("ok", 1));
    let redraw = request(&app(&mut surface, 2, press("above-prompt", "sumiu"), 0.1), "ui_render");
    assert_eq!(code(reply_of(&ok(&mut surface, &redraw, json!({"tree": button("ok", 3)}), 0.2), 2)), "erro_mod_desenho_vencido");
}

#[test]
fn unknown_site_is_botao_inexistente_for_press_and_painel_inexistente_for_show_and_close() {
    let mut surface = ready(button("ok", 1));
    assert_eq!(code(reply_of(&app(&mut surface, 1, press("painel-fechado", "ok"), 0.1), 1)), "erro_mod_botao_inexistente");
    assert_eq!(code(reply_of(&app(&mut surface, 2, ModsCall::Close { site: "above-prompt".into() }, 0.1), 2)), "erro_mod_painel_inexistente");
    assert_eq!(code(reply_of(&app(&mut surface, 4, ModsCall::Show { site: "above-prompt".into() }, 0.1), 4)), "erro_mod_painel_inexistente");
    assert_eq!(code(reply_of(&app(&mut surface, 5, ModsCall::Show { site: "fechado".into() }, 0.1), 5)), "erro_mod_painel_inexistente");
    assert_eq!(code(reply_of(&app(&mut surface, 6, ModsCall::Close { site: "fechado".into() }, 0.1), 6)), "erro_mod_painel_inexistente");
    let idle = &mut Surface::new("ui:t".into());
    assert_eq!(code(reply_of(&app(idle, 3, press("above-prompt", "ok"), 0.1), 3)), "erro_mod_botao_inexistente", "antes de ligar");
}

#[test]
fn input_goes_with_key_component_and_instance() {
    let mut surface = ready(json!({"type": "Text"}));
    let out = surface.on_notice(&panes(json!([{"id": "campos", "title": "Campos", "plugin": "vitrine"}]), "campos"), 1.0);
    let field = json!({"type": "Input", "props": {"key": "V18-campo", "value": ""}, "press": {"plugin": "vitrine", "handle": 9}});
    ok(&mut surface, &request(&out, "ui_render"), json!({"tree": field}), 1.0);
    let call = ModsCall::Input { site: "campos".into(), plugin: "vitrine".into(), key: "V18-campo".into(), submit: true, value: "olá, mundo".into() };
    let input = request(&app(&mut surface, 4, call, 1.1), "ui_input");
    assert_eq!(input["request"], json!({"subtype": "ui_input", "plugin": "vitrine", "handle": 9, "kind": "submit", "value": "olá, mundo",
        "key": "V18-campo", "component": "Pane", "instance_id": "campos", "surface": "desktop", "client_id": "hangar"}));
    let done = ok(&mut surface, &input, json!({"handled": true, "element": "V18-campo", "value": "olá, mundo"}), 1.2);
    assert_eq!(reply_of(&done, 4), Some(Ok(json!({"element": "V18-campo", "value": "olá, mundo"}))));
}

#[test]
fn close_waits_for_the_roster_and_can_be_refused() {
    let mut surface = ready(json!({"type": "Text"}));
    surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 0.5);
    let close = request(&app(&mut surface, 3, ModsCall::Close { site: "p".into() }, 1.0), "ui_close");
    assert_eq!(close["request"], json!({"subtype": "ui_close", "id": "p", "client_id": "hangar"}));
    assert!(reply_of(&ok(&mut surface, &close, json!({"closed": true}), 1.1), 3).is_none(), "fechar espera o rol");
    let out = surface.on_notice(&panes(json!([]), "p"), 1.2);
    assert_eq!(reply_of(&out, 3), Some(Ok(json!({}))));

    surface.on_notice(&panes(json!([{"id": "q", "title": "Q", "plugin": "m"}]), "q"), 2.0);
    let refused = request(&app(&mut surface, 4, ModsCall::Close { site: "q".into() }, 2.0), "ui_close");
    assert_eq!(code(reply_of(&ok(&mut surface, &refused, json!({"closed": false}), 2.1), 4)), "erro_mod_fechar_recusado");

    let silent = request(&app(&mut surface, 5, ModsCall::Close { site: "q".into() }, 3.0), "ui_close");
    ok(&mut surface, &silent, json!({"closed": true}), 3.1);
    assert!(reply_of(&surface.tick(4.9), 5).is_none(), "o prazo de 2 s ainda não venceu");
    assert_eq!(code(reply_of(&surface.tick(5.2), 5)), "erro_mod_clique_sem_resposta", "sem o rol em 2 s");
}

#[test]
fn show_is_confirmed_by_shown_id() {
    let mut surface = ready(json!({"type": "Text"}));
    surface.on_notice(&panes(json!([{"id": "a", "title": "A", "plugin": "m"}, {"id": "b", "title": "B", "plugin": "m"}]), "b"), 0.5);
    let show = request(&app(&mut surface, 6, ModsCall::Show { site: "a".into() }, 1.0), "ui_pane_show");
    assert_eq!(show["request"], json!({"subtype": "ui_pane_show", "id": "a", "surface": "desktop", "client_id": "hangar"}));
    assert_eq!(reply_of(&ok(&mut surface, &show, json!({"shown_id": "a"}), 1.1), 6), Some(Ok(json!({"shown_id": "a"}))));
    let show = request(&app(&mut surface, 7, ModsCall::Show { site: "a".into() }, 1.2), "ui_pane_show");
    assert_eq!(code(reply_of(&ok(&mut surface, &show, json!({"shown_id": "b"}), 1.3), 7)), "erro_mod_painel_inexistente");
}

#[test]
fn press_without_answer_times_out() {
    let mut surface = ready(button("ok", 1));
    app(&mut surface, 8, press("above-prompt", "ok"), 1.0);
    assert!(reply_of(&surface.tick(3.9), 8).is_none());
    assert_eq!(code(reply_of(&surface.tick(4.0), 8)), "erro_mod_clique_sem_resposta", "3 s sem resposta");
}

#[test]
fn refresh_then_press_answers_within_the_app_limit() {
    // Pior caminho de um clique: o botão não está no desenho guardado, o desenho de novo chega no fim
    // do prazo e o clique fica sem resposta. A recusa sai antes dos 7 s do ator (e dos 8 s do app).
    const { assert!(hangar_server::mods::surface::APP_CALL_MAX_S < 7.0) };
    let mut surface = ready(json!({"type": "Text"}));
    let refresh = request(&app(&mut surface, 8, press("above-prompt", "ok"), 1.0), "ui_render");
    assert_eq!(surface.deadline(), Some(4.0), "o desenho de novo do clique vale 3 s, não os 10 s do desenho de fundo");
    let out = ok(&mut surface, &refresh, json!({"tree": button("ok", 1)}), 3.9);
    assert_eq!(request(&out, "ui_press")["request"]["handle"], 1);
    assert!(reply_of(&surface.tick(6.8), 8).is_none());
    let out = surface.tick(6.9);
    assert_eq!(code(reply_of(&out, 8)), "erro_mod_clique_sem_resposta");
}

#[test]
fn refresh_without_answer_gives_up_in_three_seconds() {
    let mut surface = ready(json!({"type": "Text"}));
    app(&mut surface, 8, press("above-prompt", "ok"), 1.0);
    assert!(reply_of(&surface.tick(3.9), 8).is_none());
    assert_eq!(code(reply_of(&surface.tick(4.0), 8)), "erro_mod_clique_sem_resposta");
}

#[test]
fn exit_fails_pending_calls() {
    let mut surface = ready(button("ok", 1));
    app(&mut surface, 9, press("above-prompt", "ok"), 1.0);
    let out = surface.on_exit();
    assert_eq!(code(reply_of(&out, 9)), "erro_mod_clique_sem_resposta");
    let view = published(&out).unwrap();
    assert!(view["above"].is_null() && view["panes"] == json!([]));
    assert!(!surface.is_ready());
}

#[test]
fn no_action_leaves_without_time_for_the_answer_to_come_back() {
    // I1: com menos de 3 s do prazo de quem pediu, nada sai ao mod; senão a rota responderia antes e o
    // clique rodaria depois de o app ter mostrado erro (clique fantasma).
    let mut surface = ready(button("ok", 1));
    surface.on_notice(&panes(json!([{"id": "p", "title": "P", "plugin": "m"}]), "p"), 0.5);
    for (token, call) in [(1, press("above-prompt", "ok")), (2, ModsCall::Show { site: "p".into() }), (3, ModsCall::Close { site: "p".into() }),
                          (4, ModsCall::Input { site: "above-prompt".into(), plugin: "vitrine".into(), key: "ok".into(), submit: true, value: "x".into() })] {
        let out = surface.call(token, call, 1.0, 3.9);
        assert!(writes(&out).is_empty(), "nenhum pedido ao mod");
        assert_eq!(code(reply_of(&out, token)), "erro_mod_clique_sem_resposta");
    }
    // Com o prazo inteiro do pedido dentro do de quem pediu, sai.
    assert_eq!(request(&surface.call(5, press("above-prompt", "ok"), 1.0, 4.0), "ui_press")["request"]["handle"], 1);
}

#[test]
fn retry_after_a_slow_redraw_does_not_press_without_time_left() {
    // O desenho de novo gastou o que sobrava: a nova tentativa não leva o clique ao mod.
    let mut surface = ready(button("ok", 1));
    let first = request(&surface.call(1, press("above-prompt", "ok"), 1.0, 8.0), "ui_press");
    let redraw = request(&ok(&mut surface, &first, json!({"handled": false}), 2.0), "ui_render");
    let out = ok(&mut surface, &redraw, json!({"tree": button("ok", 2)}), 5.5);
    assert!(writes(&out).iter().all(|frame| frame["request"]["subtype"] != "ui_press"), "o press vencido não sai");
    assert_eq!(code(reply_of(&out, 1)), "erro_mod_clique_sem_resposta");
    // Botão fora do desenho guardado: o mesmo vale depois do desenho de novo.
    let mut surface = ready(json!({"type": "Text"}));
    let redraw = request(&surface.call(2, press("above-prompt", "ok"), 1.0, 8.0), "ui_render");
    let out = ok(&mut surface, &redraw, json!({"tree": button("ok", 3)}), 5.1);
    assert!(writes(&out).iter().all(|frame| frame["request"]["subtype"] != "ui_press"));
    assert_eq!(code(reply_of(&out, 2)), "erro_mod_clique_sem_resposta");
}

#[test]
fn silent_roster_is_asked_again_with_the_attach_waits() {
    // M1: o rol pedido na ligação sem resposta é pedido de novo depois de 1, 2 e 4 s, e para aí.
    let mut surface = ready(json!({"type": "Text"}));
    let mut asked = Vec::new();
    let mut now = 0.0;
    while let Some(at) = surface.deadline().filter(|at| *at < 100.0) {
        now = at;
        if writes(&surface.tick(at)).iter().any(|frame| frame["request"]["subtype"] == "ui_panes") { asked.push(at); }
    }
    assert_eq!(asked, vec![11.0, 23.0, 37.0]);
    assert_eq!((now, surface.deadline()), (47.0, None), "esgotadas as tentativas, espera o próximo aviso");
    // Respondido, o rol entra e a contagem recomeça.
    let mut surface = ready(json!({"type": "Text"}));
    surface.tick(10.0);
    let again = request(&surface.tick(11.0), "ui_panes");
    let out = ok(&mut surface, &again, json!({"panes": [{"id": "p", "title": "P", "plugin": "m"}], "shown_id": "p"}), 11.5);
    assert_eq!(published(&out).unwrap()["panes"][0]["id"], "p");
    assert_eq!(request(&out, "ui_render")["request"]["instance_id"], "p");
}

/// Só o que age no mod (clique, digitação, fechar) leva o prazo ao escritor, o mesmo em que a superfície
/// desiste; o desenho e as leituras, não.
#[test]
fn only_actions_carry_a_deadline_to_the_writer() {
    let mut surface = ready(button("ok", 1));
    let until = |out: &[SurfaceEffect], subtype: &str| out.iter().find_map(|effect| match effect {
        SurfaceEffect::Write { frame, until } if frame["request"]["subtype"] == subtype => Some(*until), _ => None }).unwrap();
    let out = app(&mut surface, 1, press("above-prompt", "ok"), 0.1);
    let deadline = until(&out, "ui_press").expect("o clique leva prazo");
    assert!(deadline > 0.1 && deadline <= 0.1 + 7.0, "{deadline}");
    let first = request(&out, "ui_press");
    let redraw = ok(&mut surface, &first, json!({"handled": false}), 0.2);
    assert_eq!(until(&redraw, "ui_render"), None, "o desenho vai sem prazo");
}

/// Dois mods desenham um controle com a mesma `key` no mesmo lugar: o app diz de qual mod é, e só o dele é
/// acionado, botão ou campo.
#[test]
fn the_same_key_from_two_mods_reaches_the_one_asked() {
    let two = |kind: &str| json!({"type": "Box", "children": [
        {"type": kind, "props": {"key": "ok", "label": "OK"}, "press": {"plugin": "um", "handle": 1}},
        {"type": kind, "props": {"key": "ok", "label": "OK"}, "press": {"plugin": "outro", "handle": 2}}]});
    let mut surface = ready(two("Button"));
    let call = ModsCall::Press { site: "above-prompt".into(), plugin: "outro".into(), key: "ok".into() };
    let sent = request(&app(&mut surface, 1, call, 0.1), "ui_press");
    assert_eq!((&sent["request"]["plugin"], &sent["request"]["handle"]), (&json!("outro"), &json!(2)));
    let mut surface = ready(two("Input"));
    let typing = ModsCall::Input { site: "above-prompt".into(), plugin: "um".into(), key: "ok".into(), submit: true, value: "x".into() };
    let sent = request(&app(&mut surface, 2, typing, 0.1), "ui_input");
    assert_eq!((&sent["request"]["plugin"], &sent["request"]["handle"]), (&json!("um"), &json!(1)));
}

/// O mesmo mod desenha a mesma `key` duas vezes no lugar: não há como saber qual, e nenhum é acionado, nem
/// botão nem campo. A resposta é a do item que não está mais na tela.
#[test]
fn the_same_key_twice_from_one_mod_triggers_neither() {
    let two = |kind: &str| json!({"type": "Box", "children": [
        {"type": kind, "props": {"key": "ok", "label": "OK"}, "press": {"plugin": "m", "handle": 1}},
        {"type": kind, "props": {"key": "ok", "label": "OK"}, "press": {"plugin": "m", "handle": 2}}]});
    let mut surface = ready(two("Button"));
    let out = app(&mut surface, 1, press("above-prompt", "ok"), 0.1);
    assert!(writes(&out).is_empty(), "nada sai ao mod: {:?}", writes(&out));
    assert_eq!(code(reply_of(&out, 1)), "erro_mod_botao_inexistente");
    let mut surface = ready(two("Input"));
    let typing = ModsCall::Input { site: "above-prompt".into(), plugin: "m".into(), key: "ok".into(), submit: true, value: "x".into() };
    let out = app(&mut surface, 2, typing, 0.1);
    assert!(writes(&out).is_empty());
    assert_eq!(code(reply_of(&out, 2)), "erro_mod_botao_inexistente");
}
