//! Saída de um grupo e o aviso a quem está em outra máquina (`_avisar_saida`, api.py), sempre
//! depois de soltar o lock: o disco é do `GroupService::leave`, a rede é daqui.
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::local::is_remote;
use super::service::{GroupError, GroupService, OrqFacts};
use crate::routes::AppState;
use crate::share::pair_routes::{Direct, end_pair};

pub(crate) const NO_SERVER_ID: &str = "CP_SERVER_ID ausente no backend/.env — obrigatório pra pareamento cross-server (é o endereço de resposta srv::sessao)";

/// Segmento de caminho como o app mandaria: só os não reservados ficam crus.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');

pub(crate) fn segment(name: &str) -> String { utf8_percent_encode(name, SEGMENT).to_string() }

/// `{sessao, erro}` de cada aviso que não saiu.
fn failed(p: &str, code: &str, msg: String, params: Value) -> Value {
    json!({"sessao": p, "erro": {"code": code, "params": params, "msg": msg}})
}

/// Sai do grupo: (ex-companheiros, quem o `notify_exit` avisa). Grupo entre máquinas sai pela dona,
/// na fila (`fed_exit`); `orq` = fases vindas de outra fonte (a ponte do Python).
/// `why` = quem pediu a saída, para o log de exclusão.
pub async fn leave(st: &AppState, groups: &GroupService, name: &str, orq: Option<&dyn OrqFacts>, why: &'static str) -> Result<(Vec<String>, Vec<String>), GroupError> {
    if let Some(left) = super::fed_exit::leave(st, groups, name, why).await {
        let ex = left?;
        let tell = super::fed_exit::notified(st, &ex);
        return Ok((ex, tell));
    }
    let ex = match orq { Some(orq) => groups.leave_with(name, orq, why).await?, None => groups.leave(name, why).await? };
    Ok((ex.clone(), ex))
}

/// `leave` e o aviso a cada ex-companheiro de fora: (ex-companheiros, falhas).
pub async fn leave_and_notify(st: &AppState, groups: &GroupService, name: &str, why: &'static str) -> Result<(Vec<String>, Vec<Value>), GroupError> {
    let (ex, tell) = leave(st, groups, name, None, why).await?;
    let warnings = notify_exit(st, groups, name, &tell).await;
    Ok((ex, warnings))
}

/// Par externo: avisa o outro lado e apaga registro e convite aqui mesmo. Par 1:1 antigo com
/// máquina própria: o `/unpair-remote` dela. Locais não são avisados (recado para quem saiu volta "sessão não encontrada").
pub async fn notify_exit(st: &AppState, groups: &GroupService, name: &str, ex: &[String]) -> Vec<Value> {
    let mut errs = Vec::new();
    for p in ex.iter().filter(|p| is_remote(p)) {
        // `name` pode ser o stem saneado (sessão morta): o fim do par acha o registro pelos dois.
        if let Some(share) = &st.share {
            if let Some(found) = end_pair(st, share, &Direct, name, p).await {
                errs.extend(found);
                continue;
            }
            // Par de fora de outro membro: quem sai só perde o acesso, e nunca vira `/unpair-remote`
            // da própria malha.
            if let Some(rec) = share.pairs.by_address(p) {
                if let Err(e) = share.store.ungrant(&rec.share_id, name) {
                    errs.push(failed(p, "erro_par_limpeza_falhou", e.to_string(), json!({"peer": p})));
                }
                continue;
            }
        } else {
            // Sem as lojas de convite o par externo não tem quem o desfaça; nunca vira máquina própria.
            let (dir, owner, address) = (groups.pair_root().to_path_buf(), name.to_owned(), p.clone());
            match tokio::task::spawn_blocking(move || crate::list::links::external_local_session(&owner, &[address], &dir)).await {
                Ok(None) => {}
                Ok(Some(_)) => {
                    errs.push(failed(p, "erro_peer_nao_avisado", "share_unavailable".into(), json!({"peer": p})));
                    continue;
                }
                Err(e) => {
                    errs.push(failed(p, "erro_par_limpeza_falhou", e.to_string(), json!({"peer": p})));
                    continue;
                }
            }
        }
        if groups.server_id().is_empty() {
            errs.push(failed(p, "erro_pareamento_server_id_ausente", NO_SERVER_ID.into(), json!({})));
            continue;
        }
        let (srv, sess) = p.split_once("::").unwrap_or((p, ""));
        let body = json!({"peer": format!("{}::{name}", groups.server_id())});
        if let Err(e) = st.peers.call(srv, reqwest::Method::POST, &format!("/api/sessions/{}/unpair-remote", segment(sess)), Some(&body)).await {
            // O sidecar de lá fica órfão até alguém desparear lá.
            tracing::warn!(code = "groups_peer_not_notified", session = name, server = srv, "groups: máquina do ex-companheiro não avisada da saída");
            errs.push(failed(p, "erro_peer_nao_avisado", e.text(srv), json!({"peer": p})));
        }
    }
    errs
}
