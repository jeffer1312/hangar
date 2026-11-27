//! A agenda das sessões paradas por queda passageira: espera a rede voltar, deixa passar um atraso
//! aleatório e manda o "continue" uma vez. Uma tarefa só para todas as sessões; sem sessão parada ela não
//! sonda nada. A única marca da espera é a nota na conversa, trocada no lugar pelo resultado: nada na linha
//! da lista e nenhuma faixa.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::classify::StopKind;
use crate::ledger::{Ledger, Source, Waiting};
use crate::probe::{HttpProbe, Probe};
use crate::text;

/// De quanto em quanto a rede é sondada enquanto há sessão esperando por ela.
pub const TICK: Duration = Duration::from_secs(15);
/// O atraso entre a rede voltar e o envio: várias sessões da máquina não saem juntas.
pub const JITTER_MIN: Duration = Duration::from_secs(10);
pub const JITTER_MAX: Duration = Duration::from_secs(30);
/// Parada mais velha que isso não continua: o que a pessoa pediu já pode não valer.
pub const STALE_S: f64 = 2.0 * 3600.0;

/// O motivo do Cancelar da nota; qualquer outro `why` do `Command::Cancel` é uma desistência com esse motivo.
pub const CANCELLED: &str = "cancelled";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StopHit {
    pub name: String,
    pub provider: String,
    pub kind: StopKind,
    /// A linha (Claude) ou o turno (Codex) em que parou; é a chave da nota e do registro.
    pub line_id: Option<String>,
    pub at: f64,
    pub life: String,
}

impl StopHit {
    pub fn key(&self) -> String {
        self.line_id.clone().unwrap_or_else(|| format!("at:{:.3}", self.at))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub life: String,
    pub off: bool,
    pub state: String,
    /// Mensagem da pessoa esperando na fila. A própria nota da continuação não conta.
    pub queue_pending: bool,
    pub question_open: bool,
    pub orq: bool,
    pub user_after_stop: bool,
    pub handoff: bool,
}

/// O que a agenda usa do servidor; nos testes, um falso que registra as chamadas.
pub trait Host: Send + Sync {
    /// `Ok(None)`: a sessão não está mais na lista. `Err` com o código: a lista não se leu, o que não prova
    /// que a sessão mudou.
    fn facts<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<Facts>, String>>;
    /// Quem a sessão precisa alcançar: `api.anthropic.com:443`, o endereço do motor… `None` não espera a rede.
    fn probe_target(&self, name: &str) -> Option<String>;
    /// `Ok(true)`: entregue; `Ok(false)`: na fila da sessão, esperando a vez.
    fn send_continue<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// `id` é o `entry_id` da nota, o mesmo em toda troca; `call` muda a cada troca.
    fn note<'a>(&'a self, name: &'a str, id: &'a str, call: &'a str, text: String, meta: Value) -> BoxFuture<'a, Result<(), String>>;
    fn diary(&self, name: &str, code: &str, reason: &'static str);
    fn now(&self) -> f64;
    /// O `HH:MM` local da nota.
    fn hhmm(&self, at: f64) -> String;
}

#[derive(Debug)]
pub enum Command {
    Stop(StopHit),
    Cancel { name: String, why: &'static str },
    /// O Cancelar da nota pela rota: `answer` sai depois de a nota trocar, com `true` quando a sessão esperava.
    CancelAnswered { name: String, answer: tokio::sync::oneshot::Sender<bool> },
    Forget(String),
    Rename { old: String, new: String },
}

pub fn spawn<H: Host + 'static>(host: Arc<H>, ledger: Arc<Mutex<Ledger>>) -> (UnboundedSender<Command>, JoinHandle<()>) {
    spawn_with_probe(host, ledger, Arc::new(HttpProbe::new()))
}

pub fn spawn_with_probe<H: Host + 'static, P: Probe + 'static>(host: Arc<H>, ledger: Arc<Mutex<Ledger>>, probe: Arc<P>)
    -> (UnboundedSender<Command>, JoinHandle<()>) {
    let (sender, commands) = tokio::sync::mpsc::unbounded_channel();
    // A espera gravada antes de o servidor reiniciar volta à agenda: sem isso a nota ficaria "continuo
    // quando a rede voltar" para sempre. A sessão que mudou de vida nesse meio desiste no primeiro tique.
    let restored = ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner).parked();
    let parked = restored.into_iter().map(|waiting| {
        let send_at = (waiting.hit.kind == StopKind::Server5xx).then(|| Instant::now() + jitter());
        let parked = Parked { key: waiting.hit.key(), hit: waiting.hit, count: waiting.count, note_out: waiting.note_out, send_at };
        (parked.hit.name.clone(), parked)
    }).collect();
    let scheduler = Scheduler { host, ledger, probe, parked, next_tick: None };
    (sender, tokio::spawn(scheduler.run(commands)))
}

struct Parked {
    hit: StopHit,
    key: String,
    /// Trocas da nota já tentadas; o `count` da próxima é este mais um.
    count: u64,
    /// A nota de espera chegou à conversa.
    note_out: bool,
    /// Rede de volta (ou 5xx, que não espera a rede): o envio está marcado.
    send_at: Option<Instant>,
}

enum End {
    Continued,
    Failed(String),
    Cancelled,
    Skipped(String),
}

pub struct Scheduler<H: Host, P: Probe> {
    host: Arc<H>,
    ledger: Arc<Mutex<Ledger>>,
    probe: Arc<P>,
    parked: HashMap<String, Parked>,
    next_tick: Option<Instant>,
}

impl<H: Host, P: Probe> Scheduler<H, P> {
    async fn run(mut self, mut commands: UnboundedReceiver<Command>) {
        self.arm_tick();
        loop {
            let wake = self.next_wake();
            let far = Instant::now() + Duration::from_secs(3600);
            // Comando antes do relógio: um Cancelar que chegou enquanto a agenda esperava o servidor não
            // pode perder o sorteio para o atraso que venceu no mesmo instante.
            tokio::select! {
                biased;
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                () = tokio::time::sleep_until(wake.unwrap_or(far)), if wake.is_some() => self.wake(&mut commands).await,
            }
        }
    }

    /// O que chegou enquanto a agenda esperava a sonda ou o servidor vale antes de um envio.
    async fn drain(&mut self, commands: &mut UnboundedReceiver<Command>) {
        while let Ok(command) = commands.try_recv() {
            self.command(command).await;
        }
    }

    /// A chave da parada da sessão cujo envio já venceu.
    fn due_key(&self, name: &str) -> Option<String> {
        let now = Instant::now();
        self.parked.get(name).filter(|parked| parked.send_at.is_some_and(|at| at <= now)).map(|parked| parked.key.clone())
    }

    async fn close(&mut self, name: &str, end: End) {
        if let Some(parked) = self.parked.remove(name) {
            self.finish(name, parked, end).await;
        }
    }

    fn persist(&self, parked: &Parked) {
        let waiting = Waiting { hit: parked.hit.clone(), count: parked.count, note_out: parked.note_out };
        self.ledger().park(&parked.hit.name, waiting);
    }

    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn next_wake(&self) -> Option<Instant> {
        self.parked.values().filter_map(|parked| parked.send_at).chain(self.next_tick).min()
    }

    /// O tique só existe com sessão esperando a rede.
    fn arm_tick(&mut self) {
        if !self.parked.values().any(|parked| parked.send_at.is_none()) {
            self.next_tick = None;
        } else if self.next_tick.is_none() {
            self.next_tick = Some(Instant::now() + TICK);
        }
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Stop(hit) => self.stop(hit).await,
            Command::Cancel { name, why } => {
                if let Some(parked) = self.parked.remove(&name) {
                    let end = if why == CANCELLED { End::Cancelled } else { End::Skipped(why.to_owned()) };
                    self.finish(&name, parked, end).await;
                }
            }
            Command::CancelAnswered { name, answer } => {
                let parked = self.parked.remove(&name);
                let waiting = parked.is_some();
                if let Some(parked) = parked {
                    self.finish(&name, parked, End::Cancelled).await;
                }
                let _ = answer.send(waiting);
            }
            Command::Forget(name) => {
                // A sessão fechou: a nota vai junto com a conversa.
                self.parked.remove(&name);
                self.ledger().forget(&name);
            }
            Command::Rename { old, new } => {
                if let Some(mut parked) = self.parked.remove(&old) {
                    parked.hit.name.clone_from(&new);
                    self.parked.insert(new.clone(), parked);
                }
                self.ledger().rename(&old, &new);
            }
        }
        self.arm_tick();
    }

    async fn stop(&mut self, hit: StopHit) {
        let name = hit.name.clone();
        let key = hit.key();
        let now = self.host.now();
        if self.parked.get(&name).is_some_and(|parked| parked.key == key) {
            return;
        }
        // A mesma linha vista de novo depois do desfecho (releitura do transcript) não volta a esperar nem
        // derruba a parada que espera agora.
        if hit.kind.is_transient() && self.ledger().has_ended(&name, &key, now) {
            return;
        }
        // Outra parada na mesma sessão: o turno andou depois da anterior, que não continua mais.
        if let Some(parked) = self.parked.remove(&name) {
            self.finish(&name, parked, End::Skipped("replaced".to_owned())).await;
        }
        if hit.kind == StopKind::Normal {
            self.ledger().real_reply(&name);
        }
        if !hit.kind.is_transient() {
            return;
        }
        if now - hit.at > STALE_S {
            self.skip_diary(&name, "stale");
            self.ledger().end(&name, &key, now);
            return;
        }
        // Na hora da parada o estado ao vivo ainda pode estar virando; trabalhando, fila e pergunta são
        // conferidos no tique e antes do envio.
        match self.host.facts(&name).await {
            Ok(facts) => if let Some(why) = give_up(&hit, facts.as_ref(), false) {
                self.skip_diary(&name, why);
                self.ledger().end(&name, &key, now);
                return;
            },
            // Espera assim mesmo: o tique relê antes de sondar.
            Err(code) => self.unreadable(&name, &code),
        }
        let allowed = self.ledger().allow(&name, Source::Transient, &key, hit.kind, now);
        if let Err(cap) = allowed {
            self.host.diary(&name, &format!("auto_continue_capped:{}", cap.code()), "continuar sozinho: um teto do registro barrou");
            self.ledger().end(&name, &key, now);
            return;
        }
        // O 5xx não espera a rede, só o atraso.
        let send_at = (hit.kind == StopKind::Server5xx).then(|| Instant::now() + jitter());
        let mut parked = Parked { hit, key, count: 0, note_out: false, send_at };
        let meta = json!({"kind": "auto_continue", "state": "waiting", "stopped_at": parked.hit.at});
        let waiting = format!("A conexão caiu às {}; continuo quando a rede voltar.", self.host.hhmm(parked.hit.at));
        parked.note_out = self.note(&name, &mut parked, "waiting", waiting, meta).await;
        self.persist(&parked);
        self.parked.insert(name, parked);
    }

    async fn wake(&mut self, commands: &mut UnboundedReceiver<Command>) {
        let now = Instant::now();
        if self.next_tick.is_some_and(|tick| tick <= now) {
            self.next_tick = None;
            self.tick().await;
        }
        let due: Vec<String> = self.parked.iter()
            .filter(|(_, parked)| parked.send_at.is_some_and(|at| at <= now))
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            self.send(&name, commands).await;
        }
        self.arm_tick();
    }

    /// Confere quem espera a rede e sonda cada alvo uma vez, para todas as sessões dele.
    async fn tick(&mut self) {
        let waiting: Vec<String> = self.parked.iter().filter(|(_, parked)| parked.send_at.is_none()).map(|(name, _)| name.clone()).collect();
        let mut targets: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
        for name in waiting {
            let Some(parked) = self.parked.get(&name) else { continue };
            let why = if self.host.now() - parked.hit.at > STALE_S {
                Some("stale")
            } else {
                match self.host.facts(&name).await {
                    Ok(facts) => self.parked.get(&name).and_then(|parked| give_up(&parked.hit, facts.as_ref(), true)),
                    // Sem os fatos não sonda nem desiste: o próximo tique relê.
                    Err(code) => {
                        self.unreadable(&name, &code);
                        continue;
                    }
                }
            };
            if let Some(why) = why {
                if let Some(parked) = self.parked.remove(&name) {
                    self.finish(&name, parked, End::Skipped(why.to_owned())).await;
                }
                continue;
            }
            targets.entry(self.host.probe_target(&name)).or_default().push(name);
        }
        let probe = &self.probe;
        let answers = futures_util::future::join_all(targets.keys().map(|target| async move {
            match target {
                Some(target) => probe.reachable(target).await,
                None => true,
            }
        })).await;
        for ((_, names), reachable) in targets.into_iter().zip(answers) {
            if !reachable {
                continue;
            }
            for name in names {
                if let Some(parked) = self.parked.get_mut(&name) {
                    parked.send_at = Some(Instant::now() + jitter());
                }
            }
        }
    }

    async fn send(&mut self, name: &str, commands: &mut UnboundedReceiver<Command>) {
        self.drain(commands).await;
        let Some(key) = self.due_key(name) else { return };
        let Some(at) = self.parked.get(name).map(|parked| parked.hit.at) else { return };
        if self.host.now() - at > STALE_S {
            return self.close(name, End::Skipped("stale".to_owned())).await;
        }
        let facts = self.host.facts(name).await;
        // O servidor pode demorar a responder; um Cancelar, renomear ou outra parada nesse meio vence.
        self.drain(commands).await;
        if self.due_key(name).as_ref() != Some(&key) {
            return;
        }
        let facts = match facts {
            Ok(facts) => facts,
            // Sem os fatos não dá para saber se a pessoa escreveu ou se o turno andou: volta a esperar o tique.
            Err(code) => {
                self.unreadable(name, &code);
                if let Some(parked) = self.parked.get_mut(name) {
                    parked.send_at = None;
                }
                return;
            }
        };
        let Some(parked) = self.parked.remove(name) else { return };
        if let Some(why) = give_up(&parked.hit, facts.as_ref(), true) {
            return self.finish(name, parked, End::Skipped(why.to_owned())).await;
        }
        let now = self.host.now();
        let allowed = self.ledger().allow(name, Source::Transient, &parked.key, parked.hit.kind, now);
        if let Err(cap) = allowed {
            return self.finish(name, parked, End::Skipped(format!("capped:{}", cap.code()))).await;
        }
        // Sai da espera gravada antes de mandar: reiniciar no meio do envio não pode mandar o continue de novo.
        self.ledger().end(name, &parked.key, now);
        match self.host.send_continue(name, text::TRANSIENT).await {
            Ok(_) => {
                let at = self.host.now();
                self.ledger().record(name, Source::Transient, &parked.key, parked.hit.kind, at);
                self.finish(name, parked, End::Continued).await;
            }
            Err(code) => self.finish(name, parked, End::Failed(code)).await,
        }
    }

    /// Diário do desfecho e a nota trocada no lugar. Desistência só troca a nota que já saiu.
    async fn finish(&mut self, name: &str, mut parked: Parked, end: End) {
        self.ledger().end(name, &parked.key, self.host.now());
        let stopped = self.host.hhmm(parked.hit.at);
        let mut meta = json!({"kind": "auto_continue", "stopped_at": parked.hit.at});
        let (state, text) = match &end {
            End::Continued => {
                let at = self.host.now();
                meta["continued_at"] = json!(at);
                self.host.diary(name, "auto_continue_sent", "continuar sozinho: a rede voltou e o turno seguiu");
                ("continued", format!("A conexão caiu às {stopped}; continuei às {}.", self.host.hhmm(at)))
            }
            End::Failed(code) => {
                meta["code"] = json!(code);
                self.host.diary(name, &format!("auto_continue_failed:{code}"), "continuar sozinho: o envio do continue foi recusado");
                ("failed", format!("A conexão caiu às {stopped}; não consegui continuar ({code})."))
            }
            End::Cancelled => {
                self.skip_diary(name, CANCELLED);
                ("cancelled", format!("A conexão caiu às {stopped}; não continuei (cancelado)."))
            }
            End::Skipped(why) => {
                meta["code"] = json!(why);
                self.skip_diary(name, why);
                if !parked.note_out {
                    return;
                }
                ("skipped", format!("A conexão caiu às {stopped}; não continuei."))
            }
        };
        meta["state"] = json!(state);
        self.note(name, &mut parked, state, text, meta).await;
    }

    fn unreadable(&self, name: &str, code: &str) {
        self.host.diary(name, &format!("auto_continue_facts_unreadable:{code}"), "continuar sozinho: a lista não se leu; a parada segue esperando");
    }

    fn skip_diary(&self, name: &str, why: &str) {
        if let Some(cap) = why.strip_prefix("capped:") {
            self.host.diary(name, &format!("auto_continue_capped:{cap}"), "continuar sozinho: um teto do registro barrou");
        } else {
            self.host.diary(name, &format!("auto_continue_skipped:{why}"), "continuar sozinho: a sessão mudou e o turno não continua");
        }
    }

    /// `true`: a nota chegou à conversa.
    async fn note(&self, name: &str, parked: &mut Parked, state: &str, text: String, mut meta: Value) -> bool {
        parked.count += 1;
        meta["count"] = json!(parked.count);
        let id = format!("autocont:{}", parked.key);
        let call = format!("{id}:{state}");
        match self.host.note(name, &id, &call, text, meta).await {
            Ok(()) => true,
            Err(code) => {
                tracing::warn!(session = name, %code, "continuar sozinho: nota não escrita");
                self.host.diary(name, &format!("auto_continue_note_failed:{code}"), "continuar sozinho: a nota da conversa não foi escrita");
                false
            }
        }
    }
}

/// Por que a sessão não continua mais. `full` inclui o que só vale depois de o estado assentar.
fn give_up(hit: &StopHit, facts: Option<&Facts>, full: bool) -> Option<&'static str> {
    let Some(facts) = facts else { return Some("life_changed") };
    let checks = [
        (facts.life != hit.life, "life_changed"),
        (facts.off, "off"),
        (facts.orq, "orq"),
        (facts.handoff, "handoff"),
        (full && facts.user_after_stop, "user_after_stop"),
        (full && facts.queue_pending, "queue_pending"),
        (full && facts.question_open, "question_open"),
        // Adormecida ou estacionada não desiste: a escrita a acorda onde estava.
        (full && facts.state == "working", "working"),
    ];
    checks.into_iter().find_map(|(gave_up, why)| gave_up.then_some(why))
}

fn jitter() -> Duration {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0);
    let spread = (JITTER_MAX - JITTER_MIN).as_millis() as u64;
    JITTER_MIN + Duration::from_millis(hasher.finish() % (spread + 1))
}
