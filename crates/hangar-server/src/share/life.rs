//! Vida da sessão (`share_life.py`) e veredito do convite (`share_gate._verdict`).
//!
//! Não depende do `ListHub`: ele só roda com lista aberta, e o convidado chega sem ela.
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde_json::Value;

use super::store::{Guest, Share, ShareStore};
use crate::list::links::session_life;
use crate::state::capture::MuxProcess;
use crate::state::parked::{codex_sidecar_path, sidecar_path};

// ponytail: cache de 2 s por nome; cada consulta é um processo do tmux e o chat faz vários
// pedidos por segundo. Revogar vale na hora: o registro é lido a cada pedido.
const LIFE_TTL: Duration = Duration::from_secs(2);
const MUX_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict { Ok, Ended, Unsure }

#[derive(Default)]
struct FactsState {
    lives: HashMap<String, String>,
    changing: HashSet<String>,
    uncertain: HashSet<String>,
    snapshot: bool,
}

/// Identidade conservada pelo ciclo de vida e pela recuperação das transferências no Rust.
#[derive(Default)]
pub struct LifeFacts { state: Mutex<FactsState> }

impl LifeFacts {
    fn lock(&self) -> std::sync::MutexGuard<'_, FactsState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Vida de origem de uma transferência em curso; `None` quando ela termina.
    pub fn set_life(&self, name: &str, life: Option<String>) {
        let mut s = self.lock();
        match life {
            Some(life) => { s.lives.insert(name.into(), life); }
            None => { s.lives.remove(name); }
        }
    }
    pub fn begin_mode_change(&self, name: &str) {
        self.lock().changing.insert(name.into());
    }
    pub fn end_mode_change(&self, name: &str) {
        self.lock().changing.remove(name);
    }
    /// Fotografia completa dos registros duráveis, conferida na subida.
    pub fn apply_snapshot(&self, lives: HashMap<String, String>, changing: HashSet<String>) {
        let mut state=self.lock();
        state.lives=lives;state.changing=changing;state.snapshot=true;
    }
    /// A recuperação da subida conserva somente os convites cuja transferência não pôde ser lida.
    pub fn apply_transfer_snapshot(&self,lives:HashMap<String,String>,uncertain:HashSet<String>) {
        let mut state=self.lock();state.lives=lives;state.uncertain.extend(uncertain);state.snapshot=true;
    }
    /// A incerteza só sai depois de uma operação explícita conferir novamente o registro da sessão.
    pub fn set_transfer_uncertain(&self,name:&str,uncertain:bool) {
        let mut state=self.lock();
        if uncertain {state.uncertain.insert(name.into());} else {state.uncertain.remove(name);}
    }
    pub fn snapshot_received(&self) -> bool {
        self.lock().snapshot
    }
    pub fn changing_mode(&self, name: &str) -> bool {
        let state=self.lock();state.changing.contains(name) || state.uncertain.contains(name)
    }
    fn transfer_life(&self, name: &str) -> Option<String> {
        self.lock().lives.get(name).cloned()
    }
}

/// O que se pergunta ao multiplexador e aos sidecars; os testes trocam por um falso.
pub trait LifeProbe: Send + Sync {
    /// Nascimento do terminal; `None` também quando o multiplexador não respondeu.
    fn birth<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<u64>>;
    /// `None` = o multiplexador não respondeu, que não é sessão morta.
    fn has_session<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<bool>>;
    /// Chave do sidecar sem terminal ou Codex, nessa ordem, e se uma transferência o escreveu
    /// (`transfer_id`).
    fn sidecar(&self, name: &str) -> Option<(String, bool)>;
    fn sidecar_exists(&self, name: &str) -> bool;
}

pub struct MuxProbe { program: OsString, home: PathBuf, mux: MuxProcess }

impl MuxProbe {
    /// `home`: o `Path.home()` do Python, base de `~/.hangar/<sidecars>`.
    pub fn new(program: impl Into<OsString>, home: PathBuf) -> Self {
        let program = program.into();
        Self { mux: MuxProcess::new(program.clone(), MUX_TIMEOUT), program, home }
    }

    fn sidecar_paths(&self, name: &str) -> [PathBuf; 2] {
        [sidecar_path(name, &self.home), codex_sidecar_path(name, &self.home)]
    }
}

fn text(meta: &Value, field: &str) -> Option<String> {
    meta.get(field).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned)
}

impl LifeProbe for MuxProbe {
    fn birth<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move {
            let mut command = crate::terminal_input::child_command(&self.program);
            // `-t =nome` sem os dois pontos imprime linha vazia com rc 0 no tmux 3.x.
            command.args(["display-message", "-p", "-t", &format!("={name}:"), "#{session_created}"])
                .stdin(std::process::Stdio::null()).kill_on_drop(true);
            let out = tokio::time::timeout(MUX_TIMEOUT, command.output()).await.ok()?.ok()?;
            if !out.status.success() {
                return None;
            }
            let secs: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
            (secs.is_finite() && secs > 0.0).then_some(secs as u64)
        })
    }
    fn has_session<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<bool>> {
        Box::pin(self.mux.has_session(name))
    }
    /// O primeiro sidecar com chave, como o laço de `share_life.session_life`.
    fn sidecar(&self, name: &str) -> Option<(String, bool)> {
        self.sidecar_paths(name).iter().find_map(|path| {
            let meta: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            Some((text(&meta, "key")?, text(&meta, "transfer_id").is_some()))
        })
    }
    fn sidecar_exists(&self, name: &str) -> bool {
        self.sidecar_paths(name).iter().any(|p| p.exists())
    }
}

type Reread = Box<dyn Fn(&str) -> Option<Guest> + Send + Sync>;

pub struct Lives {
    probe: Arc<dyn LifeProbe>,
    facts: Arc<LifeFacts>,
    cache: Mutex<HashMap<String, (Instant, Probed)>>,
    reread: Option<Reread>,
}

/// O que o multiplexador e o disco disseram; a vida de origem da transferência entra sempre na hora.
#[derive(Clone)]
enum Probed { Key { key: String, transfer: bool }, Birth(Option<u64>) }

impl Lives {
    pub fn new(probe: Arc<dyn LifeProbe>, facts: Arc<LifeFacts>) -> Self {
        Self { probe, facts, cache: Mutex::default(), reread: None }
    }

    /// Releitura dos registros de um token na loja (pelo hash).
    pub fn rereading(mut self, reread: impl Fn(&str) -> Option<Guest> + Send + Sync + 'static) -> Self {
        self.reread = Some(Box::new(reread));
        self
    }

    pub fn facts(&self) -> &LifeFacts {
        &self.facts
    }

    pub async fn life(&self, name: &str, fresh: bool) -> Option<String> {
        let cached = if fresh { None } else {
            self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(name)
                .filter(|(at, _)| at.elapsed() < LIFE_TTL).map(|(_, probed)| probed.clone())
        };
        let probed = match cached {
            Some(probed) => probed,
            None => {
                let probed = match self.probe.sidecar(name) {
                    Some((key, transfer)) => Probed::Key { key, transfer },
                    None => Probed::Birth(self.probe.birth(name).await),
                };
                self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(name.into(), (Instant::now(), probed.clone()));
                probed
            }
        };
        // Ordem de `share_life.session_life`: a vida de origem só vence o sidecar que a própria
        // transferência escreveu, e só vale sem terminal; um nascimento novo é outra vida.
        let transfer = self.facts.transfer_life(name);
        match probed {
            Probed::Key { transfer: true, .. } if transfer.is_some() => transfer,
            Probed::Key { key, .. } => session_life(Some(&key), None),
            Probed::Birth(birth) => session_life(None, birth).or(transfer),
        }
    }

    /// Só o multiplexador respondendo "não há" e nenhum sidecar sobrando: vida `None` sozinha
    /// também é o tmux que falhou.
    pub async fn confirmed_absent(&self, name: &str) -> bool {
        self.probe.has_session(name).await == Some(false) && !self.probe.sidecar_exists(name)
    }
}

async fn record_verdict(share: &Share, lives: &Lives) -> Verdict {
    if share.revoked_at.is_some() {
        return Verdict::Ended;
    }
    let facts = lives.facts();
    if facts.changing_mode(&share.session) {
        return Verdict::Unsure;
    }
    let mut life = lives.life(&share.session, false).await;
    if life.is_some() && life != share.life {
        // O cache pode ser de antes de a troca de modo mover o convite para a vida nova.
        life = lives.life(&share.session, true).await;
    }
    if life.is_some() && life == share.life {
        return Verdict::Ok;
    }
    // Sem conferência dos registros duráveis, uma transferência em curso parece sessão morta ou renascida.
    if !facts.snapshot_received() {
        return Verdict::Unsure;
    }
    match life {
        Some(_) => Verdict::Ended,
        None if lives.confirmed_absent(&share.session).await => Verdict::Ended,
        None => Verdict::Unsure,
    }
}

/// Veredito do token para a rota: com sessão, o registro dela; sem, vale se algum vale.
pub async fn verdict(guest: &Guest, session: Option<&str>, lives: &Lives) -> Verdict {
    let first = guest_verdict(guest, session, lives).await;
    if first != Verdict::Ended {
        return first;
    }
    // O registro é cópia: a troca de modo pode tê-lo movido de vida depois da leitura, e o 410 é
    // final para o convidado.
    match lives.reread.as_ref().and_then(|reread| reread(&guest.token_hash)) {
        Some(fresh) => guest_verdict(&fresh, session, lives).await,
        None => first,
    }
}

async fn guest_verdict(guest: &Guest, session: Option<&str>, lives: &Lives) -> Verdict {
    if let Some(name) = session {
        return match guest.share_for(name) {
            Some(share) => record_verdict(share, lives).await,
            None => Verdict::Ended,
        };
    }
    let mut unsure = false;
    for share in &guest.records {
        match record_verdict(share, lives).await {
            Verdict::Ok => return Verdict::Ok,
            Verdict::Unsure => unsure = true,
            Verdict::Ended => {}
        }
    }
    if unsure { Verdict::Unsure } else { Verdict::Ended }
}

/// `share_api._sweep_once` sem o túnel: só a ausência confirmada ou outra vida revogam.
pub async fn sweep(store: &ShareStore, lives: &Lives) -> std::io::Result<usize> {
    let facts = lives.facts();
    if !facts.snapshot_received() {
        return store.sweep(|_, _| true);
    }
    // A consulta é assíncrona e a closure do `sweep` não: lê antes, decide dentro.
    let probed: HashSet<(String, Option<String>)> = store.all().into_iter()
        .filter(|s| s.revoked_at.is_none()).map(|s| (s.session, s.life)).collect();
    let mut current = HashMap::new();
    for name in probed.iter().map(|(name, _)| name).collect::<HashSet<_>>() {
        let life = lives.life(name, true).await;
        let absent = life.is_none() && lives.confirmed_absent(name).await;
        current.insert(name.clone(), (life, absent));
    }
    store.sweep(|session, life| {
        if facts.changing_mode(session) {
            return true;
        }
        // Registro que mudou durante a consulta (troca de modo que acabou no meio) fica para a
        // próxima rodada: a consulta foi de antes.
        if !probed.contains(&(session.to_owned(), life.map(str::to_owned))) {
            return true;
        }
        match current.get(session) {
            None => true,
            Some((None, absent)) => !absent,
            Some((Some(now), _)) => life == Some(now.as_str()),
        }
    })
}


#[cfg(test)]
mod transfer_uncertainty_tests {
    use super::*;
    use crate::share::store::Kind;

    struct Absent;
    impl LifeProbe for Absent {
        fn birth<'a>(&'a self,_:&'a str)->BoxFuture<'a,Option<u64>> {Box::pin(async {None})}
        fn has_session<'a>(&'a self,_:&'a str)->BoxFuture<'a,Option<bool>> {Box::pin(async {Some(false)})}
        fn sidecar(&self,_:&str)->Option<(String,bool)> {None}
        fn sidecar_exists(&self,_:&str)->bool {false}
    }

    #[tokio::test]
    async fn unreadable_transfer_preserves_only_its_invite_until_explicit_recovery() {
        let dir=tempfile::tempdir().unwrap();
        let store=ShareStore::open(dir.path().join("shares.json"),Arc::new(||100.0));
        let facts=Arc::new(LifeFacts::default());let lives=Lives::new(Arc::new(Absent),facts.clone());
        let (_,held)=store.create_redeemed("uncertain",Some("k:old".into()),Kind::Share).unwrap();
        let (_,other)=store.create_redeemed("other",Some("k:old".into()),Kind::Share).unwrap();
        facts.apply_transfer_snapshot(Default::default(),["uncertain".into()].into());
        facts.end_mode_change("uncertain");
        assert_eq!(verdict(&store.lookup_token(&held).unwrap(),Some("uncertain"),&lives).await,Verdict::Unsure);
        assert_eq!(verdict(&store.lookup_token(&other).unwrap(),Some("other"),&lives).await,Verdict::Ended);
        sweep(&store,&lives).await.unwrap();
        assert!(store.active_sessions().contains("uncertain"));
        assert!(!store.active_sessions().contains("other"));
        facts.apply_snapshot(Default::default(),Default::default());
        assert!(facts.changing_mode("uncertain"),"fotografia de outra rodada não prova recuperação");
        facts.set_transfer_uncertain("uncertain",false);
        sweep(&store,&lives).await.unwrap();
        assert!(!store.active_sessions().contains("uncertain"));
    }
}
