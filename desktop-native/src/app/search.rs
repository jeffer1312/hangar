//! "Buscar conversas" (Ctrl+K): paleta no meio da janela, na superfície de vidro dos popovers, com o que o web mostra no
//! modo só-busca do `SessionSwitcherSheet.svelte` — busca de conteúdo em todas as conversas (vivas e arquivadas, `/api/search`),
//! trecho com a mensagem em volta (`/api/search/context`) e o "Perguntar" (`/api/ask-history`) — mais as sessões vivas
//! pelo nome, como a paleta do Zeron. Enter abre a sessão viva; a arquivada é retomada numa sessão nova
//! (`/api/archive/…/resume`), porque o nativo não tem a vista de arquivo do web.
//! Sessões e busca de conteúdo cobrem todas as máquinas próprias, como o web; cada trecho abre, mostra contexto e retoma na
//! máquina de onde veio. O "Perguntar" segue só na ativa, como no web.
use super::*;
use super::device::Remote;
use super::costs::web_with;
use super::sidebar::Target;
use serde::Deserialize;

/// Espera entre a última tecla e a busca, como o web.
const DEBOUNCE: Duration = Duration::from_millis(250);
const SESSIONS_MAX: usize = 8;
/// Com o índice FTS a busca responde em milissegundos; esperar mais só prende a linha numa máquina inalcançável.
const MACHINE_WAIT: Duration = Duration::from_secs(8);

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
}

/// Backend antigo em outra máquina ainda repete o trecho quando o transcript repete a mensagem: a chave repetida daria
/// dois elementos com o mesmo id.
fn without_repeats(hits: Vec<Hit>) -> Vec<Hit> {
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().filter(|hit| seen.insert(hit.key())).collect()
}

/// Junta a resposta de mais uma máquina: sem repetidos, a mais recente primeiro.
fn merge_hits(hits: &mut Vec<Hit>, more: Vec<Hit>) {
    let mut all = std::mem::take(hits);
    all.extend(more);
    *hits = without_repeats(all);
    hits.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
}

/// Junta como `merge_hits` e devolve onde foi parar o trecho `active`: a reordenação por data não pode trocar o destacado
/// (e o que o Enter abre) por outro.
fn merge_keeping(hits: &mut Vec<Hit>, more: Vec<Hit>, active: Option<usize>) -> Option<usize> {
    let key = active.and_then(|i| hits.get(i)).map(Hit::key);
    merge_hits(hits, more);
    key.and_then(|key| hits.iter().position(|hit| hit.key() == key))
}

/// Máquina da busca de conteúdo. `offline`: a lista dela já estava fora do ar quando a busca começou.
#[derive(Clone, Debug)]
struct Machine { key: String, label: String, offline: bool }

/// O que as setas percorrem: sessões vivas pelo nome e trechos da busca, na ordem da tela.
#[derive(Clone, Debug, PartialEq)]
enum Entry { Session(Target), Hit(usize), Answer(usize) }

#[derive(Default)]
pub(super) struct Search {
    pub(super) open: bool,
    input: Option<Entity<InputState>>,
    /// A busca que está na tela (ou em voo).
    query: String,
    /// Trechos das máquinas que já responderam, entrando conforme chegam.
    hits: Vec<Hit>,
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
    preview: Option<(String, Remote<Vec<ChatEvent>>)>,
    ask: Remote<(String, Vec<Hit>)>,
    /// Retomada em curso (a conversa) e o erro da última.
    resuming: Option<String>,
    resume_error: Option<String>,
    previous_focus: Option<FocusHandle>,
    scroll: ScrollHandle,
    _events: Option<Subscription>,
}

impl Search {
    /// Sem isto, a busca de um texto já trocado (ou da paleta fechada) seguia até o prazo da máquina.
    fn abort_machines(&mut self) {
        for task in self.tasks.drain(..) { task.abort(); }
    }

    /// Cada abertura começa vazia, mas o número da busca só cresce: zerado, a resposta de uma abertura anterior ainda em voo
    /// casaria número e texto com a busca nova.
    fn reopen(&mut self, previous_focus: Option<FocusHandle>) {
        self.abort_machines();
        *self = Search { open: true, input: self.input.take(), previous_focus, _events: self._events.take(), debounce: self.debounce + 1, ..Default::default() };
    }
}

/// Palavras da busca, minúsculas e sem repetição (`termos` do backend).
fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in query.to_lowercase().split_whitespace() { if !out.iter().any(|w| w == word) { out.push(word.to_owned()); } }
    out
}

/// Faixas de `text` com qualquer um dos termos, sem diferenciar maiúsculas (o `<mark>` do web).
fn marks(text: &str, terms: &[String]) -> Vec<std::ops::Range<usize>> {
    let lower = text.to_lowercase();
    // Minúscula que muda o tamanho do texto desalinharia as faixas: aí não destaca nada.
    if lower.len() != text.len() { return Vec::new(); }
    let mut ranges: Vec<std::ops::Range<usize>> = terms.iter().filter(|t| !t.is_empty())
        .flat_map(|t| lower.match_indices(t.as_str()).map(|(at, m)| at..at + m.len()).collect::<Vec<_>>()).collect();
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match merged.last_mut() { Some(last) if r.start <= last.end => last.end = last.end.max(r.end), _ => merged.push(r) }
    }
    merged
}

fn highlighted(text: String, terms: &[String]) -> StyledText {
    let style = HighlightStyle { color: Some(theme::text()), background_color: Some(theme::accent().alpha(0.28)), ..Default::default() };
    let ranges = marks(&text, terms);
    StyledText::new(text).with_highlights(ranges.into_iter().map(|r| (r, style)))
}

fn ago(ts: f64) -> String { side::ago(chrono::Local::now().timestamp() as f64 - ts) }

impl Hangar {
    /// Abre a busca de conversas (Ctrl+K e, depois, o botão da barra do topo); aberta, fecha.
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.open { self.close_search(window, cx) } else { self.open_search(window, cx) }
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
        // Cada abertura começa vazia, como o web.
        input.update(cx, |input, cx| input.set_value("", window, cx));
        let previous = window.focused(cx);
        self.search.reopen(previous);
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search.open { return; }
        self.search.open = false;
        self.search.debounce += 1;
        self.search.abort_machines();
        match self.search.previous_focus.take() { Some(focus) => focus.focus(window, cx), None => self.root_focus.focus(window, cx) }
        cx.notify();
    }

    /// Outra conexão: o que a busca mostrava era do servidor anterior.
    pub(super) fn search_reconnected(&mut self, cx: &mut Context<Self>) {
        if self.search.open { self.search_changed(cx); }
    }

    fn search_text(&self, cx: &App) -> String { self.search.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default() }

    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search_text(cx).trim().to_owned();
        (self.search.active, self.search.preview, self.search.ask, self.search.resume_error) = (0, None, Remote::default(), None);
        self.search.scroll.set_offset(point(px(0.), px(0.)));
        self.search.debounce += 1;
        self.search.abort_machines();
        let seq = self.search.debounce;
        if query.is_empty() {
            (self.search.query, self.search.hits, self.search.searching) = (String::new(), Vec::new(), false);
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
        (self.search.hits, self.search.pending, self.search.failed, self.search.error) = (Vec::new(), Vec::new(), Vec::new(), None);
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
                tokio::time::timeout(MACHINE_WAIT, api.server_read(&["search"], &[("q", query.as_str())], MACHINE_WAIT.as_secs() + 2)).await
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
                        .map_err(|e| match e.status { Some(status) => web_with("busca_motivo_http", &[("status", status.to_string())]), None => Hangar::failure(&e) })
                        .and_then(|v| serde_json::from_value::<Vec<Hit>>(v).map_err(|error| {
                            eprintln!("busca em {}: resposta inválida: {error}", machine.label);
                            tr("invalid_response")
                        })),
                };
                match result {
                    Ok(list) => {
                        let list = list.into_iter().map(|hit| Hit { server: machine.key.clone(), label: machine.label.clone(), ..hit }).collect();
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

    /// Os trechos na ordem da tela: por conversa, a mais recente primeiro.
    fn search_groups(&self) -> Vec<(String, Vec<usize>)> {
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, hit) in self.search.hits.iter().enumerate() {
            let key = hit.conversation();
            match groups.iter_mut().find(|g| g.0 == key) { Some(g) => g.1.push(i), None => groups.push((key, vec![i])) }
        }
        groups
    }

    fn search_entries(&self, cx: &App) -> Vec<Entry> {
        let query = self.search_text(cx).trim().to_owned();
        let mut out: Vec<Entry> = self.search_sessions(&query).into_iter().map(|(target, _)| Entry::Session(target)).collect();
        if let Some((_, hits)) = self.search.ask.ok() { out.extend((0..hits.len()).map(Entry::Answer)); }
        out.extend(self.search_groups().into_iter().flat_map(|(_, hits)| hits.into_iter().map(Entry::Hit)));
        out
    }

    fn search_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.search_entries(cx).len();
        if count == 0 { return; }
        self.search.active = (self.search.active as isize + delta).rem_euclid(count as isize) as usize;
        self.search.scroll.scroll_to_item(self.search.active);
        cx.notify();
    }

    fn hit_of(&self, entry: &Entry) -> Option<Hit> {
        match entry {
            Entry::Hit(i) => self.search.hits.get(*i).cloned(),
            Entry::Answer(i) => self.search.ask.ok()?.1.get(*i).cloned(),
            Entry::Session(_) => None,
        }
    }

    /// Enter (ou o botão): abre a sessão viva, retoma a arquivada.
    fn search_activate(&mut self, entry: Option<Entry>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = entry.or_else(|| self.search_entries(cx).get(self.search.active).cloned()) else { return };
        match entry {
            Entry::Session(target) => self.search_open_session(&target, window, cx),
            other => {
                let Some(hit) = self.hit_of(&other) else { return };
                match hit.live_name() {
                    Some(name) => { let target = Target::new(&hit.server, name); self.search_open_session(&target, window, cx) }
                    None => self.search_resume(hit, window, cx),
                }
            }
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
        self.close_settings(window, cx);
        self.open_target(target, window, cx);
    }

    /// Conversa arquivada: sobe uma sessão nova com `--resume` na conta dona dela, pela rota do Arquivo do web, na máquina
    /// de onde o trecho veio.
    fn search_resume(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&hit.server) else {
            self.search.resume_error = Some(self.machine_error(&hit.server));
            cx.notify();
            return;
        };
        if self.search.resuming.is_some() { return; }
        let server = hit.server.clone();
        self.search.resuming = Some(hit.conversation());
        self.search.resume_error = None;
        let task = self.runtime.spawn(async move {
            api.server_send(reqwest::Method::POST, &["archive", &hit.project, &hit.session_id, "resume"], Some(json!({})), 120).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let joined = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Sem isto o botão ficava ocupado para sempre e nenhuma outra retomada saía. Trocar a conexão ativa não
                // descarta: a sessão já nasceu na máquina do trecho, e é nela que abre.
                this.search.resuming = None;
                let Ok(result) = joined else { this.search.resume_error = Some(tr("search_interrupted")); cx.notify(); return };
                match result.map_err(|e| Self::fetch_failure(&e)).and_then(|v| serde_json::from_value::<SessionInfo>(v).map_err(|_| tr("invalid_response"))) {
                    Ok(session) => {
                        this.search.previous_focus = None;
                        this.close_search(window, cx);
                        this.close_costs(window, cx);
                        this.close_worktrees(window, cx);
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

    /// Clique no trecho: abre ou fecha a mensagem inteira com as vizinhas, sem sair da busca.
    fn search_toggle_preview(&mut self, hit: Hit, cx: &mut Context<Self>) {
        let key = hit.key();
        if self.search.preview.as_ref().is_some_and(|(k, _)| *k == key) { self.search.preview = None; cx.notify(); return; }
        let mut remote = Remote::default();
        let Some(event) = hit.event_id.clone() else {
            remote.value = Some(Err(web_with("busca_contexto_indisponivel", &[])));
            self.search.preview = Some((key, remote));
            cx.notify();
            return;
        };
        let seq = remote.start();
        self.search.preview = Some((key.clone(), remote));
        // O contexto é lido da máquina do trecho.
        let api = self.machine_api(&hit.server).ok_or_else(|| self.machine_error(&hit.server));
        let connection = self.connection;
        let task = self.runtime.spawn(async move {
            let api = match api { Ok(api) => api, Err(reason) => return Err(Failure::local(reason)) };
            api.server_read(&["search", "context"], &[("project", hit.project.as_str()), ("session_id", hit.session_id.as_str()), ("event_id", event.as_str())], 15).await
        });
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                let current = this.connection == connection;
                let Some((_, remote)) = this.search.preview.as_mut().filter(|(k, _)| *k == key) else { return };
                // Tarefa perdida ou conexão trocada: a prévia termina com o aviso em vez de carregar para sempre.
                let result = match joined {
                    Ok(result) if current => result.map_err(|e| Self::fetch_failure(&e)),
                    _ => Err(tr("search_interrupted")),
                };
                remote.finish(seq, result.and_then(|v| serde_json::from_value::<Vec<ChatEvent>>(v).map_err(|_| tr("invalid_response"))));
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// "Perguntar": o backend acha os trechos e um modelo responde onde o assunto apareceu (só neste servidor, como o web).
    fn search_ask(&mut self, cx: &mut Context<Self>) {
        let question = self.search_text(cx).trim().to_owned();
        if question.is_empty() || self.search.ask.loading { return; }
        let Some(api) = self.api.clone() else { return };
        let seq = self.search.ask.start();
        let connection = self.connection;
        let (server, label) = (self.active_key(), self.server_label(cx));
        let task = self.runtime.spawn(async move {
            api.server_send(reqwest::Method::POST, &["ask-history"], Some(json!({ "question": question })), 90).await
        });
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                // Mesmo caso da busca: sem resposta útil, o botão não pode ficar em "Perguntando…".
                let result = match joined {
                    Ok(result) if this.connection == connection => result.map_err(|e| Self::failure(&e)),
                    _ => Err(tr("search_interrupted")),
                };
                let parsed = result.and_then(|v| {
                    let answer = v.get("answer").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let hits = serde_json::from_value::<Vec<Hit>>(v.get("hits").cloned().unwrap_or(json!([]))).map_err(|_| tr("invalid_response"))?;
                    Ok((answer, without_repeats(hits.into_iter().map(|hit| Hit { server: server.clone(), label: label.clone(), ..hit }).collect())))
                });
                this.search.ask.finish(seq, parsed);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    // ── Desenho ─────────────────────────────────────────────────────────────

    pub(super) fn render_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search.open { return None; }
        let input = self.search.input.clone()?;
        let query = self.search_text(cx).trim().to_owned();
        let words = terms(&query);
        let entries = self.search_entries(cx);
        self.search.active = self.search.active.min(entries.len().saturating_sub(1));
        let active = entries.get(self.search.active).cloned();
        let viewport = window.viewport_size();
        let width = (viewport.width - px(32.)).min(px(760.));
        let list_h = (viewport.height * 0.72 - px(150.)).max(px(160.));

        let mut rows: Vec<AnyElement> = Vec::new();
        let section = |title: String| div().px(px(10.)).pt(px(10.)).pb(px(4.)).text_size(px(11.5)).font_weight(FontWeight::MEDIUM)
            .text_color(theme::faint()).child(title).into_any_element();

        let multi = self.multi_server();
        let sessions = self.search_sessions(&query);
        if !sessions.is_empty() {
            rows.push(section(tr("search_sessions")));
            for (target, s) in sessions {
                let entry = Entry::Session(target.clone());
                let on = active.as_ref() == Some(&entry);
                // Com várias máquinas, de qual é: o mesmo nome pode estar em duas.
                let place = match (multi.then(|| self.machine_label(&target.server, cx)), s.cwd.clone()) {
                    (Some(label), Some(cwd)) => Some(format!("{label} · {cwd}")),
                    (label, cwd) => label.or(cwd),
                };
                rows.push(self.search_row(format!("search-session-{}", target.id()), on, entry, cx)
                    .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(theme::status(&s.state)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                        .child(div().truncate().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).child(highlighted(s.name.clone(), &words)))
                        .children(place.map(|c| div().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::faint()).child(c))))
                    .children(s.last_activity.map(|t| div().flex_shrink_0().text_size(px(12.)).text_color(theme::faint()).child(ago(t))))
                    .into_any_element());
            }
        }

        if !query.is_empty() {
            rows.push(div().px(px(6.)).pt(px(8.)).child(Button::new("search-ask").ghost().small().w_full().loading(self.search.ask.loading)
                .disabled(self.search.ask.loading)
                .label(web_with(if self.search.ask.loading { "switcher_perguntando" } else { "switcher_perguntar" }, &[]))
                .on_click(cx.listener(|this, _, _, cx| this.search_ask(cx)))).into_any_element());
            match &self.search.ask.value {
                Some(Err(error)) => rows.push(div().px(px(10.)).py(px(4.)).text_size(px(13.)).text_color(theme::danger()).child(error.clone()).into_any_element()),
                Some(Ok((answer, hits))) => {
                    let mut card = div().mx(px(6.)).mt(px(6.)).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
                        .flex().flex_col().gap(px(6.)).child(div().text_size(px(13.)).whitespace_normal().child(answer.clone()));
                    for (i, hit) in hits.iter().enumerate() {
                        card = card.child(self.render_hit(Entry::Answer(i), hit, true, active.as_ref(), &words, cx));
                    }
                    rows.push(card.into_any_element());
                }
                None => {}
            }
        }

        rows.push(section(tr("search_conversations")));
        let state_line = |text: String, color: Hsla| div().px(px(10.)).py(px(14.)).flex().justify_center().text_size(px(13.)).text_color(color)
            .whitespace_normal().child(text).into_any_element();
        if query.is_empty() {
            rows.push(state_line(web_with("busca_digite_todas", &[]), theme::muted()));
        } else if self.search.searching || self.search.query != query {
            rows.push(state_line(web_with("switcher_buscando", &[]), theme::muted()));
        } else if let Some(error) = self.search.error.clone() {
            rows.push(state_line(error, theme::danger()));
        } else {
            // A máquina que falhou aparece mesmo com resultados das outras, como o web.
            for (machine, reason) in &self.search.failed {
                rows.push(state_line(web_with("busca_servidor_falhou_motivo", &[("servidor", machine.label.clone()), ("motivo", reason.clone())]), theme::danger()));
            }
            if !self.search.failed.is_empty() {
                rows.push(div().flex().justify_center().pb(px(6.)).child(Button::new("search-retry").outline().small()
                    .label(web_with("busca_tentar_de_novo", &[])).on_click(cx.listener(|this, _, _, cx| this.search_retry(cx)))).into_any_element());
            }
            let hits = self.search.hits.clone();
            let groups = self.search_groups();
            let waiting = |offline: bool, key: &str| {
                let names: Vec<String> = self.search.pending.iter().filter(|m| m.offline == offline).map(|m| m.label.clone()).collect();
                (!names.is_empty()).then(|| web_with(key, &[("servidores", names.join(", "))]))
            };
            let lead = if !hits.is_empty() {
                Some(format!("{} · {}",
                    if hits.len() == 1 { web_with("busca_um_trecho", &[]) } else { web_with("busca_n_trechos", &[("n", hits.len().to_string())]) },
                    if groups.len() == 1 { web_with("busca_uma_conversa", &[]) } else { web_with("busca_n_conversas", &[("n", groups.len().to_string())]) }))
            } else if self.search.failed.is_empty() && self.search.pending.is_empty() {
                Some(web_with("busca_nenhum_todas", &[("termos", words.join(", "))]))
            } else { None };
            let status: Vec<String> = [lead, waiting(false, "busca_aguardando"), waiting(true, "busca_tentando_fora")].into_iter().flatten().collect();
            if !status.is_empty() {
                let status = status.join(" · ");
                rows.push(if hits.is_empty() { state_line(status, theme::muted()) }
                    else { div().px(px(10.)).pb(px(4.)).text_size(px(12.)).text_color(theme::faint()).whitespace_normal().child(status).into_any_element() });
            }
            for (n, (_, indexes)) in groups.iter().enumerate() {
                let first = &hits[indexes[0]];
                let title = first.live_name().map(str::to_owned).unwrap_or_else(|| first.folder());
                let mut meta = Vec::new();
                if multi { meta.push(div().child(first.label.clone())); }
                if first.live_name().is_some_and(|name| name != first.folder()) { meta.push(div().font_family(theme::MONO).child(first.folder())); }
                meta.push(div().when(first.live, |el| el.text_color(theme::success()).font_weight(FontWeight::SEMIBOLD))
                    .child(web_with(if first.live { "switcher_ativa" } else { "switcher_arquivo" }, &[])));
                if first.mtime > 0. { meta.push(div().child(ago(first.mtime))); }
                let mut group = div().flex().flex_col().gap(px(2.)).pt(px(6.)).when(n > 0, |el| el.mt(px(4.)).border_t_1().border_color(theme::border()))
                    .child(div().px(px(10.)).pb(px(2.)).flex().items_center().gap(px(8.)).min_w_0()
                        .child(div().flex_shrink(1.).min_w_0().truncate().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(div().flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(theme::faint())
                            .children(meta.into_iter().enumerate().flat_map(|(i, el)| [(i > 0).then(|| div().child("·")), Some(el)].into_iter().flatten()))));
                for i in indexes {
                    group = group.child(self.render_hit(Entry::Hit(*i), &hits[*i], false, active.as_ref(), &words, cx));
                }
                rows.push(group.into_any_element());
            }
        }
        if let Some(error) = self.search.resume_error.clone() {
            rows.insert(0, div().mx(px(6.)).mt(px(6.)).px(px(10.)).py(px(8.)).rounded(px(8.)).bg(theme::danger().alpha(0.12))
                .text_size(px(13.)).text_color(theme::danger()).whitespace_normal().child(error).into_any_element());
        }

        let header = div().h(px(52.)).flex_shrink_0().px(px(14.)).flex().items_center().gap(px(10.)).border_b_1().border_color(theme::border())
            .child(chrome::small_icon(IconName::Search, 17., theme::muted()))
            .child(div().flex_1().min_w_0()
                .capture_action(cx.listener(|this, _: &MoveUp, _, cx| { this.search_move(-1, cx); cx.stop_propagation(); }))
                .capture_action(cx.listener(|this, _: &MoveDown, _, cx| { this.search_move(1, cx); cx.stop_propagation(); }))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| { this.close_search(window, cx); cx.stop_propagation(); }))
                .child(Input::new(&input).appearance(false).aria_label(web_with("lista_buscar", &[]))))
            .child(chrome::kbd("Ctrl K"));
        let footer = div().h(px(34.)).flex_shrink_0().px(px(14.)).flex().items_center().border_t_1().border_color(theme::border())
            .text_size(px(11.5)).text_color(theme::faint()).child(web_with("busca_atalhos", &[]));
        let card = div().id("search-palette").w(width).max_h(viewport.height * 0.8).flex().flex_col().rounded(px(16.)).border_1()
            .border_color(theme::glass_border()).bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow()).overflow_hidden().occlude()
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(div().id("search-results").max_h(list_h).overflow_y_scroll().track_scroll(&self.search.scroll).px(px(6.)).pb(px(8.))
                .flex().flex_col().children(rows))
            .child(footer);
        let card = if appearance::get().surface_material == appearance::SurfaceMaterial::Glass { chrome::Glass::new(card, px(16.)).into_any_element() }
            else { card.into_any_element() };
        // Sem tecla, o agente fecha pela acessibilidade: um "Fechar" sem tamanho, fora do desenho.
        let close_search = cx.listener(|this, _: &(), window, cx| this.close_search(window, cx));
        let close = div().id("search-close").absolute().size_0().role(Role::Button).aria_label(tr("close"))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| close_search(&(), window, cx));
        Some(deferred(div().id("search-overlay").role(Role::Dialog).aria_label(web_with("lista_buscar", &[]))
            .absolute().inset_0().bg(cx.theme().overlay).occlude()
            .on_any_mouse_down(cx.listener(|this, _, window, cx| { this.close_search(window, cx); cx.stop_propagation(); }))
            .flex().items_start().justify_center().pt(viewport.height / 10.).child(close).child(card))
            .with_priority(gpui_kit::base::POPUP_PRIORITY + 1).into_any_element())
    }

    /// Linha escolhível da paleta: o ponteiro que se move sobre ela leva o destaque (teclado e mouse nunca acendem duas).
    fn search_row(&self, id: String, on: bool, entry: Entry, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover_entry = entry.clone();
        div().id(SharedString::from(id)).w_full().min_h(px(40.)).px(px(10.)).py(px(6.)).rounded(px(8.)).flex().items_center().gap(px(10.))
            .cursor_pointer().when(on, |el| el.bg(theme::accent_dim()))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if let Some(ix) = this.search_entries(cx).iter().position(|e| *e == hover_entry).filter(|ix| *ix != this.search.active) {
                    this.search.active = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.search_click(entry.clone(), window, cx)))
    }

    /// Clique: a sessão viva abre na hora; a conversa arquivada abre a prévia, e retomar (que cria uma sessão) fica no botão
    /// dela ou no Enter.
    fn search_click(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        match self.hit_of(&entry).filter(|hit| hit.live_name().is_none()) {
            Some(hit) => self.search_toggle_preview(hit, cx),
            None => self.search_activate(Some(entry), window, cx),
        }
    }

    fn render_hit(&self, entry: Entry, hit: &Hit, with_folder: bool, active: Option<&Entry>, words: &[String], cx: &mut Context<Self>) -> AnyElement {
        let on = active == Some(&entry);
        let key = hit.key();
        let open = self.search.preview.as_ref().filter(|(k, _)| *k == key);
        let mine = hit.role.as_deref() == Some("user");
        let resuming = self.search.resuming.as_deref() == Some(hit.conversation().as_str());
        let who = div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(if mine { theme::accent() } else { theme::muted() })
            .child(web_with(if mine { "busca_voce" } else { "busca_assistente" }, &[]));
        let preview_hit = hit.clone();
        let head = div().w_full().flex().items_center().gap(px(8.)).child(who)
            .when(with_folder, |el| el.child(div().truncate().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::faint())
                .child(hit.live_name().map(str::to_owned).unwrap_or_else(|| hit.folder()))))
            .child(div().flex_1())
            .children(hit.ts.or(Some(hit.mtime)).filter(|t| *t > 0.).map(|t| div().text_size(px(12.)).text_color(theme::faint()).child(ago(t))))
            .child(Button::new(SharedString::from(format!("search-preview-{key}"))).ghost().xsmall()
                .icon(if open.is_some() { IconName::ChevronDown } else { IconName::ChevronRight })
                .tooltip(web_with("busca_carregando_contexto", &[]).trim_end_matches('…').to_owned())
                .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.search_toggle_preview(preview_hit.clone(), cx); })));
        let line: String = hit.line.chars().take(400).collect();
        let row = self.search_row(format!("search-hit-{}-{key}", matches!(entry, Entry::Answer(_))), on, entry.clone(), cx)
            .flex_col().items_start().gap(px(3.))
            .child(head)
            .child(div().w_full().text_size(px(13.)).line_height(relative(1.45)).text_color(theme::text()).whitespace_normal()
                .child(highlighted(line, words)))
            .when(resuming, |el| el.child(div().text_size(px(12.)).text_color(theme::muted()).child(tr("search_resuming"))));
        let Some((_, preview)) = open else { return row.into_any_element() };
        let body = match &preview.value {
            None => div().text_size(px(12.5)).text_color(theme::muted()).child(web_with("busca_carregando_contexto", &[])),
            Some(Err(error)) => div().text_size(px(12.5)).text_color(theme::danger()).child(error.clone()),
            Some(Ok(events)) => div().flex().flex_col().gap(px(10.)).children(events.iter().map(|ev| {
                let target = hit.event_id.as_deref() == Some(ev.id.as_str());
                let mine = ev.kind == "user_msg";
                let text: String = ev.text.clone().unwrap_or_default().chars().take(1200).collect();
                div().pl(px(10.)).border_l_2().border_color(if target { theme::accent() } else { theme::border() }).flex().flex_col().gap(px(3.))
                    .child(div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(if mine { theme::accent() } else { theme::muted() })
                        .child(web_with(if mine { "busca_voce" } else { "busca_assistente" }, &[])))
                    .child(div().text_size(px(12.5)).text_color(if target { theme::text() } else { theme::muted() }).whitespace_normal().child(text))
            })),
        };
        let action_hit = hit.clone();
        let label = if hit.live_name().is_some() { web_with("busca_abrir_sessao", &[]) } else { tr("search_resume") };
        div().flex().flex_col().rounded(px(8.)).bg(theme::inset()).border_1().border_color(theme::border())
            .child(row)
            .child(div().px(px(12.)).pb(px(12.)).flex().flex_col().gap(px(10.)).child(body)
                .child(div().flex().child(Button::new(SharedString::from(format!("search-open-{key}"))).primary().small().label(label)
                    .loading(resuming).disabled(self.search.resuming.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        match action_hit.live_name() {
                            Some(name) => { let target = Target::new(&action_hit.server, name); this.search_open_session(&target, window, cx) }
                            None => this.search_resume(action_hit.clone(), window, cx),
                        }
                    })))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Hit, Search, marks, merge_hits, merge_keeping, terms};

    fn hit(server: &str, line: &str, mtime: f64) -> Hit {
        Hit { project: "p".into(), session_id: "s".into(), line: line.into(), mtime, server: server.into(), ..Default::default() }
    }

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
        let mut search = Search { debounce: 7, query: "x".into(), hits: vec![hit("a", "x", 1.)], ..Default::default() };
        search.reopen(None);
        assert!(search.debounce > 7, "resposta da abertura anterior casaria com a busca nova");
        assert!(search.open && search.query.is_empty() && search.hits.is_empty());
    }

    #[test]
    fn late_machine_merges_without_repeats_newest_first() {
        let mut hits = Vec::new();
        merge_hits(&mut hits, vec![hit("a", "x", 1.), hit("a", "x", 1.)]);
        merge_hits(&mut hits, vec![hit("b", "x", 3.), hit("a", "y", 2.)]);
        let keys: Vec<String> = hits.iter().map(Hit::key).collect();
        assert_eq!(keys, ["b/p/s/x", "a/p/s/y", "a/p/s/x"], "repetido viraria dois elementos com o mesmo id");
    }

    #[test]
    fn the_same_conversation_on_two_machines_stays_apart() {
        let hit = |server: &str| Hit { project: "p".into(), session_id: "s".into(), line: "x".into(), server: server.into(), ..Default::default() };
        assert_ne!(hit("http://a:8765").conversation(), hit("http://b:8765").conversation(), "agrupar juntaria trechos de máquinas diferentes");
        assert_ne!(hit("http://a:8765").key(), hit("http://b:8765").key(), "a prévia aberta de uma abriria a da outra");
    }

    #[test]
    fn highlights_every_term_without_overlaps() {
        let words = terms("Hangar  hangar busca");
        assert_eq!(words, ["hangar", "busca"]);
        assert_eq!(marks("O Hangar busca no hangar", &words), vec![2..8, 9..14, 18..24]);
        assert!(marks("ÁRVORE", &terms("árvore")).is_empty() || marks("ÁRVORE", &terms("árvore")) == vec![0..7]);
    }
}
