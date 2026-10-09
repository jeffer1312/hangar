//! Membro de grupo cuja sessão morreu fora do app (`registry._varrer_pares_mortos`) e grupo `orq`
//! sozinho sem execução (`pair.dissolve_lone_orq`). Morto = ausente da lista viva há pelo menos
//! `MIN_ABSENCE`: `kill` e `rename` deixam o nome ausente de propósito por um instante, e só o
//! tempo separa isso de morte. Lista com erro, vazia ou sem fatos nunca varre.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::exit::leave_and_notify;
use super::service::{BoxFuture, GroupError, GroupService};
use super::store::file_stem;
use crate::list::bridge::Produced;
use crate::routes::AppState;

pub const TICK: Duration = Duration::from_secs(2);
pub const MIN_ABSENCE: Duration = Duration::from_secs(5);
/// Do `--pair --orq` ao `execucao_inicio` o árbitro escreve contrato e roda o `orq init`.
pub const ORQ_LAUNCH_GRACE: Duration = Duration::from_secs(3600);

pub const FAILED_EVENT: &str = "rust.groups_sweep_failed";
pub const RECOVERED_EVENT: &str = "rust.groups_sweep_recovered";
pub const PANICKED_EVENT: &str = "rust.groups_sweep_panicked";
const PANICKED: &str = "groups_sweep_panicked";
const EMPTY_LIST: &str = "list_empty";

/// O que a varredura pede ao resto do servidor; o teste troca por um falso.
pub trait SweepEnv: Send + Sync {
    /// Nomes crus da lista viva. `Err` = código do motivo.
    fn live_names(&self) -> BoxFuture<'_, Result<Vec<String>, String>>;
    /// Sai do grupo e avisa as outras máquinas; devolve os ex-companheiros.
    fn leave<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, GroupError>>;
    /// Uma vez por sequência de falhas (`FAILED_EVENT`), uma na volta (`RECOVERED_EVENT`) e uma por
    /// rodada em pânico (`PANICKED_EVENT`).
    fn report(&self, event: &'static str, code: &str);
}

pub struct Sweeper {
    groups: Arc<GroupService>,
    env: Arc<dyn SweepEnv>,
    absent: HashMap<String, Instant>,
    failing: Option<String>,
}

impl Sweeper {
    pub fn new(groups: Arc<GroupService>, env: Arc<dyn SweepEnv>) -> Self {
        Self { groups, env, absent: HashMap::new(), failing: None }
    }

    /// A rodada numa tarefa própria: com o laço do Python desligado, um pânico parava a varredura
    /// de vez e calado. Vai ao diário e a próxima rodada recomeça a contagem das ausências.
    pub async fn guarded_round(mut self) -> Self {
        let (groups, env) = (self.groups.clone(), self.env.clone());
        match tokio::spawn(async move { self.round().await; self }).await {
            Ok(sweeper) => sweeper,
            Err(error) => {
                tracing::error!(code = PANICKED, %error, "groups: a rodada da varredura de grupos entrou em pânico");
                env.report(PANICKED_EVENT, PANICKED);
                Sweeper::new(groups, env)
            }
        }
    }

    /// Uma rodada. Sem sidecar nenhum não pergunta a lista.
    pub async fn round(&mut self) {
        let mut sidecars = match self.groups.sidecars().await {
            Ok(found) => found,
            Err(error) => return warn_once("groups_sweep_read_failed", &error),
        };
        if sidecars.is_empty() {
            self.absent.clear();
            return;
        }
        if sidecars.iter().any(|(_, s)| s.peers.is_empty() && s.orq) {
            match self.groups.dissolve_lone_orq(ORQ_LAUNCH_GRACE).await {
                Ok(done) if done.is_empty() => {}
                Ok(done) => {
                    tracing::info!(code = "groups_lone_orq_dissolved", sessions = ?done, "groups: grupo orq sem execução viva dissolvido");
                    sidecars = match self.groups.sidecars().await {
                        Ok(found) => found,
                        Err(error) => return warn_once("groups_sweep_read_failed", &error),
                    };
                }
                Err(error) => warn_once("groups_lone_orq_failed", &error),
            }
        }
        let referenced = referenced_locals(&sidecars);
        if referenced.is_empty() {
            self.absent.clear();
            return;
        }
        let live = match self.env.live_names().await {
            Ok(names) if names.is_empty() => return self.failed(EMPTY_LIST),
            Ok(names) => names,
            Err(code) => return self.failed(&code),
        };
        if let Some(code) = self.failing.take() {
            self.env.report(RECOVERED_EVENT, &code);
        }
        // O stem do sidecar é saneado e a lista traz o nome cru: tira os dois, ou uma sessão
        // com espaço no nome seria dada como morta.
        let alive: HashSet<String> = live.iter().cloned().chain(live.iter().map(|n| file_stem(n))).collect();
        let candidates: Vec<String> = referenced.into_iter().filter(|n| !alive.contains(n)).collect();
        self.absent.retain(|n, _| candidates.contains(n));
        let now = Instant::now();
        for name in candidates {
            let first = *self.absent.entry(name.clone()).or_insert(now);
            if now.duration_since(first) < MIN_ABSENCE { continue; }
            self.absent.remove(&name);
            match self.env.leave(&name).await {
                Ok(ex) => tracing::info!(code = "groups_dead_member_left", session = %name, peers = ?ex, "groups: sessão morta fora do app saiu do grupo"),
                Err(error) => tracing::warn!(code = "groups_dead_member_leave_failed", session = %name, %error, "groups: sessão morta fora do app não saiu do grupo"),
            }
        }
    }

    /// A lista que falha não vira "ninguém vivo": a rodada não varre, e o diário ganha uma linha por sequência.
    fn failed(&mut self, code: &str) {
        if self.failing.as_deref() != Some(code) {
            tracing::warn!(code, "groups: lista de sessões indisponível, os grupos ficam");
            self.env.report(FAILED_EVENT, code);
        }
        self.failing = Some(code.to_owned());
    }
}

/// Nomes locais que têm sidecar ou aparecem como companheiro em algum. O dono entra porque num par
/// entre máquinas ele é o único local (o outro é `srv::x`).
fn referenced_locals(sidecars: &[(String, super::model::Sidecar)]) -> HashSet<String> {
    let mut out = HashSet::new();
    for (stem, sidecar) in sidecars {
        out.insert(stem.clone());
        out.extend(sidecar.peers.iter().filter(|p| !super::local::is_remote(p)).cloned());
    }
    out
}

fn warn_once(code: &'static str, error: &GroupError) {
    if crate::warn_limit::allow(None, code) {
        tracing::warn!(code, %error, "groups: a varredura de grupos não rodou");
    }
}

/// Nomes vivos do retrato da lista. Sem nenhuma resposta dos fatos, a sessão vista só por eles
/// (transferência, `orq`) é desconhecida, não ausente: a rodada falha, como na ponte da lista.
pub fn live_names_of(produced: &Produced) -> Result<Vec<String>, String> {
    if produced.facts.unknown {
        return Err("list_facts_unknown".to_owned());
    }
    Ok(produced.rows.iter().map(|row| row.name.clone()).collect())
}

struct AppEnv { st: Arc<AppState>, groups: Arc<GroupService> }

impl SweepEnv for AppEnv {
    fn live_names(&self) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async move {
            // O retrato, não a descoberta: as linhas de transferência e as `orq` vêm dos fatos.
            let produced = self.st.list.snapshot().await.map_err(|e| e.code.to_owned())?;
            live_names_of(&produced)
        })
    }

    fn leave<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, GroupError>> {
        Box::pin(async move {
            let (ex, warnings) = leave_and_notify(&self.st, &self.groups, name).await?;
            if !warnings.is_empty() {
                tracing::warn!(code = "groups_dead_member_peer_not_notified", session = name, count = warnings.len(), "groups: a saída não avisou todos os ex-companheiros de fora");
            }
            Ok(ex)
        })
    }

    fn report(&self, event: &'static str, code: &str) {
        let reason = match event {
            FAILED_EVENT => "lista de sessões indisponível; a varredura de grupos não rodou",
            PANICKED_EVENT => "a rodada da varredura de grupos entrou em pânico; a próxima recomeça",
            _ => "a lista voltou; a varredura de grupos rodou",
        };
        self.st.diag.report(event, "", code, reason);
    }
}

/// A tarefa de 2 s. `None` sem serviço de grupos (as rotas e a varredura seguem no Python).
pub fn spawn(st: Arc<AppState>) -> Option<JoinHandle<()>> {
    let groups = st.groups.clone()?;
    Some(tokio::spawn(async move {
        let mut sweeper = Sweeper::new(groups.clone(), Arc::new(AppEnv { st, groups }));
        loop {
            tokio::time::sleep(TICK).await;
            sweeper = sweeper.guarded_round().await;
        }
    }))
}
