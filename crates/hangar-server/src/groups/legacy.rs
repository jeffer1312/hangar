//! Par 1:1 entre máquinas antigo (`api.py`: `pair_remote`, `unpair_remote`): sidecar sem `fed` com
//! um peer `srv::x`. Só o lado que recebe fica, para máquina ainda em versão antiga; pedido novo
//! nasce grupo entre máquinas (`federation.rs`). Quem desfaz é a saída (`exit.rs`).
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use super::text::{Kind, TextArgs, protocol_text};
use super::local::{JoinRefusal, is_remote};
use super::routes::{Asked, MIX_MSG, PREFIX, envelope, error_text, outside_addresses, providers};
use super::service::{GroupError, JoinOutcome, JoinOwned};
use crate::routes::AppState;
use crate::session_write::input::deliver_text;
use crate::transcript::py::py_repr;

/// O protocolo de grupo a `name`, cujo par é `peer` de outra máquina (sem contrato: ele não
/// sincroniza entre máquinas).
async fn notify(asked: &Asked, peer: &str, task: &str, harness: &BTreeMap<String, String>) -> Result<(), Value> {
    let name = &asked.name;
    let args = TextArgs { me: name.clone(), others: vec![peer.to_owned()], task: task.to_owned(),
        harness: harness.iter().filter(|(n, _)| *n == name).map(|(n, p)| (n.clone(), p.clone())).collect(), ..Default::default() };
    let text = protocol_text(Kind::Group, &args)?;
    deliver_text(&asked.st, name, &text).await
}

fn joined(asked: &Asked, result: Result<JoinOutcome, GroupError>, mix: StatusCode) -> Result<JoinOutcome, Response> {
    result.map_err(|e| match e {
        GroupError::Refused(JoinRefusal::Mix) => asked.refuse(mix, "erro_pareamento_mistura_cross", MIX_MSG, json!({})),
        GroupError::Refused(JoinRefusal::TaskConflict { existing }) => asked.refuse(StatusCode::CONFLICT, "erro_pareamento_tarefa_existente",
            &format!("o grupo já tem tarefa: {} — repita com --substituir-tarefa pra trocar", py_repr(&json!(existing))), json!({"existente": existing})),
        error => asked.store_failed(&error),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRemoteBody { initiator: String, #[serde(default)] task: String }

/// Lado de quem recebe (`pair_remote`): grava o par com quem iniciou e avisa a sessão; não chama
/// de volta (o iniciador já gravou o lado dele).
pub(super) async fn pair_remote(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let body = match asked.body::<PairRemoteBody>() { Ok(body) => body, Err(refused) => return refused };
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    if !is_remote(&body.initiator) {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_initiator_invalido", "initiator precisa ser qualificado (srv::nome)", json!({}));
    }
    let name = asked.name.clone();
    if let Some(refused) = asked.orchestrator(std::slice::from_ref(&name)).await { return refused; }
    let harness = match providers(&asked.st, std::slice::from_ref(&name)).await {
        Ok(h) => h,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    if !harness.contains_key(&name) {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada_detalhe", &format!("sessão não encontrada: {name}"), json!({"detalhe": name}));
    }
    // A tarefa que chega é a combinada do iniciador: sempre vence.
    let result = asked.groups.join(JoinOwned { name: name.clone(), others: vec![body.initiator.clone()], task: body.task.clone(),
        replace_task: true, harness: harness.clone(), orq: false, outside: outside_addresses(&asked.st), linked: false }).await;
    let JoinOutcome { members, before, .. } = match joined(&asked, result, StatusCode::CONFLICT) { Ok(o) => o, Err(r) => return r };
    drop(held);
    if let Err(e) = notify(&asked, &body.initiator, &body.task, &harness).await {
        if let Err(failed) = asked.restore(before).await { return failed; }
        return asked.refuse(StatusCode::BAD_GATEWAY, "erro_pareamento_aviso_falhou",
            &format!("pareamento desfeito: falha ao avisar '{name}': {}", error_text(&e)), json!({"nome": name, "erro": e}));
    }
    // O par antigo vira grupo quando a outra máquina atualizar; quem pediu não espera por isso.
    let (st, groups) = (asked.st.clone(), asked.groups.clone());
    tokio::spawn(async move { super::convert::run_and_retry(&st, &groups).await });
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnpairRemoteBody { peer: String }

/// O par de outra máquina saiu (`unpair_remote`). Só desfaz se `name` está mesmo pareada com
/// quem diz sair: um pedido perdido, repetido ou com peer errado não dissolve um par legítimo.
pub(super) async fn unpair_remote(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let body = match asked.body::<UnpairRemoteBody>() { Ok(body) => body, Err(refused) => return refused };
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    let name = asked.name.clone();
    let link = match asked.groups.link(&name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    let Some(link) = link.filter(|l| l.peers.contains(&body.peer)) else {
        return asked.reply(StatusCode::OK, json!({"ok": true, "warning": null, "noop": format!("'{name}' não está pareado com '{}'", body.peer)}));
    };
    // Grupo que já é entre máquinas: quem sai é só `peer`, pela dona.
    if link.fed.is_some() {
        drop(held);
        return match super::fed_exit::peer_left(&asked.st, &asked.groups, &link, &body.peer).await {
            Ok(()) => asked.reply(StatusCode::OK, json!({"ok": true, "warning": null})),
            Err(super::federation::Refusal(status, detail)) => asked.reply(status, detail),
        };
    }
    let ex = match asked.groups.leave(&name, "unpair_remote").await { Ok(ex) => ex, Err(error) => return asked.store_failed(&error) };
    drop(held);
    let mut warning = Value::Null;
    if !ex.is_empty() {
        let text = format!("{PREFIX} '{}' saiu do pareamento. Volte a operar independente; use hangar-send só quando o usuário pedir.", body.peer);
        if let Err(e) = deliver_text(&asked.st, &name, &text).await {
            warning = envelope("erro_pareamento_aviso_unpair", format!("{name}: {}", error_text(&e)), json!({"sessao": name, "erro": e}));
        }
    }
    asked.reply(StatusCode::OK, json!({"ok": true, "warning": warning}))
}
