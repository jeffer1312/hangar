//! Sair, morrer e renomear em grupo entre máquinas (sidecar com `fed`). A máquina do membro grava
//! na hora e põe o `Leave`/`Rename` na fila no mesmo passo, sob o lock, sem esperar a dona; a dona
//! aplica (`remove_members`) e, quando sobra uma máquina só com membros, dissolve: o registro sai e
//! a máquina que ficou fica com grupo local comum, mesmo gid, com os pares de fora que já tinha.
use std::collections::{BTreeSet, HashSet};

use serde_json::{Value, json};

use super::federation::{Refusal, need_server_id, of, read, remove_members, store_refusal};
use super::local::{self, Snapshot, is_remote};
use super::model::{Fed, GroupRecord, Member, PROTOCOL, Sidecar};
use super::outbox::{Fan, Outbox, OutboxItem, fan_out, remote_servers};
use super::routes::{PREFIX, outside_addresses};
use super::service::{GroupError, GroupService, apply, restore_logged};
use super::store::{PairDir, StoreError};
use crate::routes::AppState;
use crate::session_write::input::deliver_text;

/// Máquinas com membro no registro (a dona só conta se tiver membro).
fn machines(record: &GroupRecord) -> usize { record.members.iter().map(|m| m.server.as_str()).collect::<BTreeSet<_>>().len() }

fn io_error(e: impl std::fmt::Display) -> GroupError { GroupError::Store(StoreError::Io(std::io::Error::other(e.to_string()))) }

fn refused(Refusal(status, body): Refusal) -> GroupError { io_error(format!("{status} {}", body["detail"]["code"])) }

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, GroupError> {
    tokio::task::spawn_blocking(f).await.map_err(io_error)
}

/// Plano local (`clears`, `writes`, `before`) gravado e o item da fila no mesmo passo; fila que
/// recusa desfaz o disco.
fn apply_and_enqueue(dir: &PairDir, outbox: &Outbox, clears: &[String], writes: &[(String, Sidecar)], before: &Snapshot, item: Option<OutboxItem>, why: &str) -> Result<(), GroupError> {
    let done = apply(dir, clears, writes, why).map_err(GroupError::from)
        .and_then(|()| item.map_or(Ok(()), |i| outbox.enqueue(vec![i]).map_err(io_error)));
    if done.is_err() { restore_logged(dir, before); }
    done
}

/// O aviso a quem ficou sem ninguém de outra máquina. Falha só ao diário.
pub(super) async fn tell_dropped(st: &AppState, names: &[String]) {
    for n in names {
        let text = format!("{PREFIX} Você saiu do grupo de trabalho entre máquinas. Volte a operar independente; use hangar-send só quando o usuário pedir.");
        if let Err(error) = deliver_text(st, n, &text).await {
            tracing::warn!(code = "groups_exit_notice_failed", session = %n, error = %error["code"], "groups: o aviso de saída não chegou");
        }
    }
}

/// `remove_and_dissolve` de um membro, com o aviso de saída já entregue.
async fn remove_and_tell(st: &AppState, groups: &GroupService, gid: &str, who: Member, why: &'static str) -> Result<Option<GroupRecord>, Refusal> {
    let Some((record, dropped)) = remove_and_dissolve(st, groups, gid, &[who], why).await? else { return Ok(None) };
    tell_dropped(st, &dropped).await;
    Ok(Some(record))
}

/// Saída de `name` se o grupo dela é entre máquinas; `None` = grupo comum (segue o caminho local).
/// Devolve os ex-companheiros.
pub async fn leave(st: &AppState, groups: &GroupService, name: &str, why: &'static str) -> Option<Result<Vec<String>, GroupError>> {
    let link = groups.link(name).await.ok().flatten()?;
    let fed = link.fed.clone()?;
    let me = groups.server_id().to_owned();
    // Sem id este processo não é membro de grupo entre máquinas nenhum (servidor de teste sobre a
    // pasta real): sair como participante apagaria o membro da dona sem tirá-lo do registro dela.
    if let Err(r) = need_server_id(&me) { return Some(Err(refused(r))); }
    if fed.owner == me {
        return Some(match remove_and_tell(st, groups, &link.gid, Member::new(&me, name, ""), why).await {
            Ok(Some(_)) => Ok(link.peers),
            // Sem registro aqui o grupo não é mais desta dona: sai como grupo comum.
            Ok(None) => groups.leave(name, why).await,
            Err(r) => Err(refused(r)),
        });
    }
    let outbox = match of(st, groups).await { Ok(f) => f.outbox().clone(), Err(r) => return Some(Err(refused(r))) };
    let (dir, name) = (groups.dir(), name.to_owned());
    let left = groups.locked(blocking(move || -> Result<Option<Vec<String>>, GroupError> {
        let read = |n: &str| read(&dir, n);
        let Some(link) = read(&name) else { return Ok(None) };
        let Some(plan) = local::plan_leave(&read, &name, false) else { return Ok(None) };
        // O `Leave` entra junto da saída: um envio da dona no meio do caminho já o vê pendente.
        let item = link.fed.filter(|f| f.owner != me).map(|f| OutboxItem::Leave { owner: f.owner, gid: link.gid.clone(), member: name.clone(), origin: me });
        apply_and_enqueue(&dir, &outbox, &plan.clears, &plan.writes, &plan.before, item, why)?;
        Ok(Some(plan.ex_peers))
    })).await.and_then(|r| r);
    if matches!(left, Ok(Some(_))) { groups.changed(); }
    Some(left.map(Option::unwrap_or_default))
}

/// Dos ex-companheiros, quem o `notify_exit` ainda avisa: sem as máquinas do dono, que a dona avisa
/// pela fila.
pub fn notified(st: &AppState, ex: &[String]) -> Vec<String> {
    let outside = outside_addresses(st);
    ex.iter().filter(|p| !is_remote(p) || outside.contains(*p)).cloned().collect()
}

/// Renomear sessão de grupo entre máquinas; `None` = grupo comum.
pub async fn rename(st: &AppState, groups: &GroupService, old: &str, new: &str) -> Option<Result<(), GroupError>> {
    let fed = groups.link(old).await.ok().flatten()?.fed?;
    let me = groups.server_id().to_owned();
    if let Err(r) = need_server_id(&me) { return Some(Err(refused(r))); }
    let outbox = match of(st, groups).await { Ok(f) => f.outbox().clone(), Err(r) => return Some(Err(refused(r))) };
    let (dir, old, new) = (groups.dir(), old.to_owned(), new.to_owned());
    let done = groups.locked(blocking(move || -> Result<(), GroupError> {
        let read = |n: &str| read(&dir, n);
        let Some(link) = read(&old) else { return Ok(()) };
        let (clears, writes) = local::plan_rename(&read, &old, &new);
        let before: Snapshot = clears.iter().chain(writes.iter().map(|(n, _)| n)).map(|n| (n.clone(), read(n))).collect();
        // Na participante o `Rename` entra com o disco: o `Push` com o nome velho já o vê pendente.
        let item = (fed.owner != me).then(|| OutboxItem::Rename { owner: fed.owner.clone(), gid: link.gid.clone(), from: old.clone(), to: new.clone(), origin: me.clone() });
        apply_and_enqueue(&dir, &outbox, &clears, &writes, &before, item, "rename")?;
        if fed.owner == me
            && let Err(r) = owner_rename(&dir, &outbox, &me, &link.gid, &Member::new(&me, &old, ""), &new) {
            restore_logged(&dir, &before);
            return Err(refused(r));
        }
        Ok(())
    })).await.and_then(|r| r);
    if done.is_ok() { groups.changed(); }
    Some(done)
}

/// Na dona: `from` passa a se chamar `to` no registro e nos sidecars daqui; versão nova a todos.
/// Membro que não está no registro (rename repetido) não muda nada.
fn owner_rename(dir: &PairDir, outbox: &Outbox, me: &str, gid: &str, from: &Member, to: &str) -> Result<u64, Refusal> {
    let Some(mut record) = dir.record(gid).map_err(store_refusal)?.filter(|r| r.owner == me) else { return Ok(0) };
    let Some(m) = record.members.iter_mut().find(|m| m.same(from)) else { return Ok(record.version) };
    m.name = to.to_owned();
    let renamed = m.clone();
    let (old_key, new_key) = (from.key(), renamed.key());
    let moved = |k: String| if k == old_key { new_key.clone() } else { k };
    record.links = std::mem::take(&mut record.links).into_iter().map(|(c, p)| (moved(c), moved(p))).collect();
    record.version += 1;
    dir.write_record(&record).map_err(store_refusal)?;
    let (old_view, new_view) = (from.view(me), renamed.view(me));
    for m in record.members.iter().filter(|m| m.server == me) {
        let Some(mut s) = read(dir, &m.name).filter(|s| s.gid == gid) else { continue };
        for p in s.peers.iter_mut().chain(s.linked_children.iter_mut()).chain(s.linked_by.iter_mut()).filter(|p| **p == old_view) { p.clone_from(&new_view); }
        if let Some(provider) = s.harness.remove(&old_view) { s.harness.insert(new_view.clone(), provider); }
        s.fed = Some(Fed { owner: me.into(), local: true, version: record.version });
        dir.write_sidecar(&m.name, &s).map_err(store_refusal)?;
    }
    outbox.enqueue(fan_out(remote_servers(&record, me), gid, record.version, me, Fan::Push)).map_err(store_refusal)?;
    Ok(record.version)
}

/// `op: leave` na dona: `member` de `origin` saiu. Repetido ou de grupo que não existe mais: nada muda.
pub async fn leave_op(st: &AppState, groups: &GroupService, gid: &str, origin: &str, name: &str) -> Result<Value, Refusal> {
    let version = remove_and_tell(st, groups, gid, Member::new(origin, name, ""), "owner_op_leave").await?.map_or(0, |r| r.version);
    Ok(json!({"protocol": PROTOCOL, "version": version}))
}

/// `op: rename` na dona.
pub async fn rename_op(st: &AppState, groups: &GroupService, gid: &str, origin: &str, from: &str, to: &str) -> Result<Value, Refusal> {
    let me = groups.server_id().to_owned();
    let outbox = of(st, groups).await?.outbox().clone();
    let (dir, gid, from, to) = (groups.dir(), gid.to_owned(), Member::new(origin, from, ""), to.to_owned());
    let version = groups.locked(blocking(move || owner_rename(&dir, &outbox, &me, &gid, &from, &to))).await.map_err(store_refusal)??;
    groups.changed();
    Ok(json!({"protocol": PROTOCOL, "version": version}))
}

/// Par 1:1 antigo que a outra máquina ainda desfaz por `/unpair-remote`, num sidecar já `fed`:
/// `peer` (`srv::x`) sai pela dona.
pub async fn peer_left(st: &AppState, groups: &GroupService, link: &Sidecar, peer: &str) -> Result<(), Refusal> {
    let (Some(fed), Some((srv, name))) = (&link.fed, peer.split_once("::")) else { return Ok(()) };
    let who = Member::new(srv, name, "");
    if fed.owner == groups.server_id() { return remove_and_tell(st, groups, &link.gid, who, "unpair_remote").await.map(drop); }
    let outbox = of(st, groups).await?.outbox().clone();
    let item = OutboxItem::Leave { owner: fed.owner.clone(), gid: link.gid.clone(), member: who.name, origin: who.server };
    blocking(move || outbox.enqueue(vec![item])).await.map_err(store_refusal)?.map_err(store_refusal)
}

/// `remove_members` e, com uma máquina só com membros, a dissolução. Devolve o registro depois da
/// saída e quem daqui ficou sozinho, para quem chamou avisar (`tell_dropped`); `None` sem registro
/// desta dona. Ninguém dos `whos` no registro: nada muda, nem a dissolução.
pub async fn remove_and_dissolve(st: &AppState, groups: &GroupService, gid: &str, whos: &[Member], why: &'static str) -> Result<Option<(GroupRecord, Vec<String>)>, Refusal> {
    let Some((record, mut dropped, changed)) = remove_members(st, groups, gid, whos, why).await? else { return Ok(None) };
    if changed && machines(&record) <= 1 {
        dropped.extend(dissolve(st, groups, gid, whos.iter().map(|w| w.server.clone()).collect(), why).await?);
    }
    Ok(Some((record, dropped)))
}

/// O registro sai, cada outra máquina recebe `Dissolved` e os sidecars daqui viram grupo local
/// comum (sem `fed`, sem membros de outra máquina, com os pares de fora). Devolve quem ficou sozinho.
/// Dona sem membro próprio continua dona enquanto houver membros em duas máquinas.
async fn dissolve(st: &AppState, groups: &GroupService, gid: &str, gone_servers: BTreeSet<String>, why: &'static str) -> Result<Vec<String>, Refusal> {
    let me = groups.server_id().to_owned();
    let outbox = of(st, groups).await?.outbox().clone();
    let outside: HashSet<String> = outside_addresses(st);
    let (dir, gid) = (groups.dir(), gid.to_owned());
    let (dropped, orphan) = groups.locked(blocking(move || dissolve_here(&dir, &outbox, &me, &gid, &outside, gone_servers, why)))
        .await.map_err(store_refusal)??;
    groups.changed();
    if let Some(gid) = orphan { groups.archive_contracts(&gid).await.map_err(store_refusal)?; }
    Ok(dropped)
}

/// O disco e a fila da dissolução, sob o lock: devolve quem ficou sozinho e o gid cujo contrato vai
/// ao arquivo.
fn dissolve_here(dir: &PairDir, outbox: &Outbox, me: &str, gid: &str, outside: &HashSet<String>, gone_servers: BTreeSet<String>, why: &str)
    -> Result<(Vec<String>, Option<String>), Refusal> {
    // Alguém pode ter entrado entre a saída e aqui.
    let Some(record) = dir.record(gid).map_err(store_refusal)?.filter(|r| r.owner == me && machines(r) <= 1) else { return Ok((Vec::new(), None)) };
    // Sem membro daqui o grupo segue (ou acaba) noutra máquina: o contrato da dona vai ao arquivo.
    let orphan = (!record.members.iter().any(|m| m.server == me)).then(|| gid.to_owned());
    let mut servers: BTreeSet<String> = record.members.iter().map(|m| m.server.clone()).collect();
    servers.extend(gone_servers);
    servers.remove(me);
    let before: Snapshot = record.members.iter().filter(|m| m.server == me).map(|m| (m.name.clone(), read(dir, &m.name))).collect();
    let done = (|| {
        let mut dropped = Vec::new();
        for m in record.members.iter().filter(|m| m.server == me) {
            let Some(mut s) = read(dir, &m.name).filter(|s| s.gid == gid) else { continue };
            s.fed = None;
            s.peers.retain(|p| !is_remote(p) || outside.contains(p));
            s.drop_stale_links();
            if s.peers.is_empty() && !s.orq {
                dir.clear_sidecar(&m.name, why).map_err(store_refusal)?;
                dropped.push(m.name.clone());
            } else {
                dir.write_sidecar(&m.name, &s).map_err(store_refusal)?;
            }
        }
        dir.remove_record(gid, why).map_err(store_refusal)?;
        outbox.enqueue(fan_out(servers, gid, record.version + 1, me, Fan::Dissolved)).map_err(store_refusal)?;
        Ok(dropped)
    })();
    // Sem o `Dissolved` na fila as outras máquinas seguiriam no grupo: daqui ele volta como era.
    if done.is_err() {
        restore_logged(dir, &before);
        if let Err(error) = dir.write_record(&record) {
            tracing::error!(code = "groups_restore_failed", %gid, %error, "groups: o registro do grupo não voltou depois da dissolução que falhou");
        }
    }
    Ok((done?, orphan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::outbox::OutboxSend;
    use crate::groups::peers::PeerError;
    use crate::groups::service::BoxFuture;

    struct Down;
    impl OutboxSend for Down {
        fn send<'a>(&'a self, _item: &'a OutboxItem) -> BoxFuture<'a, Result<(), PeerError>> { Box::pin(async { Err(PeerError::Transport("fora".into())) }) }
        fn failed(&self, _item: &OutboxItem, _code: &str, _error: &PeerError, _retry: bool) {}
    }

    /// A fila que não grava o `Dissolved` não deixa o grupo meio dissolvido: registro e sidecars
    /// daqui ficam como estavam, e as outras máquinas nunca souberam.
    #[tokio::test]
    async fn dissolve_that_cannot_enqueue_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = PairDir::new(tmp.path().to_path_buf(), tmp.path().join("archive"));
        let record = GroupRecord { protocol: PROTOCOL, gid: "g".into(), owner: "casa".into(), version: 3, task: String::new(),
            members: vec![Member::new("casa", "s0", "claude"), Member::new("casa", "s3", "claude")], outside: Vec::new(), links: Default::default() };
        dir.write_record(&record).unwrap();
        let fed = Some(Fed { owner: "casa".into(), local: true, version: 3 });
        for (name, peer) in [("s0", "s3"), ("s3", "s0")] {
            dir.write_sidecar(name, &Sidecar { peers: vec![peer.into()], gid: "g".into(), fed: fed.clone(), ..Sidecar::default() }).unwrap();
        }
        let outbox = Outbox::open(tmp.path().join("groups"), std::sync::Arc::new(Down)).unwrap();
        // O temporário da fila como pasta: a gravação dela falha.
        std::fs::create_dir_all(tmp.path().join("groups/outbox.json.tmp/x")).unwrap();
        let gone = BTreeSet::from(["lab".to_owned()]);
        assert!(dissolve_here(&dir, &outbox, "casa", "g", &HashSet::new(), gone, "test").is_err());
        assert_eq!(dir.record("g").unwrap(), Some(record));
        assert_eq!(read(&dir, "s0").and_then(|s| s.fed), fed);
        assert!(outbox.pending().is_empty());
    }
}
