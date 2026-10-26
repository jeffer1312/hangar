//! Grupo entre máquinas do dono. Uma máquina dona por grupo grava o `GroupRecord` e empurra o
//! grupo às outras pela fila (`outbox.rs`); cada máquina só grava os sidecars dos membros dela.
//! O pedido de entrada nasce em qualquer máquina (`pair`), a dona aplica (`owner_join`) e cada
//! participante confirma pelo `PUT /api/groups/{gid}` (`local_apply`). Sob o lock do grupo só disco
//! e fila; rede, espera da fila e avisos às sessões vêm depois.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::{Bytes, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::Response;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::exit::{NO_SERVER_ID, segment};
use super::local::{Snapshot, is_remote};
use super::model::{Fed, GroupRecord, Member, OutsideRef, PROTOCOL, Sidecar};
use super::outbox::{Delivery, Fan, OutboxItem, OutboxSend, OutsideOp, QUEUE_FULL, fan_out, remote_servers};
use super::peers::{PeerClient, PeerError};
use super::routes::{Asked, Bridged, outside_addresses, providers};
use super::service::{BoxFuture, GroupService, restore_logged};
use super::store::PairDir;
use super::text::{ContractLine, Kind, TextArgs, protocol_text};
use crate::routes::{AppState, cors, gate};
use crate::runtime::ingress::IngressPass;
use crate::session_write::input::{deliver_text, json_content_type};
use crate::session_write::{BODY_LIMIT, detail_body, json_response};

pub use super::outbox::Outbox;

/// Quanto a dona espera cada participante confirmar. Menor que `peers::OWNER_WAIT_TIMEOUT`, o prazo
/// de quem chamou a dona de outra máquina: a resposta chega antes de ele desistir dela.
pub const JOIN_WAIT: Duration = Duration::from_secs(12);

const CONTRACT_FAILED: &str = "erro_grupo_contrato_nao_herdado";

pub struct Federation {
    outbox: Outbox,
}

impl Federation {
    pub fn outbox(&self) -> &Outbox { &self.outbox }
}

/// Prazos e escadas do grupo entre máquinas de uma pasta `.hangar-pair`.
#[derive(Clone, Debug)]
pub struct Timing {
    /// Quanto a dona espera cada participante confirmar um join (lido a cada join).
    pub join_wait: Duration,
    /// Escada da fila de saída (lida ao abrir a fila).
    pub ladder: Vec<Duration>,
    /// Quanto a dona espera cada máquina receber um aviso (lido a cada aviso).
    pub deliver_wait: Duration,
    /// Escada das novas tentativas de converter par antigo (lida quando a volta começa).
    pub retry_ladder: Vec<Duration>,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { join_wait: JOIN_WAIT, ladder: super::outbox::LADDER.to_vec(), deliver_wait: super::fed_notice::DELIVER_WAIT,
            retry_ladder: super::convert::RETRY_LADDER.to_vec() }
    }
}

/// Uma por pasta `.hangar-pair` (uma por processo; os testes sobem várias máquinas num processo).
static ALL: LazyLock<Mutex<HashMap<PathBuf, Arc<Federation>>>> = LazyLock::new(Mutex::default);
static TIMINGS: LazyLock<Mutex<HashMap<PathBuf, Timing>>> = LazyLock::new(Mutex::default);

/// Prazos da máquina de `root` (testes); cada um vale a partir do momento em que é lido.
pub fn configure(root: &Path, timing: Timing) {
    TIMINGS.lock().unwrap_or_else(|e| e.into_inner()).insert(root.to_path_buf(), timing);
}

pub(super) fn timing(root: &Path) -> Timing {
    TIMINGS.lock().unwrap_or_else(|e| e.into_inner()).get(root).cloned().unwrap_or_default()
}

/// A federação já aberta da máquina de `root`.
pub fn existing(root: &Path) -> Option<Arc<Federation>> {
    ALL.lock().unwrap_or_else(|e| e.into_inner()).get(root).cloned()
}

/// Esquece a federação aberta de `root`: o próximo uso reabre do disco, como depois de reiniciar.
pub fn forget(root: &Path) { ALL.lock().unwrap_or_else(|e| e.into_inner()).remove(root); }

/// Abre (uma vez) a fila desta máquina, que já põe para andar o que ficou no disco.
pub async fn of(st: &AppState, groups: &GroupService) -> Result<Arc<Federation>, Refusal> {
    let root = groups.pair_root().to_path_buf();
    if let Some(found) = existing(&root) { return Ok(found); }
    let sender = Arc::new(Sender { peers: st.peers.clone(), dir: groups.dir(), diag: st.diag.clone(), lock: groups.lock_handle() });
    let opened = tokio::task::spawn_blocking(move || -> std::io::Result<Arc<Federation>> {
        let mut all = ALL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = all.get(&root) { return Ok(found.clone()); }
        let outbox = Outbox::open_with_ladder(root.join("groups"), sender, timing(&root).ladder)?;
        let fed = Arc::new(Federation { outbox });
        all.insert(root, fed.clone());
        Ok(fed)
    }).await;
    match opened {
        Ok(Ok(fed)) => Ok(fed),
        Ok(Err(error)) => { tracing::error!(code = "groups_outbox_open_failed", %error, "groups: a fila entre máquinas não abriu"); Err(unavailable("groups_outbox_open_failed")) }
        Err(error) => { tracing::error!(code = "groups_outbox_open_failed", %error, "groups: a fila entre máquinas não abriu"); Err(unavailable("groups_outbox_open_failed")) }
    }
}

// --- recusas ---------------------------------------------------------------------------------

/// Status e corpo `{"detail": {...}}` de uma recusa.
pub struct Refusal(pub StatusCode, pub Value);

pub(super) fn no(status: StatusCode, code: &str, msg: &str, params: Value) -> Refusal { Refusal(status, detail_body(code, msg, params)) }

pub(super) fn unavailable(detail: &str) -> Refusal {
    no(StatusCode::SERVICE_UNAVAILABLE, "erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": detail}))
}

pub(super) fn store_refusal(error: impl std::fmt::Display) -> Refusal {
    tracing::error!(code = "groups_store_failed", %error, "groups: o grupo entre máquinas não foi lido ou gravado");
    unavailable("groups_store_failed")
}

pub(super) fn missing(gid: &str) -> Refusal { no(StatusCode::NOT_FOUND, "erro_grupo_inexistente", "grupo inexistente nesta máquina", json!({"gid": gid})) }

fn need_server_id(me: &str) -> Result<(), Refusal> {
    if me.is_empty() { Err(no(StatusCode::BAD_REQUEST, "erro_pareamento_server_id_ausente", NO_SERVER_ID, json!({}))) } else { Ok(()) }
}

/// `missing` = os nomes já juntos para a mensagem.
fn sessions_missing(missing: &str) -> Refusal {
    no(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada_detalhe", &format!("sessão não encontrada: {missing}"), json!({"detalhe": missing}))
}

/// Os de `wanted` que a lista não achou, juntos para a mensagem; vazio = todos achados.
fn not_found(wanted: &[String], harness: &BTreeMap<String, String>) -> String {
    wanted.iter().filter(|n| !harness.contains_key(*n)).cloned().collect::<Vec<_>>().join(", ")
}

fn changed_id(saved: &str, current: &str) -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_identificador_trocado",
        &format!("o CP_SERVER_ID desta máquina mudou de '{saved}' para '{current}' com grupo entre máquinas vivo"),
        json!({"salvo": saved, "atual": current}))
}

fn unregistered(server: &str) -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_peer_nao_registrado", &format!("a máquina '{server}' não está no peers.json desta"), json!({"servidor": server}))
}

fn old_version(server: &str) -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_peer_versao_antiga", &format!("{server} está numa versão do Hangar sem grupo entre máquinas"), json!({"servidor": server}))
}

fn peer_refusal(server: &str, error: PeerError) -> Refusal {
    match error {
        PeerError::OldVersion => old_version(server),
        PeerError::Unknown => unregistered(server),
        PeerError::IdentityMismatch { key, reported } => no(StatusCode::CONFLICT, "erro_grupo_identificador_divergente",
            &PeerError::IdentityMismatch { key: key.clone(), reported: reported.clone() }.text(server), json!({"chave": key, "informado": reported})),
        // Recusa com código da outra máquina volta como veio.
        PeerError::Refused { status, detail } if detail.get("code").is_some() =>
            Refusal(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), json!({"detail": detail})),
        other => no(StatusCode::BAD_GATEWAY, "erro_grupo_dono_indisponivel", &other.text(server), json!({"servidor": server})),
    }
}

/// A identidade gravada bate com a atual (ou ainda não há grupo entre máquinas).
pub(super) fn identity_ok(dir: &PairDir, me: &str) -> Result<(), Refusal> {
    match dir.identity_check(me) {
        Ok(None) => Ok(()),
        Ok(Some(changed)) => Err(changed_id(&changed.saved, &changed.current)),
        Err(error) => Err(store_refusal(error)),
    }
}

// --- disco -----------------------------------------------------------------------------------

pub(super) fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Result<T, Refusal>> {
    async move { tokio::task::spawn_blocking(f).await.map_err(store_refusal) }
}

/// Sidecar ilegível vale como sem grupo, como no `GroupService`.
pub(super) fn read(dir: &PairDir, name: &str) -> Option<Sidecar> { dir.sidecar(name).ok().flatten() }

/// Os membros do grupo de `name` como o sidecar dele os vê: ele, e cada peer que não é par de
/// fora. O provedor vem do `harness` pela chave como o peer aparece aqui (`srv::nome` de outra
/// máquina, nome cru daqui): é assim que a dona e o `PUT` o gravam.
pub(super) fn members_from_sidecar(me: &str, name: &str, s: &Sidecar, outside: &HashSet<String>) -> Vec<Member> {
    let provider = |key: &str| s.harness.get(key).map_or("", String::as_str).to_owned();
    std::iter::once(Member::new(me, name, &provider(name)))
        .chain(s.peers.iter().filter(|p| !outside.contains(*p)).map(|p| Member { provider: provider(p), ..Member::parse(me, p) }))
        .collect()
}

// --- fila: o pedido HTTP de cada item --------------------------------------------------------

struct Sender { peers: Arc<PeerClient>, dir: Arc<PairDir>, diag: crate::diag::DiagClient, lock: Arc<tokio::sync::Mutex<()>> }

/// A máquina continuou ignorando o grupo mesmo depois de a dona pular a versão que ela lembra.
pub const VERSION_BEHIND: &str = "erro_grupo_versao_atrasada";

impl Sender {
    /// Envia o registro atual. `ignored` com versão maior que a enviada = a máquina lembra uma vida
    /// anterior deste gid (grupo desfeito e refeito daqui): a dona pula para depois dela e reenvia,
    /// senão a entrada some calada do outro lado.
    async fn push(&self, server: &str, gid: &str, joining: &[String]) -> Result<(), PeerError> {
        let path = format!("/api/groups/{}", segment(gid));
        for attempt in 0..2 {
            let (dir, g) = (self.dir.clone(), gid.to_owned());
            let record = tokio::task::spawn_blocking(move || dir.record(&g)).await
                .map_err(|e| PeerError::Transport(e.to_string()))?
                .map_err(|e| PeerError::Transport(format!("registro ilegível: {e}")))?;
            // Sem registro o grupo acabou aqui: o `Dissolved` que segue é quem avisa.
            let Some(record) = record else { return Ok(()) };
            let answer = self.peers.call_group(server, reqwest::Method::PUT, &path, Some(&push_body(&record, server, joining))).await?;
            let seen = answer.get("version").and_then(Value::as_u64).unwrap_or(0);
            if answer.get("ignored") != Some(&Value::Bool(true)) || seen <= record.version { return Ok(()); }
            if attempt == 0 { self.skip_to(gid, seen + 1).await?; }
        }
        Err(PeerError::Refused { status: 409, detail: json!({"code": VERSION_BEHIND}) })
    }

    /// Registro e sidecars daqui de `gid` na versão `to`, sob o lock de grupo.
    async fn skip_to(&self, gid: &str, to: u64) -> Result<(), PeerError> {
        let _guard = self.lock.lock().await;
        let (dir, gid) = (self.dir.clone(), gid.to_owned());
        let written = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let Some(mut record) = dir.record(&gid).map_err(std::io::Error::other)? else { return Ok(()) };
            if record.version >= to { return Ok(()) }
            record.version = to;
            dir.write_record(&record).map_err(std::io::Error::other)?;
            for m in record.members.iter().filter(|m| m.server == record.owner) {
                if let Some(mut s) = read(&dir, &m.name).filter(|s| s.gid == gid) {
                    s.fed = Some(Fed { owner: record.owner.clone(), local: true, version: to });
                    dir.write_sidecar(&m.name, &s).map_err(std::io::Error::other)?;
                }
            }
            Ok(())
        }).await.map_err(|e| PeerError::Transport(e.to_string()))?;
        written.map_err(|e| PeerError::Transport(format!("versão nova do grupo não gravada: {e}")))
    }
}

/// Corpo do `PUT /api/groups/{gid}` montado do registro na hora do envio: a versão que sai é
/// sempre a atual, e só entra em `joining` quem ainda é membro naquela máquina.
fn push_body(record: &GroupRecord, server: &str, joining: &[String]) -> Value {
    let joining: Vec<&String> = joining.iter().filter(|j| record.members.iter().any(|m| m.server == server && &m.name == *j)).collect();
    json!({"protocol": PROTOCOL, "owner": record.owner, "version": record.version, "task": record.task,
        "members": record.members, "outside": record.outside, "joining": joining})
}

impl OutboxSend for Sender {
    fn send<'a>(&'a self, item: &'a OutboxItem) -> BoxFuture<'a, Result<(), PeerError>> {
        Box::pin(async move {
            let (server, method, path, body) = match item {
                OutboxItem::Push { server, gid, joining, .. } => return self.push(server, gid, joining).await,
                OutboxItem::Dissolved { server, gid, version, origin } => (server, reqwest::Method::PUT, format!("/api/groups/{}", segment(gid)),
                    json!({"protocol": PROTOCOL, "owner": origin, "version": version, "dissolved": true})),
                OutboxItem::Leave { owner, gid, member, origin } => (owner, reqwest::Method::POST, format!("/api/groups/{}/ops", segment(gid)),
                    json!({"op": "leave", "protocol": PROTOCOL, "origin": origin, "member": member})),
                OutboxItem::Rename { owner, gid, from, to, origin } => (owner, reqwest::Method::POST, format!("/api/groups/{}/ops", segment(gid)),
                    json!({"op": "rename", "protocol": PROTOCOL, "origin": origin, "from": from, "to": to})),
                OutboxItem::Outside { owner, gid, op, origin } => {
                    let (link, outside) = match op { OutsideOp::Link(o) => (true, o), OutsideOp::Unlink(o) => (false, o) };
                    (owner, reqwest::Method::POST, format!("/api/groups/{}/ops", segment(gid)),
                        json!({"op": "outside", "protocol": PROTOCOL, "origin": origin, "link": link, "outside": outside}))
                }
            };
            self.peers.call_group(server, method, &path, Some(&body)).await.map(drop)
        })
    }

    fn failed(&self, item: &OutboxItem, code: &str, error: &PeerError, retry: bool) {
        let (server, gid, kind) = (item.destination(), item.gid(), item.kind());
        if retry {
            return tracing::warn!(code, %server, %gid, kind, error = %error.text(server), "groups: o item da fila não saiu e fica nela");
        }
        tracing::warn!(code = "grupos.recusado", %server, %gid, kind, refusal = %code, "groups: a outra máquina recusou o item da fila");
        // Sem quem espere (saída, renomeação, par de fora, dissolução), a recusa só aparece aqui.
        self.diag.report("rust.groups_outbox_refused", &format!("{kind}:{server}:{gid}"), code, "a outra máquina recusou o item da fila, que saiu dela sem efeito");
    }
}

// --- rotas entre máquinas --------------------------------------------------------------------

/// Pedido de outra máquina do dono a `/api/groups/{gid}[/…]`: só o token de dono (ou a ponte
/// privada). Nunca 404 sem código, que a outra máquina leria como versão antiga.
pub(super) struct Machine { pub(super) st: Arc<AppState>, pub(super) groups: Arc<GroupService>, pub(super) gid: String, parts: Parts, bytes: Bytes }

impl Machine {
    pub(super) async fn take(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Result<Machine, Response> {
        let bridged = req.extensions().get::<Bridged>().is_some();
        let (fwd, owner) = gate(&st, peer, &req);
        if !owner && !bridged {
            st.auth.record_fail(&fwd.client_ip);
            return Err(json_response(StatusCode::UNAUTHORIZED, detail_body("erro_nao_autorizado", "unauthorized", json!({}))));
        }
        let Some(groups) = st.groups.clone() else { return Err(reply(unavailable("groups_unavailable"))) };
        let gid = req.uri().path().strip_prefix("/api/groups/").and_then(|rest| rest.split('/').next())
            .map(|g| percent_encoding::percent_decode_str(g).decode_utf8_lossy().into_owned()).unwrap_or_default();
        let (parts, body) = req.into_parts();
        let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return Err(reply(no(StatusCode::PAYLOAD_TOO_LARGE, "erro_corpo_grande", "corpo grande demais", json!({})))) };
        Ok(Machine { st, groups, gid, parts, bytes })
    }

    pub(super) fn body<T: DeserializeOwned>(&self) -> Result<T, Response> {
        json_content_type(&self.parts.headers).then(|| serde_json::from_slice(&self.bytes).ok()).flatten().ok_or_else(|| self.invalid())
    }

    pub(super) fn invalid(&self) -> Response { crate::session_write::invalid_body(&self.parts.headers) }

    pub(super) fn method(&self) -> &axum::http::Method { &self.parts.method }

    pub(super) fn me(&self) -> String { self.groups.server_id().to_owned() }

    pub(super) fn answer(&self, result: Result<Value, Refusal>) -> Response {
        let (status, mut body) = match result { Ok(body) => (StatusCode::OK, body), Err(Refusal(status, body)) => (status, body) };
        body["server_id"] = json!(self.me());
        let mut response = json_response(status, body);
        cors(&self.parts.headers, response.headers_mut());
        response
    }
}

fn reply(Refusal(status, body): Refusal) -> Response { json_response(status, body) }

/// `GET /api/groups/{gid}`: o registro na dona. A participante pede isto na subida.
pub(super) async fn get_record(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let m = match Machine::take(st, peer, req).await { Ok(m) => m, Err(r) => return r };
    let (dir, gid, me) = (m.groups.dir(), m.gid.clone(), m.me());
    // Id trocado: "inexistente" faria a participante achar que o grupo acabou.
    let found = blocking(move || identity_ok(&dir, &me).map(|()| (dir.record(&gid), me))).await;
    let result = match found {
        Ok(Err(refused)) => Err(refused),
        Ok(Ok((Ok(Some(record)), me))) if record.owner == me => Ok(serde_json::to_value(&record).unwrap_or_default()),
        Ok(Ok((Ok(_), _))) => Err(missing(&m.gid)),
        Ok(Ok((Err(error), _))) => Err(store_refusal(error)),
        Err(refused) => Err(refused),
    };
    m.answer(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushBody {
    pub protocol: u32,
    pub owner: String,
    pub version: u64,
    #[serde(default)] pub task: String,
    #[serde(default)] pub members: Vec<Member>,
    #[serde(default)] pub outside: Vec<OutsideRef>,
    #[serde(default)] pub joining: Vec<String>,
    #[serde(default)] pub dissolved: bool,
    /// `srv::nome` do par 1:1 antigo que este envio converte (`convert.rs`).
    #[serde(default)] pub convert_from_legacy: Option<String>,
}

/// `PUT /api/groups/{gid}`: a dona empurra o grupo. Nunca entra na porta das sessões: só sidecar
/// sob o lock do grupo, senão um join iniciado fora da dona travaria no passe de quem iniciou.
pub(super) async fn put_record(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let m = match Machine::take(st, peer, req).await { Ok(m) => m, Err(r) => return r };
    let body = match m.body::<PushBody>() { Ok(b) if b.protocol == PROTOCOL => b, Ok(_) => return m.invalid(), Err(r) => return r };
    let result = local_apply(&m.st, &m.groups, &m.gid, body).await;
    m.answer(result)
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum OpsBody {
    Join { protocol: u32, #[serde(rename = "origin")] _origin: String, members: Vec<Member>, #[serde(default)] task: String, #[serde(default)] replace_task: bool,
        #[serde(default)] expected: BTreeMap<String, String> },
    Outside { protocol: u32, #[serde(rename = "origin")] _origin: String, link: bool, outside: OutsideRef },
    Leave { protocol: u32, origin: String, member: String },
    Rename { protocol: u32, origin: String, from: String, to: String },
    Message { protocol: u32, origin: String, sender: String, text: String, #[serde(default)] notice: String, #[serde(default)] from_outside: bool },
}

/// `POST /api/groups/{gid}/ops`: pedido de outra máquina à dona. `leave`/`rename`/`message` são
/// das Tasks seguintes; até lá, corpo desconhecido é 422.
pub(super) async fn ops(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let m = match Machine::take(st, peer, req).await { Ok(m) => m, Err(r) => return r };
    let body = match m.body::<OpsBody>() { Ok(b) => b, Err(r) => return r };
    let result = match body {
        OpsBody::Join { protocol, _origin: _, members, task, replace_task, expected } if protocol == PROTOCOL => {
            let ask = JoinAsk { gid: m.gid.clone(), members, task, replace_task, expected };
            owner_join(&m.st, &m.groups, ask, None).await.map(|done| {
                let mut body = serde_json::to_value(done).unwrap_or_default();
                body["protocol"] = json!(PROTOCOL);
                body
            })
        }
        OpsBody::Outside { protocol, _origin: _, link, outside } if protocol == PROTOCOL => {
            let op = if link { OutsideOp::Link(outside) } else { OutsideOp::Unlink(outside) };
            owner_outside(&m.st, &m.groups, &m.gid, op).await.map(|version| json!({"protocol": PROTOCOL, "version": version}))
        }
        OpsBody::Leave { protocol, origin, member } if protocol == PROTOCOL => super::fed_exit::leave_op(&m.st, &m.groups, &m.gid, &origin, &member).await,
        OpsBody::Rename { protocol, origin, from, to } if protocol == PROTOCOL => super::fed_exit::rename_op(&m.st, &m.groups, &m.gid, &origin, &from, &to).await,
        OpsBody::Message { protocol, origin, sender, text, notice, from_outside } if protocol == PROTOCOL && notice.len() <= 64 =>
            super::fed_notice::owner_message(&m.st, &m.groups, &m.gid, super::fed_notice::Notice::new(origin, sender, text, notice, from_outside)).await,
        _ => return m.invalid(),
    };
    m.answer(result)
}

// --- a dona ----------------------------------------------------------------------------------

pub struct JoinAsk {
    /// Grupo alvo; vazio = criar.
    pub gid: String,
    /// Quem entra, qualificado (inclui quem pediu).
    pub members: Vec<Member>,
    pub task: String,
    pub replace_task: bool,
    /// `srv::nome` → gid que quem pediu viu ("" = solto). Mudou → 409 `erro_grupo_mudou`.
    pub expected: BTreeMap<String, String>,
}

/// Resposta do `op: join` (com `protocol` e `server_id` a mais, que quem lê ignora).
#[derive(serde::Serialize, Deserialize)]
pub struct JoinDone { pub gid: String, pub version: u64, #[serde(default)] pub task: String, pub members: Vec<Member>,
    #[serde(default)] pub unconfirmed: Vec<Member>,
    /// Acesso de fora não gravado na dona (`grant_members`).
    #[serde(default)] pub access_failed: Vec<String>,
    /// Grupos absorvidos cujo contrato não foi herdado (`merge_contract`).
    #[serde(default)] pub contract_failed: Vec<String> }

/// O que a dona grava num join, e como desfazer.
struct OwnerPlan {
    record: GroupRecord,
    before_records: Vec<(String, Option<GroupRecord>)>,
    before: Snapshot,
    writes: Vec<(String, Sidecar)>,
    /// Membros novos por máquina participante.
    joiners: BTreeMap<String, Vec<String>>,
    /// Membros daqui que estavam soltos: recebem o protocolo.
    newcomers: Vec<String>,
    losers: Vec<GroupRecord>,
}

/// Grupo de `gid` visto daqui: o registro (dona) ou os sidecars locais com esse gid.
/// `records` = os registros válidos já lidos; o arquivo só é relido quando o gid não está entre eles
/// (registro torto continua recusando, em vez de valer como ausente).
fn base_record(dir: &PairDir, me: &str, gid: &str, outside: &HashSet<String>, all: &[(String, Sidecar)], records: &[GroupRecord]) -> Result<Option<GroupRecord>, Refusal> {
    if let Some(record) = records.iter().find(|r| r.gid == gid) { return Ok(Some(record.clone())); }
    if let Some(record) = dir.record(gid).map_err(store_refusal)? { return Ok(Some(record)); }
    let locals: Vec<&(String, Sidecar)> = all.iter().filter(|(_, s)| s.gid == gid).collect();
    // Grupo de outra dona que só tem membros aqui: pedido mandado à máquina errada, nunca vira registro dela.
    if locals.iter().any(|(_, s)| s.fed.as_ref().is_some_and(|f| f.owner != me)) { return Err(missing(gid)); }
    for (_, s) in &locals { legacy_refusal(s, outside)?; }
    let Some((_, first)) = locals.first() else { return Ok(None) };
    let mut record = GroupRecord { protocol: PROTOCOL, gid: gid.into(), owner: me.into(), version: 0,
        task: first.task.clone().unwrap_or_default(), members: Vec::new(), outside: Vec::new() };
    for (name, s) in &locals {
        record.members.push(Member::new(me, name, s.harness.get(name).map_or("", String::as_str)));
        for p in s.peers.iter().filter(|p| outside.contains(*p)) {
            let o = OutsideRef { server: me.into(), address: p.clone() };
            if !record.outside.contains(&o) { record.outside.push(o); }
        }
    }
    Ok(Some(record))
}

fn plan_owner(dir: &PairDir, me: &str, ask: &JoinAsk, outside: &HashSet<String>, harness: &BTreeMap<String, String>) -> Result<OwnerPlan, Refusal> {
    let all = dir.sidecars().map_err(store_refusal)?;
    let records = dir.records().map_err(store_refusal)?;
    let local = |name: &str| all.iter().find(|(n, _)| n == name).map(|(_, s)| s.clone());
    // Grupo em que cada candidato está, visto daqui.
    let current = |m: &Member| -> Result<String, Refusal> {
        if m.server == me {
            let Some(s) = local(&m.name) else { return Ok(String::new()) };
            if s.orq { return Err(orq_refusal()); }
            if s.fed.as_ref().is_some_and(|f| f.owner != me) { return Err(owners_refusal()); }
            legacy_refusal(&s, outside)?;
            return Ok(s.gid);
        }
        Ok(records.iter().find(|r| r.owner == me && r.members.iter().any(|x| x.same(m))).map(|r| r.gid.clone()).unwrap_or_default())
    };
    let mut gids: Vec<String> = Vec::new();
    if !ask.gid.is_empty() { gids.push(ask.gid.clone()); }
    for m in &ask.members {
        let gid = current(m)?;
        if let Some(seen) = ask.expected.get(&m.key()) && *seen != gid {
            return Err(no(StatusCode::CONFLICT, "erro_grupo_mudou", "o grupo mudou desde a consulta; tente de novo", json!({"membro": m.key()})));
        }
        if !gid.is_empty() && !gids.contains(&gid) { gids.push(gid); }
    }
    let mut bases = Vec::new();
    for gid in &gids {
        match base_record(dir, me, gid, outside, &all, &records)? {
            Some(r) if r.owner != me => return Err(owners_refusal()),
            Some(r) => bases.push(r),
            None => {}
        }
    }
    let survivor = bases.first().map_or_else(super::service::fresh_gid, |r| r.gid.clone());
    let mut members: Vec<Member> = Vec::new();
    for m in bases.iter().flat_map(|r| r.members.iter()).chain(ask.members.iter()) {
        let mut m = m.clone();
        if m.server == me && let Some(p) = harness.get(&m.name) { m.provider.clone_from(p); }
        match members.iter_mut().find(|x| x.same(&m)) {
            Some(x) => if x.provider.is_empty() { x.provider = m.provider },
            None => members.push(m),
        }
    }
    let existing = bases.iter().map(|r| r.task.as_str()).find(|t| !t.is_empty()).unwrap_or("");
    let asked = ask.task.trim();
    if !asked.is_empty() && !existing.is_empty() && asked != existing && !ask.replace_task {
        return Err(no(StatusCode::CONFLICT, "erro_pareamento_tarefa_existente",
            &format!("o grupo já tem tarefa: {} — repita com --substituir-tarefa pra trocar", crate::transcript::py::py_repr(&json!(existing))),
            json!({"existente": existing})));
    }
    let task = if !asked.is_empty() && (existing.is_empty() || ask.replace_task) { asked } else { existing }.to_owned();
    let version = bases.iter().map(|r| r.version).max().unwrap_or(0) + 1;
    let mut outside_refs: Vec<OutsideRef> = Vec::new();
    for o in bases.iter().flat_map(|r| r.outside.iter()) { if !outside_refs.contains(o) { outside_refs.push(o.clone()); } }
    let record = GroupRecord { protocol: PROTOCOL, gid: survivor.clone(), owner: me.into(), version, task: task.clone(), members: members.clone(), outside: outside_refs };

    let survivor_before = bases.first().filter(|r| r.gid == survivor);
    let mut joiners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in members.iter().filter(|m| m.server != me) {
        if !survivor_before.is_some_and(|r| r.members.iter().any(|x| x.same(m))) {
            joiners.entry(m.server.clone()).or_default().push(m.name.clone());
        }
    }
    // Pares de fora e harness que os sidecars daqui já tinham continuam valendo.
    let involved: Vec<Sidecar> = all.iter().filter(|(_, s)| gids.contains(&s.gid)).map(|(_, s)| s.clone()).collect();
    let mut kept_outside: Vec<String> = Vec::new();
    let mut harness_all: BTreeMap<String, String> = BTreeMap::new();
    for s in &involved {
        for p in s.peers.iter().filter(|p| outside.contains(*p)) { if !kept_outside.contains(p) { kept_outside.push(p.clone()); } }
        harness_all.extend(s.harness.clone());
    }
    harness_all.extend(harness.clone());
    for m in members.iter().filter(|m| m.server != me && !m.provider.is_empty()) { harness_all.insert(m.view(me), m.provider.clone()); }
    let mut before = Snapshot::new();
    let mut writes = Vec::new();
    let mut newcomers = Vec::new();
    for m in members.iter().filter(|m| m.server == me) {
        let old = local(&m.name);
        if old.is_none() { newcomers.push(m.name.clone()); }
        let mut peers: Vec<String> = members.iter().filter(|x| !x.same(m)).map(|x| x.view(me)).collect();
        peers.extend(kept_outside.iter().cloned());
        let linked_by = old.as_ref().and_then(|s| s.linked_by.clone());
        let lead = old.as_ref().and_then(|s| s.lead.clone());
        writes.push((m.name.clone(), Sidecar { peers, task: Some(task.clone()), gid: survivor.clone(), harness: harness_all.clone(), orq: false,
            fed: Some(Fed { owner: me.into(), local: true, version }), linked_by, lead }));
        before.insert(m.name.clone(), old);
    }
    let before_records = gids.iter().map(|g| (g.clone(), records.iter().find(|r| &r.gid == g).cloned())).collect();
    let losers = bases.into_iter().skip(1).collect();
    Ok(OwnerPlan { record, before_records, before, writes, joiners, newcomers, losers })
}

fn orq_refusal() -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_orq_entre_maquinas", "grupo de orquestração não se estende a outra máquina", json!({}))
}

/// Par 1:1 antigo com outra máquina do dono que não converteu (ela está fora ou em versão antiga):
/// o grupo novo o deixaria para trás sem aviso.
fn legacy_refusal(s: &Sidecar, outside: &HashSet<String>) -> Result<(), Refusal> {
    let Some(peer) = s.peers.iter().find(|p| s.fed.is_none() && is_remote(p) && !outside.contains(*p)) else { return Ok(()) };
    let server = peer.split_once("::").map_or(peer.as_str(), |(srv, _)| srv);
    Err(no(StatusCode::CONFLICT, "erro_grupo_par_antigo",
        &format!("há um par antigo com {server}: desfaça e refaça o par com {server} ou atualize o Hangar dela"), json!({"servidor": server})))
}

fn owners_refusal() -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_fusao_entre_donos", "os grupos têm máquinas donas diferentes e não se juntam", json!({}))
}

/// Só depois de `identity_ok` passar: id gravado diferente aqui é troca sem grupo vivo, e vale o atual.
pub(super) fn remember(dir: &PairDir, me: &str) {
    if dir.self_id().ok().flatten().as_deref() != Some(me)
        && let Err(error) = dir.remember_self(me) {
        tracing::warn!(code = "groups_identity_not_saved", %error, "groups: o id desta máquina não foi gravado");
    }
}

/// Grava registro, sidecars daqui e os itens da fila no mesmo passo; falha desfaz tudo.
fn apply_owner(dir: &PairDir, outbox: &Outbox, me: &str, plan: &OwnerPlan) -> Result<(), Refusal> {
    let mut items = fan_out(remote_servers(&plan.record, me), &plan.record.gid, plan.record.version, me, Fan::Push);
    for item in &mut items {
        if let OutboxItem::Push { server, joining, .. } = item { *joining = plan.joiners.get(server).cloned().unwrap_or_default(); }
    }
    for loser in &plan.losers { items.extend(fan_out(remote_servers(loser, me), &loser.gid, loser.version + 1, me, Fan::Dissolved)); }
    let written = (|| -> Result<(), super::store::StoreError> {
        dir.write_record(&plan.record)?;
        // O contrato do absorvido só se junta depois da confirmação: desfazer não separa texto.
        for loser in &plan.losers { dir.remove_record(&loser.gid)?; }
        super::service::apply(dir, &[], &plan.writes)
    })();
    let queued = written.map_err(store_refusal).and_then(|()| outbox.enqueue(items).map_err(|e| {
        if e.to_string() == QUEUE_FULL {
            no(StatusCode::SERVICE_UNAVAILABLE, QUEUE_FULL, "a fila para outra máquina está cheia", json!({}))
        } else { store_refusal(e) }
    }));
    if let Err(refused) = queued {
        restore_logged(dir, &plan.before);
        for (gid, record) in &plan.before_records {
            if let Err(error) = match record { Some(r) => dir.write_record(r), None => dir.remove_record(gid) } {
                tracing::error!(code = "groups_record_restore_failed", gid = %gid, %error, "groups: o registro do grupo não voltou ao de antes");
            }
        }
        return Err(refused);
    }
    remember(dir, me);
    Ok(())
}

/// Entrada no grupo, na dona. Quem não confirma no prazo sai do registro; ninguém confirmado e
/// alguém de outra máquina pedido = 502 e o grupo volta ao que era. `known` = provedores dos
/// membros daqui que quem chamou já conferiu na lista (o `/pair` desta máquina); `None` = conferir.
pub async fn owner_join(st: &Arc<AppState>, groups: &Arc<GroupService>, mut ask: JoinAsk, known: Option<BTreeMap<String, String>>) -> Result<JoinDone, Refusal> {
    let me = groups.server_id().to_owned();
    need_server_id(&me)?;
    let fed = of(st, groups).await?;
    let registered = st.peers.addressable().await;
    if let Some(server) = ask.members.iter().map(|m| m.server.as_str()).find(|s| *s != me && !registered.contains(*s)) {
        return Err(unregistered(server));
    }
    let locals: Vec<String> = ask.members.iter().filter(|m| m.server == me).map(|m| m.name.clone()).collect();
    let harness = match known {
        Some(harness) => harness,
        None => providers(st, &locals).await.map_err(|failed| Refusal(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})))?,
    };
    let absent = not_found(&locals, &harness);
    if !absent.is_empty() { return Err(sessions_missing(&absent)); }
    if !locals.is_empty() {
        match super::orq::is_orchestrator(st, &locals).await {
            Ok(found) if found.is_empty() => {}
            Ok(_) => return Err(no(StatusCode::CONFLICT, "erro_sessao_orq", "o orquestrador não recebe mensagens; fale com o árbitro", json!({}))),
            Err(failed) => return Err(Refusal(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed}))),
        }
    }
    for m in ask.members.iter_mut().filter(|m| m.server == me) { if let Some(p) = harness.get(&m.name) { m.provider.clone_from(p); } }
    let outside = outside_addresses(st);
    let dir = groups.dir();
    let plan = {
        let (dir, outbox, me) = (dir.clone(), fed.outbox.clone(), me.clone());
        groups.locked(blocking(move || -> Result<OwnerPlan, Refusal> {
            identity_ok(&dir, &me)?;
            let plan = plan_owner(&dir, &me, &ask, &outside, &harness)?;
            apply_owner(&dir, &outbox, &me, &plan)?;
            Ok(plan)
        })).await??
    };
    groups.changed();
    // Fora do lock: cada máquina com membro novo confirma (ou não) em paralelo.
    let (gid, version, wait) = (plan.record.gid.clone(), plan.record.version, timing(groups.pair_root()).join_wait);
    let waits = plan.joiners.iter().map(|(server, names)| {
        let (fed, gid, server, names) = (fed.clone(), gid.clone(), server.clone(), names.clone());
        async move { (fed.outbox.wait(&server, &gid, version, wait).await, server, names) }
    });
    let results = futures_util::future::join_all(waits).await;
    let mut unconfirmed: Vec<Member> = Vec::new();
    let mut confirmed_any = false;
    for (delivery, server, names) in results {
        match delivery {
            Delivery::Confirmed | Delivery::Dissolved => confirmed_any = true,
            Delivery::Refused { code } => {
                tracing::warn!(code = "erro_grupo_membro_nao_confirmado", server = %server, refusal = %code, "groups: a máquina recusou os membros novos");
                unconfirmed.extend(names.iter().map(|n| Member::new(&server, n, "")));
            }
            Delivery::Pending => {
                tracing::warn!(code = "erro_grupo_membro_nao_confirmado", server = %server, "groups: a máquina não confirmou no prazo");
                unconfirmed.extend(names.iter().map(|n| Member::new(&server, n, "")));
            }
        }
    }
    if !plan.joiners.is_empty() && !confirmed_any {
        undo_owner(groups, &fed, &plan).await?;
        let names: Vec<String> = unconfirmed.iter().map(Member::key).collect();
        return Err(no(StatusCode::BAD_GATEWAY, "erro_grupo_membro_nao_confirmado",
            &format!("nenhuma outra máquina confirmou ({}); o grupo voltou ao que era", names.join(", ")), json!({"membros": names})));
    }
    let mut contract_failed = Vec::new();
    if !plan.losers.is_empty() {
        let (dir, losers, survivor) = (dir.clone(), plan.losers.iter().map(|r| r.gid.clone()).collect::<Vec<_>>(), gid.clone());
        contract_failed = groups.locked(blocking(move || losers.into_iter().filter(|loser| !dir.merge_contract(loser, &survivor)).collect::<Vec<_>>())).await?;
        // Os registros absorvidos já saíram: o contrato que ficou no disco só aparece aqui e na resposta.
        for loser in &contract_failed {
            st.diag.report("rust.groups_contract_merge_failed", loser, "groups_contract_merge_failed", "a fusão não herdou o contrato do grupo absorvido");
        }
    }
    let mut record = plan.record.clone();
    if !unconfirmed.is_empty() {
        // Todos de uma vez; o aviso de saída a quem ficou sozinho sai depois da resposta.
        if let Some((r, dropped)) = super::fed_exit::remove_and_dissolve(st, groups, &gid, &unconfirmed).await? {
            record = r;
            let st = st.clone();
            tokio::spawn(async move { super::fed_exit::tell_dropped(&st, &dropped).await });
        }
    }
    // Acesso de fora depois de tudo gravado, só a quem ficou; é disco, e a falha volta a quem pediu.
    let access_failed = grant_locals(st, groups, &me, &record).await;
    // O protocolo sai depois da resposta: uma sessão lenta faria quem pediu desistir do grupo já
    // gravado. Falha vai ao diário daqui.
    let alive: Vec<String> = plan.newcomers.iter().filter(|n| record.members.iter().any(|m| m.server == me && &&m.name == n)).cloned().collect();
    if !alive.is_empty() {
        let (st, groups, me, told) = (st.clone(), groups.clone(), me.clone(), record.clone());
        tokio::spawn(async move { tell_newcomers(&st, &groups, &me, &told, &alive).await });
    }
    Ok(JoinDone { gid, version: record.version, task: record.task.clone(), members: record.members.clone(), unconfirmed, access_failed, contract_failed })
}

/// Volta o grupo ao de antes do join: sidecars daqui, registros e uma versão nova às máquinas que
/// podem ter aplicado o join no meio do caminho.
async fn undo_owner(groups: &GroupService, fed: &Federation, plan: &OwnerPlan) -> Result<(), Refusal> {
    let me = groups.server_id().to_owned();
    let (dir, outbox) = (groups.dir(), fed.outbox.clone());
    let (record, before, before_records, joiners) = (plan.record.clone(), plan.before.clone(), plan.before_records.clone(), plan.joiners.clone());
    groups.locked(blocking(move || -> Result<(), Refusal> {
        for (server, names) in &joiners {
            for n in names { outbox.cancel_joining(server, &record.gid, n).map_err(store_refusal)?; }
        }
        restore_logged(&dir, &before);
        let mut items = Vec::new();
        // A fila pode ter pulado a versão do registro durante a espera (`Sender::skip_to`).
        let current = dir.record(&record.gid).map_err(store_refusal)?.map_or(0, |r| r.version);
        let bump = record.version.max(current) + 1;
        for (gid, old) in &before_records {
            match old {
                Some(r) => {
                    let back = GroupRecord { version: bump, ..r.clone() };
                    dir.write_record(&back).map_err(store_refusal)?;
                    let mut servers = remote_servers(&back, &me);
                    if *gid == record.gid { servers.extend(remote_servers(&record, &me)); }
                    items.extend(fan_out(servers, gid, bump, &me, Fan::Push));
                }
                None => {
                    dir.remove_record(gid).map_err(store_refusal)?;
                    if *gid == record.gid { items.extend(fan_out(remote_servers(&record, &me), gid, bump, &me, Fan::Dissolved)); }
                }
            }
        }
        if !before_records.iter().any(|(g, _)| *g == record.gid) {
            dir.remove_record(&record.gid).map_err(store_refusal)?;
            items.extend(fan_out(remote_servers(&record, &me), &record.gid, bump, &me, Fan::Dissolved));
        }
        outbox.enqueue(items).map_err(store_refusal)
    })).await??;
    groups.changed();
    Ok(())
}

/// Tira `whos` do grupo `gid` desta dona num passo só: registro com uma versão nova, cada um fora
/// dos `joining` pendentes, sidecars daqui e o grupo empurrado de novo (inclusive à máquina de cada
/// um, que pode ter aplicado o join atrasado). Devolve o registro novo e quem daqui ficou sem
/// ninguém (sidecar apagado) e se mudou algo; sem registro, `None`. Quem já não está no registro
/// (saída repetida) não muda nada: volta o registro como está, com `false`.
pub async fn remove_members(st: &AppState, groups: &GroupService, gid: &str, whos: &[Member]) -> Result<Option<(GroupRecord, Vec<String>, bool)>, Refusal> {
    let me = groups.server_id().to_owned();
    let fed = of(st, groups).await?;
    let (dir, outbox, gid) = (groups.dir(), fed.outbox.clone(), gid.to_owned());
    let whos: Vec<Member> = whos.to_vec();
    let done = groups.locked(blocking(move || -> Result<Option<(GroupRecord, Vec<String>, bool)>, Refusal> {
        let Some(mut record) = dir.record(&gid).map_err(store_refusal)?.filter(|r| r.owner == me) else { return Ok(None) };
        let whos: Vec<&Member> = whos.iter().filter(|w| record.members.iter().any(|m| m.same(w))).collect();
        if whos.is_empty() { return Ok(Some((record, Vec::new(), false))); }
        record.members.retain(|m| !whos.iter().any(|w| m.same(w)));
        record.version += 1;
        dir.write_record(&record).map_err(store_refusal)?;
        for who in &whos { outbox.cancel_joining(&who.server, &gid, &who.name).map_err(store_refusal)?; }
        let gone: Vec<String> = whos.iter().map(|w| w.view(&me)).collect();
        let mut alone = Vec::new();
        for m in record.members.iter().filter(|m| m.server == me) {
            if let Some(mut s) = read(&dir, &m.name).filter(|s| s.gid == gid) {
                s.peers.retain(|p| !gone.contains(p));
                // Sem peers o sidecar já não vale como grupo: a dissolução não o veria para apagar e avisar.
                if s.peers.is_empty() && !s.orq {
                    dir.clear_sidecar(&m.name).map_err(store_refusal)?;
                    alone.push(m.name.clone());
                    continue;
                }
                s.fed = Some(Fed { owner: me.clone(), local: true, version: record.version });
                dir.write_sidecar(&m.name, &s).map_err(store_refusal)?;
            }
        }
        let mut servers = remote_servers(&record, &me);
        for who in &whos {
            if who.server == me { dir.clear_sidecar(&who.name).map_err(store_refusal)?; } else { servers.insert(who.server.clone()); }
        }
        outbox.enqueue(fan_out(servers, &gid, record.version, &me, Fan::Push)).map_err(store_refusal)?;
        Ok(Some((record, alone, true)))
    })).await??;
    if done.as_ref().is_some_and(|(_, _, changed)| *changed) { groups.changed(); }
    Ok(done)
}

/// Par de fora ligado ou desfeito numa máquina do grupo, aplicado na dona (só informativo: o par
/// continua só na máquina dele).
pub async fn owner_outside(st: &AppState, groups: &GroupService, gid: &str, op: OutsideOp) -> Result<u64, Refusal> {
    let me = groups.server_id().to_owned();
    let fed = of(st, groups).await?;
    let (dir, outbox, gid) = (groups.dir(), fed.outbox.clone(), gid.to_owned());
    let version = groups.locked(blocking(move || -> Result<(u64, bool), Refusal> {
        let Some(mut record) = dir.record(&gid).map_err(store_refusal)?.filter(|r| r.owner == me) else { return Err(missing(&gid)) };
        let changed = match &op {
            OutsideOp::Link(o) if !record.outside.contains(o) => { record.outside.push(o.clone()); true }
            OutsideOp::Unlink(o) if record.outside.contains(o) => { record.outside.retain(|x| x != o); true }
            _ => false,
        };
        if !changed { return Ok((record.version, false)); }
        record.version += 1;
        dir.write_record(&record).map_err(store_refusal)?;
        for m in record.members.iter().filter(|m| m.server == me) {
            if let Some(mut s) = read(&dir, &m.name).filter(|s| s.gid == gid) {
                s.fed = Some(Fed { owner: me.clone(), local: true, version: record.version });
                dir.write_sidecar(&m.name, &s).map_err(store_refusal)?;
            }
        }
        outbox.enqueue(fan_out(remote_servers(&record, &me), &gid, record.version, &me, Fan::Push)).map_err(store_refusal)?;
        Ok((record.version, true))
    })).await??;
    if version.1 { groups.changed(); }
    Ok(version.0)
}

/// Depois de um par de fora entrar (`link`) ou sair do grupo de `sidecar` nesta máquina: em grupo
/// entre máquinas, a dona fica sabendo (direto, se é esta; senão pela fila). Falha só ao diário:
/// o par já vale ou já acabou aqui.
pub async fn outside_changed(st: &AppState, sidecar: &Sidecar, address: &str, link: bool) {
    let (Some(groups), Some(f)) = (st.groups.clone(), sidecar.fed.clone()) else { return };
    let me = groups.server_id().to_owned();
    let o = OutsideRef { server: me.clone(), address: address.to_owned() };
    let op = if link { OutsideOp::Link(o) } else { OutsideOp::Unlink(o) };
    let done = if f.owner == me {
        owner_outside(st, &groups, &sidecar.gid, op).await.map(drop)
    } else {
        match of(st, &groups).await {
            Ok(fed) => {
                let item = OutboxItem::Outside { owner: f.owner.clone(), gid: sidecar.gid.clone(), op, origin: me };
                blocking(move || fed.outbox.enqueue(vec![item])).await.and_then(|r| r.map_err(store_refusal))
            }
            Err(refused) => Err(refused),
        }
    };
    if let Err(Refusal(status, body)) = done {
        tracing::warn!(code = "groups_outside_not_told", %status, detail = %body["detail"]["code"], "groups: a dona não ficou sabendo do par de fora");
    }
}

/// O protocolo a quem entrou solto nesta máquina.
async fn tell_newcomers(st: &AppState, groups: &GroupService, me: &str, record: &GroupRecord, names: &[String]) {
    let contract = if record.owner == me { ContractLine { path: Some(groups.contract_path(&record.gid)), remote_owner: None } }
        else { ContractLine { path: None, remote_owner: Some(record.owner.clone()) } };
    for n in names {
        let Some(mine) = record.members.iter().find(|m| m.server == me && &m.name == n) else { continue };
        let args = TextArgs { me: n.clone(), others: record.members.iter().filter(|m| !m.same(mine)).map(|m| m.view(me)).collect(),
            task: record.task.clone(), harness: BTreeMap::from([(n.clone(), if mine.provider.is_empty() { "claude".into() } else { mine.provider.clone() })]),
            contract: contract.clone(), across: true, ..Default::default() };
        let sent = match protocol_text(Kind::Group, &args) { Ok(text) => deliver_text(st, n, &text).await, Err(e) => Err(e) };
        if let Err(error) = sent {
            tracing::warn!(code = "groups_protocol_not_delivered", session = %n, error = %error["code"], "groups: o protocolo não chegou ao membro novo");
        }
    }
}

/// Membro daqui que entrou num grupo com par de fora daqui ganha "ver e recado" (só nesta máquina).
/// Devolve as falhas de disco, que quem pediu vê como aviso com código.
async fn grant_locals(st: &AppState, groups: &GroupService, me: &str, record: &GroupRecord) -> Vec<String> {
    let Some(share) = &st.share else { return Vec::new() };
    let mut members: Vec<String> = record.members.iter().filter(|m| m.server == me).map(|m| m.name.clone()).collect();
    if let Some(first) = members.first().cloned()
        && let Ok(Some(s)) = groups.link(&first).await {
        members.extend(s.peers.into_iter().filter(|p| is_remote(p)));
    }
    crate::share::pair_routes::grant_members(share, &members).await
}

// --- a participante --------------------------------------------------------------------------

enum Applied { Ignored(u64), Done { version: u64, names: Vec<String>, newcomers: Vec<String>, dropped: Vec<String> } }

/// `PUT` da dona aplicado aqui: só sidecar de quem está em `joining` nasce; só sidecar da mesma
/// (dona, gid) muda ou sai; renomear e sair pendentes nesta máquina vencem o envio.
pub async fn local_apply(st: &Arc<AppState>, groups: &Arc<GroupService>, gid: &str, body: PushBody) -> Result<Value, Refusal> {
    let me = groups.server_id().to_owned();
    need_server_id(&me)?;
    // Sem a dona no peers.json daqui, saída, renomear e aviso dela não teriam para onde ir.
    if st.peers.base_url(&body.owner).is_none() { return Err(unregistered(&body.owner)); }
    if body.owner == me { return Err(no(StatusCode::CONFLICT, "erro_grupo_identificador_igual", "a dona do grupo tem o mesmo id desta máquina", json!({}))); }
    if let Some(from) = body.convert_from_legacy.clone() { return super::convert::accept(groups, gid, body, from).await; }
    let fed = of(st, groups).await?;
    // Leitura antecipada só para conferir as sessões na lista; quem vale é a da fila sob o lock.
    let (renamed, leaving) = fed.outbox.moves_for(&body.owner, gid);
    let joining = moved(&body.joining, &renamed, &leaving);
    let harness = if joining.is_empty() || body.dissolved { BTreeMap::new() } else {
        providers(st, &joining).await.map_err(|failed| Refusal(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})))?
    };
    if !body.dissolved && let Some(absent) = joining.iter().find(|n| !harness.contains_key(*n)) { return Err(sessions_missing(absent)); }
    let outside = outside_addresses(st);
    let dir = groups.dir();
    let applied = {
        let (dir, me, gid, outbox, outside) = (dir.clone(), me.clone(), gid.to_owned(), fed.outbox.clone(), outside.clone());
        groups.locked(blocking(move || {
            // Renomear e sair que entraram na fila até agora vencem o envio.
            let (renamed, leaving) = outbox.moves_for(&body.owner, &gid);
            let joining = moved(&body.joining, &renamed, &leaving);
            apply_push(&dir, &me, &gid, &body, &renamed, &leaving, &joining, &harness, &outside)
        })).await??
    };
    let (version, names, newcomers, dropped) = match applied {
        Applied::Done { version, names, newcomers, dropped } => (version, names, newcomers, dropped),
        Applied::Ignored(version) => return Ok(json!({"protocol": PROTOCOL, "version": version, "members": [], "ignored": true})),
    };
    groups.changed();
    // Avisos depois da resposta: a dona espera a confirmação por pouco tempo, e uma sessão ocupada
    // a faria tirar do grupo quem já entrou. Falha vai ao diário daqui.
    let (st, groups, gid, reply) = (st.clone(), groups.clone(), gid.to_owned(), json!({"protocol": PROTOCOL, "version": version, "members": names}));
    tokio::spawn(async move {
        if !newcomers.is_empty() && let Some(record) = record_view(&dir, &me, &gid, &names, &outside) {
            tell_newcomers(&st, &groups, &me, &record, &newcomers).await;
            for failed in grant_locals(&st, &groups, &me, &record).await {
                tracing::warn!(code = super::routes::ACCESS_FAILED, error = %failed, "groups: acesso de fora ao membro novo não gravado");
            }
        }
        super::fed_exit::tell_dropped(&st, &dropped).await;
    });
    Ok(reply)
}

fn moved(names: &[String], renamed: &HashMap<String, String>, leaving: &HashSet<String>) -> Vec<String> {
    names.iter().map(|n| renamed.get(n).cloned().unwrap_or_else(|| n.clone())).filter(|n| !leaving.contains(n)).collect()
}

/// O grupo como os sidecars daqui o veem, para o texto do protocolo.
fn record_view(dir: &PairDir, me: &str, gid: &str, names: &[String], outside: &HashSet<String>) -> Option<GroupRecord> {
    let first = names.first()?;
    let s = read(dir, first)?;
    let f = s.fed.clone()?;
    let members = members_from_sidecar(me, first, &s, outside);
    Some(GroupRecord { protocol: PROTOCOL, gid: gid.into(), owner: f.owner, version: f.version, task: s.task.unwrap_or_default(), members, outside: Vec::new() })
}

/// O sidecar de `name` entre os já lidos (`sidecars()` devolve o nome do arquivo, não o da sessão).
fn loaded<'a>(all: &'a [(String, Sidecar)], name: &str) -> Option<&'a Sidecar> {
    let stem = super::store::file_stem(name);
    all.iter().find(|(n, _)| *n == stem).map(|(_, s)| s)
}

/// Maior versão aplicada por `dona::gid`, também depois de o último sidecar sair: um envio atrasado
/// de antes da dissolução não recria ninguém, nem depois de reiniciar. Fora de `.json` para o
/// `records()` não o ler como registro.
fn applied_path(dir: &PairDir) -> PathBuf { dir.groups_dir().join("applied.versions") }

fn read_applied(dir: &PairDir) -> BTreeMap<String, u64> {
    match std::fs::read(applied_path(dir)) {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|error| {
            tracing::warn!(code = "groups_applied_unreadable", %error, "groups: versões aplicadas ilegíveis, valem as dos sidecars");
            BTreeMap::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => {
            tracing::warn!(code = "groups_applied_unreadable", %error, "groups: versões aplicadas ilegíveis, valem as dos sidecars");
            BTreeMap::new()
        }
    }
}

fn write_applied(dir: &PairDir, applied: &BTreeMap<String, u64>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.groups_dir())?;
    super::store::replace_atomic(&applied_path(dir), &serde_json::to_vec(applied).map_err(std::io::Error::other)?)
}

#[allow(clippy::too_many_arguments)]
fn apply_push(dir: &PairDir, me: &str, gid: &str, body: &PushBody, renamed: &HashMap<String, String>, leaving: &HashSet<String>,
    joining: &[String], harness: &BTreeMap<String, String>, outside: &HashSet<String>) -> Result<Applied, Refusal> {
    identity_ok(dir, me)?;
    let all = dir.sidecars().map_err(store_refusal)?;
    let ours: Vec<(String, Sidecar)> = all.iter().filter(|(_, s)| s.gid == gid && s.fed.as_ref().is_some_and(|f| f.owner == body.owner)).cloned().collect();
    let seen_key = format!("{}::{gid}", body.owner);
    let mut applied = read_applied(dir);
    // A menor versão entre os sidecars do grupo: queda no meio da gravação deixa versões misturadas, e
    // o reenvio da mesma versão conserta. Sem sidecar, vale a última aplicada.
    let stored = ours.iter().filter_map(|(_, s)| s.fed.as_ref().map(|f| f.version)).min()
        .unwrap_or_else(|| applied.get(&seen_key).copied().unwrap_or(0));
    // Dois joins à mesma máquina saem com a versão atual do registro: a mesma versão com alguém do
    // `joining` ainda sem sidecar nela é aplicada de novo (reaplicar não muda o que já está certo).
    let joins_left = joining.iter().any(|j| !ours.iter().any(|(n, s)| n == j && s.fed.as_ref().is_some_and(|f| f.version == body.version)));
    if body.version < stored || (body.version == stored && stored > 0 && !joins_left) { return Ok(Applied::Ignored(stored)); }
    let mut writes: Vec<(String, Sidecar)> = Vec::new();
    let mut clears: Vec<String> = Vec::new();
    let mut names = Vec::new();
    let mut newcomers = Vec::new();
    let mut dropped = Vec::new();
    if body.dissolved {
        // Fica um grupo daqui comum: sem `fed`, sem os membros de outras máquinas.
        for (name, mut s) in ours {
            s.fed = None;
            s.peers.retain(|p| !is_remote(p) || outside.contains(p));
            if s.peers.is_empty() { clears.push(name.clone()); dropped.push(name); } else { writes.push((name, s)); }
        }
    } else {
        let rename = |n: &str| renamed.get(n).cloned().unwrap_or_else(|| n.to_owned());
        let members: Vec<Member> = body.members.iter().map(|m| if m.server == me { Member { name: rename(&m.name), ..m.clone() } } else { m.clone() })
            .filter(|m| !(m.server == me && leaving.contains(&m.name))).collect();
        for m in members.iter().filter(|m| m.server == me) {
            let existing = ours.iter().find(|(n, _)| *n == m.name).map(|(_, s)| s.clone());
            let old = match existing {
                Some(s) => Some(s),
                None if joining.contains(&m.name) => {
                    match loaded(&all, &m.name) {
                        None => None,
                        // Outro grupo da mesma dona é a fusão dela; qualquer outro grupo, recusa.
                        Some(s) if s.fed.as_ref().is_some_and(|f| f.owner == body.owner) && !s.orq => Some(s.clone()),
                        Some(_) => return Err(no(StatusCode::CONFLICT, "erro_grupo_mudou", &format!("'{}' já está em outro grupo", m.name), json!({"membro": m.name}))),
                    }
                }
                // Nunca nasce sidecar fora do `joining`: quem saiu não volta por um envio atrasado.
                None => continue,
            };
            let was_loose = old.is_none();
            let old = old.unwrap_or_default();
            let mut peers: Vec<String> = members.iter().filter(|x| !x.same(m)).map(|x| x.view(me)).collect();
            peers.extend(old.peers.iter().filter(|p| outside.contains(*p)).cloned());
            let mut h = old.harness.clone();
            h.extend(harness.iter().filter(|(k, _)| **k == m.name).map(|(k, v)| (k.clone(), v.clone())));
            for x in members.iter().filter(|x| !x.provider.is_empty()) { h.entry(x.view(me)).or_insert_with(|| x.provider.clone()); }
            writes.push((m.name.clone(), Sidecar { peers, task: Some(body.task.clone()), gid: gid.into(), harness: h, orq: false,
                fed: Some(Fed { owner: body.owner.clone(), local: false, version: body.version }), linked_by: old.linked_by, lead: old.lead }));
            if was_loose { newcomers.push(m.name.clone()); }
            names.push(m.name.clone());
        }
        for (name, _) in &ours {
            if !names.contains(name) { clears.push(name.clone()); dropped.push(name.clone()); }
        }
    }
    let before: Snapshot = clears.iter().chain(writes.iter().map(|(n, _)| n)).map(|n| (n.clone(), loaded(&all, n).cloned())).collect();
    applied.insert(seen_key, body.version);
    // A versão só vale gravada junto dos sidecars: sem ela, a dona reenvia e a mesma versão aplica de novo.
    let written = super::service::apply(dir, &clears, &writes).map_err(store_refusal).and_then(|()| write_applied(dir, &applied).map_err(store_refusal));
    if let Err(refused) = written {
        restore_logged(dir, &before);
        return Err(refused);
    }
    if !body.dissolved { remember(dir, me); }
    Ok(Applied::Done { version: body.version, names, newcomers, dropped })
}

// --- quem pede -------------------------------------------------------------------------------

/// `GET /api/sessions/{name}/group`: o grupo da sessão como esta máquina o vê.
pub(super) async fn session_group(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let me = asked.groups.server_id().to_owned();
    let link = match asked.groups.link(&asked.name).await { Ok(l) => l, Err(error) => return asked.store_failed(&error) };
    let Some(s) = link.filter(|s| !s.peers.is_empty() || s.orq) else {
        return asked.reply(StatusCode::NOT_FOUND, json!({"detail": {"code": "erro_sessao_sem_grupo", "params": {}, "msg": "sessão não está num grupo"}, "server_id": me}));
    };
    let outside = outside_addresses(&asked.st);
    let record = match &s.fed {
        Some(f) if f.owner == me => {
            let (dir, gid) = (asked.groups.dir(), s.gid.clone());
            match blocking(move || dir.record(&gid)).await {
                Ok(Ok(record)) => record,
                Ok(Err(error)) => return asked.store_failed(&super::service::GroupError::Store(error)),
                Err(Refusal(status, body)) => return asked.reply(status, body),
            }
        }
        _ => None,
    };
    let (owner, version, members, outside_refs) = match record {
        Some(r) => (r.owner, r.version, r.members, r.outside),
        None => {
            let members = members_from_sidecar(&me, &asked.name, &s, &outside);
            let outside_refs = s.peers.iter().filter(|p| outside.contains(*p)).map(|p| OutsideRef { server: me.clone(), address: p.clone() }).collect();
            let (owner, version) = s.fed.as_ref().map_or((me.clone(), 0), |f| (f.owner.clone(), f.version));
            (owner, version, members, outside_refs)
        }
    };
    let sync = existing(asked.groups.pair_root()).is_none_or(|fed| !fed.outbox.touches(&s.gid));
    asked.reply(StatusCode::OK, json!({"server_id": me, "gid": s.gid, "owner": owner, "version": version, "task": s.task.unwrap_or_default(),
        "orq": s.orq, "members": members, "outside": outside_refs, "sync": sync}))
}

/// O que outra máquina diz do grupo de uma sessão dela: (dona, gid), ou solta.
async fn remote_group(st: &AppState, who: &Member) -> Result<Option<(String, String)>, Refusal> {
    let path = format!("/api/sessions/{}/group", segment(&who.name));
    match st.peers.call(&who.server, reqwest::Method::GET, &path, None).await {
        Ok(Some(body)) => {
            let reported = body["server_id"].as_str().unwrap_or_default();
            if reported != who.server {
                return Err(peer_refusal(&who.server, PeerError::IdentityMismatch { key: who.server.clone(), reported: reported.into() }));
            }
            let (owner, gid) = (body["owner"].as_str().unwrap_or_default(), body["gid"].as_str().unwrap_or_default());
            if owner.is_empty() || gid.is_empty() { return Err(old_version(&who.server)); }
            if body["orq"] == true { return Err(orq_refusal()); }
            Ok(Some((owner.into(), gid.into())))
        }
        Ok(None) => Err(old_version(&who.server)),
        // A rota não existe lá: máquina antiga.
        Err(PeerError::Refused { status: 404, detail }) if detail.get("code").is_none() => Err(old_version(&who.server)),
        Err(PeerError::Refused { status: 404, .. }) => Ok(None),
        Err(PeerError::Unknown) => Err(unregistered(&who.server)),
        // Fora do ar ou outra recusa: tratada como solta; a confirmação da máquina decide depois.
        Err(other) => {
            tracing::warn!(code = "groups_remote_group_unknown", server = %who.server, error = %other.text(&who.server), "groups: o grupo da sessão de outra máquina não foi lido");
            Ok(None)
        }
    }
}

/// `/pair` com membro de outra máquina (ou num grupo que já é entre máquinas): decide a dona e pede
/// a ela. O passe de entrada sai antes de qualquer chamada a outra máquina.
pub(super) async fn pair(asked: Asked, held: Option<IngressPass>, others: Vec<String>, task: String, replace_task: bool, orq: bool) -> Response {
    drop(held);
    let respond = |r: Refusal| asked.reply(r.0, r.1);
    let me = asked.groups.server_id().to_owned();
    if let Err(r) = need_server_id(&me) { return respond(r); }
    if orq { return respond(orq_refusal()); }
    {
        let (dir, me) = (asked.groups.dir(), me.clone());
        match blocking(move || identity_ok(&dir, &me)).await { Ok(Ok(())) => {}, Ok(Err(r)) | Err(r) => return respond(r) }
    }
    if others.iter().any(|o| o.split_once("::").is_some_and(|(srv, _)| srv == me || srv.is_empty())) {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_grupo_identificador_igual", "o membro de outra máquina tem o id desta", json!({}));
    }
    let candidates: Vec<Member> = std::iter::once(Member::new(&me, &asked.name, "")).chain(others.iter().map(|o| Member::parse(&me, o))).collect();
    let registered = asked.st.peers.addressable().await;
    if let Some(c) = candidates.iter().find(|c| c.server != me && !registered.contains(&c.server)) {
        return respond(unregistered(&c.server));
    }
    let locals: Vec<String> = candidates.iter().filter(|c| c.server == me).map(|c| c.name.clone()).collect();
    super::convert::run(&asked.st, &asked.groups, &locals).await;
    let harness = match providers(&asked.st, &locals).await {
        Ok(h) => h,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    let absent = not_found(&locals, &harness);
    if !absent.is_empty() { return respond(sessions_missing(&absent)); }
    // O grupo de cada candidato: daqui pelo sidecar, de fora pela máquina dele, todos ao mesmo
    // tempo; a primeira recusa, na ordem dos candidatos, é a que volta.
    let lookups = futures_util::future::join_all(candidates.iter().map(|c| async {
        if c.server != me { return remote_group(&asked.st, c).await.map_err(Err); }
        match asked.groups.link(&c.name).await {
            Ok(Some(s)) if s.orq => Err(Err(orq_refusal())),
            Ok(Some(s)) => Ok(Some((s.owner_or(&me), s.gid))),
            Ok(None) => Ok(None),
            Err(error) => Err(Ok(error)),
        }
    })).await;
    let mut expected = BTreeMap::new();
    let mut owners: Vec<(String, String)> = Vec::new();
    for (c, found) in candidates.iter().zip(lookups) {
        let found = match found { Ok(found) => found, Err(Ok(error)) => return asked.store_failed(&error), Err(Err(r)) => return respond(r) };
        expected.insert(c.key(), found.as_ref().map_or_else(String::new, |(_, g)| g.clone()));
        if let Some(found) = found && !owners.contains(&found) { owners.push(found); }
    }
    let distinct: BTreeSet<&str> = owners.iter().map(|(o, _)| o.as_str()).collect();
    if distinct.len() > 1 { return respond(owners_refusal()); }
    let candidates: Vec<Member> = candidates.into_iter().map(|c| match harness.get(&c.name) {
        Some(p) if c.server == me => Member { provider: p.clone(), ..c },
        _ => c,
    }).collect();
    let (owner, gid) = owners.first().cloned().unwrap_or_else(|| (me.clone(), String::new()));
    let result = if owner == me {
        owner_join(&asked.st, &asked.groups, JoinAsk { gid, members: candidates, task, replace_task, expected }, Some(harness)).await
    } else {
        if !registered.contains(&owner) { return respond(unregistered(&owner)); }
        let body = json!({"op": "join", "protocol": PROTOCOL, "origin": me, "members": candidates, "task": task, "replace_task": replace_task, "expected": expected});
        match asked.st.peers.call_group_waiting(&owner, &format!("/api/groups/{}/ops", segment(&gid)), &body).await {
            Ok(done) => serde_json::from_value::<JoinDone>(done)
                .map_err(|_| no(StatusCode::BAD_GATEWAY, "erro_grupo_dono_indisponivel", "a dona respondeu fora do formato", json!({"servidor": owner}))),
            Err(error) => Err(peer_refusal(&owner, error)),
        }
    };
    let done = match result { Ok(done) => done, Err(r) => return respond(r) };
    let unconfirmed: Vec<String> = done.unconfirmed.iter().map(Member::key).collect();
    let mut warning = if unconfirmed.is_empty() { super::routes::access_warning(&done.access_failed) } else {
        json!({"code": "erro_grupo_membro_nao_confirmado", "params": {"membros": unconfirmed, "acesso": done.access_failed},
            "msg": format!("não confirmado: {}", unconfirmed.join(", "))})
    };
    if !done.contract_failed.is_empty() {
        if warning.is_null() {
            warning = super::routes::envelope(CONTRACT_FAILED, format!("o grupo ficou, mas o contrato destes grupos não foi herdado: {}", done.contract_failed.join(", ")),
                json!({"grupos": done.contract_failed}));
        } else {
            warning["params"]["grupos"] = json!(done.contract_failed);
        }
    }
    let members: Vec<String> = done.members.iter().map(|m| m.view(&me)).collect();
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members, "gid": done.gid, "owner": owner, "warning": warning}))
}

// --- subida ----------------------------------------------------------------------------------

/// Na subida: a dona reenvia cada registro dela; a participante pede à dona cada grupo em que tem
/// membro e aplica. Com o id trocado a subida nem abre a fila (o diário já disse por quê).
pub fn spawn(st: Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let groups = st.groups.clone()?;
    if groups.server_id().is_empty() { return None; }
    Some(tokio::spawn(async move {
        let me = groups.server_id().to_owned();
        let read = { let (dir, me) = (groups.dir(), me.clone()); blocking(move || (identity_ok(&dir, &me).map(drop), dir.records(), dir.sidecars())).await };
        let (records, sidecars) = match read {
            Ok((Ok(()), Ok(records), Ok(sidecars))) => (records, sidecars),
            Ok((Err(Refusal(_, body)), _, _)) => {
                tracing::warn!(code = "groups_startup_skipped", detail = %body["detail"]["code"], "groups: a subida não reenviou nem pediu grupos");
                return;
            }
            Ok((_, Err(error), _) | (_, _, Err(error))) => {
                tracing::warn!(code = "groups_startup_skipped", %error, "groups: a subida não leu registros ou sidecars; nada reenviado");
                return;
            }
            Err(Refusal(_, body)) => {
                tracing::warn!(code = "groups_startup_skipped", detail = %body["detail"]["code"], "groups: a leitura da subida não rodou");
                return;
            }
        };
        let fed = match of(&st, &groups).await {
            Ok(fed) => fed,
            Err(Refusal(_, body)) => {
                tracing::warn!(code = "groups_startup_skipped", detail = %body["detail"]["params"]["detalhe"], "groups: a fila não abriu na subida");
                return;
            }
        };
        let pushes = super::outbox::restart_pushes(&records, &me);
        if !pushes.is_empty() {
            let outbox = fed.outbox.clone();
            if let Ok(Err(error)) = blocking(move || outbox.enqueue(pushes)).await {
                tracing::warn!(code = "groups_restart_push_failed", %error, "groups: o reenvio da subida não entrou na fila");
            }
        }
        let asks: BTreeSet<(String, String)> = sidecars.iter().filter_map(|(_, s)| s.fed.as_ref().filter(|f| f.owner != me).map(|f| (f.owner.clone(), s.gid.clone()))).collect();
        use futures_util::StreamExt as _;
        // Uma dona fora do ar não atrasa as outras: os pedidos saem juntos, até `ASKS` de uma vez.
        const ASKS: usize = 8;
        let (st_ref, groups_ref) = (&st, &groups);
        futures_util::stream::iter(asks).for_each_concurrent(ASKS, |(owner, gid)| async move {
            match st_ref.peers.call_group(&owner, reqwest::Method::GET, &format!("/api/groups/{}", segment(&gid)), None).await {
                Ok(record) => match serde_json::from_value::<GroupRecord>({ let mut r = record; if let Some(o) = r.as_object_mut() { o.remove("server_id"); } r }) {
                    Ok(r) => {
                        let body = PushBody { protocol: PROTOCOL, owner: r.owner, version: r.version, task: r.task, members: r.members, outside: r.outside, joining: Vec::new(), dissolved: false, convert_from_legacy: None };
                        if let Err(Refusal(status, _)) = local_apply(st_ref, groups_ref, &gid, body).await {
                            tracing::warn!(code = "groups_restart_apply_failed", %owner, %status, "groups: o grupo pedido à dona não foi aplicado");
                        }
                    }
                    Err(_) => tracing::warn!(code = "groups_restart_record_invalid", %owner, "groups: a dona respondeu um registro fora do formato"),
                },
                // Só o `Dissolved` da dona (fila durável) desfaz o grupo aqui: "inexistente" pode ser
                // registro perdido ou id trocado, e dissolver por isso seria para sempre.
                Err(error) => tracing::warn!(code = "groups_restart_ask_failed", %owner, error = %error.text(&owner), "groups: a dona não respondeu na subida"),
            }
        }).await;
        super::convert::run_and_retry(&st, &groups).await;
    }))
}

// --- repasse da porta do Connect ---------------------------------------------------------------

/// Rotas de grupo entre máquinas que o Python das portas dele (Connect) repassa a esta.
pub fn forwarded_route(method: &axum::http::Method, path: &str) -> bool {
    let groups = path.strip_prefix("/api/groups/").is_some_and(|rest| match rest.split_once('/') {
        None => !rest.is_empty(),
        Some((gid, tail)) => !gid.is_empty() && (tail == "ops" || super::fed_notice::owns_tail(tail)),
    });
    let session = *method == axum::http::Method::GET && path.strip_prefix("/api/sessions/").and_then(|r| r.strip_suffix("/group")).is_some_and(|n| !n.is_empty() && !n.contains('/'));
    groups || session
}

/// Pedido que o Python já autenticou como dono. Convidado nunca: recusa com código.
pub async fn serve_forwarded(st: Arc<AppState>, peer: SocketAddr, mut req: Request, guest: bool) -> Response {
    if guest { return reply(no(StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessao compartilhada", json!({}))); }
    req.extensions_mut().insert(Bridged);
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let state = State(st);
    let info = ConnectInfo(peer);
    // O Python segura este pedido: recado a sessão dele volta na resposta, nunca chamado daqui.
    super::bridge::deferring(async move {
        if path.starts_with("/api/sessions/") { return session_group(state, info, req).await; }
        if path.rsplit_once('/').is_some_and(|(_, tail)| super::fed_notice::owns_tail(tail)) {
            return super::fed_notice::serve_forwarded(state.0, peer, req).await;
        }
        match (method, path.ends_with("/ops")) {
            (axum::http::Method::POST, true) => ops(state, info, req).await,
            (axum::http::Method::GET, false) => get_record(state, info, req).await,
            (axum::http::Method::PUT, false) => put_record(state, info, req).await,
            _ => reply(no(StatusCode::METHOD_NOT_ALLOWED, "erro_metodo_nao_permitido", "método não aceito nesta rota", json!({}))),
        }
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O provedor de membro de outra máquina vem da chave `srv::nome` do `harness` (como a dona e o
    /// `PUT` gravam), e par de fora não vira membro.
    #[test]
    fn members_from_sidecar_reads_providers_by_view_and_skips_outside() {
        let s = Sidecar { peers: vec!["lab::s3".into(), "s1".into(), "ana::x".into()], gid: "g".into(),
            harness: BTreeMap::from([("s0".into(), "claude".into()), ("s1".into(), "pi".into()), ("lab::s3".into(), "codex".into())]),
            ..Sidecar::default() };
        let outside = HashSet::from(["ana::x".to_owned()]);
        assert_eq!(members_from_sidecar("casa", "s0", &s, &outside),
            [Member::new("casa", "s0", "claude"), Member::new("lab", "s3", "codex"), Member::new("casa", "s1", "pi")]);
    }
}
