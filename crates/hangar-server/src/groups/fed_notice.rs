//! Aviso de grupo e contrato num grupo entre máquinas: os dois passam pela dona. O aviso não entra
//! na fila (`outbox.rs`): a dona conta o limite de tempestade, tenta cada máquina direto e uma vez,
//! e quem falha volta em `pulados`, qualificado (`srv::nome`, ou o endereço do par de fora). Aviso
//! que veio de fora (`from_outside`) nunca sai a outro par de fora: o salto único vale na fronteira.
//! O contrato mora só na dona; as outras leem e gravam por ela, com o `mtime` lido (I-4A).
use std::collections::BTreeSet;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::exit::segment;
use super::federation::{Machine, Refusal, missing, no, unavailable};
use super::local::is_remote;
use super::model::{PROTOCOL, Sidecar};
use super::peers::PeerError;
use super::routes::{Asked, STORM_MAX, STORM_WINDOW, envelope, failures_text, outside_addresses, storm};
use super::service::GroupService;
use crate::routes::AppState;
use crate::session_write::input::deliver_text;
use crate::session_write::{detail_body, json_response};

const OWNER_DOWN: &str = "erro_grupo_dono_indisponivel";

/// Quanto a dona espera cada máquina, e as sessões dela, receberem o aviso (tudo em paralelo).
/// Menor que `peers::OWNER_WAIT_TIMEOUT` de quem pediu: a resposta chega antes de ele desistir.
pub const DELIVER_WAIT: Duration = Duration::from_secs(5);
/// Quanto cada máquina espera as sessões dela no `deliver`: abaixo do `DELIVER_WAIT` da dona, para
/// responder com as falhas dela antes de a dona dar a máquina inteira por pulada.
fn session_wait(deliver: Duration) -> Duration { deliver * 3 / 5 }
const _: () = assert!(DELIVER_WAIT.as_secs() < super::peers::OWNER_WAIT_TIMEOUT.as_secs()
    && super::federation::JOIN_WAIT.as_secs() < super::peers::OWNER_WAIT_TIMEOUT.as_secs());

fn deliver_wait(groups: &GroupService) -> Duration { super::federation::timing(groups.pair_root()).deliver_wait }

fn owner_down(owner: &str) -> Refusal {
    no(StatusCode::SERVICE_UNAVAILABLE, OWNER_DOWN, &format!("a máquina dona do grupo ({owner}) não respondeu"), json!({"servidor": owner}))
}

/// Recusa com código da dona volta como veio; o resto (rede, modo `python`, versão antiga) é dona fora.
fn from_owner(owner: &str, error: PeerError) -> Refusal {
    match error {
        PeerError::Refused { status, detail } if status != 503 && detail.get("code").is_some() =>
            Refusal(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), json!({"detail": detail})),
        other => {
            tracing::warn!(code = OWNER_DOWN, server = %owner, error = %other.text(owner), "groups: a dona do grupo não respondeu");
            owner_down(owner)
        }
    }
}

/// Limite de tempestade de um grupo entre máquinas nesta máquina (a dona; ou quem manda, com ela fora).
fn stormed(me: &str, gid: &str) -> Option<Refusal> {
    storm(&format!("{me}::{gid}")).then(|| no(StatusCode::TOO_MANY_REQUESTS, "erro_group_message_tempestade",
        &format!("mais de {STORM_MAX} avisos de grupo em {}s — parece loop; espere ou responda 1:1", STORM_WINDOW.as_secs()),
        json!({"max": STORM_MAX, "janela": STORM_WINDOW.as_secs()})))
}

fn qualify(me: &str, label: &str) -> String { if label.contains("::") { label.to_owned() } else { format!("{me}::{label}") } }

// --- aviso -----------------------------------------------------------------------------------

/// Corpo do `op: message` (à dona) e do `POST /api/groups/{gid}/deliver` (dela às outras).
/// `origin` = máquina de quem mandou, ou a ponta do par de fora; `notice` vazio = a dona gera.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice { pub protocol: u32, pub origin: String, pub sender: String, pub text: String, pub notice: String, #[serde(default)] pub from_outside: bool }

impl Notice {
    pub fn new(origin: String, sender: String, text: String, notice: String, from_outside: bool) -> Notice {
        Notice { protocol: PROTOCOL, origin, sender, text, notice, from_outside }
    }

    /// O texto que as sessões de `me` recebem.
    fn line(&self, me: &str) -> String {
        if self.from_outside { format!("[de fora: {} via {}] {}", self.sender, self.origin, self.text) }
        else if self.origin == me { format!("[grupo: {}] {}", self.sender, self.text) }
        else { format!("[grupo: {}::{}] {}", self.origin, self.sender, self.text) }
    }

    fn body(&self) -> Value { serde_json::to_value(self).unwrap_or_default() }

    /// O `op: message` à dona.
    fn ops_body(&self) -> Value {
        let mut ask = self.body();
        ask["op"] = json!("message");
        ask
    }
}

/// Entrega às sessões daqui do grupo `gid` (menos quem mandou) e, se o aviso não veio de fora, aos
/// pares de fora daqui, todos ao mesmo tempo e cada um no máximo `wait`. Devolve quem não recebeu
/// (ou não confirmou no prazo): nome daqui ou endereço de fora, com o erro.
async fn deliver_here(st: &AppState, groups: &GroupService, gid: &str, n: &Notice, wait: Duration) -> Result<Vec<(String, Value)>, Refusal> {
    use crate::share::pair_routes::{Direct, notice_claim, notice_key, notify_outside};
    use futures_util::FutureExt as _;
    let me = groups.server_id();
    let all = groups.sidecars().await.map_err(|e| { tracing::error!(code = "groups_store_failed", error = %e, "groups: sidecars ilegíveis no aviso"); unavailable("groups_store_failed") })?;
    let mine: Vec<(String, Sidecar)> = all.into_iter().filter(|(_, s)| s.gid == gid && s.fed.is_some()).collect();
    let line = n.line(&me);
    let mut sends: Vec<(String, futures_util::future::BoxFuture<'_, Result<(), Value>>)> = Vec::new();
    for (name, _) in &mine {
        if !n.from_outside && n.origin == me && *name == n.sender { continue; }
        // O mesmo aviso pode chegar direto do par de fora e pela dona: entra uma vez. A espera por
        // outra entrega em andamento fica dentro do prazo `wait` abaixo.
        let (key, line) = (notice_key(&me, &n.notice, name), &line);
        sends.push((name.clone(), async move {
            let Some(claim) = notice_claim(key).await else { return Ok(()) };
            deliver_text(st, name, line).await.map(|()| claim.delivered())
        }.boxed()));
    }
    if let Some(share) = st.share.as_ref().filter(|_| !n.from_outside) {
        let outside = outside_addresses(st);
        let addresses: BTreeSet<&String> = mine.iter().flat_map(|(_, s)| s.peers.iter()).filter(|p| outside.contains(*p)).collect();
        for rec in addresses.into_iter().filter_map(|a| share.pairs.by_address(a)) {
            let address = rec.address();
            sends.push((address, async move { notify_outside(st, share, &Direct, &rec, &n.sender, &n.text, &n.notice).await }.boxed()));
        }
    }
    // shortcut: a entrega que estoura o prazo é abandonada e vai como pulada, mesmo se chegar depois.
    // Ganha confirmação tardia quando o pulado falso levar quem mandou a repetir o aviso.
    let done = futures_util::future::join_all(sends.into_iter().map(|(label, send)| async move {
        let late = || envelope("erro_envio_falhou", "não deu pra enviar: groups_notice_timeout".into(), json!({"erro": "groups_notice_timeout"}));
        (label, tokio::time::timeout(wait, send).await.unwrap_or_else(|_| Err(late())))
    })).await;
    Ok(done.into_iter().filter_map(|(label, sent)| sent.err().map(|e| (label, e))).collect())
}

/// Na dona: limite de tempestade do grupo e o aviso a cada máquina dele menos a de origem (que
/// entrega aos dela). Devolve o `notice` usado e os pulados, qualificados.
async fn owner_fanout(st: &AppState, groups: &GroupService, gid: &str, mut n: Notice) -> Result<(String, Vec<String>), Refusal> {
    let me = groups.server_id().to_owned();
    let (dir, g) = (groups.dir(), gid.to_owned());
    let record = tokio::task::spawn_blocking(move || dir.record(&g)).await.map_err(|e| store_failed(st, e))?
        .map_err(|e| store_failed(st, e))?.filter(|r| r.owner == me).ok_or_else(|| missing(gid))?;
    if let Some(refused) = stormed(&me, gid) { return Err(refused); }
    if n.notice.is_empty() { n.notice = crate::mods::state::random_hex(8); }
    let servers: Vec<&str> = record.members.iter().map(|m| m.server.as_str()).filter(|s| *s != me && *s != n.origin)
        .collect::<BTreeSet<_>>().into_iter().collect();
    let (path, body) = (format!("/api/groups/{}/deliver", segment(gid)), n.body());
    let (wait, path, body) = (deliver_wait(groups), &path, &body);
    let remote = futures_util::future::join_all(servers.iter().map(|s| async move {
        tokio::time::timeout(wait, st.peers.call_group(s, reqwest::Method::POST, path, Some(body))).await
            .unwrap_or_else(|_| Err(PeerError::Transport(format!("{s} não respondeu ao aviso em {} s", wait.as_secs_f32()))))
    }));
    // As sessões daqui ao mesmo tempo que as outras máquinas, no mesmo prazo; a de origem entrega as dela.
    let here = async { if n.origin == me { Ok(Vec::new()) } else { deliver_here(st, groups, gid, &n, wait).await } };
    let (answers, here) = tokio::join!(remote, here);
    let mut skipped: Vec<String> = here?.into_iter().map(|(label, _)| qualify(&me, &label)).collect();
    for (server, answer) in servers.iter().zip(answers) {
        match answer {
            Ok(v) => skipped.extend(v["failed"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|f| qualify(server, f))),
            Err(error) => {
                tracing::warn!(code = "groups_notice_not_delivered", %server, error = %error.text(server), "groups: o aviso não chegou a uma máquina do grupo");
                skipped.extend(record.members.iter().filter(|m| m.server == *server).map(|m| format!("{}::{}", m.server, m.name)));
            }
        }
    }
    Ok((n.notice, skipped))
}

/// `group-message` de uma sessão daqui num grupo entre máquinas.
pub(super) async fn group_message(asked: &Asked, link: &Sidecar, text: String) -> Response {
    let me = asked.groups.server_id().to_owned();
    let owner = link.owner_or(&me);
    let mut n = Notice::new(me.clone(), asked.name.clone(), text, String::new(), false);
    let mut down = false;
    let skipped = if owner == me {
        match owner_fanout(&asked.st, &asked.groups, &link.gid, n.clone()).await {
            Ok((notice, skipped)) => { n.notice = notice; skipped }
            Err(Refusal(status, body)) => return asked.reply(status, body),
        }
    } else {
        match asked.st.peers.call_group_waiting(&owner, &format!("/api/groups/{}/ops", segment(&link.gid)), &n.ops_body()).await {
            Ok(v) => {
                n.notice = v["notice"].as_str().unwrap_or_default().to_owned();
                v["pulados"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
            }
            Err(error) => match from_owner(&owner, error) {
                Refusal(status, body) if status != StatusCode::SERVICE_UNAVAILABLE => return asked.reply(status, body),
                // Dona fora: os membros daqui ainda se avisam, com o limite contado aqui.
                _ => {
                    if let Some(Refusal(status, body)) = stormed(&me, &link.gid) { return asked.reply(status, body); }
                    down = true;
                    let outside = outside_addresses(&asked.st);
                    link.peers.iter().filter(|p| is_remote(p) && !outside.contains(*p)).cloned().collect()
                }
            },
        }
    };
    if n.notice.is_empty() { n.notice = crate::mods::state::random_hex(8); }
    let failed = match deliver_here(&asked.st, &asked.groups, &link.gid, &n, deliver_wait(&asked.groups)).await {
        Ok(failed) => failed,
        Err(Refusal(status, body)) => return asked.reply(status, body),
    };
    let failed: Vec<Value> = failed.into_iter().map(|(sessao, erro)| json!({"sessao": sessao, "erro": erro})).collect();
    let warning = if !failed.is_empty() {
        envelope("erro_pareamento_grupo_falha", format!("falha em: {}", failures_text(&failed)), json!({"avisos": failed}))
    } else if down {
        envelope(OWNER_DOWN, format!("a máquina dona do grupo ({owner}) não respondeu: só os membros daqui receberam"), json!({"servidor": owner}))
    } else { Value::Null };
    asked.reply(StatusCode::OK, json!({"ok": true, "peers": link.peers, "pulados": skipped, "warning": warning}))
}

/// `op: message` na dona.
pub(super) async fn owner_message(st: &AppState, groups: &GroupService, gid: &str, n: Notice) -> Result<Value, Refusal> {
    let (notice, skipped) = owner_fanout(st, groups, gid, n).await?;
    Ok(json!({"protocol": PROTOCOL, "notice": notice, "pulados": skipped}))
}

/// Aviso de grupo que chegou de fora à sessão `anchor` daqui, já entregue aos membros daqui: em
/// grupo entre máquinas sobe à dona, que leva às outras máquinas e a nenhum par de fora.
/// Roda depois da resposta a quem é de fora; falha só ao diário. O fim da tarefa é o fim da
/// entrega (a dona só responde depois de levar às outras máquinas).
pub fn relay_outside(st: Arc<AppState>, anchor: String, sender: String, text: String, notice: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(groups) = st.groups.clone() else { return };
        let link = match groups.link(&anchor).await {
            Ok(Some(link)) => link,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(code = "groups_outside_notice_not_relayed", session = %anchor, %error, "groups: grupo da sessão ilegível, o aviso de fora não subiu à dona");
                return st.diag.report("rust.groups_store_failed", &anchor, "groups_store_failed", "grupo da sessão ilegível; o aviso de fora não subiu à dona");
            }
        };
        let Some(owner) = link.fed.map(|f| f.owner) else { return };
        let me = groups.server_id().to_owned();
        let n = Notice::new(me.clone(), sender, text, notice, true);
        let done = if owner == me { owner_fanout(&st, &groups, &link.gid, n).await.map(|(_, skipped)| skipped.len()) } else {
            st.peers.call_group_waiting(&owner, &format!("/api/groups/{}/ops", segment(&link.gid)), &n.ops_body()).await
                .map(|v| v["pulados"].as_array().map_or(0, Vec::len)).map_err(|e| from_owner(&owner, e))
        };
        match done {
            Ok(0) => {}
            Ok(skipped) => tracing::warn!(code = "groups_outside_notice_partial", session = %anchor, skipped, "groups: aviso de fora não chegou a todas as máquinas"),
            Err(Refusal(status, body)) => tracing::warn!(code = "groups_outside_notice_not_relayed", session = %anchor, %status,
                detail = %body["detail"]["code"], "groups: aviso de fora não subiu à dona"),
        }
    })
}

// --- rotas entre máquinas --------------------------------------------------------------------

/// `POST /api/groups/{gid}/deliver`: a dona entrega um aviso aos membros desta máquina.
pub(super) async fn deliver(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let m = match Machine::take(st, peer, req).await { Ok(m) => m, Err(r) => return r };
    let n = match m.body::<Notice>() {
        Ok(n) if n.protocol == PROTOCOL && !n.notice.is_empty() && n.notice.len() <= 64 => n,
        Ok(_) => return m.invalid(),
        Err(r) => return r,
    };
    let me = m.groups.server_id().to_owned();
    let known = m.groups.sidecars().await.map(|all| all.iter().any(|(_, s)| s.gid == m.gid && s.fed.is_some()));
    let result = match known {
        Ok(true) => deliver_here(&m.st, &m.groups, &m.gid, &n, session_wait(deliver_wait(&m.groups))).await
            .map(|failed| json!({"protocol": PROTOCOL, "failed": failed.into_iter().map(|(label, _)| qualify(&me, &label)).collect::<Vec<_>>()})),
        Ok(false) => Err(missing(&m.gid)),
        Err(e) => Err(store_failed(&m.st, e)),
    };
    m.answer(result)
}

// --- contrato --------------------------------------------------------------------------------

/// `mtime` do contrato: nanossegundos em texto (o número não cabe exato no JSON de quem lê em JS);
/// `None` = ainda não existe.
fn mtime_of(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_nanos().to_string())
}

fn read_contract(path: PathBuf) -> std::io::Result<(String, Option<String>)> {
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok((content, std::fs::metadata(&path).ok().as_ref().and_then(mtime_of))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), None)),
        Err(e) => Err(e),
    }
}

/// 500, não o 503 de grupo indisponível: é o status do `/pair/contract` (como no Python), e a dona o
/// repassa a quem pediu de outra máquina, que o lê como recusa dela e não como "dona fora do ar".
fn unreadable(error: impl std::fmt::Display) -> Refusal {
    tracing::error!(code = "groups_contract_unreadable", %error, "groups: o contrato do grupo não foi lido ou gravado");
    no(StatusCode::INTERNAL_SERVER_ERROR, "erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": "groups_contract_unreadable"}))
}

/// Grava se o `mtime` lido ainda é o do arquivo; sem `mtime`, só cria. Sob o lock do grupo: duas
/// gravações nunca passam pela mesma leitura.
async fn write_contract(groups: &GroupService, gid: &str, content: String, expected: Option<String>) -> Result<String, Refusal> {
    let path = groups.contract_path(gid);
    let work = tokio::task::spawn_blocking(move || -> Result<String, Refusal> {
        let current = match std::fs::metadata(&path) {
            Ok(meta) => mtime_of(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(unreadable(e)),
        };
        if current != expected {
            return Err(no(StatusCode::CONFLICT, "erro_contrato_mudou", "o contrato mudou desde a leitura — leia de novo", json!({"mtime": current})));
        }
        let written = if expected.is_none() {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut f| f.write_all(content.as_bytes()))
        } else { super::store::replace_atomic(&path, content.as_bytes()) };
        written.map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists {
            no(StatusCode::CONFLICT, "erro_contrato_mudou", "o contrato mudou desde a leitura — leia de novo", json!({}))
        } else { unreadable(e) })?;
        std::fs::metadata(&path).ok().as_ref().and_then(mtime_of).ok_or_else(|| unreadable("mtime ausente"))
    });
    groups.locked(work).await.map_err(unreadable)?
}

/// A dona do grupo `gid` é esta máquina (registro gravado).
async fn owned_here(st: &AppState, groups: &GroupService, gid: &str) -> Result<(), Refusal> {
    let (dir, g, me) = (groups.dir(), gid.to_owned(), groups.server_id().to_owned());
    match tokio::task::spawn_blocking(move || dir.record(&g)).await {
        Ok(Ok(Some(r))) if r.owner == me => Ok(()),
        Ok(Ok(_)) => Err(missing(gid)),
        Ok(Err(e)) => Err(store_failed(st, e)),
        Err(e) => Err(store_failed(st, e)),
    }
}

/// Disco do grupo ilegível: a causa vai ao log e ao diário, e quem pediu recebe o código.
fn store_failed(st: &AppState, error: impl std::fmt::Display) -> Refusal {
    st.diag.report("rust.groups_store_failed", "", "groups_store_failed", "o grupo entre máquinas não foi lido no aviso ou no contrato");
    super::federation::store_refusal(error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerPut { protocol: u32, content: String, mtime: Option<String> }

/// `GET|PUT /api/groups/{gid}/contract`: o contrato na dona, para as outras máquinas do dono.
pub(super) async fn owner_contract(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let m = match Machine::take(st, peer, req).await { Ok(m) => m, Err(r) => return r };
    if let Err(refused) = owned_here(&m.st, &m.groups, &m.gid).await { return m.answer(Err(refused)); }
    let result = if m.method() == Method::PUT {
        let body = match m.body::<OwnerPut>() { Ok(b) if b.protocol == PROTOCOL => b, Ok(_) => return m.invalid(), Err(r) => return r };
        write_contract(&m.groups, &m.gid, body.content, body.mtime).await.map(|mtime| json!({"protocol": PROTOCOL, "mtime": mtime}))
    } else {
        let path = m.groups.contract_path(&m.gid);
        match tokio::task::spawn_blocking(move || read_contract(path)).await {
            Ok(Ok((content, mtime))) => Ok(json!({"protocol": PROTOCOL, "content": content, "mtime": mtime})),
            Ok(Err(e)) => Err(unreadable(e)),
            Err(e) => Err(unreadable(e)),
        }
    };
    m.answer(result)
}

/// `GET /pair/contract`: o arquivo local na dona (ausente = vazio, ilegível = 500); em outra máquina
/// do grupo, pela dona (`path` vazio).
pub(super) async fn session_contract(asked: &Asked, link: &Sidecar) -> Response {
    let me = asked.groups.server_id().to_owned();
    let owner = link.owner_or(&me);
    let read = if owner == me {
        let path = asked.groups.contract_path(&link.gid);
        let shown = path.to_string_lossy().into_owned();
        match tokio::task::spawn_blocking(move || read_contract(path)).await {
            Ok(Ok((content, mtime))) => Ok((shown, content, json!(mtime))),
            Ok(Err(e)) => Err(unreadable(e)),
            Err(e) => Err(unreadable(e)),
        }
    } else {
        asked.st.peers.call_group(&owner, reqwest::Method::GET, &format!("/api/groups/{}/contract", segment(&link.gid)), None).await
            .map(|v| (String::new(), v["content"].as_str().unwrap_or_default().to_owned(), v["mtime"].clone()))
            .map_err(|e| from_owner(&owner, e))
    };
    match read {
        Ok((path, content, mtime)) => asked.reply(StatusCode::OK, json!({"peers": link.peers, "path": path, "content": content, "owner": owner, "mtime": mtime})),
        Err(Refusal(status, body)) => asked.reply(status, body),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionPut { content: String, mtime: Option<String> }

/// `PUT /api/sessions/{name}/pair/contract {content, mtime}` → `{mtime}` ou 409 `erro_contrato_mudou`.
pub(super) async fn session_contract_put(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let body = match asked.body::<SessionPut>() { Ok(body) => body, Err(refused) => return refused };
    let link = match asked.groups.link(&asked.name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    let Some(link) = link else {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_pareada", "sessão não está pareada", json!({}));
    };
    let me = asked.groups.server_id().to_owned();
    let owner = link.owner_or(&me);
    let written = if owner == me { write_contract(&asked.groups, &link.gid, body.content, body.mtime).await } else {
        let ask = json!({"protocol": PROTOCOL, "content": body.content, "mtime": body.mtime});
        asked.st.peers.call_group(&owner, reqwest::Method::PUT, &format!("/api/groups/{}/contract", segment(&link.gid)), Some(&ask)).await
            .map(|v| v["mtime"].as_str().unwrap_or_default().to_owned()).map_err(|e| from_owner(&owner, e))
    };
    match written {
        Ok(mtime) => asked.reply(StatusCode::OK, json!({"mtime": mtime})),
        Err(Refusal(status, body)) => asked.reply(status, body),
    }
}

// --- repasse da porta do Connect -------------------------------------------------------------

/// Rotas desta parte sob `/api/groups/{gid}/`.
pub fn owns_tail(tail: &str) -> bool { matches!(tail, "deliver" | "contract") }

/// Pedido que o Python das portas dele repassou (já autenticado como dono).
pub(super) async fn serve_forwarded(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let (method, deliver_path) = (req.method().clone(), req.uri().path().ends_with("/deliver"));
    match (method, deliver_path) {
        (Method::POST, true) => deliver(State(st), ConnectInfo(peer), req).await,
        (Method::GET | Method::PUT, false) => owner_contract(State(st), ConnectInfo(peer), req).await,
        _ => json_response(StatusCode::METHOD_NOT_ALLOWED, detail_body("erro_metodo_nao_permitido", "método não aceito nesta rota", json!({}))),
    }
}
