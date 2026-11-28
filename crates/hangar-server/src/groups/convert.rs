//! Par 1:1 antigo entre máquinas (sidecar sem `fed` com um só peer `srv::x`) vira grupo entre
//! máquinas sozinho (I-8B). As duas pontas chegam à mesma dona (menor `server_id`) e ao mesmo
//! `gid` sem conversar; só a dona age, e só grava depois de a outra confirmar. Máquina antiga ou
//! que não responde: o par segue 1:1 e a dona tenta de novo, com espera crescente.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::exit::segment;
use super::federation::{PushBody, Refusal, identity_ok, no, remember, unavailable};
use super::local::is_remote;
use super::model::{Fed, GroupRecord, Member, PROTOCOL, Sidecar};
use super::outbox::OutboxItem;
use super::peers::PeerError;
use super::routes::outside_addresses;
use super::service::GroupService;
use super::store::{PairDir, StoreError};
use crate::routes::AppState;

/// `gid` do par convertido: 12 hex do sha1 dos dois nomes qualificados, em ordem de bytes.
pub fn group_id(a: &str, b: &str) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    sha1_smol::Sha1::from(format!("{lo}\n{hi}")).digest().to_string()[..12].to_owned()
}

/// Espera entre as novas tentativas enquanto sobra par antigo: as máquinas do dono atualizam em
/// dias diferentes, e a dona não pode depender de reiniciar.
pub(super) const RETRY_LADDER: [Duration; 5] = [Duration::from_secs(30), Duration::from_secs(60), Duration::from_secs(120), Duration::from_secs(300), Duration::from_secs(600)];

#[derive(Default)]
struct Kick { notify: Notify, kicked: AtomicBool }

/// A volta viva por pasta `.hangar-pair` (os testes sobem várias máquinas num processo).
static RETRIES: LazyLock<Mutex<HashMap<PathBuf, Arc<Kick>>>> = LazyLock::new(Mutex::default);

fn retries() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Arc<Kick>>> { RETRIES.lock().unwrap_or_else(|e| e.into_inner()) }

/// Subida: converte já e, sobrando par antigo em que esta é a dona, segue tentando com espera
/// crescente até não sobrar nenhum. Sem par antigo, nada fica rodando.
pub async fn run_and_retry(st: &Arc<AppState>, groups: &Arc<GroupService>) {
    if run(st, groups, &[]).await == 0 { return; }
    let root = groups.pair_root().to_path_buf();
    let (kick, ladder) = {
        let mut all = retries();
        if let Some(kick) = all.get(&root) {
            // Já há uma volta: recomeça a escada dela.
            kick.kicked.store(true, Ordering::SeqCst);
            kick.notify.notify_one();
            return;
        }
        let kick = Arc::new(Kick::default());
        all.insert(root.clone(), kick.clone());
        let ladder = super::federation::timing(&root).retry_ladder;
        (kick, if ladder.is_empty() { RETRY_LADDER.to_vec() } else { ladder })
    };
    // shortcut: a volta não é abortada com o servidor (um por processo em produção); só acaba sem par
    // antigo. Ganha cancelamento quando o servidor puder parar e subir de novo no mesmo processo.
    tokio::spawn(retry(st.clone(), groups.clone(), root, kick, ladder));
}

async fn retry(st: Arc<AppState>, groups: Arc<GroupService>, root: PathBuf, kick: Arc<Kick>, ladder: Vec<Duration>) {
    let mut step = 0;
    loop {
        tokio::select! {
            () = tokio::time::sleep(ladder[step.min(ladder.len() - 1)]) => step += 1,
            () = kick.notify.notified() => step = 0,
        }
        kick.kicked.store(false, Ordering::SeqCst);
        if run(&st, &groups, &[]).await > 0 { continue; }
        let mut all = retries();
        // Pedido novo durante a última tentativa: mais uma volta.
        if kick.kicked.swap(false, Ordering::SeqCst) { continue; }
        all.remove(&root);
        return;
    }
}

/// `name` daqui em par antigo com `remote` da máquina `server` (chave do `peers.json`).
struct Legacy { name: String, server: String, remote: String, sidecar: Sidecar }

/// Os sidecars de `names` (vazio = todos). Falha ao ler só ao log: o par segue como está.
fn sidecars(dir: &PairDir, names: &[String]) -> Vec<(String, Sidecar)> {
    let unreadable = |error: StoreError| tracing::warn!(code = "groups_convert_unreadable", %error, "groups: os pares antigos não foram lidos");
    if names.is_empty() { return dir.sidecars().unwrap_or_else(|e| { unreadable(e); Vec::new() }); }
    names.iter().filter_map(|n| match dir.sidecar(n) {
        Ok(s) => s.map(|s| (n.clone(), s)),
        Err(e) => { unreadable(e); None }
    }).collect()
}

/// Pares antigos desta máquina (`names` vazio = todos) e os ambíguos com par de fora.
fn find(dir: &PairDir, peers: &HashSet<String>, aliases: &HashSet<String>, outside: &HashSet<String>, names: &[String]) -> (Vec<Legacy>, Vec<String>) {
    let (mut found, mut ambiguous) = (Vec::new(), Vec::new());
    for (name, s) in sidecars(dir, names) {
        let [peer] = s.peers.as_slice() else { continue };
        if s.fed.is_some() || s.orq || !is_remote(peer) { continue; }
        let Some((server, remote)) = peer.split_once("::") else { continue };
        if !peers.contains(server) { continue; }
        // Apelido de par de fora igual ao nome da máquina: não dá para saber qual dos dois é.
        if aliases.contains(server) || outside.contains(peer)
            || crate::list::links::external_local_session(&name, std::slice::from_ref(peer), dir.root()).is_some() {
            ambiguous.push(name);
            continue;
        }
        found.push(Legacy { server: server.to_owned(), remote: remote.to_owned(), name, sidecar: s });
    }
    (found, ambiguous)
}

/// Pares antigos (`names` vazio = todos) em que esta máquina é a dona, e os ambíguos.
async fn scan(st: &AppState, groups: &GroupService, me: &str, names: &[String]) -> (Vec<Legacy>, Vec<String>) {
    let aliases: HashSet<String> = st.share.as_ref().map(|s| s.pairs.all().into_iter().map(|r| r.alias).collect()).unwrap_or_default();
    let (outside, dir, names, book) = (outside_addresses(st), groups.dir(), names.to_vec(), st.peers.clone());
    let (found, ambiguous) = tokio::task::spawn_blocking(move || find(&dir, &book.ids(), &aliases, &outside, &names)).await.unwrap_or_default();
    // Chave igual ao id (conferido na conversão) dá a mesma dona que o id: a de chave maior espera a outra.
    (found.into_iter().filter(|l| me < l.server.as_str()).collect(), ambiguous)
}

/// Converte os pares antigos desta máquina em que ela é a dona e devolve quantos sobraram.
/// Cada máquina em paralelo, os pares de uma mesma máquina em fila. Falha só ao log e ao diário.
pub async fn run(st: &Arc<AppState>, groups: &Arc<GroupService>, names: &[String]) -> usize {
    let me = groups.server_id().to_owned();
    if me.is_empty() { return 0; }
    let (found, ambiguous) = scan(st, groups, &me, names).await;
    for name in &ambiguous {
        st.diag.report("rust.grupos_conversao_ambigua", name, "grupos_conversao_ambigua", "par 1:1 antigo que também parece par de fora; não convertido");
    }
    let mut by_server: BTreeMap<String, Vec<Legacy>> = BTreeMap::new();
    for legacy in found { by_server.entry(legacy.server.clone()).or_default().push(legacy); }
    let me = me.as_str();
    let left = futures_util::future::join_all(by_server.into_values().map(|pairs| async move {
        let mut left = 0;
        for legacy in pairs { if convert(st, groups, me, legacy).await { left += 1; } }
        left
    })).await;
    left.into_iter().sum()
}

/// Devolve se o par continua antigo (sobra para a próxima tentativa).
async fn convert(st: &Arc<AppState>, groups: &Arc<GroupService>, me: &str, l: Legacy) -> bool {
    let server = l.server.as_str();
    // A sessão de lá como a máquina de lá a vê: quem atendeu, e se o par ainda é este.
    let seen = match st.peers.call(server, reqwest::Method::GET, &format!("/api/sessions/{}/group", segment(&l.remote)), None).await {
        Ok(Some(body)) => body,
        Ok(None) => return old(server),
        Err(PeerError::Refused { status: 404, detail }) if detail.get("code").is_none() => return old(server),
        Err(error) => {
            tracing::warn!(code = "groups_convert_not_confirmed", %server, error = %error.text(server), "groups: o par antigo não foi conferido na outra máquina");
            return true;
        }
    };
    let reported = seen["server_id"].as_str().unwrap_or_default();
    if reported != server {
        st.diag.report("rust.grupos_conversao_identidade", &l.name, "erro_grupo_identificador_divergente", "a chave do peers.json não é o id da outra máquina");
        tracing::warn!(code = "erro_grupo_identificador_divergente", key = %server, %reported, "groups: par antigo não convertido");
        return true;
    }
    let (a, b) = (format!("{me}::{}", l.name), format!("{server}::{}", l.remote));
    let gid = group_id(&a, &b);
    let members: Vec<Member> = serde_json::from_value(seen["members"].clone()).unwrap_or_default();
    let back = members.len() == 2 && members.iter().any(|m| m.server == me && m.name == l.name);
    let legacy_there = seen["owner"] == server && seen["version"] == 0;
    let converted_there = seen["owner"] == me && seen["gid"] == gid.as_str();
    if !back || !(legacy_there || converted_there) {
        tracing::warn!(code = "groups_convert_pair_differs", %server, "groups: a outra máquina não tem o mesmo par antigo; não convertido");
        return true;
    }
    let provider = |m: &str| l.sidecar.harness.get(m).cloned().unwrap_or_default();
    let theirs = members.iter().find(|m| m.server == server).map(|m| m.provider.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| provider(&b));
    let task = l.sidecar.task.clone().unwrap_or_default();
    let record = GroupRecord { protocol: PROTOCOL, gid: gid.clone(), owner: me.into(), version: 1, task: task.clone(),
        members: vec![Member { server: me.into(), name: l.name.clone(), provider: provider(&l.name) }, Member { server: server.into(), name: l.remote.clone(), provider: theirs }],
        outside: Vec::new(), links: Default::default() };
    let body = json!({"protocol": PROTOCOL, "owner": me, "version": 1, "task": task, "members": record.members, "outside": [], "joining": [],
        "convert_from_legacy": a});
    match st.peers.call_group(server, reqwest::Method::PUT, &format!("/api/groups/{}", segment(&gid)), Some(&body)).await {
        Ok(_) => {}
        Err(PeerError::OldVersion) => return old(server),
        Err(error) => {
            tracing::warn!(code = "groups_convert_not_confirmed", %server, error = %error.text(server), "groups: a outra máquina não confirmou a conversão");
            return true;
        }
    }
    // Confirmado: só agora a dona grava registro e `fed`, se o par daqui não mudou no meio.
    let (dir, me_owned, name, peer) = (groups.dir(), me.to_owned(), l.name.clone(), b.clone());
    let written = groups.locked(async move {
        tokio::task::spawn_blocking(move || write_owner(&dir, &me_owned, &name, &peer, record)).await
    }).await;
    match written {
        Ok(Ok(Written::Done)) => { groups.changed(); false }
        Ok(Ok(Written::Already)) => false,
        // Id trocado não desfaz o grupo na outra ponta: só o diário diz por quê.
        Ok(Ok(Written::IdChanged)) => {
            tracing::error!(code = "erro_grupo_identificador_trocado", "groups: conversão confirmada não gravada: o CP_SERVER_ID desta máquina mudou");
            true
        }
        Ok(Ok(Written::Gone)) => {
            // A outra já converteu: o grupo acaba lá também, como acabou aqui.
            tracing::warn!(code = "groups_convert_pair_gone", %server, "groups: o par antigo mudou durante a conversão; o grupo foi desfeito");
            let item = OutboxItem::Dissolved { server: server.to_owned(), gid, version: 2, origin: me.to_owned() };
            match super::federation::of(st, groups).await {
                Ok(fed) => {
                    let outbox = fed.outbox().clone();
                    if !matches!(tokio::task::spawn_blocking(move || outbox.enqueue(vec![item])).await, Ok(Ok(()))) {
                        tracing::error!(code = "groups_convert_undo_failed", %server, "groups: a dissolução da conversão não entrou na fila");
                    }
                }
                Err(_) => tracing::error!(code = "groups_convert_undo_failed", %server, "groups: a dissolução da conversão não entrou na fila"),
            }
            false
        }
        Ok(Err(error)) => { tracing::error!(code = "groups_convert_store_failed", %error, "groups: a conversão confirmada não foi gravada; tenta de novo na subida"); true }
        Err(error) => { tracing::error!(code = "groups_convert_store_failed", %error, "groups: a conversão confirmada não foi gravada; tenta de novo na subida"); true }
    }
}

fn old(server: &str) -> bool {
    tracing::info!(code = "groups_convert_old_version", %server, "groups: a outra máquina é de versão antiga; o par segue 1:1");
    true
}

enum Written { Done, Already, Gone, IdChanged }

fn write_owner(dir: &PairDir, me: &str, name: &str, peer: &str, record: GroupRecord) -> Result<Written, StoreError> {
    if dir.identity_check(me)?.is_some() { return Ok(Written::IdChanged); }
    let Some(s) = dir.sidecar(name)? else { return Ok(Written::Gone) };
    if s.gid == record.gid && s.fed.as_ref().is_some_and(|f| f.owner == me) { return Ok(Written::Already); }
    if s.fed.is_some() || s.orq || s.peers != [peer] { return Ok(Written::Gone); }
    dir.write_record(&record)?;
    let converted = Sidecar { task: Some(record.task.clone()), gid: record.gid.clone(), fed: Some(Fed { owner: me.into(), local: true, version: record.version }), ..s };
    if let Err(error) = dir.write_sidecar(name, &converted) {
        if let Err(undo) = dir.remove_record(&record.gid, "convert_undo") {
            tracing::error!(code = "groups_record_restore_failed", gid = %record.gid, error = %undo, "groups: o registro da conversão não foi desfeito");
        }
        return Err(error);
    }
    remember(dir, me);
    Ok(Written::Done)
}

fn differs(name: &str) -> Refusal {
    no(StatusCode::CONFLICT, "erro_grupo_mudou", &format!("'{name}' não está no par antigo que a dona quer converter"), json!({"membro": name}))
}

fn store(error: impl std::fmt::Display) -> Refusal {
    tracing::error!(code = "groups_store_failed", %error, "groups: a conversão do par antigo não foi lida ou gravada");
    unavailable("groups_store_failed")
}

/// A participante recebe o `PUT` da dona com `convert_from_legacy`: converte o próprio sidecar se
/// ele é exatamente esse par antigo (ou já é este grupo). Não cria registro nem avisa a sessão.
pub(super) async fn accept(groups: &Arc<GroupService>, gid: &str, body: PushBody, from: String) -> Result<Value, Refusal> {
    let me = groups.server_id().to_owned();
    let (mine, theirs): (Vec<&Member>, Vec<&Member>) = body.members.iter().partition(|m| m.server == me);
    let ([m], [o]) = (mine.as_slice(), theirs.as_slice()) else { return Err(differs(&from)) };
    let (name, owner_side, provider) = (m.name.clone(), o.server.clone(), o.provider.clone());
    let expected = format!("{}::{}", o.server, o.name);
    // Dona e gid recalculados aqui: a regra é a mesma nas duas pontas.
    if body.dissolved || owner_side != body.owner || from != expected || body.owner >= me || group_id(&from, &format!("{me}::{name}")) != gid {
        return Err(differs(&name));
    }
    let (dir, gid_owned, reply_name) = (groups.dir(), gid.to_owned(), name.clone());
    let done = groups.locked(async move {
        tokio::task::spawn_blocking(move || -> Result<(u64, bool), Refusal> {
            identity_ok(&dir, &me)?;
            let Some(s) = dir.sidecar(&name).map_err(store)? else { return Err(differs(&name)) };
            if let Some(f) = s.fed.as_ref().filter(|f| f.owner == body.owner && s.gid == gid_owned) { return Ok((f.version, false)); }
            if s.fed.is_some() || s.orq || s.peers != [from.clone()] { return Err(differs(&name)); }
            let mut harness = s.harness.clone();
            if !provider.is_empty() { harness.entry(from).or_insert(provider); }
            let converted = Sidecar { task: Some(body.task), gid: gid_owned, harness, fed: Some(Fed { owner: body.owner, local: false, version: body.version }), ..s };
            dir.write_sidecar(&name, &converted).map_err(store)?;
            remember(&dir, &me);
            Ok((body.version, true))
        }).await
    }).await;
    let (version, changed) = done.map_err(store)??;
    if changed { groups.changed(); }
    Ok(json!({"protocol": PROTOCOL, "version": version, "members": [reply_name]}))
}
