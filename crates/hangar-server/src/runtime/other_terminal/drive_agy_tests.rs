//! Ciclo de modo do Antigravity pelo rodapé, com telas medidas. Fora do `drive.rs` porque o teste de
//! integração inclui aquele arquivo sozinho e não tem o alvo de teste do módulo.
use super::drive::control;
use super::{OtherFrame, OtherIo, OtherTarget, OtherWriteGuard};
use crate::interactions::InteractionStore;
use crate::runtime::protocol::{Disposition as RuntimeDisposition, RuntimeError, RuntimeReply};
use serde_json::{Value, json};
use futures_util::future::BoxFuture;
use std::{collections::VecDeque, sync::Mutex};

fn screen(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hangar-core/tests/fixtures/agy").join(name)).unwrap()
}
/// Devolve as telas em ordem e repete a última; guarda as teclas.
struct Screens { frames: Mutex<VecDeque<String>>, keys: Mutex<Vec<String>> }
impl Screens {
    fn new(names: &[&str]) -> Self {
        Self { frames: Mutex::new(names.iter().map(|name| screen(name)).collect()), keys: Mutex::default() }
    }
    fn keys(&self) -> Vec<String> { self.keys.lock().unwrap().clone() }
}
impl OtherIo for Screens {
    fn prove<'a>(&'a self, _: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> { Box::pin(async { Ok(true) }) }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>> {
        Box::pin(async {
            let mut frames = self.frames.lock().unwrap();
            let text = if frames.len() > 1 { frames.pop_front().unwrap() } else { frames.front().cloned().unwrap_or_default() };
            Ok(OtherFrame { text, typed: None, styled: None, editor: None })
        })
    }
    fn write<'a>(&'a self, _: &'a OtherTarget, value: &'a str, _: bool, _: Option<&'a OtherWriteGuard>) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async move { self.keys.lock().unwrap().push(value.to_owned()); Ok(()) })
    }
    fn release<'a>(&'a self) -> BoxFuture<'a, ()> { Box::pin(async {}) }
}
fn target() -> OtherTarget {
    let mut target = super::tests::target();
    target.provider = hangar_transcript::Provider::Antigravity;
    target
}
async fn run(io: &Screens, kind: &str, payload: Value) -> RuntimeReply {
    let store = InteractionStore::default();
    control(&target(), io, &store, "op", kind, &payload, None).await.unwrap().reply
}

#[tokio::test(start_paused = true)]
async fn the_modes_read_comes_from_the_footer_in_the_cycle_order() {
    let io = Screens::new(&["footer_default.txt"]);
    let reply = run(&io, "permission_modes", json!({"probe": false})).await;
    assert_eq!(reply.disposition, RuntimeDisposition::Accepted);
    assert_eq!(reply.payload["current"], "manual");
    assert_eq!(reply.payload["modes"], json!(["auto", "plan", "manual", "acceptEdits", "bypassPermissions"]));
    assert!(io.keys().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_switch_presses_shift_tab_until_the_footer_shows_the_mode() {
    let io = Screens::new(&["idle.txt", "footer_plan.txt", "footer_default.txt", "footer_accept_edits.txt"]);
    let reply = run(&io, "set_permission_mode", json!({"mode": "acceptEdits"})).await;
    assert_eq!(reply.disposition, RuntimeDisposition::Accepted, "{}", reply.payload);
    assert_eq!(reply.payload["current"], "acceptEdits");
    assert_eq!(io.keys(), ["BTab", "BTab", "BTab"]);
}

/// Sem rodapé novo depois da tecla, outra tecla giraria às cegas: a resposta é incerta.
#[tokio::test(start_paused = true)]
async fn a_key_without_a_new_footer_stops_uncertain() {
    let io = Screens::new(&["idle.txt"]);
    let reply = run(&io, "set_permission_mode", json!({"mode": "plan"})).await;
    assert_eq!(reply.disposition, RuntimeDisposition::Unknown);
    assert_eq!(reply.payload["error_code"], "permission_key_timeout");
    assert_eq!(io.keys(), ["BTab"]);
}

#[tokio::test(start_paused = true)]
async fn draft_turn_menu_and_bypass_are_refused_without_a_key() {
    for (name, mode, code) in [
        ("draft.txt", "plan", "erro_caixa_com_rascunho"),
        ("working.txt", "plan", "erro_sessao_trabalhando"),
        ("permission.txt", "plan", "permission_menu_open"),
        ("idle.txt", "bypassPermissions", "erro_permissao_so_na_abertura"),
        ("idle.txt", "dontAsk", "erro_permissao_invalida"),
    ] {
        let io = Screens::new(&[name]);
        let reply = run(&io, "set_permission_mode", json!({"mode": mode})).await;
        assert_eq!((reply.disposition, reply.payload["error_code"].as_str()), (RuntimeDisposition::Rejected, Some(code)), "{name}");
        assert!(io.keys().is_empty(), "{name}");
    }
}

/// Entrar ou sair do bypass reabre o pane pela rota: aqui só a tela é conferida, sem tecla.
#[tokio::test(start_paused = true)]
async fn the_bypass_reopen_check_refuses_a_draft_and_presses_nothing() {
    for (name, mode, code) in [
        ("draft.txt", "bypassPermissions", Some("erro_caixa_com_rascunho")),
        ("working.txt", "plan", Some("erro_sessao_trabalhando")),
        ("permission.txt", "bypassPermissions", Some("permission_menu_open")),
        ("idle.txt", "bypassPermissions", None),
        ("idle.txt", "auto", None),
    ] {
        let io = Screens::new(&[name]);
        let reply = run(&io, "set_permission_mode", json!({"mode": mode, "reopen": true})).await;
        match code {
            Some(code) => assert_eq!((reply.disposition, reply.payload["error_code"].as_str()), (RuntimeDisposition::Rejected, Some(code)), "{name}"),
            None => assert_eq!((reply.disposition, reply.payload["requires_reopen"].as_bool()), (RuntimeDisposition::Accepted, Some(true)), "{name}"),
        }
        assert!(io.keys().is_empty(), "{name}");
    }
}

/// A tarefa em segundo plano (`1 task(s)`) aparece entre linhas abaixo da caixa: lida como rascunho, a fila parava.
#[test]
fn a_background_task_panel_below_the_box_is_not_a_draft() {
    assert_eq!(super::drive::agy_draft(&screen("background_task.txt")).as_deref(), Some(""));
    assert_eq!(super::drive::agy_draft(&screen("background_task_draft.txt")).as_deref(), Some("arrumar o teste do login"));
    assert_eq!(super::drive::agy_draft(&screen("draft.txt")).as_deref(), Some("arrumar o teste do login"));
}

/// Os Termos do Google: o Hangar só aperta as teclas da opção que a pessoa tocou, relendo a tela a cada uma.
#[tokio::test(start_paused = true)]
async fn the_terms_card_unchecks_the_data_box_and_presses_done_only_for_the_chosen_option() {
    let io = Screens::new(&["terms.txt", "terms.txt", "terms-unchecked.txt", "terms-unchecked-previous.txt", "terms-unchecked-done.txt", "idle.txt"]);
    let store = InteractionStore::default();
    let target = target();
    let binding = crate::interactions::InteractionBinding { key: target.key.clone(), conversation: target.binding.conversation.clone(), generation: target.generation };
    store.bind(&binding).unwrap();
    let analysis = crate::terminal_state::agy_analyze(&screen("terms.txt"));
    let pending = super::menu_interaction(analysis.question, analysis.options.unwrap());
    let id = store.observe(&binding, &pending.target, Some(pending.semantic)).unwrap().unwrap();
    let reply = control(&target, &io, &store, "op", "select", &json!({"option": 2, "interaction_id": id}), None).await.unwrap().reply;
    assert_eq!(reply.disposition, RuntimeDisposition::Accepted, "{}", reply.payload);
    assert_eq!(io.keys(), ["Enter", "Down", "Right", "Enter"]);
}
