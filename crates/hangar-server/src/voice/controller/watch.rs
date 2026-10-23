//! A vigia: lê a lista de sessões em tarefa própria e diz quais acabaram um turno; o laço decide qual resposta falar.
use super::{Controller, Done, Key};
use crate::voice::log;
use crate::voice::machines::Machines;
use crate::voice::rules::{last_reply, question_answer, question_expired, question_marked, triples, waiting_text};
use hangar_api::{chat::ChatEvent, session::SessionRow};
use std::{collections::{HashMap, HashSet}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::sync::mpsc;

const WATCH_EVERY: Duration = Duration::from_millis(1500);
/// O prazo do `PeerClient` é 16 s: peer que falhou fica fora das leituras, senão a vigia ficaria presa nele.
const PEER_BACKOFF: Duration = Duration::from_secs(30);
const HISTORY_LIMIT: usize = 20;

/// Peer → até quando fica fora das leituras.
pub(super) type Backoff = Arc<Mutex<HashMap<String, Instant>>>;

/// Peer que falhou fica marcado até responder de novo. Só a vigia (`retry_failed`) volta a tentá-lo, vencida a espera: a
/// busca de um pedido da voz nunca espera o prazo de um peer caído.
pub(super) async fn read_rows(machines: &Machines, backoff: &Backoff, retry_failed: bool) -> (Vec<(String, SessionRow)>, Vec<String>) {
    let now = Instant::now();
    let (skip, retried): (HashSet<String>, Vec<String>) = {
        let map = backoff.lock().unwrap_or_else(|e| e.into_inner());
        let skip = map.iter().filter(|(_, until)| !retry_failed || **until > now).map(|(k, _)| k.clone()).collect::<HashSet<_>>();
        let retried = map.keys().filter(|k| !skip.contains(*k)).cloned().collect();
        (skip, retried)
    };
    let (rows, unreachable, failed) = machines.sessions_except(&skip).await;
    {
        let mut map = backoff.lock().unwrap_or_else(|e| e.into_inner());
        for peer in retried.iter().filter(|p| !failed.contains(p)) { map.remove(peer); }
    }
    if !failed.is_empty() {
        log(format!("voice peers backed off count={}", failed.len()));
        let until = Instant::now() + PEER_BACKOFF;
        let mut map = backoff.lock().unwrap_or_else(|e| e.into_inner());
        for peer in failed { map.insert(peer, until); }
    }
    (rows, unreachable)
}

/// Fim de turno visto pela lista. A borda não é `working→idle`, que um turno curto pula entre duas leituras: é o
/// `last_reply_at` mudar (ou a sessão ter sido vista trabalhando) e ela estar parada, ou virar `awaiting_input`.
#[derive(Default)]
pub(super) struct Edges { last: HashMap<Key, (Option<f64>, String)>, pending: HashSet<Key> }

impl Edges {
    pub(super) fn update(&mut self, rows: &[(String, SessionRow)]) -> Vec<(Key, SessionRow)> {
        let mut fired = Vec::new();
        for (machine, row) in rows {
            let key = (machine.clone(), row.name.clone());
            let state = row.state.as_str();
            match self.last.get(&key) {
                Some((last_reply_at, last_state)) => {
                    if row.last_reply_at != *last_reply_at || state == "working" { self.pending.insert(key.clone()); }
                    if state == "awaiting_input" && last_state != "awaiting_input" {
                        self.pending.remove(&key);
                        fired.push((key.clone(), row.clone()));
                    } else if state != "working" && self.pending.remove(&key) {
                        fired.push((key.clone(), row.clone()));
                    }
                }
                // Primeira leitura é a base: só "trabalhando" já conta como turno em curso.
                None => if state == "working" { self.pending.insert(key.clone()); },
            }
            self.last.insert(key, (row.last_reply_at, row.state.clone()));
        }
        // Máquina que respondeu sem a sessão: ela fechou. A que não respondeu guarda a base para a volta.
        let answered: HashSet<&str> = rows.iter().map(|(m, _)| m.as_str()).collect();
        let listed: HashSet<(&str, &str)> = rows.iter().map(|(m, r)| (m.as_str(), r.name.as_str())).collect();
        self.last.retain(|(m, n), _| !answered.contains(m.as_str()) || listed.contains(&(m.as_str(), n.as_str())));
        self.pending.retain(|key| self.last.contains_key(key));
        fired
    }
}

pub(super) async fn watcher(machines: Arc<Machines>, backoff: Backoff, done: mpsc::UnboundedSender<Done>) {
    let mut edges = Edges::default();
    let mut tick = tokio::time::interval(WATCH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let (rows, unreachable) = read_rows(&machines, &backoff, true).await;
        let fired = edges.update(&rows);
        if done.send(Done::Watch { rows, unreachable, fired }).is_err() { break; }
    }
}

impl Controller {
    pub(super) fn on_watch(&mut self, rows: Vec<(String, SessionRow)>, unreachable: &[String], fired: Vec<(Key, SessionRow)>) {
        // A chamada conhece os nomes para o `set_mode` não confundir sessão com modo.
        let names: Vec<String> = rows.iter().map(|(_, r)| r.name.clone()).collect();
        if names != self.session_names {
            self.voice.set_sessions(names.clone());
            self.session_names = names;
        }
        self.rows = rows;
        self.rows_changed(unreachable);
        let screen = self.screen_key();
        for (key, row) in fired {
            let asked = self.watched.remove(&key);
            let question = self.pending_question.as_ref().is_some_and(|(k, _)| *k == key);
            if self.muted.contains(&key) && !question { continue; }
            if !(asked || question || self.followed.contains(&key) || screen.as_ref() == Some(&key)) { continue; }
            log(format!("watch turn ended state={} asked={asked} question={question}", row.state));
            let ctx = self.ctx();
            tokio::spawn(async move {
                let result = ctx.machines.history(&key.0, &key.1, HISTORY_LIMIT).await;
                let _ = ctx.done.send(Done::History { key, row, result });
            });
        }
    }

    pub(super) fn on_history(&mut self, key: Key, row: SessionRow, result: Result<Vec<ChatEvent>, String>) {
        // Silenciada (inclusive a leitura que já estava a caminho quando ele pediu para parar): nada é falado, mas a
        // resposta a uma pergunta do organizador (`ask_intercept`) ainda volta a ele.
        let muted = self.muted.contains(&key);
        let events = match result {
            Ok(events) => triples(&events),
            Err(_) => {
                log("history read failed");
                // Sem este aviso o resultado da sessão se perderia calado.
                if !muted { self.voice.session_result(key.1.clone(), format!("Não consegui ler a resposta da sessão {}.", key.1)); }
                return;
            }
        };
        if row.state == "awaiting_input" {
            let questions: Vec<&str> = row.question.as_deref().into_iter().collect();
            let text = waiting_text(&questions, row.last_reply.as_deref());
            if self.ask_intercept(&key, &events, Some(text.clone())) || muted { return; }
            self.voice.session_result(key.1, text);
            return;
        }
        if self.ask_intercept(&key, &events, None) || muted { return; }
        let Some((id, text)) = last_reply(&events) else { return };
        if !self.spoken.insert(id) { return; }
        self.talked.insert(key.clone(), Instant::now());
        self.voice.session_result(key.1, text);
    }

    /// Fim de turno de `key`: se a pergunta pendente é dela, a resposta vai ao organizador e não é falada como resultado.
    /// `waiting`: o que dizer se a sessão parou esperando o usuário sem responder.
    fn ask_intercept(&mut self, key: &Key, events: &[(String, String, String)], waiting: Option<String>) -> bool {
        if self.pending_question.as_ref().is_none_or(|(k, _)| k != key) { return false; }
        let answer = match question_answer(events) {
            Some((id, text)) => self.spoken.insert(id).then_some(text),
            None if question_marked(events) => None,
            // A pergunta ainda não está no histórico lido: este fim de turno é de outro pedido.
            None => return false,
        };
        // Sem resposta ainda: o próximo fim de turno tenta de novo (o prazo segue valendo).
        let Some(text) = answer.or(waiting) else { return true };
        self.pending_question = None;
        log(format!("ask_session answered bytes={}", text.len()));
        self.voice.session_answer(text);
        true
    }

    /// Pergunta sem resposta por `ASK_TIMEOUT` volta ao organizador como falha.
    pub(super) fn ask_expiry(&mut self) {
        let Some((_, since)) = &self.pending_question else { return };
        if !question_expired(*since, Instant::now()) { return; }
        self.pending_question = None;
        log("ask_session timeout");
        self.voice.session_answer("A sessão não respondeu a tempo.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::peers::{PeerBook, PeerClient};
    use crate::voice::machines::{HERE, SelfApi};
    use axum::{Router, routing::get};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn serve(app: Router) -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    }

    /// Aceita a conexão e nunca responde: o servidor fora do ar que custava 8 s a cada busca.
    async fn silent_peer(accepts: Arc<AtomicUsize>) -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = l.accept().await { accepts.fetch_add(1, Ordering::SeqCst); held.push(socket); }
        });
        addr
    }

    #[tokio::test]
    async fn a_lookup_never_waits_for_a_peer_that_failed() {
        let here = serve(Router::new().route("/api/sessions", get(|| async { ([("content-type", "application/json")], "[]") }))).await;
        let ok = serve(Router::new().route("/api/sessions", get(|| async { ([("content-type", "application/json")], "[]") }))).await;
        let accepts = Arc::new(AtomicUsize::new(0));
        let dead = silent_peer(accepts.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("peers.json");
        std::fs::write(&book, json!({"morto": {"base_url": format!("http://{dead}"), "token": "t"},
            "volta": {"base_url": format!("http://{ok}"), "token": "t"}}).to_string()).unwrap();
        let machines = Machines { own: SelfApi::new(here, "t"), peers: Arc::new(PeerClient::new(PeerBook::new(Some(book)))), own_label: "casa".into() };
        let backoff: Backoff = Arc::default();
        // Os dois falharam antes e o prazo de espera já venceu: a vigia tentaria de novo; a busca não.
        let expired = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        backoff.lock().unwrap().extend([("morto".to_owned(), expired), ("volta".to_owned(), expired)]);
        let started = Instant::now();
        let (_, unreachable) = read_rows(&machines, &backoff, false).await;
        assert!(started.elapsed() < Duration::from_secs(2), "a busca não espera o prazo do peer: {:?}", started.elapsed());
        assert_eq!(accepts.load(Ordering::SeqCst), 0, "nem chega a conectar no que falhou");
        assert_eq!(unreachable.len(), 2, "os dois contam como sem resposta");
        backoff.lock().unwrap().remove("morto");
        read_rows(&machines, &backoff, true).await;
        assert!(!backoff.lock().unwrap().contains_key("volta"), "a vigia viu responder: volta às buscas");
        let _ = HERE;
    }
}
