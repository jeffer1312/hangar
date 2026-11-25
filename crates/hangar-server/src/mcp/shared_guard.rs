//! Guarda fail-closed de sessão compartilhada para WhatsApp e integrações MCP externas.
//! Impede que uma sessão visível por convidado, par externo ou membro remoto acesse recursos
//! exclusivos do dono (ou vaze resultados para o grupo).

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request};
use serde_json::{Value, json};
use tower_service::Service;

use crate::groups::exit::segment;
use crate::groups::local::is_remote;
use crate::groups::model::Sidecar;
use crate::login_guests::{gate::session_life, store::Result as GuestResult};
use crate::routes::AppState;
use crate::share::pairs::PairStore;
use crate::share::store::ShareStore;

const ROUTE_BODY_LIMIT: usize = 16 << 20;

/// Mensagens de recusa parametrizadas por consumidor (WhatsApp vs Integrações MCP).
#[derive(Clone, Copy)]
pub(super) struct GuardMessages {
    pub shared_refusal: &'static str,
    pub check_failed_prefix: &'static str,
    pub check_failed_suffix: &'static str,
}

impl GuardMessages {
    pub(super) fn failed(&self, why: &str) -> String {
        format!("{}: {why}{}", self.check_failed_prefix, self.check_failed_suffix)
    }
}

pub(super) const WHATSAPP: GuardMessages = GuardMessages {
    shared_refusal: "whatsapp_shared_session: esta sessão tem convidado ou par de fora (convite, convidado \
com login que a vê, par externo, ou membro de outra máquina ou de fora no grupo) e não lê o WhatsApp; use uma sessão só sua",
    check_failed_prefix: "whatsapp_shared_check_failed",
    check_failed_suffix: "; a sessão não lê o WhatsApp sem saber se é compartilhada",
};

pub(super) const INTEGRATIONS: GuardMessages = GuardMessages {
    shared_refusal: "integration_shared_session: esta sessão tem convidado ou par de fora (convite, convidado \
com login que a vê, par externo, ou membro de outra máquina ou de fora no grupo) e não acessa integrações externas; use uma sessão só sua",
    check_failed_prefix: "integration_shared_session",
    check_failed_suffix: "; a sessão não acessa integrações externas sem saber se é compartilhada",
};

/// A própria sessão e os membros do grupo dela nesta máquina; `srv::nome` é de outra máquina ou de fora.
pub(super) fn local_members<'a>(me: &'a str, link: Option<&'a Sidecar>) -> Vec<&'a str> {
    let peers = link.into_iter().flat_map(|l| l.peers.iter().map(String::as_str)).filter(|p| !p.contains("::"));
    std::iter::once(me).chain(peers).collect()
}

/// Convite ou par vivo na própria sessão ou num membro do grupo nesta máquina (o grupo repassa o que ela lê).
/// Registro de convite de outra vida com o mesmo nome também conta: na dúvida, recusa.
pub(super) fn shared_here(store: &ShareStore, pairs: &PairStore, me: &str, link: Option<&Sidecar>) -> bool {
    local_members(me, link).iter().any(|m| store.session_has_active(m) || !pairs.by_local(m).is_empty())
}

/// Membro de outra máquina (do dono ou de fora) no grupo: daqui não se confere o que a outra máquina compartilha.
pub(super) fn has_remote_member(link: Option<&Sidecar>) -> bool {
    link.is_some_and(|l| l.peers.iter().any(|p| is_remote(p)))
}

/// Se o registro da dona de um grupo entre máquinas tem par de fora ou membro de outra máquina: a dona grava o
/// registro antes dos sidecars, então ele vale mesmo com o sidecar daqui ainda sem o membro. Sem as duas listas,
/// não dá para saber.
pub(super) fn record_is_shared(record: &Value, here: &str) -> Option<bool> {
    let outside = record.get("outside")?.as_array()?;
    let members = record.get("members")?.as_array()?;
    let elsewhere = members.iter().any(|m| m.get("server").and_then(Value::as_str) != Some(here));
    Some(!outside.is_empty() || elsewhere)
}

/// Falha fechada: grupo (ou o `fed` dele), convites, pares externos, convidados ou registro da dona que não se lê
/// também recusa. As leituras independentes correm juntas; a recusa segue a ordem das conferências.
pub(super) async fn ensure_unshared(
    st: &AppState,
    api: &Router,
    me: &str,
    messages: GuardMessages,
) -> Result<(), String> {
    let link = async {
        match &st.groups {
            Some(groups) => groups.link_strict(me).await.map_err(|_| messages.failed("grupo da sessão ilegível")),
            None => Ok(None),
        }
    };
    // A vida pela mesma fonte que o porteiro do convidado usa: sessão de outra vida com o mesmo nome não é dele.
    let (link, life) = tokio::join!(link, session_life(st, me));
    let link = link?;
    // Vale com ou sem configuração de convites: o que a outra máquina compartilha não se confere daqui.
    if has_remote_member(link.as_ref()) {
        return Err(messages.shared_refusal.into());
    }
    // Os membros do grupo nesta máquina passam pelas mesmas conferências: o grupo repassa o que a sessão lê.
    let others: Vec<String> = local_members(me, link.as_ref()).into_iter().skip(1).map(str::to_owned).collect();
    let other_lives = futures_util::future::join_all(others.iter().map(|m| session_life(st, m))).await;
    let members: Vec<(String, Option<String>)> = std::iter::once((me.to_owned(), life))
        .chain(others.into_iter().zip(other_lives))
        .collect();
    match &st.share {
        // Configuração de convidados que não se carregou não prova que não há convite nem par: recusa.
        None if st.share_unavailable => return Err(messages.failed("estado de compartilhamento indisponível")),
        // Sem configuração de convites não há convite nem par a conferir.
        None => {}
        Some(share) if share.pairs.unreadable() => return Err(messages.failed("arquivo de pares externos ilegível")),
        Some(share) if shared_here(&share.store, &share.pairs, me, link.as_ref()) => {
            return Err(messages.shared_refusal.into());
        }
        Some(_) => {}
    }
    let (guests, record) = tokio::join!(
        guest_unseen(st, members, messages),
        owner_record_unshared(st, api, link, messages),
    );
    guests.and(record)
}

/// Nenhum convidado com login vê a sessão nem um membro do grupo dela nesta máquina (nome e vida de cada um).
async fn guest_unseen(
    st: &AppState,
    members: Vec<(String, Option<String>)>,
    messages: GuardMessages,
) -> Result<(), String> {
    let guests = st.login_guests.store.clone();
    let seen = tokio::task::spawn_blocking(move || -> GuestResult<bool> {
        for (name, life) in &members {
            if guests.seen_by_any_guest(name, life.as_deref())? {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|_| messages.failed("leitura dos convidados interrompida"))?
    .map_err(|_| messages.failed("arquivo de convidados ilegível"))?;
    if seen {
        Err(messages.shared_refusal.into())
    } else {
        Ok(())
    }
}

/// Grupo entre máquinas: o par de fora feito noutra máquina só aparece no registro da dona.
async fn owner_record_unshared(
    st: &AppState,
    api: &Router,
    link: Option<Sidecar>,
    messages: GuardMessages,
) -> Result<(), String> {
    let (Some(link), Some(groups)) = (link, &st.groups) else {
        return Ok(());
    };
    let Some(fed) = &link.fed else {
        return Ok(());
    };
    let path = format!("/api/groups/{}", segment(&link.gid));
    let record = if fed.owner == groups.server_id() {
        local_group_get(st, api, &path)
            .await
            .map_err(|e| messages.failed(&format!("registro do grupo: {e}")))?
    } else {
        st.peers
            .call_group(&fed.owner, reqwest::Method::GET, &path, None)
            .await
            .map_err(|e| messages.failed(&e.text(&fed.owner)))?
    };
    match record_is_shared(&record, &groups.server_id()) {
        Some(false) => Ok(()),
        Some(true) => Err(messages.shared_refusal.into()),
        None => Err(messages.failed(&format!("a dona '{}' não disse quem está no grupo", fed.owner))),
    }
}

async fn local_group_get(st: &AppState, api: &Router, path: &str) -> Result<Value, String> {
    let Ok(mut request) = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("user-agent", "hangar-mcp")
        .header("authorization", format!("Bearer {}", st.cfg.auth_token))
        .body(Body::empty())
    else {
        return Err("caminho inválido".into());
    };
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    let Ok(response) = api.clone().call(request).await;
    let status = response.status().as_u16();
    let raw = axum::body::to_bytes(response.into_body(), ROUTE_BODY_LIMIT)
        .await
        .unwrap_or_default();
    let value: Value = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| json!({"detail": String::from_utf8_lossy(&raw)}));
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        let d = &value["detail"];
        let msg = d["msg"]
            .as_str()
            .or_else(|| d.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| if d.is_null() { format!("HTTP {status}") } else { d.to_string() });
        Err(match d["code"].as_str() {
            Some(code) => format!("{code}: {msg}"),
            None => msg,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login_guests::store::Store as GuestStore;
    use crate::share::pairs::ExternalPair;
    use crate::share::store::Kind;
    use std::sync::Arc;

    fn stores(dir: &std::path::Path) -> (ShareStore, PairStore) {
        (
            ShareStore::open(dir.join("shares.json"), Arc::new(|| 1_000.0)),
            PairStore::open(dir.join("external_pairs.json")),
        )
    }

    fn external(local: &str) -> ExternalPair {
        ExternalPair {
            share_id: "sh1".into(),
            local_session: local.into(),
            alias: "ana".into(),
            peer_owner: "Ana".into(),
            peer_session: "dela".into(),
            peer_address: "https://ana.tail.ts.net:8443".into(),
            peer_token: "t".repeat(24),
            created_at: 1.0,
        }
    }

    fn group(peers: &[&str]) -> Sidecar {
        Sidecar {
            peers: peers.iter().map(|p| (*p).to_owned()).collect(),
            gid: "g1".into(),
            ..Sidecar::default()
        }
    }

    #[test]
    fn shared_here_detects_invites_external_pairs_and_local_group_peers() {
        let dir = tempfile::tempdir().unwrap();
        let (store, pairs) = stores(dir.path());
        assert!(!shared_here(&store, &pairs, "s0", None));
        assert!(!shared_here(&store, &pairs, "s0", Some(&group(&["s1", "s2"]))));

        store.create("s1", None, Kind::Share, false).unwrap();
        assert!(shared_here(&store, &pairs, "s1", None));
        assert!(!shared_here(&store, &pairs, "s0", None));
        assert!(shared_here(&store, &pairs, "s0", Some(&group(&["s1"]))));

        pairs.add(external("s2")).unwrap();
        assert!(shared_here(&store, &pairs, "s2", None));
        assert!(shared_here(&store, &pairs, "s0", Some(&group(&["s2", "ana::dela"]))));
    }

    #[test]
    fn has_remote_member_and_record_is_shared_fail_closed() {
        assert!(!has_remote_member(None));
        assert!(!has_remote_member(Some(&group(&["s1", "s2"]))));
        assert!(has_remote_member(Some(&group(&["s1", "lab::s1"]))));
        assert!(has_remote_member(Some(&group(&["ana::dela"]))));

        let local = json!([{"server": "casa", "name": "s0"}, {"server": "casa", "name": "s1"}]);
        assert_eq!(record_is_shared(&json!({"members": local, "outside": []}), "casa"), Some(false));
        assert_eq!(
            record_is_shared(&json!({"members": local, "outside": [{"server": "lab", "address": "ana::dela"}]}), "casa"),
            Some(true)
        );
        let remote = json!([{"server": "casa", "name": "s0"}, {"server": "lab", "name": "s1"}]);
        assert_eq!(record_is_shared(&json!({"members": remote, "outside": []}), "casa"), Some(true));
        assert_eq!(record_is_shared(&json!({"members": local}), "casa"), None);
    }

    #[test]
    fn messages_preserve_prefixes_for_whatsapp_and_integrations() {
        assert!(WHATSAPP.shared_refusal.starts_with("whatsapp_shared_session:"));
        assert!(WHATSAPP.failed("x").starts_with("whatsapp_shared_check_failed: x"));
        assert!(INTEGRATIONS.shared_refusal.starts_with("integration_shared_session:"));
        assert!(INTEGRATIONS.failed("x").starts_with("integration_shared_session: x"));
    }

    #[test]
    fn login_guest_and_corrupted_guests_store_are_caught() {
        let dir = tempfile::tempdir().unwrap();
        let store = GuestStore::new(dir.path().join("guests.json"));
        store.create("ana", &dir.path().to_string_lossy(), false, true, "ana".into(), "tok-a".into()).unwrap();
        assert!(!store.seen_by_any_guest("s0", None).unwrap());
        store.claim("s1", "ana", Some("v1")).unwrap();
        assert!(store.seen_by_any_guest("s1", Some("v1")).unwrap());
        std::fs::write(dir.path().join("guests.json"), "{torto").unwrap();
        assert!(store.seen_by_any_guest("s0", None).is_err());
    }
}
