//! A resposta final da filha ligada volta sozinha ao pai como `[de: <filha>] …`, e a pergunta que ela espera
//! chega a ele uma vez (só a `Question` o convida a responder). Só roda com filha
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
use crate::interactions::InteractionKind;
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
/// Tamanho do recado ao pai e da página do `child_status`, em `char`.
pub(crate) const MAX_CHARS: usize = 8000;
/// Sem filha nas linhas, o arquivo de grupo confirma antes de soltar: a lista pode ser de antes da ligação.
const RESCAN: Duration = Duration::from_secs(30);
/// Pai e filha trocando turnos sozinhos: o limite é por filha e longo, para não virar laço.
const RATE_MAX: usize = 3;
const RATE_WINDOW: Duration = Duration::from_secs(600);
/// Falha passageira (histórico ilegível, pai ocupado ou reabrindo) tenta de novo depois disso.
const BACKOFF: Duration = Duration::from_secs(30);
const EVENT: &str = "rust.link_return_failed";
/// Texto da pergunta da filha no recado ao pai, em `char`.
const ASK_CHARS: usize = 300;
/// Resposta dada na filha, no recado ao pai, em `char`.
const ANSWER_CHARS: usize = 1000;
/// Eventos do fim da conversa da filha onde a resposta da pergunta fechada é procurada.
const ANSWER_WINDOW: usize = 200;
/// A resposta chega ao transcript um pouco depois de a pergunta sumir da linha: espera isto antes de dizer que não leu.
const ANSWER_GRACE: Duration = Duration::from_secs(10);
/// Ferramentas de pergunta ao dono: o resultado da chamada é a resposta dada.
const ASK_TOOLS: [&str; 2] = ["AskUserQuestion", "request_user_input"];
/// Marca, no `metadata.source` do AskUserQuestion do pai, da pergunta que repassa a de uma filha.
const RELAY_SOURCE: &str = "hangar-child:";
/// Chave de aviso de menu e permissão; a de pergunta é o `interaction_id`.
const NOTICE_KEY: &str = "notice:";
/// Tipo da interação ainda ilegível (vínculo da conversa por ligar): espera isto além do `SETTLE` antes de
/// avisar como pedido do dono, que não se refaz como pergunta depois.
const KIND_GRACE: Duration = Duration::from_secs(60);

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
            let children: Vec<&SessionRow> = rows.iter().filter(|r| r.linked_by.is_some() && returnable(&r.provider)).collect();
            if !children.is_empty() {
                watcher.round(&st, &groups.server_id(), &children, &|| sub.rows()).await;
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
pub(crate) fn returnable(provider: &str) -> bool {
    matches!(Provider::parse(provider), Some(Provider::Claude | Provider::ClaudeHeadless | Provider::Codex | Provider::Antigravity))
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
    /// Desde quando a filha espera nesta interação (`interaction_id`).
    asking: HashMap<String, (Instant, String)>,
    /// Avisos de pergunta ao pai: limite próprio, fora do das respostas.
    question_rate: RateLimit,
    question_retry_at: HashMap<String, Instant>,
    /// Tirar da fila do pai o aviso que perdeu o sentido falhou passageiro: tenta de novo depois disso.
    cancel_retry_at: HashMap<String, Instant>,
    /// Filhas com aviso ainda na fila do pai, pela última leitura: a rodada relê o arquivo por elas.
    open_asks: HashSet<String>,
    /// O arquivo já foi lido uma vez nesta vigia: aviso pendente de antes do reinício entra em `open_asks`.
    asks_seeded: bool,
    /// Última interação avisada (ou já avisada antes do reinício) por filha, nesta vigia: dois menus com o
    /// mesmo texto são avisos distintos, e falha ao gravar o registro não reavisa.
    notified: HashMap<String, String>,
    /// Linha do aviso na fila do pai que o registro não gravou: o cancelamento ainda a acha.
    unrecorded: HashMap<String, String>,
    /// Desde quando a filha está sem interação com a chave de um aviso ainda gravada.
    closed: HashMap<String, Instant>,
    /// Pergunta respondida na filha já contada ao pai: a gravação que falha não conta de novo.
    told: HashMap<String, String>,
    answered_retry_at: HashMap<String, Instant>,
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

    async fn round(&mut self, st: &Arc<AppState>, server: &str, children: &[&SessionRow], current: &Rows<'_>) {
        let now = Instant::now();
        let present = |n: &String| children.iter().any(|r| &r.name == n);
        self.idle.retain(|n, _| present(n));
        self.handled.retain(|n, _| present(n));
        self.retry_at.retain(|n, _| present(n));
        self.known.retain(|n| present(n));
        self.asking.retain(|n, _| present(n));
        self.question_retry_at.retain(|n, _| present(n));
        self.cancel_retry_at.retain(|n, _| present(n));
        self.open_asks.retain(|n| present(n));
        self.notified.retain(|n, _| present(n));
        self.unrecorded.retain(|n, _| present(n));
        self.closed.retain(|n, _| present(n));
        self.told.retain(|n, _| present(n));
        self.answered_retry_at.retain(|n, _| present(n));
        self.rate.forget_old(now);
        self.question_rate.forget_old(now);
        // Já na primeira vista: o registro criado aqui só deixa voltar resposta posterior a ele.
        let unseen: Vec<String> = children.iter().map(|r| r.name.clone()).filter(|n| !self.known.contains(n)).collect();
        if !unseen.is_empty() {
            let (dir, names, at) = (st.hangar_dir.clone(), unseen.clone(), wall_now());
            match blocking(move || link_state::ensure_linked(&dir, &names, at)).await {
                Ok(created) => {
                    for child in created {
                        report(st, &child, "link_returns_entry_missing", "filha ligada sem registro de ligação; só as respostas de agora em diante voltam", "");
                    }
                    self.known.extend(unseen);
                }
                Err(e) => return unreadable(st, &e),
            }
        }
        let mut links: Option<LinkReturns> = None;
        // Interação já avisada nesta vigia não relê o arquivo; só a que fechou depois do aviso.
        let asks = !self.asks_seeded || children.iter().any(|r| match open_interaction(r) {
            Some(id) => self.notified.get(&r.name) != Some(&id),
            None => self.open_asks.contains(&r.name) || self.notified.contains_key(&r.name) || self.closed.contains_key(&r.name),
        });
        if asks {
            let dir = st.hangar_dir.clone();
            let read = match blocking(move || link_state::read(&dir)).await {
                Ok(read) => read,
                Err(e) => return unreadable(st, &e),
            };
            self.asks_seeded = true;
            self.open_asks = children.iter().filter(|r| read.get(&r.name).is_some_and(|l| l.asked_queue_id.is_some()) || self.unrecorded.contains_key(&r.name))
                .map(|r| r.name.clone()).collect();
            for row in children {
                let Some(link) = read.get(&row.name) else { continue };
                let mut link = link.clone();
                if link.asked_queue_id.is_none() { link.asked_queue_id = self.unrecorded.get(&row.name).cloned(); }
                self.ask(st, row, &link, now, current).await;
            }
            links = Some(read);
        }
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
            let mut check = Check {
                last_reply_at: row.last_reply_at, link: links.as_ref().and_then(|l| l.get(&row.name)).cloned(), now: wall_now(),
                parent: row.linked_by.as_deref().unwrap_or_default(), server, continues: false,
            };
            self.handled.insert(row.name.clone(), reply_at);
            if !settled(&check) { continue; }
            check.continues = continues_alone(st, row).await;
            let outcome = give_back(st, row, &check, reply_at, &mut self.rate, current).await;
            self.finish(&row.name, outcome, now);
        }
    }
}

/// O que fazer nesta rodada com a interação aberta da filha.
#[derive(Debug, PartialEq)]
enum QuestionStep {
    /// Avisar o pai com este texto.
    Send(String),
    /// A interação avisada sumiu: tirar da fila do pai a linha do aviso, se ainda espera.
    Cancel(String),
    /// Aberta há menos de `SETTLE`.
    Wait,
    Skip,
    /// Avisos demais desta filha na janela de `RATE_WINDOW`.
    Limited,
    /// A interação avisada fechou sem linha na fila: a chave gravada sai, ou o próximo pedido de mesmo texto,
    /// depois de um reinício, seria dado como já avisado.
    Forget,
    /// A pergunta avisada com este `interaction_id` fechou sem o `child_answer` do pai: o dono respondeu na filha.
    Answered(String),
}

impl Watcher {
    /// Decisão pura sobre a linha e o registro da filha; só mexe no relógio da espera e consulta o limite.
    fn question_step(&mut self, row: &SessionRow, link: &LinkReturn, kind: Option<&InteractionKind>, now: Instant) -> QuestionStep {
        let child = &row.name;
        let Some(id) = open_interaction(row) else {
            self.asking.remove(child);
            if let Some(queued) = &link.asked_queue_id { return QuestionStep::Cancel(queued.clone()); }
            if let Some(key) = &link.asked_key {
                // Fechada depois de avisada nesta vigia, ou fechada há `SETTLE`: logo depois do reinício o pedido ainda
                // aberto pode voltar com outro ID, e a chave é o que segura o aviso repetido.
                let since = *self.closed.entry(child.clone()).or_insert(now);
                if !self.notified.contains_key(child) && now - since < SETTLE { return QuestionStep::Wait; }
                return if answered_in_child(key, link) { QuestionStep::Answered(key.clone()) } else { QuestionStep::Forget };
            }
            // Nada gravado a limpar: a vigia esquece a interação fechada e para de reler o arquivo por ela.
            self.closed.remove(child);
            self.notified.remove(child);
            return QuestionStep::Skip;
        };
        self.closed.remove(child);
        let since = match self.asking.get(child) {
            Some((since, seen)) if *seen == id => *since,
            _ => { self.asking.insert(child.clone(), (now, id.clone())); now }
        };
        let text = question_message(child, &id, row, kind);
        let asked = match self.notified.get(child) {
            Some(notified) => *notified == id,
            // Vigia nova (reinício): o ID antigo morreu, vale a chave gravada, que o reinício não muda.
            None => link.asked_key.as_deref() == Some(ask_key(&id, kind, &text).as_str()),
        };
        if asked {
            self.notified.insert(child.clone(), id);
            return QuestionStep::Skip;
        }
        if let Some(queued) = &link.asked_queue_id { return QuestionStep::Cancel(queued.clone()); }
        if now - since < SETTLE { return QuestionStep::Wait; }
        if kind.is_none() && now - since < SETTLE + KIND_GRACE { return QuestionStep::Wait; }
        if !self.question_rate.allow(child, now) { return QuestionStep::Limited; }
        QuestionStep::Send(text)
    }

    /// Avisa o pai da interação aberta da filha, ou tira da fila dele o aviso que perdeu o sentido.
    async fn ask(&mut self, st: &Arc<AppState>, row: &SessionRow, link: &LinkReturn, now: Instant, current: &Rows<'_>) {
        let child = row.name.as_str();
        let parent = row.linked_by.as_deref().unwrap_or_default();
        let id = open_interaction(row);
        // Na espera da nova tentativa só o cancelamento da linha que perdeu o sentido segue.
        // Pai de outra máquina não recebe aviso de pergunta: o `child_answer` só alcança filha desta máquina.
        if parent.is_empty() || is_remote(parent) || (id.is_some() && self.question_retry_at.get(child).is_some_and(|t| now < *t)) { return; }
        let kind = match id.as_deref() {
            Some(id) if self.notified.get(child).map(String::as_str) != Some(id) => interaction_kind(st, child, id).await,
            _ => None,
        };
        match self.question_step(row, link, kind.as_ref(), now) {
            QuestionStep::Wait | QuestionStep::Skip => {}
            QuestionStep::Limited => {
                report(st, child, "link_question_rate_limited", "perguntas demais da filha em 10 min; não avisa o pai", "");
                self.question_retry_at.insert(child.to_owned(), now + BACKOFF);
            }
            QuestionStep::Forget => {
                // Gravação que falha mantém `notified`: o próximo pedido de mesmo texto é outro aviso, e tenta de novo.
                if record_ask(st, child, |entry| if entry.asked_queue_id.is_none() { entry.asked_key = None; entry.parent_answered = None; }).await {
                    self.notified.remove(child);
                    self.closed.remove(child);
                }
            }
            QuestionStep::Answered(id) => {
                if self.answered_retry_at.get(child).is_some_and(|t| now < *t) { return; }
                if self.told.get(child) != Some(&id) {
                    let answer = match read_child_answer(row).await {
                        Ok(answer) => answer,
                        Err(e) => { report(st, child, "link_question_answer_unreadable", "resposta dada na filha ilegível", &io_kind(&e)); None }
                    };
                    let since = self.closed.get(child).copied().unwrap_or(now);
                    if answer.is_none() && now - since < ANSWER_GRACE { return; }
                    if tell_parent(st, parent, child, &id, answer.as_deref(), current).await == Told::Retry {
                        self.answered_retry_at.insert(child.to_owned(), now + BACKOFF);
                        return;
                    }
                    self.told.insert(child.to_owned(), id);
                }
                self.answered_retry_at.remove(child);
                if record_ask(st, child, |entry| if entry.asked_queue_id.is_none() { entry.asked_key = None; entry.parent_answered = None; }).await {
                    self.notified.remove(child);
                    self.closed.remove(child);
                    self.told.remove(child);
                }
            }
            QuestionStep::Cancel(queued) => {
                if self.cancel_retry_at.get(child).is_some_and(|t| now < *t) { return; }
                let cancelled = crate::session_write::control::cancel_waiting(st, parent, &queued).await;
                let settled = cancel_settled(&cancelled);
                // Com o pai ocupado só a primeira falha da série vai ao diário; as novas tentativas não.
                if let Err(code) = &cancelled && (settled || !self.cancel_retry_at.contains_key(child)) {
                    report(st, child, "link_question_cancel_failed", "pergunta já resolvida segue na fila do pai", code);
                }
                // Pai ocupado ou acordando: a linha segue registrada e a tentativa volta depois.
                if !settled {
                    self.cancel_retry_at.insert(child.to_owned(), now + BACKOFF);
                    return;
                }
                self.cancel_retry_at.remove(child);
                self.open_asks.remove(child);
                self.unrecorded.remove(child);
                // A interação fechada leva a chave junto; trocada por outra, a chave fica até a nova ser avisada. Aviso
                // que já tinha saído da fila o pai leu: a chave fica, e a pergunta fechada vai ao `Answered`.
                let forget = id.is_none() && cancelled == Ok(true);
                let cleared = record_ask(st, child, move |entry| if entry.asked_queue_id.as_deref() == Some(queued.as_str()) {
                    entry.asked_queue_id = None;
                    if forget { entry.asked_key = None; entry.parent_answered = None; }
                }).await;
                if forget && cleared { self.notified.remove(child); }
            }
            QuestionStep::Send(text) => {
                let Some(id) = id else { return };
                let key = ask_key(&id, kind.as_ref(), &text);
                let ((status, answer), queued) = crate::session_write::input::deliver_reply_entry(st, parent, &text).await;
                if !status.is_success() {
                    let code = answer["detail"]["code"].as_str().unwrap_or("erro_envio_falhou_desconhecida");
                    report(st, child, code, "pergunta da filha não chegou ao pai", &status.as_u16().to_string());
                    if transient(status) { self.question_retry_at.insert(child.to_owned(), now + BACKOFF); return; }
                    // Recusa de vez (pai sumiu, pedido recusado): esta pergunta não insiste.
                    self.notified.insert(child.to_owned(), id);
                    self.unrecorded.remove(child);
                    record_ask(st, child, move |entry| { entry.asked_key = Some(key); entry.asked_queue_id = None; }).await;
                    return;
                }
                self.question_rate.record(child, now);
                if queued.is_some() { self.open_asks.insert(child.to_owned()); }
                self.notified.insert(child.to_owned(), id);
                let line = queued.clone();
                // Sem o registro gravado, a linha fica na memória: a pergunta que fechar ainda a tira da fila do pai.
                if record_ask(st, child, move |entry| { entry.asked_key = Some(key); entry.asked_queue_id = queued; }).await {
                    self.unrecorded.remove(child);
                } else if let Some(line) = line {
                    self.unrecorded.insert(child.to_owned(), line);
                } else {
                    self.unrecorded.remove(child);
                }
                st.diag.record_line(forwarded_line(child));
            }
        }
    }
}

/// Chave gravada do aviso, que vale depois de reiniciar, quando o `interaction_id` nasce outro. A pergunta
/// fica pelo ID: o antigo morreu e o pai precisa do novo para o `child_answer`, então ela vai de novo uma
/// vez. Menu e permissão ficam pelo texto do aviso, que não cita o ID: não se repetem.
fn ask_key(id: &str, kind: Option<&InteractionKind>, text: &str) -> String {
    if kind == Some(&InteractionKind::Question) { return id.to_owned(); }
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    let hex: String = digest.as_ref()[..16].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{NOTICE_KEY}{hex}")
}

/// Grava a pergunta avisada no registro da filha; falha vai ao diário (`notified` segura o reaviso). `true` = gravado.
async fn record_ask(st: &AppState, child: &str, change: impl FnOnce(&mut LinkReturn) + Send + 'static) -> bool {
    let (dir, name) = (st.hangar_dir.clone(), child.to_owned());
    let written = blocking(move || link_state::update(&dir, |all| { if let Some(entry) = all.get_mut(&name) { change(entry); } })).await;
    if let Err(e) = &written { report(st, child, "link_returns_write_failed", "pergunta da filha avisada não gravada", &io_kind(e)); }
    written.is_ok()
}

fn forwarded_line(child: &str) -> serde_json::Value {
    serde_json::json!({"evento": "rust.link_question_forwarded", "nivel": "ok", "sessao": child, "codigo": "link_question_forwarded"})
}

/// A interação que a filha espera: o formulário da pergunta vence o menu, que é o mesmo pedido na tela. O
/// `child_status` publica a mesma, que é a que o aviso cita e o `/answer` confere.
pub(crate) fn open_interaction(row: &SessionRow) -> Option<String> {
    if row.state != "awaiting_input" { return None; }
    row.ask_form.as_ref().and_then(|form| form["interaction_id"].as_str()).or(row.interaction_id.as_deref())
        .filter(|id| !id.is_empty()).map(str::to_owned)
}

/// O cancelamento do aviso acabou: tirado, já fora da fila, ou recusado de vez. Falha passageira não.
fn cancel_settled(cancelled: &Result<bool, String>) -> bool {
    let Err(code) = cancelled else { return true };
    !(matches!(code.as_str(), "busy" | "asleep_waking" | "runtime_watcher" | "runtime_unhealthy" | "other_terminal_starting" | "session_transfer_busy")
        || code == crate::runtime::gateway::ASLEEP_WAKING || code == crate::runtime::codex_relaunch::STARTING)
}

/// O tipo pela identidade publicada; vínculo ou ID ilegível é `None`: espera `KIND_GRACE`, depois vale como permissão.
async fn interaction_kind(st: &AppState, child: &str, id: &str) -> Option<InteractionKind> {
    let runtime = st.state.runtime.get()?;
    let binding = runtime.interaction_binding_readonly(child).await.ok()?;
    st.interactions.expected(&binding, Some(id)).ok().map(|expected| expected.target.kind)
}

fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max { return text.to_owned(); }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

/// Perguntas abertas como (texto, opções): o formulário inteiro quando há, senão a pergunta da linha.
fn asked_items(row: &SessionRow) -> Vec<(String, Vec<String>)> {
    let form = row.ask_form.clone().and_then(|form| serde_json::from_value::<hangar_api::ask::AskQuestion>(form).ok());
    match form.filter(|form| !form.questions.is_empty()) {
        Some(form) => form.questions.into_iter().map(|q| (q.question, q.options.into_iter().map(|o| o.label).collect())).collect(),
        None => vec![(row.question.clone().unwrap_or_default(), row.options.clone().unwrap_or_default())],
    }
}

/// Só a pergunta (`Question`) convida o pai a responder; menu, plano e permissão são do dono.
fn question_message(child: &str, id: &str, row: &SessionRow, kind: Option<&InteractionKind>) -> String {
    if kind != Some(&InteractionKind::Question) {
        let text = row.question.as_deref().or(row.label.as_deref()).filter(|t| !t.trim().is_empty()).unwrap_or("sem texto");
        return format!("[de: {child}] pedido esperando o dono: {}. O dono decide no app.", cut(text, ASK_CHARS));
    }
    let items = asked_items(row);
    let mut text = format!("[de: {child}] pergunta (interaction_id {id}):");
    for (n, (question, options)) in items.iter().enumerate() {
        if items.len() > 1 { text.push_str(&format!("\n{}) {}", n + 1, cut(question, ASK_CHARS))); } else { text.push_str(&format!(" {}", cut(question, ASK_CHARS))); }
        for (i, option) in options.iter().enumerate() { text.push_str(&format!("\n{}. {}", i + 1, cut(option, ASK_CHARS))); }
    }
    text.push_str(&format!("\nResponda com child_answer (alvo {child}, interaction_id {id})."));
    text.push_str(&format!("\nSe for perguntar ao dono, use AskUserQuestion com metadata {{source: \"{RELAY_SOURCE}{id}\"}}."));
    text
}

/// A pergunta avisada (chave = `interaction_id`) fechou sem o `child_answer` do pai: quem respondeu foi o dono, na filha.
fn answered_in_child(key: &str, link: &LinkReturn) -> bool {
    !key.starts_with(NOTICE_KEY) && link.parent_answered.as_deref() != Some(key)
}

/// O pai vai responder pelo `child_answer`: gravado antes da rota, porque a pergunta pode fechar já na resposta.
pub(crate) async fn parent_answering(st: &AppState, child: &str, id: &str) {
    let id = id.to_owned();
    record_ask(st, child, move |entry| entry.parent_answered = Some(id)).await;
}

/// A resposta do pai não saiu: a marca sai, senão a resposta que o dono der na filha não chega a ele.
pub(crate) async fn parent_answer_dropped(st: &AppState, child: &str, id: &str) {
    let id = id.to_owned();
    record_ask(st, child, move |entry| if entry.parent_answered.as_deref() == Some(id.as_str()) { entry.parent_answered = None; }).await;
}

/// Saiu ou pode ter saído: só a recusa clara tira a marca do pai.
pub(crate) fn answer_maybe_sent(status: u16, value: &serde_json::Value) -> bool {
    (200..300).contains(&status) || matches!(value["detail"]["code"].as_str(), Some("erro_sem_confirmacao_resposta" | "erro_envio_falhou"))
}

/// A resposta da última pergunta ao dono na conversa: o resultado da chamada da ferramenta, cortado.
fn asked_answer(events: &[ChatEvent]) -> Option<String> {
    let call = events.iter().rev().find(|e| e.kind == ChatKind::ToolUse && e.tool_name.as_deref().is_some_and(|n| ASK_TOOLS.contains(&n)))?;
    let id = call.tool_use_id.as_deref().filter(|id| !id.is_empty())?;
    let result = events.iter().rev().find(|e| e.kind == ChatKind::ToolResult && e.tool_use_id.as_deref() == Some(id))?;
    result.result.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| cut(t, ANSWER_CHARS))
}

async fn read_child_answer(row: &SessionRow) -> io::Result<Option<String>> {
    let (Some(provider), Some(jsonl)) = (Provider::parse(&row.provider), row.jsonl.clone().filter(|j| !j.is_empty())) else { return Ok(None) };
    blocking(move || {
        let events = merged_history_capped(&HistoryRequest {
            provider, jsonl: PathBuf::from(jsonl), queue: None, limit: Some(ANSWER_WINDOW), tail_window: TAIL_WINDOW,
        }, MAX_READ)?;
        Ok(asked_answer(&events))
    }).await
}

/// A pergunta aberta do pai que repassa a da filha, pela marca no `metadata.source`: (`interaction_id`, formulário).
fn relay_question(parent: &SessionRow, source: &str) -> Option<(String, serde_json::Value)> {
    let form = parent.ask_form.as_ref().filter(|form| form["source"].as_str() == Some(source))?;
    Some((open_interaction(parent)?, form.clone()))
}

/// O mesmo texto livre em cada pergunta do formulário; o campo de texto vem depois das opções.
fn free_text_body(form: &serde_json::Value, interaction_id: &str, text: &str) -> serde_json::Value {
    let answers: Vec<serde_json::Value> = form["questions"].as_array().into_iter().flatten()
        .map(|q| serde_json::json!({"kind": "text", "value": text, "type_index": q["options"].as_array().map_or(0, Vec::len)}))
        .collect();
    serde_json::json!({"answers": answers, "interaction_id": interaction_id})
}

/// Caractere de controle que o `/answer` recusa vira espaço; parágrafo e tabulação ficam.
fn plain(text: &str) -> String {
    text.chars().map(|c| if c.is_control() && !matches!(c, '\n' | '\t') { ' ' } else { c }).collect()
}

/// Resposta livre à pergunta de repasse do pai.
fn relay_answer_text(child: &str, answer: Option<&str>) -> String {
    plain(&match answer {
        Some(answer) => format!("Respondida pelo dono na filha {child}: {}. Não repita nem mande à filha.", answer.trim_end_matches('.')),
        None => format!("Encerrada na filha {child} sem resposta lida. Não repita nem mande à filha."),
    })
}

/// Recado ao pai sem pergunta de repasse aberta (pai Codex, pai livre, ou sem a marca).
fn answered_message(child: &str, id: &str, answer: Option<&str>) -> String {
    match answer {
        Some(answer) => format!("[de: {child}] a pergunta (interaction_id {id}) foi respondida pelo dono na filha: {}. Não responda nem repita.", answer.trim_end_matches('.')),
        None => format!("[de: {child}] a pergunta (interaction_id {id}) foi encerrada na filha sem resposta lida. Não responda nem repita."),
    }
}

#[derive(Debug, PartialEq)]
enum Told { Done, Retry }

/// Fecha a pergunta de repasse do pai pelo `/answer`, ou manda o recado pela fila quando não há uma.
async fn tell_parent(st: &Arc<AppState>, parent: &str, child: &str, id: &str, answer: Option<&str>, current: &Rows<'_>) -> Told {
    let source = format!("{RELAY_SOURCE}{id}");
    let relay = current().and_then(|rows| rows.iter().find(|r| r.name == parent).and_then(|r| relay_question(r, &source)));
    if let Some((interaction_id, form)) = relay {
        let body = free_text_body(&form, &interaction_id, &relay_answer_text(child, answer));
        let (status, value) = crate::session_write::answer::answer_as_owner(st, parent, &body).await;
        if answer_maybe_sent(status.as_u16(), &value) {
            st.diag.record_line(answered_line(child));
            return Told::Done;
        }
        let code = value["detail"]["code"].as_str().unwrap_or("erro_resposta_desconhecida").to_owned();
        report(st, child, &code, "resposta dada na filha não fechou a pergunta do pai", &status.as_u16().to_string());
        // Pergunta do pai trocada ou fechada nesse meio: segue o recado. Pai ocupado tenta de novo.
        if !matches!(code.as_str(), "erro_interacao_desatualizada" | "erro_interacao_sem_identidade") && transient(status) { return Told::Retry; }
    }
    let (status, value) = crate::session_write::input::deliver_reply(st, parent, &answered_message(child, id, answer)).await;
    if status.is_success() {
        st.diag.record_line(answered_line(child));
        return Told::Done;
    }
    let code = value["detail"]["code"].as_str().unwrap_or("erro_envio_falhou_desconhecida");
    report(st, child, code, "resposta dada na filha não chegou ao pai", &status.as_u16().to_string());
    if transient(status) { Told::Retry } else { Told::Done }
}

fn answered_line(child: &str) -> serde_json::Value {
    serde_json::json!({"evento": "rust.link_question_answered_in_child", "nivel": "ok", "sessao": child, "codigo": "link_question_answered_in_child"})
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
    /// O continuar sozinho vai seguir o turno parado no erro (`continues_alone`).
    pub continues: bool,
}

/// Critério 3 pela linha (parada há `SETTLE` e provedor devolvível já filtrados no `round`): resposta
/// nova desde o último retorno, com menos de `MAX_AGE`. Sem retorno ainda, a de antes da ligação também
/// passa, e o `decide` só a aceita se for a do `initial_prompt`: a filha de outra máquina é ligada
/// depois de abrir e pode responder antes. A entrada recuperada não sabe quando a ligação foi feita: só
/// resposta posterior a ela.
fn settled(c: &Check) -> bool {
    let (Some(at), Some(link)) = (c.last_reply_at, &c.link) else { return false };
    let early = link.returned_at.is_none() && !link.recovered;
    (early || at > link.returned_at.unwrap_or(f64::MIN).max(link.linked_at)) && c.now - at < MAX_AGE
}

/// A resposta lida é de antes da ligação.
fn before_link(c: &Check) -> bool {
    matches!((c.last_reply_at, &c.link), (Some(at), Some(link)) if at <= link.linked_at)
}

/// O turno é o primeiro da conversa (o do `initial_prompt`), lido desde o começo.
fn first_turn(turn: &Turn) -> bool {
    turn.whole && turn.events.iter().filter(|e| user_request(e)).count() == 1
}

/// Eventos do fim da conversa que chegam até a última mensagem do usuário; `whole` = desde o começo.
struct Turn { events: Vec<ChatEvent>, whole: bool }

/// Mensagem do usuário escrita por alguém. O "continue" que o Hangar manda sozinho depois de uma queda não
/// é pedido de ninguém: o turno é o da mensagem antes dele. O `child_status` lê o turno por ela também.
pub(crate) fn user_request(e: &ChatEvent) -> bool {
    e.kind == ChatKind::UserMsg && !hangar_auto_continue::text::is_auto_continue(e.text.as_deref().unwrap_or_default())
}

/// Lê para trás crescendo a janela até achar a última mensagem do usuário. `None` = não achada no teto.
fn read_turn(mut read: impl FnMut(usize) -> io::Result<Vec<ChatEvent>>, file_len: u64) -> io::Result<Option<Turn>> {
    for limit in WINDOWS {
        let events = read(limit)?;
        let has_user = events.iter().any(user_request);
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
    let mut users = turn.events.iter().filter(|e| user_request(e));
    let Some(last) = users.next_back() else { return false };
    let text = last.text.as_deref().unwrap_or_default().trim_start();
    let from_parent = text.starts_with(&format!("[de: {parent}]"))
        || (!server.is_empty() && text.starts_with(&format!("[de: {server}::{parent}]")))
        // A primeira mensagem da filha, mesmo quando a conversa é longa demais para ser lida inteira.
        || text.starts_with(&hangar_delegation::guidance::child_prefix(parent));
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
    /// O turno parou numa queda passageira que o continuar sozinho vai seguir: a resposta real volta depois.
    ApiError,
}

/// Critérios 4 em diante, sobre o turno lido: devolve o texto e a identidade da resposta.
fn decide<'a>(c: &Check, turn: &'a Turn) -> Result<(&'a str, String), Refusal> {
    if !asked_by_parent(turn, c.parent, c.server) || (before_link(c) && !first_turn(turn)) { return Err(Refusal::NotAsked); }
    let returned = |key: &str| c.link.as_ref().and_then(|l| l.returned_key.as_deref()) == Some(key);
    let Some((answer, asked)) = final_answer(&turn.events) else {
        let key = unanswered_key(&turn.events);
        return Err(if returned(&key) { Refusal::AlreadyReturned } else { Refusal::NoAnswer(key) });
    };
    if c.continues && api_error(answer) { return Err(Refusal::ApiError); }
    let key = answer_key(answer, asked);
    if returned(&key) { return Err(Refusal::AlreadyReturned); }
    // `last_reply_at` cai no mtime do transcript sem hora no evento; a do evento é a da resposta.
    let at = answer.ts.filter(|&t| t != 0.0).or(c.last_reply_at).unwrap_or(0.0);
    if c.now - at >= MAX_AGE { return Err(Refusal::TooOld); }
    Ok((answer.text.as_deref().unwrap_or_default(), key))
}

/// A linha `isApiErrorMessage` do Claude chega como resposta do modelo, marcada pelo parser com
/// `notice_meta.level = "error"`; o texto `API Error:` cobre quem não leva a marca.
fn api_error(e: &ChatEvent) -> bool {
    e.kind == ChatKind::AssistantMsg
        && (e.notice_meta.as_ref().is_some_and(|meta| meta.get("level").and_then(|level| level.as_str()) == Some("error"))
            || e.text.as_deref().is_some_and(|text| text.trim_start().starts_with("API Error")))
}

/// Só a queda passageira com o interruptor ligado e o registro esperando a rede (ou já tendo mandado o
/// "continue" dela) segue sozinha. Limite, login recusado, prompt longo, teto ou Cancelar nunca continuam: o
/// erro volta ao pai, senão ele espera para sempre.
async fn continues_alone(st: &AppState, row: &SessionRow) -> bool {
    if row.auto_continue_off || !matches!(Provider::parse(&row.provider), Some(Provider::Claude | Provider::ClaudeHeadless)) { return false; }
    let (Some(auto), Some(jsonl)) = (st.auto_continue.get(), row.jsonl.clone().filter(|j| !j.is_empty())) else { return false };
    let Ok(Some(key)) = blocking(move || Ok(transient_stop(Path::new(&jsonl)))).await else { return false };
    auto.ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner).continuing(&row.name, &key, wall_now())
}

/// A chave (a do `StopHit`) da última linha da conversa, quando ela é uma queda passageira.
fn transient_stop(jsonl: &Path) -> Option<String> {
    use crate::session_write::limit_switch::{file_tail, last_main_line};
    let line = last_main_line(&file_tail(jsonl)?)?;
    if !hangar_auto_continue::classify_claude_line(&line).is_transient() { return None; }
    line["uuid"].as_str().map(str::to_owned)
}

fn blank_answer(e: &ChatEvent) -> bool { e.kind == ChatKind::AssistantMsg && e.text.as_deref().is_none_or(|t| t.trim().is_empty()) }

/// A resposta final só existe se o turno terminou nela: interrompido ou parado em ferramenta traz
/// texto pela metade. Volta junto a mensagem do usuário que ela responde (o continue sozinho não é ela).
pub(crate) fn final_answer(events: &[ChatEvent]) -> Option<(&ChatEvent, Option<&ChatEvent>)> {
    let asked = events.iter().rposition(user_request);
    let turn = &events[asked.map_or(0, |i| i + 1)..];
    // A interrupção antes de um continue sozinho ficou para trás: o turno seguiu depois dele.
    let resumed = turn.iter().rposition(|e| e.kind == ChatKind::UserMsg).map_or(0, |i| i + 1);
    if turn[resumed..].iter().any(|e| e.kind == ChatKind::Notice && matches!(e.text.as_deref(), Some("interrupted" | "turn_aborted"))) {
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

/// A identidade que o retorno grava em `returned_key` para o fim desta conversa: a da resposta final, ou a do turno
/// sem resposta. O `child_status` compara com ela para dizer se a resposta já voltou.
pub(crate) fn reply_key(events: &[ChatEvent]) -> String {
    match final_answer(events) {
        Some((answer, asked)) => answer_key(answer, asked),
        None => unanswered_key(events),
    }
}

/// Turno sem resposta: a pergunta que ele responde, com prefixo próprio para não colidir com uma resposta.
fn unanswered_key(events: &[ChatEvent]) -> String {
    let asked = events.iter().rfind(|e| user_request(e));
    match asked {
        Some(e) if !e.id.is_empty() => format!("sem-resposta:{}", e.id),
        _ => format!("sem-resposta:{}", sha1_smol::Sha1::from(asked.and_then(|e| e.text.as_deref()).unwrap_or_default()).digest()),
    }
}

/// O pai pediu e espera: resposta descartada vira um aviso curto, não silêncio.
fn not_returned(child: &str, why: &str) -> String {
    format!("[de: {child}] (a resposta não voltou sozinha: {why}; abra a conversa de {child})")
}

/// `key` vai no corte: a página seguinte pedida com ela não junta o pedaço de outra resposta.
fn message(child: &str, answer: &str, key: &str) -> String {
    if answer.chars().count() <= MAX_CHARS { return format!("[de: {child}] {answer}"); }
    let cut: String = answer.chars().take(MAX_CHARS).collect();
    format!("[de: {child}] {cut}\n(cortado; o resto pela tool child_status, offset {MAX_CHARS}, answer_key {key})")
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

/// A filha ligada abriu com a linha de retorno e sem a ligação: a resposta dela não volta sozinha.
pub(crate) fn line_unlinked(st: &AppState, child: &str, detail: &str) {
    report(st, child, "link_line_unlinked", "filha recebeu a linha de retorno e abriu sem ligação", detail);
}

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
        // Sem aviso ao pai: o continue sozinho dá a resposta real, que volta com outra chave. Relê depois: a espera
        // que desiste (Cancelar, teto, envio recusado) devolve o erro ao pai.
        Err(Refusal::ApiError) => {
            report(st, child, "link_return_api_error", "turno da filha parou numa queda que continua sozinha; o erro não volta", "");
            return Outcome::Retry;
        }
        Ok(_) | Err(Refusal::NoAnswer(_)) => {}
    }
    if !still_same(current().as_deref().map(Vec::as_slice), child, reply_at) { return Outcome::Moved; }
    let Some(Reply { text, key, dropped }) = reply(&signed(child, check), found, || rate.allow(child, Instant::now())) else { return Outcome::Done };
    if let Some((code, reason)) = dropped { report(st, child, code, reason, ""); }
    if dropped == Some(RATE_LIMITED) && !rate.notice_once(child, Instant::now()) { return Outcome::Done; }
    match deliver(st, check.parent, &text).await {
        Sent::Yes => {}
        Sent::Maybe => report(st, child, "link_return_maybe_delivered", "a resposta saiu para o pai e a confirmação não veio; talvez entregue, não repete", ""),
        Sent::No(status, answer) => {
            let code = answer["detail"]["code"].as_str().unwrap_or("erro_envio_falhou_desconhecida");
            report(st, child, code, "entrega da resposta da filha ao pai falhou", &status.as_u16().to_string());
            return if transient(status) { Outcome::Retry } else { Outcome::Done };
        }
    }
    if dropped.is_none() { rate.record(child, Instant::now()); }
    let (dir, name, notice) = (st.hangar_dir.clone(), child.to_owned(), dropped.is_some());
    let written = blocking(move || link_state::update(&dir, |all| {
        if let Some(entry) = all.get_mut(&name) { entry.returned_at = Some(reply_at); entry.returned_key = Some(key); entry.returned_notice = notice; }
    })).await;
    if let Err(e) = written {
        report(st, child, "link_returns_write_failed", "returned_at da filha não gravado", &io_kind(&e));
    }
    // O aviso de resposta descartada já tem código próprio no diário.
    if dropped.is_none() { st.diag.record_line(delivered_line(child)); }
    Outcome::Done
}

/// Como o pai chama a filha: com a máquina daqui quando ele é de outra (`[de: srv::filha]`).
fn signed(child: &str, check: &Check) -> String {
    if is_remote(check.parent) && !check.server.is_empty() { format!("{}::{child}", check.server) } else { child.to_owned() }
}

/// Como terminou a entrega ao pai.
#[derive(Debug, PartialEq)]
enum Sent {
    Yes,
    /// Saiu daqui e a resposta não veio: pode ter chegado, e repetir daria o recado em dobro.
    Maybe,
    No(StatusCode, serde_json::Value),
}

/// O pai de outra máquina recebe pela API de lá, como o recado `servidor::sessao`. Lá fora do ar (a conexão
/// nem abriu) é falha passageira: tenta de novo até a resposta passar de `MAX_AGE`.
async fn deliver(st: &AppState, parent: &str, text: &str) -> Sent {
    let Some((server, name)) = parent.split_once("::") else {
        let (status, answer) = crate::session_write::input::deliver_reply(st, parent, text).await;
        return if status.is_success() { Sent::Yes } else { Sent::No(status, answer) };
    };
    let path = format!("/api/sessions/{}/input", super::exit::segment(name));
    remote_sent(st.peers.call_unsent(server, reqwest::Method::POST, &path, Some(&serde_json::json!({"text": text}))).await)
}

fn remote_sent(result: Result<Option<serde_json::Value>, (crate::groups::peers::PeerError, bool)>) -> Sent {
    use crate::groups::peers::PeerError;
    use serde_json::json;
    match result {
        Ok(_) => Sent::Yes,
        Err((PeerError::Refused { status, detail }, _)) => Sent::No(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), json!({"detail": detail})),
        Err((PeerError::Unknown, _)) => Sent::No(StatusCode::NOT_FOUND, json!({"detail": {"code": "link_return_parent_machine_unknown"}})),
        Err((PeerError::Transport(_), false)) => Sent::Maybe,
        Err(_) => Sent::No(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": {"code": "link_return_parent_unreachable"}})),
    }
}

/// Sucesso vai como `ok`: o `report` grava `erro`, e passado o teto do dia só `ok` é descartado.
fn delivered_line(child: &str) -> serde_json::Value {
    serde_json::json!({"evento": "rust.link_return_delivered", "nivel": "ok", "sessao": child, "codigo": "link_return_delivered"})
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
        Ok((answer, key)) if allow() => Some(Reply { text: message(child, answer, &key), key, dropped: None }),
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
            link: Some(LinkReturn { linked_at: NOW - 600.0, ..LinkReturn::default() }), now: NOW, parent: "pai", server: "casa", continues: false,
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
    fn a_parent_on_another_machine_asks_and_reads_the_child_with_this_machine() {
        let remote = Check { parent: "casa::pai", server: "lab", ..check() };
        assert!(passes(&remote, asked("[de: casa::pai] faz isso")));
        assert!(!passes(&remote, asked("[de: pai] faz isso")), "o homônimo daqui não é o pai");
        assert_eq!(signed("f", &remote), "lab::f");
        assert_eq!(signed("f", &check()), "f", "o pai daqui lê o nome cru");
        let Reply { text, .. } = reply(&signed("f", &remote), Ok(("feito", "k".into())), || true).unwrap();
        assert_eq!(text, "[de: lab::f] feito");
    }

    #[test]
    fn the_initial_prompt_answered_before_the_link_returns() {
        let early = Check { link: Some(LinkReturn { linked_at: NOW - 30.0, ..LinkReturn::default() }), ..check() };
        assert!(settled(&early));
        let first = vec![ev(ChatKind::UserMsg, "prompt inicial"), ev(ChatKind::AssistantMsg, "pronto")];
        assert_eq!(decide(&early, &turn(first.clone(), true)).map(|(a, _)| a), Ok("pronto"));
        assert_eq!(decide(&early, &turn(first, false)), Err(Refusal::NotAsked), "sem ler desde o começo, não é o prompt inicial");
        assert_eq!(decide(&early, &turn(asked("[de: pai] x"), true)), Err(Refusal::NotAsked), "outro turno de antes da ligação não volta");
        // A entrada recuperada (`ensure_linked`) não é a ligação: a resposta de antes dela não volta, nem a do prompt inicial.
        let recovered = Check { link: Some(LinkReturn { linked_at: NOW - 30.0, recovered: true, ..LinkReturn::default() }), ..check() };
        assert!(!settled(&recovered));
        assert!(settled(&Check { last_reply_at: Some(NOW - 10.0), ..recovered }), "a de depois volta");
    }

    #[test]
    fn only_a_request_that_never_left_is_sent_again() {
        use crate::groups::peers::PeerError;
        let no = |sent| match sent { Sent::No(status, _) => status, other => panic!("{other:?}") };
        assert_eq!(remote_sent(Ok(None)), Sent::Yes);
        assert_eq!(remote_sent(Err((PeerError::Transport("prazo de leitura".into()), false))), Sent::Maybe, "saiu: talvez entregue");
        assert!(transient(no(remote_sent(Err((PeerError::Transport("recusada".into()), true))))), "nem conectou: repete");
        let busy = PeerError::Refused { status: 409, detail: serde_json::json!({"code": "ocupado"}) };
        assert!(transient(no(remote_sent(Err((busy, false))))));
        assert!(!transient(no(remote_sent(Err((PeerError::Unknown, true))))));
    }

    #[test]
    fn the_child_line_keeps_the_first_message_returnable() {
        let first = format!("{} faz a busca", hangar_delegation::guidance::child_prefix("pai"));
        let whole = turn(vec![ev(ChatKind::UserMsg, &first), ev(ChatKind::AssistantMsg, "achei")], true);
        assert!(asked_by_parent(&whole, "pai", "casa"), "a primeira da conversa continua sendo do pai");
        assert_eq!(decide(&check(), &whole).map(|(a, _)| a), Ok("achei"));
        // Conversa lida só pelo fim: é a linha que diz que o pedido veio do pai.
        let cut = turn(vec![ev(ChatKind::UserMsg, &first), ev(ChatKind::AssistantMsg, "achei")], false);
        assert!(asked_by_parent(&cut, "pai", "casa"), "a linha marca o pedido do pai");
        assert!(!asked_by_parent(&cut, "outro", "casa"), "a linha de outro pai não serve");
        let plain = turn(vec![ev(ChatKind::UserMsg, "faz a busca"), ev(ChatKind::AssistantMsg, "achei")], false);
        assert!(!asked_by_parent(&plain, "pai", "casa"), "sem a linha e sem ler o começo, o turno não é do pai");
    }

    #[test]
    fn needs_a_reply() {
        assert!(!settled(&Check { last_reply_at: None, ..check() }));
        assert!(settled(&check()));
    }

    #[test]
    fn claude_codex_and_antigravity() {
        for p in ["claude", "claude-headless", "codex", "antigravity"] { assert!(returnable(p), "{p}"); }
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
        let link = |linked_at, returned_at| Some(LinkReturn { linked_at, returned_at, ..LinkReturn::default() });
        assert!(!settled(&Check { link: None, ..check() }));
        assert!(settled(&Check { link: link(at + 1.0, None), ..check() }), "antes da ligação, o decide confere");
        assert!(!settled(&Check { link: link(at + 1.0, Some(0.0)), ..check() }), "depois de um retorno, só resposta posterior à ligação");
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
        let returned = Check { link: Some(LinkReturn { linked_at: 0.0, returned_at: Some(1.0), returned_key: Some(key), ..LinkReturn::default() }), ..check() };
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

    /// O turno parou numa queda que continua sozinha: a linha de erro não volta ao pai, com ou sem a marca do
    /// parser. Sem continuar (limite, login, teto, interruptor desligado), o erro volta como antes.
    #[test]
    fn an_api_error_line_is_not_the_answer() {
        let c = Check { continues: true, ..check() };
        let marked = ChatEvent {
            notice_meta: serde_json::json!({"level": "error"}).as_object().cloned(),
            ..ev(ChatKind::AssistantMsg, "Can't reach the API server (EAI_AGAIN)")
        };
        for error in [marked, ev(ChatKind::AssistantMsg, "API Error: Connection lost mid-response.")] {
            let mut events = asked("[de: pai] x");
            events[3] = error.clone();
            assert_eq!(decide(&c, &turn(events.clone(), false)), Err(Refusal::ApiError));
            assert!(reply("f", Err(Refusal::ApiError), || true).is_none(), "nem aviso de sem resposta");
            let back = decide(&check(), &turn(events, false)).map(|(a, _)| a.to_owned());
            assert_eq!(back, Ok(error.text.unwrap()), "sem o continue, o pai recebe o erro");
        }
        let limit = ChatEvent {
            notice_meta: serde_json::json!({"level": "error"}).as_object().cloned(),
            ..ev(ChatKind::AssistantMsg, "You've hit your 5-hour limit")
        };
        let mut events = asked("[de: pai] x");
        events[3] = limit;
        assert_eq!(decide(&check(), &turn(events, false)).map(|(a, _)| a), Ok("You've hit your 5-hour limit"));
        // O aviso da CLI no formato de resposta (só com `key`) não é a linha de erro.
        let note = ChatEvent { notice_meta: serde_json::json!({"key": "x"}).as_object().cloned(), ..ev(ChatKind::AssistantMsg, "feito") };
        assert!(!api_error(&note));
    }

    /// Só a queda passageira no fim da conversa tem chave de continuar; limite, login e resposta normal, não.
    #[test]
    fn only_a_transient_stop_at_the_end_continues() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("f.jsonl");
        let line = |fields: serde_json::Value| {
            let mut line = serde_json::json!({"type": "assistant", "isSidechain": false, "uuid": "e1", "isApiErrorMessage": true,
                "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "API Error"}]}});
            for (key, value) in fields.as_object().unwrap() { line[key] = value.clone(); }
            format!("{line}\n")
        };
        std::fs::write(&jsonl, line(serde_json::json!({"error": "server_error", "apiError": "connection_failed"}))).unwrap();
        assert_eq!(transient_stop(&jsonl).as_deref(), Some("e1"));
        for refused in [serde_json::json!({"error": "rate_limit", "apiError": "usage_limit_reached", "apiErrorStatus": 429}),
            serde_json::json!({"error": "authentication_failed"}), serde_json::json!({"error": "invalid_request", "apiError": "prompt_too_long"}),
            serde_json::json!({"isApiErrorMessage": false, "message": {"model": "claude-opus-5-5", "content": [{"type": "text", "text": "ok"}]}})] {
            std::fs::write(&jsonl, line(refused.clone())).unwrap();
            assert_eq!(transient_stop(&jsonl), None, "{refused}");
        }
        assert_eq!(transient_stop(&dir.path().join("ausente.jsonl")), None);
    }

    /// O pai pede, a rede cai, o Hangar manda o "continue" sozinho e a filha termina: a resposta volta ao pai.
    #[test]
    fn an_auto_continue_does_not_hide_the_parent_request() {
        let c = check();
        let error = || ev(ChatKind::AssistantMsg, "API Error: Unable to connect to API (EAI_AGAIN)");
        let continued = |request: &str, again: &str| {
            let mut events = asked(request);
            events.truncate(3);
            events.extend([error(), ev(ChatKind::UserMsg, again), ev(ChatKind::AssistantMsg, "feito")]);
            events
        };
        for again in [hangar_auto_continue::text::TRANSIENT, hangar_auto_continue::text::LIMIT, hangar_auto_continue::text::REBOOT] {
            assert_eq!(decide(&c, &turn(continued("[de: pai] x", again), false)).map(|(a, _)| a), Ok("feito"), "{again}");
            // O "é a primeira da conversa" também pula o continue.
            let first = vec![ev(ChatKind::UserMsg, "prompt inicial"), error(), ev(ChatKind::UserMsg, again), ev(ChatKind::AssistantMsg, "pronto")];
            assert_eq!(decide(&c, &turn(first, true)).map(|(a, _)| a), Ok("pronto"), "{again}");
            // O turno cortado antes do continue terminou depois dele.
            let mut cut = continued("[de: pai] x", again);
            cut[3] = ev(ChatKind::Notice, "interrupted");
            assert_eq!(decide(&c, &turn(cut, false)).map(|(a, _)| a), Ok("feito"), "{again}");
        }
        // O "continua" que a pessoa escreve na filha é turno dado à mão.
        assert_eq!(decide(&c, &turn(continued("[de: pai] x", "continua"), true)), Err(Refusal::NotAsked));
        // A janela cresce até a mensagem do pai, mesmo com o continue dentro da primeira.
        let mut long = asked("[de: pai] x");
        long.truncate(3);
        long.extend(std::iter::repeat_n(ev(ChatKind::ToolResult, ""), 500));
        long.extend([error(), ev(ChatKind::UserMsg, hangar_auto_continue::text::TRANSIENT), ev(ChatKind::AssistantMsg, "feito")]);
        let read = |limit: usize| Ok(long[long.len().saturating_sub(limit)..].to_vec());
        let found = read_turn(read, 1024).unwrap().unwrap();
        assert_eq!(decide(&c, &found).map(|(a, _)| a), Ok("feito"));
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
        let notified = Check { link: Some(LinkReturn { linked_at: 0.0, returned_at: Some(1.0), returned_key: Some(key), ..LinkReturn::default() }), ..check() };
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

    /// O `child_status` diz `returned` pela mesma identidade que o retorno grava, com e sem resposta final.
    #[test]
    fn the_reply_key_is_the_one_the_return_records() {
        let mut events = asked("[de: pai] x");
        events[3].id = "uuid-resposta".into();
        let (_, key) = decide(&check(), &turn(events.clone(), false)).unwrap();
        assert_eq!(reply_key(&events), key);
        let mut tool = asked("[de: pai] x");
        tool.push(ev(ChatKind::ToolUse, ""));
        let Err(Refusal::NoAnswer(unanswered)) = decide(&check(), &turn(tool.clone(), false)) else { panic!("esperava NoAnswer") };
        assert_eq!(reply_key(&tool), unanswered);
    }

    /// O retorno entregue é sucesso no diário, nunca uma linha de erro.
    #[test]
    fn a_delivered_return_is_an_ok_line() {
        let line = delivered_line("filha");
        assert_eq!((&line["evento"], &line["nivel"], &line["sessao"], &line["codigo"]),
            (&serde_json::json!("rust.link_return_delivered"), &serde_json::json!("ok"), &serde_json::json!("filha"), &serde_json::json!("link_return_delivered")));
    }

    fn waiting(id: &str, question: &str, options: &[&str]) -> SessionRow {
        serde_json::from_value(serde_json::json!({"name": "f", "provider": "claude", "state": "awaiting_input", "linked_by": "pai",
            "interaction_id": id, "question": question, "options": options})).unwrap()
    }

    fn linked() -> LinkReturn { LinkReturn { linked_at: 1.0, ..LinkReturn::default() } }

    #[tokio::test(start_paused = true)]
    async fn a_question_goes_once_per_interaction_after_settle() {
        let (mut w, t0) = (Watcher::default(), Instant::now());
        let row = waiting("q1", "Qual pasta?", &["src", "docs"]);
        let question = Some(&InteractionKind::Question);
        assert_eq!(w.question_step(&row, &linked(), question, t0), QuestionStep::Wait);
        assert_eq!(w.question_step(&row, &linked(), question, t0 + SETTLE - Duration::from_secs(1)), QuestionStep::Wait);
        let QuestionStep::Send(text) = w.question_step(&row, &linked(), question, t0 + SETTLE) else { panic!("esperava Send") };
        assert_eq!(text, "[de: f] pergunta (interaction_id q1): Qual pasta?\n1. src\n2. docs\nResponda com child_answer (alvo f, interaction_id q1).\nSe for perguntar ao dono, use AskUserQuestion com metadata {source: \"hangar-child:q1\"}.");
        // Avisada (o mesmo ID gravado como `asked_key`): não vai de novo.
        let asked = LinkReturn { asked_key: Some("q1".into()), ..linked() };
        assert_eq!(w.question_step(&row, &asked, question, t0 + SETTLE * 2), QuestionStep::Skip);
        assert_eq!(Watcher::default().question_step(&row, &asked, question, t0), QuestionStep::Skip);
        // Outra interação recomeça a espera.
        assert_eq!(w.question_step(&waiting("q2", "E agora?", &[]), &linked(), question, t0 + SETTLE * 2), QuestionStep::Wait);
        // Parada ou sem `interaction_id`: nada a avisar.
        let mut idle = waiting("q1", "x", &[]);
        idle.state = "idle".into();
        assert_eq!(w.question_step(&idle, &linked(), question, t0 + SETTLE * 3), QuestionStep::Skip);
        assert_eq!(w.question_step(&waiting("", "x", &[]), &linked(), question, t0 + SETTLE * 3), QuestionStep::Skip);
        // O formulário da pergunta vence o menu da tela e traz o texto e as opções dele.
        let mut form = waiting("menu1", "Do you want to proceed?", &["Yes", "No"]);
        form.ask_form = Some(serde_json::json!({"interaction_id": "form1", "questions": [{"header": "h", "question": "Qual base?",
            "options": [{"label": "dev"}, {"label": "qa"}]}]}));
        let mut w = Watcher::default();
        w.question_step(&form, &linked(), question, t0);
        let QuestionStep::Send(text) = w.question_step(&form, &linked(), question, t0 + SETTLE) else { panic!("esperava Send") };
        assert!(text.starts_with("[de: f] pergunta (interaction_id form1): Qual base?\n1. dev\n2. qa\n"), "{text}");
    }

    #[test]
    fn a_long_question_is_cut_at_300_chars() {
        let long = "é".repeat(ASK_CHARS + 10);
        let row = waiting("q1", &long, &[]);
        let text = question_message("f", "q1", &row, Some(&InteractionKind::Question));
        assert!(text.contains(&format!("{}…", "é".repeat(ASK_CHARS))), "{text}");
        assert!(!text.contains(&"é".repeat(ASK_CHARS + 1)));
    }

    fn only_notifies(text: &str, asked: &str) {
        assert_eq!(text, format!("[de: f] pedido esperando o dono: {asked}. O dono decide no app."));
        assert!(!text.contains("child_answer"), "{text}");
    }

    fn settled_step(row: &SessionRow, kind: Option<&InteractionKind>, t0: Instant) -> QuestionStep {
        let mut w = Watcher::default();
        w.question_step(row, &linked(), kind, t0);
        w.question_step(row, &linked(), kind, t0 + SETTLE)
    }

    #[tokio::test(start_paused = true)]
    async fn a_permission_only_notifies() {
        let row = waiting("p1", "Permitir Bash?", &["Sim", "Não"]);
        let QuestionStep::Send(text) = settled_step(&row, Some(&InteractionKind::Approval), Instant::now()) else { panic!("esperava Send") };
        only_notifies(&text, "Permitir Bash?");
    }

    #[tokio::test(start_paused = true)]
    async fn a_terminal_menu_only_notifies() {
        // "Do you want to proceed?" e a aprovação de plano (`plan:<id>`) são `Menu` no Claude com terminal.
        for id in ["m1", "plan:toolu_1"] {
            let row = waiting(id, "Do you want to proceed?", &["Yes", "No"]);
            let QuestionStep::Send(text) = settled_step(&row, Some(&InteractionKind::Menu), Instant::now()) else { panic!("esperava Send") };
            only_notifies(&text, "Do you want to proceed?");
        }
    }

    /// Tipo ilegível (vínculo ainda por ligar) espera: lido depois, a pergunta vai como pergunta; nunca lido,
    /// vira o aviso ao dono passado `KIND_GRACE`.
    #[tokio::test(start_paused = true)]
    async fn an_unread_kind_waits_before_it_counts_as_a_permission() {
        let (row, t0) = (waiting("x1", "Qual pasta?", &["src"]), Instant::now());
        let mut w = Watcher::default();
        assert_eq!(w.question_step(&row, &linked(), None, t0), QuestionStep::Wait);
        assert_eq!(w.question_step(&row, &linked(), None, t0 + SETTLE), QuestionStep::Wait);
        let QuestionStep::Send(text) = w.question_step(&row, &linked(), Some(&InteractionKind::Question), t0 + SETTLE + Duration::from_secs(2))
            else { panic!("esperava Send") };
        assert!(text.contains("child_answer"), "{text}");
        let mut w = Watcher::default();
        w.question_step(&row, &linked(), None, t0);
        assert_eq!(w.question_step(&row, &linked(), None, t0 + SETTLE + KIND_GRACE - Duration::from_secs(1)), QuestionStep::Wait);
        let QuestionStep::Send(text) = w.question_step(&row, &linked(), None, t0 + SETTLE + KIND_GRACE) else { panic!("esperava Send") };
        only_notifies(&text, "Qual pasta?");
    }

    /// Só a falha passageira do cancelamento mantém a linha do aviso registrada para tentar de novo.
    #[test]
    fn a_transient_cancel_failure_keeps_the_line() {
        assert!(cancel_settled(&Ok(true)) && cancel_settled(&Ok(false)));
        for code in ["busy", "asleep_waking", "runtime_unhealthy", crate::runtime::codex_relaunch::STARTING] {
            assert!(!cancel_settled(&Err(code.into())), "{code}");
        }
        for code in ["unowned", "queue_entry_not_found"] { assert!(cancel_settled(&Err(code.into())), "{code}"); }
    }

    #[tokio::test(start_paused = true)]
    async fn three_notices_per_child_in_ten_minutes() {
        let (mut w, t0) = (Watcher::default(), Instant::now());
        let question = Some(&InteractionKind::Question);
        let send = |w: &mut Watcher, id: &str, at: Instant| {
            let row = waiting(id, "x", &[]);
            w.question_step(&row, &linked(), question, at);
            w.question_step(&row, &linked(), question, at + SETTLE)
        };
        for i in 0..RATE_MAX {
            let at = t0 + Duration::from_secs(60 * i as u64);
            assert!(matches!(send(&mut w, format!("q{i}").as_str(), at), QuestionStep::Send(_)), "{i}");
            w.question_rate.record("f", at + SETTLE);
        }
        assert_eq!(send(&mut w, "q9", t0 + Duration::from_secs(300)), QuestionStep::Limited);
        // As respostas têm limite próprio: o das perguntas não o gasta.
        assert!(w.rate.allow("f", t0 + Duration::from_secs(300)));
        let later = t0 + SETTLE + RATE_WINDOW + Duration::from_secs(60 * RATE_MAX as u64);
        assert!(matches!(send(&mut w, "q10", later), QuestionStep::Send(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_restart_a_notice_is_not_repeated_but_a_question_is() {
        let t0 = Instant::now();
        let approval = Some(&InteractionKind::Approval);
        // Menu e permissão: o reinício troca o ID, o texto gravado segura o reaviso, com a linha na fila.
        let QuestionStep::Send(text) = settled_step(&waiting("p1", "Permitir Bash?", &[]), approval, t0) else { panic!("esperava Send") };
        let asked = LinkReturn { asked_key: Some(ask_key("p1", approval, &text)), asked_queue_id: Some("row1".into()), ..linked() };
        let reborn = waiting("p2", "Permitir Bash?", &[]);
        let mut w = Watcher::default();
        assert_eq!(w.question_step(&reborn, &asked, approval, t0), QuestionStep::Skip);
        assert_eq!(w.question_step(&reborn, &asked, approval, t0 + SETTLE), QuestionStep::Skip);
        assert_eq!(w.question_step(&reborn, &asked, None, t0 + SETTLE), QuestionStep::Skip);
        // Sem reinício, outro pedido de mesmo texto é outro aviso.
        let mut w = Watcher::default();
        w.notified.insert("f".into(), "p1".into());
        let done = LinkReturn { asked_queue_id: None, ..asked.clone() };
        assert_eq!(w.question_step(&reborn, &done, approval, t0), QuestionStep::Wait);
        assert!(matches!(w.question_step(&reborn, &done, approval, t0 + SETTLE), QuestionStep::Send(_)));
        // A pergunta vai de novo com o ID novo: a linha do aviso antigo sai antes.
        let question = Some(&InteractionKind::Question);
        let asked = LinkReturn { asked_key: Some("q1".into()), asked_queue_id: Some("row1".into()), ..linked() };
        let reborn = waiting("q2", "Qual pasta?", &[]);
        let mut w = Watcher::default();
        assert_eq!(w.question_step(&reborn, &asked, question, t0), QuestionStep::Cancel("row1".into()));
        let cleared = LinkReturn { asked_queue_id: None, ..asked };
        let QuestionStep::Send(text) = w.question_step(&reborn, &cleared, question, t0 + SETTLE) else { panic!("esperava Send") };
        assert!(text.contains("interaction_id q2"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_resolved_question_cancels_the_waiting_line() {
        let (mut w, t0) = (Watcher::default(), Instant::now());
        let question = Some(&InteractionKind::Question);
        let queued = LinkReturn { asked_key: Some("q1".into()), asked_queue_id: Some("row1".into()), ..linked() };
        // Ainda aberta: a linha fica.
        assert_eq!(w.question_step(&waiting("q1", "x", &[]), &queued, question, t0), QuestionStep::Skip);
        // Respondida (a filha voltou a trabalhar ou parou): tira a linha se ainda espera.
        let mut working = waiting("q1", "x", &[]);
        working.state = "working".into();
        assert_eq!(w.question_step(&working, &queued, question, t0), QuestionStep::Cancel("row1".into()));
        // Trocada por outra: a do aviso antigo sai antes de avisar a nova.
        assert_eq!(w.question_step(&waiting("q2", "y", &[]), &queued, question, t0 + SETTLE), QuestionStep::Cancel("row1".into()));
        // Linha já tirada (ou entregue sem id): a pergunta fechou sem o `child_answer` do pai, e a chave sai depois.
        let done = LinkReturn { asked_queue_id: None, ..queued };
        assert_eq!(w.question_step(&working, &done, question, t0), QuestionStep::Answered("q1".into()));
        // Sem chave nem linha: nada, e a vigia esquece a interação.
        w.notified.insert("f".into(), "q1".into());
        assert_eq!(w.question_step(&working, &linked(), question, t0), QuestionStep::Skip);
        assert!(!w.notified.contains_key("f"));
    }

    /// Permissão avisada que fechou: depois de um reinício, outra de mesmo texto é outro aviso.
    #[tokio::test(start_paused = true)]
    async fn a_closed_notice_does_not_hide_the_next_one_with_the_same_text() {
        let (t0, approval) = (Instant::now(), Some(&InteractionKind::Approval));
        let QuestionStep::Send(text) = settled_step(&waiting("p1", "Permitir Bash?", &[]), approval, t0) else { panic!("esperava Send") };
        let asked = LinkReturn { asked_key: Some(ask_key("p1", approval, &text)), ..linked() };
        let mut closed = waiting("p1", "Permitir Bash?", &[]);
        closed.state = "working".into();
        let mut w = Watcher::default();
        w.notified.insert("f".into(), "p1".into());
        assert_eq!(w.question_step(&closed, &asked, approval, t0), QuestionStep::Forget);
        // Vigia nova com a chave ainda gravada: espera o pedido voltar com outro ID antes de soltar a chave.
        let mut fresh = Watcher::default();
        assert_eq!(fresh.question_step(&closed, &asked, approval, t0), QuestionStep::Wait);
        assert_eq!(fresh.question_step(&closed, &asked, approval, t0 + SETTLE), QuestionStep::Forget);
        // A chave saiu do registro: a vigia nova avisa o pedido seguinte de mesmo texto.
        let mut reborn = Watcher::default();
        let next = waiting("p2", "Permitir Bash?", &[]);
        reborn.question_step(&next, &linked(), approval, t0);
        assert!(matches!(reborn.question_step(&next, &linked(), approval, t0 + SETTLE), QuestionStep::Send(_)));
    }

    /// A pergunta avisada que fecha sem o `child_answer` do pai foi respondida na filha; a que o pai respondeu, não.
    #[tokio::test(start_paused = true)]
    async fn a_question_closed_without_the_parent_answer_was_answered_in_the_child() {
        let t0 = Instant::now();
        let question = Some(&InteractionKind::Question);
        let mut closed = waiting("q1", "Qual pasta?", &[]);
        closed.state = "working".into();
        let asked = LinkReturn { asked_key: Some("q1".into()), ..linked() };
        let mut w = Watcher::default();
        w.notified.insert("f".into(), "q1".into());
        assert_eq!(w.question_step(&closed, &asked, question, t0), QuestionStep::Answered("q1".into()));
        // O pai respondeu esta pelo `child_answer`: só a chave sai.
        let by_parent = LinkReturn { parent_answered: Some("q1".into()), ..asked.clone() };
        let mut w = Watcher::default();
        w.notified.insert("f".into(), "q1".into());
        assert_eq!(w.question_step(&closed, &by_parent, question, t0), QuestionStep::Forget);
        // A marca de uma pergunta anterior não vale para esta.
        let old_mark = LinkReturn { parent_answered: Some("q0".into()), ..asked.clone() };
        assert_eq!(w.question_step(&closed, &old_mark, question, t0), QuestionStep::Answered("q1".into()));
        // Depois de reiniciar, espera o `SETTLE` (o pedido pode voltar com outro ID) e conta igual.
        let mut fresh = Watcher::default();
        assert_eq!(fresh.question_step(&closed, &asked, question, t0), QuestionStep::Wait);
        assert_eq!(fresh.question_step(&closed, &asked, question, t0 + SETTLE), QuestionStep::Answered("q1".into()));
        // Menu e permissão o dono sempre decide na filha: não viram resposta ao pai.
        let notice = LinkReturn { asked_key: Some(format!("{NOTICE_KEY}abc")), ..linked() };
        let mut w = Watcher::default();
        w.notified.insert("f".into(), "p1".into());
        assert_eq!(w.question_step(&closed, &notice, Some(&InteractionKind::Approval), t0), QuestionStep::Forget);
    }

    fn call(name: &str, id: &str) -> ChatEvent {
        ChatEvent { kind: ChatKind::ToolUse, tool_name: Some(name.into()), tool_use_id: Some(id.into()), ..Default::default() }
    }

    fn result(id: &str, text: &str) -> ChatEvent {
        ChatEvent { kind: ChatKind::ToolResult, tool_use_id: Some(id.into()), result: Some(text.into()), ..Default::default() }
    }

    #[test]
    fn the_answer_given_in_the_child_is_the_result_of_its_last_question() {
        let answered = "User has answered your questions: \"Qual pasta?\"=\"src\".";
        let events = vec![call("AskUserQuestion", "t0"), result("t0", "antiga"), ev(ChatKind::UserMsg, "x"),
            call("Bash", "b1"), result("b1", "saída"), call("AskUserQuestion", "t1"), result("t1", answered)];
        assert_eq!(asked_answer(&events).as_deref(), Some(answered));
        // A resposta ainda não chegou ao transcript: nada lido, nunca a da pergunta anterior.
        assert_eq!(asked_answer(&events[..6]), None);
        // Codex pergunta pelo `request_user_input`.
        assert_eq!(asked_answer(&[call("request_user_input", "c1"), result("c1", "docs")]).as_deref(), Some("docs"));
        let long = "é".repeat(ANSWER_CHARS + 5);
        assert_eq!(asked_answer(&[call("AskUserQuestion", "t"), result("t", &long)]).unwrap().chars().count(), ANSWER_CHARS + 1);
    }

    fn parent(form: serde_json::Value) -> SessionRow {
        serde_json::from_value(serde_json::json!({"name": "pai", "provider": "claude", "state": "awaiting_input", "ask_form": form})).unwrap()
    }

    /// O pai parado na pergunta que repassa a da filha recebe a resposta nela, pelo ID publicado e em texto livre.
    #[test]
    fn the_relay_question_of_the_parent_is_found_by_its_source_and_answered_in_free_text() {
        let form = serde_json::json!({"interaction_id": "pq1", "source": "hangar-child:q1", "questions": [
            {"header": "h", "question": "Qual pasta?", "options": [{"label": "src"}, {"label": "docs"}]}]});
        let (id, found) = relay_question(&parent(form.clone()), "hangar-child:q1").expect("pergunta de repasse");
        assert_eq!(id, "pq1");
        let text = relay_answer_text("f", Some("\"Qual pasta?\"=\"src\"."));
        assert_eq!(text, "Respondida pelo dono na filha f: \"Qual pasta?\"=\"src\". Não repita nem mande à filha.");
        let body = free_text_body(&found, &id, &text);
        assert_eq!(body, serde_json::json!({"interaction_id": "pq1", "answers": [{"kind": "text", "value": text, "type_index": 2}]}));
        // O corpo passa pela mesma validação da rota.
        let parsed = crate::session_write::answer::parse_body(&axum::body::Bytes::from(body.to_string())).expect("corpo do /answer");
        assert_eq!(parsed.answers_json()[0]["value"], serde_json::json!(text));
        // Outra filha, outra pergunta, ou pergunta sem a marca: não é esta.
        assert!(relay_question(&parent(form.clone()), "hangar-child:q2").is_none());
        let mut unmarked = form.clone();
        unmarked.as_object_mut().unwrap().remove("source");
        assert!(relay_question(&parent(unmarked), "hangar-child:q1").is_none());
        let mut idle = parent(form);
        idle.state = "idle".into();
        assert!(relay_question(&idle, "hangar-child:q1").is_none(), "pergunta já fechada no pai");
        assert_eq!(relay_answer_text("f", None), "Encerrada na filha f sem resposta lida. Não repita nem mande à filha.");
        assert_eq!(plain("a\u{1b}b\nc\td\re"), "a b\nc\td e");
    }

    /// Sem pergunta de repasse no pai, o recado vai pela fila dizendo que não há o que responder.
    #[test]
    fn without_a_relay_question_the_parent_gets_a_line() {
        assert_eq!(answered_message("f", "q1", Some("src.")),
            "[de: f] a pergunta (interaction_id q1) foi respondida pelo dono na filha: src. Não responda nem repita.");
        assert_eq!(answered_message("f", "q1", None),
            "[de: f] a pergunta (interaction_id q1) foi encerrada na filha sem resposta lida. Não responda nem repita.");
    }

    /// Só a recusa clara tira a marca do `child_answer`: resposta que pode ter saído fica marcada.
    #[test]
    fn the_parent_mark_stays_when_the_answer_may_have_gone() {
        let coded = |code: &str| serde_json::json!({"detail": {"code": code}});
        assert!(answer_maybe_sent(200, &serde_json::json!({"ok": true})));
        assert!(answer_maybe_sent(409, &coded("erro_sem_confirmacao_resposta")));
        assert!(answer_maybe_sent(502, &coded("erro_envio_falhou")));
        assert!(!answer_maybe_sent(409, &coded("erro_interacao_desatualizada")));
        assert!(!answer_maybe_sent(503, &coded("runtime_unhealthy")));
    }

    #[test]
    fn answer_is_cut() {
        assert_eq!(message("f", "oi", "k1"), "[de: f] oi");
        let long = "é".repeat(MAX_CHARS + 5);
        let cut = message("f", &long, "k1");
        assert!(cut.starts_with(&format!("[de: f] {}", "é".repeat(MAX_CHARS))));
        assert!(cut.ends_with("(cortado; o resto pela tool child_status, offset 8000, answer_key k1)"), "{cut}");
    }
}
