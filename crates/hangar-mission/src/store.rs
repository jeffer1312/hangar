//! `~/.hangar/mission/mission.json` e a pasta das evidências: trocado de forma atômica, sob trava.
use crate::model::{Evidence, Event, EventKind, Item, Snapshot, Stage, default_stages, now};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const FORMAT_VERSION: u32 = 3;
const MAX_STAGES: usize = 12;
const MAX_EVIDENCE: usize = 200;
/// Teto de um arquivo que a sessão anexa pelo caminho: cabe um vídeo curto de funcionamento.
const MAX_EVIDENCE_BYTES: u64 = 512 * 1024 * 1024;
/// Teto do envio do dono pelo app, que passa inteiro pela memória (o mesmo do anexo do app).
pub const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;
const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Invalid, NotFound, Gate, Unavailable }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error { pub kind: Kind, pub code: &'static str, pub message: String }

impl Error {
    pub(crate) fn new(kind: Kind, code: &'static str, message: impl Into<String>) -> Self { Self { kind, code, message: message.into() } }
    pub fn invalid(message: impl Into<String>) -> Self { Self::new(Kind::Invalid, "invalid_params", message) }
    fn io(action: &str, error: &io::Error) -> Self { Self::new(Kind::Unavailable, "mission_unavailable", format!("{action}: {:?}", error.kind())) }
    fn item_not_found() -> Self { Self::new(Kind::NotFound, "mission_item_not_found", "ticket sem registro no painel de tickets") }
    fn evidence_not_found() -> Self { Self::new(Kind::NotFound, "mission_evidence_not_found", "evidência não encontrada") }
    fn not_linked() -> Self {
        Self::new(Kind::NotFound, "mission_session_not_linked", "esta sessão não está ligada a um ticket")
    }
    fn gate(stage: &Stage) -> Self {
        Self::new(Kind::Gate, "mission_gate_pending",
            format!("a etapa {} espera a aprovação do dono no painel de tickets; anexe as evidências e aguarde", stage.name))
    }
    fn too_much_evidence() -> Self { Self::invalid(format!("no máximo {MAX_EVIDENCE} evidências por ticket")) }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    stages: Vec<Stage>,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    remote_links: std::collections::BTreeMap<String,String>,
    #[serde(default)]
    remote_groups: std::collections::BTreeMap<String,String>,
}

impl Default for File {
    fn default() -> Self { Self { version: FORMAT_VERSION, stages: default_stages(), items: Vec::new(), remote_links:Default::default(),remote_groups:Default::default() } }
}

impl File {
    fn item_mut(&mut self, connector: &str, ticket_id: &str) -> Result<&mut Item, Error> {
        self.items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)
    }

    fn upsert(&mut self, connector: &str, ticket_id: &str, ticket: TicketMetadata) -> Result<&mut Item, Error> {
        if !self.items.iter().any(|i| i.connector == connector && i.ticket_id == ticket_id) {
            self.items.push(Item { connector: connector.to_owned(), ticket_id: ticket_id.to_owned(), key: String::new(),
                title: String::new(), url: String::new(), session: None, session_key: None, conversation: None, stage: None, approved: Vec::new(),
                history: Vec::new(), evidence: Vec::new(), delivered_at: None, sessions: Vec::new(), session_keys:Default::default(), group_ids:Vec::new(), group_members:Default::default(), repository: None });
        }
        let item = self.item_mut(connector, ticket_id)?;
        (item.key, item.title, item.url) = (ticket.key, ticket.title, ticket.url);
        Ok(item)
    }
}

/// Ticket que a sessão vai trabalhar, com o que mostrar dele sem perguntar ao conector.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub key: String,
    pub title: String,
    pub url: String,
    pub session: String,
}

pub struct TicketMetadata { pub key: String, pub title: String, pub url: String }

pub struct ImportedEvidence { evidence: Evidence, path: PathBuf, kept: bool }
pub struct ReportBinding { pub session_key: Option<String>, pub history_revision: usize }
impl Drop for ImportedEvidence {
    fn drop(&mut self) {if !self.kept {let _=std::fs::remove_file(&self.path);}}
}

impl TicketMetadata {
    pub fn validate(&self) -> Result<(),Error> {metadata_valid(self)}
}

fn metadata_valid(ticket: &TicketMetadata) -> Result<(), Error> {
    text("key", &ticket.key, 1024, false)?;
    text("title", &ticket.title, 4096, true)?;
    text("url", &ticket.url, 4096, false)
}

/// Mudança feita pelo dono na tela. Em `approve` e `changes`, `stage` é a etapa que a tela mostrava:
/// ela vale quando o ticket ainda não tem etapa gravada (a tela a deduziu pelo status ou pelo plano).
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OwnerChange {
    Repository { path: String, key: String, title: String, url: String },
    /// Põe o ticket numa etapa, sem portão: o dono decide.
    Stage { stage: String },
    /// Aprova a etapa com portão em que o ticket está e o leva à seguinte.
    Approve {
        #[serde(default)]
        stage: Option<String>,
    },
    /// Recusa a etapa atual com o motivo: volta à anterior e desfaz as aprovações daí em diante.
    Changes {
        note: String,
        #[serde(default)]
        stage: Option<String>,
    },
    /// Validação entregue: o ticket sai das etapas.
    Delivered {
        #[serde(default)]
        note: Option<String>,
    },
    /// Volta a deduzir a etapa (desfaz a escolha manual e a entrega).
    Reset,
}

/// O que a sessão informa pela tool `mission`.
pub struct Report {
    pub stage: Option<String>,
    pub note: Option<String>,
    pub evidence: Vec<(PathBuf, String)>,
}

pub struct Store { path: PathBuf, evidence_dir: PathBuf, actions: Mutex<HashSet<(String, String)>> }

pub struct ActionGuard<'a> { store: &'a Store, key: (String, String) }

impl Drop for ActionGuard<'_> {
    fn drop(&mut self) { self.store.actions.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key); }
}

fn action_busy() -> Error { Error::new(Kind::Gate, "mission_action_busy", "aguarde a ação do ticket terminar antes de alterar o vínculo ou as etapas") }

fn text(field: &str, value: &str, max: usize, empty_ok: bool) -> Result<(), Error> {
    if (!empty_ok && value.trim().is_empty()) || value.len() > max || value.contains('\0') {
        return Err(Error::invalid(format!("{field} vazio, com NUL ou acima de {max} bytes")));
    }
    Ok(())
}

fn valid_stage_id(id: &str) -> bool {
    hangar_core::ids::valid_slug(id, 32)
}

/// Tipo pelo nome: só imagem, vídeo, PDF e texto viram evidência.
fn evidence_mime(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

fn event(kind: EventKind, by: &str, stage: Option<&str>, note: Option<&str>) -> Event {
    Event { at: now(), kind, by: by.to_owned(), stage: stage.map(str::to_owned), note: note.map(str::to_owned) }
}

/// Primeira etapa com portão antes de `target` ainda sem aprovação.
fn pending_gate<'a>(stages: &'a [Stage], item: &Item, target: usize) -> Option<&'a Stage> {
    gate_before(stages, &item.approved, target)
}

fn gate_before<'a>(stages: &'a [Stage], approved: &[String], target: usize) -> Option<&'a Stage> {
    stages[..target].iter().find(|stage| stage.gate && !approved.contains(&stage.id))
}

/// Volta à etapa `index`: o portão dela e os seguintes fecham de novo.
fn reopen_from(stages: &[Stage], item: &mut Item, index: usize) {
    let kept: HashSet<&str> = stages[..index].iter().map(|s| s.id.as_str()).collect();
    item.approved.retain(|id| kept.contains(id.as_str()));
}

fn stage_index(stages: &[Stage], id: &str) -> Result<usize, Error> {
    stages.iter().position(|s| s.id == id).ok_or_else(|| Error::invalid(format!(
        "etapa desconhecida: {id}; as etapas são {}", stages.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "))))
}

/// Cópias feitas antes da trava: saem se a mudança não for gravada.
struct Staged<'a> { store: &'a Store, ids: Vec<String>, kept: bool }

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if !self.kept { for id in &self.ids { self.store.drop_evidence(id); } }
    }
}

impl Store {
    /// `dir` guarda o `mission.json` e a pasta `evidence/`.
    pub fn new(dir: PathBuf) -> Self { Self { path: dir.join("mission.json"), evidence_dir: dir.join("evidence"), actions: Mutex::default() } }

    pub fn begin_action(&self, connector: &str, ticket_id: &str) -> Result<ActionGuard<'_>, Error> {
        if !hangar_core::ids::valid_slug(connector, 64) { return Err(Error::invalid("conector fora do formato")); }
        text("ticket_id", ticket_id, 1024, false)?;
        let key = (connector.to_owned(), ticket_id.to_owned());
        let mut active = self.actions.lock().unwrap_or_else(|e| e.into_inner());
        if !active.insert(key.clone()) { return Err(action_busy()); }
        Ok(ActionGuard { store: self, key })
    }

    pub fn action_stage(&self, action: &ActionGuard<'_>, stage: &str) -> Result<(Stage, Option<Item>), Error> {
        let _active = self.mutation(&action.key.0, &action.key.1, Some(action))?;
        let file = self.read()?;
        let index = stage_index(&file.stages, stage)?;
        let item = file.items.iter().find(|i| i.connector == action.key.0 && i.ticket_id == action.key.1).cloned();
        if let Some(gate) = gate_before(&file.stages, item.as_ref().map(|i| i.approved.as_slice()).unwrap_or_default(), index) {
            return Err(Error::gate(gate));
        }
        Ok((file.stages[index].clone(), item))
    }

    pub fn link_action(&self, action: &ActionGuard<'_>, link: Link) -> Result<(Item, bool), Error> {
        self.link_with_action(&action.key.0, &action.key.1, link, Some(action),None,None)
    }

    pub fn link_bound_action(&self, action: &ActionGuard<'_>, link: Link, key: &str, conversation: Option<&str>) -> Result<(Item,bool), Error> {
        text("session_key",key,1024,false)?;
        if let Some(value) = conversation {text("conversation",value,1024,false)?;}
        self.link_with_action(&action.key.0,&action.key.1,link,Some(action),Some((key,conversation)),None)
    }

    pub fn track_action(&self, action: &ActionGuard<'_>, ticket: TicketMetadata) -> Result<Item, Error> {
        metadata_valid(&ticket)?;
        let _active = self.mutation(&action.key.0, &action.key.1, Some(action))?;
        self.update(|file| file.upsert(&action.key.0, &action.key.1, ticket).cloned())
    }

    pub fn commit_action(&self, action: &ActionGuard<'_>, stage: &str, history: usize, note: &str) -> Result<Item, Error> {
        self.commit_stage(action,stage,Some(history),note)
    }

    fn commit_stage(&self,action: &ActionGuard<'_>,stage: &str,history: Option<usize>,note: &str) -> Result<Item,Error> {
        text("note", note, MAX_TEXT, false)?;
        let _active = self.mutation(&action.key.0, &action.key.1, Some(action))?;
        self.update(|file| {
            let target = stage_index(&file.stages, stage)?;
            let item = file.items.iter_mut().find(|i| i.connector == action.key.0 && i.ticket_id == action.key.1).ok_or_else(Error::item_not_found)?;
            let reported = match history {
                Some(history)=>item.history.get(history..).ok_or_else(||Error::invalid("histórico da ação desatualizado"))?
                    .iter().any(|event|event.kind==EventKind::Stage),
                None=>false,
            };
            if !reported {
                if let Some(gate) = pending_gate(&file.stages, item, target) { return Err(Error::gate(gate)); }
                reopen_from(&file.stages, item, target);
                item.history.push(event(EventKind::Stage, "dono", Some(stage), Some(note)));
                item.stage = Some(stage.to_owned());
                item.delivered_at = None;
            } else {
                item.history.push(event(EventKind::Note, "dono", item.stage.as_deref(), Some(note)));
            }
            Ok(item.clone())
        })
    }

    pub fn move_action(&self,action: &ActionGuard<'_>,stage: &str) -> Result<Item,Error> {
        self.commit_stage(action,stage,None,"Etapa escolhida pelo dono, sem executar ação.")
    }

    fn mutation(&self, connector: &str, ticket_id: &str, action: Option<&ActionGuard<'_>>) -> Result<MutexGuard<'_, HashSet<(String, String)>>, Error> {
        let owns = action.is_some_and(|a| std::ptr::eq(a.store, self) && a.key.0 == connector && a.key.1 == ticket_id);
        if action.is_some() && !owns { return Err(Error::invalid("reserva de ação inválida")); }
        let active = self.actions.lock().unwrap_or_else(|e| e.into_inner());
        if !owns && active.iter().any(|(c, t)| c == connector && t == ticket_id) { return Err(action_busy()); }
        Ok(active)
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let file = self.read()?;
        Ok(Snapshot { stages: file.stages, items: file.items })
    }

    pub fn remote_owner(&self,name: &str,life: &str) -> Result<Option<String>,Error> {
        Ok(self.read()?.remote_links.get(&format!("{name}\0{life}")).cloned())
    }

    pub fn remember_remote(&self,name: &str,life: &str,owner: &str) -> Result<(),Error> {
        if self.remote_owner(name,life)?.as_deref()==Some(owner) {return Ok(());}
        self.update(|file|{file.remote_links.insert(format!("{name}\0{life}"),owner.to_owned());Ok(())})
    }

    pub fn remote_group(&self,gid: &str) -> Result<Option<String>,Error> {Ok(self.read()?.remote_groups.get(gid).cloned())}

    pub fn remember_remote_group(&self,gid: &str,owner: &str) -> Result<(),Error> {
        if self.remote_group(gid)?.as_deref()==Some(owner) {return Ok(());}
        self.update(|file|{file.remote_groups.insert(gid.to_owned(),owner.to_owned());Ok(())})
    }

    /// Agrupar associa os participantes ao mesmo ticket. Separar ou fechar o grupo conserva o trabalho já associado.
    pub fn sync_groups(&self, groups: &[crate::model::Group]) -> Result<Snapshot, Error> {
        fn apply(items: &mut [Item], groups: &[crate::model::Group]) -> bool {
            let mut changed=false;
            for item in items {
                if let Some(anchor)=&item.session && !item.sessions.contains(anchor) {item.sessions.push(anchor.clone());changed=true;}
                if let Some((anchor,key))=item.session.as_ref().zip(item.session_key.as_ref()) {
                    if item.session_keys.get(anchor)!=Some(key) {item.session_keys.insert(anchor.clone(),key.clone());changed=true;}
                }
                for group in groups {
                    let known_group=item.group_ids.contains(&group.id);
                    let known_member=group.members.iter().any(|member|item.sessions.contains(member)
                        && item.session_keys.get(member).is_some_and(|key|group.keys.get(member)==Some(key)));
                    if !known_group && !known_member {continue;}
                    if !item.group_ids.contains(&group.id) {item.group_ids.push(group.id.clone());changed=true;}
                    if item.group_members.get(&group.id)!=Some(&group.members) {item.group_members.insert(group.id.clone(),group.members.clone());changed=true;}
                    for member in &group.members {
                        if !known_group && let Some(expected)=item.session_keys.get(member) && group.keys.get(member).is_some_and(|key|key!=expected) {continue;}
                        if !item.sessions.contains(member) {item.sessions.push(member.clone());changed=true;}
                        if let Some(key)=group.keys.get(member) && item.session_keys.get(member)!=Some(key) {item.session_keys.insert(member.clone(),key.clone());changed=true;}
                        if item.session.as_ref()==Some(member) && let Some(key)=group.keys.get(member) && item.session_key.as_ref()!=Some(key) {item.session_key=Some(key.clone());changed=true;}
                    }
                }
                item.sessions.sort();item.sessions.dedup();
            }
            changed
        }
        let mut snapshot=self.snapshot()?;
        if !apply(&mut snapshot.items,groups) {return Ok(snapshot);}
        self.update(|file| {
            apply(&mut file.items,groups);
            Ok(Snapshot {stages:file.stages.clone(),items:file.items.clone()})
        })
    }

    pub fn bind_participant(&self, connector: &str, ticket: &str, name: &str, key: &str) -> Result<Item,Error> {
        text("session_key",key,1024,false)?;
        self.update(|file| {
            let item=file.item_mut(connector,ticket)?;
            if item.session_keys.get(name).is_some_and(|expected|expected!=key) {return Err(Error::not_linked());}
            if !item.sessions.iter().any(|member|member==name) {item.sessions.push(name.to_owned());}
            item.session_keys.insert(name.to_owned(),key.to_owned());
            Ok(item.clone())
        })
    }

    pub fn rename_session(&self,old: &str,new: &str) -> Result<bool,Error> {
        if !hangar_core::ids::valid_session_name(old) || !hangar_core::ids::valid_session_name(new) {return Err(Error::invalid("nome da sessão inválido"));}
        let _active=self.actions.lock().unwrap_or_else(|e|e.into_inner());
        self.update(|file| {
            let mut changed=false;
            let prefix=format!("{old}\0");
            let routes=file.remote_links.iter().filter_map(|(key,owner)|key.strip_prefix(&prefix).map(|life|(key.clone(),format!("{new}\0{life}"),owner.clone()))).collect::<Vec<_>>();
            for (old,new,owner) in routes {file.remote_links.remove(&old);file.remote_links.insert(new,owner);changed=true;}
            for item in &mut file.items {
                if item.session.as_deref()==Some(old) {item.session=Some(new.to_owned());changed=true;}
                for session in &mut item.sessions {if session==old {*session=new.to_owned();changed=true;}}
                item.sessions.sort();item.sessions.dedup();
                if let Some(key)=item.session_keys.remove(old) {item.session_keys.insert(new.to_owned(),key);changed=true;}
                for members in item.group_members.values_mut() {for member in members {if member==old {*member=new.to_owned();changed=true;}}}
            }
            Ok(changed)
        })
    }

    pub fn set_stages(&self, stages: Vec<Stage>) -> Result<Snapshot, Error> {
        let active = self.actions.lock().unwrap_or_else(|e| e.into_inner());
        if !active.is_empty() { return Err(action_busy()); }
        if stages.is_empty() || stages.len() > MAX_STAGES { return Err(Error::invalid(format!("de 1 a {MAX_STAGES} etapas"))); }
        let mut seen = HashSet::new();
        for stage in &stages {
            if !valid_stage_id(&stage.id) { return Err(Error::invalid(format!("id de etapa fora do formato: {:?}", stage.id))); }
            if !seen.insert(stage.id.as_str()) { return Err(Error::invalid(format!("etapa repetida: {}", stage.id))); }
            text("name", &stage.name, 60, false)?;
            for status in &stage.statuses { text("statuses", status, 120, false)?; }
            if let Some(prompt) = &stage.prompt { text("prompt", prompt, MAX_TEXT, true)?; }
            if let Some(connectors) = &stage.actions {
                if connectors.len() > 64 { return Err(Error::invalid("no máximo 64 conectores por etapa")); }
                for (connector, actions) in connectors {
                    if !hangar_core::ids::valid_slug(connector, 64) { return Err(Error::invalid("id do conector fora do formato")); }
                    if actions.len() > 64 { return Err(Error::invalid("no máximo 64 ações por conector na etapa")); }
                    let mut ids = HashSet::new();
                    if actions.iter().any(|id| !hangar_core::ids::valid_slug(id, 64) || !ids.insert(id)) {
                        return Err(Error::invalid("id de ação inválido ou repetido"));
                    }
                }
            }
        }
        self.update(|file| {
            let mut stages = stages;
            // Um cliente antigo não conhece estes campos; omiti-los não apaga a configuração.
            for stage in &mut stages {
                if let Some(old) = file.stages.iter().find(|old| old.id == stage.id) {
                    if stage.prompt.is_none() { stage.prompt = old.prompt.clone(); }
                    if stage.actions.is_none() { stage.actions = old.actions.clone(); }
                }
            }
            let known: HashSet<&str> = stages.iter().map(|s| s.id.as_str()).collect();
            // Etapa que saiu deixa de valer nos tickets: eles voltam a ser deduzidos.
            for item in &mut file.items {
                if item.stage.as_deref().is_some_and(|id| !known.contains(id)) { item.stage = None; }
                item.approved.retain(|id| known.contains(id.as_str()));
            }
            file.stages = stages;
            Ok(Snapshot { stages: file.stages.clone(), items: file.items.clone() })
        })
    }

    /// Liga o ticket à sessão. A sessão sai de outro ticket em que estivesse: uma sessão, um ticket.
    /// `true` junto do item quando a sessão é nova para o ticket (quem chama a avisa).
    pub fn link(&self, connector: &str, ticket_id: &str, link: Link) -> Result<(Item, bool), Error> {
        self.link_with_action(connector, ticket_id, link, None,None,None)
    }

    pub fn link_owner(&self,connector: &str,ticket: &str,link: Link,key: Option<&str>,groups: &[crate::model::Group]) -> Result<(Item,bool),Error> {
        if let Some(key)=key {text("session_key",key,1024,false)?;}
        self.link_with_action(connector,ticket,link,None,key.map(|key|(key,None)),Some(groups))
    }

    fn link_with_action(&self, connector: &str, ticket_id: &str, link: Link, action: Option<&ActionGuard<'_>>, binding: Option<(&str,Option<&str>)>, owner_groups: Option<&[crate::model::Group]>) -> Result<(Item, bool), Error> {
        let ticket = TicketMetadata { key: link.key, title: link.title, url: link.url };
        metadata_valid(&ticket)?;
        if !hangar_core::ids::valid_session_name(&link.session) { return Err(Error::invalid("nome de sessão fora do formato")); }
        let active = self.mutation(connector, ticket_id, action)?;
        self.update(|file| {
            if let Some(groups)=owner_groups {
                let group=groups.iter().find(|group|group.members.contains(&link.session) && binding.is_some_and(|(key,_)|group.keys.get(&link.session).is_some_and(|current|current==key)));
                let members=group.map_or_else(||vec![link.session.clone()],|group|group.members.clone());
                for other in file.items.iter_mut().filter(|item|item.connector!=connector || item.ticket_id!=ticket_id) {
                    let same_life=members.iter().any(|name|other.sessions.contains(name) && (group.is_some() || binding.is_none_or(|(key,_)|other.session_keys.get(name).is_some_and(|stored|stored==key))));
                    if group.is_some_and(|group|other.group_ids.contains(&group.id)) || same_life {
                        if active.iter().any(|(source,ticket)|source==&other.connector && ticket==&other.ticket_id) {return Err(action_busy());}
                        other.sessions.retain(|name|!members.contains(name));
                        other.session_keys.retain(|name,_|!members.contains(name));
                        let old_ids=other.group_members.iter().filter(|(_,prior)|prior.iter().any(|name|members.contains(name))).map(|(id,_)|id.clone()).collect::<std::collections::HashSet<_>>();
                        other.group_ids.retain(|id|!old_ids.contains(id) && group.is_none_or(|group|id!=&group.id));
                        other.group_members.retain(|id,_|!old_ids.contains(id) && group.is_none_or(|group|id!=&group.id));
                        if other.session.as_ref().is_some_and(|name|members.contains(name)) {other.session=None;other.session_key=None;other.conversation=None;}
                    }
                }
            }
            for other in file.items.iter_mut().filter(|i| {
                let named=i.session.as_deref()==Some(&link.session) || i.sessions.contains(&link.session);
                named && binding.is_none_or(|(key,_)|i.session_keys.get(&link.session).is_none_or(|old|old==key))
                    || binding.is_some_and(|(key,_)|i.session_key.as_deref()==Some(key))
            }) {
                if action.is_some() && (other.connector!=connector || other.ticket_id!=ticket_id) {
                    return Err(Error::new(Kind::Gate,"ticket_session_assigned","a sessão já está associada a outro ticket"));
                }
                if (other.connector != connector || other.ticket_id != ticket_id)
                    && active.iter().any(|(c, t)| c == &other.connector && t == &other.ticket_id) { return Err(action_busy()); }
                if other.connector != connector || other.ticket_id != ticket_id {
                    other.sessions.retain(|session|session!=&link.session);
                    other.session_keys.remove(&link.session);
                    if other.session.as_deref()==Some(&link.session) {other.session=None;other.session_key=None;other.conversation=None;}
                }
            }
            let item = file.upsert(connector, ticket_id, ticket)?;
            if binding.is_some() && item.session_key.is_none() && item.group_ids.is_empty()
                && let Some(old)=item.session.clone().filter(|old|old!=&link.session && !item.session_keys.contains_key(old)) {
                item.sessions.retain(|member|member!=&old);
            }
            if !item.sessions.contains(&link.session) {item.sessions.push(link.session.clone());}
            let fresh = item.session.as_deref() != Some(link.session.as_str()) || binding.is_some_and(|(key,_)|item.session_key.as_deref()!=Some(key));
            if fresh {
                item.history.push(event(EventKind::Linked, "dono", None, Some(&link.session)));
                item.session = Some(link.session);
            }
            item.session_key=binding.map(|(key,_)|key.to_owned());
            if action.is_none() && binding.is_none() {item.session_keys.remove(item.session.as_ref().unwrap());}
            if let Some((key,_))=binding {item.session_keys.insert(item.session.clone().unwrap(),key.to_owned());}
            if let Some(group)=owner_groups.and_then(|groups|groups.iter().find(|group|item.session.as_ref().is_some_and(|name|group.members.contains(name))
                && binding.is_some_and(|(key,_)|item.session.as_ref().and_then(|name|group.keys.get(name)).is_some_and(|current|current==key)))) {
                if !item.group_ids.contains(&group.id) {item.group_ids.push(group.id.clone());}
                item.group_members.insert(group.id.clone(),group.members.clone());
                for name in &group.members {if !item.sessions.contains(name) {item.sessions.push(name.clone());}}
                item.session_keys.extend(group.keys.clone());
            }
            item.conversation=binding.and_then(|(_,conversation)|conversation.map(str::to_owned));
            Ok((item.clone(), fresh))
        })
    }

    /// Esquece o ticket, com as evidências dele.
    pub fn remove(&self, connector: &str, ticket_id: &str) -> Result<Item, Error> {
        let _active = self.mutation(connector, ticket_id, None)?;
        let item = self.update(|file| {
            let index = file.items.iter().position(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            Ok(file.items.remove(index))
        })?;
        for evidence in &item.evidence { self.drop_evidence(&evidence.id); }
        Ok(item)
    }

    pub fn owner_change(&self, connector: &str, ticket_id: &str, change: OwnerChange) -> Result<Item, Error> {
        if let OwnerChange::Repository { path, key, title, url } = &change {
            text("repository", path, 4096, false)?;
            if path.contains('\0') || path.trim() != path { return Err(Error::invalid("pasta inválida")); }
            let metadata = TicketMetadata { key:key.clone(), title:title.clone(), url:url.clone() };
            metadata_valid(&metadata)?;
            let _active = self.mutation(connector, ticket_id, None)?;
            return self.update(|file| {
                let item = file.upsert(connector,ticket_id,metadata)?;
                item.repository = Some(path.clone());
                Ok(item.clone())
            });
        }
        let _active = self.mutation(connector, ticket_id, None)?;
        self.update(|file| {
            let File { stages, items, .. } = file;
            let item = items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            // A etapa gravada vence; sem ela, a que a tela mostrava.
            let current = |item: &Item, shown: Option<&str>| -> Result<Option<usize>, Error> {
                match item.stage.as_deref().or(shown) {
                    Some(id) => stage_index(stages, id).map(Some),
                    None => Ok(None),
                }
            };
            match change {
                OwnerChange::Stage { stage } => {
                    stage_index(stages, &stage)?;
                    item.history.push(event(EventKind::Stage, "dono", Some(&stage), None));
                    (item.stage, item.delivered_at) = (Some(stage), None);
                }
                OwnerChange::Approve { stage } => {
                    let Some(index) = current(item, stage.as_deref())?.filter(|i| stages[*i].gate) else {
                        return Err(Error::invalid("o ticket não está numa etapa com portão"));
                    };
                    let stage = &stages[index];
                    if !item.approved.contains(&stage.id) { item.approved.push(stage.id.clone()); }
                    item.history.push(event(EventKind::Approved, "dono", Some(&stage.id), None));
                    let next = stages.get(index + 1).unwrap_or(stage);
                    if next.id != stage.id { item.history.push(event(EventKind::Stage, "dono", Some(&next.id), None)); }
                    item.stage = Some(next.id.clone());
                }
                OwnerChange::Changes { note, stage } => {
                    text("note", &note, MAX_TEXT, false)?;
                    let Some(index) = current(item, stage.as_deref())? else { return Err(Error::invalid("o ticket não tem etapa para recusar")) };
                    let back = index.saturating_sub(1);
                    reopen_from(stages, item, back);
                    item.history.push(event(EventKind::Changes, "dono", Some(&stages[index].id), Some(&note)));
                    (item.stage, item.delivered_at) = (Some(stages[back].id.clone()), None);
                }
                OwnerChange::Delivered { note } => {
                    if let Some(note) = &note { text("note", note, MAX_TEXT, true)?; }
                    if let Some(gate) = pending_gate(stages, item, stages.len()) { return Err(Error::gate(gate)); }
                    item.history.push(event(EventKind::Delivered, "dono", None, note.as_deref()));
                    item.delivered_at = Some(now());
                }
                OwnerChange::Reset => { (item.stage, item.delivered_at) = (None, None); }
                OwnerChange::Repository { .. } => unreachable!("repositório já tratado"),
            }
            Ok(item.clone())
        })
    }

    /// Relato da sessão: etapa, nota e evidências. O agente não passa de etapa com portão sem aprovação, e voltar
    /// para antes de um portão o fecha de novo.
    pub fn report(&self, session: &str, report: Report) -> Result<Item, Error> {
        let linked = self.read()?.items.into_iter().find(|i| i.session.as_deref() == Some(session)).ok_or_else(Error::not_linked)?;
        self.report_for(&linked.connector, &linked.ticket_id, session, report)
    }

    /// O servidor confere o grupo antes de aceitar o relato de outro participante.
    pub fn report_for(&self, connector: &str, ticket_id: &str, session: &str, report: Report) -> Result<Item, Error> {
        self.report_imported(connector,ticket_id,session,report,Vec::new(),None)
    }

    pub fn report_imported(&self, connector: &str, ticket_id: &str, session: &str, report: Report, mut imported: Vec<ImportedEvidence>, binding: Option<ReportBinding>) -> Result<Item,Error> {
        if let Some(note) = &report.note { text("note", note, MAX_TEXT, true)?; }
        for (_, caption) in &report.evidence { text("caption", caption, 500, true)?; }
        // Antes de copiar: sessão sem ticket ou ticket cheio não ocupam disco.
        let linked = self.read()?.items.into_iter().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::not_linked)?;
        let binding=binding.unwrap_or_else(||ReportBinding {session_key:linked.session_keys.get(session).cloned(),history_revision:linked.history.len()});
        if linked.evidence.len() + report.evidence.len() + imported.len() > MAX_EVIDENCE { return Err(Error::too_much_evidence()); }
        let mut staged = Staged { store: self, ids: Vec::new(), kept: false };
        let mut copies = imported.iter().map(|entry|Evidence {by:session.to_owned(),..entry.evidence.clone()}).collect::<Vec<_>>();
        for (path, caption) in &report.evidence {
            let evidence = self.stage_evidence(path)?;
            staged.ids.push(evidence.id.clone());
            copies.push(Evidence { caption: caption.clone(), by: session.to_owned(), ..evidence });
        }
        let item = self.update(|file| {
            let File { stages, items, .. } = file;
            let item = items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::not_linked)?;
            if item.session.as_deref()!=Some(session) && !item.sessions.iter().any(|member|member==session)
                || binding.session_key.as_ref().is_some_and(|key|item.session_keys.get(session)!=Some(key)) {
                return Err(Error::new(Kind::Gate,"ticket_association_changed","a associação mudou durante o relato; confira o ticket novamente"));
            }
            if report.stage.is_some() && report.stage!=item.stage && item.history.get(binding.history_revision..).is_none_or(|events|
                events.iter().any(|event|matches!(event.kind,EventKind::Stage|EventKind::Approved|EventKind::Changes|EventKind::Delivered))) {
                return Err(Error::new(Kind::Gate,"ticket_stage_changed","a etapa mudou durante o relato; consulte o ticket antes de repetir"));
            }
            let note = report.note.as_deref().filter(|n| !n.trim().is_empty());
            match &report.stage {
                Some(stage) if item.stage.as_deref() != Some(stage.as_str()) => {
                    let target = stage_index(stages, stage)?;
                    if let Some(gate) = pending_gate(stages, item, target) { return Err(Error::gate(gate)); }
                    reopen_from(stages, item, target);
                    item.history.push(event(EventKind::Stage, session, Some(stage), note));
                    item.stage = Some(stage.clone());
                }
                _ => if let Some(note) = note { item.history.push(event(EventKind::Note, session, item.stage.as_deref(), Some(note))); },
            }
            if item.evidence.len() + copies.len() > MAX_EVIDENCE { return Err(Error::too_much_evidence()); }
            for evidence in copies {
                item.history.push(event(EventKind::Evidence, session, item.stage.as_deref(), Some(&evidence.name)));
                item.evidence.push(evidence);
            }
            Ok(item.clone())
        })?;
        staged.kept = true;
        for entry in &mut imported {entry.kept=true;}
        Ok(item)
    }

    pub fn import_evidence(&self,name: &str,bytes: &[u8],caption: &str) -> Result<ImportedEvidence,Error> {
        text("caption",caption,500,true)?;
        let mut evidence=self.store_bytes(name,bytes)?;
        evidence.caption=caption.to_owned();
        Ok(ImportedEvidence {path:self.evidence_path(&evidence.id),evidence,kept:false})
    }

    pub fn source_file(&self,path: &Path) -> Result<(std::fs::File,String,String),Error> {
        if !path.is_absolute() {return Err(Error::invalid("caminho precisa ser absoluto"));}
        let file=std::fs::File::open(path).map_err(|error|Error::io("leitura da evidência",&error))?;
        let meta=file.metadata().map_err(|error|Error::io("leitura da evidência",&error))?;
        let name=path.file_name().and_then(|name|name.to_str()).ok_or_else(||Error::invalid("nome inválido"))?;
        let mime=evidence_mime(name).ok_or_else(||Error::invalid("só imagem, vídeo, PDF ou texto"))?;
        if !meta.is_file() || meta.len()>MAX_EVIDENCE_BYTES {return Err(Error::invalid("evidência inválida ou maior que 512 MiB"));}
        Ok((file,name.to_owned(),mime.to_owned()))
    }

    /// Evidência enviada pelo dono (bytes já lidos), no ticket.
    pub fn add_owner_evidence(&self, connector: &str, ticket_id: &str, name: &str, bytes: &[u8], caption: &str) -> Result<Item, Error> {
        text("caption", caption, 500, true)?;
        let evidence = self.store_bytes(name, bytes)?;
        let mut staged = Staged { store: self, ids: vec![evidence.id.clone()], kept: false };
        let item = self.update(|file| {
            let item = file.item_mut(connector, ticket_id)?;
            if item.evidence.len() >= MAX_EVIDENCE { return Err(Error::too_much_evidence()); }
            item.history.push(event(EventKind::Evidence, "dono", item.stage.as_deref(), Some(&evidence.name)));
            item.evidence.push(Evidence { caption: caption.to_owned(), by: "dono".into(), ..evidence });
            Ok(item.clone())
        })?;
        staged.kept = true;
        Ok(item)
    }

    pub fn remove_evidence(&self, connector: &str, ticket_id: &str, id: &str) -> Result<Item, Error> {
        let item = self.update(|file| {
            let item = file.item_mut(connector, ticket_id)?;
            let index = item.evidence.iter().position(|e| e.id == id).ok_or_else(Error::evidence_not_found)?;
            item.evidence.remove(index);
            Ok(item.clone())
        })?;
        self.drop_evidence(id);
        Ok(item)
    }

    /// Arquivo e tipo de uma evidência registrada. O id só vira caminho depois de achado no registro.
    pub fn evidence_file(&self, id: &str) -> Result<(PathBuf, Evidence), Error> {
        let file = self.read()?;
        let evidence = file.items.iter().flat_map(|i| i.evidence.iter()).find(|e| e.id == id).cloned().ok_or_else(Error::evidence_not_found)?;
        Ok((self.evidence_path(id), evidence))
    }

    fn evidence_path(&self, id: &str) -> PathBuf { self.evidence_dir.join(id) }

    fn drop_evidence(&self, id: &str) { let _ = std::fs::remove_file(self.evidence_path(id)); }

    fn new_evidence(&self, name: &str, size: u64) -> Result<Evidence, Error> {
        let mime = evidence_mime(name).ok_or_else(|| Error::invalid(format!("{name}: só imagem, vídeo, PDF ou texto")))?;
        if size > MAX_EVIDENCE_BYTES { return Err(Error::invalid(format!("{name}: acima de {} MiB", MAX_EVIDENCE_BYTES / 1024 / 1024))); }
        std::fs::create_dir_all(&self.evidence_dir).map_err(|e| Error::io("pasta das evidências", &e))?;
        Ok(Evidence { id: hangar_core::ids::random_hex(12), name: name.to_owned(), mime: mime.to_owned(), size, caption: String::new(), at: now(), by: String::new() })
    }

    /// Arquivo novo da evidência; nunca sobrescreve outro.
    fn create(&self, evidence: &Evidence) -> Result<std::fs::File, Error> {
        std::fs::OpenOptions::new().write(true).create_new(true).open(self.evidence_path(&evidence.id))
            .map_err(|e| Error::io("arquivo da evidência", &e))
    }

    /// Copia o arquivo da sessão para a pasta das evidências: apagar o original depois não tira a prova.
    fn stage_evidence(&self, path: &Path) -> Result<Evidence, Error> {
        if !path.is_absolute() { return Err(Error::invalid(format!("{}: caminho precisa ser absoluto", path.display()))); }
        let meta = std::fs::metadata(path).map_err(|e| Error::invalid(format!("{}: {:?}", path.display(), e.kind())))?;
        if !meta.is_file() { return Err(Error::invalid(format!("{}: não é arquivo", path.display()))); }
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::invalid("nome de arquivo inválido"))?;
        let evidence = self.new_evidence(name, meta.len())?;
        let mut target = self.create(&evidence)?;
        let copied = std::fs::File::open(path).and_then(|mut source| io::copy(&mut source, &mut target));
        if let Err(error) = copied { self.drop_evidence(&evidence.id); return Err(Error::io("cópia da evidência", &error)); }
        Ok(evidence)
    }

    fn store_bytes(&self, name: &str, bytes: &[u8]) -> Result<Evidence, Error> {
        let name = Path::new(name).file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::invalid("nome de arquivo inválido"))?;
        text("name", name, 240, false)?;
        let evidence = self.new_evidence(name, bytes.len() as u64)?;
        let written = self.create(&evidence).and_then(|mut file| io::Write::write_all(&mut file, bytes).map_err(|e| Error::io("gravação da evidência", &e)));
        if let Err(error) = written { self.drop_evidence(&evidence.id); return Err(error); }
        Ok(evidence)
    }

    fn read(&self) -> Result<File, Error> {
        let bytes = match hangar_core::fs::read_replaced(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(File::default()),
            Err(error) => return Err(Error::io("leitura dos tickets", &error)),
        };
        let mut file: File = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new(Kind::Unavailable, "mission_unreadable", "mission.json ilegível"))?;
        if matches!(file.version,1|2) { file.version = FORMAT_VERSION; }
        if file.version != FORMAT_VERSION {
            return Err(Error::new(Kind::Unavailable, "mission_unreadable", format!("mission.json na versão {} desconhecida", file.version)));
        }
        Ok(file)
    }

    /// Lê, muda e regrava sob a trava; mudança recusada não grava nada.
    fn update<T>(&self, change: impl FnOnce(&mut File) -> Result<T, Error>) -> Result<T, Error> {
        let parent = self.path.parent().ok_or_else(|| Error::new(Kind::Unavailable, "mission_unavailable", "sem pasta"))?;
        std::fs::create_dir_all(parent).map_err(|e| Error::io("pasta dos tickets", &e))?;
        let _lock = hangar_core::fs::lock_file(&self.path.with_extension("lock")).map_err(|e| Error::io("trava dos tickets", &e))?;
        let mut file = self.read()?;
        let value = change(&mut file)?;
        let bytes = serde_json::to_vec_pretty(&file).map_err(|_| Error::new(Kind::Unavailable, "mission_unavailable", "não serializou"))?;
        hangar_core::fs::atomic_write(&self.path, &bytes).map_err(|e| Error::io("gravação dos tickets", &e))?;
        Ok(value)
    }
}
