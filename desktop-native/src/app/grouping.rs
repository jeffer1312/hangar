//! Grupos de sessões como o web (Sidebar + dragToGroup + GrupoDropDialog): blocos na lista por `pair_gid`, arrastar uma
//! sessão sobre outra (ou sobre o cabeçalho do bloco) e soltar no fundo da lista para sair. O gesto nunca agrupa direto: abre
//! o diálogo, porque agrupar funde os grupos inteiros e sair avisa quem ficou, sem desfazer. O menu da sessão chega ao mesmo
//! diálogo por "Agrupar com…" e "Sair do grupo".
use super::*;
use super::activity::web;
use super::sidebar::{SidebarReply, Target};
use gpui_kit::base::AccordionTrigger;
use gpui_kit::component::{WindowExt, menu::PopupMenu};
use crate::api::dto::PairExternalEntry;
use std::{cell::RefCell, rc::Rc};

/// Por que soltar a origem sobre o alvo não agrupa (`DropReason` do web).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal { Same, Orq, Dead, Invite, SameGroup, NoIdentity, OtherOwner }

impl Refusal {
    pub(super) fn text(self) -> String {
        web(match self {
            Refusal::Invite => "grupo_recusa_convite",
            Refusal::Same => "grupo_recusa_same",
            Refusal::Orq => "grupo_recusa_orq",
            Refusal::Dead => "grupo_recusa_dead",
            Refusal::SameGroup => "grupo_recusa_same_group",
            Refusal::NoIdentity => "grupo_recusa_sem_identificador",
            Refusal::OtherOwner => "grupo_recusa_outra_dona",
        })
    }
}

// Modelo global de grupos: cópia de `packages/core/src/pairGroups.ts` e do `canPair`, com os casos das duas em
// `packages/core/src/pairGroups.fixtures.json`.

/// Uma linha da lista com a chave da máquina dela no app; `invite` = máquina de convite.
#[derive(Clone, Copy)]
pub(super) struct Row<'a> { pub server: &'a str, pub session: &'a SessionInfo, pub invite: bool }

/// Membro de um grupo: sessão numa máquina conectada aqui, citada numa máquina sem conexão aqui (só texto), ou de fora.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize), serde(tag = "kind", rename_all = "lowercase"))]
pub(super) enum Member { Here { server: String, name: String }, Missing { server_id: String, name: String }, Outside { alias: String, session: String } }

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(super) struct GlobalGroup { pub key: String, pub label: String, pub members: Vec<Member> }

/// Identificador da máquina do app. Ausente, vazio ou igual ao de outra máquina não vale: não se adivinha.
pub(super) fn identity_of<'a>(server: &str, ids: &'a HashMap<String, String>) -> Option<&'a str> {
    let id = ids.get(server).filter(|id| !id.is_empty())?;
    (!ids.iter().any(|(other, value)| other != server && value == id)).then_some(id.as_str())
}

fn owned(s: &SessionInfo) -> Option<&str> { s.pair_owner.as_deref().filter(|o| !o.is_empty()) }

/// Chave do grupo, fora o par antigo: dona + gid entre máquinas, máquina + gid no grupo de uma só.
pub(super) fn group_key(row: Row) -> Option<String> {
    let gid = row.session.pair_gid.as_deref().filter(|g| !g.is_empty())?;
    Some(match owned(row.session) { Some(owner) => format!("fed:{owner}/{gid}"), None => format!("local:{}/{gid}", row.server) })
}

/// Par 1:1 antigo entre máquinas (sem dona): cada uma é o único par da outra, pelo identificador.
pub(super) fn legacy_pair(a: Row, b: Row, ids: &HashMap<String, String>) -> bool {
    let points = |from: Row, to: Row| {
        let peers = from.session.peers();
        owned(from.session).is_none() && peers.len() == 1 && from.server != to.server
            && identity_of(to.server, ids).is_some_and(|id| peers[0] == format!("{id}::{}", to.session.name))
    };
    points(a, b) && points(b, a)
}

/// Grupos de todas as máquinas do app. Membro citado numa máquina sem conexão aqui vira `Missing`; com alguma máquina do
/// app sem identificador válido, quem não aparece fica de fora em vez de virar `Missing` por palpite.
// ponytail: buscas quadráticas no número de linhas; a lista de sessões é curta.
pub(super) fn global_groups(rows: &[Row], ids: &HashMap<String, String>) -> Vec<GlobalGroup> {
    let servers: HashSet<&str> = ids.keys().map(String::as_str).chain(rows.iter().map(|r| r.server)).collect();
    let mut known: HashMap<&str, &str> = HashMap::new();
    let mut unresolved = false;
    for server in servers {
        match identity_of(server, ids) { Some(id) => { known.insert(id, server); } None => unresolved = true }
    }
    // Posição da única linha com esta máquina e nome; repetida não vale.
    let unique = |server: &str, name: &str| {
        let mut hits = rows.iter().enumerate().filter(|(_, r)| r.server == server && r.session.name == name).map(|(i, _)| i);
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    };

    let mut keys: Vec<Option<String>> = vec![None; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        if keys[i].is_some() { continue; }
        let peers = row.session.peers();
        let peer = (peers.len() == 1).then(|| peers[0].split_once("::")).flatten()
            .and_then(|(id, name)| unique(known.get(id)?, name));
        if let Some(j) = peer.filter(|&j| unique(row.server, &row.session.name) == Some(i) && legacy_pair(*row, rows[j], ids)) {
            let mut pair = [format!("{}::{}", row.server, row.session.name), format!("{}::{}", rows[j].server, rows[j].session.name)];
            pair.sort();
            let key = format!("remote:{}", serde_json::to_string(&pair).unwrap_or_default());
            keys[j] = Some(key.clone());
            keys[i] = Some(key);
            continue;
        }
        keys[i] = group_key(*row);
    }

    let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
    for (row, key) in rows.iter().zip(keys) {
        let Some(key) = key else { continue };
        match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, members)) => members.push(*row), None => groups.push((key, vec![*row])) }
    }
    groups.into_iter().map(|(key, members)| {
        let mut outside: Vec<(String, Member)> = Vec::new();
        for m in &members {
            for p in m.session.outside_pairs().iter() {
                let address = format!("{}::{}", p.alias, p.session);
                if !outside.iter().any(|(a, _)| *a == address) {
                    outside.push((address, Member::Outside { alias: p.alias.clone(), session: p.session.clone() }));
                }
            }
        }
        let mut missing: Vec<(&str, Member)> = Vec::new();
        for m in members.iter().filter(|_| !unresolved) {
            for address in m.session.peers() {
                let Some((id, name)) = address.split_once("::") else { continue };
                if id.is_empty() || name.is_empty() || known.contains_key(id) || outside.iter().any(|(a, _)| a == address)
                    || missing.iter().any(|(a, _)| a == address) { continue; }
                missing.push((address, Member::Missing { server_id: id.to_owned(), name: name.to_owned() }));
            }
        }
        let label = members.iter().filter_map(|m| m.session.pair_task.as_deref().map(str::trim)).find(|t| !t.is_empty()).map(str::to_owned)
            .unwrap_or_else(|| members.iter().map(|m| m.session.name.as_str()).collect::<Vec<_>>().join(", "));
        let here = members.iter().map(|m| Member::Here { server: m.server.to_owned(), name: m.session.name.clone() });
        GlobalGroup { key, label, members: here.chain(missing.into_iter().map(|(_, m)| m)).chain(outside.into_iter().map(|(_, m)| m)).collect() }
    }).collect()
}

/// Sessão local que passa a ter o par `alias` com acesso completo ao entrar no grupo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Grant { pub session: String, pub alias: String, pub owner: String }

/// Juntar `side` (uma sessão e seus membros locais) a um grupo cujo par de fora tem acesso completo: cada sessão que chega
/// fica aberta para ele, e a tela confirma antes. Pendente fica de fora: só o "Liberar" abre acesso completo.
pub(super) fn full_grants(group: &SessionInfo, side: &SessionInfo) -> Vec<Grant> {
    let full: Vec<PairExternalEntry> = group.outside_pairs().iter().filter(|p| p.level == "full" && !p.pending).cloned().collect();
    let mine = side.outside_pairs();
    let names = std::iter::once(side.name.clone()).chain(side.peers().iter().filter(|p| !p.contains("::")).cloned());
    names.flat_map(|session| full.iter().filter(|p| !mine.iter().any(|m| m.alias == p.alias && m.session == p.session))
        .map(move |p| Grant { session: session.clone(), alias: p.alias.clone(), owner: p.owner.clone() })).collect()
}

/// Selo do membro de grupo que atravessa máquinas: a chave do ticket da tarefa, senão quantas máquinas. Grupo de uma
/// máquina só não tem selo; membro de fora não conta máquina.
pub(super) fn seal_text(group: &GlobalGroup) -> Option<String> {
    let mut machines: Vec<&str> = Vec::new();
    for m in &group.members {
        let machine = match m { Member::Here { server, .. } => server, Member::Missing { server_id, .. } => server_id, Member::Outside { .. } => continue };
        if !machines.contains(&machine.as_str()) { machines.push(machine); }
    }
    if machines.len() < 2 { return None; }
    let (code, rest) = split_code(&group.label);
    // Sem chave de ticket o `split_code` devolve o rótulo inteiro: aí vale a contagem.
    Some(if rest.is_empty() && code == group.label { tr_shared("grupo_selo_maquinas", &[("n", &machines.len().to_string())]) } else { code.to_owned() })
}

/// Nome da sessão de `from` como o servidor `to` o entende: o nome sozinho na mesma máquina, `<identificador>::<nome>` na
/// outra. `None`: a máquina de origem não tem identificador válido.
pub(super) fn qualified(from: &str, to: &str, name: &str, ids: &HashMap<String, String>) -> Option<String> {
    if from == to { return Some(name.to_owned()); }
    identity_of(from, ids).map(|id| format!("{id}::{name}"))
}

/// Endereço de membro escrito do ponto de vista da máquina `from` (a lista dela) passado para o de `to`: o que é de `to`
/// perde o prefixo, o que era sem prefixo (de `from`) ganha o de `from`.
pub(super) fn rebased(from: &str, to: &str, address: &str, ids: &HashMap<String, String>) -> String {
    if from == to { return address.to_owned(); }
    match address.split_once("::") {
        Some((id, name)) if identity_of(to, ids) == Some(id) => name.to_owned(),
        Some(_) => address.to_owned(),
        None => qualified(from, to, address, ids).unwrap_or_else(|| address.to_owned()),
    }
}

/// Cabeçalho do grupo: "tarefa ↔ máquina::sessão" para os membros de máquinas sem conexão aqui e "apelido::sessão (dono)"
/// para cada par de fora.
pub(super) fn pair_header_label(label: &str, elsewhere: &[String], pairs: &[PairExternalEntry]) -> String {
    let mut out = label.to_owned();
    if !elsewhere.is_empty() {
        out.push_str(" ↔ ");
        out.push_str(&elsewhere.join(", "));
        if !pairs.is_empty() { out.push_str(", "); }
    }
    for (i, p) in pairs.iter().enumerate() {
        let one = tr_shared("pair_header", &[("local", ""), ("alias", &p.alias), ("sessao", &p.session), ("dono", &p.owner)]);
        // A frase começa com "{local} ↔ "; o que vem depois da primeira seta entra depois de vírgula.
        out.push_str(if i == 0 && elsewhere.is_empty() { &one } else { one.trim_start_matches(|c: char| c == ' ' || c == '↔') });
        if i + 1 < pairs.len() { out.push_str(", "); }
    }
    out
}

/// Pede o grupo e, aceito, abre cada sessão nova para o par de fora com acesso completo; o que não abriu vira aviso.
pub(super) async fn pair_and_grant(api: Api, target: String, peers: Vec<String>, task: String, replace: bool, grants: Vec<Grant>)
    -> Result<PairResult, Failure> {
    let mut result = api.pair(&target, &peers, &task, replace).await?;
    let mut failed = Vec::new();
    for g in grants {
        if let Err(error) = api.pair_access(&g.session, &g.alias, crate::api::PairLevel::Full).await {
            failed.push(tr_shared("native_pair_release_failed", &[("sessao", &g.session), ("dono", &g.owner), ("erro", &Hangar::fetch_failure(&error))]));
        }
    }
    if !failed.is_empty() { result.warning = Some(result.warning.into_iter().chain(failed).collect::<Vec<_>>().join("\n")); }
    Ok(result)
}

/// `canPair` do web, na mesma ordem. Membro de fora não impede.
/// Candidatas a entrar no grupo de `origin`: as que o arrastar aceitaria em `machines`, a da origem primeiro, por nome,
/// sem as que estão sendo apagadas.
pub(super) fn candidates<'a>(origin: Row, mut machines: Vec<String>, sessions: impl Fn(&str) -> &'a [SessionInfo],
    hidden: impl Fn(&str, &str) -> bool, ids: &HashMap<String, String>) -> Vec<(Target, &'a SessionInfo)> {
    machines.sort_by_key(|k| k != origin.server);
    machines.iter().flat_map(|key| {
        let mut list: Vec<&SessionInfo> = sessions(key).iter()
            .filter(|s| !hidden(key, &s.name) && can_pair(origin, Row { server: key, session: s, invite: false }, ids).is_ok()).collect();
        list.sort_by_cached_key(|s| s.name.to_lowercase());
        list.into_iter().map(|s| (Target::new(key, &s.name), s)).collect::<Vec<_>>()
    }).collect()
}

pub(super) fn can_pair(origin: Row, target: Row, ids: &HashMap<String, String>) -> Result<(), Refusal> {
    let (o, t) = (origin.session, target.session);
    let same_server = origin.server == target.server;
    if same_server && o.name == t.name { return Err(Refusal::Same); }
    // O grupo da orquestração é montado pelo orq; agrupar a linha dele à mão desfaz a execução.
    if o.orq() || t.orq() { return Err(Refusal::Orq); }
    if o.state == "dead" || t.state == "dead" { return Err(Refusal::Dead); }
    // Parear é rota do servidor inteiro do dono.
    if origin.invite || target.invite { return Err(Refusal::Invite); }
    if group_key(origin).is_some_and(|k| Some(k) == group_key(target)) || legacy_pair(origin, target, ids) { return Err(Refusal::SameGroup); }
    // O pedido leva `<identificador>::<nome>` da outra máquina.
    if !same_server && (identity_of(origin.server, ids).is_none() || identity_of(target.server, ids).is_none()) {
        return Err(Refusal::NoIdentity);
    }
    // Só grupos da mesma dona se fundem; o grupo de uma máquina só tem ela como dona.
    let grouped = |s: &SessionInfo| s.pair_gid.as_deref().is_some_and(|g| !g.is_empty());
    if grouped(o) && grouped(t) && !(same_server && owned(o).is_none() && owned(t).is_none()) {
        match (owner(origin, ids), owner(target, ids)) {
            (Some(a), Some(b)) if a != b => return Err(Refusal::OtherOwner),
            (Some(_), Some(_)) => {}
            _ => return Err(Refusal::NoIdentity),
        }
    }
    Ok(())
}

fn owner<'a>(row: Row<'a>, ids: &'a HashMap<String, String>) -> Option<&'a str> { owned(row.session).or_else(|| identity_of(row.server, ids)) }

/// `canLeave` do web: os pares também contam, para o par de outro servidor que venha sem `pair_gid` (o externo tem um).
pub(super) fn can_leave(s: &SessionInfo) -> bool { !s.orq() && (s.pair_gid.is_some() || !s.peers().is_empty()) }

pub(super) enum ListRow<'a> {
    /// `key`: a chave do bloco recolhido, a mesma para os membros de todas as máquinas.
    Header { key: String, gid: String, label: String, members: Vec<Row<'a>> },
    /// `block`: a chave do bloco em que a linha está.
    Session { row: Row<'a>, block: Option<String> },
    /// A sessão da outra pessoa, logo abaixo dos membros do grupo; uma por par de fora. `own`: a máquina do membro que tem o par.
    External { block: String, gid: String, own: &'a str, pair: PairExternalEntry },
}

/// `clusterByPair` do web, entre máquinas: `sections` são as seções da barra na ordem da tela. Cada grupo do modelo global
/// vira um bloco só, com o cabeçalho no lugar do primeiro membro seguido de todos os membros (de qualquer máquina e seção),
/// que saem de onde estariam. O rótulo é a tarefa ou os nomes; só membro de máquina sem conexão aqui entra depois da seta.
// ponytail: busca dos membros quadrática no número de linhas; a lista de sessões é curta.
pub(super) fn cluster<'a>(sections: &[Vec<Row<'a>>], ids: &HashMap<String, String>) -> Vec<Vec<ListRow<'a>>> {
    let all: Vec<Row<'a>> = sections.iter().flatten().copied().collect();
    let groups = global_groups(&all, ids);
    let of: Vec<Option<usize>> = all.iter().map(|r| groups.iter().position(|g| g.members.iter()
        .any(|m| matches!(m, Member::Here { server, name } if server == r.server && *name == r.session.name)))).collect();
    let mut done = vec![false; groups.len()];
    let mut at = 0;
    sections.iter().map(|section| {
        let mut out = Vec::new();
        for row in section {
            at += 1;
            let Some(g) = of[at - 1] else { out.push(ListRow::Session { row: *row, block: None }); continue };
            if std::mem::replace(&mut done[g], true) { continue; }
            let group = &groups[g];
            let block = format!("pair:{}", group.key);
            let members: Vec<Row<'a>> = all.iter().zip(&of).filter(|(_, o)| **o == Some(g)).map(|(r, _)| *r).collect();
            let gid = members.iter().find_map(|m| m.session.pair_gid.clone().filter(|g| !g.is_empty())).unwrap_or_else(|| group.key.clone());
            let mut outside: Vec<(&'a str, PairExternalEntry)> = Vec::new();
            for m in &members {
                for p in m.session.outside_pairs().iter() {
                    if !outside.iter().any(|(_, o)| o.alias == p.alias && o.session == p.session) { outside.push((m.server, p.clone())); }
                }
            }
            let missing: Vec<String> = group.members.iter().filter_map(|m| match m {
                Member::Missing { server_id, name } => Some(format!("{server_id}::{name}")), _ => None }).collect();
            let pairs: Vec<PairExternalEntry> = outside.iter().map(|(_, p)| p.clone()).collect();
            out.push(ListRow::Header { key: block.clone(), gid: gid.clone(), label: pair_header_label(&group.label, &missing, &pairs), members: members.clone() });
            out.extend(members.into_iter().map(|row| ListRow::Session { row, block: Some(block.clone()) }));
            out.extend(outside.into_iter().map(|(own, pair)| ListRow::External { block: block.clone(), gid: gid.clone(), own, pair }));
        }
        out
    }).collect()
}

/// `pairCodigo`/`pairResto` do web (`^([A-Za-z][\w.]*-\d+)\b\s*(.*)$`): a chave do ticket em destaque e o assunto em cinza.
pub(super) fn split_code(label: &str) -> (&str, &str) {
    let code = || -> Option<usize> {
        if !label.starts_with(|c: char| c.is_ascii_alphabetic()) { return None; }
        let dash = label.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))?;
        if !label[dash..].starts_with('-') { return None; }
        let end = label[dash + 1..].find(|c: char| !c.is_ascii_digit()).map_or(label.len(), |n| dash + 1 + n);
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        (end > dash + 1 && !label[end..].starts_with(word)).then_some(end)
    };
    match code() { Some(end) => (&label[..end], label[end..].trim_start()), None => (label, "") }
}

/// O que anda com o arrasto: a sessão (máquina + nome).
#[derive(Clone)]
pub(super) struct SessionDrag { target: Target }

/// Onde o ponteiro está durante o arrasto. `Pair` é o cabeçalho do bloco, que vale pelo primeiro membro (agrupar funde o grupo
/// inteiro). Alvo de outra máquina vale como o da mesma: quem recusa é o `can_pair`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DropSpot { Row(Target), Pair { gid: String, rep: Target } }

enum DropAction { Join(Target), Leave }

/// O pedido de entrar vai ao servidor do alvo, com a origem pelo nome que ele entende (`qualified`).
enum AskMode { Join { origin: Target, target: Target }, Leave { origin: Target } }

/// O que o diálogo mostra. Mora fora do `Hangar` porque o `open_dialog` desenha na hora, com o `Hangar` em atualização; a
/// lista nova de sessões refaz os campos vivos (`refresh_group_ask`).
#[derive(Default)]
struct AskView {
    join: bool,
    affected: Vec<String>,
    /// Entrando num grupo que já existe: a tarefa dele (vazia se não tem). `None` mostra o campo da tarefa.
    inherited: Option<String>,
    block: Option<String>,
    busy: bool,
    suggesting: bool,
    error: Option<String>,
    /// 409 do servidor: o grupo já tem outra tarefa, e o botão passa a ser "Substituir a tarefa".
    conflict: Option<String>,
    /// Feito, mas o aviso não chegou a alguém: resta fechar.
    done: bool,
    /// Entrando: sessões que ficam abertas com acesso completo para um par de fora; confirmar libera.
    grants: Vec<Grant>,
    /// Saindo: donos dos pares que esta sessão segura; sair desfaz o par para o grupo inteiro.
    anchors: Vec<String>,
}

struct GroupAsk { mode: AskMode, seq: u64, task: Entity<InputState>, view: Rc<RefCell<AskView>>, _events: Subscription }

pub(super) enum GroupReply { Paired(Result<PairResult, Failure>), Left(Result<PairResult, Failure>), Suggested(Result<String, Failure>) }

#[derive(Default)]
pub(super) struct Grouping {
    /// Sessão arrastada agora; só vale com um arrasto em curso (`has_active_drag`).
    origin: Option<Target>,
    spot: Option<DropSpot>,
    /// Ponteiro dentro da área visível da lista: linha rolada para fora ainda tem caixa, e só isto a separa da visível.
    in_list: bool,
    ask: Option<GroupAsk>,
    pub(super) seq: u64,
    /// Painel do grupo aberto pelo chip do compositor.
    pub(super) sheet: Option<super::group_sheet::Sheet>,
    /// "Mandar pro grupo" ligado nesta sessão, com os membros de quando ligou: grupo que muda desliga, como no web.
    pub(super) send_to_group: Option<(SessionKey, String)>,
    /// Identificador de cada máquina (`/api/peers/identificador`), lido uma vez por máquina.
    ids: HashMap<String, IdSlot>,
    /// Selos da última lista: a barra se desenha a cada quadro e o modelo global é quadrático.
    seals: RefCell<Option<(u64, Rc<Seals>)>>,
    /// Faixas das abas, pelo mesmo motivo dos selos.
    pub(super) tabs: RefCell<Option<(u64, Rc<Vec<super::session_tabs::TabGroup>>)>>,
    /// O que soltar no ponto atual faria; refeito só quando o ponto muda, não por linha a cada quadro.
    verdict: Option<Result<DropAction, Refusal>>,
}

pub(super) type Seals = HashMap<Target, (String, String)>;

/// Leitura do identificador de uma máquina; identificador vazio vira `Failed` (`id_slot`).
#[derive(Clone)]
pub(super) enum IdSlot { Loading, Known(String), Failed(Instant) }

/// Falha na leitura do identificador espera isto antes de tentar de novo, como o web.
const ID_RETRY: Duration = Duration::from_secs(30);

/// A máquina tem identificador válido: em voo, falha, vazio ou não lido ainda não dizem nada sobre quem está fora.
pub(super) fn identity_settled(slot: Option<&IdSlot>) -> bool { matches!(slot, Some(IdSlot::Known(_))) }

/// Resposta de `/api/peers/identificador` (`None` = a leitura falhou). Vazio é máquina sem `CP_SERVER_ID`: conta como falha
/// e é relido depois de `ID_RETRY`, para o conserto aparecer sem reconectar.
pub(super) fn id_slot(read: Option<&Value>) -> IdSlot {
    match read.and_then(|v| v.get("identificador")).and_then(Value::as_str).filter(|id| !id.is_empty()) {
        Some(id) => IdSlot::Known(id.to_owned()),
        None => IdSlot::Failed(Instant::now()),
    }
}

/// Valor guardado pela chave da entrada; só recalcula quando ela muda.
pub(super) fn memo<T>(cell: &RefCell<Option<(u64, Rc<T>)>>, key: u64, compute: impl FnOnce() -> T) -> Rc<T> {
    if let Some((k, value)) = cell.borrow().as_ref() && *k == key { return value.clone(); }
    let value = Rc::new(compute());
    *cell.borrow_mut() = Some((key, value.clone()));
    value
}

impl Grouping {
    pub(super) fn reset(&mut self) {
        let seq = self.seq + 1;
        *self = Self { seq, ..Self::default() };
    }
}

/// A sessão presa ao ponteiro; embaixo, o motivo da recusa ou o aviso de saída, como o `title` do alvo no web.
struct DragChip { name: String, hangar: WeakEntity<Hangar> }

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = self.hangar.upgrade().and_then(|hangar| hangar.read(cx).drop_hint(cx));
        div().max_w(px(280.)).px_3().py(px(6.)).flex().flex_col().gap(px(2.)).rounded(px(8.)).border_1().border_color(theme::border_strong())
            .bg(theme::raised()).shadow(theme::popover_shadow()).text_sm()
            .child(div().flex().items_center().gap(px(6.)).child(glyph(13., theme::accent()))
                .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(self.name.clone())))
            .when_some(hint, |el, (text, refused)| el.child(div().text_xs().whitespace_normal()
                .text_color(if refused { theme::danger() } else { theme::muted() }).child(text)))
    }
}

pub(super) fn glyph(size: f32, color: Hsla) -> Svg { svg().path(crate::GROUP_GLYPH).size(px(size)).flex_shrink_0().text_color(color) }

pub(super) fn web_with(key: &str, name: &str, value: &str) -> String {
    crate::i18n::tr_web(key, &HashMap::from([(name.to_owned(), value.to_owned())])).unwrap_or_else(|| key.to_owned())
}

impl Hangar {
    /// Identificador de cada máquina do app já lido (`/api/peers/identificador`).
    pub(super) fn identities(&self) -> HashMap<String, String> {
        self.sidebar.grouping.ids.iter().filter_map(|(k, slot)| match slot { IdSlot::Known(id) => Some((k.clone(), id.clone())), _ => None }).collect()
    }

    /// Os identificadores lidos na chave de um `memo`, sem copiar o mapa.
    pub(super) fn hash_identities(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        let mut known: Vec<(&String, &String)> = self.sidebar.grouping.ids.iter()
            .filter_map(|(k, slot)| match slot { IdSlot::Known(id) => Some((k, id)), _ => None }).collect();
        known.sort();
        known.hash(hasher);
    }

    /// Leitura do identificador de `key`; `None` antes de qualquer leitura.
    pub(super) fn identity_slot(&self, key: &str) -> Option<&IdSlot> { self.sidebar.grouping.ids.get(key) }

    /// Máquinas do dono com lista na barra (sem as de convite): as que o grupo entre máquinas alcança.
    pub(super) fn group_machines(&self) -> Vec<String> {
        let active = self.active_key();
        let mut keys: Vec<String> = (!active.is_empty()).then_some(active).into_iter().collect();
        for key in self.servers.iter().map(|s| servers::norm(&s.address)) {
            if self.remote.contains_key(&key) && !keys.contains(&key) { keys.push(key); }
        }
        keys.retain(|k| self.server_entry(k).is_none_or(|s| !s.invite));
        keys
    }

    /// Com mais de uma máquina (ou grupo citando outra), lê o identificador de cada uma que ainda não tem; falha tenta de novo depois de `ID_RETRY`,
    /// na próxima lista que chegar. Uma leitura por máquina, nunca por linha nem por quadro.
    pub(super) fn ensure_identities(&mut self) {
        // Com uma máquina só, o identificador só importa se algum grupo cita outra (membro `máquina::nome`).
        if !self.multi_server() && !self.sessions.iter().any(|s| s.peers().iter().any(|p| p.contains("::"))) { return; }
        for key in self.group_machines() {
            let due = match self.sidebar.grouping.ids.get(&key) { None => true, Some(IdSlot::Failed(at)) => at.elapsed() >= ID_RETRY, _ => false };
            let Some(api) = due.then(|| self.machine_api(&key)).flatten() else { continue };
            self.sidebar.grouping.ids.insert(key.clone(), IdSlot::Loading);
            let tell = self.sidebar_tell();
            self.runtime.spawn(async move {
                let result = api.server_read(&["peers", "identificador"], &[], 15).await;
                tell.send(SidebarReply::Identity(key, result)).await;
            });
        }
    }

    pub(super) fn receive_identity(&mut self, key: String, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        let slot = match result {
            Ok(v) => id_slot(Some(&v)),
            Err(error) => { eprintln!("identificador de {key} não lido: {}", Self::fetch_failure(&error)); id_slot(None) }
        };
        self.sidebar.grouping.ids.insert(key, slot);
        self.refresh_group_ask();
        self.refresh_group_sheet(window, cx);
        cx.notify();
    }

    /// Selo de cada membro de grupo que atravessa máquinas: o texto (`seal_text`) e a dica com a tarefa e os membros.
    /// Recalculado só quando muda o que ele lê (campos de grupo das linhas, identificadores, nomes das máquinas, idioma).
    pub(super) fn group_seals(&self, cx: &App) -> Rc<Seals> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let machines = self.group_machines();
        if machines.len() < 2 && self.sessions.iter().all(|s| s.pair_owner.is_none()) { return Rc::default(); }
        let mut hasher = DefaultHasher::new();
        crate::i18n::english().hash(&mut hasher);
        self.hash_identities(&mut hasher);
        for key in &machines {
            (key, self.machine_label(key, cx)).hash(&mut hasher);
            for s in self.sessions_of(key) {
                (&s.name, &s.pair_gid, &s.pair_owner, s.peers(), &s.pair_task).hash(&mut hasher);
                for p in s.outside_pairs().iter() { (&p.alias, &p.session).hash(&mut hasher); }
            }
        }
        memo(&self.sidebar.grouping.seals, hasher.finish(), || self.compute_seals(&machines, &self.identities(), cx))
    }

    fn compute_seals(&self, machines: &[String], ids: &HashMap<String, String>, cx: &App) -> Seals {
        let rows: Vec<Row> = machines.iter().flat_map(|key| self.sessions_of(key).iter().map(move |s| Row { server: key, session: s, invite: false })).collect();
        let mut out = HashMap::new();
        for g in global_groups(&rows, ids) {
            let Some(text) = seal_text(&g) else { continue };
            let names: Vec<String> = g.members.iter().map(|m| match m {
                Member::Here { server, name } => format!("{}::{name}", self.machine_label(server, cx)),
                Member::Missing { server_id, name } => format!("{server_id}::{name}"),
                Member::Outside { alias, session } => format!("{alias}::{session}"),
            }).collect();
            let tip = format!("{} · {}", web_with("sessao_grupo_pareado", "label", &g.label), names.join(", "));
            for m in &g.members {
                if let Member::Here { server, name } = m { out.insert(Target::new(server, name), (text.clone(), tip.clone())); }
            }
        }
        out
    }

    /// O selo na linha: glifo do grupo e o texto; clicar abre o painel do grupo desta sessão. Fora da ordem do Tab: pelo
    /// teclado o painel abre pelo chip do compositor da sessão aberta.
    pub(super) fn render_seal(&self, target: &Target, (text, tip): (String, String), cx: &mut Context<Self>) -> Button {
        let open = target.clone();
        Button::new(SharedString::from(format!("group-seal-{}", target.id()))).ghost().xsmall().tab_stop(false)
            .h(px(17.)).px(px(4.)).rounded(px(5.)).flex_shrink_0()
            .accessibility_label(tip.clone()).tooltip(tip)
            .child(div().flex().items_center().gap(px(3.)).text_size(px(11.)).text_color(theme::muted())
                .child(glyph(11., theme::accent())).child(div().max_w(px(96.)).truncate().child(text)))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_group_sheet_on(open.clone(), window, cx);
            }))
    }

    /// A linha da sessão para o modelo de grupos.
    fn pair_row<'a>(&'a self, target: &'a Target) -> Option<Row<'a>> {
        Some(Row { server: &target.server, session: self.target_session(target)?, invite: self.invite_target(target) })
    }

    /// Candidatas do "Agrupar com…": as que o arrastar aceitaria, de todas as máquinas do dono, a da origem primeiro, com o
    /// texto do item (o nome; de outra máquina, "nome · máquina").
    pub(super) fn group_candidates(&self, origin: &Target, cx: &App) -> Vec<(Target, String)> {
        let Some(row) = self.pair_row(origin) else { return Vec::new() };
        candidates(row, self.group_machines(), |k| self.sessions_of(k), |k, n| self.sidebar.is_hidden(k, n), &self.identities()).into_iter()
            .map(|(t, s)| {
                let text = if t.server == origin.server { s.name.clone() } else { format!("{} · {}", s.name, self.machine_label(&t.server, cx)) };
                (t, text)
            }).collect()
    }

    // ── Arrastar ──

    fn dragging<'a>(&'a self, cx: &App) -> Option<&'a Target> { self.sidebar.grouping.origin.as_ref().filter(|_| cx.has_active_drag()) }

    /// `None`: ponteiro fora da lista. `Some(None)`: no fundo dela.
    fn current_spot(&self) -> Option<Option<&DropSpot>> {
        let g = &self.sidebar.grouping;
        g.in_list.then_some(g.spot.as_ref())
    }

    /// O que soltar ali faria, pela sessão viva: `None` é nada (fundo sem grupo, sessão sumida).
    fn verdict(&self, origin: &Target, spot: Option<&DropSpot>) -> Option<Result<DropAction, Refusal>> {
        let row = self.pair_row(origin)?;
        match spot {
            // Convidado não desagrupa: parear é rota do servidor inteiro do dono.
            None if row.invite => Some(Err(Refusal::Invite)),
            None => can_leave(row.session).then_some(Ok(DropAction::Leave)),
            Some(DropSpot::Row(target) | DropSpot::Pair { rep: target, .. }) => {
                Some(can_pair(row, self.pair_row(target)?, &self.identities()).map(|_| DropAction::Join(target.clone())))
            }
        }
    }

    fn drop_hint(&self, cx: &App) -> Option<(String, bool)> {
        self.dragging(cx)?;
        match self.sidebar.grouping.verdict.as_ref()? {
            Ok(DropAction::Leave) => Some((tr("group_drop_leave"), false)),
            Ok(DropAction::Join(_)) => None,
            Err(refusal) => Some((refusal.text(), true)),
        }
    }

    /// Este alvo está sob o ponteiro: `Some(true)` aceita, `Some(false)` recusa.
    fn targeted(&self, spot: &DropSpot, cx: &App) -> Option<bool> {
        self.dragging(cx)?;
        if self.current_spot()? != Some(spot) { return None; }
        Some(self.sidebar.grouping.verdict.as_ref()?.is_ok())
    }

    fn refresh_verdict(&mut self) {
        let g = &self.sidebar.grouping;
        let verdict = g.origin.as_ref().zip(self.current_spot()).and_then(|(origin, spot)| self.verdict(origin, spot));
        self.sidebar.grouping.verdict = verdict;
    }

    fn start_session_drag(&mut self, target: Target, cx: &mut Context<Self>) {
        self.row_release(); // senão o pressionar longo abre o renomear no meio do arrasto
        self.clear_hover();
        let g = &mut self.sidebar.grouping;
        (g.origin, g.spot, g.in_list) = (Some(target), None, true);
        self.refresh_verdict();
        cx.notify();
    }

    /// Cada alvo diz se o ponteiro está dentro dele; a lista diz se ele está na parte visível.
    fn drag_moved(&mut self, spot: Option<DropSpot>, inside: bool, cx: &mut Context<Self>) {
        let g = &mut self.sidebar.grouping;
        let before = (g.spot.clone(), g.in_list);
        match spot {
            None => g.in_list = inside,
            Some(spot) if inside => g.spot = Some(spot),
            Some(spot) => if g.spot.as_ref() == Some(&spot) { g.spot = None },
        }
        if before != (g.spot.clone(), g.in_list) {
            self.refresh_verdict();
            cx.notify();
        }
    }

    pub(super) fn end_session_drag(&mut self, cx: &mut Context<Self>) {
        let g = &mut self.sidebar.grouping;
        (g.origin, g.spot, g.in_list, g.verdict) = (None, None, false, None);
        cx.notify();
    }

    /// Soltou: confere de novo pela sessão viva e abre o pedido; recusado não faz nada.
    fn drop_session(&mut self, origin: Target, spot: Option<DropSpot>, window: &mut Window, cx: &mut Context<Self>) {
        let verdict = self.verdict(&origin, spot.as_ref());
        self.end_session_drag(cx);
        match verdict {
            Some(Ok(DropAction::Join(target))) => self.request_group(origin, target, window, cx),
            Some(Ok(DropAction::Leave)) => self.request_leave(origin, window, cx),
            _ => {}
        }
    }

    /// Alvo de soltar: realce de aceite (contorno e fundo de destaque) ou de recusa (tracejado), como o web.
    fn drop_target<E: InteractiveElement + Styled + ParentElement + IntoElement>(&self, el: E, spot: DropSpot, radius: f32,
        cx: &mut Context<Self>) -> E {
        let state = self.targeted(&spot, cx);
        let moved = spot.clone();
        el.on_drag_move(cx.listener(move |this, event: &DragMoveEvent<SessionDrag>, _, cx| {
                this.drag_moved(Some(moved.clone()), event.bounds.contains(&event.event.position), cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &SessionDrag, window, cx| this.drop_session(dragged.target.clone(), Some(spot.clone()), window, cx)))
            .when_some(state, |el, ok| el.when(ok, |el| el.bg(theme::accent_dim()))
                .child(div().absolute().inset_0().rounded(px(radius)).border_2()
                    .map(|b| if ok { b.border_color(theme::accent()) } else { b.border_dashed().border_color(theme::muted()) })))
    }

    /// Fundo da lista: sabe se o ponteiro está nela e, soltando ali, pede a saída do grupo.
    pub(super) fn drop_background(&self, el: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        el.on_drag_move(cx.listener(|this, event: &DragMoveEvent<SessionDrag>, _, cx| {
                this.drag_moved(None, event.bounds.contains(&event.event.position), cx);
            }))
            .on_drop(cx.listener(|this, dragged: &SessionDrag, window, cx| this.drop_session(dragged.target.clone(), None, window, cx)))
    }

    /// Linha da lista, de qualquer máquina: arrastável, alvo de soltar e, no bloco, recuada com a faixa do grupo (âmbar
    /// quando espera resposta, como o web). `remote` é a chave da máquina quando ela não é a ativa.
    pub(super) fn group_row(&self, el: Stateful<Div>, session: &SessionInfo, remote: Option<&str>, radius: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let target = Target::new(&remote.map(str::to_owned).unwrap_or_else(|| self.active_key()), &session.name);
        let member = session.pair_gid.is_some();
        let lifted = self.dragging(cx) == Some(&target);
        let bar = if session.state == "awaiting_input" { theme::warning() } else { theme::accent() };
        let el = el.when(member, |el| el.ml(px(8.)).child(div().absolute().left_0().top(px(6.)).bottom(px(6.)).w(px(2.)).rounded_full().bg(bar)))
            .when(lifted, |el| el.opacity(0.45));
        let weak = cx.entity().downgrade();
        let el = el.on_drag(SessionDrag { target: target.clone() }, move |dragged, _, _, cx| {
            let _ = weak.update(cx, |this, cx| this.start_session_drag(dragged.target.clone(), cx));
            cx.new(|_| DragChip { name: dragged.target.name.clone(), hangar: weak.clone() })
        });
        self.drop_target(el, DropSpot::Row(target), radius, cx)
    }

    /// Cabeçalho do bloco (`.pair-head` do web): ▾, glifo, chave da tarefa em destaque e o resto em cinza, quantas esperam e o
    /// total. Clicar recolhe; soltar sobre ele agrupa com o bloco.
    pub(super) fn render_pair_header(&self, key: &str, gid: &str, label: &str, members: &[Row], window: &mut Window,
        cx: &mut Context<Self>) -> AnyElement {
        let key = key.to_owned();
        let open = !self.sidebar.is_collapsed(&key);
        let awaiting = members.iter().filter(|m| m.session.state == "awaiting_input").count();
        let (code, rest) = split_code(label);
        let id = SharedString::from(format!("pair-head-{key}"));
        let focus = window.use_keyed_state(SharedString::from(format!("{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        let title = web_with("sessao_grupo_pareado", "label", label);
        let tip = title.clone();
        // Sem membro o cabeçalho não é alvo de soltar: um `Target` de nome vazio não é sessão nenhuma.
        let spot = members.first().map(|rep| DropSpot::Pair { gid: gid.to_owned(), rep: Target::new(rep.server, &rep.session.name) });
        let pill = |text: String, color: Hsla, bg: Hsla| div().flex_shrink_0().min_w(px(18.)).px(px(6.)).rounded_full().bg(bg)
            .flex().justify_center().font_family(theme::MONO).text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(text);
        let weak = cx.entity().downgrade();
        let header = AccordionTrigger::new(id).open(open).track_focus(&focus).aria_label(format!("{title} · {}", members.len()))
            .relative().flex_shrink_0().mt(px(4.)).min_h(px(26.)).px(px(8.)).rounded(px(8.)).border_1().border_color(transparent_black())
            .flex().items_center().gap(px(6.)).cursor_pointer().hover(|el| el.bg(theme::hover())).focus_visible(|el| el.border_color(theme::accent_focus()))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint()))
            .child(glyph(13., theme::accent()))
            .child(div().flex_1().min_w_0().flex().items_center().gap(px(4.)).text_xs()
                .child(div().flex_shrink_0().max_w_full().truncate().font_weight(FontWeight::SEMIBOLD).text_color(theme::accent_text()).child(code.to_owned()))
                .when(!rest.is_empty(), |el| el.child(div().min_w_0().truncate().text_color(theme::muted()).child(rest.to_owned()))))
            .when(awaiting > 0, |el| el.child(pill(awaiting.to_string(), theme::warning(), theme::warning().opacity(0.14))))
            .child(pill(members.len().to_string(), theme::muted(), theme::inset()))
            .on_change(move |_, _, _, cx| { let _ = weak.update(cx, |this, cx| this.toggle_group(key.clone(), cx)); });
        match spot { Some(spot) => self.drop_target(header, spot, 8., cx).into_any_element(), None => header.into_any_element() }
    }

    // ── Diálogo ──

    /// `target` é outra sessão, desta ou de outra máquina do dono.
    pub(super) fn request_group(&mut self, origin: Target, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        // Tarefa que já existe entra no campo (a do alvo antes), como o web.
        let task = [&target, &origin].into_iter().filter_map(|t| self.target_session(t)?.pair_task.clone()).find(|t| !t.trim().is_empty());
        self.open_group_ask(AskMode::Join { origin, target }, task.unwrap_or_default(), window, cx);
    }

    pub(super) fn request_leave(&mut self, origin: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.open_group_ask(AskMode::Leave { origin }, String::new(), window, cx);
    }

    fn open_group_ask(&mut self, mode: AskMode, task: String, window: &mut Window, cx: &mut Context<Self>) {
        self.machines_tray.open = false;
        let origin = ask_origin(&mode).clone();
        let join = matches!(mode, AskMode::Join { .. });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(web("grupo_drop_tarefa")).default_value(task));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            // Enter faz o que o botão principal à vista faz.
            InputEvent::PressEnter { .. } => {
                let replace = this.sidebar.grouping.ask.as_ref().is_some_and(|a| a.view.borrow().conflict.is_some());
                this.confirm_group(replace, window, cx);
            }
            // Mexer na tarefa desfaz o "Substituir": o 409 era da tarefa anterior.
            InputEvent::Change => {
                if let Some(ask) = this.sidebar.grouping.ask.as_ref() { ask.view.borrow_mut().conflict = None; }
                cx.notify();
            }
            _ => {}
        });
        let g = &mut self.sidebar.grouping;
        g.seq += 1;
        let seq = g.seq;
        let view = Rc::new(RefCell::new(AskView { join, ..AskView::default() }));
        g.ask = Some(GroupAsk { mode, seq, task: input.clone(), view: view.clone(), _events: events });
        self.refresh_group_ask();
        if join && view.borrow().inherited.is_none() {
            let field = input.clone();
            cx.defer_in(window, move |_, window, cx| field.update(cx, |state, cx| state.focus(window, cx)));
        }
        // O diálogo devolve ao fechar o foco de quando abriu: o da linha de origem.
        self.focus_origin(&origin, window, cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let v = view.borrow();
            let title = web(match (v.join, v.done) {
                (true, false) => "grupo_drop_titulo", (true, true) => "grupo_drop_titulo_feito",
                (false, false) => "grupo_drop_sair_titulo", (false, true) => "grupo_drop_sair_titulo_feito",
            });
            let act = |id: &'static str, label: String, run: fn(&mut Hangar, &mut Window, &mut Context<Hangar>)| {
                let weak = weak.clone();
                Button::new(id).label(label).on_click(move |_, window, cx| { let _ = weak.update(cx, |this, cx| run(this, window, cx)); })
            };
            let affected = (!v.affected.is_empty()).then(|| div().flex().flex_col().gap(px(4.))
                .child(div().text_sm().text_color(theme::muted()).child(web("grupo_drop_afetadas")))
                .child(div().flex().flex_col().gap(px(2.)).text_sm()
                    .children(v.affected.iter().map(|name| div().min_w_0().truncate().font_family(theme::MONO).child(name.clone())))));
            let task_row = match (&v.inherited, v.join && !v.done) {
                (_, false) => None,
                (Some(task), true) => (!task.trim().is_empty()).then(|| div().text_sm().text_color(theme::muted()).whitespace_normal()
                    .child(web_with("grupo_drop_tarefa_herdada", "tarefa", task)).into_any_element()),
                (None, true) => Some(div().flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&input).disabled(v.busy || v.suggesting).aria_label(web("grupo_drop_tarefa"))))
                    .child(act("group-suggest", web(if v.suggesting { "grupo_drop_sugerindo" } else { "grupo_drop_sugerir" }), |this, _, cx| this.suggest_group_task(cx))
                        .loading(v.suggesting).disabled(v.busy || v.suggesting || v.block.is_some()))
                    .into_any_element()),
            };
            // Acesso completo é o terminal desta máquina: a frase fica à vista antes do confirmar, como o desfazer do par.
            let notices: Vec<String> = v.grants.iter().map(|g| tr_shared("pair_full_confirm", &[("sessao", &g.session), ("dono", &g.owner)]))
                .chain(v.anchors.iter().map(|dono| tr_shared("pair_undo_anchor_body", &[("dono", dono)]))).collect();
            let notices = (!notices.is_empty() && !v.done).then(|| div().p_2().rounded_md().bg(theme::warning().opacity(0.12)).flex().flex_col().gap(px(4.))
                .text_sm().whitespace_normal().children(notices));
            let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).text_sm().whitespace_normal().text_color(theme::danger()).child(text);
            let alerts = [
                v.error.clone().map(|t| alert("group-error", t)),
                v.conflict.clone().map(|t| alert("group-conflict", t)),
                v.block.clone().filter(|_| v.error.is_none() && v.conflict.is_none() && !v.busy).map(|t| alert("group-block", t)),
            ];
            let variant = if v.join { ButtonVariant::Primary } else { ButtonVariant::Danger };
            // Fechar o diálogo, confirmado ou cancelado, é o gesto do passo do tour.
            let learned = |weak: WeakEntity<Hangar>| move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.close_group_ask(window, cx));
            };
            let footer = div().flex().justify_end().gap_2().map(|el| if v.done {
                el.child(Button::new("group-close").label(web("sessao_fechar")).with_variant(variant).on_click(learned(weak.clone())))
            } else {
                let replace = v.join && v.conflict.is_some();
                let label = web(if replace { "grupo_drop_substituir_tarefa" } else if v.join { "grupo_drop_confirmar" } else { "grupo_drop_sair_confirmar" });
                let confirm = weak.clone();
                el.child(Button::new("group-cancel").label(tr("cancel")).disabled(v.busy).on_click(learned(weak.clone())))
                    .child(Button::new("group-ok").label(label).with_variant(variant).loading(v.busy).disabled(v.busy || v.suggesting || v.block.is_some())
                        .on_click(move |_, window, cx| { let _ = confirm.update(cx, |this, cx| this.confirm_group(replace, window, cx)); }))
            });
            let close = weak.clone();
            // Com a chamada em voo o diálogo não fecha: a resposta (aviso, 409, erro) cairia no vazio.
            popup::dialog(dialog).w(px(420.)).title(title).keyboard(!v.busy).overlay_closable(!v.busy).close_button(!v.busy)
                .child(div().flex().flex_col().gap(px(12.)).children(affected).children(notices).children(task_row).children(alerts.into_iter().flatten()))
                .footer(footer)
                .on_ok(super::machines::enter_to_focused)
                .on_close(move |_, _, cx| { let _ = close.update(cx, |this, cx| {
                    if this.sidebar.grouping.ask.as_ref().is_some_and(|a| a.seq == seq) { this.sidebar.grouping.ask = None; }
                    this.tour_event(crate::tour::TourEvent::GroupClosed, cx);
                }); })
        });
        cx.notify();
    }

    /// Relê o pedido aberto na lista viva: sessão que morreu ou mudou de grupo durante a confirmação decide aqui, não o clique.
    pub(super) fn refresh_group_ask(&self) {
        let Some(ask) = self.sidebar.grouping.ask.as_ref() else { return };
        let mut v = ask.view.borrow_mut();
        match &ask.mode {
            AskMode::Join { origin, target } => {
                let (o, t) = (self.target_session(origin), self.target_session(target));
                let ids = self.identities();
                // Agrupar funde os grupos inteiros: todos os afetados, não só os dois. Cada lista escreve os membros do
                // ponto de vista da máquina dela; aqui todos saem do desta, com o identificador só nos de outra máquina.
                let here = self.active_key();
                let mut names: Vec<String> = Vec::new();
                for (s, side) in [(o, origin), (t, target)] {
                    let Some(s) = s else { continue };
                    for name in std::iter::once(&side.name).chain(s.peers()) {
                        let name = rebased(&side.server, &here, name, &ids);
                        if !names.contains(&name) { names.push(name); }
                    }
                }
                v.affected = names;
                // Com a chamada em voo fica o que se via no clique.
                if !v.busy { v.inherited = existing_group(o, t).map(|g| g.pair_task.clone().unwrap_or_default()); }
                v.block = match (o, t) {
                    (Some(o), Some(t)) => {
                        let row = Row { server: &origin.server, session: o, invite: self.invite_target(origin) };
                        can_pair(row, Row { server: &target.server, session: t, invite: self.invite_target(target) }, &ids).err().map(Refusal::text)
                    }
                    _ => Some(Refusal::Dead.text()),
                };
                // O par de fora só ganha acesso na máquina onde o par foi feito: entre máquinas não se abre nada.
                if !v.busy {
                    v.grants = match (o, t) { (Some(o), Some(t)) if origin.server == target.server => [full_grants(t, o), full_grants(o, t)].concat(), _ => Vec::new() };
                }
            }
            AskMode::Leave { origin } => {
                let o = self.target_session(origin);
                v.affected = o.map(|s| s.peers().to_vec()).unwrap_or_default();
                v.block = o.is_none().then(|| Refusal::Dead.text());
                v.anchors = self.anchored_owners(origin);
            }
        }
    }

    fn confirm_group(&mut self, replace: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_group_ask();
        let Some(ask) = self.sidebar.grouping.ask.as_ref() else { return };
        let mut v = ask.view.borrow_mut();
        if v.busy || v.done || v.suggesting { return; }
        if let Some(block) = v.block.clone() { v.error = Some(block); cx.notify(); return; }
        // Entrar vai ao servidor do alvo; sair, ao da origem.
        let server = match &ask.mode { AskMode::Join { target, .. } => &target.server, AskMode::Leave { origin } => &origin.server };
        let Some(api) = self.machine_api(server) else { v.error = Some(tr("connection_failed")); cx.notify(); return };
        let (seq, tell) = (ask.seq, self.sidebar_tell());
        // Entrando num grupo que existe, a tarefa é a dele: o campo nem aparece.
        let task = if v.inherited.is_some() { String::new() } else { ask.task.read(cx).value().trim().to_owned() };
        (v.busy, v.error) = (true, None);
        match &ask.mode {
            AskMode::Join { origin, target } => {
                let Some(from) = qualified(&origin.server, &target.server, &origin.name, &self.identities()) else {
                    (v.busy, v.error) = (false, Some(Refusal::NoIdentity.text()));
                    cx.notify();
                    return;
                };
                let (origin, target, grants) = (from, target.name.clone(), v.grants.clone());
                self.runtime.spawn(async move {
                    let result = pair_and_grant(api, target, vec![origin], task, replace, grants).await;
                    tell.send(SidebarReply::Group(seq, GroupReply::Paired(result))).await;
                });
            }
            AskMode::Leave { origin } => {
                let origin = origin.name.clone();
                self.runtime.spawn(async move { tell.send(SidebarReply::Group(seq, GroupReply::Left(api.unpair(&origin).await))).await; });
            }
        }
        cx.notify();
    }

    fn suggest_group_task(&mut self, cx: &mut Context<Self>) {
        let Some(ask) = self.sidebar.grouping.ask.as_ref() else { return };
        let mut v = ask.view.borrow_mut();
        if v.busy || v.suggesting || v.block.is_some() { return; }
        let Some(api) = self.machine_api(ask_origin(&ask.mode).server.as_str()) else { v.error = Some(tr("connection_failed")); cx.notify(); return };
        let (seq, tell, names) = (ask.seq, self.sidebar_tell(), v.affected.clone());
        (v.suggesting, v.error) = (true, None);
        self.runtime.spawn(async move {
            tell.send(SidebarReply::Group(seq, GroupReply::Suggested(api.suggest_group_task(&names).await))).await;
        });
        cx.notify();
    }

    /// Resposta de um pedido: só o diálogo que a pediu a recebe; fechado, ela não mexe em nada.
    pub(super) fn receive_group(&mut self, seq: u64, reply: GroupReply, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.sidebar.grouping.ask.as_ref().filter(|a| a.seq == seq) else { return };
        let task = ask.task.clone();
        let mut close = false;
        {
            let mut v = ask.view.borrow_mut();
            match reply {
                GroupReply::Paired(Ok(result)) | GroupReply::Left(Ok(result)) => {
                    v.busy = false;
                    match result.warning {
                        // O vínculo mudou, mas alguém não foi avisado: fica à vista, com o título de feito.
                        Some(warning) => (v.done, v.conflict, v.error) = (true, None, Some(warning)),
                        None => close = true,
                    }
                }
                GroupReply::Paired(Err(error)) => {
                    v.busy = false;
                    if error.status == Some(409) { v.conflict = Some(Self::fetch_failure(&error)); }
                    else { (v.conflict, v.error) = (None, Some(failed(&error, "grupo_drop_falhou"))); }
                }
                GroupReply::Left(Err(error)) => (v.busy, v.error) = (false, Some(failed(&error, "grupo_drop_sair_falhou"))),
                GroupReply::Suggested(Ok(text)) => {
                    (v.suggesting, v.conflict) = (false, None);
                    drop(v);
                    task.update(cx, |state, cx| state.set_value(text, window, cx));
                }
                GroupReply::Suggested(Err(error)) => (v.suggesting, v.error) = (false, Some(failed(&error, "grupo_drop_falhou"))),
            }
        }
        if close { self.close_group_ask(window, cx); }
        cx.notify();
    }

    /// Fecha o diálogo por código. `close_dialog` não chama o `on_close` dele: o passo do tour sai daqui, uma vez.
    fn close_group_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.grouping.ask = None;
        window.close_dialog(cx);
        self.tour_event(crate::tour::TourEvent::GroupClosed, cx);
    }
}

fn ask_origin(mode: &AskMode) -> &Target { match mode { AskMode::Join { origin, .. } | AskMode::Leave { origin } => origin } }

/// Motivo do servidor; sem ele, a frase do web.
pub(super) fn failed(error: &Failure, fallback: &str) -> String {
    let text = Hangar::fetch_failure(error);
    if text.trim().is_empty() { web(fallback) } else { text }
}

/// Sessão solta + grupo que já existe = só entrar, herdando a tarefa dele. Duas soltas ou dois grupos: nasce/funde, com o campo.
fn existing_group<'a>(origin: Option<&'a SessionInfo>, target: Option<&'a SessionInfo>) -> Option<&'a SessionInfo> {
    let (o, t) = (origin?, target?);
    let grouped = |s: &SessionInfo| !s.peers().is_empty();
    if grouped(o) == grouped(t) { return None; }
    Some(if grouped(t) { t } else { o })
}

/// Submenu "Agrupar com…": as candidatas em mono; escolher abre o diálogo.
pub(super) fn fill_group(menu: PopupMenu, hangar: &WeakEntity<Hangar>, origin: &Target, candidates: &[(Target, String)]) -> PopupMenu {
    let menu = sidebar::menu_style(menu).label(tr("group_with"));
    if candidates.is_empty() { return menu.item(PopupMenuItem::new(tr("group_none")).disabled(true)); }
    candidates.iter().fold(menu.min_w(px(220.)).max_h(px(260.)).scrollable(true), |menu, (target, text)| {
        let (hangar, origin, target) = (hangar.clone(), origin.clone(), target.clone());
        menu.item(sidebar::mono_item(text.clone(), false).on_click(move |_, window, cx| {
            let _ = hangar.update(cx, |this, cx| this.request_group(origin.clone(), target.clone(), window, cx));
        }))
    })
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{GlobalGroup, Grant, IdSlot, ListRow, Member, Refusal, Row, SessionInfo, can_leave, can_pair, candidates, cluster, existing_group, full_grants,
        global_groups, id_slot, identity_settled, memo, pair_header_label, qualified, seal_text, split_code};
    use crate::api::dto::{PairExternal, PairExternalEntry, PairResult};
    use serde_json::Value;
    use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};

    fn s(name: &str, gid: Option<&str>, peers: &[&str]) -> SessionInfo {
        SessionInfo { name: name.into(), state: "idle".into(), pair_gid: gid.map(str::to_owned),
            pair_peers: (!peers.is_empty()).then(|| peers.iter().map(|p| (*p).to_owned()).collect()), ..Default::default() }
    }

    fn fixtures() -> Value { serde_json::from_str(include_str!("../../../packages/core/src/pairGroups.fixtures.json")).unwrap() }

    fn identities(case: &Value, all: &Value) -> HashMap<String, String> {
        serde_json::from_value(case.get("identities").unwrap_or(&all["defaultIdentities"]).clone()).unwrap()
    }

    fn row<'a>(v: &'a Value, session: &'a SessionInfo) -> Row<'a> {
        Row { server: v["server"].as_str().unwrap(), session, invite: v["invite"].as_bool().unwrap_or(false) }
    }

    #[test]
    fn global_groups_follow_the_shared_fixture() {
        let all = fixtures();
        for case in all["groups"].as_array().unwrap() {
            let raw = case["rows"].as_array().unwrap();
            let sessions: Vec<SessionInfo> = raw.iter().map(|v| serde_json::from_value(v.clone()).unwrap()).collect();
            let rows: Vec<Row> = raw.iter().zip(&sessions).map(|(v, s)| row(v, s)).collect();
            assert_eq!(serde_json::to_value(global_groups(&rows, &identities(case, &all))).unwrap(), case["groups"], "{}", case["name"]);
        }
    }

    #[test]
    fn can_pair_follows_the_shared_fixture() {
        let all = fixtures();
        for case in all["canPair"].as_array().unwrap() {
            let origin: SessionInfo = serde_json::from_value(case["origin"].clone()).unwrap();
            let target: SessionInfo = serde_json::from_value(case["target"].clone()).unwrap();
            let want = match case["result"].as_str().unwrap() {
                "ok" => Ok(()),
                "same" => Err(Refusal::Same),
                "orq" => Err(Refusal::Orq),
                "dead" => Err(Refusal::Dead),
                "invite" => Err(Refusal::Invite),
                "same_group" => Err(Refusal::SameGroup),
                "no_identity" => Err(Refusal::NoIdentity),
                "other_owner" => Err(Refusal::OtherOwner),
                other => panic!("motivo desconhecido na fixture: {other}"),
            };
            let got = can_pair(row(&case["origin"], &origin), row(&case["target"], &target), &identities(case, &all));
            assert_eq!(got, want, "{}", case["name"]);
        }
    }

    #[test]
    fn candidates_skip_sessions_being_deleted_and_invite_origins() {
        let here = vec![s("me", None, &[]), s("Bye", None, &[]), s("b", None, &[])];
        let me = &here[0];
        let ids = HashMap::new();
        let names = |origin: Row| candidates(origin, vec!["h".into()], |_| &here[..], |_, n| n == "Bye", &ids)
            .into_iter().map(|(t, _)| t.name).collect::<Vec<_>>();
        assert_eq!(names(Row { server: "h", session: me, invite: false }), ["b"], "a que está sendo apagada não entra");
        assert!(names(Row { server: "h", session: me, invite: true }).is_empty(), "sessão de servidor de convite não agrupa");
    }

    #[test]
    fn leaving_needs_a_group_or_a_remote_pair() {
        assert!(!can_leave(&s("a", None, &[])));
        assert!(can_leave(&s("a", Some("g"), &["b"])));
        assert!(can_leave(&s("a", None, &["srv::x"])), "par de outro servidor não tem gid");
        assert!(!can_leave(&SessionInfo { provider: "orq".into(), ..s("o", Some("g"), &["b"]) }), "o grupo do orq é dele");
    }

    fn shape(sections: &[Vec<ListRow>]) -> Vec<Vec<String>> {
        sections.iter().map(|rows| rows.iter().map(|row| match row {
            ListRow::Header { label, members, .. } => format!("[{label} {}]", members.len()),
            ListRow::Session { row, .. } => format!("{}:{}", row.server, row.session.name),
            ListRow::External { pair, .. } => format!("ext {}", pair.session),
        }).collect()).collect()
    }

    fn on<'a>(server: &'a str, list: &'a [SessionInfo]) -> Vec<Row<'a>> { list.iter().map(|session| Row { server, session, invite: false }).collect() }

    #[test]
    fn cluster_puts_the_header_where_the_first_member_was() {
        let all = [s("a", None, &[]), s("b", Some("g"), &["d"]), s("c", None, &[]), s("d", Some("g"), &["b"])];
        let ids = HashMap::new();
        assert_eq!(shape(&cluster(&[on("h", &all)], &ids)), [["h:a", "[b, d 2]", "h:b", "h:d", "h:c"]], "sem tarefa o rótulo são os nomes");
        let tasked = [s("x", Some("g"), &["y"]), SessionInfo { pair_task: Some(" ABC-1 tela ".into()), ..s("y", Some("g"), &["x"]) }];
        assert!(matches!(&cluster(&[on("h", &tasked)], &ids)[0][0], ListRow::Header { label, .. } if label == "ABC-1 tela"));
    }

    // O grupo g1 do relato: duas sessões em cada máquina, dono lab.
    fn fed(name: &str, peers: &[&str]) -> SessionInfo {
        SessionInfo { pair_owner: Some("lab".into()), ..s(name, Some("g1"), peers) }
    }

    fn two_machines() -> (Vec<SessionInfo>, Vec<SessionInfo>, HashMap<String, String>) {
        let nb = vec![s("aaa", None, &[]),
            fed("ABC-1-coord", &["ABC-1-proxy", "vm::ABC-1-app", "vm::abc2-app"]),
            fed("ABC-1-proxy", &["ABC-1-coord", "vm::ABC-1-app", "vm::abc2-app"])];
        let dp = vec![fed("ABC-1-app", &["abc2-app", "lab::ABC-1-coord", "lab::ABC-1-proxy"]),
            fed("abc2-app", &["ABC-1-app", "lab::ABC-1-coord", "lab::ABC-1-proxy"]),
            s("zzz", None, &[])];
        (nb, dp, HashMap::from([("nb".to_owned(), "lab".to_owned()), ("dp".to_owned(), "vm".to_owned())]))
    }

    #[test]
    fn a_group_across_machines_is_one_block_where_its_first_member_is() {
        let (nb, dp, ids) = two_machines();
        let got = cluster(&[on("dp", &dp), on("nb", &nb)], &ids);
        assert_eq!(shape(&got), [vec!["[ABC-1-app, abc2-app, ABC-1-coord, ABC-1-proxy 4]", "dp:ABC-1-app",
            "dp:abc2-app", "nb:ABC-1-coord", "nb:ABC-1-proxy", "dp:zzz"], vec!["nb:aaa"]],
            "os quatro juntos na seção do primeiro, sem os da outra máquina no cabeçalho, e fora da seção deles");
        let blocks: Vec<&str> = got.iter().flatten().filter_map(|r| match r { ListRow::Session { block: Some(b), .. } => Some(b.as_str()), _ => None }).collect();
        assert!(blocks.len() == 4 && blocks.iter().all(|b| *b == blocks[0]), "um bloco só recolhe os quatro: {blocks:?}");
        assert!(matches!(&got[0][0], ListRow::Header { key, .. } if key == blocks[0]));

        // Com "Aguardando" no topo: o primeiro membro na ordem da barra está lá, e o bloco inteiro vai junto.
        assert_eq!(shape(&cluster(&[on("nb", &nb[2..]), on("nb", &nb[..2]), on("dp", &dp)], &ids)),
            [vec!["[ABC-1-proxy, ABC-1-coord, ABC-1-app, abc2-app 4]", "nb:ABC-1-proxy",
                "nb:ABC-1-coord", "dp:ABC-1-app", "dp:abc2-app"], vec!["nb:aaa"], vec!["dp:zzz"]]);

        let mut tasked = nb.clone();
        tasked[1].pair_task = Some("ABC-1 bulas".into());
        assert!(matches!(&cluster(&[on("dp", &dp), on("nb", &tasked)], &ids)[0][0], ListRow::Header { label, .. } if label == "ABC-1 bulas"),
            "com tarefa, só ela");
    }

    #[test]
    fn every_sidebar_mode_shows_the_group_once_and_counts_what_each_section_shows() {
        use super::super::sidebar::{MachineList, layout, section_counts, sections};
        let (nb, dp, ids) = two_machines();
        let none = std::collections::HashSet::new();
        for by_project in [false, true] {
            let machines = [MachineList { entry: None, key: "nb".into(), remote: None, layout: layout(&nb, "", by_project, &none) },
                MachineList { entry: None, key: "dp".into(), remote: Some("dp".into()), layout: layout(&dp, "", by_project, &none) }];
            // Por máquina: Aguardando, Trabalhando, nb, dp. Nos outros: nb, Aguardando dela, o grupo dela; o mesmo para dp.
            for (by_machine, want) in [(true, vec![0, 0, 5, 1]), (false, vec![5, 0, 5, 1, 0, 1])] {
                let parts = sections(&machines, by_machine, true);
                let got = cluster(&parts.iter().map(|p| p.rows.clone()).collect::<Vec<_>>(), &ids);
                let mode = format!("por máquina {by_machine}, por projeto {by_project}");
                let shown = shape(&got);
                let block = shown.iter().position(|rows| rows.iter().any(|r| r.ends_with(" 4]"))).expect(&mode);
                assert_eq!(shown[block].iter().filter(|r| r.contains("ABC-1") || r.contains("abc2")).count(), 5, "{mode}: cabeçalho e os quatro");
                assert!(shown.iter().enumerate().all(|(i, rows)| i == block || rows.iter().all(|r| !r.contains("ABC-1") && !r.contains("abc2"))),
                    "{mode}: nenhum membro fora do bloco: {shown:?}");
                assert_eq!(section_counts(&parts, &got), want, "{mode}");
            }
        }
    }

    #[test]
    fn external_peer_row_follows_its_group() {
        let x = [SessionInfo { pair_external: Some(PairExternal { alias: "pc-ana".into(), owner: "pc-ana".into(), session: "Y".into() }),
            ..s("X", Some("g"), &["pc-ana::Y"]) }];
        let rows = cluster(&[on("h", &x)], &HashMap::new()).remove(0);
        assert!(matches!(rows.last(), Some(ListRow::External { gid, own, pair, .. }) if pair.session == "Y" && gid == "g" && *own == "h"));
        assert_eq!(rows.len(), 3, "cabeçalho, membro e a linha da outra pessoa");
    }

    fn ext(alias: &str, owner: &str, session: &str, level: &str, pending: bool) -> PairExternalEntry {
        PairExternalEntry { alias: alias.into(), owner: owner.into(), session: session.into(), address: format!("https://{alias}.ts.net:8443"),
            level: level.into(), pending }
    }

    #[test]
    fn cluster_lists_every_outside_pair_once_and_names_them_in_the_header() {
        let pairs = vec![ext("ana", "Ana", "Y", "view", false), ext("bia", "Bia", "Z", "full", false)];
        let xw = [SessionInfo { pair_externals: Some(pairs.clone()), pair_task: Some("ABC-1".into()), ..s("X", Some("g"), &["W", "ana::Y", "bia::Z"]) },
            SessionInfo { pair_externals: Some(pairs), ..s("W", Some("g"), &["X", "ana::Y", "bia::Z"]) }];
        let rows = cluster(&[on("h", &xw)], &HashMap::new()).remove(0);
        let outside: Vec<&str> = rows.iter().filter_map(|r| match r { ListRow::External { pair, .. } => Some(pair.session.as_str()), _ => None }).collect();
        assert_eq!(outside, ["Y", "Z"], "um por par, mesmo com dois membros locais o tendo");
        let ListRow::Header { label, .. } = &rows[0] else { panic!("cabeçalho primeiro") };
        assert!(label.starts_with("ABC-1 ") && label.contains("ana::Y (Ana)") && label.contains("bia::Z (Bia)"), "{label}");
        assert_eq!(pair_header_label("t", &[], &[]), "t", "sem par, o rótulo de sempre");
    }

    #[test]
    fn joining_a_group_with_a_full_outside_pair_opens_every_newcomer() {
        let group = SessionInfo { pair_externals: Some(vec![ext("ana", "Ana", "Y", "full", false), ext("bia", "Bia", "Z", "view", false)]),
            ..s("X", Some("g"), &["ana::Y", "bia::Z"]) };
        let side = s("a", Some("h"), &["b", "srv::c"]);
        let got = full_grants(&group, &side);
        assert_eq!(got, vec![Grant { session: "a".into(), alias: "ana".into(), owner: "Ana".into() },
            Grant { session: "b".into(), alias: "ana".into(), owner: "Ana".into() }], "só o par completo, só as locais");
        let pending = SessionInfo { pair_externals: Some(vec![ext("ana", "Ana", "Y", "view", true), ext("bia", "Bia", "Z", "full", true)]),
            ..s("X", Some("g"), &["ana::Y", "bia::Z"]) };
        assert!(full_grants(&pending, &s("a", None, &[])).is_empty(), "pendente só abre pelo Liberar");
        assert!(full_grants(&s("a", None, &[]), &group).is_empty(), "grupo sem par de fora não pede nada");
    }

    #[test]
    fn header_names_the_members_on_machines_not_connected_before_the_outside_pairs() {
        let xw = [SessionInfo { pair_externals: Some(vec![ext("ana", "Ana", "Y", "view", false)]), pair_task: Some("ABC-1".into()),
            ..s("X", Some("g"), &["W", "casa::a", "ana::Y"]) }, s("W", Some("g"), &["X", "casa::a", "lab::b", "ana::Y"])];
        let ids = HashMap::from([("h".to_owned(), "pc".to_owned())]);
        let ListRow::Header { label, .. } = &cluster(&[on("h", &xw)], &ids)[0][0] else { panic!("cabeçalho primeiro") };
        assert_eq!(label, "ABC-1 ↔ casa::a, lab::b, ana::Y (Ana)", "cada membro de máquina sem conexão uma vez, o par de fora por último");
        assert_eq!(pair_header_label("t", &["casa::a".into()], &[]), "t ↔ casa::a");
    }

    fn group(label: &str, members: Vec<Member>) -> GlobalGroup { GlobalGroup { key: "k".into(), label: label.into(), members } }
    fn here(server: &str, name: &str) -> Member { Member::Here { server: server.into(), name: name.into() } }

    #[test]
    fn seal_shows_the_ticket_or_how_many_machines_only_across_machines() {
        assert_eq!(seal_text(&group("ABC-12 tela", vec![here("a", "x"), here("b", "y")])).as_deref(), Some("ABC-12"));
        assert_eq!(seal_text(&group("x, y", vec![here("a", "x"), here("b", "y"), Member::Missing { server_id: "lab".into(), name: "z".into() }]))
            .as_deref(), Some("3 máquinas"), "máquina sem conexão aqui também conta");
        assert_eq!(seal_text(&group("x, y", vec![here("a", "x"), here("a", "y"), Member::Outside { alias: "ana".into(), session: "Y".into() }])),
            None, "uma máquina só, mesmo com par de fora");
    }

    #[test]
    fn request_names_the_origin_by_its_machine_identifier_on_another_machine() {
        let ids = HashMap::from([("a".to_owned(), "casa".to_owned()), ("b".to_owned(), "lab".to_owned()), ("c".to_owned(), String::new())]);
        assert_eq!(qualified("a", "a", "x", &ids).as_deref(), Some("x"));
        assert_eq!(qualified("a", "b", "x", &ids).as_deref(), Some("casa::x"));
        assert_eq!(qualified("c", "b", "x", &ids), None, "identificador vazio não se adivinha");
    }

    #[test]
    fn members_listed_by_another_machine_are_named_from_this_one() {
        // "a" é esta máquina (casa); a lista de "b" (lab) escreve as sessões daqui com prefixo e as dela sem.
        let ids = HashMap::from([("a".to_owned(), "casa".to_owned()), ("b".to_owned(), "lab".to_owned())]);
        assert_eq!(super::rebased("b", "a", "casa::coordenadora", &ids), "coordenadora");
        assert_eq!(super::rebased("b", "a", "revisor", &ids), "lab::revisor");
        assert_eq!(super::rebased("b", "a", "oficina::outra", &ids), "oficina::outra");
        assert_eq!(super::rebased("a", "a", "x", &ids), "x");
    }

    #[test]
    fn task_code_splits_like_the_web_regex() {
        assert_eq!(split_code("ABC-1234 Assunto comprido"), ("ABC-1234", "Assunto comprido"));
        assert_eq!(split_code("XYZ-42: tela"), ("XYZ-42", ": tela"), "\\b antes da pontuação");
        assert_eq!(split_code("hangar.v2-12"), ("hangar.v2-12", ""));
        assert_eq!(split_code("ABC-12x resto"), ("ABC-12x resto", ""), "dígito colado em letra não é chave");
        assert_eq!(split_code("a, b"), ("a, b", ""));
        assert_eq!(split_code("1-2 x"), ("1-2 x", ""));
    }

    #[test]
    fn joining_an_existing_group_inherits_its_task() {
        let (lone, grouped) = (s("a", None, &[]), s("b", Some("g"), &["c"]));
        assert_eq!(existing_group(Some(&lone), Some(&grouped)).map(|g| g.name.as_str()), Some("b"));
        assert!(existing_group(Some(&lone), Some(&s("z", None, &[]))).is_none(), "duas soltas: grupo novo");
        assert!(existing_group(Some(&grouped), Some(&s("d", Some("h"), &["e"]))).is_none(), "dois grupos: fundem");
    }

    #[test]
    fn identity_counts_only_when_read_and_not_empty() {
        assert!(identity_settled(Some(&IdSlot::Known("casa".into()))));
        for slot in [None, Some(IdSlot::Loading), Some(IdSlot::Failed(Instant::now()))] {
            assert!(!identity_settled(slot.as_ref()), "leitura em voo, falha ou vazio não dizem que a máquina está fora");
        }
    }

    #[test]
    fn empty_identifier_is_a_failed_read_retried_later() {
        assert!(matches!(id_slot(Some(&serde_json::json!({"identificador": ""}))), IdSlot::Failed(_)));
        assert!(matches!(id_slot(Some(&serde_json::json!({}))), IdSlot::Failed(_)));
        assert!(matches!(id_slot(None), IdSlot::Failed(_)));
        assert!(matches!(id_slot(Some(&serde_json::json!({"identificador": "casa"}))), IdSlot::Known(id) if id == "casa"));
    }

    #[test]
    fn seals_are_recomputed_only_when_the_key_changes() {
        let cell = RefCell::new(None);
        let mut runs = 0;
        let first = memo(&cell, 1, || { runs += 1; 10 });
        let again = memo(&cell, 1, || { runs += 1; 20 });
        assert!(Rc::ptr_eq(&first, &again), "mesma lista: o mesmo cálculo");
        let changed = memo(&cell, 2, || { runs += 1; 30 });
        assert_eq!((*changed, runs), (30, 2));
    }

    #[test]
    fn every_refusal_has_a_phrase() {
        for r in [Refusal::Same, Refusal::Orq, Refusal::Dead, Refusal::Invite, Refusal::SameGroup, Refusal::NoIdentity, Refusal::OtherOwner] {
            let text = r.text();
            assert!(!text.starts_with("grupo_recusa") && !text.starts_with("group_"), "{r:?} sem frase: {text}");
        }
    }

    #[test]
    fn pair_warning_reads_the_envelope_message() {
        assert_eq!(PairResult::from_value(&serde_json::json!({"ok": true, "warning": null})).warning, None);
        assert_eq!(PairResult::from_value(&serde_json::json!({"warning": {"code": "x", "msg": "aviso falhou em: b"}})).warning.as_deref(),
            Some("aviso falhou em: b"));
        assert_eq!(PairResult::from_value(&serde_json::json!({"warning": "cru"})).warning.as_deref(), Some("cru"));
    }
}
