//! A resposta final da filha ligada volta sozinha ao pai como `[de: <filha>] …`. Só roda com filha
//! ligada: enquanto houver alguma, segura uma assinatura interna da lista que não conta como app
//! aberto; sem nenhuma, dorme até o grupo mudar no disco.
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::http::StatusCode;
use hangar_api::chat::{ChatEvent, ChatKind};
use hangar_api::session::SessionRow;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::link_state::{self, LinkReturn, LinkReturns, blocking};
use super::local::is_remote;
use super::service::{GroupError, GroupService};
use super::store::{StoreError, file_stem};
use crate::routes::AppState;
use crate::state::monitor::wall_now;
use crate::transcript::history::merged_history_capped;
use crate::transcript::{HistoryRequest, Provider, TAIL_WINDOW};

const TICK: Duration = Duration::from_secs(2);
/// Parada há tanto tempo seguido: entre dois turnos a linha passa por `idle` por um instante.
const SETTLE: Duration = Duration::from_secs(20);
/// Resposta acumulada no modo `python` não chega horas depois.
const MAX_AGE: f64 = 2.0 * 3600.0;
/// Janelas de eventos lidas para trás até achar a última mensagem do usuário; a última só para no teto de bytes.
const WINDOWS: [usize; 2] = [400, usize::MAX];
const MAX_READ: u64 = 32 * 1024 * 1024;
const MAX_CHARS: usize = 8000;
/// Sem filha nas linhas, o arquivo de grupo confirma antes de soltar: a lista pode ser de antes da ligação.
const RESCAN: Duration = Duration::from_secs(30);
/// Pai e filha trocando turnos sozinhos: cada volta leva 20 s ou mais, então o limite é por filha e longo.
const RATE_MAX: usize = 3;
const RATE_WINDOW: Duration = Duration::from_secs(600);
/// Falha passageira (histórico ilegível, pai ocupado ou reabrindo) tenta de novo depois disso.
const BACKOFF: Duration = Duration::from_secs(30);
const EVENT: &str = "rust.link_return_failed";

static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);

/// O grupo mudou no disco: pode ter nascido uma filha ligada.
pub fn wake() { WAKE.notify_one(); }

pub fn spawn(st: Arc<AppState>) -> Option<JoinHandle<()>> {
    let groups = st.groups.clone()?;
    Some(tokio::spawn(async move {
        // Pânico não pode parar o retorno de vez e calado; pânico em laço não pode girar a cada 2 s.
        let mut pause = PANIC_PAUSE;
        loop {
            // Abortar o laço de fora tem de abortar o `run`: soltar o JoinHandle só desanexa.
            let mut inner = crate::AbortOnDrop(tokio::spawn(run(st.clone(), groups.clone())));
            match (&mut inner.0).await {
                Err(e) if e.is_panic() => {
                    report(&st, "", "link_return_panicked", "retorno da filha ligada caiu; recomeça", "");
                    tokio::time::sleep(pause).await;
                    pause = next_pause(pause);
                }
                _ => return,
            }
        }
    }))
}

const PANIC_PAUSE: Duration = Duration::from_secs(2);
const PANIC_PAUSE_MAX: Duration = Duration::from_secs(60);

fn next_pause(pause: Duration) -> Duration { (pause * 2).min(PANIC_PAUSE_MAX) }

/// Sem filha devolvível, o que esperar antes de varrer de novo.
#[derive(Debug, PartialEq)]
enum Idle {
    /// Há filha devolvível: vigia a lista.
    Watch,
    /// Nenhuma: só a mudança do grupo no disco acorda.
    UntilWake,
    /// Arquivos de grupo ilegíveis: a mudança que os conserta pode não passar pelo `wake`.
    Retry(Duration),
}

fn idle(links: Option<&[(String, String)]>) -> Idle {
    match links {
        None => Idle::Retry(RESCAN),
        Some(l) if any_returnable(l, None) => Idle::Watch,
        Some(_) => Idle::UntilWake,
    }
}

async fn run(st: Arc<AppState>, groups: Arc<GroupService>) {
    loop {
        match idle(scan_links(&st, &groups).await.as_deref()) {
            Idle::Watch => {}
            Idle::UntilWake => { WAKE.notified().await; continue }
            Idle::Retry(after) => { let _ = tokio::time::timeout(after, WAKE.notified()).await; continue }
        }
        let sub = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
        let mut watcher = Watcher::default();
        let mut scanned = Instant::now();
        loop {
            tokio::time::sleep(TICK).await;
            // Lista em erro: não dá para provar que a filha ficou parada o tempo todo.
            let Some(rows) = sub.rows() else { watcher.idle.clear(); continue };
            let children: Vec<&SessionRow> = rows.iter()
                .filter(|r| r.linked_by.as_deref().is_some_and(|p| !is_remote(p)) && returnable(&r.provider)).collect();
            if !children.is_empty() {
                watcher.round(&st, groups.server_id(), &children, &|| sub.rows()).await;
            }
            if scanned.elapsed() >= RESCAN {
                scanned = Instant::now();
                if scan_links(&st, &groups).await.is_some_and(|l| !any_returnable(&l, Some(&rows))) { break; }
            }
        }
    }
}

/// Filha renomeada: o registro dela segue o nome novo. Falhar não desfaz a troca de nome, já feita.
pub(crate) async fn renamed(st: &AppState, old: &str, new: &str) {
    let (dir, from, to) = (st.hangar_dir.clone(), old.to_owned(), new.to_owned());
    if let Err(e) = blocking(move || link_state::rename(&dir, &from, &to)).await {
        report(st, new, "link_returns_rename_failed", "registro da filha ligada não seguiu o nome novo", &io_kind(&e));
    }
}

/// Pi, omp e Kimi não têm retorno automático: nem seguram a assinatura da lista.
fn returnable(provider: &str) -> bool {
    matches!(Provider::parse(provider), Some(Provider::Claude | Provider::ClaudeHeadless | Provider::Codex))
}

/// Filhas com vínculo válido (o pai entre os `peers`), como (stem, provedor do `harness`), e poda o
/// `link-returns.json` das que não têm mais. `None` = arquivos de grupo ilegíveis, já relatado.
async fn scan_links(st: &AppState, groups: &GroupService) -> Option<Vec<(String, String)>> {
    let all = match groups.sidecars().await {
        Ok(all) => all,
        Err(e) => {
            let detail = match &e { GroupError::Store(StoreError::Io(io)) => io_kind(io), GroupError::Store(StoreError::Json(_)) => "json".into(), _ => "other".into() };
            report(st, "", "link_return_groups_unreadable", "arquivos de grupo ilegíveis; retorno da filha parado", &detail);
            return None
        }
    };
    let linked: Vec<(String, String)> = all.into_iter()
        .filter(|(_, s)| s.linked_parent().is_some())
        .map(|(stem, s)| {
            let provider = s.harness.iter().find(|(n, _)| file_stem(n) == stem).map(|(_, p)| p.clone()).unwrap_or_default();
            (stem, provider)
        }).collect();
    let (dir, stems) = (st.hangar_dir.clone(), linked.iter().map(|(s, _)| s.clone()).collect::<HashSet<_>>());
    if let Err(e) = blocking(move || link_state::retain(&dir, |name| stems.contains(&file_stem(name)))).await { unreadable(st, &e); }
    Some(linked)
}

/// Provedor da linha quando ela existe; senão o do arquivo de grupo; desconhecido conta como devolvível.
fn any_returnable(linked: &[(String, String)], rows: Option<&[SessionRow]>) -> bool {
    // A primeira linha de cada stem vence, como o `find` por filha fazia.
    let mut by_stem: HashMap<String, &str> = HashMap::new();
    for r in rows.unwrap_or_default() { by_stem.entry(file_stem(&r.name)).or_insert(&r.provider); }
    linked.iter().any(|(stem, harness)| {
        let provider = by_stem.get(stem).copied().unwrap_or(harness);
        provider.is_empty() || returnable(provider)
    })
}

/// Retornos por filha numa janela; conta a tentativa quando deixa passar. O segundo mapa guarda o último
/// aviso de limite: um aviso por resposta manteria o laço girando no mesmo ritmo.
#[derive(Default)]
struct RateLimit(HashMap<String, VecDeque<Instant>>, HashMap<String, Instant>);

impl RateLimit {
    fn allow(&mut self, child: &str, now: Instant) -> bool {
        let hits = self.0.entry(child.to_owned()).or_default();
        while hits.front().is_some_and(|t| now.duration_since(*t) >= RATE_WINDOW) { hits.pop_front(); }
        hits.len() < RATE_MAX
    }

    /// Só entrega feita conta: tentativa que falhou passageira e volta não pode gastar vaga.
    fn record(&mut self, child: &str, now: Instant) { self.0.entry(child.to_owned()).or_default().push_back(now); }

    /// Um aviso de limite por filha por janela.
    fn notice_once(&mut self, child: &str, now: Instant) -> bool {
        if self.1.get(child).is_some_and(|t| now.duration_since(*t) < RATE_WINDOW) { return false; }
        self.1.insert(child.to_owned(), now);
        true
    }

    fn forget_old(&mut self, now: Instant) {
        self.0.retain(|_, hits| hits.back().is_some_and(|t| now.duration_since(*t) < RATE_WINDOW));
        self.1.retain(|_, t| now.duration_since(*t) < RATE_WINDOW);
    }
}

/// As linhas publicadas agora, para conferir antes de entregar.
type Rows<'a> = dyn Fn() -> Option<Arc<Vec<SessionRow>>> + Sync + 'a;

/// Como terminou a tentativa de devolver uma resposta.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// Devolvida ou recusada de vez: esta resposta não é relida.
    Done,
    /// Falha passageira: tenta de novo depois de `BACKOFF`.
    Retry,
    /// A linha mudou entre a lista e o histórico: decide de novo pelo relógio dela.
    Moved,
}

#[derive(Default)]
struct Watcher {
    /// Desde quando a filha está parada nesta resposta (`last_reply_at`).
    idle: HashMap<String, (Instant, Option<f64>)>,
    /// Última resposta já decidida: não relê nem reentrega.
    handled: HashMap<String, f64>,
    retry_at: HashMap<String, Instant>,
    rate: RateLimit,
    /// Filhas com registro em `link-returns.json` já conferido nesta vigia.
    known: HashSet<String>,
}

impl Watcher {
    fn due(&self, child: &str, reply_at: f64, now: Instant) -> bool {
        self.handled.get(child) != Some(&reply_at) && self.retry_at.get(child).is_none_or(|t| now >= *t)
    }

    fn finish(&mut self, child: &str, outcome: Outcome, now: Instant) {
        match outcome {
            Outcome::Done => { self.retry_at.remove(child); }
            Outcome::Retry => { self.handled.remove(child); self.retry_at.insert(child.to_owned(), now + BACKOFF); }
            Outcome::Moved => { self.handled.remove(child); }
        }
    }

    async fn round(&mut self, st: &AppState, server: &str, children: &[&SessionRow], current: &Rows<'_>) {
        let now = Instant::now();
        let present = |n: &String| children.iter().any(|r| &r.name == n);
        self.idle.retain(|n, _| present(n));
        self.handled.retain(|n, _| present(n));
        self.retry_at.retain(|n, _| present(n));
        self.known.retain(|n| present(n));
        self.rate.forget_old(now);
        // Já na primeira vista: o registro criado aqui só deixa voltar resposta posterior a ele.
        let unseen: Vec<String> = children.iter().map(|r| r.name.clone()).filter(|n| !self.known.contains(n)).collect();
        if !unseen.is_empty() {
            let (dir, names, at) = (st.hangar_dir.clone(), unseen.clone(), wall_now());
            match blocking(move || link_state::ensure_linked(&dir, &names, at)).await {
                Ok(created) => {
                    for child in created {
                        report(st, &child, "link_returns_entry_missing", "filha ligada sem registro de ligação; só respostas de agora em diante voltam", "");
                    }
                    self.known.extend(unseen);
                }
                Err(e) => return unreadable(st, &e),
            }
        }
        let mut links: Option<LinkReturns> = None;
        for row in children {
            if row.state != "idle" {
                self.idle.remove(&row.name);
                continue;
            }
            let since = match self.idle.get(&row.name) {
                Some((since, at)) if *at == row.last_reply_at => *since,
                _ => { self.idle.insert(row.name.clone(), (now, row.last_reply_at)); now }
            };
            let Some(reply_at) = row.last_reply_at else { continue };
            if now - since < SETTLE || !self.due(&row.name, reply_at, now) { continue; }
            if links.is_none() {
                let dir = st.hangar_dir.clone();
                match blocking(move || link_state::read(&dir)).await {
                    Ok(read) => links = Some(read),
                    Err(e) => return unreadable(st, &e),
                }
            }
            let check = Check {
                last_reply_at: row.last_reply_at, link: links.as_ref().and_then(|l| l.get(&row.name)).cloned(), now: wall_now(),
                parent: row.linked_by.as_deref().unwrap_or_default(), server,
            };
            self.handled.insert(row.name.clone(), reply_at);
            if !settled(&check) { continue; }
            let outcome = give_back(st, row, &check, reply_at, &mut self.rate, current).await;
            self.finish(&row.name, outcome, now);
        }
    }
}

/// O que a decisão precisa saber da filha, já lido.
pub(crate) struct Check<'a> {
    pub last_reply_at: Option<f64>,
    pub link: Option<LinkReturn>,
    /// Relógio de parede, em segundos (o mesmo de `last_reply_at`).
    pub now: f64,
    pub parent: &'a str,
    /// Id desta máquina; vazio quando não configurado.
    pub server: &'a str,
}

/// Critério 3 pela linha (parada há `SETTLE` e provedor devolvível já filtrados no `round`): resposta
/// nova desde a ligação e o último retorno, com menos de `MAX_AGE`.
fn settled(c: &Check) -> bool {
    let (Some(at), Some(link)) = (c.last_reply_at, &c.link) else { return false };
    let since = link.returned_at.map_or(link.linked_at, |r| r.max(link.linked_at));
    at > since && c.now - at < MAX_AGE
}

/// Eventos do fim da conversa que chegam até a última mensagem do usuário; `whole` = desde o começo.
struct Turn { events: Vec<ChatEvent>, whole: bool }

/// Lê para trás crescendo a janela até achar a última mensagem do usuário. `None` = não achada no teto.
fn read_turn(mut read: impl FnMut(usize) -> io::Result<Vec<ChatEvent>>, file_len: u64) -> io::Result<Option<Turn>> {
    for limit in WINDOWS {
        let events = read(limit)?;
        let has_user = events.iter().any(|e| e.kind == ChatKind::UserMsg);
        // Menos eventos que o pedido: a leitura parou no começo do arquivo ou no teto de bytes.
        let exhausted = events.len() < limit;
        if has_user || exhausted {
            let whole = exhausted && file_len <= MAX_READ;
            return Ok((has_user || whole).then_some(Turn { events, whole }));
        }
    }
    Ok(None)
}

/// Critério 4: a última mensagem do usuário veio do pai, ou é a primeira da conversa (o
/// `initial_prompt`). Turno dado à mão na filha não volta.
fn asked_by_parent(turn: &Turn, parent: &str, server: &str) -> bool {
    let mut users = turn.events.iter().filter(|e| e.kind == ChatKind::UserMsg);
    let Some(last) = users.next_back() else { return false };
    let text = last.text.as_deref().unwrap_or_default().trim_start();
    let from_parent = text.starts_with(&format!("[de: {parent}]"))
        || (!server.is_empty() && text.starts_with(&format!("[de: {server}::{parent}]")));
    from_parent || (turn.whole && users.next().is_none())
}

/// Por que a resposta não volta.
#[derive(Debug, PartialEq)]
enum Refusal {
    NotAsked,
    /// Pedido pelo pai, mas sem resposta final em texto; leva a identidade do turno para o aviso.
    NoAnswer(String),
    AlreadyReturned,
    TooOld,
}

/// Critérios 4 em diante, sobre o turno lido: devolve o texto e a identidade da resposta.
fn decide<'a>(c: &Check, turn: &'a Turn) -> Result<(&'a str, String), Refusal> {
    if !asked_by_parent(turn, c.parent, c.server) { return Err(Refusal::NotAsked); }
    let returned = |key: &str| c.link.as_ref().and_then(|l| l.returned_key.as_deref()) == Some(key);
    let Some((answer, asked)) = final_answer(&turn.events) else {
        let key = unanswered_key(&turn.events);
        return Err(if returned(&key) { Refusal::AlreadyReturned } else { Refusal::NoAnswer(key) });
    };
    let key = answer_key(answer, asked);
    if returned(&key) { return Err(Refusal::AlreadyReturned); }
    // `last_reply_at` cai no mtime do transcript sem hora no evento; a do evento é a da resposta.
    let at = answer.ts.filter(|&t| t != 0.0).or(c.last_reply_at).unwrap_or(0.0);
    if c.now - at >= MAX_AGE { return Err(Refusal::TooOld); }
    Ok((answer.text.as_deref().unwrap_or_default(), key))
}

fn blank_answer(e: &ChatEvent) -> bool { e.kind == ChatKind::AssistantMsg && e.text.as_deref().is_none_or(|t| t.trim().is_empty()) }

/// A resposta final só existe se o turno terminou nela: interrompido ou parado em ferramenta traz
/// texto pela metade. Volta junto a mensagem do usuário que ela responde.
fn final_answer(events: &[ChatEvent]) -> Option<(&ChatEvent, Option<&ChatEvent>)> {
    let asked = events.iter().rposition(|e| e.kind == ChatKind::UserMsg);
    let turn = &events[asked.map_or(0, |i| i + 1)..];
    if turn.iter().any(|e| e.kind == ChatKind::Notice && matches!(e.text.as_deref(), Some("interrupted" | "turn_aborted"))) {
        return None;
    }
    let last = turn.iter().rev().find(|e| !matches!(e.kind, ChatKind::Notice | ChatKind::Other(_)) && !blank_answer(e))?;
    (last.kind == ChatKind::AssistantMsg).then(|| (last, asked.map(|i| &events[i])))
}

/// O id do evento é estável nos dois parsers; sem ele, o texto com a pergunta que ele responde.
fn answer_key(answer: &ChatEvent, asked: Option<&ChatEvent>) -> String {
    if !answer.id.is_empty() { return answer.id.clone(); }
    let asked = asked.and_then(|e| e.text.as_deref()).unwrap_or_default();
    sha1_smol::Sha1::from(format!("{asked}\n{}", answer.text.as_deref().unwrap_or_default())).digest().to_string()
}

/// Turno sem resposta: a pergunta que ele responde, com prefixo próprio para não colidir com uma resposta.
fn unanswered_key(events: &[ChatEvent]) -> String {
    let asked = events.iter().rfind(|e| e.kind == ChatKind::UserMsg);
    match asked {
        Some(e) if !e.id.is_empty() => format!("sem-resposta:{}", e.id),
        _ => format!("sem-resposta:{}", sha1_smol::Sha1::from(asked.and_then(|e| e.text.as_deref()).unwrap_or_default()).digest()),
    }
}

/// O pai pediu e espera: resposta descartada vira um aviso curto, não silêncio.
fn not_returned(child: &str, why: &str) -> String {
    format!("[de: {child}] (a resposta não voltou sozinha: {why}; abra a conversa de {child})")
}

fn message(child: &str, answer: &str) -> String {
    if answer.chars().count() <= MAX_CHARS { return format!("[de: {child}] {answer}"); }
    let cut: String = answer.chars().take(MAX_CHARS).collect();
    format!("[de: {child}] {cut}\n(cortado; resposta completa na conversa de {child})")
}

/// Pai ocupado, reabrindo ou runtime doente passam; o resto (sessão sumida, pedido recusado) não.
fn transient(status: StatusCode) -> bool {
    status == StatusCode::CONFLICT || status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// A linha segue parada na mesma resposta que o histórico leu.
fn still_same(rows: Option<&[SessionRow]>, child: &str, reply_at: f64) -> bool {
    rows.is_some_and(|rows| rows.iter().any(|r| r.name == child && r.state == "idle" && r.last_reply_at == Some(reply_at)))
}

fn unreadable(st: &AppState, e: &io::Error) { report(st, "", "link_returns_unreadable", "link-returns.json ilegível; retorno parado", &io_kind(e)); }

fn io_kind(e: &io::Error) -> String { format!("{:?}", e.kind()) }

/// Falha que se repete a cada tique (arquivo ilegível, pai fora) sai uma vez por minuto. `detail` é o tipo
/// do erro (E/S, status HTTP), nunca texto da conversa; vai ao código como `codigo:detalhe`.
fn report(st: &AppState, child: &str, code: &str, reason: &'static str, detail: &str) {
    let code = if detail.is_empty() { code.to_owned() } else { format!("{code}:{detail}") };
    if !crate::warn_limit::allow(Some(child), &code) { return; }
    tracing::warn!(code, session = %child, reason, "groups: retorno da filha ligada falhou");
    st.diag.report(EVENT, child, &code, reason);
}

fn read_history(provider: Provider, jsonl: &Path) -> io::Result<Option<Turn>> {
    let file_len = std::fs::metadata(jsonl).map_or(u64::MAX, |m| m.len());
    read_turn(|limit| merged_history_capped(&HistoryRequest {
        provider, jsonl: jsonl.to_path_buf(), queue: None, limit: Some(limit), tail_window: TAIL_WINDOW,
    }, MAX_READ), file_len)
}

/// Lê o turno, decide e entrega; `returned_at` só depois da entrega: perder um retorno é pior que
/// repetir um após queda no meio.
async fn give_back(st: &AppState, row: &SessionRow, check: &Check<'_>, reply_at: f64, rate: &mut RateLimit,
    current: &Rows<'_>) -> Outcome {
    let child = row.name.as_str();
    let (Some(provider), Some(jsonl)) = (Provider::parse(&row.provider), row.jsonl.as_deref().filter(|j| !j.is_empty())) else {
        report(st, child, "link_return_no_transcript", "filha parada sem transcript", "");
        return Outcome::Done;
    };
    let jsonl = PathBuf::from(jsonl);
    let turn = match blocking(move || read_history(provider, &jsonl)).await {
        Ok(Some(turn)) => turn,
        Ok(None) => {
            report(st, child, "link_return_turn_unknown", "começo do turno da filha fora do teto de leitura; resposta não volta", "");
            return Outcome::Done;
        }
        Err(e) => { report(st, child, "link_return_history_failed", "histórico da filha ilegível", &io_kind(&e)); return Outcome::Retry }
    };
    let found = decide(check, &turn);
    match &found {
        Err(Refusal::TooOld) => {
            report(st, child, "link_return_too_old", "resposta da filha com mais de 2 h; não volta", "");
            return Outcome::Done;
        }
        Err(Refusal::NotAsked | Refusal::AlreadyReturned) => return Outcome::Done,
        Ok(_) | Err(Refusal::NoAnswer(_)) => {}
    }
    if !still_same(current().as_deref().map(Vec::as_slice), child, reply_at) { return Outcome::Moved; }
    let Some(Reply { text, key, dropped }) = reply(child, found, || rate.allow(child, Instant::now())) else { return Outcome::Done };
    if let Some((code, reason)) = dropped { report(st, child, code, reason, ""); }
    if dropped == Some(RATE_LIMITED) && !rate.notice_once(child, Instant::now()) { return Outcome::Done; }
    let (status, answer) = crate::session_write::input::deliver_reply(st, check.parent, &text).await;
    if !status.is_success() {
        let code = answer["detail"]["code"].as_str().unwrap_or("erro_envio_falhou_desconhecida");
        report(st, child, code, "entrega da resposta da filha ao pai falhou", &status.as_u16().to_string());
        return if transient(status) { Outcome::Retry } else { Outcome::Done };
    }
    if dropped.is_none() { rate.record(child, Instant::now()); }
    let (dir, name) = (st.hangar_dir.clone(), child.to_owned());
    let written = blocking(move || link_state::update(&dir, |all| {
        if let Some(entry) = all.get_mut(&name) { entry.returned_at = Some(reply_at); entry.returned_key = Some(key); }
    })).await;
    if let Err(e) = written {
        report(st, child, "link_returns_write_failed", "returned_at da filha não gravado", &io_kind(&e));
    }
    Outcome::Done
}

/// O que segue ao pai; `dropped` = a resposta não foi inteira, com o código e o motivo do diário.
#[derive(Debug, PartialEq)]
struct Reply { text: String, key: String, dropped: Option<(&'static str, &'static str)> }

const RATE_LIMITED: (&str, &str) = ("link_return_rate_limited", "retornos demais da filha em 10 min; parece laço com o pai");
const NO_ANSWER: (&str, &str) = ("link_return_no_answer", "turno da filha sem resposta final em texto (interrompido ou parado em ferramenta)");

/// Resposta pedida pelo pai e descartada vira aviso, com a mesma identidade: avisa uma vez e não relê.
/// `allow` só é consultado com resposta inteira (conta no limite quando deixa passar).
fn reply(child: &str, found: Result<(&str, String), Refusal>, allow: impl FnOnce() -> bool) -> Option<Reply> {
    match found {
        Ok((answer, key)) if allow() => Some(Reply { text: message(child, answer), key, dropped: None }),
        Ok((_, key)) => Some(Reply {
            text: not_returned(child, "respostas demais em 10 minutos, parece um laço entre vocês"), key, dropped: Some(RATE_LIMITED),
        }),
        Err(Refusal::NoAnswer(key)) => Some(Reply {
            text: not_returned(child, "o turno terminou sem resposta em texto (interrompido ou parado em ferramenta)"), key, dropped: Some(NO_ANSWER),
        }),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 10_000.0;

    fn check() -> Check<'static> {
        Check {
            last_reply_at: Some(NOW - 60.0),
            link: Some(LinkReturn { linked_at: NOW - 600.0, ..LinkReturn::default() }), now: NOW, parent: "pai", server: "casa",
        }
    }

    fn ev(kind: ChatKind, text: &str) -> ChatEvent { ChatEvent { kind, text: Some(text.to_owned()), ..Default::default() } }

    fn asked(text: &str) -> Vec<ChatEvent> {
        vec![ev(ChatKind::UserMsg, "tarefa inicial"), ev(ChatKind::AssistantMsg, "ok"), ev(ChatKind::UserMsg, text), ev(ChatKind::AssistantMsg, "feito")]
    }

    fn turn(events: Vec<ChatEvent>, whole: bool) -> Turn { Turn { events, whole } }

    fn row(provider: &str, state: &str, reply: f64) -> SessionRow {
        serde_json::from_value(serde_json::json!({"name": "f", "provider": provider, "state": state, "last_reply_at": reply})).unwrap()
    }

    fn passes(c: &Check, events: Vec<ChatEvent>) -> bool { settled(c) && decide(c, &turn(events, true)).is_ok() }

    #[test]
    fn all_criteria_pass() {
        assert_eq!(decide(&check(), &turn(asked("[de: pai] faz isso"), false)).map(|(a, _)| a), Ok("feito"));
    }

    #[test]
    fn needs_a_reply() {
        assert!(!settled(&Check { last_reply_at: None, ..check() }));
        assert!(settled(&check()));
    }

    #[test]
    fn only_claude_and_codex() {
        for p in ["claude", "claude-headless", "codex"] { assert!(returnable(p), "{p}"); }
        for p in ["pi", "omp", "kimi"] { assert!(!returnable(p), "{p}"); }
    }

    #[test]
    fn non_returnable_linked_children_do_not_hold_the_subscription() {
        let linked = |p: &str| vec![("f".to_owned(), p.to_owned())];
        assert!(!any_returnable(&linked("pi"), None));
        assert!(any_returnable(&linked("codex"), None));
        assert!(any_returnable(&linked(""), None), "provedor desconhecido segura, para não perder retorno");
        let row = row("kimi", "idle", 1.0);
        assert!(!any_returnable(&linked(""), Some(std::slice::from_ref(&row))), "a linha vence o arquivo de grupo");
    }

    #[test]
    fn reply_newer_than_link_and_last_return_and_under_two_hours() {
        let at = NOW - 60.0;
        let link = |linked_at, returned_at| Some(LinkReturn { linked_at, returned_at, returned_key: None });
        assert!(!settled(&Check { link: None, ..check() }));
        assert!(!settled(&Check { link: link(at + 1.0, None), ..check() }));
        assert!(!settled(&Check { link: link(0.0, Some(at)), ..check() }));
        assert!(settled(&Check { link: link(0.0, Some(at - 1.0)), ..check() }));
        assert!(!settled(&Check { last_reply_at: Some(NOW - MAX_AGE - 1.0), ..check() }));
        // A idade vale pela hora da resposta no evento, não pelo mtime da linha.
        let mut old = asked("[de: pai] x");
        old[3].ts = Some(NOW - MAX_AGE - 1.0);
        assert_eq!(decide(&check(), &turn(old, false)), Err(Refusal::TooOld));
    }

    #[test]
    fn the_same_answer_never_returns_twice() {
        let mut events = asked("[de: pai] x");
        events[3].id = "uuid-resposta".into();
        let (_, key) = decide(&check(), &turn(events.clone(), false)).unwrap();
        assert_eq!(key, "uuid-resposta");
        let returned = Check { link: Some(LinkReturn { linked_at: 0.0, returned_at: Some(1.0), returned_key: Some(key) }), ..check() };
        // `last_reply_at` andou (mtime) mas a resposta é a mesma.
        assert_eq!(decide(&Check { last_reply_at: Some(NOW - 5.0), ..returned }, &turn(events, false)), Err(Refusal::AlreadyReturned));
        // Sem id: o texto com a pergunta.
        let a = answer_key(&ev(ChatKind::AssistantMsg, "feito"), Some(&ev(ChatKind::UserMsg, "[de: pai] x")));
        assert_eq!(a, answer_key(&ev(ChatKind::AssistantMsg, "feito"), Some(&ev(ChatKind::UserMsg, "[de: pai] x"))));
        assert_ne!(a, answer_key(&ev(ChatKind::AssistantMsg, "feito"), Some(&ev(ChatKind::UserMsg, "[de: pai] y"))));
    }

    #[test]
    fn last_user_message_from_parent_or_first_of_conversation() {
        let c = check();
        assert!(passes(&c, asked("  [de: casa::pai] x")));
        assert!(!passes(&c, asked("feito à mão")));
        assert!(!passes(&c, asked("[de: outra] x")));
        assert!(!passes(&c, asked("[de: paizão] x")));
        let first = vec![ev(ChatKind::UserMsg, "prompt inicial"), ev(ChatKind::AssistantMsg, "pronto")];
        assert!(decide(&c, &turn(first.clone(), true)).is_ok());
        // Sem ler desde o começo, a primeira lida pode não ser a primeira da conversa.
        assert_eq!(decide(&c, &turn(first, false)), Err(Refusal::NotAsked));
        assert_eq!(decide(&c, &turn(vec![ev(ChatKind::AssistantMsg, "só resposta")], true)), Err(Refusal::NotAsked));
    }

    #[test]
    fn the_window_grows_until_the_last_user_message() {
        // Turno longo: 3000 eventos de ferramenta depois da pergunta do pai.
        let mut all = asked("[de: pai] x");
        all.truncate(3);
        all.extend(std::iter::repeat_n(ev(ChatKind::ToolResult, ""), 3000));
        all.push(ev(ChatKind::AssistantMsg, "feito"));
        let mut asked_for = Vec::new();
        let read = |limit: usize| { asked_for.push(limit); Ok(all[all.len().saturating_sub(limit)..].to_vec()) };
        let found = read_turn(read, 1024).unwrap().unwrap();
        assert_eq!(asked_for, WINDOWS);
        assert!(found.whole);
        assert_eq!(decide(&check(), &found).map(|(a, _)| a), Ok("feito"));
        // Arquivo maior que o teto e nenhuma pergunta na janela: turno desconhecido, nunca "não pedido" calado.
        let tools = vec![ev(ChatKind::ToolResult, ""); 5000];
        let capped = |limit: usize| Ok(tools[..limit.min(4000)].to_vec());
        assert!(read_turn(capped, MAX_READ + 1).unwrap().is_none());
        // Conversa curta lida inteira para na primeira janela.
        let mut calls = 0;
        let short = |_: usize| { calls += 1; Ok(asked("[de: pai] x")) };
        assert!(read_turn(short, 10).unwrap().unwrap().whole);
        assert_eq!(calls, 1);
    }

    #[test]
    fn interrupted_or_tool_ending_turns_have_no_final_answer() {
        let mut tool = asked("[de: pai] x");
        tool.push(ev(ChatKind::ToolUse, ""));
        let no_answer = |events| matches!(decide(&check(), &turn(events, false)), Err(Refusal::NoAnswer(_)));
        assert!(no_answer(tool));
        for notice in ["interrupted", "turn_aborted"] {
            let mut cut = asked("[de: pai] x");
            cut.push(ev(ChatKind::Notice, notice));
            assert!(no_answer(cut), "{notice}");
        }
        let mut tail = asked("[de: pai] x");
        tail.push(ev(ChatKind::AssistantMsg, "  "));
        tail.push(ev(ChatKind::Notice, "compacted"));
        assert_eq!(final_answer(&tail).map(|(a, _)| a.text.as_deref()), Some(Some("feito")));
        assert!(final_answer(&asked("[de: pai] x")[..3]).is_none());
    }

    #[test]
    fn a_dropped_answer_the_parent_asked_for_becomes_a_notice_once() {
        // Turno pedido pelo pai e parado em ferramenta: aviso com a identidade do turno.
        let mut tool = asked("[de: pai] x");
        tool.push(ev(ChatKind::ToolUse, ""));
        let Err(Refusal::NoAnswer(key)) = decide(&check(), &turn(tool.clone(), false)) else { panic!("esperava NoAnswer") };
        let notice = reply("f", Err(Refusal::NoAnswer(key.clone())), || panic!("sem resposta não conta no limite")).unwrap();
        assert_eq!(notice.key, key);
        assert_eq!(notice.dropped, Some(NO_ANSWER));
        assert!(notice.text.starts_with("[de: f] (a resposta não voltou sozinha: "), "{}", notice.text);
        assert!(notice.text.ends_with("; abra a conversa de f)"), "{}", notice.text);
        // Avisado (a chave gravada como `returned_key`): não avisa de novo.
        let notified = Check { link: Some(LinkReturn { linked_at: 0.0, returned_at: Some(1.0), returned_key: Some(key) }), ..check() };
        assert_eq!(decide(&notified, &turn(tool.clone(), false)), Err(Refusal::AlreadyReturned));
        // A identidade do aviso não colide com a de uma resposta: a resposta que vier depois volta.
        let mut answered = tool;
        answered.push(ev(ChatKind::AssistantMsg, "feito agora"));
        assert_eq!(decide(&notified, &turn(answered, false)).map(|(a, _)| a), Ok("feito agora"));

        // Limite estourado: a resposta não vai, o aviso vai com a identidade dela.
        let limited = reply("f", Ok(("texto", "k1".to_owned())), || false).unwrap();
        assert_eq!((limited.key.as_str(), limited.dropped), ("k1", Some(RATE_LIMITED)));
        assert!(!limited.text.contains("texto"));
        let ok = reply("f", Ok(("texto", "k1".to_owned())), || true).unwrap();
        assert_eq!(ok, Reply { text: "[de: f] texto".into(), key: "k1".into(), dropped: None });
        // Velha demais, não pedida ou já devolvida: nada vai ao pai.
        for refusal in [Refusal::TooOld, Refusal::NotAsked, Refusal::AlreadyReturned] {
            assert!(reply("f", Err(refusal), || true).is_none());
        }
    }

    #[test]
    fn unreadable_group_files_retry_on_their_own() {
        assert_eq!(idle(None), Idle::Retry(RESCAN));
        assert_eq!(idle(Some(&[("f".to_owned(), "pi".to_owned())])), Idle::UntilWake);
        assert_eq!(idle(Some(&[])), Idle::UntilWake);
        assert_eq!(idle(Some(&[("f".to_owned(), "codex".to_owned())])), Idle::Watch);
    }

    #[test]
    fn panic_pause_doubles_up_to_a_minute() {
        let mut pause = PANIC_PAUSE;
        let seen: Vec<u64> = (0..7).map(|_| { let s = pause.as_secs(); pause = next_pause(pause); s }).collect();
        assert_eq!(seen, [2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn at_most_three_returns_per_child_in_ten_minutes() {
        let mut rate = RateLimit::default();
        let t0 = Instant::now();
        assert!(rate.allow("f", t0) && rate.allow("f", t0), "consultar sem entregar não gasta vaga");
        for i in 0..RATE_MAX {
            let at = t0 + Duration::from_secs(30 * i as u64);
            assert!(rate.allow("f", at), "{i}");
            rate.record("f", at);
        }
        assert!(!rate.allow("f", t0 + Duration::from_secs(120)));
        assert!(rate.allow("outra", t0 + Duration::from_secs(120)), "o limite é por filha");
        assert!(rate.allow("f", t0 + RATE_WINDOW + Duration::from_secs(1)));
        rate.forget_old(t0 + RATE_WINDOW * 3);
        assert!(rate.0.is_empty());
    }

    #[test]
    fn the_rate_limit_notice_goes_once_per_window() {
        let mut rate = RateLimit::default();
        let t0 = Instant::now();
        assert!(rate.notice_once("f", t0));
        assert!(!rate.notice_once("f", t0 + Duration::from_secs(60)), "segundo aviso no laço seguraria o pai girando");
        assert!(rate.notice_once("outra", t0), "por filha");
        assert!(rate.notice_once("f", t0 + RATE_WINDOW));
        rate.forget_old(t0 + RATE_WINDOW * 3);
        assert!(rate.1.is_empty());
    }

    #[test]
    fn transient_failures_retry_after_backoff_and_permanent_ones_stay() {
        let mut w = Watcher::default();
        let t0 = Instant::now();
        w.handled.insert("f".into(), 1.0);
        assert!(!w.due("f", 1.0, t0));
        w.finish("f", Outcome::Retry, t0);
        assert!(!w.due("f", 1.0, t0 + Duration::from_secs(1)), "espera o recuo");
        assert!(w.due("f", 1.0, t0 + BACKOFF));
        w.handled.insert("f".into(), 1.0);
        w.finish("f", Outcome::Done, t0 + BACKOFF);
        assert!(!w.due("f", 1.0, t0 + BACKOFF * 2));
        assert!(w.due("f", 2.0, t0 + BACKOFF * 2), "resposta nova é decidida de novo");
        w.finish("f", Outcome::Moved, t0);
        assert!(w.due("f", 1.0, t0));
        for (status, retry) in [(StatusCode::CONFLICT, true), (StatusCode::SERVICE_UNAVAILABLE, true), (StatusCode::NOT_FOUND, false), (StatusCode::BAD_REQUEST, false)] {
            assert_eq!(transient(status), retry, "{status}");
        }
    }

    #[test]
    fn delivery_waits_for_the_same_idle_reply() {
        let idle = row("claude", "idle", 5.0);
        assert!(still_same(Some(std::slice::from_ref(&idle)), "f", 5.0));
        assert!(!still_same(Some(std::slice::from_ref(&idle)), "f", 6.0));
        let working = row("claude", "working", 5.0);
        assert!(!still_same(Some(std::slice::from_ref(&working)), "f", 5.0));
        assert!(!still_same(None, "f", 5.0));
    }

    #[test]
    fn answer_is_cut() {
        assert_eq!(message("f", "oi"), "[de: f] oi");
        let long = "é".repeat(MAX_CHARS + 5);
        let cut = message("f", &long);
        assert!(cut.starts_with(&format!("[de: f] {}", "é".repeat(MAX_CHARS))));
        assert!(cut.ends_with("(cortado; resposta completa na conversa de f)"));
    }
}
