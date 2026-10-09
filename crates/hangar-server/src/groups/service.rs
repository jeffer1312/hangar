//! Aplica os planos de `local.rs` sob um lock por processo, restaurando o estado anterior quando
//! a escrita falha no meio. Só disco: nada aqui fala com outra máquina nem entrega recado.
use super::local::{self, JoinInput, JoinRefusal, Snapshot};
use super::model::Sidecar;
use super::store::{PairDir, StoreError};
use std::collections::{BTreeMap, hash_map::RandomState};
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrqPhase { Live, Ended, NotStarted, Unknown }

/// Fatos da orquestração, que moram no Python.
pub trait OrqFacts: Send + Sync {
    /// Falha em descobrir = `Unknown`.
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase>;
    fn promote<'a>(&'a self, name: &'a str, gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>>;
}

/// Python sem resposta não é conflito: "o arquivo mudou, recarregue" seria mentira.
#[derive(Debug, PartialEq, Eq)]
pub enum PromoteError {
    /// Texto do 409 do Python.
    Conflict(String),
    /// Código do motivo (prazo, 5xx, rota ausente).
    Unavailable(String),
}

#[derive(Debug)]
pub enum GroupError { Refused(JoinRefusal), Orq(PromoteError), Store(StoreError) }
impl From<StoreError> for GroupError { fn from(e: StoreError) -> Self { GroupError::Store(e) } }
impl std::fmt::Display for GroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupError::Refused(r) => write!(f, "refused: {r:?}"),
            GroupError::Orq(e) => write!(f, "orq: {e:?}"),
            GroupError::Store(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for GroupError {}

pub struct JoinOwned {
    pub name: String,
    pub others: Vec<String>,
    pub task: String,
    pub replace_task: bool,
    pub harness: BTreeMap<String, String>,
    pub orq: bool,
}

pub struct JoinOutcome {
    pub members: Vec<String>,
    pub gid: String,
    pub task: String,
    pub orq: bool,
    /// Membros locais que estavam soltos: só eles recebem o protocolo.
    pub newcomers: Vec<String>,
    pub before: Snapshot,
}

pub struct GroupService {
    dir: Arc<PairDir>,
    lock: Mutex<()>,
    orq: Arc<dyn OrqFacts>,
    server_id: String,
    /// Avisa a lista de que o disco do grupo mudou: sem isso ela só relê no prazo da descoberta.
    on_change: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl GroupService {
    pub fn new(dir: PairDir, orq: Arc<dyn OrqFacts>, server_id: String) -> Self {
        Self { dir: Arc::new(dir), lock: Mutex::new(()), orq, server_id, on_change: None }
    }

    pub fn with_change_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.on_change = Some(hook);
        self
    }

    fn changed(&self) {
        if let Some(hook) = &self.on_change { hook() }
    }

    pub fn server_id(&self) -> &str { &self.server_id }

    /// Sidecar de `name` sem o lock, como o `PairLink.get`: leitura de quem só consulta.
    pub async fn link(&self, name: &str) -> Result<Option<Sidecar>, GroupError> {
        let (dir, name) = (self.dir.clone(), name.to_owned());
        blocking(move || reader(&dir)(&name)).await
    }

    /// Todos os sidecars válidos (stem, sidecar), sem o lock: leitura de quem só consulta.
    pub async fn sidecars(&self) -> Result<Vec<(String, Sidecar)>, GroupError> {
        let dir = self.dir.clone();
        Ok(blocking(move || dir.sidecars()).await??)
    }

    pub fn contract_path(&self, gid: &str) -> std::path::PathBuf { self.dir.contract_path(gid) }

    /// A pasta `.hangar-pair`, onde o Python também guarda `external_pairs.json`.
    pub fn pair_root(&self) -> &std::path::Path { self.dir.root() }

    pub async fn join(&self, input: JoinOwned) -> Result<JoinOutcome, GroupError> {
        let _guard = self.lock.lock().await;
        let dir = self.dir.clone();
        let name = input.name.clone();
        let plan = blocking(move || -> Result<local::JoinPlan, GroupError> {
            let read = reader(&dir);
            let join = JoinInput { name: &input.name, others: &input.others, task: &input.task, replace_task: input.replace_task, harness: &input.harness, orq: input.orq };
            let plan = local::plan_join(&read, &join, &fresh_gid).map_err(GroupError::Refused)?;
            for loser in &plan.merged { dir.merge_contract(loser, &plan.gid); }
            if let Err(e) = apply(&dir, &[], &plan.writes) {
                restore_logged(&dir, &plan.before);
                return Err(e.into());
            }
            Ok(plan)
        }).await??;
        // Promover sob o mesmo lock: um join concorrente não enxerga o grupo meio configurado.
        if plan.orq && plan.new_gid && let Err(failed) = self.orq.promote(&name, &plan.gid).await {
            let restored = self.restore_blocking(plan.before.clone()).await;
            self.changed();
            // Sidecar `orq` que não voltou pesa mais que a promoção: 409/503 diria que nada ficou.
            restored?;
            return Err(GroupError::Orq(failed));
        }
        self.changed();
        let newcomers = plan.members.iter().filter(|m| matches!(plan.before.get(*m), Some(None))).cloned().collect();
        Ok(JoinOutcome { members: plan.members, gid: plan.gid, task: plan.task, orq: plan.orq, newcomers, before: plan.before })
    }

    /// Devolve os ex-companheiros. O contrato é arquivado depois de soltar o lock: faxina não
    /// desfaz uma saída que já valeu.
    pub async fn leave(&self, name: &str) -> Result<Vec<String>, GroupError> {
        let (peers, archive, wrote) = {
            let _guard = self.lock.lock().await;
            let dir = self.dir.clone();
            let owner = name.to_owned();
            // A fase só interessa a sidecar `orq`; `Live` e `Unknown` contam como viva.
            let link = blocking({ let (dir, owner) = (dir.clone(), owner.clone()); move || reader(&dir)(&owner) }).await?;
            let alive = match link {
                Some(s) if s.orq => matches!(self.orq.phase(&s.gid).await, OrqPhase::Live | OrqPhase::Unknown),
                _ => false,
            };
            blocking(move || -> Result<(Vec<String>, Option<String>, bool), GroupError> {
                let Some(plan) = local::plan_leave(&reader(&dir), &owner, alive) else { return Ok((Vec::new(), None, false)) };
                if let Err(e) = apply(&dir, &plan.clears, &plan.writes) {
                    restore_logged(&dir, &plan.before);
                    return Err(GroupError::from(e));
                }
                Ok((plan.ex_peers, plan.archive, true))
            }).await??
        };
        if wrote { self.changed(); }
        if let Some(gid) = archive {
            let dir = self.dir.clone();
            blocking(move || dir.archive_contracts(&gid)).await?;
        }
        Ok(peers)
    }

    /// Grupo `orq` de um membro só vive enquanto a execução dele vive (`pair.dissolve_lone_orq`):
    /// acabada, ou nunca iniciada depois de `grace` sem escrita, o sidecar sai e o contrato vai para
    /// o arquivo. Devolve os stems dissolvidos. Falha numa sidecar vai ao diário e não para as outras.
    pub async fn dissolve_lone_orq(&self, grace: std::time::Duration) -> Result<Vec<String>, GroupError> {
        let mut dissolved = Vec::new();
        {
            let _guard = self.lock.lock().await;
            let dir = self.dir.clone();
            let lone = blocking(move || dir.sidecars()).await??;
            for (stem, sidecar) in lone.into_iter().filter(|(_, s)| s.peers.is_empty() && s.orq) {
                let phase = self.orq.phase(&sidecar.gid).await;
                let (d, s) = (self.dir.clone(), stem.clone());
                let Ok(Ok(modified)) = blocking(move || d.sidecar_modified(&s)).await else { continue };
                // Relógio atrás do arquivo conta como recém-escrito.
                let young = std::time::SystemTime::now().duration_since(modified).unwrap_or_default() < grace;
                if matches!(phase, OrqPhase::Live | OrqPhase::Unknown) || (phase == OrqPhase::NotStarted && young) { continue; }
                let (d, s) = (self.dir.clone(), stem.clone());
                match blocking(move || d.clear_sidecar(&s)).await? {
                    Ok(()) => dissolved.push((stem, sidecar.gid)),
                    Err(error) => tracing::warn!(code = "groups_lone_orq_clear_failed", name = %stem, %error, "groups: o grupo orq sozinho não foi dissolvido"),
                }
            }
        }
        if !dissolved.is_empty() { self.changed(); }
        // Arquivar é faxina depois do lock, como na saída.
        let mut names = Vec::new();
        for (stem, gid) in dissolved {
            let dir = self.dir.clone();
            blocking(move || dir.archive_contracts(&gid)).await?;
            names.push(stem);
        }
        Ok(names)
    }

    pub async fn rename(&self, old: &str, new: &str) -> Result<(), GroupError> {
        let _guard = self.lock.lock().await;
        let (dir, old, new) = (self.dir.clone(), old.to_owned(), new.to_owned());
        let done = blocking(move || {
            let read = reader(&dir);
            let (clears, writes) = local::plan_rename(&read, &old, &new);
            let before: Snapshot = clears.iter().chain(writes.iter().map(|(n, _)| n)).map(|n| (n.clone(), read(n))).collect();
            apply(&dir, &clears, &writes).map_err(|e| { restore_logged(&dir, &before); GroupError::from(e) })
        }).await?;
        self.changed();
        done
    }

    /// Volta cada sidecar ao estado do snapshot (um join que não pôde ser concluído).
    pub async fn restore(&self, before: Snapshot) -> Result<(), GroupError> {
        let _guard = self.lock.lock().await;
        let dir = self.dir.clone();
        let done = blocking(move || restore_all(&dir, &before)).await?.map_err(GroupError::from);
        self.changed();
        done
    }

    pub async fn external_link(&self, local: &str, address: &str, harness: BTreeMap<String, String>) -> Result<String, GroupError> {
        let _guard = self.lock.lock().await;
        let (dir, local, address) = (self.dir.clone(), local.to_owned(), address.to_owned());
        let gid = blocking(move || -> Result<String, GroupError> {
            let writes = local::plan_external_link(&reader(&dir), &local, &address, &harness, &fresh_gid).map_err(GroupError::Refused)?;
            apply(&dir, &[], &writes)?;
            Ok(writes.into_iter().next().map(|(_, s)| s.gid).unwrap_or_default())
        }).await??;
        self.changed();
        Ok(gid)
    }

    /// Tira `address` da lista de `local`; sem mais ninguém (e sem orq), o grupo some e o contrato
    /// vai para o arquivo.
    pub async fn external_unlink(&self, local: &str, address: &str) -> Result<(), GroupError> {
        let archive = {
            let _guard = self.lock.lock().await;
            let (dir, local, address) = (self.dir.clone(), local.to_owned(), address.to_owned());
            blocking(move || {
                let Some(mut st) = reader(&dir)(&local) else { return Ok(None) };
                if !st.peers.contains(&address) { return Ok(None); }
                st.peers.retain(|p| *p != address);
                if st.peers.is_empty() && !st.orq {
                    dir.clear_sidecar(&local)?;
                    return Ok(Some(st.gid));
                }
                dir.write_sidecar(&local, &st)?;
                Ok::<_, GroupError>(None)
            }).await??
        };
        self.changed();
        if let Some(gid) = archive {
            let dir = self.dir.clone();
            blocking(move || dir.archive_contracts(&gid)).await?;
        }
        Ok(())
    }

    /// `work` sob o lock de grupo: a associação do time do `orq`, que mexe no grupo pelo Python.
    pub async fn locked<T>(&self, work: impl Future<Output = T>) -> T {
        let _guard = self.lock.lock().await;
        work.await
    }

    /// `restore` de quem já segura o lock.
    async fn restore_blocking(&self, before: Snapshot) -> Result<(), GroupError> {
        let dir = self.dir.clone();
        blocking(move || restore_all(&dir, &before)).await?.map_err(GroupError::from)
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, GroupError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| GroupError::Store(StoreError::Io(std::io::Error::other(e))))
}

/// Sidecar ilegível ou torto vale como sem grupo e vai ao diário.
fn reader(dir: &PairDir) -> impl Fn(&str) -> Option<Sidecar> + '_ {
    move |name| match dir.sidecar(name) {
        Ok(s) => s,
        Err(error) => {
            if crate::warn_limit::allow(Some(name), "groups_sidecar_unreadable") {
                tracing::warn!(code = "groups_sidecar_unreadable", name, %error, "groups: sidecar de grupo ilegível ou torto, tratado como sem grupo");
            }
            None
        }
    }
}

fn apply(dir: &PairDir, clears: &[String], writes: &[(String, Sidecar)]) -> Result<(), StoreError> {
    for name in clears { dir.clear_sidecar(name)?; }
    for (name, sidecar) in writes { dir.write_sidecar(name, sidecar)?; }
    Ok(())
}

/// Tenta todos mesmo que um falhe: parar no primeiro deixaria o resto assimétrico. Devolve o primeiro erro.
fn restore_all(dir: &PairDir, before: &Snapshot) -> Result<(), StoreError> {
    let mut first = None;
    for (name, state) in before {
        let done = match state {
            None => dir.clear_sidecar(name),
            Some(s) => dir.write_sidecar(name, s),
        };
        if let Err(error) = done {
            tracing::warn!(code = "groups_restore_failed", name, %error, "groups: o sidecar não voltou ao estado anterior");
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Restauração de quem já está devolvendo outro erro: o erro original é o que importa ao chamador.
fn restore_logged(dir: &PairDir, before: &Snapshot) { let _ = restore_all(dir, before); }

/// 8 hex como o `uuid4().hex[:8]` do Python; `RandomState` é semeado pelo sistema, sem dependência nova.
fn fresh_gid() -> String { format!("{:08x}", RandomState::new().build_hasher().finish() as u32) }
