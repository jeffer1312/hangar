//! Operações de grupo de uma máquina como planejadores puros: leem sidecars por uma função e
//! devolvem o que gravar e apagar. Quem aplica (e restaura em falha) é o `GroupService`.
//! Porta de `join_group`, `leave` e `rename_pair` do `pair.py`.
use super::model::Sidecar;
use std::collections::BTreeMap;

pub type Snapshot = BTreeMap<String, Option<Sidecar>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinRefusal { Mix, TaskConflict { existing: String }, OrqOutside, Orq }

pub struct JoinInput<'a> {
    pub name: &'a str,
    pub others: &'a [String],
    pub task: &'a str,
    pub replace_task: bool,
    pub harness: &'a BTreeMap<String, String>,
    pub orq: bool,
    /// Membro remoto que é par externo (`alias::sessao` de outra pessoa), não máquina própria.
    pub outside: &'a dyn Fn(&str) -> bool,
    /// `others` são filhas que `name` criou: ganham `linked_by = name`.
    pub linked: bool,
}

pub struct JoinPlan {
    /// União dos grupos, em ordem estável; inclui membros remotos (`servidor::sessao`).
    pub members: Vec<String>,
    pub gid: String,
    pub new_gid: bool,
    pub task: String,
    pub orq: bool,
    /// gids dos grupos absorvidos, para herdar o contrato no sobrevivente.
    pub merged: Vec<String>,
    pub writes: Vec<(String, Sidecar)>,
    /// Estado anterior dos membros locais, para desfazer.
    pub before: Snapshot,
}

pub struct LeavePlan {
    pub ex_peers: Vec<String>,
    pub clears: Vec<String>,
    pub writes: Vec<(String, Sidecar)>,
    pub archive: Option<String>,
    pub before: Snapshot,
}

/// Membro remoto não tem sidecar aqui: vive na máquina dele.
pub fn is_remote(name: &str) -> bool { name.contains("::") }

fn dedup<I: IntoIterator<Item = String>>(items: I) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) { out.push(item); }
    }
    out
}

/// (membros incluindo `name`, tarefa, gid); sem grupo = (`[name]`, "", "").
fn members_of(read: &dyn Fn(&str) -> Option<Sidecar>, name: &str) -> (Vec<String>, String, String) {
    let link = if is_remote(name) { None } else { read(name) };
    match link {
        None => (vec![name.to_owned()], String::new(), String::new()),
        Some(s) => {
            let members = std::iter::once(name.to_owned()).chain(s.peers.iter().cloned()).collect();
            (members, s.task.unwrap_or_default(), s.gid)
        }
    }
}

fn snapshot(read: &dyn Fn(&str) -> Option<Sidecar>, names: impl IntoIterator<Item = String>) -> Snapshot {
    names.into_iter().filter(|n| !is_remote(n)).map(|n| { let s = read(&n); (n, s) }).collect()
}

pub fn plan_join(read: &dyn Fn(&str) -> Option<Sidecar>, input: &JoinInput, fresh_gid: &dyn Fn() -> String) -> Result<JoinPlan, JoinRefusal> {
    let names = dedup(std::iter::once(input.name.to_owned()).chain(input.others.iter().cloned()));
    let infos: Vec<_> = names.iter().map(|n| members_of(read, n)).collect();
    let members = dedup(infos.iter().flat_map(|(ms, _, _)| ms.iter().cloned()));

    // Remoto de máquina própria aqui só é o par 1:1 antigo (uma sessão local + um remoto): a união
    // nunca o arrasta para dentro de um grupo; grupo entre máquinas passa pela federação. Membro de
    // fora é membro comum.
    let outsiders = members.iter().filter(|m| is_remote(m) && (input.outside)(m)).count();
    let own = members.iter().filter(|m| is_remote(m)).count() - outsiders;
    let locals = members.len() - own - outsiders;
    if own > 0 && (own > 1 || locals != 1 || outsiders > 0) {
        return Err(JoinRefusal::Mix);
    }

    let before = snapshot(read, members.iter().cloned());
    // Executores entram e saem do grupo `orq` o tempo todo: ninguém de fora acompanha isso.
    if outsiders > 0 && (input.orq || before.values().flatten().any(|s| s.orq)) {
        return Err(JoinRefusal::OrqOutside);
    }
    let existing = infos.iter().map(|(_, t, _)| t.as_str()).find(|t| !t.is_empty()).unwrap_or("");
    let asked = input.task.trim();
    if !asked.is_empty() && !existing.is_empty() && asked != existing && !input.replace_task {
        return Err(JoinRefusal::TaskConflict { existing: existing.to_owned() });
    }
    let task = if !asked.is_empty() && (existing.is_empty() || input.replace_task) { asked } else { existing }.to_owned();

    let gids = dedup(infos.iter().map(|(_, _, g)| g.clone()).filter(|g| !g.is_empty()));
    let new_gid = gids.is_empty();
    let gid = gids.first().cloned().unwrap_or_else(fresh_gid);
    let merged = gids.iter().skip(1).cloned().collect();

    // O que os sidecars já sabiam vale até o chamador trazer o provider atual.
    let mut harness: BTreeMap<String, String> = BTreeMap::new();
    for sidecar in before.values().flatten() {
        harness.extend(sidecar.harness.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    harness.extend(input.harness.iter().map(|(k, v)| (k.clone(), v.clone())));
    // Grupo de orquestração continua sendo, mesmo quando o join seguinte não repete a marca.
    let orq = input.orq || before.values().flatten().any(|s| s.orq);
    // Interromper e devolver resposta são do criador; dentro de `orq` isso disputaria com o árbitro.
    if input.linked && orq { return Err(JoinRefusal::Orq); }

    // Quem forma o grupo é a principal; juntar a um grupo existente mantém a dele (a do sobrevivente).
    let lead = if new_gid { Some(input.name.to_owned()) } else {
        let olds = || before.values().flatten();
        olds().filter(|s| s.gid == gid).find_map(|s| s.lead.clone()).or_else(|| olds().find_map(|s| s.lead.clone()))
    };
    let writes = members.iter().filter(|m| !is_remote(m)).map(|m| {
        let peers = members.iter().filter(|p| *p != m).cloned().collect();
        let old = before.get(m).cloned().flatten();
        let linked_by = if input.linked && input.others.contains(m) { Some(input.name.to_owned()) }
            else { old.as_ref().and_then(|s| s.linked_by.clone()) };
        // Grupo entre máquinas vai pela dona (`federation.rs`); aqui o `fed` só não se perde.
        let fed = old.and_then(|s| s.fed);
        (m.clone(), Sidecar { peers, task: Some(task.clone()), gid: gid.clone(), harness: harness.clone(), orq, fed, linked_by, lead: lead.clone() })
    }).collect();
    Ok(JoinPlan { members, gid, new_gid, task, orq, merged, writes, before })
}

/// `orq_alive`: grupo `orq` com execução viva (ou ilegível) fica com o árbitro sozinho.
pub fn plan_leave(read: &dyn Fn(&str) -> Option<Sidecar>, name: &str, orq_alive: bool) -> Option<LeavePlan> {
    let link = read(name)?;
    let alive = link.orq && orq_alive;
    let peers = link.peers.clone();
    let before = snapshot(read, std::iter::once(name.to_owned()).chain(peers.iter().cloned()));
    let mut clears = vec![name.to_owned()];
    let mut writes = Vec::new();
    if peers.len() == 1 && !alive {
        if !is_remote(&peers[0]) { clears.push(peers[0].clone()); }
    } else {
        for peer in peers.iter().filter(|p| !is_remote(p)) {
            if let Some(mut st) = read(peer) {
                st.peers.retain(|x| x != name);
                // Vínculo velho reviveria se o pai voltasse ao grupo ou outra sessão herdasse o nome.
                if st.linked_by.as_deref() == Some(name) { st.linked_by = None; }
                writes.push((peer.clone(), st));
            }
        }
    }
    let archive = (peers.len() <= 1 && !alive && !link.gid.is_empty()).then(|| link.gid.clone());
    Some(LeavePlan { ex_peers: peers, clears, writes, archive, before })
}

/// (clears, writes): aplicar nesta ordem. Migra o próprio sidecar e a lista de cada companheiro.
pub fn plan_rename(read: &dyn Fn(&str) -> Option<Sidecar>, old: &str, new: &str) -> (Vec<String>, Vec<(String, Sidecar)>) {
    let clears = vec![old.to_owned()];
    let Some(link) = read(old) else { return (clears, Vec::new()) };
    let renamed = |h: &BTreeMap<String, String>| -> BTreeMap<String, String> {
        h.iter().map(|(n, p)| (if n == old { new.to_owned() } else { n.clone() }, p.clone())).collect()
    };
    let mut own = link.clone();
    own.harness = renamed(&link.harness);
    if own.lead.as_deref() == Some(old) { own.lead = Some(new.to_owned()); }
    let mut writes = vec![(new.to_owned(), own)];
    for peer in link.peers.iter().filter(|p| !is_remote(p)) {
        if let Some(mut st) = read(peer) {
            for x in st.peers.iter_mut().filter(|x| *x == old) { *x = new.to_owned(); }
            st.harness = renamed(&st.harness);
            if st.linked_by.as_deref() == Some(old) { st.linked_by = Some(new.to_owned()); }
            if st.lead.as_deref() == Some(old) { st.lead = Some(new.to_owned()); }
            writes.push((peer.clone(), st));
        }
    }
    (clears, writes)
}

/// Por que o grupo de `link` não recebe um par de fora: `orq` (o árbitro sozinho tem aviso próprio),
/// ou par antigo 1:1 com máquina própria. Grupo entre máquinas (`fed`) aceita: a dona fica sabendo.
pub fn external_link_refusal(link: Option<&Sidecar>, outside: &dyn Fn(&str) -> bool) -> Option<JoinRefusal> {
    let link = link?;
    if link.orq { return Some(if link.peers.is_empty() { JoinRefusal::Orq } else { JoinRefusal::OrqOutside }); }
    (link.fed.is_none() && link.peers.iter().any(|p| is_remote(p) && !outside(p))).then_some(JoinRefusal::Mix)
}

/// Par com sessão de outra pessoa: o endereço entra no sidecar de cada membro local do grupo de
/// `local`; sessão solta ganha grupo novo só com ele.
pub fn plan_external_link(read: &dyn Fn(&str) -> Option<Sidecar>, local: &str, address: &str,
    harness: &BTreeMap<String, String>, outside: &dyn Fn(&str) -> bool, fresh_gid: &dyn Fn() -> String) -> Result<Vec<(String, Sidecar)>, JoinRefusal> {
    let Some(link) = read(local) else {
        let sidecar = Sidecar { peers: vec![address.to_owned()], task: Some(String::new()), gid: fresh_gid(), harness: harness.clone(), orq: false, fed: None, linked_by: None, lead: None };
        return Ok(vec![(local.to_owned(), sidecar)]);
    };
    if let Some(refused) = external_link_refusal(Some(&link), outside) { return Err(refused); }
    let locals = std::iter::once(local.to_owned()).chain(link.peers.iter().filter(|p| !is_remote(p)).cloned());
    Ok(locals.filter_map(|m| {
        let mut st = if m == local { link.clone() } else { read(&m)? };
        if !st.peers.iter().any(|p| p == address) { st.peers.push(address.to_owned()); }
        st.harness.extend(harness.iter().map(|(k, v)| (k.clone(), v.clone())));
        Some((m, st))
    }).collect())
}

/// Tira `address` de cada membro local do grupo de `local`; quem fica sem ninguém (e sem `orq`)
/// sai do grupo. (clears, writes, gid a arquivar quando o grupo acabou.)
pub fn plan_external_unlink(read: &dyn Fn(&str) -> Option<Sidecar>, local: &str, address: &str) -> (Vec<String>, Vec<(String, Sidecar)>, Option<String>) {
    let Some(link) = read(local).filter(|l| l.peers.iter().any(|p| p == address)) else { return (Vec::new(), Vec::new(), None) };
    let (mut clears, mut writes, mut archive) = (Vec::new(), Vec::new(), None);
    let locals = std::iter::once(local.to_owned()).chain(link.peers.iter().filter(|p| !is_remote(p)).cloned());
    for m in locals {
        let Some(mut st) = (if m == local { Some(link.clone()) } else { read(&m) }) else { continue };
        if !st.peers.iter().any(|p| p == address) { continue; }
        st.peers.retain(|p| p != address);
        if st.peers.is_empty() && !st.orq {
            archive = Some(st.gid.clone());
            clears.push(m);
        } else {
            writes.push((m, st));
        }
    }
    (clears, writes, archive)
}
