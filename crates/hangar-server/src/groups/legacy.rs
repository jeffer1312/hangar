//! Par 1:1 entre máquinas de hoje (`api.py`: `_pair_cross_server`, `pair_remote`, `unpair_remote`):
//! sidecar sem `fed` com um peer `srv::x`, cada lado gravando o seu. Fica como está até o grupo
//! entre máquinas existir; quem desfaz é a saída (`exit.rs`).
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use super::deliver::{ProtocolArgs, protocol_text};
use super::exit::{NO_SERVER_ID, segment};
use super::local::{JoinRefusal, is_remote};
use super::routes::{Asked, MIX_MSG, PREFIX, envelope, error_text, providers};
use super::service::{GroupError, JoinOutcome, JoinOwned};
use crate::routes::AppState;
use crate::runtime::ingress::IngressPass;
use crate::session_write::input::deliver_text;
use crate::transcript::py::py_repr;

/// O protocolo de grupo a `name`, cujo par é `peer` de outra máquina (sem contrato: ele não
/// sincroniza entre máquinas).
async fn notify(asked: &Asked, peer: &str, task: &str, harness: &BTreeMap<String, String>) -> Result<(), Value> {
    let name = &asked.name;
    let args = ProtocolArgs { me: name.clone(), others: vec![peer.to_owned()], task: task.to_owned(),
        harness: harness.iter().filter(|(n, _)| *n == name).map(|(n, p)| (n.clone(), p.clone())).collect(), ..Default::default() };
    let text = protocol_text(&asked.st, "group", &args).await?;
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

/// Lado de quem inicia (`_pair_cross_server`), já passada a recusa de orquestrador: grava o vínculo
/// daqui, pede ao outro lado o reverso pelo `/pair-remote` e desfaz este se ele não confirmar.
pub(super) async fn pair_cross(asked: Asked, held: Option<IngressPass>, others: Vec<String>, task: String, replace_task: bool) -> Response {
    if others.len() != 1 {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_pareamento_cross_1_1",
            "pareamento cross-server é 1:1 por enquanto: um peer remoto, sem misturar com grupo local", json!({}));
    }
    let server_id = asked.groups.server_id().to_owned();
    if server_id.is_empty() {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_pareamento_server_id_ausente", NO_SERVER_ID, json!({}));
    }
    let name = asked.name.clone();
    let harness = match providers(&asked.st, std::slice::from_ref(&name)).await {
        Ok(h) => h,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    if !harness.contains_key(&name) {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada_detalhe", &format!("sessão não encontrada: {name}"), json!({"detalhe": name}));
    }
    let peer = others[0].clone();
    let result = asked.groups.join(JoinOwned { name: name.clone(), others, task, replace_task, harness: harness.clone(), orq: false }).await;
    let JoinOutcome { members, task, before, .. } = match joined(&asked, result, StatusCode::BAD_REQUEST) { Ok(o) => o, Err(r) => return r };
    drop(held);
    let (srv, sess) = peer.split_once("::").unwrap_or((&peer, ""));
    let initiator = format!("{server_id}::{name}");
    let asked_remote = asked.st.peers.call(srv, reqwest::Method::POST, &format!("/api/sessions/{}/pair-remote", segment(sess)),
        Some(&json!({"initiator": initiator, "task": task}))).await;
    if let Err(e) = asked_remote {
        if let Err(failed) = asked.restore(before).await { return failed; }
        let text = e.text(srv);
        if e.is_transport() {
            // O outro lado pode ter gravado antes de a resposta se perder: tenta desfazer lá também.
            let _ = asked.st.peers.call(srv, reqwest::Method::POST, &format!("/api/sessions/{}/unpair-remote", segment(sess)),
                Some(&json!({"peer": initiator}))).await;
            return asked.refuse(StatusCode::BAD_GATEWAY, "erro_pareamento_nao_confirmado",
                &format!("pareamento NÃO confirmado (falha de rede com '{srv}'): desfeito deste lado; se o peer tiver ficado pareado, rode unpair lá. ({text})"),
                json!({"srv": srv, "erro": text}));
        }
        return asked.refuse(StatusCode::BAD_GATEWAY, "erro_pareamento_rejeitado", &format!("pareamento desfeito (peer rejeitou): {text}"), json!({"erro": text}));
    }
    // O vínculo já vale dos dois lados: aviso local que falha só avisa, não desfaz.
    let warning = match notify(&asked, &peer, &task, &harness).await {
        Ok(()) => Value::Null,
        Err(e) => envelope("erro_pareamento_aviso_local",
            format!("vínculo criado, mas o aviso local falhou ({name}: {}) — refaça o pair se precisar", error_text(&e)),
            json!({"sessao": name, "erro": e})),
    };
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members, "warning": warning}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRemoteBody { initiator: String, #[serde(default)] task: String }

/// Lado de quem recebe (`pair_remote`): grava o par com quem iniciou e avisa a sessão; não chama
/// de volta (o iniciador já gravou o lado dele).
pub(super) async fn pair_remote(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let Some(body) = asked.body::<PairRemoteBody>() else { return asked.to_python().await };
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
        replace_task: true, harness: harness.clone(), orq: false }).await;
    let JoinOutcome { members, before, .. } = match joined(&asked, result, StatusCode::CONFLICT) { Ok(o) => o, Err(r) => return r };
    drop(held);
    if let Err(e) = notify(&asked, &body.initiator, &body.task, &harness).await {
        if let Err(failed) = asked.restore(before).await { return failed; }
        return asked.refuse(StatusCode::BAD_GATEWAY, "erro_pareamento_aviso_falhou",
            &format!("pareamento desfeito: falha ao avisar '{name}': {}", error_text(&e)), json!({"nome": name, "erro": e}));
    }
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnpairRemoteBody { peer: String }

/// O par de outra máquina saiu (`unpair_remote`). Só desfaz se `name` está mesmo pareada com
/// quem diz sair: um pedido perdido, repetido ou com peer errado não dissolve um par legítimo.
pub(super) async fn unpair_remote(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let Some(body) = asked.body::<UnpairRemoteBody>() else { return asked.to_python().await };
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    let name = asked.name.clone();
    let link = match asked.groups.link(&name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    if !link.is_some_and(|l| l.peers.contains(&body.peer)) {
        return asked.reply(StatusCode::OK, json!({"ok": true, "warning": null, "noop": format!("'{name}' não está pareado com '{}'", body.peer)}));
    }
    let ex = match asked.groups.leave(&name).await { Ok(ex) => ex, Err(error) => return asked.store_failed(&error) };
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
