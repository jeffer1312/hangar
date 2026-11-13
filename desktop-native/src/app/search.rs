//! "Buscar conversas" (Ctrl+K): paleta no meio da janela, na superfície de vidro dos popovers. Duas abas:
//! - Trechos: busca de conteúdo em todas as conversas (vivas e arquivadas, `/api/search`), agrupada por conversa na lista da
//!   esquerda, e a conversa inteira do trecho escolhido à direita, com o mesmo desenho do chat e rolada até ele. As sessões
//!   vivas pelo nome entram no topo da lista, como a paleta do Zeron.
//! - Perguntar (`/api/ask-history`): a resposta e só as conversas que ela cita.
//!
//! Enter abre a sessão viva; a arquivada é retomada numa sessão nova (`/api/archive/…/resume`). Sessões e busca de conteúdo
//! cobrem todas as máquinas próprias, como o web; cada trecho abre e retoma na máquina de onde veio. O "Perguntar" segue só
//! na ativa, como no web.
use super::*;
use super::device::Remote;
use super::resume_folder::{FOLDER_GONE, ResumeFolder, ResumeFolderEvent};
use super::costs::web_with;
use super::sidebar::Target;
use super::tab_preview::ConversationSlot;
use serde::Deserialize;
use std::ops::Range;

/// Espera entre a última tecla e a busca, como o web.
const DEBOUNCE: Duration = Duration::from_millis(250);
const SESSIONS_MAX: usize = 8;
/// Com o índice FTS a busca responde em milissegundos; esperar mais só prende a linha numa máquina inalcançável.
const MACHINE_WAIT: Duration = Duration::from_secs(8);
/// Espera da prévia: percorrer a lista com as setas não lê a conversa de cada trecho do caminho.
const PREVIEW_DELAY: Duration = Duration::from_millis(150);
/// Trechos mostrados por conversa.
const HITS_SHOWN: usize = 3;
/// Total de conversas onde a máquina procurou; servidor antigo não manda.
const TOTAL_HEADER: &str = "x-hangar-conversations";
const LABEL_MAX: usize = 58;
/// Mensagens lidas da sessão viva escolhida pelo nome, que não tem trecho para alcançar.
const SESSION_TAIL: usize = 120;
/// Mensagens que a prévia desenha antes e depois do trecho, e quantas cada borda traz ao rolar até ela.
const PREVIEW_BEFORE: usize = 40;
const PREVIEW_AFTER: usize = 40;
const PREVIEW_PAGE: usize = 40;
/// Abaixo desta largura, lista e conversa não cabem lado a lado: a conversa abre no lugar da lista.
const NARROW: f32 = 860.;
const PALETTE_MAX: (f32, f32) = (1180., 880.);
const AGENTS: [&str; 5] = ["claude", "codex", "pi", "kimi", "omp"];

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Hit {
    project: String,
    session_id: String,
    session_name: Option<String>,
    cwd: Option<String>,
    line: String,
    mtime: f64,
    live: bool,
    role: Option<String>,
    event_id: Option<String>,
    ts: Option<f64>,
    /// Agente e conta Codex do trecho: o contexto lê pelo parser dele.
    provider: Option<String>,
    codex_account: Option<String>,
    /// Conta da conversa ("claude:<pasta>", "codex:<casa>") e a pasta de config que o Arquivo pede para lê-la.
    conta: Option<String>,
    config_dir: Option<String>,
    /// Todos os termos como palavra inteira; servidor antigo não manda, e a chegada calcula.
    whole: Option<bool>,
    /// Nome da sessão ao fechar e a primeira mensagem: o rótulo da conversa arquivada.
    name: Option<String>,
    preview: Option<String>,
    /// Máquina de onde o trecho veio (chave e rótulo), marcada ao chegar.
    #[serde(skip)]
    server: String,
    #[serde(skip)]
    label: String,
}

impl Hit {
    fn key(&self) -> String { format!("{}/{}/{}/{}", self.server, self.project, self.session_id, self.event_id.as_deref().unwrap_or(&self.line)) }
    fn conversation(&self) -> String { format!("{}/{}/{}", self.server, self.project, self.session_id) }
    fn folder(&self) -> String {
        self.cwd.as_deref().map(super::costs::project_label).unwrap_or_else(|| self.project.clone())
    }
    /// A sessão viva abre; a morta não tem nome de sessão.
    fn live_name(&self) -> Option<&str> { self.session_name.as_deref().filter(|_| self.live) }
    fn agent(&self) -> &str { self.provider.as_deref().filter(|p| !p.is_empty()).unwrap_or("claude") }
    fn is_whole(&self) -> bool { self.whole != Some(false) }
    /// O nome da conversa na lista: o da sessão viva, o que ela tinha ao fechar, o começo da primeira mensagem ou, de
    /// servidor antigo, a pasta com o começo do próprio trecho (só a pasta deixava várias conversas com o mesmo nome).
    fn title(&self) -> String {
        self.names().into_iter().next().unwrap_or_else(|| title_from_line(&self.folder(), &self.line))
    }
    /// Os nomes que a conversa tem, sem a pasta: é por eles que a resposta do Perguntar a cita.
    fn names(&self) -> Vec<String> {
        let name = self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
        let start = self.preview.as_deref().map(first_words).filter(|s| !s.is_empty());
        self.live_name().map(str::to_owned).into_iter().chain(name).chain(start).collect()
    }
    /// A pasta com a casa como `~`.
    fn place(&self) -> String { self.cwd.as_deref().map(home_short).unwrap_or_else(|| self.project.clone()) }
    /// Quando a mensagem foi escrita; servidor antigo só manda a data da conversa.
    fn when(&self) -> f64 { self.ts.filter(|t| *t > 0.).unwrap_or(self.mtime) }
    /// Identidade da conversa da prévia: a viva pelo nome da sessão, a arquivada pelo arquivo.
    fn chat_key(&self) -> String {
        match self.live_name() { Some(name) => format!("live:{}:{name}", self.server), None => format!("archive:{}", self.conversation()) }
    }
}

/// A sessão viva da lista como um trecho sem mensagem: a prévia e o cabeçalho a tratam como uma conversa da busca.
fn session_hit(target: &Target, session: &SessionInfo) -> Hit {
    Hit { session_name: Some(session.name.clone()), live: true, cwd: session.cwd.clone(), provider: Some(session.provider.clone()),
        conta: session.conta.clone(), mtime: session.last_activity.unwrap_or(0.), server: target.server.clone(), ..Default::default() }
}

/// O começo da primeira mensagem como nome: sem endereço, sem o "[de: …]" do recado de outra sessão, cortado numa palavra.
fn first_words(text: &str) -> String {
    let text = super::quotes::summary(text);
    let mut words: Vec<&str> = text.split_whitespace().filter(|w| !w.starts_with("http://") && !w.starts_with("https://")).collect();
    if words.first().is_some_and(|w| w.starts_with("[de:")) {
        let end = words.iter().position(|w| w.ends_with(']')).map_or(words.len(), |i| i + 1);
        words.drain(..end);
    }
    let line = words.join(" ");
    if line.chars().count() <= LABEL_MAX { return line; }
    let cut: String = line.chars().take(LABEL_MAX).collect();
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head))
}

/// "pasta · começo do trecho", sem as reticências do corte que o servidor põe no trecho.
fn title_from_line(folder: &str, line: &str) -> String {
    let start = first_words(plain(line).trim().trim_matches('…'));
    if start.is_empty() { folder.to_owned() } else { format!("{folder} · {start}") }
}

/// Onde a paleta fica na janela: centralizada, com margem, do tamanho que couber; `true` quando estreita demais para lista
/// e conversa lado a lado.
fn palette_frame(window: gpui_kit::Size<Pixels>) -> (Bounds<Pixels>, bool) {
    let margin = if window.width < px(700.) || window.height < px(560.) { px(8.) } else { px(24.) };
    let width = (window.width - margin * 2.).min(px(PALETTE_MAX.0)).max(px(0.));
    let height = (window.height - margin * 2.).min(px(PALETTE_MAX.1)).max(px(0.));
    let origin = point((window.width - width) / 2., (window.height - height) / 2.);
    (Bounds::new(origin, size(width, height)), width < px(NARROW))
}

/// Os eventos que a prévia desenha: `before` antes e `after` depois do trecho (sem trecho, o fim da conversa).
fn window_around(len: usize, focus: Option<usize>, before: usize, after: usize) -> Range<usize> {
    match focus {
        Some(at) => at.saturating_sub(before)..(at + after + 1).min(len),
        None => len.saturating_sub(before + after)..len,
    }
}

/// `/home/<usuário>/x` como `~/x`, como a lista de sessões mostra.
fn home_short(path: &str) -> String {
    for root in ["/home/", "/Users/"] {
        if let Some(rest) = path.strip_prefix(root) {
            return match rest.split_once('/') { Some((_, tail)) => format!("~/{tail}"), None => "~".into() };
        }
    }
    path.to_owned()
}

/// Backend antigo em outra máquina ainda repete o trecho quando o transcript repete a mensagem: a chave repetida daria
/// dois elementos com o mesmo id.
fn without_repeats(hits: Vec<Hit>) -> Vec<Hit> {
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().filter(|hit| seen.insert(hit.key())).collect()
}

/// Junta a resposta de mais uma máquina: sem repetidos, palavra inteira primeiro (como cada máquina já manda), a mais
/// recente primeiro dentro de cada parte.
fn merge_hits(hits: &mut Vec<Hit>, more: Vec<Hit>) {
    let mut all = std::mem::take(hits);
    all.extend(more);
    *hits = without_repeats(all);
    hits.sort_by(|a, b| b.is_whole().cmp(&a.is_whole()).then(b.mtime.total_cmp(&a.mtime)));
}

/// Junta como `merge_hits` e devolve onde foi parar o trecho `active`: a reordenação não pode trocar o destacado (e o que
/// o Enter abre) por outro.
fn merge_keeping(hits: &mut Vec<Hit>, more: Vec<Hit>, active: Option<usize>) -> Option<usize> {
    let key = active.and_then(|i| hits.get(i)).map(Hit::key);
    merge_hits(hits, more);
    key.and_then(|key| hits.iter().position(|hit| hit.key() == key))
}

/// O que os botões de filtro deixam na lista: um agente, só as abertas ou só as arquivadas.
#[derive(Clone, Debug, Default, PartialEq)]
struct Filter { agent: Option<String>, live: Option<bool> }

impl Filter {
    fn keeps(&self, hit: &Hit) -> bool {
        self.agent.as_deref().is_none_or(|agent| agent == hit.agent()) && self.live.is_none_or(|live| live == hit.live)
    }
}

/// Os trechos por conversa, na ordem em que chegaram, só os que o filtro deixa e até `cap` por conversa.
fn groups(hits: &[Hit], filter: &Filter, cap: usize) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, hit) in hits.iter().enumerate().filter(|(_, hit)| filter.keeps(hit)) {
        let key = hit.conversation();
        match out.iter_mut().find(|g| g.0 == key) {
            Some(group) => if group.1.len() < cap { group.1.push(i) },
            None => out.push((key, vec![i])),
        }
    }
    out
}

/// A resposta do Perguntar cita a conversa: um nome dela aparece no texto.
fn cites(answer: &str, hit: &Hit) -> bool {
    let answer = answer.to_lowercase();
    hit.names().iter().map(|n| n.trim_end_matches('…').trim()).filter(|n| n.chars().count() >= 3)
        .any(|n| answer.contains(&n.to_lowercase()))
}

/// A primeira entrada da conversa seguinte (o Tab); da última, volta ao começo.
fn next_group(keys: &[String], active: usize) -> usize {
    let current = keys.get(active);
    (active + 1..keys.len()).find(|&i| Some(&keys[i]) != current).unwrap_or(0)
}

/// Máquina da busca de conteúdo. `offline`: a lista dela já estava fora do ar quando a busca começou.
#[derive(Clone, Debug)]
struct Machine { key: String, label: String, offline: bool }

/// O que as setas percorrem: sessões vivas pelo nome e trechos da busca (ou da resposta), na ordem da tela.
#[derive(Clone, Debug, PartialEq)]
enum Entry { Session(Target), Hit(usize), Answer(usize) }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode { #[default] Hits, Ask }

/// A conversa da prévia: o pacote de conversa do chat (o mesmo da prévia da aba), lido uma vez pelo histórico.
pub(super) struct SearchChat {
    /// O trecho escolhido (ou a sessão, como trecho sem mensagem): o cabeçalho e as ações saem dele.
    hit: Hit,
    key: String,
    /// A mensagem do trecho e a linha da conversa onde ela está.
    focus: Option<String>,
    focus_row: Option<usize>,
    seq: u64,
    /// `None` só enquanto o pacote está dentro do `Hangar`.
    slot: Option<ConversationSlot>,
    loaded: bool,
    error: Option<String>,
    /// A conversa inteira e o pedaço dela que a prévia desenha: desenhar tudo deixava a rolagem pesada.
    events: Vec<ChatEvent>,
    shown: Range<usize>,
    /// Chegar a uma borda traz mais mensagens uma vez; só sair de perto dela arma de novo.
    top_armed: bool,
    bottom_armed: bool,
}

/// Linha da lista da esquerda: o modelo do que ela desenha, para a lista virtual pedir só as linhas à vista.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    ResumeError(String),
    Section(&'static str),
    Session(Target),
    Status(String, bool),
    Retry,
    Group(String, Vec<usize>),
}

#[derive(Default)]
pub(super) struct Search {
    pub(super) open: bool,
    input: Option<Entity<InputState>>,
    mode: Mode,
    /// A busca que está na tela (ou em voo).
    query: String,
    /// Trechos das máquinas que já responderam, entrando conforme chegam.
    hits: Vec<Hit>,
    /// O total de conversas de cada máquina que respondeu (`None`: ela não disse).
    totals: Vec<Option<u64>>,
    filter: Filter,
    /// Nenhuma máquina respondeu ainda: é o "Buscando…".
    searching: bool,
    pending: Vec<Machine>,
    /// Máquinas que falharam, com o motivo: as outras mostram o que acharam, e "Tentar de novo" refaz só estas.
    failed: Vec<(Machine, String)>,
    /// Falha da busca inteira (nenhuma máquina para buscar).
    error: Option<String>,
    /// Número da busca: toda tecla, reconexão e fechamento o trocam, e a resposta de número antigo é descartada.
    debounce: u64,
    /// Buscas por máquina em voo.
    tasks: Vec<tokio::task::AbortHandle>,
    active: usize,
    /// A seta mexeu na escolha: o próximo desenho rola a lista até ela.
    scroll_pending: bool,
    chat: Option<SearchChat>,
    /// Número da prévia: só cresce, para a resposta de uma prévia trocada nunca entrar na nova.
    chat_seq: u64,
    /// "Ver inteira": a prévia ocupa a paleta toda. Na paleta estreita, é a prévia aberta no lugar da lista.
    wide: bool,
    /// A janela não cabe lista e conversa lado a lado (do último desenho).
    narrow: bool,
    /// A lista da esquerda: as linhas e a lista virtual, com a escolha e as palavras do último desenho.
    rows: Vec<Row>,
    list: Option<ListState>,
    drawn_active: Option<Entry>,
    drawn_words: Vec<String>,
    drawn_resuming: Option<String>,
    /// A pergunta da resposta na tela: Enter com o mesmo texto abre a conversa escolhida, com outro pergunta de novo.
    asked: String,
    ask: Remote<(String, Vec<Hit>)>,
    /// O Perguntar em voo: fechar a paleta o interrompe.
    ask_task: Option<tokio::task::AbortHandle>,
    /// Retomada em curso (a conversa) e o erro da última.
    resuming: Option<String>,
    resume_error: Option<String>,
    /// A pasta da conversa sumiu: o seletor por cima da paleta.
    resume_folder: Option<(Entity<ResumeFolder>, Subscription)>,
    /// Sobe ao fechar a paleta: a resposta de uma retomada abandonada não abre seletor na abertura seguinte.
    resume_turn: u64,
    previous_focus: Option<FocusHandle>,
    ask_scroll: ScrollHandle,
    _events: Option<Subscription>,
}

impl Search {
    /// Sem isto, a busca de um texto já trocado (ou da paleta fechada) seguia até o prazo da máquina.
    fn abort_machines(&mut self) {
        for task in self.tasks.drain(..) { task.abort(); }
    }

    /// Cada abertura começa vazia, mas os números da busca e da prévia só crescem: zerados, a resposta de uma abertura
    /// anterior ainda em voo casaria com a nova.
    fn reopen(&mut self, previous_focus: Option<FocusHandle>) {
        self.abort_machines();
        self.abort_ask();
        let mut ask = std::mem::take(&mut self.ask);
        ask.reset();
        *self = Search { open: true, input: self.input.take(), previous_focus, _events: self._events.take(), debounce: self.debounce + 1,
            chat_seq: self.chat_seq, ask, ..Default::default() };
    }

    fn abort_ask(&mut self) {
        if let Some(task) = self.ask_task.take() { task.abort(); }
    }

    /// "Tentar de novo" da prévia que falhou: ela volta a carregar; `false` sem falha para refazer.
    fn retry_chat(&mut self) -> bool {
        let Some(chat) = self.chat.as_mut().filter(|c| c.error.is_some()) else { return false };
        self.chat_seq += 1;
        (chat.seq, chat.error, chat.loaded) = (self.chat_seq, None, false);
        true
    }

    /// Onde a busca procurou, quando toda máquina respondeu e disse; enquanto alguma falta ou falhou, não há total.
    fn total(&self) -> Option<u64> {
        if self.totals.is_empty() || !self.pending.is_empty() || !self.failed.is_empty() { return None; }
        self.totals.iter().copied().sum()
    }
}

/// O motivo da máquina que não respondeu à busca.
fn machine_reason(e: &Failure) -> String {
    let Some(status) = e.status else { return Hangar::failure(e) };
    let http = web_with("busca_motivo_http", &[("status", status.to_string())]);
    // O servidor diz o porquê (contas ilegíveis, lista fora); sem corpo, `detail` é só o "HTTP n" de novo.
    let said = e.detail.trim();
    if said.is_empty() || said == format!("HTTP {status}") { http } else { format!("{http}: {said}") }
}

/// A resposta do Perguntar e os trechos que ela usou. Sem o texto da resposta é resposta inválida, nunca uma vazia.
fn parse_answer(v: &Value) -> Result<(String, Vec<Hit>), String> {
    let answer = v.get("answer").and_then(Value::as_str).ok_or_else(|| tr("invalid_response"))?.to_owned();
    let hits = serde_json::from_value::<Vec<Hit>>(v.get("hits").cloned().unwrap_or(json!([]))).map_err(|_| tr("invalid_response"))?;
    Ok((answer, hits))
}

/// Palavras da busca, minúsculas e sem repetição (`termos` do backend).
fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in query.to_lowercase().split_whitespace() { if !out.iter().any(|w| w == word) { out.push(word.to_owned()); } }
    out
}

/// Faixas de `text` com algum dos termos, sem diferenciar maiúsculas; `true` quando o termo está ali como palavra inteira,
/// `false` quando é pedaço de outra ("tour" em "estourava").
fn marks(text: &str, terms: &[String]) -> Vec<(Range<usize>, bool)> {
    // Caractere cuja minúscula muda de tamanho desalinharia as faixas (o total pode empatar: Ⱥ cresce, o K de Kelvin
    // encolhe) e cortaria um caractere ao meio: aí não destaca nada.
    if !text.chars().all(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() == c.len_utf8()) { return Vec::new(); }
    let lower = text.to_lowercase();
    let letter = |c: Option<char>| c.is_some_and(char::is_alphabetic);
    let mut ranges: Vec<(Range<usize>, bool)> = terms.iter().filter(|t| !t.is_empty()).flat_map(|t| lower.match_indices(t.as_str()).map(|(at, m)| {
        let end = at + m.len();
        (at..end, !letter(lower[..at].chars().next_back()) && !letter(lower[end..].chars().next()))
    }).collect::<Vec<_>>()).collect();
    ranges.sort_by_key(|(r, _)| r.start);
    let mut merged: Vec<(Range<usize>, bool)> = Vec::new();
    for (r, whole) in ranges {
        match merged.last_mut() {
            Some((last, was_whole)) if r.start <= last.end => { last.end = last.end.max(r.end); *was_whole |= whole; }
            _ => merged.push((r, whole)),
        }
    }
    merged
}

/// Todos os termos aparecem ao menos uma vez como palavra inteira.
fn all_whole(text: &str, terms: &[String]) -> bool {
    terms.iter().filter(|t| !t.is_empty()).all(|t| marks(text, std::slice::from_ref(t)).iter().any(|(_, whole)| *whole))
}

/// O trecho sem as marcas de markdown que ele traz do transcript (negrito, código, título).
fn plain(text: &str) -> String {
    text.lines().map(|line| {
        let line = line.trim_start();
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && line[hashes..].starts_with(' ') { line[hashes + 1..].to_owned() } else { line.to_owned() }
    }).collect::<Vec<_>>().join(" ").replace("**", "").replace('`', "")
}

pub(crate) fn highlighted(text: String, terms: &[String]) -> StyledText {
    let whole = HighlightStyle { color: Some(theme::text()), background_color: Some(theme::accent().alpha(0.28)), ..Default::default() };
    // Pedaço de palavra: outra cor e sublinhado, para não parecer o mesmo acerto.
    let part = HighlightStyle { color: Some(theme::text()), background_color: Some(theme::warning().alpha(0.2)),
        underline: Some(UnderlineStyle { thickness: px(1.), color: Some(theme::warning()), wavy: false }), ..Default::default() };
    let ranges = marks(&text, terms);
    StyledText::new(text).with_highlights(ranges.into_iter().map(|(r, w)| (r, if w { whole } else { part })))
}

/// "hoje 14:02", "ontem 11:24", "30/10 19:34"; de outro ano, com o ano.
fn moment(at: chrono::NaiveDateTime, now: chrono::NaiveDateTime, today: &str, yesterday: &str, date_format: &str) -> String {
    use chrono::Datelike;
    let time = at.format("%H:%M");
    if at.date() == now.date() { return format!("{today} {time}"); }
    if now.date().pred_opt() == Some(at.date()) { return format!("{yesterday} {time}"); }
    if at.year() == now.year() { format!("{} {time}", at.format(date_format)) } else { format!("{}/{} {time}", at.format(date_format), at.format("%y")) }
}

fn when(ts: f64) -> String {
    let Some(at) = chrono::DateTime::from_timestamp(ts as i64, 0) else { return String::new() };
    let at = at.with_timezone(&chrono::Local).naive_local();
    moment(at, chrono::Local::now().naive_local(), &tr("search_today"), &tr("search_yesterday"), &tr("search_date_format"))
}

fn ago(ts: f64) -> String { side::ago(chrono::Local::now().timestamp() as f64 - ts) }

impl Hangar {
    /// Abre a busca de conversas (Ctrl+K e, depois, o botão da barra do topo); aberta, fecha.
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.open { return self.close_search(window, cx); }
        self.open_search(window, cx);
    }

    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog || self.api.is_none() { return; }
        self.close_popups();
        let input = self.search.input.get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder(web_with("switcher_buscar_conversas", &[])))).clone();
        if self.search._events.is_none() {
            self.search._events = Some(cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.search_changed(cx),
                InputEvent::PressEnter { .. } => this.search_activate(None, window, cx),
                _ => {}
            }));
        }
        // Cada abertura começa vazia, como o web, e na aba dos trechos.
        input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(web_with("switcher_buscar_conversas", &[]), window, cx);
        });
        let previous = window.focused(cx);
        self.search.reopen(previous);
        input.update(cx, |input, cx| input.focus(window, cx));
        // Aqui, não em quem chama: a busca aberta pelo grupo de ocultas também conclui o passo.
        if self.search.open { self.tour_event(crate::tour::TourEvent::SearchOpened, cx); }
        cx.notify();
    }

    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search.open { return; }
        self.search.open = false;
        self.search.debounce += 1;
        self.search.abort_machines();
        self.search.abort_ask();
        // A resposta da pergunta interrompida não entra em nada: nem nesta paleta, nem na próxima.
        self.search.ask.reset();
        self.search.chat = None;
        self.search.resume_folder = None;
        self.search.resume_turn += 1;
        match self.search.previous_focus.take() { Some(focus) => focus.focus(window, cx), None => self.root_focus.focus(window, cx) }
        cx.notify();
    }

    /// Outra conexão: o que a busca mostrava era do servidor anterior.
    pub(super) fn search_reconnected(&mut self, cx: &mut Context<Self>) {
        if self.search.open { self.search_changed(cx); }
    }

    fn search_text(&self, cx: &App) -> String { self.search.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default() }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        // Escrever a pergunta não busca trechos: a resposta só sai no Enter.
        if self.search.mode == Mode::Ask { cx.notify(); return; }
        let query = self.search_text(cx).trim().to_owned();
        (self.search.active, self.search.resume_error, self.search.filter) = (0, None, Filter::default());
        if let Some(list) = &self.search.list { list.scroll_to(ListOffset::default()); }
        self.search.debounce += 1;
        self.search.abort_machines();
        let seq = self.search.debounce;
        if query.is_empty() {
            (self.search.query, self.search.hits, self.search.totals, self.search.searching) = (String::new(), Vec::new(), Vec::new(), false);
            (self.search.pending, self.search.failed, self.search.error) = (Vec::new(), Vec::new(), None);
            cx.notify();
            return;
        }
        self.search.searching = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| if this.search.debounce == seq { this.run_search(query, cx) });
        }).detach();
        cx.notify();
    }

    /// Trechos ou Perguntar; o texto do campo fica, e a aba dos trechos busca o que estiver nele.
    fn search_set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.mode == mode { return; }
        (self.search.mode, self.search.active, self.search.wide) = (mode, 0, false);
        let placeholder = web_with(if mode == Mode::Ask { "mobile_busca_perguntar" } else { "switcher_buscar_conversas" }, &[]);
        if let Some(input) = self.search.input.clone() {
            input.update(cx, |input, cx| { input.set_placeholder(placeholder, window, cx); input.focus(window, cx) });
        }
        if mode == Mode::Hits && self.search.query != self.search_text(cx).trim() { self.search_changed(cx); }
        cx.notify();
    }

    /// Máquinas da busca de conteúdo: as próprias ligadas, a ativa primeiro. Convite fica de fora: a busca é rota do servidor
    /// inteiro do dono.
    /// As que já estavam fora do ar vão por último, marcadas.
    fn search_machines(&self, cx: &App) -> Vec<Machine> {
        let active = self.active_key();
        let machine = |key: String, label: String| {
            let offline = key != active && self.remote.get(&key).is_some_and(|l| l.error.is_some());
            Machine { key, label, offline }
        };
        let mut list: Vec<Machine> = self.servers.iter().filter(|s| !s.disabled && !s.invite)
            .map(|s| machine(servers::norm(&s.address), s.label.clone())).collect();
        if let Some(ix) = list.iter().position(|m| m.key == active) { let first = list.remove(ix); list.insert(0, first); }
        // A ativa ainda fora da lista gravada (a conexão grava depois da primeira resposta) entra do mesmo jeito.
        else if !active.is_empty() && !self.active_invite() { list.insert(0, machine(active.clone(), self.server_label(cx))); }
        list.sort_by_key(|m| m.offline);
        list
    }

    /// Rótulo da máquina `key` para a busca.
    pub(super) fn machine_label(&self, key: &str, cx: &App) -> String {
        if self.is_active_key(key) { return self.server_label(cx); }
        self.server_entry(key).map_or_else(|| key.to_owned(), |s| s.label.clone())
    }

    fn run_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.search.query = query;
        (self.search.hits, self.search.totals, self.search.pending, self.search.failed, self.search.error) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None);
        let machines = self.search_machines(cx);
        // Nenhuma máquina para buscar não é "nada encontrado".
        if machines.is_empty() {
            (self.search.error, self.search.searching) = (Some(tr("search_no_machines")), false);
            cx.notify();
            return;
        }
        for machine in machines { self.search_machine(machine, cx); }
        cx.notify();
    }

    /// Uma busca por máquina, como o fan-out do web: cada resposta entra na lista assim que chega, e a lenta ou fora do ar
    /// falha sozinha, sem segurar as outras.
    fn search_machine(&mut self, machine: Machine, cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&machine.key) else {
            let reason = self.machine_error(&machine.key);
            self.search.failed.push((machine, reason));
            self.search.searching = false;
            return;
        };
        let (seq, query) = (self.search.debounce, self.search.query.clone());
        self.search.pending.push(machine.clone());
        let task = {
            let query = query.clone();
            // O prazo de fora é o que vale: só ele sabe dizer "tempo esgotado"; o da requisição fica de reserva.
            self.runtime.spawn(async move {
                tokio::time::timeout(MACHINE_WAIT, api.server_read_header(&["search"], &[("q", query.as_str())], MACHINE_WAIT.as_secs() + 2, TOTAL_HEADER)).await
            })
        };
        self.search.tasks.push(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                // Resposta velha: outra tecla, reconexão ou a paleta fechou.
                if this.search.debounce != seq || this.search.query != query { return; }
                this.search.pending.retain(|m| m.key != machine.key);
                this.search.searching = false;
                // A tela só mostra o rótulo do motivo; a causa vai para o log.
                let result = match joined {
                    Err(error) => {
                        eprintln!("busca em {}: tarefa perdida: {error}", machine.label);
                        Err(tr("search_interrupted"))
                    }
                    Ok(Err(_)) => Err(web_with("busca_motivo_timeout", &[])),
                    Ok(Ok(result)) => result
                        .map_err(|e| machine_reason(&e))
                        .and_then(|(v, total)| serde_json::from_value::<Vec<Hit>>(v).map(|list| (list, total)).map_err(|error| {
                            eprintln!("busca em {}: resposta inválida: {error}", machine.label);
                            tr("invalid_response")
                        })),
                };
                match result {
                    Ok((list, total)) => {
                        this.search.totals.push(total.and_then(|t| t.trim().parse().ok()));
                        let words = terms(&query);
                        let list = list.into_iter().map(|mut hit| {
                            hit.whole.get_or_insert_with(|| all_whole(&plain(&hit.line), &words));
                            Hit { server: machine.key.clone(), label: machine.label.clone(), ..hit }
                        }).collect();
                        let active = match this.search_entries(cx).get(this.search.active) { Some(Entry::Hit(i)) => Some(*i), _ => None };
                        let moved = merge_keeping(&mut this.search.hits, list, active);
                        if active.is_some() {
                            this.search.active = moved.and_then(|i| this.search_entries(cx).iter().position(|e| *e == Entry::Hit(i))).unwrap_or(0);
                        }
                    }
                    Err(reason) => this.search.failed.push((machine, reason)),
                }
                cx.notify();
            });
        }).detach();
    }

    /// Refaz só as que falharam, na mesma busca: o que já chegou fica.
    fn search_retry(&mut self, cx: &mut Context<Self>) {
        // O botão só aparece com a lista do texto atual; isto cobre o clique que chega junto de uma tecla, cuja busca nova
        // já vem a caminho.
        if self.search.failed.is_empty() || self.search.query != self.search_text(cx).trim() { return; }
        for (machine, _) in std::mem::take(&mut self.search.failed) { self.search_machine(machine, cx); }
        cx.notify();
    }

    /// Sessões vivas de todas as máquinas cujo nome ou pasta tem a busca, as mais recentes primeiro.
    fn search_sessions(&self, query: &str) -> Vec<(Target, SessionInfo)> {
        let q = query.to_lowercase();
        let matches = |s: &SessionInfo| q.is_empty() || s.name.to_lowercase().contains(&q) || s.cwd.as_deref().is_some_and(|c| c.to_lowercase().contains(&q));
        let active = self.active_key();
        let mut list: Vec<(Target, SessionInfo)> = self.sessions.iter().filter(|s| !self.sidebar.is_hidden(&active, &s.name))
            .map(|s| (active.clone(), s))
            .chain(self.remote.iter().flat_map(|(key, l)| l.sessions.iter().filter(|s| !self.sidebar.is_hidden(key, &s.name)).map(move |s| (key.clone(), s))))
            .filter(|(_, s)| matches(s))
            .map(|(key, s)| (Target::new(&key, &s.name), s.clone())).collect();
        list.sort_by(|a, b| b.1.last_activity.unwrap_or(0.).total_cmp(&a.1.last_activity.unwrap_or(0.)));
        list.truncate(SESSIONS_MAX);
        list
    }

    /// As conversas da resposta: só as que ela cita.
    fn cited_groups(&self) -> Vec<(String, Vec<usize>)> {
        let Some((answer, hits)) = self.search.ask.ok() else { return Vec::new() };
        groups(hits, &Filter::default(), HITS_SHOWN).into_iter().filter(|(_, ix)| cites(answer, &hits[ix[0]])).collect()
    }

    fn search_entries(&self, cx: &App) -> Vec<Entry> {
        if self.search.mode == Mode::Ask {
            return self.cited_groups().into_iter().flat_map(|(_, ix)| ix.into_iter().map(Entry::Answer)).collect();
        }
        let query = self.search_text(cx).trim().to_owned();
        let mut out: Vec<Entry> = self.search_sessions(&query).into_iter().map(|(target, _)| Entry::Session(target)).collect();
        out.extend(groups(&self.search.hits, &self.search.filter, HITS_SHOWN).into_iter().flat_map(|(_, ix)| ix.into_iter().map(Entry::Hit)));
        out
    }

    /// A conversa da entrada, para o Tab pular para a próxima.
    fn entry_conversation(&self, entry: &Entry) -> String {
        match entry {
            Entry::Session(target) => format!("session:{}", target.id()),
            other => self.hit_of(other).map(|hit| hit.conversation()).unwrap_or_default(),
        }
    }

    fn search_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.search_entries(cx).len();
        if count == 0 { return; }
        self.search.active = (self.search.active as isize + delta).rem_euclid(count as isize) as usize;
        self.search.scroll_pending = true;
        cx.notify();
    }

    fn search_next_conversation(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.search_entries(cx).iter().map(|e| self.entry_conversation(e)).collect();
        if keys.is_empty() { return; }
        self.search.active = next_group(&keys, self.search.active);
        self.search.scroll_pending = true;
        cx.notify();
    }

    fn search_select(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if let Some(ix) = self.search_entries(cx).iter().position(|e| e == entry) {
            self.search.active = ix;
            // Na paleta estreita, escolher abre a conversa no lugar da lista; "Voltar à lista" volta.
            if self.search.narrow && self.search.mode == Mode::Hits { self.search.wide = true; }
            cx.notify();
        }
    }

    fn hit_of(&self, entry: &Entry) -> Option<Hit> {
        match entry {
            Entry::Hit(i) => self.search.hits.get(*i).cloned(),
            Entry::Answer(i) => self.search.ask.ok()?.1.get(*i).cloned(),
            Entry::Session(_) => None,
        }
    }

    fn preview_hit(&self, entry: &Entry) -> Option<Hit> {
        match entry {
            Entry::Session(target) => self.target_session(target).map(|s| session_hit(target, s)),
            other => self.hit_of(other),
        }
    }

    /// Enter: abre a sessão viva, retoma a arquivada. No Perguntar, texto novo pergunta de novo.
    fn search_activate(&mut self, entry: Option<Entry>, window: &mut Window, cx: &mut Context<Self>) {
        if entry.is_none() && self.search.mode == Mode::Ask
            && (self.search.ask.value.is_none() || self.search.asked != self.search_text(cx).trim()) {
            return self.search_ask(cx);
        }
        let Some(entry) = entry.or_else(|| self.search_entries(cx).get(self.search.active).cloned()) else { return };
        match entry {
            Entry::Session(target) => self.search_open_session(&target, window, cx),
            other => {
                let Some(hit) = self.hit_of(&other) else { return };
                self.search_open_hit(hit, window, cx);
            }
        }
    }

    fn search_open_hit(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        match hit.live_name() {
            Some(name) => { let target = Target::new(&hit.server, name); self.search_open_session(&target, window, cx) }
            None => self.search_resume(hit, None, window, cx),
        }
    }

    /// Abre na máquina dela, sem trocar o servidor ativo.
    fn search_open_session(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.target_session(target).is_none() {
            self.search.resume_error = Some(tr("search_session_gone").replace("{name}", &target.name));
            cx.notify();
            return;
        }
        self.search.previous_focus = None;
        self.close_search(window, cx);
        self.close_costs(window, cx);
        self.close_worktrees(window, cx);
        self.close_reviews(window, cx);
        self.close_mission(window, cx);
        self.close_settings(window, cx);
        self.open_target(target, window, cx);
    }

    /// Conversa arquivada: sobe uma sessão nova com `--resume` na conta dona dela, pela rota do Arquivo do web, na máquina
    /// de onde o trecho veio. `cwd`: a pasta escolhida depois que a da conversa sumiu.
    fn search_resume(&mut self, hit: Hit, cwd: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&hit.server) else {
            self.search.resume_error = Some(self.machine_error(&hit.server));
            cx.notify();
            return;
        };
        if self.search.resuming.is_some() { return; }
        let server = hit.server.clone();
        self.search.resuming = Some(hit.conversation());
        self.search.resume_error = None;
        let (gone_hit, gone_api, turn) = (hit.clone(), api.clone(), self.search.resume_turn);
        let body = cwd.map_or_else(|| json!({}), |cwd| json!({"cwd": cwd}));
        let task = self.runtime.spawn(async move {
            api.server_send(reqwest::Method::POST, &["archive", &hit.project, &hit.session_id, "resume"], Some(body), 120).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let joined = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Sem isto o botão ficava ocupado para sempre e nenhuma outra retomada saía. Trocar a conexão ativa não
                // descarta: a sessão já nasceu na máquina do trecho, e é nela que abre.
                this.search.resuming = None;
                let Ok(result) = joined else { this.search.resume_error = Some(tr("search_interrupted")); cx.notify(); return };
                if this.search.resume_turn == turn && let Err(failure) = &result && failure.code.as_deref() == Some(FOLDER_GONE) {
                    this.search_choose_folder(gone_hit, gone_api, Self::fetch_failure(failure), window, cx);
                    cx.notify();
                    return;
                }
                match result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| serde_json::from_value::<SessionInfo>(v).map_err(|_| tr("invalid_response"))) {
                    Ok(session) => {
                        this.search.previous_focus = None;
                        this.close_search(window, cx);
                        this.close_costs(window, cx);
                        this.close_worktrees(window, cx);
                        this.close_mission(window, cx);
                        this.close_settings(window, cx);
                        let readable = session.readable();
                        // Nasceu em outra máquina: entra na lista guardada dela, senão a leitura que chega primeiro a fecharia.
                        if let Some(list) = this.remote.get_mut(&server).filter(|l| l.loaded && !l.sessions.iter().any(|s| s.name == session.name)) {
                            list.sessions.push(session.clone());
                        }
                        if this.select_on(&server, session, window, cx) && readable { this.composer.update(cx, |input, cx| input.focus(window, cx)); }
                    }
                    Err(error) => this.search.resume_error = Some(error),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn search_choose_folder(&mut self, hit: Hit, api: Api, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let runtime = self.runtime.clone();
        let picker = cx.new(|cx| ResumeFolder::new(api, runtime, message, cx));
        let events = cx.subscribe_in(&picker, window, move |this: &mut Self, _, event: &ResumeFolderEvent, window, cx| {
            this.search.resume_folder = None;
            if let Some(input) = this.search.input.clone() { input.update(cx, |input, cx| input.focus(window, cx)); }
            if let ResumeFolderEvent::Chosen(cwd) = event { this.search_resume(hit.clone(), Some(cwd.clone()), window, cx); }
            cx.notify();
        });
        // Com o foco no campo, Enter retomaria de novo sem a pasta.
        let focus = picker.read(cx).focus.clone();
        focus.focus(window, cx);
        self.search.resume_folder = Some((picker, events));
    }

    /// "Perguntar": o backend acha os trechos e um modelo responde onde o assunto apareceu (só neste servidor, como o web).
    fn search_ask(&mut self, cx: &mut Context<Self>) {
        let question = self.search_text(cx).trim().to_owned();
        if question.is_empty() || self.search.ask.loading { return; }
        let Some(api) = self.api.clone() else { return };
        let seq = self.search.ask.start();
        (self.search.asked, self.search.active) = (question.clone(), 0);
        let connection = self.connection;
        let (server, label) = (self.active_key(), self.server_label(cx));
        let words = terms(&question);
        let task = self.runtime.spawn(async move {
            api.server_send(reqwest::Method::POST, &["ask-history"], Some(json!({ "question": question })), 90).await
        });
        self.search.ask_task = Some(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                // Mesmo caso da busca: sem resposta útil, o botão não pode ficar em "Perguntando…".
                let result = match joined {
                    Ok(result) if this.connection == connection => result.map_err(|e| Self::failure(&e)),
                    _ => Err(tr("search_interrupted")),
                };
                let parsed = result.and_then(|v| {
                    let (answer, hits) = parse_answer(&v)?;
                    let hits = hits.into_iter().map(|mut hit| {
                        hit.whole.get_or_insert_with(|| all_whole(&plain(&hit.line), &words));
                        Hit { server: server.clone(), label: label.clone(), ..hit }
                    }).collect();
                    Ok((answer, without_repeats(hits)))
                });
                this.search.ask.finish(seq, parsed);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    // ── Prévia da conversa ──────────────────────────────────────────────────

    /// Roda `f` com a conversa da prévia no lugar da principal e destroca antes de devolver, como a prévia da aba.
    fn with_search_slot<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Context<Self>) -> R) -> Option<R> {
        let mut slot = self.search.chat.as_mut()?.slot.take()?;
        self.leave_tab_preview_event();
        self.swap_conversation(&mut slot);
        let inside = std::mem::replace(&mut self.in_tab_preview, true);
        let result = f(self, cx);
        self.in_tab_preview = inside;
        self.swap_conversation(&mut slot);
        if let Some(chat) = self.search.chat.as_mut() { chat.slot = Some(slot); }
        Some(result)
    }

    /// Clique de linha que veio da prévia da busca (abrir ferramenta, pensamento): mexe na conversa dela.
    pub(super) fn toggle_in_search_preview(&mut self, key: String, cx: &mut Context<Self>) -> bool {
        let owned = self.search.chat.as_ref().and_then(|c| c.slot.as_ref()).is_some_and(|slot| slot.owns(&key));
        if self.in_tab_preview || !owned { return false; }
        self.with_search_slot(cx, |this, cx| this.toggle_here(key, cx));
        true
    }

    /// A prévia acompanha a escolha: conversa nova monta o pacote e lê o histórico depois da espera; a mesma conversa só
    /// rola até a mensagem nova.
    fn search_sync_chat(&mut self, hit: Option<Hit>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = hit else { self.search.chat = None; return };
        let key = hit.chat_key();
        if let Some(chat) = self.search.chat.as_mut().filter(|chat| chat.key == key) {
            if chat.focus != hit.event_id {
                chat.focus = hit.event_id.clone();
                chat.hit = hit;
                // Trecho fora do pedaço desenhado: o pedaço cresce até ele (só crescer: a conversa já na tela continua).
                let at = chat.focus.as_deref().and_then(|id| chat.events.iter().position(|e| e.id == id));
                let shown = chat.shown.clone();
                if let Some(at) = at.filter(|at| !shown.contains(at)) {
                    let around = window_around(chat.events.len(), Some(at), PREVIEW_BEFORE, PREVIEW_AFTER);
                    self.search_show(shown.start.min(around.start)..shown.end.max(around.end), cx);
                }
                self.search_scroll_to_focus(cx);
            }
            return;
        }
        let session = hit.live_name().and_then(|name| self.target_session(&Target::new(&hit.server, name)).cloned())
            .unwrap_or_else(|| SessionInfo { name: hit.title(), cwd: hit.cwd.clone(), provider: hit.agent().to_owned(), conta: hit.conta.clone(), ..Default::default() });
        // Como a principal: `open_api` só existe para sessão de outra máquina.
        let api = (!self.is_active_key(&hit.server)).then(|| self.machine_api(&hit.server)).flatten();
        let composer = cx.new(|cx| TextareaState::new(window, cx));
        self.search.chat_seq += 1;
        let seq = self.search.chat_seq;
        self.search.chat = Some(SearchChat { key, focus: hit.event_id.clone(), focus_row: None, seq,
            slot: Some(ConversationSlot::new(session, api, composer, window)), loaded: false, error: None, hit,
            events: Vec::new(), shown: 0..0, top_armed: false, bottom_armed: false });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let _ = this.update(cx, |this, cx| if this.search.chat.as_ref().is_some_and(|c| c.seq == seq) { this.search_fetch_chat(cx) });
        }).detach();
    }

    /// A conversa inteira: a viva pelo `/history` da sessão, a arquivada pelo histórico do Arquivo, na máquina do trecho.
    fn search_fetch_chat(&mut self, cx: &mut Context<Self>) {
        let Some((seq, hit)) = self.search.chat.as_ref().map(|c| (c.seq, c.hit.clone())) else { return };
        let Some(api) = self.machine_api(&hit.server) else {
            let error = self.machine_error(&hit.server);
            if let Some(chat) = self.search.chat.as_mut() { (chat.error, chat.loaded) = (Some(error), true); }
            cx.notify();
            return;
        };
        let live = hit.live_name().map(str::to_owned);
        // `limit` 0 é o histórico inteiro: o trecho pode estar longe do fim. Sem trecho (a sessão pelo nome), só a cauda.
        let limit = if hit.event_id.is_some() { 0 } else { SESSION_TAIL };
        let task = self.runtime.spawn(async move {
            match live {
                Some(name) => api.history(&name, limit, None).await.map(|h| h.events.unwrap_or_default()),
                None => {
                    let mut query: Vec<(&str, &str)> = Vec::new();
                    query.extend(hit.provider.as_deref().map(|p| ("provider", p)));
                    query.extend(hit.config_dir.as_deref().map(|d| ("config_dir", d)));
                    query.extend(hit.codex_account.as_deref().map(|a| ("codex_account", a)));
                    api.server_read(&["archive", &hit.project, &hit.session_id, "history"], &query, 30).await
                        .and_then(|v| serde_json::from_value::<Vec<ChatEvent>>(v).map_err(|_| Failure::local("invalid_response")))
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                let result = match joined { Ok(result) => result.map_err(|e| Self::fetch_failure(&e)), Err(_) => Err(tr("search_interrupted")) };
                let Some(chat) = this.search.chat.as_mut().filter(|c| c.seq == seq) else { return };
                chat.loaded = true;
                match result {
                    Err(error) => chat.error = Some(error),
                    Ok(events) => {
                        let at = chat.focus.as_deref().and_then(|id| events.iter().position(|e| e.id == id));
                        let shown = window_around(events.len(), at, PREVIEW_BEFORE, PREVIEW_AFTER);
                        chat.events = events;
                        this.search_show(shown, cx);
                        this.search_scroll_to_focus(cx);
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    /// Desenha o pedaço `shown` da conversa da prévia. Só cresce: o histórico se junta ao que já está desenhado.
    fn search_show(&mut self, shown: Range<usize>, cx: &mut Context<Self>) {
        let Some(chat) = self.search.chat.as_mut() else { return };
        let slice = chat.events[shown.clone()].to_vec();
        chat.shown = shown;
        self.with_search_slot(cx, |this, cx| {
            this.chat.merge_history(slice);
            (this.loading, this.history_installed) = (false, true);
            this.sync_tab_preview_rows(cx);
        });
    }

    /// A rolagem chegou a uma borda do pedaço desenhado: traz mais `PREVIEW_PAGE` mensagens daquele lado, com a linha do
    /// topo parada no lugar quando elas entram por cima.
    fn search_extend(&mut self, top: bool, cx: &mut Context<Self>) {
        let Some(chat) = self.search.chat.as_ref() else { return };
        let (shown, len) = (chat.shown.clone(), chat.events.len());
        let more = if top { shown.start.saturating_sub(PREVIEW_PAGE)..shown.end } else { shown.start..(shown.end + PREVIEW_PAGE).min(len) };
        if more == shown { return; }
        let anchor = self.with_search_slot(cx, |this, _| {
            let top = this.list_state.logical_scroll_top();
            this.row_ids.get(top.item_ix).cloned().map(|id| (id, top.offset_in_item))
        }).flatten();
        self.search_show(more, cx);
        if top && let Some((id, offset)) = anchor {
            self.with_search_slot(cx, |this, _| {
                if let Some(ix) = this.row_ids.iter().position(|row| *row == id) {
                    this.list_state.scroll_to(ListOffset { item_ix: ix, offset_in_item: offset });
                }
            });
        }
        self.search_focus_row(false, cx);
    }

    /// Rola a prévia até a linha da mensagem do trecho; sem mensagem (a sessão pelo nome), fica no fim.
    fn search_scroll_to_focus(&mut self, cx: &mut Context<Self>) { self.search_focus_row(true, cx) }

    /// Acha a linha do trecho (as linhas mudam quando o pedaço desenhado cresce) e, com `scroll`, rola até ela.
    fn search_focus_row(&mut self, scroll: bool, cx: &mut Context<Self>) {
        let Some(chat) = self.search.chat.as_ref().filter(|c| c.loaded && c.error.is_none()) else { return };
        let (focus, old) = (chat.focus.clone(), chat.focus_row);
        let row = self.with_search_slot(cx, |this, _| {
            let row = focus.as_deref().and_then(|id| this.row_of(id));
            // A linha do trecho ganha a etiqueta e o contorno: a altura dela e a da anterior mudam.
            for r in [old, row].into_iter().flatten() { this.list_state.remeasure_items(r..r + 1); }
            if scroll && let Some(r) = row { this.list_state.scroll_to(ListOffset { item_ix: r, offset_in_item: px(0.) }); }
            row
        }).flatten();
        if let Some(chat) = self.search.chat.as_mut() { chat.focus_row = row; }
        cx.notify();
    }

    // ── Desenho ─────────────────────────────────────────────────────────────

    /// Selo da conta: o nome que as etiquetas da máquina dão, senão o sufixo da pasta, como a lista de sessões.
    fn search_account(&self, hit: &Hit) -> Option<(String, Hsla)> {
        let conta = hit.conta.as_deref()?;
        let chip = account_chip(Some(conta));
        match self.tags.get(&hit.server).and_then(|t| t.accounts.get(conta)) {
            Some(badge) => Some((badge.name.clone(), badge.color.as_deref().and_then(theme::tag_color)
                .or_else(|| chip.map(|(_, color)| color)).unwrap_or_else(theme::muted))),
            None => chip,
        }
    }

    /// O avatar da lista de sessões; a viva com o anel de aberta agora.
    fn search_avatar(&self, hit: &Hit, side: f32) -> Div {
        let look = match hit.live_name().and_then(|name| self.target_session(&Target::new(&hit.server, name))) {
            Some(session) => sidebar::session_look(session).ring(theme::success()),
            None => chrome::AvatarLook::new(hit.agent(), None, "", false),
        };
        chrome::session_avatar(&look, side, None)
    }

    pub(super) fn render_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search.open { return None; }
        let input = self.search.input.clone()?;
        let mode = self.search.mode;
        let entries = self.search_entries(cx);
        self.search.active = self.search.active.min(entries.len().saturating_sub(1));
        let active = entries.get(self.search.active).cloned();
        if mode == Mode::Hits {
            let hit = active.as_ref().and_then(|e| self.preview_hit(e));
            self.search_sync_chat(hit, window, cx);
        }
        let (frame, narrow) = palette_frame(window.viewport_size());
        self.search.narrow = narrow;

        let tabs = settings::segments("search-mode", &[tr("search_tab_hits"), tr("search_tab_ask")], if mode == Mode::Ask { 1 } else { 0 }, 2,
            false, String::new(), |this, n, window, cx| this.search_set_mode(if n == 0 { Mode::Hits } else { Mode::Ask }, window, cx), cx);
        let field = search_field(div().flex_1().min_w_0()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| { this.search_move(-1, cx); cx.stop_propagation(); }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| { this.search_move(1, cx); cx.stop_propagation(); }))
            .capture_action(cx.listener(|this, _: &IndentInline, _, cx| { this.search_next_conversation(cx); cx.stop_propagation(); }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| { this.close_search(window, cx); cx.stop_propagation(); }))
            .child(Input::new(&input).appearance(false).aria_label(web_with("lista_buscar", &[]))));
        let header = div().flex_shrink_0().px(px(16.)).pt(px(14.)).pb(px(8.)).flex().items_center().gap(px(10.)).child(field).child(tabs);
        let body = match mode {
            Mode::Hits => self.render_search_hits(active.as_ref(), cx),
            Mode::Ask => self.render_search_ask(active.as_ref(), cx),
        };
        let footer = div().h(px(34.)).flex_shrink_0().px(px(16.)).flex().items_center().border_t_1().border_color(theme::border())
            .text_size(px(11.5)).text_color(theme::faint()).child(tr("search_shortcuts"));
        let folder = self.search.resume_folder.as_ref().map(|(picker, _)| div().absolute().inset_0().p_4().flex().items_center().justify_center()
            .bg(cx.theme().overlay).occlude().child(div().w_full().max_w(px(520.)).child(picker.clone())));
        let card = div().id("search-palette").relative().w(frame.size.width).h(frame.size.height).flex().flex_col().rounded(px(16.)).border_1()
            .border_color(theme::glass_border()).bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow()).overflow_hidden().occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(self.render_search_scope(cx))
            .child(div().flex_1().min_h_0().flex().border_t_1().border_color(theme::border()).child(body))
            .child(footer)
            .children(folder);
        let card = if appearance::get().surface_material == appearance::SurfaceMaterial::Glass { chrome::Glass::new(card, px(16.)).into_any_element() }
            else { card.into_any_element() };
        // Sem tecla, o agente fecha pela acessibilidade: um "Fechar" sem tamanho, fora do desenho.
        let close_search = cx.listener(|this, _: &(), window, cx| this.close_search(window, cx));
        let close = div().id("search-close").absolute().size_0().role(Role::Button).aria_label(tr("close"))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| close_search(&(), window, cx));
        Some(deferred(div().id("search-overlay").role(Role::Dialog).aria_label(web_with("lista_buscar", &[]))
            .absolute().inset_0().bg(cx.theme().overlay).occlude()
            .on_any_mouse_down(cx.listener(|this, _, window, cx| { this.close_search(window, cx); cx.stop_propagation(); }))
            // Posição na janela, como a prévia da aba: centralizar pelo fluxo do pai deixava a paleta sair pela esquerda e por cima.
            .child(close).child(anchored().position_mode(AnchoredPositionMode::Window).position(frame.origin).child(card)))
            .with_priority(gpui_kit::base::POPUP_PRIORITY + 1).into_any_element())
    }

    /// A linha de baixo da busca: onde ela procurou, quanto achou, e os filtros por agente e por estado.
    fn render_search_scope(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.search.mode == Mode::Ask {
            return scope_line().child(div().truncate().child(tr("search_ask_hint"))).into_any_element();
        }
        let scope = match self.search.total() {
            Some(n) => tr("search_scope_total").replace("{n}", &n.to_string()),
            None => tr("search_scope_all"),
        };
        let hits = &self.search.hits;
        let all = groups(hits, &Filter::default(), usize::MAX);
        let found = (!hits.is_empty()).then(|| ScopeFound { hits: hits.len(), conversations: all.len(),
            many_terms: terms(&self.search.query).len() > 1,
            partial: all.iter().filter(|(_, ix)| !ix.iter().any(|i| hits[*i].is_whole())).count() });
        let line = search_scope_summary(scope, found);
        if hits.is_empty() { return line.into_any_element(); }
        // Filtros: cada agente que aparece, e aberta/arquivada.
        let filter = self.search.filter.clone();
        let chip = |id: String, on: bool, label: AnyElement, count: usize, pick: Filter| {
            search_filter_chip(id, on, label, count)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.search.filter = if this.search.filter == pick { Filter::default() } else { pick.clone() };
                    this.search.active = 0;
                    cx.notify();
                }))
        };
        let mut chips: Vec<AnyElement> = Vec::new();
        for agent in AGENTS {
            let count = hits.iter().filter(|h| h.agent() == agent).count();
            if count == 0 { continue; }
            let pick = Filter { agent: Some(agent.to_owned()), live: None };
            chips.push(chip(format!("search-filter-{agent}"), filter == pick, search_agent_label(agent), count, pick).into_any_element());
        }
        for live in [true, false] {
            let count = hits.iter().filter(|h| h.live == live).count();
            if count == 0 { continue; }
            let pick = Filter { agent: None, live: Some(live) };
            chips.push(chip(format!("search-filter-live-{live}"), filter == pick, search_live_label(live), count, pick).into_any_element());
        }
        search_scope_block(line, chips).into_any_element()
    }

    /// As linhas da lista da esquerda, na ordem da tela.
    fn search_rows(&self, query: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(error) = self.search.resume_error.clone() { rows.push(Row::ResumeError(error)); }
        let sessions = self.search_sessions(query);
        if !sessions.is_empty() {
            rows.push(Row::Section("search_sessions"));
            rows.extend(sessions.into_iter().map(|(target, _)| Row::Session(target)));
            if !query.is_empty() { rows.push(Row::Section("search_conversations")); }
        }
        if query.is_empty() {
            rows.push(Row::Status(web_with("busca_digite_todas", &[]), false));
        } else if self.search.searching || self.search.query != query {
            rows.push(Row::Status(web_with("switcher_buscando", &[]), false));
        } else if let Some(error) = self.search.error.clone() {
            rows.push(Row::Status(error, true));
        } else {
            // A máquina que falhou aparece mesmo com resultados das outras, como o web.
            for (machine, reason) in &self.search.failed {
                rows.push(Row::Status(web_with("busca_servidor_falhou_motivo", &[("servidor", machine.label.clone()), ("motivo", reason.clone())]), true));
            }
            if !self.search.failed.is_empty() { rows.push(Row::Retry); }
            let waiting = |offline: bool, key: &str| {
                let names: Vec<String> = self.search.pending.iter().filter(|m| m.offline == offline).map(|m| m.label.clone()).collect();
                (!names.is_empty()).then(|| web_with(key, &[("servidores", names.join(", "))]))
            };
            let lead = (self.search.hits.is_empty() && self.search.failed.is_empty() && self.search.pending.is_empty())
                .then(|| web_with("busca_nenhum_todas", &[("termos", terms(query).join(", "))]));
            let status: Vec<String> = [lead, waiting(false, "busca_aguardando"), waiting(true, "busca_tentando_fora")].into_iter().flatten().collect();
            if !status.is_empty() { rows.push(Row::Status(status.join(" · "), false)); }
            rows.extend(groups(&self.search.hits, &self.search.filter, HITS_SHOWN).into_iter().map(|(key, ix)| Row::Group(key, ix)));
        }
        rows
    }

    /// Aba Trechos: a lista à esquerda e a conversa escolhida à direita. "Ver inteira", ou a paleta estreita com uma
    /// conversa aberta: só a conversa.
    fn render_search_hits(&mut self, active: Option<&Entry>, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search_text(cx).trim().to_owned();
        let rows = self.search_rows(&query);
        let active_row = rows.iter().position(|row| match row {
            Row::Session(target) => active == Some(&Entry::Session(target.clone())),
            Row::Group(_, ix) => ix.iter().any(|i| active == Some(&Entry::Hit(*i))),
            _ => false,
        });
        // Lista virtual, como a conversa: só as linhas à vista são desenhadas.
        let list_state = self.search.list.get_or_insert_with(|| ListState::new(0, ListAlignment::Top, px(400.))).clone();
        // A lista guarda a altura de cada linha: mudou o que ela mostra (linhas ou o "Retomando…" de um trecho), remede.
        if rows != self.search.rows || self.search.resuming != self.search.drawn_resuming {
            list_state.reset(rows.len());
            (self.search.rows, self.search.drawn_resuming) = (rows, self.search.resuming.clone());
        }
        (self.search.drawn_active, self.search.drawn_words) = (active.cloned(), terms(&query));
        if std::mem::take(&mut self.search.scroll_pending) && let Some(row) = active_row { list_state.scroll_to_reveal_item(row); }
        let view = cx.entity().downgrade();
        let results = div().relative().size_full()
            .child(list(list_state.clone(), move |i, _, cx| {
                view.update(cx, |this, cx| this.render_search_row(i, cx)).unwrap_or_else(|_| div().into_any_element())
            }).size_full().pt(px(8.)))
            .child(div().absolute().inset_0().child(Scrollbar::vertical(&list_state).mode(ScrollbarMode::Always)));
        let show_list = !(self.search.wide && self.search.chat.is_some());
        let show_chat = !self.search.narrow || !show_list;
        div().size_full().flex()
            .when(show_list, |el| el.child(div().h_full().min_w_0()
                .map(|el| if show_chat { el.w(relative(0.44)).flex_shrink_0().border_r_1().border_color(theme::border()) } else { el.flex_1() })
                .child(results)))
            .when(show_chat, |el| el.child(div().flex_1().min_w_0().h_full().child(self.render_search_chat(cx))))
            .into_any_element()
    }

    /// Uma linha da lista virtual da esquerda.
    fn render_search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.search.rows.get(index).cloned() else { return div().into_any_element() };
        let (active, words) = (self.search.drawn_active.clone(), self.search.drawn_words.clone());
        let content = match row {
            Row::ResumeError(error) => div().id("search-resume-error").px(px(10.)).py(px(8.)).rounded(px(8.)).bg(theme::danger().alpha(0.12))
                .role(Role::Alert).text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error).into_any_element(),
            Row::Section(key) => div().px(px(10.)).pt(px(6.)).pb(px(2.)).text_size(px(11.5)).font_weight(FontWeight::MEDIUM)
                .text_color(theme::faint()).child(tr(key)).into_any_element(),
            Row::Status(text, danger) => search_status_line(text, if danger { theme::danger() } else { theme::muted() }).into_any_element(),
            Row::Retry => div().flex().justify_center().pb(px(6.)).child(Button::new("search-retry").outline().small()
                .label(web_with("busca_tentar_de_novo", &[])).on_click(cx.listener(|this, _, _, cx| this.search_retry(cx)))).into_any_element(),
            Row::Group(key, indexes) => {
                self.render_search_group(&format!("hits-{key}"), &self.search.hits, &indexes, Entry::Hit, active.as_ref(), &words, false, cx)
            }
            Row::Session(target) => {
                let Some(s) = self.target_session(&target).cloned() else { return div().into_any_element() };
                let entry = Entry::Session(target.clone());
                let on = active.as_ref() == Some(&entry);
                // Com várias máquinas, de qual é: o mesmo nome pode estar em duas.
                let place = match (self.multi_server().then(|| self.machine_label(&target.server, cx)), s.cwd.as_deref().map(home_short)) {
                    (Some(label), Some(cwd)) => Some(format!("{label} · {cwd}")),
                    (label, cwd) => label.or(cwd),
                };
                self.search_row(format!("search-session-{}", target.id()), on, entry, cx)
                    .child(chrome::session_avatar(&sidebar::session_look(&s), 22., None))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                        .child(div().flex().items_center().gap(px(6.)).min_w_0()
                            .child(div().truncate().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).child(highlighted(s.name.clone(), &words)))
                            // A busca é o caminho até a oculta: a marca diz por que ela não está na barra.
                            .when(s.hidden_from_list, |el| el.child(div().flex_shrink_0().px(px(6.)).rounded_full().bg(theme::inset())
                                .text_size(px(10.5)).text_color(theme::muted()).child(tr("sidebar_hidden_badge"))))
                            .when(s.asleep, |el| el.child(div().flex_shrink_0().px(px(6.)).rounded_full().bg(theme::inset())
                                .text_size(px(10.5)).text_color(theme::muted()).child(tr_shared("session_asleep_badge", &[])))))
                        .children(place.map(|c| div().truncate().text_size(px(11.5)).text_color(theme::faint()).child(c))))
                    .children(s.last_activity.map(|t| div().flex_shrink_0().text_size(px(12.)).text_color(theme::faint()).child(ago(t))))
                    .into_any_element()
            }
        };
        // A barra de rolagem fica por cima da borda direita: a linha não passa por baixo dela.
        div().w_full().pl(px(8.)).pr(px(14.)).pb(px(4.)).child(content).into_any_element()
    }

    /// Uma conversa da lista: o cabeçalho (avatar, nome, estado, conta) e até três trechos dela. `actions`: os botões de
    /// abrir na conversa escolhida (o Perguntar não tem a prévia, onde eles moram na outra aba).
    #[allow(clippy::too_many_arguments)]
    fn render_search_group(&self, id: &str, hits: &[Hit], indexes: &[usize], entry: fn(usize) -> Entry, active: Option<&Entry>, words: &[String],
        actions: bool, cx: &mut Context<Self>) -> AnyElement {
        let first = &hits[indexes[0]];
        let selected = indexes.iter().any(|i| active == Some(&entry(*i)));
        let sub = search_group_sub(first.agent(), self.multi_server().then(|| first.label.clone()), first.place(), first.mtime);
        let pick = entry(indexes[0]);
        let header = search_group_header(self.search_avatar(first, 22.), first.title(), first.live, self.search_account(first), sub)
            .id(SharedString::from(format!("search-group-head-{id}"))).cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.search_select(&pick, cx)));
        let mut group = search_group_frame(selected).id(SharedString::from(format!("search-group-{id}"))).child(header);
        for i in indexes {
            let e = entry(*i);
            let hit = &hits[*i];
            let on = active == Some(&e);
            let who = search_who(hit.role.as_deref() == Some("user"), hit.agent());
            let line: String = plain(&hit.line).chars().take(600).collect();
            let resuming = self.search.resuming.as_deref() == Some(hit.conversation().as_str());
            let aria = format!("{who}: {line}");
            group = group.child(search_hit_line(on, who, hit.when(), line, words, resuming)
                .id(SharedString::from(format!("search-hit-{}-{}", matches!(e, Entry::Answer(_)), hit.key()))).cursor_pointer()
                .role(Role::Button).aria_selected(on).aria_label(aria)
                .on_click(cx.listener(move |this, _, _, cx| this.search_select(&e, cx))));
        }
        if actions && selected { group = group.child(div().pl(px(30.)).pt(px(4.)).flex().gap(px(6.)).child(self.search_open_button(first, cx))); }
        group.into_any_element()
    }

    /// "Ir para a conversa" (viva) ou "Retomar" (arquivada).
    fn search_open_button(&self, hit: &Hit, cx: &mut Context<Self>) -> Button {
        let resuming = self.search.resuming.as_deref() == Some(hit.conversation().as_str());
        let hit = hit.clone();
        search_open_look(format!("search-open-{}", hit.conversation()), hit.live_name().is_some())
            .loading(resuming).disabled(self.search.resuming.is_some())
            .on_click(cx.listener(move |this, _, window, cx| this.search_open_hit(hit.clone(), window, cx)))
    }

    /// A conversa do trecho escolhido, desenhada pelo `render_row` do chat com o pacote da prévia no lugar.
    fn render_search_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let center = |text: String, color: Hsla| div().size_full().p_6().flex().items_center().justify_center().text_size(px(13.)).text_color(color)
            .whitespace_normal().child(text).into_any_element();
        let Some(chat) = self.search.chat.as_ref() else { return center(tr("search_pick"), theme::faint()) };
        let hit = chat.hit.clone();
        let live = hit.live_name().is_some();
        let sub = search_chat_sub(hit.agent(), self.search_account(&hit).map(|(account, _)| account), hit.place(), live, hit.mtime);
        let mut buttons = Vec::new();
        // Na paleta estreita a conversa ocupa o lugar da lista: o botão é o caminho de volta.
        if !live || self.search.narrow {
            buttons.push(search_whole_look("search-whole", self.search.wide)
                .on_click(cx.listener(|this, _, _, cx| { this.search.wide = !this.search.wide; cx.notify(); })).into_any_element());
        }
        buttons.push(self.search_open_button(&hit, cx).into_any_element());
        let header = search_chat_header(self.search_avatar(&hit, 28.), hit.title(), sub, buttons);
        let (loaded, error) = (chat.loaded, chat.error.clone());
        let body = match (loaded, error) {
            (false, _) => popup::skeleton("search-chat-loading", 8).p_4().into_any_element(),
            (true, Some(error)) => div().id("search-chat-error").p_4().flex().flex_col().items_start().gap(px(8.)).role(Role::Alert)
                .child(div().text_size(px(13.)).text_color(theme::warning()).whitespace_normal().child(error))
                .child(Button::new("search-chat-retry").outline().small().label(web_with("busca_tentar_de_novo", &[]))
                    .on_click(cx.listener(|this, _, _, cx| if this.search.retry_chat() { this.search_fetch_chat(cx); cx.notify(); })))
                .into_any_element(),
            (true, None) => match self.with_search_slot(cx, |this, _| this.list_state.clone()) {
                None => center(tr("loading"), theme::faint()),
                Some(list_state) => {
                    self.search_watch_edges(&list_state, cx);
                    let view = cx.entity().downgrade();
                    div().relative().size_full()
                        .child(list(list_state.clone(), move |i, window, cx| {
                            view.update(cx, |this, cx| {
                                let focus = this.search.chat.as_ref().and_then(|c| c.focus_row) == Some(i);
                                let row = this.with_search_slot(cx, |this, cx| this.render_row(i, window, cx))?;
                                Some(if !focus { row } else { search_found_frame(row).into_any_element() })
                            }).ok().flatten().unwrap_or_else(|| div().into_any_element())
                        }).size_full())
                        .child(div().absolute().inset_0().child(Scrollbar::vertical(&list_state).mode(ScrollbarMode::Always)))
                        .into_any_element()
                }
            },
        };
        div().size_full().flex().flex_col().overflow_hidden().child(header).child(div().flex_1().min_h_0().min_w_0().pt(px(6.)).overflow_hidden().child(body))
            .into_any_element()
    }

    /// Rolar até perto de uma borda do pedaço desenhado traz mais mensagens daquele lado, uma vez por chegada.
    fn search_watch_edges(&mut self, list_state: &ListState, cx: &mut Context<Self>) {
        const NEAR: f32 = 240.;
        let scrolled = -f32::from(list_state.scroll_px_offset_for_scrollbar().y);
        let max = f32::from(list_state.max_offset_for_scrollbar().y);
        let Some(chat) = self.search.chat.as_mut() else { return };
        let (near_top, near_bottom) = (scrolled < NEAR, max - scrolled < NEAR);
        let mut extend = None;
        if !near_top { chat.top_armed = true; } else if chat.top_armed && chat.shown.start > 0 { chat.top_armed = false; extend = Some(true); }
        if !near_bottom { chat.bottom_armed = true; } else if chat.bottom_armed && chat.shown.end < chat.events.len() { chat.bottom_armed = false; extend = Some(false); }
        let Some(top) = extend else { return };
        let seq = chat.seq;
        let view = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = view.update(cx, |this, cx| if this.search.chat.as_ref().is_some_and(|c| c.seq == seq) { this.search_extend(top, cx) });
        });
    }

    /// Aba Perguntar: a pergunta, a resposta e só as conversas que ela cita.
    fn render_search_ask(&mut self, active: Option<&Entry>, cx: &mut Context<Self>) -> AnyElement {
        let center = |text: String, color: Hsla| div().py(px(24.)).flex().justify_center().text_size(px(13.)).text_color(color)
            .whitespace_normal().child(text).into_any_element();
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(error) = self.search.resume_error.clone() {
            rows.push(div().id("search-ask-resume-error").px(px(10.)).py(px(8.)).rounded(px(8.)).bg(theme::danger().alpha(0.12)).role(Role::Alert)
                .text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error).into_any_element());
        }
        let label = |text: String| div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(text.to_uppercase());
        if self.search.ask.loading {
            rows.push(center(web_with("switcher_perguntando", &[]), theme::muted()));
        } else {
            match &self.search.ask.value {
                None => rows.push(center(tr("search_ask_empty"), theme::faint())),
                Some(Err(error)) => {
                    rows.push(div().id("search-ask-error").py(px(16.)).flex().flex_col().items_center().gap(px(8.)).role(Role::Alert)
                        .child(div().text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error.clone()))
                        .child(Button::new("search-ask-retry").outline().small().label(web_with("busca_tentar_de_novo", &[]))
                            .on_click(cx.listener(|this, _, _, cx| this.search_ask(cx))))
                        .into_any_element());
                }
                Some(Ok((answer, hits))) => {
                    let (answer, hits) = (answer.clone(), hits.clone());
                    rows.push(div().p(px(16.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::inset()).flex().flex_col().gap(px(6.))
                        .child(label(tr("search_question")))
                        .child(div().text_size(px(13.)).whitespace_normal().child(self.search.asked.clone()))
                        .child(label(tr("search_answer")).mt(px(8.)))
                        .child(gpui_kit::base::TextView::markdown("search-answer", answer.clone()).selectable(true).scrollable(false).style(theme::conversation_markdown(cx)))
                        .into_any_element());
                    let words = terms(&self.search.asked);
                    let cited = self.cited_groups();
                    if cited.is_empty() {
                        rows.push(div().pt(px(4.)).text_size(px(12.)).text_color(theme::faint()).child(tr("search_no_cited")).into_any_element());
                    } else {
                        rows.push(div().pt(px(8.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted())
                            .child(tr("search_cited").replace("{n}", &cited.len().to_string())).into_any_element());
                        for (key, indexes) in &cited {
                            rows.push(self.render_search_group(&format!("ask-{key}"), &hits, indexes, Entry::Answer, active, &words, true, cx));
                        }
                    }
                    let shown: std::collections::HashSet<&str> = cited.iter().map(|(key, _)| key.as_str()).collect();
                    let left = hits.iter().filter(|h| !shown.contains(h.conversation().as_str())).count();
                    if left > 0 && !cited.is_empty() {
                        rows.push(div().text_size(px(11.5)).text_color(theme::faint()).whitespace_normal()
                            .child(tr("search_uncited").replace("{n}", &left.to_string())).into_any_element());
                    }
                }
            }
        }
        div().relative().size_full()
            .child(div().id("search-ask").size_full().overflow_y_scroll().track_scroll(&self.search.ask_scroll).p(px(16.)).pr(px(22.))
                .child(div().max_w(px(860.)).flex().flex_col().gap(px(8.)).children(rows)))
            .child(div().absolute().inset_0().child(Scrollbar::vertical(&self.search.ask_scroll).mode(ScrollbarMode::Always)))
            .into_any_element()
    }

    /// Linha escolhível de sessão viva: o clique abre.
    fn search_row(&self, id: String, on: bool, entry: Entry, cx: &mut Context<Self>) -> Stateful<Div> {
        div().id(SharedString::from(id)).w_full().min_h(px(40.)).px(px(10.)).py(px(6.)).rounded(px(8.)).flex().items_center().gap(px(10.))
            .cursor_pointer().when(on, |el| el.bg(theme::accent_dim())).when(!on, |el| el.hover(|el| el.bg(theme::hover())))
            .on_click(cx.listener(move |this, _, window, cx| this.search_activate(Some(entry.clone()), window, cx)))
    }
}

fn state_badge(live: bool) -> Div {
    let (text, fg, bg) = if live { (tr("search_badge_live"), theme::success_text(), theme::success().alpha(0.16)) }
        else { (tr("search_badge_archived"), theme::muted(), theme::inset()) };
    div().flex_shrink_0().px(px(7.)).h(px(18.)).flex().items_center().rounded_full().bg(bg).text_size(px(10.5)).text_color(fg).child(text)
}

/// O selo de conta da lista de sessões.
fn account_pill(label: String, color: Hsla) -> Div {
    div().flex_shrink_1().min_w_0().max_w(px(110.)).h(px(18.)).px(px(6.)).flex().items_center().gap(px(4.)).rounded_full().bg(theme::hover())
        .text_size(px(10.)).font_weight(FontWeight::MEDIUM).text_color(theme::muted())
        .child(div().size(px(5.)).flex_shrink_0().rounded_full().bg(color))
        .child(div().min_w_0().truncate().child(label))
}

// Partes de vista da paleta que a miniatura da novidade (`crate::news::search_conversations`) também desenha.

/// O campo da busca: lupa, o que se digita (`content`) e o Esc.
pub(crate) fn search_field(content: impl IntoElement) -> Div {
    div().flex_1().min_w_0().h(px(40.)).px(px(12.)).flex().items_center().gap(px(10.)).rounded(px(10.))
        .border_1().border_color(theme::border_strong()).bg(theme::inset())
        .child(chrome::small_icon(IconName::Search, 16., theme::muted()))
        .child(content)
        .child(chrome::kbd("Esc"))
}

/// Quebra em vez de passar da borda na paleta estreita.
fn scope_line() -> Div {
    div().px(px(16.)).pb(px(8.)).flex().flex_wrap().items_center().gap(px(6.)).min_w_0().text_size(px(12.)).text_color(theme::faint())
}

/// O que a busca achou, para a linha de escopo.
pub(crate) struct ScopeFound { pub hits: usize, pub conversations: usize, pub many_terms: bool, pub partial: usize }

/// A linha de escopo: onde a busca procurou e, com resultado, quanto achou.
pub(crate) fn search_scope_summary(scope: String, found: Option<ScopeFound>) -> Div {
    let mut parts: Vec<AnyElement> = Vec::new();
    if let Some(found) = found {
        let count = |one: &str, many: &str, n: usize| if n == 1 { web_with(one, &[]) } else { web_with(many, &[("n", n.to_string())]) };
        parts.push(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).text_color(theme::muted())
            .child(format!("{} {}", count("busca_um_trecho", "busca_n_trechos", found.hits), count("busca_uma_conversa", "busca_n_conversas", found.conversations)))
            .into_any_element());
        if found.many_terms { parts.push(div().truncate().child(format!("· {}", tr("search_same_message"))).into_any_element()); }
        if found.partial > 0 {
            parts.push(div().flex_shrink_0().text_color(theme::warning_text())
                .child(format!("· {}", tr("search_partial_only").replace("{n}", &found.partial.to_string()))).into_any_element());
        }
    }
    scope_line()
        .child(div().flex_shrink_0().text_color(theme::muted()).child(scope))
        .child(div().min_w_0().truncate().child(format!("· {}", tr("search_scope_everywhere"))))
        .child(div().flex_1())
        .children(parts)
}

/// Um botão de filtro, sem a ação.
pub(crate) fn search_filter_chip(id: String, on: bool, label: AnyElement, count: usize) -> Button {
    Button::new(SharedString::from(id)).outline().xsmall().selected(on)
        .child(div().flex().items_center().gap(px(5.)).child(label)
            .child(div().text_color(theme::faint()).child(count.to_string())))
}

/// O rótulo do filtro de agente: a marca dele e o nome.
pub(crate) fn search_agent_label(agent: &str) -> AnyElement {
    let (color, _) = theme::provider(agent);
    let logo = chrome::provider_logo(agent, 12., if agent == "codex" { theme::text() } else { color }).map(|l| l.into_any_element());
    div().flex().items_center().gap(px(5.)).children(logo).child(agent_name(agent)).into_any_element()
}

/// O rótulo do filtro de aberta ou arquivada.
pub(crate) fn search_live_label(live: bool) -> AnyElement {
    div().flex().items_center().gap(px(5.))
        .when(live, |el| el.child(div().size(px(6.)).rounded_full().bg(theme::success())))
        .child(tr(if live { "search_filter_live" } else { "search_filter_archived" })).into_any_element()
}

/// A linha de escopo com os filtros embaixo.
pub(crate) fn search_scope_block(line: Div, chips: Vec<AnyElement>) -> Div {
    div().flex().flex_col().child(line).child(div().px(px(16.)).pb(px(10.)).flex().flex_wrap().gap(px(6.)).children(chips))
}

/// Linha de estado da lista ("Buscando…", nada achado, falha).
pub(crate) fn search_status_line(text: String, color: Hsla) -> Div {
    // Centrado pelo texto, não pelo flex: a frase maior que a lista quebra em vez de sair pelos dois lados.
    div().w_full().px(px(10.)).py(px(14.)).text_center().text_size(px(13.)).text_color(color).whitespace_normal().child(text)
}

/// A caixa de uma conversa da lista; a escolhida ganha borda e fundo.
pub(crate) fn search_group_frame(selected: bool) -> Div {
    div().p(px(10.)).rounded(px(10.)).flex().flex_col().gap(px(4.))
        .border_1().border_color(if selected { theme::border_strong() } else { transparent_black() })
        .when(selected, |el| el.bg(theme::hover()))
}

/// O cabeçalho da conversa: avatar, nome, aberta ou arquivada, conta e a linha de baixo.
pub(crate) fn search_group_header(avatar: Div, title: String, live: bool, account: Option<(String, Hsla)>, sub: String) -> Div {
    div().flex().flex_col().gap(px(2.))
        .child(div().flex().items_center().gap(px(8.)).min_w_0()
            .child(avatar)
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(state_badge(live))
            .children(account.map(|(label, color)| account_pill(label, color))))
        .child(div().pl(px(30.)).truncate().text_size(px(11.5)).text_color(theme::faint()).child(sub))
}

/// "Claude · máquina · pasta · última atividade …"; a máquina só com várias.
pub(crate) fn search_group_sub(agent: &str, machine: Option<String>, place: String, mtime: f64) -> String {
    let mut sub = vec![agent_name(agent).to_owned()];
    sub.extend(machine);
    sub.push(place);
    if mtime > 0. { sub.push(tr("search_last_activity").replace("{when}", &when(mtime))); }
    sub.join(" · ")
}

/// Quem escreveu o trecho: "você" ou o agente.
pub(crate) fn search_who(user: bool, agent: &str) -> String { if user { tr("search_you") } else { agent_name(agent).to_owned() } }

/// Um trecho: quem e quando à esquerda, a mensagem com as palavras marcadas à direita.
pub(crate) fn search_hit_line(on: bool, who: String, ts: f64, line: String, words: &[String], resuming: bool) -> Div {
    div().ml(px(22.)).px(px(8.)).py(px(6.)).rounded(px(8.)).flex().items_start().gap(px(10.))
        .when(on, |el| el.bg(theme::accent_dim())).when(!on, |el| el.hover(|el| el.bg(theme::hover())))
        .child(div().w(px(64.)).flex_shrink_0().flex().flex_col().text_size(px(11.)).text_color(theme::muted())
            .child(div().truncate().child(who))
            .child(div().text_size(px(10.5)).text_color(theme::faint()).child(when(ts))))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
            .child(div().text_size(px(13.)).line_height(relative(1.45)).text_color(theme::text()).line_clamp(4).child(highlighted(line, words)))
            .when(resuming, |el| el.child(div().text_size(px(12.)).text_color(theme::muted()).child(tr("search_resuming")))))
}

/// "Ir para a conversa" (viva) ou "Retomar" (arquivada), sem a ação.
pub(crate) fn search_open_look(id: String, live: bool) -> Button {
    Button::new(SharedString::from(id)).primary().small().label(tr(if live { "search_go" } else { "search_resume_short" }))
}

/// "Ver inteira" / "Voltar à lista", sem a ação.
fn search_whole_look(id: &'static str, wide: bool) -> Button {
    Button::new(id).ghost().small().label(tr(if wide { "search_back_to_list" } else { "search_view_whole" }))
}

/// "Claude · conta X · pasta · aberta agora / arquivada há …" do cabeçalho da prévia.
pub(crate) fn search_chat_sub(agent: &str, account: Option<String>, place: String, live: bool, mtime: f64) -> String {
    let mut sub = vec![agent_name(agent).to_owned()];
    if let Some(account) = account { sub.push(tr("search_account").replace("{name}", &account)); }
    sub.push(place);
    sub.push(if live { tr("search_badge_live") } else if mtime > 0. { tr("search_archived_ago").replace("{ago}", &ago(mtime)) } else { tr("search_badge_archived") });
    sub.join(" · ")
}

/// O cabeçalho da prévia: avatar, nome, a linha de baixo e os botões.
pub(crate) fn search_chat_header(avatar: Div, title: String, sub: String, buttons: Vec<AnyElement>) -> Div {
    div().flex_shrink_0().px(px(14.)).py(px(10.)).flex().items_center().gap(px(10.)).border_b_1().border_color(theme::border())
        .child(avatar)
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
            .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().truncate().text_size(px(11.5)).text_color(theme::faint()).child(sub)))
        .child(div().flex_shrink_0().flex().items_center().gap(px(6.)).children(buttons))
}

/// A mensagem do trecho na prévia, com o contorno e o rótulo "trecho encontrado".
pub(crate) fn search_found_frame(row: AnyElement) -> Div {
    // `w_full` nas duas caixas: sem largura definida, a linha media o texto sem quebra e passava da borda.
    div().w_full().px(px(8.)).py(px(4.)).flex().flex_col().gap(px(4.))
        .child(div().px(px(4.)).text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(theme::accent_text()).child(tr("search_found_snippet")))
        .child(div().w_full().rounded(px(10.)).border_1().border_color(theme::accent()).overflow_hidden().child(row))
}

#[cfg(test)]
mod tests {
    use super::{Filter, Hit, Machine, Search, SearchChat, all_whole, cites, first_words, groups, machine_reason, marks, merge_hits, merge_keeping,
        moment, next_group, palette_frame, parse_answer, plain, terms, window_around};
    use crate::api::Failure;
    use gpui_kit::{px, size};
    use proptest::prelude::*;

    #[test]
    fn a_question_from_a_closed_palette_never_answers_the_new_one() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let mut search = Search::default();
        let old = search.ask.start();
        let task = runtime.spawn(std::future::pending::<()>());
        search.ask_task = Some(task.abort_handle());
        search.reopen(None);
        assert!(!search.ask.loading && search.ask.value.is_none());
        let new = search.ask.start();
        assert_ne!(old, new, "a resposta da pergunta A entraria como a da B");
        assert!(!search.ask.finish(old, Ok((String::from("A"), Vec::new()))));
        assert!(runtime.block_on(task).is_err_and(|e| e.is_cancelled()), "a pergunta A seguiria em voo");
    }

    #[test]
    fn a_machine_failure_says_why_when_the_server_did() {
        let fail = |status: u16, detail: &str| Failure { status: Some(status), detail: detail.into(), retry_after: None, uncertain: false,
            code: Some("erro_busca_contas".into()) };
        let said = machine_reason(&fail(503, "as contas Claude não puderam ser lidas"));
        assert!(said.contains("503") && said.contains("as contas Claude não puderam ser lidas"), "{said}");
        let bare = machine_reason(&fail(503, "HTTP 503"));
        assert!(bare.contains("503") && bare.matches("503").count() == 1, "{bare}");
    }

    #[test]
    fn an_answer_without_text_is_an_invalid_response() {
        assert!(parse_answer(&serde_json::json!({"hits": []})).is_err(), "200 sem answer viraria resposta vazia");
        assert!(parse_answer(&serde_json::json!({"answer": 5, "hits": []})).is_err());
        assert_eq!(parse_answer(&serde_json::json!({"answer": "na sessão x", "hits": []})).unwrap().0, "na sessão x");
    }

    #[test]
    fn marks_never_split_a_character() {
        // Ⱥ cresce um byte na minúscula e o K de Kelvin perde dois: o total fica igual e as faixas desalinham.
        let text = "\u{23A}\u{23A}\u{212A}";
        assert_eq!(text.len(), text.to_lowercase().len());
        for (range, _) in marks(text, &terms("k")) {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end), "{range:?} corta um caractere de {text:?}");
        }
    }

    /// Texto Unicode qualquer, com peso nos caracteres que mudam de tamanho na minúscula.
    fn unicode(max: usize) -> impl Strategy<Value = String> {
        let tricky = prop_oneof![Just('\u{23A}'), Just('\u{212A}'), Just('\u{130}'), Just('ß'), Just('k'), Just('a'), Just(' ')];
        prop::collection::vec(prop_oneof![tricky, any::<char>()], 0..max).prop_map(|chars| chars.into_iter().collect())
    }

    proptest! {
        #[test]
        fn marks_stay_on_character_boundaries(text in unicode(40), query in unicode(6)) {
            for (range, _) in marks(&text, &terms(&query)) {
                prop_assert!(range.start < range.end && range.end <= text.len());
                prop_assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            }
        }
    }

    #[test]
    fn the_total_waits_for_every_machine() {
        let machine = Machine { key: "b".into(), label: "B".into(), offline: false };
        let pending = Search { totals: vec![Some(10)], pending: vec![machine.clone()], ..Default::default() };
        assert_eq!(pending.total(), None, "o total sairia antes da outra máquina responder");
        let failed = Search { totals: vec![Some(10)], failed: vec![(machine, "x".into())], ..Default::default() };
        assert_eq!(failed.total(), None, "a máquina que falhou ficaria fora do total");
        assert_eq!(Search { totals: vec![Some(10), Some(5)], ..Default::default() }.total(), Some(15));
    }

    #[test]
    fn a_failed_preview_can_be_tried_again() {
        let chat = |error: Option<&str>| SearchChat { hit: Hit::default(), key: "k".into(), focus: None, focus_row: None, seq: 4, slot: None,
            loaded: true, error: error.map(str::to_owned), events: Vec::new(), shown: 0..0, top_armed: false, bottom_armed: false };
        let mut search = Search { chat: Some(chat(Some("caiu"))), chat_seq: 4, ..Default::default() };
        assert!(search.retry_chat(), "sem saída do erro da prévia");
        let retried = search.chat.as_ref().unwrap();
        assert!(!retried.loaded && retried.error.is_none());
        assert!(retried.seq > 4, "a resposta da tentativa anterior entraria na nova");
        let mut fine = Search { chat: Some(chat(None)), ..Default::default() };
        assert!(!fine.retry_chat());
    }

    #[test]
    fn the_palette_fits_any_window_centered_with_a_margin() {
        for (w, h) in [(1350., 820.), (2560., 1440.), (900., 600.), (520., 380.), (300., 200.)] {
            let window = size(px(w), px(h));
            let (frame, _) = palette_frame(window);
            assert!(frame.origin.x >= px(0.) && frame.origin.y >= px(0.), "{w}×{h}: a borda esquerda ou a de cima sai da janela");
            assert!(frame.origin.x + frame.size.width <= window.width && frame.origin.y + frame.size.height <= window.height, "{w}×{h}: passa da janela");
            assert_eq!(frame.origin.x * 2. + frame.size.width, window.width, "{w}×{h}: fora do centro");
            assert_eq!(frame.origin.y * 2. + frame.size.height, window.height, "{w}×{h}: fora do centro");
            assert!(frame.origin.x >= px(8.) && frame.origin.y >= px(8.), "{w}×{h}: sem margem");
        }
        assert!(palette_frame(size(px(2560.), px(1440.))).0.size.width <= px(1180.), "janela grande esticaria a paleta");
        assert!(!palette_frame(size(px(1350.), px(820.))).1, "1350 cabe lista e conversa lado a lado");
        assert!(palette_frame(size(px(800.), px(700.))).1, "800 empilha");
    }

    #[test]
    fn the_preview_draws_a_window_around_the_snippet() {
        assert_eq!(window_around(1000, Some(500), 40, 40), 460..541);
        assert_eq!(window_around(1000, Some(10), 40, 40), 0..51);
        assert_eq!(window_around(1000, Some(990), 40, 40), 950..1000);
        assert_eq!(window_around(1000, None, 40, 40), 920..1000, "sem trecho, o fim da conversa");
        assert_eq!(window_around(30, Some(5), 40, 40), 0..30);
        assert_eq!(window_around(0, None, 40, 40), 0..0);
    }

    #[test]
    fn an_old_server_hit_is_named_by_folder_and_its_own_start() {
        let old = |line: &str| Hit { cwd: Some("/home/j/pessoal/hangar".into()), ..hit("a", line, 1.) };
        assert_eq!(old("…junto. 4. Trocar **modelo**").title(), "hangar · junto. 4. Trocar modelo");
        assert_ne!(old("travou outra vez").title(), old("O PC está travando").title(), "várias conversas da mesma pasta ficariam com o mesmo nome");
    }

    #[test]
    fn date_phrases_never_repeat_the_ago_word() {
        for (lang, source) in [("pt", include_str!("../../../messages/pt.json")), ("en", include_str!("../../../messages/en.json"))] {
            let dict: serde_json::Value = serde_json::from_str(source).unwrap();
            let text = |key: &str| dict[format!("native_{key}")].as_str().unwrap().to_owned();
            for ago in ["ago_now", "ago_min", "ago_h", "ago_d"] {
                let phrase = text("search_archived_ago").replace("{ago}", &text(ago).replace("{n}", "24"));
                let words: Vec<&str> = phrase.split_whitespace().collect();
                assert!(!words.windows(2).any(|w| w[0] == w[1]), "{lang}: {phrase}");
                assert!(phrase.matches(" ago").count() <= 1 && phrase.matches("há").count() <= 1, "{lang}: {phrase}");
            }
        }
    }

    fn hit(server: &str, line: &str, mtime: f64) -> Hit {
        Hit { project: "p".into(), session_id: "s".into(), line: line.into(), mtime, server: server.into(), ..Default::default() }
    }

    fn at(text: &str) -> chrono::NaiveDateTime { chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap() }

    #[test]
    fn highlighted_hit_stays_put_when_a_late_machine_reorders() {
        let mut hits = Vec::new();
        merge_hits(&mut hits, vec![hit("a", "x", 2.), hit("a", "y", 1.)]);
        let moved = merge_keeping(&mut hits, vec![hit("b", "z", 3.)], Some(1));
        assert_eq!(moved.map(|i| hits[i].key()).as_deref(), Some("a/p/s/y"), "o Enter abriria um trecho que a pessoa não escolheu");
        assert_eq!(merge_keeping(&mut hits, vec![], None), None);
    }

    #[test]
    fn reopening_never_reuses_a_search_number() {
        let mut search = Search { debounce: 7, chat_seq: 3, query: "x".into(), hits: vec![hit("a", "x", 1.)], ..Default::default() };
        search.reopen(None);
        assert!(search.debounce > 7, "resposta da abertura anterior casaria com a busca nova");
        assert_eq!(search.chat_seq, 3, "a prévia nova reusaria o número de uma resposta em voo");
        assert!(search.open && search.query.is_empty() && search.hits.is_empty());
    }

    #[test]
    fn late_machine_merges_without_repeats_whole_words_first_then_newest() {
        let mut hits = Vec::new();
        merge_hits(&mut hits, vec![hit("a", "x", 1.), hit("a", "x", 1.)]);
        let part = Hit { whole: Some(false), ..hit("c", "w", 9.) };
        merge_hits(&mut hits, vec![hit("b", "x", 3.), hit("a", "y", 2.), part]);
        let keys: Vec<String> = hits.iter().map(Hit::key).collect();
        assert_eq!(keys, ["b/p/s/x", "a/p/s/y", "a/p/s/x", "c/p/s/w"], "pedaço de palavra subiria acima da palavra inteira");
    }

    #[test]
    fn the_same_conversation_on_two_machines_stays_apart() {
        let hit = |server: &str| Hit { project: "p".into(), session_id: "s".into(), line: "x".into(), server: server.into(), ..Default::default() };
        assert_ne!(hit("http://a:8765").conversation(), hit("http://b:8765").conversation(), "agrupar juntaria trechos de máquinas diferentes");
        assert_ne!(hit("http://a:8765").key(), hit("http://b:8765").key(), "a prévia aberta de uma abriria a da outra");
    }

    #[test]
    fn groups_by_conversation_in_arrival_order_capped_and_filtered() {
        let conv = |sid: &str, line: &str, provider: &str, live: bool| Hit { session_id: sid.into(), provider: Some(provider.into()), live, ..hit("a", line, 1.) };
        let hits = vec![conv("s1", "a", "claude", true), conv("s2", "b", "codex", false), conv("s1", "c", "claude", true),
            conv("s1", "d", "claude", true), conv("s1", "e", "claude", true)];
        assert_eq!(groups(&hits, &Filter::default(), 3), vec![("a/p/s1".to_owned(), vec![0, 2, 3]), ("a/p/s2".to_owned(), vec![1])]);
        assert_eq!(groups(&hits, &Filter::default(), usize::MAX)[0].1.len(), 4, "a contagem leva os trechos além dos mostrados");
        let codex = Filter { agent: Some("codex".into()), live: None };
        assert_eq!(groups(&hits, &codex, 3), vec![("a/p/s2".to_owned(), vec![1])]);
        let archived = Filter { agent: None, live: Some(false) };
        assert_eq!(groups(&hits, &archived, 3).len(), 1);
        assert_eq!(groups(&[hit("a", "x", 1.)], &Filter { agent: Some("claude".into()), live: None }, 3).len(), 1, "sem provider é Claude");
    }

    #[test]
    fn the_label_is_the_live_name_then_the_closing_name_then_the_first_message_then_the_folder() {
        let base = Hit { cwd: Some("/home/j/Projetos/hangar".into()), ..hit("a", "x", 1.) };
        let full = Hit { session_name: Some("viva".into()), live: true, name: Some("ao-fechar".into()), preview: Some("oi".into()), ..base.clone() };
        assert_eq!(full.title(), "viva");
        assert_eq!(Hit { live: false, ..full.clone() }.title(), "ao-fechar", "a morta não tem nome de sessão");
        assert_eq!(Hit { live: false, name: Some("  ".into()), ..full.clone() }.title(), "oi");
        assert_eq!(base.title(), "hangar · x", "servidor antigo: a pasta com o começo do trecho");
        assert_eq!(first_words("[de: outra] veja https://x.y/z o tour"), "veja o tour");
        let long = first_words(&"palavra ".repeat(20));
        assert!(long.ends_with('…') && long.chars().count() <= 59 && !long.contains("palavr…"), "cortou no meio da palavra: {long}");
        assert_eq!(base.place(), "~/Projetos/hangar");
    }

    #[test]
    fn the_moment_says_today_yesterday_or_the_date() {
        let now = at("2026-11-01 15:00");
        let say = |when: &str| moment(at(when), now, "hoje", "ontem", "%d/%m");
        assert_eq!(say("2026-11-01 14:02"), "hoje 14:02");
        assert_eq!(say("2026-10-31 11:24"), "ontem 11:24");
        assert_eq!(say("2026-10-30 19:34"), "30/10 19:34");
        assert_eq!(say("2025-10-30 19:34"), "30/10/25 19:34", "de outro ano, sem o ano parece deste");
    }

    #[test]
    fn whole_words_and_word_pieces_are_marked_apart() {
        let words = terms("Tour  tour uso");
        assert_eq!(words, ["tour", "uso"]);
        assert_eq!(marks("O tour de uso", &words), vec![(2..6, true), (10..13, true)]);
        assert_eq!(marks("estourava o uso", &words), vec![(2..6, false), (12..15, true)]);
        assert!(all_whole("o Tour, de uso.", &words));
        assert!(!all_whole("estourava o uso", &words), "pedaço contaria como acerto inteiro");
        assert_eq!(plain("**Recomendação:** `x`\n## Título\n#296 ok"), "Recomendação: x Título #296 ok");
    }

    #[test]
    fn the_answer_cites_a_conversation_by_one_of_its_names_only() {
        let named = Hit { session_name: Some("tela-onboarding-hangar".into()), live: true, ..hit("a", "x", 1.) };
        assert!(cites("Na sessão **tela-onboarding-hangar** (aberta agora)", &named));
        assert!(!cites("Em outra conversa", &named));
        let archived = Hit { name: Some("Tarefa: redesenhar".into()), cwd: Some("/home/j/hangar".into()), ..hit("a", "x", 1.) };
        assert!(cites("Na tarefa: redesenhar a tela", &archived), "sem diferenciar maiúsculas");
        assert!(!cites("o hangar mudou", &archived), "a pasta não é nome: toda resposta citaria a conversa");
    }

    #[test]
    fn tab_goes_to_the_next_conversation_and_wraps() {
        let keys: Vec<String> = ["a", "a", "b", "c", "c"].map(str::to_owned).to_vec();
        assert_eq!(next_group(&keys, 0), 2);
        assert_eq!(next_group(&keys, 2), 3);
        assert_eq!(next_group(&keys, 3), 0);
    }

    #[test]
    fn an_old_server_without_the_new_fields_still_parses() {
        let hits: Vec<Hit> = serde_json::from_value(serde_json::json!([{"project": "p", "session_id": "s", "line": "x", "mtime": 1.0, "live": false}])).unwrap();
        let hit = &hits[0];
        assert!(hit.provider.is_none() && hit.conta.is_none() && hit.whole.is_none() && hit.name.is_none() && hit.preview.is_none());
        assert_eq!(hit.agent(), "claude");
        let new: Vec<Hit> = serde_json::from_value(serde_json::json!([{"project": "p", "session_id": "s", "line": "x", "provider": "codex",
            "conta": "codex:/h/.codex", "config_dir": null, "codex_account": "w", "whole": false, "name": null, "preview": "oi"}])).unwrap();
        assert!(!new[0].is_whole() && new[0].agent() == "codex" && new[0].preview.as_deref() == Some("oi"));
    }
}
