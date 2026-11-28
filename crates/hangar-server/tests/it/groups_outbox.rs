//! Fila de saída entre máquinas: ordem por destino, coalescência, recusa definitiva × nova tentativa.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_server::groups::model::{GroupRecord, Member, OutsideRef};
use hangar_server::groups::outbox::{Delivery, Outbox, OutboxItem, OutboxSend, OutsideOp, MAX_PER_DESTINATION, QUEUE_FULL, Verdict, restart_pushes, verdict};
use hangar_server::groups::peers::PeerError;
use hangar_server::groups::service::BoxFuture;
use serde_json::json;

/// Responde da fila de respostas e, vazia, com a última; guarda o que recebeu.
struct Fake {
    answers: Mutex<VecDeque<Result<(), PeerError>>>, last: fn() -> Result<(), PeerError>, sent: Mutex<Vec<OutboxItem>>, called: tokio::sync::Notify,
    failed: Mutex<Vec<(OutboxItem, String, bool)>>,
}

impl Fake {
    fn new(first: Vec<Result<(), PeerError>>, last: fn() -> Result<(), PeerError>) -> Arc<Fake> {
        Arc::new(Fake { answers: Mutex::new(first.into()), last, sent: Mutex::default(), called: tokio::sync::Notify::new(), failed: Mutex::default() })
    }
    fn ok() -> Arc<Fake> { Fake::new(vec![], || Ok(())) }
    fn down() -> Arc<Fake> { Fake::new(vec![], || Err(PeerError::Transport("lab inacessível".into()))) }
    fn sent(&self) -> Vec<OutboxItem> { self.sent.lock().unwrap().clone() }
    fn failed(&self) -> Vec<(OutboxItem, String, bool)> { self.failed.lock().unwrap().clone() }
}

impl OutboxSend for Fake {
    fn failed(&self, item: &OutboxItem, code: &str, _error: &PeerError, retry: bool) {
        self.failed.lock().unwrap().push((item.clone(), code.to_owned(), retry));
    }

    fn send<'a>(&'a self, item: &'a OutboxItem) -> BoxFuture<'a, Result<(), PeerError>> {
        Box::pin(async move {
            self.sent.lock().unwrap().push(item.clone());
            let answer = self.answers.lock().unwrap().pop_front().unwrap_or_else(self.last);
            self.called.notify_one();
            answer
        })
    }
}

const LONG: [Duration; 1] = [Duration::from_secs(600)];
const SHORT: [Duration; 1] = [Duration::from_millis(10)];
const DEADLINE: Duration = Duration::from_secs(10);

fn push(server: &str, gid: &str, version: u64, joining: &[&str]) -> OutboxItem {
    OutboxItem::Push { server: server.into(), gid: gid.into(), version, joining: joining.iter().map(|j| j.to_string()).collect(), origin: "casa".into() }
}

fn refused(status: u16, code: Option<&str>) -> PeerError {
    let detail = match code { Some(c) => json!({"code": c, "params": {}, "msg": "x"}), None => json!("x") };
    PeerError::Refused { status, detail }
}

fn open(tmp: &tempfile::TempDir, sender: Arc<Fake>, ladder: &[Duration]) -> Outbox {
    Outbox::open_with_ladder(tmp.path().join("groups"), sender, ladder.to_vec()).unwrap()
}

/// Espera o trabalhador de `dest` sair; teto generoso, só estoura se ele ficar vivo.
async fn worker_gone(outbox: &Outbox, dest: &str) {
    let gone = tokio::time::timeout(DEADLINE, async {
        while outbox.workers().iter().any(|w| w == dest) { tokio::task::yield_now().await; }
    }).await;
    assert!(gone.is_ok(), "o trabalhador de {dest} continuou vivo sem item");
}

/// Espera o dublê receber `n` envios, pelo aviso de cada chamada.
async fn sent_at_least(fake: &Fake, n: usize) {
    let reached = tokio::time::timeout(DEADLINE, async { while fake.sent().len() < n { fake.called.notified().await; } }).await;
    assert!(reached.is_ok(), "o dublê recebeu {} envios, esperava {n}", fake.sent().len());
}

#[tokio::test]
async fn outbox_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let down = Fake::down();
    let first = open(&tmp, down.clone(), &LONG);
    first.enqueue(vec![push("lab", "g", 3, &["a"])]).unwrap();
    down.called.notified().await;
    assert!(tmp.path().join("groups/outbox.json").is_file(), "a fila vai ao disco antes da rede");
    let up = Fake::ok();
    let second = open(&tmp, up.clone(), &LONG);
    assert_eq!(second.wait("lab", "g", 3, DEADLINE).await, Delivery::Confirmed);
    assert_eq!(up.sent(), [push("lab", "g", 3, &["a"])], "a subida reenvia o que estava na fila, com o joining");
    assert!(second.pending().is_empty());
}

#[tokio::test]
async fn push_without_joining_coalesces() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::down(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &["a"]), push("vm", "g", 1, &[])]).unwrap();
    outbox.enqueue(vec![push("lab", "g", 2, &[]), push("vm", "g", 2, &[])]).unwrap();
    outbox.enqueue(vec![push("lab", "g", 3, &[])]).unwrap();
    // Um Push por destino e gid: a versão nova, sem perder quem ainda está entrando.
    assert_eq!(outbox.pending(), [push("lab", "g", 3, &["a"]), push("vm", "g", 2, &[])]);
    // Push com `joining` não some dentro de outro.
    outbox.enqueue(vec![push("lab", "g", 4, &["b"])]).unwrap();
    assert_eq!(outbox.pending().len(), 3);
}

#[tokio::test]
async fn dissolved_replaces_pending_push() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::down(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &["a"]), push("vm", "g", 1, &["b"]), push("lab", "h", 1, &[])]).unwrap();
    let dissolved = OutboxItem::Dissolved { server: "lab".into(), gid: "g".into(), version: 2, origin: "casa".into() };
    outbox.enqueue(vec![dissolved.clone()]).unwrap();
    assert_eq!(outbox.pending(), [push("vm", "g", 1, &["b"]), push("lab", "h", 1, &[]), dissolved]);
}

#[tokio::test]
async fn refusal_leaves_the_queue() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![], || Err(refused(409, Some("erro_grupo_mudou"))));
    let outbox = open(&tmp, sender.clone(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &["a"])]).unwrap();
    assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Refused { code: "erro_grupo_mudou".into() });
    assert!(outbox.pending().is_empty());
    worker_gone(&outbox, "lab").await;
    assert_eq!(sender.sent().len(), 1, "recusa definitiva não tenta de novo");
    let reopened = open(&tmp, Fake::ok(), &LONG);
    assert!(reopened.pending().is_empty(), "a recusa saiu do disco também");
}

#[tokio::test]
async fn python_mode_503_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![Err(refused(503, Some("groups_need_rust_server")))], || Ok(()));
    let outbox = open(&tmp, sender.clone(), &SHORT);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Confirmed);
    assert_eq!(sender.sent().len(), 2);
}

#[tokio::test]
async fn worker_exists_only_with_items() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::ok();
    let outbox = open(&tmp, sender.clone(), &LONG);
    assert!(outbox.workers().is_empty(), "fila vazia, nenhum trabalhador");
    let leave = OutboxItem::Leave { owner: "casa2".into(), gid: "g".into(), member: "a".into(), origin: "lab".into() };
    let link = OutboxItem::Outside { owner: "casa2".into(), gid: "g".into(), op: OutsideOp::Link(OutsideRef { server: "lab".into(), address: "ana::x".into() }), origin: "lab".into() };
    outbox.enqueue(vec![push("vm", "g", 1, &[]), leave.clone(), link.clone()]).unwrap();
    assert_eq!(outbox.wait("vm", "g", 1, DEADLINE).await, Delivery::Confirmed);
    worker_gone(&outbox, "vm").await;
    worker_gone(&outbox, "casa2").await;
    assert!(outbox.pending().is_empty());
    let sent = sender.sent();
    let casa2: Vec<_> = sent.iter().filter(|i| **i != push("vm", "g", 1, &[])).cloned().collect();
    assert_eq!(casa2, [leave, link], "saída e par de fora vão à dona, em ordem");
}

#[tokio::test]
async fn wait_after_backoff_retries_now() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![Err(PeerError::Transport("lab inacessível".into()))], || Ok(()));
    let outbox = open(&tmp, sender.clone(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    sender.called.notified().await;
    // O trabalhador dorme 10 min pela falha antiga; o item novo o acorda dentro do prazo do `wait`.
    outbox.enqueue(vec![push("lab", "h", 1, &["a"])]).unwrap();
    assert_eq!(outbox.wait("lab", "h", 1, Duration::from_secs(5)).await, Delivery::Confirmed);
    assert_eq!(outbox.wait("lab", "g", 1, Duration::from_secs(5)).await, Delivery::Confirmed);
}

#[test]
fn busy_429_401_408_are_retried() {
    for err in [refused(409, Some("session_transfer_busy")), refused(409, Some("erro_sessao_trabalhando")),
                refused(429, Some("erro_x")), refused(401, None), refused(408, None),
                refused(503, Some("groups_need_rust_server")), PeerError::Transport("x".into()), PeerError::Unknown] {
        assert_eq!(verdict(&err), Verdict::Retry, "{err:?}");
    }
    for (err, code) in [(refused(409, Some("erro_grupo_mudou")), "erro_grupo_mudou"),
                        (refused(404, Some("erro_grupo_inexistente")), "erro_grupo_inexistente"),
                        (refused(400, None), "http_400"),
                        (PeerError::IdentityMismatch { key: "lab".into(), reported: "vm".into() }, "erro_grupo_identificador_divergente")] {
        assert_eq!(verdict(&err), Verdict::Refused(code.into()), "{err:?}");
    }
}

#[tokio::test]
async fn unknown_peer_is_retried_with_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![], || Err(PeerError::Unknown));
    let outbox = open(&tmp, sender.clone(), &SHORT);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    assert_eq!(outbox.wait("lab", "g", 1, Duration::from_millis(300)).await, Delivery::Pending);
    assert!(sender.sent().len() >= 2, "tentou de novo");
    assert_eq!(outbox.pending(), [push("lab", "g", 1, &[])], "nunca descartado");
}

#[tokio::test]
async fn cancel_joining_drops_member_from_pending_push() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::down(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &["a", "b"]), push("vm", "g", 1, &["a"]), push("lab", "h", 1, &["a"])]).unwrap();
    outbox.cancel_joining("lab", "g", "a").unwrap();
    assert_eq!(outbox.pending(), [push("lab", "g", 1, &["b"]), push("vm", "g", 1, &["a"]), push("lab", "h", 1, &["a"])]);
    let reopened = open(&tmp, Fake::down(), &LONG);
    assert_eq!(reopened.pending()[0], push("lab", "g", 1, &["b"]), "o corte foi ao disco");
}

#[test]
fn owner_restart_pushes_each_other_server_once() {
    let member = |server: &str, name: &str| Member { server: server.into(), name: name.into(), provider: String::new() };
    let record = |gid: &str, owner: &str| GroupRecord { protocol: 1, gid: gid.into(), owner: owner.into(), version: 4, task: String::new(),
        members: vec![member("casa", "a"), member("lab", "b"), member("lab", "c"), member("vm", "d")], outside: vec![], links: Default::default() };
    let items = restart_pushes(&[record("g", "casa"), record("h", "lab")], "casa");
    let push = |server: &str| OutboxItem::Push { server: server.into(), gid: "g".into(), version: 4, joining: vec![], origin: "casa".into() };
    assert_eq!(items, [push("lab"), push("vm")], "só os grupos desta dona, uma vez por máquina");
}

mod disk_proptest {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn outbox_file_from_disk_never_panics(raw in prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..64),
            "[{}\\[\\]\":,a-z0-9 ]{0,64}".prop_map(String::into_bytes),
            Just(br#"{"next":1,"items":[{"id":0,"item":{"kind":"push"}}]}"#.to_vec()),
            Just(br#"{"next":0,"items":[{"id":5,"item":{"kind":"leave","owner":"","gid":"","member":"","origin":""}}]}"#.to_vec()),
        ]) {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join("groups")).unwrap();
            std::fs::write(tmp.path().join("groups/outbox.json"), &raw).unwrap();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                if let Ok(outbox) = Outbox::open_with_ladder(tmp.path().join("groups"), Fake::down(), LONG.to_vec()) {
                    let _ = outbox.pending();
                    let _ = outbox.enqueue(vec![push("lab", "g", 1, &[])]);
                }
            });
        }
    }
}

#[tokio::test]
async fn server_error_is_retried() {
    for err in [refused(503, Some("groups_need_rust_server")), refused(502, None), refused(500, Some("erro_x"))] {
        assert_eq!(verdict(&err), Verdict::Retry, "{err:?}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![Err(refused(503, Some("groups_need_rust_server"))), Err(refused(502, None))], || Ok(()));
    let outbox = open(&tmp, sender.clone(), &SHORT);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Confirmed);
    assert_eq!(sender.sent().len(), 3, "as duas falhas de servidor ficaram na fila e foram tentadas de novo");
}

#[tokio::test]
async fn wait_on_dissolved_push_returns_now() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::down(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &["a"])]).unwrap();
    let waiting = outbox.wait("lab", "g", 1, DEADLINE);
    let dissolve = async {
        tokio::task::yield_now().await;
        outbox.enqueue(vec![OutboxItem::Dissolved { server: "lab".into(), gid: "g".into(), version: 2, origin: "casa".into() }]).unwrap();
    };
    let (got, ()) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(waiting, dissolve) }).await.expect("o wait esperou o prazo inteiro");
    assert_eq!(got, Delivery::Dissolved);
}

#[tokio::test]
async fn old_version_is_retried_not_dropped() {
    assert_eq!(verdict(&PeerError::OldVersion), Verdict::Retry);
    let tmp = tempfile::tempdir().unwrap();
    let sender = Fake::new(vec![Err(PeerError::OldVersion)], || Ok(()));
    let outbox = open(&tmp, sender.clone(), &SHORT);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    // A máquina pode estar atualizando, ou com o Python atendendo enquanto o Rust volta.
    assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Confirmed);
    assert_eq!(sender.sent().len(), 2);
}

#[tokio::test]
async fn corrupt_outbox_is_set_aside_and_queue_starts() {
    for raw in ["{", ""] {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("groups")).unwrap();
        std::fs::write(tmp.path().join("groups/outbox.json"), raw).unwrap();
        let outbox = open(&tmp, Fake::ok(), &LONG);
        assert!(outbox.pending().is_empty(), "{raw:?}");
        assert_eq!(std::fs::read_to_string(tmp.path().join("groups/outbox.json.bad")).unwrap(), raw, "o arquivo torto fica guardado");
        outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
        assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Confirmed, "a fila anda depois");
    }
}

#[tokio::test]
async fn queue_cap_is_a_visible_error() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::down(), &LONG);
    let leave = |n: usize| OutboxItem::Leave { owner: "casa".into(), gid: "g".into(), member: format!("m{n}"), origin: "lab".into() };
    outbox.enqueue((0..MAX_PER_DESTINATION).map(leave).collect()).unwrap();
    let err = outbox.enqueue(vec![leave(MAX_PER_DESTINATION)]).unwrap_err();
    assert_eq!(err.to_string(), QUEUE_FULL);
    assert_eq!(outbox.pending().len(), MAX_PER_DESTINATION, "a recusa não grava nada");
    // Outro destino segue livre.
    outbox.enqueue(vec![push("vm", "g", 1, &[])]).unwrap();
}

/// Segura o envio do `Push` até `gate` abrir; o `Dissolved` fica na fila (rede fora).
struct Gate { gate: tokio::sync::Semaphore, started: tokio::sync::Notify, next: tokio::sync::Notify }
impl OutboxSend for Gate {
    fn failed(&self, _item: &OutboxItem, _code: &str, _error: &PeerError, _retry: bool) {}
    fn send<'a>(&'a self, item: &'a OutboxItem) -> BoxFuture<'a, Result<(), PeerError>> {
        Box::pin(async move {
            if !matches!(item, OutboxItem::Push { .. }) { self.next.notify_one(); return Err(PeerError::Transport("fora".into())); }
            self.started.notify_one();
            let _permit = self.gate.acquire().await.unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn push_confirmed_after_dissolve_reports_dissolved() {
    let tmp = tempfile::tempdir().unwrap();
    let gate = Arc::new(Gate { gate: tokio::sync::Semaphore::new(0), started: tokio::sync::Notify::new(), next: tokio::sync::Notify::new() });
    let outbox = Outbox::open_with_ladder(tmp.path().join("groups"), gate.clone(), LONG.to_vec()).unwrap();
    outbox.enqueue(vec![push("lab", "g", 1, &["a"])]).unwrap();
    gate.started.notified().await;
    outbox.enqueue(vec![OutboxItem::Dissolved { server: "lab".into(), gid: "g".into(), version: 2, origin: "casa".into() }]).unwrap();
    gate.gate.add_permits(1);
    // O Push em voo confirma depois do Dissolved (o trabalhador já passou ao item seguinte):
    // quem espera ouve que não há o que confirmar.
    gate.next.notified().await;
    assert_eq!(outbox.wait("lab", "g", 1, Duration::from_secs(5)).await, Delivery::Dissolved);
}

/// Um FIFO no lugar do temporário prende a escrita do disco; a fila em memória segue legível.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enqueue_does_not_block_wait_during_write() {
    let tmp = tempfile::tempdir().unwrap();
    let outbox = open(&tmp, Fake::ok(), &LONG);
    outbox.enqueue(vec![push("lab", "g", 1, &[])]).unwrap();
    assert_eq!(outbox.wait("lab", "g", 1, DEADLINE).await, Delivery::Confirmed);
    let fifo = tmp.path().join("groups/outbox.json.tmp");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let (started, writing) = tokio::sync::oneshot::channel();
    let writer = { let o = outbox.clone(); tokio::task::spawn_blocking(move || { let _ = started.send(()); o.enqueue(vec![push("vm", "g", 1, &[])]) }) };
    // As leituras só começam com a escrita já a caminho do FIFO.
    writing.await.unwrap();
    // Sem espera fixa: lê várias vezes com a escrita presa no FIFO; a leitura que esperasse o disco estoura o prazo.
    let mut probe = Ok(());
    for _ in 0..200 {
        let reader = { let o = outbox.clone(); tokio::task::spawn_blocking(move || o.pending().len()) };
        if let Err(e) = tokio::time::timeout(Duration::from_secs(5), reader).await { probe = Err(e); break; }
        tokio::task::yield_now().await;
    }
    let held = !writer.is_finished();
    // Solta a escrita presa antes de qualquer asserção, para a thread não sobrar.
    let drain = std::thread::spawn(move || { let mut s = String::new(); let _ = std::io::Read::read_to_string(&mut std::fs::File::open(&fifo).unwrap(), &mut s); });
    let _ = writer.await;
    drain.join().unwrap();
    assert!(held, "a escrita deveria seguir presa no FIFO durante as leituras");
    assert!(probe.is_ok(), "ler a fila esperou a escrita do disco");
    assert!(tokio::time::timeout(Duration::from_secs(5), outbox.wait("lab", "g", 1, DEADLINE)).await.is_ok());
}

/// Recusa definitiva tira o item da fila sem ninguém esperando (`Leave`, `Rename`): quem usa a
/// fila é avisado com o item e o código. Nova tentativa (rede, token recusado) avisa uma vez por
/// minuto por destino, não a cada volta da escada.
#[tokio::test]
async fn refusals_and_retries_reach_who_uses_the_queue() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = Fake::new(vec![Err(refused(404, Some("erro_grupo_inexistente")))], || Err(PeerError::Transport("fora".into())));
    let outbox = open(&tmp, fake.clone(), &SHORT);
    let leave = OutboxItem::Leave { owner: "rel-refusal".into(), gid: "g".into(), member: "a".into(), origin: "casa".into() };
    let rename = OutboxItem::Rename { owner: "rel-refusal".into(), gid: "g".into(), from: "a".into(), to: "b".into(), origin: "casa".into() };
    outbox.enqueue(vec![leave.clone(), rename.clone()]).unwrap();
    // A escada curta tenta de novo.
    sent_at_least(&fake, 5).await;
    assert_eq!(fake.failed(), [(leave, "erro_grupo_inexistente".into(), false), (rename.clone(), "grupos.peer_inacessivel".into(), true)]);

    let token = Fake::new(vec![], || Err(refused(401, None)));
    let other = open(&tempfile::tempdir().unwrap(), token.clone(), &SHORT);
    let rename = OutboxItem::Rename { owner: "rel-token".into(), gid: "g".into(), from: "a".into(), to: "b".into(), origin: "casa".into() };
    other.enqueue(vec![rename.clone()]).unwrap();
    sent_at_least(&token, 3).await;
    assert_eq!(token.failed(), [(rename, "grupos.peer_token_recusado".into(), true)]);
}
