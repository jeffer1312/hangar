//! A agenda pela interface pública, com um servidor falso que grava as chamadas e o relógio pausado.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_auto_continue::scheduler::{BoxFuture, CANCELLED, cancel};
use hangar_auto_continue::{Cap, Command, Facts, Host, Ledger, Probe, Source, StopHit, StopKind, ledger, spawn_with_probe, text};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::Instant;

const BASE: f64 = 1_800_000_000.0;
const TARGET: &str = "api.anthropic.com:443";

#[derive(Clone, Debug)]
struct Note {
    name: String,
    id: String,
    call: String,
    text: String,
    meta: Value,
}

struct Fake {
    start: Instant,
    facts: Mutex<Option<Facts>>,
    /// A lista não se lê, com este código.
    facts_error: Mutex<Option<String>>,
    /// Quanto o servidor demora a dar os fatos.
    facts_delay: Mutex<Duration>,
    send: Mutex<Result<bool, String>>,
    /// Quanto o envio demora, como o `/input` acordando uma sessão adormecida.
    send_delay: Mutex<Duration>,
    /// A sonda falha estas vezes antes de responder.
    fails_left: Mutex<usize>,
    /// Rede fora de vez.
    down: Mutex<bool>,
    probes: Mutex<Vec<(f64, String)>>,
    sends: Mutex<Vec<(f64, String, String)>>,
    notes: Mutex<Vec<Note>>,
    diary: Mutex<Vec<String>>,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            facts: Mutex::new(Some(Facts { life: "L1".into(), state: "idle".into(), ..Facts::default() })),
            facts_error: Mutex::new(None),
            facts_delay: Mutex::new(Duration::ZERO),
            send: Mutex::new(Ok(true)),
            send_delay: Mutex::new(Duration::ZERO),
            fails_left: Mutex::new(0),
            down: Mutex::new(false),
            probes: Mutex::default(),
            sends: Mutex::default(),
            notes: Mutex::default(),
            diary: Mutex::default(),
        })
    }

    fn clock(&self) -> f64 {
        BASE + self.start.elapsed().as_secs_f64()
    }

    fn set_facts(&self, change: impl FnOnce(&mut Facts)) {
        change(self.facts.lock().unwrap().as_mut().unwrap());
    }

    fn probes(&self) -> Vec<(f64, String)> {
        self.probes.lock().unwrap().clone()
    }

    fn sends(&self) -> Vec<(f64, String, String)> {
        self.sends.lock().unwrap().clone()
    }

    fn notes(&self) -> Vec<Note> {
        self.notes.lock().unwrap().clone()
    }

    fn last_note(&self) -> Note {
        self.notes().last().cloned().expect("nenhuma nota")
    }

    fn diary(&self) -> Vec<String> {
        self.diary.lock().unwrap().clone()
    }
}

impl Host for Fake {
    fn facts<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, Result<Option<Facts>, String>> {
        Box::pin(async move {
            let delay = *self.facts_delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if let Some(code) = self.facts_error.lock().unwrap().clone() {
                return Err(code);
            }
            Ok(self.facts.lock().unwrap().clone())
        })
    }
    fn probe_target(&self, _name: &str) -> Option<String> {
        Some(TARGET.to_owned())
    }
    fn send_continue<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            self.sends.lock().unwrap().push((self.clock(), name.to_owned(), text.to_owned()));
            let delay = *self.send_delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            self.send.lock().unwrap().clone()
        })
    }
    fn note<'a>(&'a self, name: &'a str, id: &'a str, call: &'a str, text: String, meta: Value) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.notes.lock().unwrap().push(Note { name: name.to_owned(), id: id.to_owned(), call: call.to_owned(), text, meta });
            Ok(())
        })
    }
    fn diary(&self, _name: &str, code: &str, _reason: &'static str) {
        self.diary.lock().unwrap().push(code.to_owned());
    }
    fn now(&self) -> f64 {
        self.clock()
    }
    fn hhmm(&self, at: f64) -> String {
        let secs = at as i64;
        format!("{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60)
    }
}

impl Probe for Fake {
    fn reachable<'a>(&'a self, target: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.probes.lock().unwrap().push((self.clock(), target.to_owned()));
            if *self.down.lock().unwrap() {
                return false;
            }
            let mut fails = self.fails_left.lock().unwrap();
            if *fails > 0 {
                *fails -= 1;
                return false;
            }
            true
        })
    }
}

struct Run {
    commands: UnboundedSender<Command>,
    ledger: Arc<Mutex<Ledger>>,
    task: JoinHandle<()>,
    home: tempfile::TempDir,
}

fn start(fake: &Arc<Fake>) -> Run {
    let home = tempfile::tempdir().unwrap();
    let ledger = Arc::new(Mutex::new(Ledger::load(ledger::path(home.path()))));
    let (commands, task) = spawn_with_probe(fake.clone(), ledger.clone(), fake.clone());
    Run { commands, ledger, task, home }
}

fn hit(name: &str, line: &str, kind: StopKind, at: f64) -> StopHit {
    StopHit { name: name.into(), provider: "claude".into(), kind, line_id: Some(line.into()), at, life: "L1".into() }
}

async fn wait(secs: u64) {
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

/// O comando chega à agenda antes de o relógio andar.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(10)).await;
}

impl Run {
    /// O servidor reinicia: a agenda morre e a nova lê o mesmo arquivo do registro.
    async fn restart(&mut self, fake: &Arc<Fake>) {
        self.task.abort();
        let _ = (&mut self.task).await;
        let ledger = Arc::new(Mutex::new(Ledger::load(ledger::path(self.home.path()))));
        let (commands, task) = spawn_with_probe(fake.clone(), ledger.clone(), fake.clone());
        (self.commands, self.ledger, self.task) = (commands, ledger, task);
    }

    fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }

    fn allow(&self, name: &str, line: &str, now: f64) -> Result<(), Cap> {
        self.ledger.lock().unwrap().allow(name, Source::Transient, line, StopKind::Network, now)
    }
}

#[tokio::test(start_paused = true)]
async fn one_continue_after_the_network_returns() {
    let fake = Fake::new();
    *fake.fails_left.lock().unwrap() = 3;
    let run = start(&fake);
    let at = fake.clock();
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, at)));
    settle().await;
    let waiting = fake.last_note();
    assert_eq!(waiting.name, "s");
    assert_eq!(waiting.id, "autocont:line-1");
    assert_eq!(waiting.meta["kind"], "auto_continue");
    assert_eq!(waiting.meta["state"], "waiting");
    assert_eq!(waiting.meta["stopped_at"].as_f64(), Some(at));
    assert_eq!(waiting.meta["count"], 1);
    assert_eq!(waiting.text, format!("A conexão caiu às {}; continuo quando a rede voltar.", fake.hhmm(at)));
    assert!(fake.sends().is_empty());

    wait(300).await;
    let probes = fake.probes();
    assert_eq!(probes.len(), 4, "três falhas e a que respondeu: {probes:?}");
    assert!(probes.iter().all(|(_, target)| target == TARGET));
    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "{sends:?}");
    let (sent_at, name, sent) = &sends[0];
    assert_eq!((name.as_str(), sent.as_str()), ("s", text::TRANSIENT));
    let delay = sent_at - probes[3].0;
    assert!((10.0 - 1e-6..=30.0 + 1e-6).contains(&delay), "atraso {delay}");

    let done = fake.last_note();
    assert_eq!(done.id, "autocont:line-1");
    assert_eq!(done.meta["state"], "continued");
    assert_eq!(done.meta["continued_at"].as_f64(), Some(*sent_at));
    assert_eq!(done.text, format!("A conexão caiu às {}; continuei às {}.", fake.hhmm(at), fake.hhmm(*sent_at)));
    assert!(fake.diary().contains(&"auto_continue_sent".to_owned()));
    assert_eq!(run.allow("s", "line-1", fake.clock()), Err(Cap::Once));
}

#[tokio::test(start_paused = true)]
async fn the_waiting_note_is_replaced_not_stacked() {
    let fake = Fake::new();
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::StreamLost, fake.clock())));
    wait(120).await;
    let notes = fake.notes();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(notes[0].id, notes[1].id);
    assert_eq!((notes[0].meta["count"].as_u64(), notes[1].meta["count"].as_u64()), (Some(1), Some(2)));
    assert_eq!(notes[0].call, "autocont:line-1:waiting");
    assert_eq!(notes[1].call, "autocont:line-1:continued");
    assert_ne!(notes[0].call, notes[1].call);
}

#[tokio::test(start_paused = true)]
async fn a_stop_older_than_two_hours_is_dropped() {
    let fake = Fake::new();
    let run = start(&fake);
    run.send(Command::Stop(hit("old", "line-1", StopKind::Network, fake.clock() - 7201.0)));
    settle().await;
    assert!(fake.notes().is_empty());
    assert_eq!(fake.diary(), ["auto_continue_skipped:stale"]);
    wait(600).await;
    assert!(fake.probes().is_empty());
    assert!(fake.sends().is_empty());

    // A rede não volta em 2 h: a nota de espera vira desistência e a sondagem para.
    *fake.down.lock().unwrap() = true;
    run.send(Command::Stop(hit("s", "line-2", StopKind::Network, fake.clock())));
    wait(2 * 3600 + 60).await;
    assert!(fake.sends().is_empty());
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "skipped");
    assert_eq!(note.meta["code"], "stale");
    assert!(note.text.ends_with("; não continuei."), "{}", note.text);
    assert_eq!(fake.diary().iter().filter(|code| *code == "auto_continue_skipped:stale").count(), 2);
    let probed = fake.probes().len();
    assert!(probed > 0);
    wait(600).await;
    assert_eq!(fake.probes().len(), probed);
}

#[tokio::test(start_paused = true)]
async fn a_message_after_the_stop_cancels() {
    let fake = Fake::new();
    *fake.fails_left.lock().unwrap() = 1;
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    fake.set_facts(|facts| facts.user_after_stop = true);
    wait(300).await;
    assert!(fake.sends().is_empty());
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "skipped");
    assert_eq!(note.meta["code"], "user_after_stop");
    assert!(fake.diary().contains(&"auto_continue_skipped:user_after_stop".to_owned()));
    assert_eq!(run.allow("s", "line-1", fake.clock()), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_life_change_cancels() {
    let fake = Fake::new();
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Server5xx, fake.clock())));
    settle().await;
    assert_eq!(fake.last_note().meta["state"], "waiting");
    // A conta trocou durante o atraso: outra vida, que não pediu nada.
    fake.set_facts(|facts| facts.life = "L2".into());
    wait(120).await;
    assert!(fake.sends().is_empty());
    assert_eq!(fake.last_note().meta["code"], "life_changed");
    assert!(fake.diary().contains(&"auto_continue_skipped:life_changed".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn cancel_during_the_jitter_sends_nothing() {
    let fake = Fake::new();
    let run = start(&fake);
    let at = fake.clock();
    run.send(Command::Stop(hit("s", "line-1", StopKind::Server5xx, at)));
    wait(5).await;
    assert!(fake.sends().is_empty());
    run.send(Command::Cancel { name: "s".into(), why: CANCELLED });
    wait(120).await;
    assert!(fake.sends().is_empty());
    // O 5xx não espera a rede: nem sonda.
    assert!(fake.probes().is_empty());
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "cancelled");
    assert_eq!(note.meta["count"], 2);
    assert_eq!(note.call, "autocont:line-1:cancelled");
    assert_eq!(note.text, format!("A conexão caiu às {}; não continuei (cancelado).", fake.hhmm(at)));
    assert!(fake.diary().contains(&"auto_continue_skipped:cancelled".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn the_route_cancel_marks_and_the_note_changes_after() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    assert_eq!(fake.last_note().meta["state"], "waiting");
    assert!(cancel(&run.ledger, &run.commands, "s"), "a sessão esperava a rede");
    settle().await;
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "cancelled");
    assert_eq!(note.call, "autocont:line-1:cancelled");
    assert!(run.ledger.lock().unwrap().parked().is_empty(), "a espera sai do registro");
    assert!(!cancel(&run.ledger, &run.commands, "s"), "sem espera, nada a cancelar");
    settle().await;
    assert_eq!(fake.notes().len(), 2, "a segunda não escreve nota");
}

/// O Cancelar responde na hora com a agenda presa no envio lento de outra sessão, e o continue não sai.
#[tokio::test(start_paused = true)]
async fn the_route_cancel_answers_while_the_scheduler_is_busy_sending() {
    let fake = Fake::new();
    *fake.send_delay.lock().unwrap() = Duration::from_secs(60);
    let run = start(&fake);
    run.send(Command::Stop(hit("a", "line-a", StopKind::Server5xx, fake.clock())));
    run.send(Command::Stop(hit("b", "line-b", StopKind::Server5xx, fake.clock())));
    // Os dois atrasos vencem em até 30 s; o primeiro envio prende a agenda por 60 s.
    wait(31).await;
    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "{sends:?}");
    let other = if sends[0].1 == "a" { "b" } else { "a" };
    assert!(cancel(&run.ledger, &run.commands, other), "a outra sessão ainda esperava");
    wait(120).await;
    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "o continue da cancelada não saiu: {sends:?}");
    let note = fake.notes().into_iter().rfind(|note| note.name == other).unwrap();
    assert_eq!(note.meta["state"], "cancelled");
    assert!(fake.diary().contains(&"auto_continue_skipped:cancelled".to_owned()));
}

/// Cancelada pouco antes de o servidor parar: a agenda nova troca a nota e não manda nada.
#[tokio::test(start_paused = true)]
async fn a_cancel_marked_before_a_restart_is_kept() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let mut run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    // A rota marca, mas o servidor para antes de a agenda trocar a nota.
    assert!(run.ledger.lock().unwrap().cancel("s"));
    run.restart(&fake).await;
    *fake.down.lock().unwrap() = false;
    wait(120).await;
    assert!(fake.sends().is_empty(), "{:?}", fake.sends());
    let note = fake.last_note();
    assert_eq!((note.id.as_str(), note.meta["state"].as_str()), ("autocont:line-1", Some("cancelled")));
    assert!(run.ledger.lock().unwrap().parked().is_empty());
}

#[tokio::test(start_paused = true)]
async fn no_probe_without_a_parked_session() {
    let fake = Fake::new();
    let run = start(&fake);
    wait(600).await;
    assert!(fake.probes().is_empty());
    run.send(Command::Stop(hit("s", "line-1", StopKind::Normal, fake.clock())));
    run.send(Command::Stop(hit("s", "line-2", StopKind::Other, fake.clock())));
    wait(600).await;
    assert!(fake.probes().is_empty());
    assert!(fake.notes().is_empty());
    // Desligada na sessão: a parada nem chega a esperar.
    fake.set_facts(|facts| facts.off = true);
    run.send(Command::Stop(hit("s", "line-3", StopKind::Network, fake.clock())));
    wait(600).await;
    assert!(fake.probes().is_empty());
    assert!(fake.notes().is_empty());
    assert_eq!(fake.diary(), ["auto_continue_skipped:off"]);
}

#[tokio::test(start_paused = true)]
async fn one_probe_per_target_per_tick() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let run = start(&fake);
    run.send(Command::Stop(hit("a", "line-a", StopKind::Network, fake.clock())));
    run.send(Command::Stop(hit("b", "line-b", StopKind::StreamLost, fake.clock())));
    wait(16).await;
    assert_eq!(fake.probes().len(), 1);
    wait(15).await;
    assert_eq!(fake.probes().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_refused_send_writes_the_failure_note() {
    let fake = Fake::new();
    *fake.send.lock().unwrap() = Err("erro_envio_falhou".into());
    let run = start(&fake);
    let at = fake.clock();
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, at)));
    wait(300).await;
    assert_eq!(fake.sends().len(), 1);
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "failed");
    assert_eq!(note.meta["code"], "erro_envio_falhou");
    assert_eq!(note.text, format!("A conexão caiu às {}; não consegui continuar (erro_envio_falhou).", fake.hhmm(at)));
    assert!(fake.diary().contains(&"auto_continue_failed:erro_envio_falhou".to_owned()));
    // Não saiu: o registro não conta.
    assert_eq!(run.allow("s", "line-1", fake.clock()), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_rename_moves_the_parked_stop() {
    let fake = Fake::new();
    *fake.fails_left.lock().unwrap() = 2;
    let run = start(&fake);
    run.send(Command::Stop(hit("old", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    run.send(Command::Rename { old: "old".into(), new: "new".into() });
    wait(300).await;
    let sends = fake.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].1, "new");
    let note = fake.last_note();
    assert_eq!((note.name.as_str(), note.id.as_str()), ("new", "autocont:line-1"));
    assert_eq!(note.meta["state"], "continued");
    assert_eq!(run.allow("new", "line-1", fake.clock()), Err(Cap::Once));
    assert_eq!(run.allow("old", "line-1", fake.clock()), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_cancel_while_the_server_answers_slowly_wins() {
    let fake = Fake::new();
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Server5xx, fake.clock())));
    settle().await;
    // O atraso vence e o servidor leva mais que ele todo para dar os fatos: o Cancelar chega nesse meio.
    *fake.facts_delay.lock().unwrap() = Duration::from_secs(40);
    wait(31).await;
    assert!(fake.sends().is_empty());
    run.send(Command::Cancel { name: "s".into(), why: CANCELLED });
    wait(120).await;
    assert!(fake.sends().is_empty(), "{:?}", fake.sends());
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "cancelled");
    assert_eq!(note.call, "autocont:line-1:cancelled");
}

#[tokio::test(start_paused = true)]
async fn cancel_after_the_network_returns_sends_nothing() {
    let fake = Fake::new();
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    // A rede responde no primeiro tique e o atraso começa.
    wait(16).await;
    assert_eq!(fake.probes().len(), 1);
    run.send(Command::Cancel { name: "s".into(), why: CANCELLED });
    wait(120).await;
    assert!(fake.sends().is_empty());
    assert_eq!(fake.last_note().meta["state"], "cancelled");
    assert_eq!(fake.probes().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_same_line_again_after_the_end_is_ignored() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let run = start(&fake);
    let at = fake.clock();
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, at)));
    settle().await;
    run.send(Command::Cancel { name: "s".into(), why: CANCELLED });
    settle().await;
    assert_eq!(fake.notes().len(), 2);
    // A releitura do transcript manda a mesma linha de novo: nem espera, nem derruba a parada nova.
    run.send(Command::Stop(hit("s", "line-2", StopKind::Network, fake.clock())));
    settle().await;
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, at)));
    settle().await;
    let notes = fake.notes();
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert_eq!((notes[2].id.as_str(), notes[2].meta["state"].as_str()), ("autocont:line-2", Some("waiting")));
    *fake.down.lock().unwrap() = false;
    wait(120).await;
    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "{sends:?}");
    let done = fake.last_note();
    assert_eq!((done.id.as_str(), done.meta["state"].as_str()), ("autocont:line-2", Some("continued")));
}

#[tokio::test(start_paused = true)]
async fn the_waiting_stop_survives_a_restart() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let mut run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    assert_eq!(fake.last_note().meta["state"], "waiting");
    run.restart(&fake).await;
    *fake.down.lock().unwrap() = false;
    wait(120).await;
    assert_eq!(fake.sends().len(), 1);
    let done = fake.last_note();
    assert_eq!(done.id, "autocont:line-1");
    assert_eq!(done.meta["state"], "continued");
    assert_eq!(done.meta["count"], 2);
    // O continue saiu: outro reinício não manda de novo.
    run.restart(&fake).await;
    wait(120).await;
    assert_eq!(fake.sends().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_restored_stop_of_another_life_gives_up() {
    let fake = Fake::new();
    *fake.down.lock().unwrap() = true;
    let mut run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    run.restart(&fake).await;
    // Enquanto o servidor estava fora a sessão foi reaberta.
    fake.set_facts(|facts| facts.life = "L2".into());
    wait(60).await;
    assert!(fake.sends().is_empty());
    let note = fake.last_note();
    assert_eq!(note.meta["state"], "skipped");
    assert_eq!(note.meta["code"], "life_changed");
    assert_eq!(note.meta["count"], 2);
}

#[tokio::test(start_paused = true)]
async fn a_busy_session_gives_up() {
    let later: [(&str, fn(&mut Facts)); 3] = [
        ("working", |facts: &mut Facts| facts.state = "working".into()),
        ("question_open", |facts: &mut Facts| facts.question_open = true),
        ("queue_pending", |facts: &mut Facts| facts.queue_pending = true),
    ];
    for (code, change) in later {
        let fake = Fake::new();
        let run = start(&fake);
        run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
        settle().await;
        assert_eq!(fake.last_note().meta["state"], "waiting", "{code}");
        fake.set_facts(change);
        wait(300).await;
        assert!(fake.sends().is_empty(), "{code}");
        let note = fake.last_note();
        assert_eq!((note.meta["state"].as_str(), note.meta["code"].as_str()), (Some("skipped"), Some(code)));
        assert!(fake.diary().contains(&format!("auto_continue_skipped:{code}")), "{code}");
    }
    let at_once: [(&str, fn(&mut Facts)); 2] = [
        ("orq", |facts: &mut Facts| facts.orq = true),
        ("handoff", |facts: &mut Facts| facts.handoff = true),
    ];
    for (code, change) in at_once {
        let fake = Fake::new();
        fake.set_facts(change);
        let run = start(&fake);
        run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
        wait(300).await;
        assert!(fake.notes().is_empty(), "{code}");
        assert!(fake.probes().is_empty(), "{code}");
        assert_eq!(fake.diary(), [format!("auto_continue_skipped:{code}")]);
    }
}

/// Lista ilegível por um instante não é sessão sumida: a parada segue esperando e continua quando a lista volta.
#[tokio::test(start_paused = true)]
async fn an_unreadable_list_keeps_the_wait() {
    let fake = Fake::new();
    *fake.facts_error.lock().unwrap() = Some("mux_unavailable".into());
    let run = start(&fake);
    run.send(Command::Stop(hit("s", "line-1", StopKind::Network, fake.clock())));
    settle().await;
    assert_eq!(fake.last_note().meta["state"], "waiting");
    wait(60).await;
    assert!(fake.probes().is_empty(), "sem os fatos não sonda");
    assert!(fake.sends().is_empty());
    assert_eq!(fake.last_note().meta["state"], "waiting");
    assert!(fake.diary().iter().all(|code| code == "auto_continue_facts_unreadable:mux_unavailable"), "{:?}", fake.diary());
    assert!(!fake.diary().is_empty());
    *fake.facts_error.lock().unwrap() = None;
    wait(60).await;
    assert_eq!(fake.sends().len(), 1);
    assert_eq!(fake.sends()[0].2, text::TRANSIENT);
    assert_eq!(fake.last_note().meta["state"], "continued");
    assert!(!fake.diary().iter().any(|code| code.starts_with("auto_continue_skipped")), "{:?}", fake.diary());
}

/// Registro estragado na subida: os tetos recomeçam do zero, e isso vai ao diário em vez de só ao log.
#[tokio::test(start_paused = true)]
async fn a_broken_ledger_goes_to_the_diary() {
    let fake = Fake::new();
    let home = tempfile::tempdir().unwrap();
    let file = ledger::path(home.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"{nao e json").unwrap();
    let shared = Arc::new(Mutex::new(Ledger::load(file)));
    let (_commands, _task) = spawn_with_probe(fake.clone(), shared, fake.clone());
    settle().await;
    assert!(fake.diary().contains(&ledger::UNREADABLE.to_owned()), "{:?}", fake.diary());
}
