//! Painel direito da sessão: estado, contexto, limites, projeto e atalhos. Tudo lido de fontes do
//! backend (stream, lista, rotas); nenhuma ação sai sem gesto, e desconhecido aparece como tal.
use super::*;
use crate::status::StatusFields;
use crate::appearance::SideTab;

const MIN_WIDTH: f32 = 240.;
// Navegador, como no web: abaixo de 400 uma página não serve, e nasce com 42% da janela.
const BROWSER_MIN: f32 = 400.;
const BROWSER_SHARE: f32 = 0.42;
// Caixa solta com o painel aberto: 10 de margem em cada lado da janela e os dois vãos de 10 entre as três caixas.
const FLOATING_GAPS: f32 = 40.;
// Largura que a conversa mantém; abaixo disso o painel sai de cena em vez de espremer o texto.
const CHAT_MIN: f32 = 400.;
// A borda do cartão fica DENTRO da largura pedida (o layout mede por border-box), e o meio pixel da
// janela some no arredondamento. Quem divide a linha em colunas exatas tem de descontar os dois: um
// pixel a mais na conta derruba a última coluna para a linha de baixo.
const PANEL_EDGE: f32 = 3.;
const COST_EVERY: u64 = 30;
const DIFF_MAX: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shortcut {
    Send { label: String, text: String, direct: bool, confirm: bool, icon: Option<String> },
    /// `key` é a identidade do atalho (`global:<id>` ou `project:<repo>:<id>`): com ela o backend acha o terminal dele;
    /// `hangar` roda uma cópia só do servidor (na home se `home`), e `ask` mostra a pergunta do terminal no app.
    Shell { label: String, command: String, confirm: bool, icon: Option<String>, pasta: Option<String>, key: String, hangar: bool, home: bool, ask: bool },
    Attach,
    Run,
    Terminal,
    Mode,
    Browser,
    External,
}

impl Shortcut {
    fn confirm(&self) -> bool { matches!(self, Shortcut::Send { confirm: true, .. } | Shortcut::Shell { confirm: true, .. }) }
    pub(super) fn label(&self) -> String {
        match self {
            Shortcut::Send { label, .. } | Shortcut::Shell { label, .. } => label.clone(),
            Shortcut::Attach => tr("attach"),
            Shortcut::Run => tr("shortcuts_native_rodar"),
            Shortcut::Terminal => tr("shortcuts_native_terminal"),
            Shortcut::Mode => tr("shortcuts_native_modo"),
            Shortcut::Browser => tr("shortcuts_native_navegador"),
            Shortcut::External => tr("shortcuts_native_externo"),
        }
    }

    /// Credencial que a importação deixou em branco: o atalho não roda até alguém preencher.
    fn missing_secret(&self) -> Option<String> {
        match self {
            Shortcut::Send { text, .. } => super::shortcut_transfer::missing_secret(text),
            Shortcut::Shell { command, .. } => super::shortcut_transfer::missing_secret(command),
            _ => None,
        }
    }

    /// O que o painel roda de um atalho da config. `project` é a chave do repositório quando o atalho é do projeto.
    pub(super) fn from_item(item: &shortcuts::Item, project: Option<&str>) -> Option<Self> {
        let (label, icon, confirm) = (item.label().to_owned(), item.icon().map(str::to_owned), item.confirm());
        match item.kind() {
            "send_text" => Some(Shortcut::Send { label, text: item.content().to_owned(), direct: item.sends_direct(), confirm, icon }),
            "shell" => Some(Shortcut::Shell { label, command: item.content().to_owned(), confirm, icon, pasta: item.pasta().map(str::to_owned),
                key: super::hangar_live::shortcut_key(project, item.id()), hangar: item.runs_in_hangar(), home: item.hangar_home(), ask: item.answer_in_app() }),
            "internal" if item.action() == "rodar" => Some(Shortcut::Run),
            // Os outros internos só viram bloco quando marcados nas configurações.
            "internal" if !item.tile() => None,
            "internal" if item.action() == "anexos" => Some(Shortcut::Attach),
            "internal" if item.action() == "terminal" => Some(Shortcut::Terminal),
            "internal" if item.action() == "modo" => Some(Shortcut::Mode),
            "internal" if item.action() == "navegador" => Some(Shortcut::Browser),
            "internal" if item.action() == "externo" => Some(Shortcut::External),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct GitFile { path: String, code: String, added: Option<i64>, removed: Option<i64> }

#[derive(Clone, Debug)]
struct Cost { usd: Option<f64>, has_usage: bool, missing: Vec<String> }

/// `GET/PUT /project-shortcuts`: nome da pasta do projeto e os atalhos dele.
#[derive(Clone, Debug)]
pub(super) struct Project {
    pub(super) name: String,
    /// Repositório git do projeto: identidade dos atalhos dele No Hangar.
    pub(super) key: String,
    pub(super) items: Vec<shortcuts::Item>,
    /// A lista como veio: o PUT regrava a inteira, e o que a tela não mostra não pode sumir dela.
    pub(super) raw: Vec<Value>,
}

impl Project {
    pub(super) fn parse(value: &Value) -> Self {
        Self { name: value.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
            key: value.get("key").and_then(Value::as_str).unwrap_or("").to_owned(), items: shortcuts::project_items(value.get("items")),
            raw: value.get("items").and_then(Value::as_array).cloned().unwrap_or_default() }
    }
}

/// Atalhos do projeto da sessão aberta. Troca de sessão ou de servidor descarta leitura e gravação em voo pelos números.
#[derive(Default)]
pub(super) struct ProjectShortcuts {
    pub(super) owner: Option<SessionKey>,
    pub(super) list: super::device::Remote<Project>,
    /// Número da gravação em voo: outra só sai quando ela volta.
    pub(super) saving: Option<u64>,
    pub(super) save_seq: u64,
    pub(super) save_error: Option<String>,
}

impl ProjectShortcuts {
    fn reset(&mut self) {
        self.list.reset();
        (self.owner, self.saving, self.save_error) = (None, None, None);
    }

    /// A lista lida desta sessão; `None` enquanto carrega, com erro ou de outra sessão.
    pub(super) fn of(&self, key: Option<&SessionKey>) -> Option<&Project> {
        self.list.ok().filter(|_| key.is_some() && self.owner.as_ref() == key)
    }
}

pub(super) struct Side {
    pub open: bool,
    /// O corpo mostra o menu de ferramentas: aberto pelo "+" das abas, fecha ao escolher uma linha, no "+" ou com Esc.
    pub(super) menu: bool,
    width: f32,
    /// A aba Navegador mantém uma largura própria e nasce com uma fração da janela.
    browser_width: Option<f32>,
    /// (x do início, largura do início, espaço que sobra para o painel).
    drag: Option<(f32, f32, f32)>,
    shortcuts: Option<Result<Vec<Shortcut>, String>>,
    pub(super) project: ProjectShortcuts,
    // Custo do Codex: o último valor fica visível quando uma leitura falha; o erro vai junto.
    cost: Option<(SessionKey, Option<Cost>, Option<String>)>,
    cost_task: Option<(SessionKey, JoinHandle<()>)>,
    cost_gen: u64,
    /// A aba Orquestração da sessão `orq` aberta.
    pub(super) orq: super::orq_panel::State,
    files: Option<(SessionKey, Option<Result<Vec<GitFile>, String>>)>,
    diff: Option<(SessionKey, String, Option<Result<(String, bool), String>>)>,
    reloading: HashSet<SessionKey>,
    run_code_pending: HashSet<SessionKey>,
    /// A aba Git da sessão aberta (dono = `session_owner`).
    pub(super) git: Option<(SessionOwner, Entity<super::git::GitPanel>)>,
    /// Terminais dos atalhos shell por nome de sessão, e a aba que o painel deve trazer pra frente.
    pub(super) shortcut_terms: HashMap<String, Vec<super::terminal::ShortcutTerm>>,
    pub(super) shortcut_focus: HashMap<String, String>,
    /// Aviso "rodando" do último atalho por sessão: (id do terminal, texto). Sai quando o terminal fecha ou morre.
    pub(super) shortcut_running: HashMap<String, (String, String)>,
    pub(super) shortcut_recheck: HashMap<String, std::time::Instant>,
    /// Há um run vivo no projeto desta sessão (botão Rodar aceso).
    pub(super) run: Option<(SessionKey, bool)>,
    /// No Windows, um navegador por sessão (chave `servidor::sessão`), como o do Electron, para o hangar-preview
    /// dirigir o da sessão certa. Nos outros sistemas a chave é uma só: no Linux um segundo motor não nasce no processo.
    pub(super) browsers: HashMap<String, Entity<super::browser::BrowserPanel>>,
    /// Sessões (chave de `browser_key`) com a aba Navegador na fileira. Fechar só esconde a página.
    pub(super) browser_open: HashSet<String>,
    /// Aba escolhida por sessão (chave de `browser_key`); sessão sem escolha usa a última preferência gravada.
    pub(super) tabs: HashMap<String, SideTab>,
}

impl Default for Side {
    fn default() -> Self {
        let saved = appearance::get();
        Self { open: true, menu: false, width: saved.side_width, browser_width: saved.side_browser_width, drag: None, shortcuts: None, project: ProjectShortcuts::default(), cost: None, cost_task: None, cost_gen: 0, orq: Default::default(),
            files: None, diff: None, reloading: HashSet::new(), run_code_pending: HashSet::new(), git: None, run: None, browsers: HashMap::new(), browser_open: HashSet::new(), tabs: HashMap::new(),
            shortcut_terms: HashMap::new(), shortcut_focus: HashMap::new(), shortcut_running: HashMap::new(), shortcut_recheck: HashMap::new() }
    }
}

impl Side {
    pub fn reset_server(&mut self) {
        self.shortcuts = None;
        self.stop_cost();
        self.cost = None;
        self.orq.reset();
        self.on_select();
        self.reloading.clear();
        self.run_code_pending.clear();
        self.shortcut_terms.clear();
        self.shortcut_focus.clear();
        self.shortcut_running.clear();
        self.shortcut_recheck.clear();
    }

    pub fn on_select(&mut self) {
        self.project.reset();
        self.files = None;
        self.diff = None;
        self.git = None;
    }

    fn stop_cost(&mut self) {
        if let Some((_, task)) = self.cost_task.take() { task.abort(); }
        self.cost_gen += 1;
    }

    /// A lista que a página Atalhos leu ou gravou: o painel mostra na hora, sem reler a config.
    pub(super) fn set_shortcuts(&mut self, items: &[shortcuts::Item]) {
        self.shortcuts = Some(Ok(items.iter().filter_map(|item| Shortcut::from_item(item, None)).collect()));
    }

    pub fn shortcuts_failed(&self) -> bool { matches!(self.shortcuts, Some(Err(_))) }

    pub fn receive_config(&mut self, result: Result<Value, String>) {
        self.shortcuts = Some(result.map(|config| parse_shortcuts(
            config.pointer("/campos/shortcuts/valor").and_then(Value::as_str).unwrap_or(""))));
    }

    // Largura efetiva: nunca tira da conversa menos que CHAT_MIN; sem espaço, o painel não aparece.
    // Com abas não há barra lateral ocupando a esquerda.
    pub(super) fn fitted(&self, viewport: f32, floating: bool, sidebar_width: f32, browser: bool) -> Option<f32> {
        let room = Self::room(viewport, floating, sidebar_width);
        (room >= MIN_WIDTH).then(|| if browser {
            self.browser_width.unwrap_or((viewport * BROWSER_SHARE).round()).max(BROWSER_MIN).min(room)
        } else {
            self.width.max(MIN_WIDTH).min(room)
        })
    }

    fn room(viewport: f32, floating: bool, sidebar_width: f32) -> f32 {
        viewport - sidebar_width - CHAT_MIN - if floating { FLOATING_GAPS } else { 0. }
    }
}

/// A resolução do web (`shortcuts::resolve`), reduzida ao que o painel nativo roda.
fn parse_shortcuts(raw: &str) -> Vec<Shortcut> { shortcuts::resolve(raw).iter().filter_map(|item| Shortcut::from_item(item, None)).collect() }

/// Blocos do painel: os globais e depois os do projeto (`project_key` = repositório dele), com o id do bloco e se é do
/// projeto. O id do global é a posição e o do projeto leva o id do item: o mesmo id nas duas listas não colide.
fn merged_tiles(globals: &[Shortcut], project: &[shortcuts::Item], project_key: &str) -> Vec<(String, Shortcut, bool)> {
    let globals = globals.iter().enumerate().map(|(n, s)| (format!("shortcut-{n}"), s.clone(), false));
    let own = project.iter().filter_map(|item| Some((format!("shortcut-p-{}", item.id()), Shortcut::from_item(item, Some(project_key))?, true)));
    globals.chain(own).collect()
}

/// Tokens como o painel web: milhar arredondado em "k", milhão com uma casa, menos de mil cru.
pub(super) fn tokens(n: f64) -> String {
    if n >= 1e6 { format!("{}M", trim_zero(format!("{:.1}", (n / 1e5).round() / 10.))) }
    else if n >= 1e3 { format!("{}k", (n / 1e3).round()) }
    else { format!("{}", n.round()) }
}

fn trim_zero(s: String) -> String { s.strip_suffix(".0").map(str::to_owned).unwrap_or(s) }

fn duration(ms: f64) -> String {
    let s = ms / 1000.;
    if s < 10. { format!("{s:.1}s") } else if s < 60. { format!("{}s", s.round()) }
    else if s < 3600. { format!("{}m{:02}s", (s / 60.).floor(), (s % 60.).floor()) }
    else { format!("{}h{:02}m", (s / 3600.).floor(), ((s % 3600.) / 60.).floor()) }
}

pub(super) fn ago(seconds: f64) -> String {
    let s = seconds.max(0.);
    if s < 60. { tr("ago_now") }
    else if s < 3600. { tr("ago_min").replace("{n}", &(s / 60.).floor().to_string()) }
    else if s < 86_400. { tr("ago_h").replace("{n}", &(s / 3600.).floor().to_string()) }
    else { tr("ago_d").replace("{n}", &(s / 86_400.).floor().to_string()) }
}

/// Tempo curto da lista ("agora", "2m", "1h", "3d"), a partir do instante da última atividade.
pub(super) fn since(at: f64) -> String {
    let s = (now_seconds() - at).max(0.);
    if s < 60. { tr("since_now") }
    else if s < 3600. { format!("{}m", (s / 60.).floor()) }
    else if s < 86_400. { format!("{}h", (s / 3600.).floor()) }
    else { format!("{}d", (s / 86_400.).floor()) }
}

pub(super) fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.)
}

/// Aviso do painel (`.aviso` do web): alerta em âmbar, grave em vermelho.
fn notice(text: String, serious: bool) -> Div {
    let color = if serious { theme::danger() } else { theme::warning() };
    div().flex().flex_col().gap_2().px_3().py_2().rounded(px(12.)).border_1().border_color(color.opacity(0.34)).bg(color.opacity(0.10))
        .child(div().flex().items_start().gap_2()
            .child(chrome::small_icon(IconName::Info, 15., color))
            .child(div().flex_1().min_w_0().text_size(px(11.)).text_color(theme::muted()).child(text)))
}

fn notice_button(id: &'static str, label: String, serious: bool, cx: &App) -> Button {
    let color = if serious { theme::danger() } else { theme::warning() };
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(color).hover(color.opacity(0.14)).active(color.opacity(0.2)))
        .xsmall().rounded_full().border_1().border_color(color.opacity(0.45)).px(px(10.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).child(label))
}

pub(super) fn agent_label(provider: &str) -> String {
    let mut chars = provider.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// O uso da sessão em pares rótulo e valor, na ordem da grade do cartão de contexto; o que não foi medido fica fora.
pub(super) fn stats_cells(stats: &Stats) -> Vec<(String, String)> {
    let mut cells = vec![
        (tr("ctx_card_turns"), stats.turns.to_string()),
        (tr("ctx_card_calls"), stats.steps.to_string()),
        // Entrada só com o que não veio do cache: o lido do cache aparece na linha Cache.
        (tr("ctx_card_in"), format!("{} tok", tokens(stats.in_tok.saturating_sub(stats.cache_read_tok) as f64))),
        (tr("ctx_card_out"), format!("{} tok", tokens(stats.out_tok as f64))),
    ];
    if let Some(ms) = stats.llm_ms.filter(|v| *v > 0.) {
        cells.push((tr("ctx_card_llm"), duration(ms)));
        if let Some(tool) = stats.tool_ms.filter(|v| *v > 0.) { cells.push((tr("ctx_card_tools"), duration(tool))); }
    }
    if let Some(rate) = stats.tok_s.filter(|v| *v > 0.) { cells.push((tr("ctx_card_rate"), tr("stats_rate").replace("{n}", &rate.round().to_string()))); }
    // Sem "~" quando a medida veio do stream da resposta, não do transcript.
    let rate = |v: f64| tr(if stats.tok_s_exact { "stats_rate_exact" } else { "stats_rate" }).replace("{n}", &v.round().to_string());
    if let Some(v) = stats.tok_s_now.filter(|v| *v > 0.) { cells.push((tr("ctx_card_rate_now"), rate(v))); }
    if let Some(v) = stats.tok_s_recent.filter(|v| *v > 0.) { cells.push((tr("ctx_card_rate_recent"), rate(v))); }
    if let Some(ms) = stats.ttft_ms.filter(|v| *v > 0.) { cells.push((tr("ctx_card_ttft"), format!("~{}", duration(ms)))); }
    if let Some(cache) = stats.cache_pct {
        let value = if stats.cache_read_tok > 0 { format!("{} tok · {}%", tokens(stats.cache_read_tok as f64), cache.round()) } else { format!("{}%", cache.round()) };
        cells.push((tr("ctx_card_cache"), value));
    }
    cells
}

impl Hangar {
    pub(super) fn toggle_side(&mut self, cx: &mut Context<Self>) {
        self.side.open = !self.side.open;
        // Abre nas abas; o menu de ferramentas fica atrás do "+".
        self.side.menu = false;
        if !self.side.open { self.side.stop_cost(); }
        self.sync_activity(cx);
        cx.notify();
    }

    /// Esc com o menu à vista volta à aba de antes.
    pub(super) fn side_menu_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.side.open || !self.side_menu_shown() { return false; }
        self.side.menu = false;
        self.sync_activity(cx);
        cx.notify();
        true
    }

    pub(super) fn drag_side(&mut self, x: f32, pressed: bool, cx: &mut Context<Self>) {
        let Some((start_x, start_width, room)) = self.side.drag else { return; };
        if !pressed { return self.end_drag(cx); }
        let wanted = start_width + start_x - x;
        // Preso ao espaço de agora: arrastar além dele não acumula largura que depois teria de ser desfeita.
        if self.side_browser() { self.side.browser_width = Some(wanted.clamp(BROWSER_MIN.min(room), room.max(MIN_WIDTH))); }
        else { self.side.width = wanted.clamp(MIN_WIDTH, room.max(MIN_WIDTH)); }
        cx.notify();
    }

    pub(super) fn side_dragging(&self) -> bool { self.side.drag.is_some() }

    pub(super) fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.side.drag.take().is_none() { return; }
        // Grava só ao soltar: durante o arrasto a largura vive no `Side`.
        let mut next = appearance::get();
        (next.side_width, next.side_browser_width) = (self.side.width, self.side.browser_width);
        self.apply_appearance(next, true, cx);
    }

    // Custo do Codex: só com o painel visível e a sessão aberta; troca de sessão cancela a leitura em curso.
    fn sync_cost(&mut self, visible: bool) {
        let want = self.selected_key().filter(|_| visible && self.provider().0 == "codex" && self.chat_online && !self.open_read_only());
        if self.side.cost_task.as_ref().map(|(key, _)| key) == want.as_ref() { return; }
        self.side.stop_cost();
        let (Some(key), Some(api)) = (want, self.session_api()) else { return; };
        if self.side.cost.as_ref().is_some_and(|(owner, ..)| owner != &key) { self.side.cost = None; }
        let (connection, tx, generation, name) = (self.connection, self.tx.clone(), self.side.cost_gen, key.name.clone());
        let owner = key.clone();
        let task = self.runtime.spawn(async move {
            loop {
                let result = api.read(&name, &["cost"], &[], 25).await;
                if tx.send(Envelope { connection, selection: None, payload: Payload::Reply(owner.clone(), Reply::Cost(generation), result) }).await.is_err() { return; }
                tokio::time::sleep(Duration::from_secs(COST_EVERY)).await;
            }
        });
        self.side.cost_task = Some((key, task));
    }

    pub(super) fn load_files(&mut self, cx: &mut Context<Self>) {
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        self.side.files = Some((key.clone(), None));
        self.side.diff = None;
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["git", "files"], &[], 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::GitFiles, result) }).await;
        });
        cx.notify();
    }

    /// Atalhos do projeto da sessão aberta: lidos ao escolher a sessão e ao abrir a página Atalhos.
    pub(super) fn load_project_shortcuts(&mut self) {
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        let project = &mut self.side.project;
        if project.owner.as_ref() != Some(&key) { project.reset(); project.owner = Some(key.clone()); }
        let seq = project.list.start();
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.read(&key.name, &["project-shortcuts"], &[], 15).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::ProjectShortcuts(seq), result) }).await;
        });
    }

    // POST só de leitura: o backend confere que o caminho está na lista de alterados.
    fn open_diff(&mut self, path: String, cx: &mut Context<Self>) {
        if self.open_read_only() { return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        if self.side.diff.as_ref().is_some_and(|(owner, current, _)| owner == &key && current == &path) { self.side.diff = None; cx.notify(); return; }
        self.side.diff = Some((key.clone(), path.clone(), None));
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["git", "diff"], Some(json!({"path": path})), false, 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Diff(path), result) }).await;
        });
        cx.notify();
    }

    pub(super) fn prefill(&mut self, text: &str, protect: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.composer.read(cx).value().to_string();
        if protect && !current.trim().is_empty() && current != text {
            self.confirm = Some(Confirm::Prefill(text.to_owned()));
            cx.notify();
            return;
        }
        self.composer.update(cx, |input, cx| { input.set_value(text.to_owned(), window, cx); input.focus(window, cx); });
        cx.notify();
    }

    pub(super) fn run_shortcut(&mut self, shortcut: Shortcut, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        if let Some(name) = shortcut.missing_secret() {
            self.action_feedback.insert(key, (tr("shortcut_secret_missing").replace("{name}", &name), true));
            cx.notify();
            return;
        }
        if shortcut.confirm() && !confirmed {
            self.confirm = Some(Confirm::Shortcut(shortcut.label(), shortcut));
            cx.notify();
            return;
        }
        match shortcut {
            Shortcut::Attach => self.pick_files(cx),
            Shortcut::Run => self.open_run(window, cx),
            Shortcut::Terminal => self.show_terminal(window, cx),
            Shortcut::Browser => self.open_browser(window, cx),
            Shortcut::External => self.open_external_terminal(window, cx),
            Shortcut::Mode => if let Some(target) = self.selected_target() { self.confirm_mode(target, window, cx) },
            Shortcut::Send { text, direct: false, .. } => self.prefill(&text, true, window, cx),
            Shortcut::Send { text, .. } => {
                if !self.can_send() || self.delivery.pending(&key) || self.uploading.contains_key(&key) {
                    self.action_feedback.insert(key, (tr("shortcut_busy"), true));
                } else {
                    let known = self.known_user_ids();
                    self.deliver(key, text, String::new(), false, known, None, cx);
                }
            }
            // Sempre pelo backend, também com a sessão nesta máquina: é ele quem cria o terminal escondido que vira
            // aba do painel, onde dá pra ver a saída e fechar o programa.
            Shortcut::Shell { label, command, pasta, key: shortcut_key, hangar, home, ask, .. } => {
                let Some(api) = self.api_for(&key.server) else { return; };
                // Terminal desse atalho perguntando: o clique abre a pergunta em vez de rodar outro.
                if let Some(asking) = self.asking_term(&key.server, &shortcut_key, hangar, &key.name) {
                    self.open_question(&key.server, &asking.owner, &asking.term.id, window, cx);
                    return;
                }
                self.action_feedback.insert(key.clone(), (tr("shortcut_started").replace("{label}", &label), false));
                let (connection, tx) = (self.connection, self.tx.clone());
                let mut body = json!({"command": command, "label": label, "ask": ask, "key": shortcut_key});
                if let Some(pasta) = pasta { body["pasta"] = json!(pasta); }
                if hangar { body["runs_in"] = json!("hangar"); body["home"] = json!(home); }
                self.runtime.spawn(async move {
                    let result = api.act(&key.name, &["shortcut-shell"], Some(body), false, 30).await;
                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Shell(label, hangar), result) }).await;
                });
            }
        }
        cx.notify();
    }

    pub(super) fn run_code_command(&mut self, code: String, language: Option<String>, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return; };
        let command = code.trim();
        if command.is_empty() || command.len() > 4096 || command.contains('\0') {
            self.action_feedback.insert(key, (tr("code_run_invalid"), true));
            cx.notify();
            return;
        }
        if !self.side.run_code_pending.insert(key.clone()) { return; }
        let Some(api) = self.api_for(&key.server) else {
            self.side.run_code_pending.remove(&key);
            self.action_feedback.insert(key, (tr("term_disconnected"), true));
            cx.notify();
            return;
        };
        self.action_feedback.insert(key.clone(), (tr("code_run_starting"), false));
        let (connection, tx) = (self.connection, self.tx.clone());
        let request_key = format!("run-code:{}", servers::new_id());
        let body = json!({"command": command, "language": language, "key": request_key.clone()});
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["run-code"], Some(body), false, 30).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::RunCode(request_key), result) }).await;
        });
        cx.notify();
    }

    fn reload_allowed(&self) -> bool {
        self.chat_online && self.chat.state.state == "idle"
            && self.selected_key().is_some_and(|key| !self.side.reloading.contains(&key))
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.reload_allowed() { cx.notify(); return; }
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return; };
        self.side.reloading.insert(key.clone());
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &["recarregar"], None, false, 60).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Reply(key, Reply::Reload, result) }).await;
        });
        cx.notify();
    }

    fn read_failure(error: &Failure) -> String {
        if error.uncertain && error.status.is_none() { tr("network_error") } else { Self::fetch_failure(error) }
    }

    pub(super) fn receive_reply(&mut self, key: SessionKey, reply: Reply, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            Reply::Cost(generation) => {
                if generation != self.side.cost_gen { return; }
                let previous = self.side.cost.take().filter(|(owner, ..)| owner == &key).and_then(|(_, cost, _)| cost);
                self.side.cost = Some(match result {
                    Ok(value) => (key, Some(Cost {
                        usd: value.get("cost_usd").and_then(Value::as_f64),
                        has_usage: value.get("has_usage").and_then(Value::as_bool).unwrap_or(false),
                        missing: value.get("missing_models").and_then(Value::as_array)
                            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
                    }), None),
                    Err(error) => (key, previous, Some(Self::failure(&error))),
                });
            }
            Reply::OrqPanel(generation) => self.receive_orq_panel(generation, key, result),
            Reply::GitFiles => {
                let Some((owner, slot)) = self.side.files.as_mut() else { return; };
                if owner != &key { return; }
                *slot = Some(result.map_err(|error| Self::failure(&error)).map(|value| {
                    let mut files: Vec<GitFile> = value.get("files").and_then(Value::as_array).map(|list| list.iter().filter_map(|f| Some(GitFile {
                        path: f.get("path")?.as_str()?.to_owned(),
                        code: f.get("code").and_then(Value::as_str).unwrap_or("").trim().to_owned(),
                        added: f.get("added").and_then(Value::as_i64),
                        removed: f.get("removed").and_then(Value::as_i64),
                    })).collect()).unwrap_or_default();
                    files.sort_by_key(|f| std::cmp::Reverse(f.added.unwrap_or(0) + f.removed.unwrap_or(0)));
                    files
                }));
            }
            Reply::Diff(path) => {
                let Some((owner, current, slot)) = self.side.diff.as_mut() else { return; };
                if owner != &key || current != &path { return; }
                *slot = Some(match result {
                    Ok(value) => Ok((value.get("diff").and_then(Value::as_str).unwrap_or("").to_owned(), value.get("truncated").and_then(Value::as_bool).unwrap_or(false))),
                    Err(error) => Err(Self::read_failure(&error)),
                });
            }
            Reply::Shell(label, hangar) => {
                // A aba do terminal vai pra frente (sem abrir o painel); a que falhou também, com a saída inteira.
                let terminal = result.as_ref().ok().and_then(|value| value.pointer("/terminal/id")).and_then(Value::as_str).map(str::to_owned);
                if hangar {
                    self.receive_hangar_shell(key, label, terminal, result, window, cx);
                    return;
                }
                // Só o 422 deixa terminal para trás; sem ele, a aba "mais nova" seria a de outro atalho.
                let failed_with_terminal = result.as_ref().err().is_some_and(|error| error.status == Some(422));
                if terminal.is_some() || failed_with_terminal {
                    self.side.shortcut_focus.insert(key.name.clone(), terminal.clone().unwrap_or_default());
                }
                self.refresh_shortcut_terms(&key.name);
                let note = match result {
                    Ok(_) => {
                        let text = tr("shortcut_launched").replace("{label}", &label);
                        if let Some(id) = terminal { self.side.shortcut_running.insert(key.name.clone(), (id, text.clone())); }
                        (text, false)
                    }
                    Err(error) if matches!(error.status, Some(404 | 405)) => (tr("shortcut_shell_unsupported"), true),
                    Err(error) => (format!("{label}: {}", Self::failure(&error)), true),
                };
                self.action_feedback.insert(key, note);
            }
            Reply::RunCode(request_key) => {
                self.side.run_code_pending.remove(&key);
                let may_have_terminal = result.is_ok() || result.as_ref().err().is_some_and(|error| error.status == Some(422) || error.status.is_some_and(|status| status >= 500) || error.uncertain);
                if may_have_terminal && self.selected_key().as_ref() == Some(&key) {
                    self.read_run_code_terms(key.clone(), request_key, 0);
                }
                let note = match result {
                    Ok(_) => (tr("code_run_started"), false),
                    Err(error) if error.status == Some(404) => (tr("code_run_update_server"), true),
                    Err(error) => (Self::failure(&error), true),
                };
                self.action_feedback.insert(key, note);
            }
            Reply::RunState => self.receive_run_state(key, result),
            Reply::ProjectShortcuts(seq) => {
                let project = &mut self.side.project;
                if project.owner.as_ref() != Some(&key) { return; }
                project.list.finish(seq, result.map(|value| Project::parse(&value)).map_err(|error| Self::read_failure(&error)));
            }
            Reply::ProjectSaved(seq) => self.receive_project_saved(key, seq, result, window, cx),
            Reply::Reload => {
                self.side.reloading.remove(&key);
                let note = match result { Ok(_) => (tr("reload_sent"), false), Err(error) => (Self::failure(&error), true) };
                self.action_feedback.insert(key, note);
            }
            other => self.receive_control(key, other, result, window, cx),
        }
    }

    fn loop_text(&self) -> Option<String> {
        let state = &self.chat.state;
        let session = self.selected.as_ref()?;
        let status = state.loop_status.clone().or_else(|| session.loop_status.clone())?;
        let iter = state.loop_iter.or(session.loop_iter);
        let max = state.loop_max.or(session.loop_max);
        let count = match (iter, max) { (Some(i), Some(m)) => format!(" {i}/{m}"), (Some(i), None) => format!(" {i}"), _ => String::new() };
        let known = ["running", "paused_awaiting", "done_claimed", "done", "stopped", "exhausted", "failed"];
        let label = if known.contains(&status.as_str()) { tr(&format!("loop_{status}")) } else { status };
        Some(format!("{}{count} · {label}", tr("loop")))
    }

    fn render_context(&mut self, status: Option<&StatusFields>, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let (provider, _) = self.provider();
        let provider = provider.to_owned();
        let pct = status.and_then(|s| s.ctx_pct);
        let used = status.and_then(|s| s.ctx_used).map(tokens);
        let total = status.and_then(|s| s.ctx_total).map(tokens);
        let window_text = match (used, total) {
            (Some(u), Some(t)) => tr("side_ctx_of").replace("{used}", &u).replace("{total}", &t),
            (None, Some(t)) => t,
            _ => String::new(),
        };
        let pct_color = match pct { Some(p) if p >= 90. => theme::danger(), Some(p) if p >= 70. => theme::warning(), Some(_) => theme::text(), None => theme::faint() };
        let cost = if provider == "codex" {
            let owned = self.selected_key().and_then(|key| self.side.cost.as_ref().filter(|(owner, ..)| owner == &key));
            match owned {
                None => Some((tr("side_cost_loading"), None)),
                Some((_, Some(c), error)) => {
                    let value = match c.usd { Some(usd) => self.money(usd), None => "—".into() };
                    let note = if !c.has_usage { Some(tr("side_cost_no_usage")) }
                        else if !c.missing.is_empty() { Some(tr("side_cost_missing").replace("{models}", &c.missing.join(", "))) }
                        else { Some(tr("side_cost_estimate")) };
                    Some((value, error.clone().map(|e| tr("side_cost_stale").replace("{reason}", &e)).or(note)))
                }
                Some((_, None, error)) => Some(("—".into(), error.clone())),
            }
        } else { status.and_then(|s| s.cost_usd).map(|usd| (self.money(usd), Some(tr("side_cost_session")))) };
        let mut line = Vec::new();
        if self.chat.state.state != "working" {
            if let Some(at) = self.selected.as_ref().and_then(|s| s.last_activity) { line.push((tr("side_idle_for").replace("{t}", &ago(now_seconds() - at)), false)); }
        }
        let (tin, tout) = (status.and_then(|s| s.turn_in), status.and_then(|s| s.turn_out));
        if tin.is_some() || tout.is_some() {
            line.push((tr("side_last_turn_line").replace("{in}", &tokens(tin.unwrap_or(0.))).replace("{out}", &tout.map(tokens).unwrap_or_else(|| "—".into())), true));
        }
        // `.sec-agora` do web: número de 44 px com "do contexto · usado de total" na mesma linha de base, custo à direita,
        // barra larga, a linha do turno e as janelas de cota lado a lado, tudo num bloco só. Painel até 380 px segue a
        // container query do web: legenda desce para a linha de baixo e as partes da linha do turno empilham.
        let narrow = width <= 380.;
        let number = div().flex_shrink_0().whitespace_nowrap().text_size(px(44.)).line_height(px(44.)).font_weight(FontWeight::SEMIBOLD).text_color(pct_color)
            .child(pct.map(|p| format!("{}%", p.round())).unwrap_or_else(|| "—".into()));
        let caption = div().min_w_0().flex().items_baseline().when(narrow, |el| el.flex_wrap()).text_xs().text_color(theme::faint())
            .child(div().flex_shrink_0().whitespace_nowrap().child(tr("side_ctx_label")))
            .when(!window_text.is_empty(), |el| el.child(div().min_w_0().when(!narrow, |el| el.truncate()).font_family(crate::theme::MONO).child(format!("\u{a0}· {window_text}"))));
        let cost = cost.map(|(value, note)| div().flex_shrink_0().flex().flex_col().items_end()
            .child(div().whitespace_nowrap().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(value))
            .when_some(note, |el, note| el.child(div().max_w(px(180.)).truncate().text_size(px(11.)).text_color(theme::faint()).child(note))));
        let top = if narrow {
            div().mb_2().flex().flex_col().gap(px(2.))
                .child(div().flex().items_start().justify_between().gap_2().child(number).children(cost))
                .child(caption)
        } else {
            div().mb_2().flex().items_end().justify_between().gap_3()
                .child(div().min_w_0().flex().items_baseline().gap_2().child(number).child(caption))
                .children(cost)
        };
        let mut body = div().flex().flex_col().child(top);
        body = match pct {
            Some(p) => body.child(div().mt_3().child(chrome::meter(p))),
            None => body.child(div().mt(px(3.)).text_xs().text_color(theme::faint()).child(tr("side_ctx_unknown"))),
        };
        if !line.is_empty() {
            let texts = line.into_iter().map(|(text, mono)| div().min_w_0().when(!narrow, |el| el.truncate()).when(mono, |el| el.font_family(crate::theme::MONO)).child(text));
            body = body.child(div().mt_2().flex().text_size(px(11.)).text_color(theme::faint())
                .map(|el| if narrow { el.flex_col().gap(px(2.)) } else { el.items_baseline().justify_between().gap_2() })
                .children(texts));
        }
        body = body.children(self.render_limits(status).map(|limits| div().mt_4().child(limits)));
        let _ = cx;
        body.into_any_element()
    }

    /// Aviso do contexto cheio, no molde de `.aviso` do web: borda e fundo de alerta, texto e botão em pílula.
    fn render_ctx_warning(&self, status: Option<&StatusFields>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let provider = self.provider().0;
        let pct = status.and_then(|s| s.ctx_pct).filter(|p| *p >= 60. && matches!(provider, "claude" | "codex"))?;
        let serious = pct >= 85.;
        Some(notice(tr(if serious { "side_ctx_serious" } else { "side_ctx_attention" }), serious)
            .child(div().flex().child(notice_button("side-compact", tr("side_compact"), serious, cx)
                .on_click(cx.listener(|this, _, window, cx| this.fill_command("compact", true, window, cx)))))
            .into_any_element())
    }

    fn render_limits(&self, status: Option<&StatusFields>) -> Option<AnyElement> {
        // Antes do primeiro `state` da conversa vale o que a lista de sessões diz.
        let limited = if self.chat.state.state.is_empty() { self.selected.as_ref().and_then(|s| s.limited) == Some(true) } else { self.chat.state.limited };
        let reset = self.chat.state.limit_reset.clone().or_else(|| self.selected.as_ref().and_then(|s| s.limit_reset.clone()));
        let windows: Vec<(String, f64, Option<String>)> = status.map(|s| [
            (tr("limit_5h"), s.five_hour_pct, s.five_hour_reset.clone()),
            (tr("side_limit_7d"), s.weekly_pct, s.weekly_reset.clone()),
            (tr("limit_30d"), s.monthly_pct, s.monthly_reset.clone()),
        ].into_iter().filter_map(|(label, pct, reset)| Some((label, pct?, reset))).collect()).unwrap_or_default();
        if !limited && windows.is_empty() { return None; }
        // `RateChips variant="bars"` do web: janelas lado a lado (quebram quando a coluna estreita), rótulo mono calmo
        // à esquerda e número forte à direita, trilho de 4 px e "reseta" embaixo; o aviso de limite vem depois.
        Some(div().flex().flex_col().gap_2()
            .child(div().flex().flex_wrap().gap_3()
                .children(windows.into_iter().map(|(label, pct, reset)| {
                    let tone = if pct >= 90. { theme::danger() } else if pct >= 70. { theme::warning() } else { theme::text() };
                    div().flex_grow(1.).flex_basis(px(118.)).min_w_0().flex().flex_col()
                        .child(div().flex().justify_between().gap_2().text_xs().font_family(crate::theme::MONO)
                            .child(div().text_color(theme::faint()).child(label))
                            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(tone).child(format!("{}%", pct.round()))))
                        .child(div().mt(px(6.)).child(chrome::meter(pct)))
                        .when_some(reset, |el, r| el.child(div().mt(px(3.)).text_size(px(11.)).text_color(theme::faint()).truncate().child(tr("side_resets").replace("{reset}", &r))))
                })))
            .when(limited, |el| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::limited())
                .child(reset.map(|r| tr("side_limited_until").replace("{reset}", &r)).unwrap_or_else(|| tr("side_limited")))))
            .into_any_element())
    }

    fn render_project(&mut self, status: Option<&StatusFields>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.selected.clone()?;
        let key = self.selected_key()?;
        let repo = status.and_then(|s| s.repo.clone());
        if repo.is_none() && session.git_dirty.is_none() && session.git_added.is_none() { return None; }
        let mut body = div().flex().flex_col().gap_2().child(chrome::section_label(tr("side_repository")));
        if let Some(repo) = repo {
            let branch = status.and_then(|s| s.branch.clone()).unwrap_or_default();
            let dirty = status.and_then(|s| s.dirty) == Some(true);
            body = body.child(div().flex().items_center().gap_1().min_w_0().text_xs().font_family(crate::theme::MONO).font_weight(FontWeight::SEMIBOLD).text_color(theme::muted())
                .child(div().min_w_0().truncate().child(format!("{repo} · {branch}")))
                .when(dirty, |el| el.child(div().text_color(theme::warning()).child("*"))));
        }
        let changes = match (session.git_added, session.git_removed) {
            (Some(a), Some(r)) if a + r > 0 => div().flex().gap_2().text_xs()
                .child(div().font_family(crate::theme::MONO).text_color(theme::success()).child(format!("+{a}")))
                .child(div().font_family(crate::theme::MONO).text_color(theme::danger()).child(format!("−{r}")))
                .child(div().text_color(theme::faint()).child(tr("side_changes_tree"))).into_any_element(),
            _ if session.git_dirty.is_some_and(|n| n > 0) => div().text_xs().text_color(theme::faint()).child(tr("side_changes_local")).into_any_element(),
            _ => div().text_xs().text_color(theme::faint()).child(if session.git_dirty.is_some() { tr("side_changes_none") } else { String::new() }).into_any_element(),
        };
        body = body.child(changes);
        let open = self.side.files.as_ref().is_some_and(|(owner, _)| owner == &key);
        body = body.child(div().flex().gap_1()
            .child(Button::new("side-files").xsmall().ghost().selected(open).label(tr(if open { "side_files_reload" } else { "side_files" }))
                .on_click(cx.listener(|this, _, _, cx| this.load_files(cx))))
            .when(open, |el| el.child(Button::new("side-files-close").xsmall().ghost().label(tr("close"))
                .on_click(cx.listener(|this, _, _, cx| { this.side.files = None; this.side.diff = None; cx.notify(); })))));
        if open {
            let listing = match self.side.files.as_ref().and_then(|(_, slot)| slot.as_ref()) {
                None => div().text_xs().text_color(theme::muted()).child(tr("side_files_loading")).into_any_element(),
                Some(Err(reason)) => div().text_xs().text_color(theme::warning()).child(reason.clone()).into_any_element(),
                Some(Ok(files)) if files.is_empty() => div().text_xs().text_color(theme::muted()).child(tr("side_files_empty")).into_any_element(),
                Some(Ok(files)) => {
                    let current = self.side.diff.as_ref().map(|(_, path, _)| path.clone());
                    div().id("side-files-list").max_h(px(220.)).overflow_y_scroll().flex().flex_col()
                        .children(files.iter().enumerate().map(|(n, file)| {
                            let (path, open) = (file.path.clone(), file.path.clone());
                            let counts = match (file.added, file.removed) { (Some(a), Some(r)) => format!("+{a} −{r}"), _ => file.code.clone() };
                            // O clique na linha mostra o diff; o botão ao lado abre o arquivo no visor.
                            div().w_full().flex_shrink_0().flex().items_center()
                                .child(Button::new(SharedString::from(format!("side-file-{n}"))).ghost().xsmall().flex_1().min_w_0().selected(current.as_deref() == Some(file.path.as_str()))
                                    .child(div().flex_1().min_w_0().truncate().font_family(crate::theme::MONO).child(file.path.clone()))
                                    .child(div().flex_shrink_0().text_color(theme::muted()).child(counts))
                                    .on_click(cx.listener(move |this, _, _, cx| this.open_diff(path.clone(), cx))))
                                .child(Button::new(SharedString::from(format!("side-file-open-{n}"))).ghost().xsmall().icon(IconName::FileText)
                                    .tooltip(tr("side_file_open")).accessibility_label(tr("side_file_open"))
                                    .on_click(cx.listener(move |this, _, window, cx| this.open_file(open.clone(), None, window, cx))))
                        })).into_any_element()
                }
            };
            body = body.child(listing);
        }
        if let Some((owner, path, slot)) = self.side.diff.clone().filter(|(owner, ..)| owner == &key) {
            let _ = owner;
            let content = match slot {
                None => div().text_xs().text_color(theme::muted()).child(tr("side_diff_loading")).into_any_element(),
                Some(Err(reason)) => div().text_xs().text_color(theme::warning()).child(reason).into_any_element(),
                Some(Ok((diff, truncated))) if diff.trim().is_empty() => div().text_xs().text_color(theme::muted())
                    .child(tr(if truncated { "side_diff_truncated" } else { "side_diff_empty" })).into_any_element(),
                Some(Ok((diff, truncated))) => {
                    let (shown, clipped) = conversation::clip(&diff, DIFF_MAX);
                    let view = self.text_view(&format!("side-diff:{path}"), "__side__", conversation::fenced(shown), cx);
                    div().flex().flex_col().gap_1()
                        .child(div().id("side-diff").max_h(px(360.)).overflow_y_scroll().text_xs().child(TextView::new(&view).selectable(true).scrollable(false)))
                        .when(truncated || clipped, |el| el.child(div().text_xs().text_color(theme::muted()).child(tr("side_diff_truncated"))))
                        .into_any_element()
                }
            };
            body = body.child(div().flex().flex_col().gap_1().p_2().rounded(px(12.)).bg(theme::inset()).border_1().border_color(theme::border())
                .child(div().text_xs().font_family(crate::theme::MONO).truncate().child(path)).child(content));
        }
        Some(body.into_any_element())
    }

    pub(super) fn render_shortcuts(&self, readable: bool, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.selected_key();
        let failure = |text: String| div().text_xs().text_color(theme::warning()).child(text);
        // Carregando ou com erro, os do projeto não escondem os globais: o erro vira uma linha discreta embaixo.
        let project = self.side.project.of(key.as_ref());
        let project_error = match &self.side.project.list.value {
            Some(Err(reason)) if self.side.project.owner == key => Some(failure(tr("side_project_shortcuts_failed").replace("{reason}", reason))),
            _ => None,
        };
        let (globals, global_error) = match self.side.shortcuts.as_ref() {
            Some(Ok(list)) => (list.as_slice(), None),
            Some(Err(reason)) => (&[][..], Some(failure(tr("side_shortcuts_failed").replace("{reason}", reason)))),
            None => (&[][..], None),
        };
        let mut list = merged_tiles(globals, project.map_or(&[][..], |p| p.items.as_slice()), project.map_or("", |p| p.key.as_str()));
        // Os internos seguem as regras do "+" e do menu da sessão: some o bloco que não vale para esta sessão.
        let session = self.selected.as_ref();
        let (terminal, browser) = (self.side_menu_terminal(), self.side_menu_browser());
        let mode = session.is_some_and(|s| matches!(s.provider.as_str(), "claude" | "codex"));
        let external = self.external_terminal_available();
        list.retain(|(_, s, _)| match s { Shortcut::Terminal => terminal, Shortcut::Browser => browser, Shortcut::Mode => mode,
            Shortcut::External => external, _ => true });
        let idle = session.is_some_and(|s| s.state == "idle");
        let headless = session.is_some_and(|s| s.headless);
        if list.is_empty() {
            let errors: Vec<Div> = global_error.into_iter().chain(project_error).collect();
            return (!errors.is_empty()).then(|| div().flex().flex_col().gap_1().children(errors).into_any_element());
        }
        let project_tip = project.map(|p| tr("shortcuts_project_tip").replace("{name}", &p.name));
        let busy = key.as_ref().is_some_and(|key| self.uploading.contains_key(key));
        let running = self.side.run.as_ref().is_some_and(|(owner, on)| *on && Some(owner) == key.as_ref());
        // "Ações" do mock: grade de blocos iguais, ícone em cima e rótulo embaixo. As colunas saem da largura do painel
        // (mais colunas quando ele alarga, no máximo cinco), e cada bloco tem a largura exata da coluna: a grade fica no
        // mesmo recuo do título, sem sobra desigual no fim da linha.
        let (_, tile) = shortcut_grid(width - SIDE_PAD * 2. - PANEL_EDGE, list.len());
        // Uma linha por atalho No Hangar vivo que outra sessão abriu: vale também onde a dica do bloco não aparece.
        let notes = key.as_ref().map(|key| self.hangar_notes(&key.server, &key.name, &list)).unwrap_or_default();
        // O respiro do topo é da GRADE, não do bloco: reservado só onde a marca "No Hangar" aparecia, ele empurrava
        // aquele bloco para baixo e tirava ícone e rótulo do prumo dos vizinhos.
        let lift = if key.as_ref().is_some_and(|key| list.iter().any(|(_, s, _)| self.tile_for(&key.server, &key.name, s).mark)) { 22. } else { 8. };
        let buttons: Vec<Button> = list.into_iter().map(|(id, shortcut, own)| {
            // O ícone salvo (glifo ou emoji), como no web; anexos mantém o clipe e Rodar vira parada acesa com o run vivo.
            // 18 px é a medida do web (`.acao-bloco svg`): em 16 o ícone ficava miúdo dentro do bloco.
            let icon = match &shortcut {
                Shortcut::Attach => chrome::small_icon(IconName::Paperclip, 18., theme::muted()).into_any_element(),
                Shortcut::Run if running => chrome::small_icon(IconName::CircleStop, 18., theme::accent()).into_any_element(),
                Shortcut::Run => chrome::small_icon(IconName::Play, 18., theme::muted()).into_any_element(),
                Shortcut::Terminal | Shortcut::External => chrome::small_icon(IconName::SquareTerminal, 18., theme::muted()).into_any_element(),
                Shortcut::Mode => chrome::small_icon(IconName::GitBranch, 18., theme::muted()).into_any_element(),
                Shortcut::Browser => chrome::small_icon(IconName::Globe, 18., theme::muted()).into_any_element(),
                Shortcut::Send { icon, .. } | Shortcut::Shell { icon, .. } => shortcuts::icon_element(icon.as_deref(), 18., theme::muted()),
            };
            let (label, tip) = match &shortcut {
                Shortcut::Run if running => (tr("run_running"), tr("run_running_open")),
                Shortcut::Run => (shortcut.label(), tr("run_project")),
                // O rótulo é o destino, como no menu da sessão; trocar reinicia o processo, então só parada.
                Shortcut::Mode => {
                    let label = super::activity::web(if headless { "modo_abrir_no_terminal" } else { "modo_continuar_sem_terminal" });
                    let tip = super::activity::web(if !idle { "modo_so_ociosa" } else if headless { "modo_abrir_no_terminal_detalhe" } else { "modo_continuar_sem_terminal_detalhe" });
                    (label, tip)
                }
                _ if own => (shortcut.label(), project_tip.clone().unwrap_or_else(|| shortcut.label())),
                _ => (shortcut.label(), shortcut.label()),
            };
            let missing = shortcut.missing_secret();
            // Estado do terminal do atalho (No Hangar: a cópia do servidor; na sessão: só a pergunta): borda e fundo verdes
            // rodando, âmbar perguntando, com a marca HANGAR no canto e a linha de estado embaixo.
            let live = key.as_ref().map(|key| self.tile_for(&key.server, &key.name, &shortcut)).unwrap_or_default();
            let tip = live.tip.clone().unwrap_or(tip);
            // A credencial em branco vale mais que a dica: o clique não roda nada.
            let tip = missing.as_ref().map_or(tip, |name| tr("shortcut_secret_missing").replace("{name}", name));
            let tone = match live.state { hangar_live::TileState::Running => Some((theme::success(), 0.45)), hangar_live::TileState::Asking => Some((theme::warning(), 0.55)), _ => None };
            let fill = tone.map_or_else(|| theme::raised().opacity(0.5), |(color, _)| color.opacity(0.08));
            let text_tone = match live.state { hangar_live::TileState::Running => Some(theme::success_text()), hangar_live::TileState::Asking => Some(theme::warning_text()), _ => None };
            let edge = tone.map_or_else(theme::border, |(color, alpha)| color.opacity(alpha));
            let accessible = if live.line.is_empty() { label.clone() } else { format!("{label} · {}", live.line) };
            Button::new(SharedString::from(id))
                .custom(ButtonCustomVariant::new(cx).color(fill).foreground(if running && shortcut == Shortcut::Run { theme::accent() } else { theme::text() })
                    .hover(theme::hover()).active(theme::hover()))
                // Piso de altura em vez de caixa fixa para o rótulo: o bloco de uma linha deixava de sobra a segunda,
                // e o ícone flutuava acima de um vão. Os da mesma linha se igualam pelo esticar do flex, como no web.
                .w(px(tile)).flex_shrink_0().h_auto().min_h(px(58.)).px(px(4.)).pt(px(lift)).pb(px(8.)).rounded(px(10.)).border_1().border_color(edge)
                .tooltip(tip).accessibility_label(accessible).disabled(!readable || busy || (shortcut == Shortcut::Mode && !idle))
                // Credencial em branco: o bloco fica apagado, e o clique avisa em vez de rodar.
                .when(missing.is_some(), |el| el.opacity(0.55))
                .child(div().relative().w_full().min_w_0().flex().flex_col().items_center().justify_center().gap(px(if tone.is_some() { 6. } else { 4. }))
                    // Marca de "deste projeto", no canto: o bloco segue igual aos outros e o motivo está na dica.
                    .when(own, |el| el.child(div().absolute().top(px(9. - lift)).right(px(4.)).child(chrome::small_icon(IconName::Folder, 10., theme::faint()))))
                    .when(live.mark, |el| el.child(div().absolute().top(px(8. - lift)).right(px(if own { 20. } else { 4. })).flex().items_center().gap(px(4.))
                        .text_size(px(10.)).text_color(text_tone.unwrap_or_else(theme::faint))
                        .child(chrome::small_icon(IconName::Globe, 11., text_tone.unwrap_or_else(theme::faint))).child(tr_shared("term_grupo_hangar", &[]))))
                    .child(icon)
                    // Duas linhas antes de cortar: "Iniciar sessão" e "delphi-vm ide" cabem inteiros num bloco estreito.
                    // Sem `whitespace_normal` o rótulo não quebra: a caixa passa da largura do bloco e, centralizada, perde as
                    // duas pontas ("car Review Au").
                    .child(div().w_full().min_w_0().flex().items_center().justify_center()
                        .child(div().w_full().whitespace_normal().text_center().line_clamp(2).text_ellipsis()
                            .text_size(px(13.)).line_height(px(16.)).child(label)))
                    .when(!live.line.is_empty(), |el| el.child(div().flex().items_center().justify_center().gap(px(5.)).text_size(px(11.))
                        .text_color(text_tone.unwrap_or_else(theme::faint))
                        .when(live.state == hangar_live::TileState::Running, |el| el.child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(theme::success())))
                        .child(live.line.clone()))))
                .on_click(cx.listener(move |this, _, window, cx| this.run_shortcut(shortcut.clone(), false, window, cx)))
        }).collect();
        let grid = div().flex().flex_wrap().gap(px(SHORTCUT_GAP)).children(buttons);
        let add = Button::new("side-shortcut-add").ghost().xsmall().icon(IconName::Plus).tooltip(tr("shortcuts_add"))
            .accessibility_label(tr("shortcuts_add"))
            .on_click(cx.listener(|this, _, window, cx| this.open_settings(super::settings::Page::Shortcuts, window, cx)));
        Some(div().flex().flex_col().gap(px(10.))
            .child(div().flex().items_center().justify_between().child(chrome::section_label(tr("side_actions")))
                .child(div().flex().items_center().gap(px(2.)).child(self.transfer_menu_button(cx)).child(add)))
            .child(grid)
            // Uma nota só para todos os terminais abertos por outra sessão, com o nome do atalho em destaque.
            .when(!notes.is_empty(), |el| el.child(div().flex().flex_col().gap(px(8.)).px(px(14.)).py(px(12.)).rounded(px(10.)).bg(theme::raised().opacity(0.5))
                .text_size(px(12.)).line_height(px(18.)).text_color(theme::muted())
                .children(notes.into_iter().map(|(label, origin)| {
                    let text = format!("{label} {}", tr_shared("atalho_tile_nota_hangar", &[("sessao", &origin)]));
                    let bold = HighlightStyle { color: Some(theme::success_text()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
                    div().w_full().whitespace_normal().child(StyledText::new(text).with_highlights([(0..label.len(), bold)]))
                }))))
            .children(global_error).children(project_error).children(self.transfer_note_element()).into_any_element())
    }

    // Mesma regra do botão de terminal do cabeçalho.
    fn side_menu_terminal(&self) -> bool { self.selected.as_ref().is_some_and(|s| !s.headless) || self.has_shortcut_terms() }

    // A janela abre nesta máquina: só serve à sessão com terminal do servidor local.
    pub(super) fn external_terminal_available(&self) -> bool {
        cfg!(any(target_os = "linux", windows)) && self.selected.as_ref().is_some_and(|s| !s.headless)
            && self.session_api().is_some_and(|api| api.is_loopback())
    }

    fn open_external_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.external_terminal_available() { return; }
        let Some(name) = self.selected.as_ref().map(|s| s.name.clone()) else { return };
        let started = external_command(&name).ok_or_else(|| tr("external_terminal_missing")).and_then(|mut command| {
            let mut child = command.spawn().map_err(|e| tr("external_terminal_failed").replace("{error}", &e.to_string()))?;
            // Sem quem espere, o terminal fechado vira processo zumbi até o app sair.
            std::thread::spawn(move || { let _ = child.wait(); });
            Ok(())
        });
        if let Err(error) = started { window.push_notification(Notification::warning(error), cx); }
    }

    // Mesma regra da aba Git.
    fn side_menu_git(&self) -> bool { self.selected.as_ref().is_some_and(|s| s.readable() && super::sidebar::has_git(s)) }

    // Nesta máquina o motor roda (no Linux, há um Chrome ou Chromium).
    fn side_menu_browser(&self) -> bool {
        let available = crate::browser::Engine::available();
        // A linha some sem dizer por quê: o motivo vai ao log, uma vez só (isto roda a cada desenho).
        static LOGGED: std::sync::Once = std::sync::Once::new();
        if let Err(error) = &available { LOGGED.call_once(|| eprintln!("navegador indisponível: {error}")); }
        available.is_ok()
    }

    /// Alguma linha do menu vale para esta sessão; sem nenhuma, o "+" some.
    fn side_tools(&self) -> bool { self.side_menu_browser() || self.side_menu_terminal() || self.side_menu_git() }

    /// O menu toma o corpo do painel; sem nenhuma ferramenta para esta sessão, fica a aba lembrada.
    pub(super) fn side_menu_shown(&self) -> bool {
        self.side.menu && !self.subagent_tab_open() && self.side_tools()
    }

    /// Menu da superfície vazia do Zeron: uma linha por ferramenta, no meio do painel.
    fn render_side_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let (browser, terminal, git) = (self.side_menu_browser(), self.side_menu_terminal(), self.side_menu_git());
        let row = |id: &'static str, icon: IconName, label: String, cx: &mut Context<Self>| Button::new(id)
            .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h(px(44.)).px(px(14.)).rounded(px(10.)).border_1().border_color(theme::border()).accessibility_label(label.clone())
            // O Button centraliza o conteúdo: a fileira de largura cheia devolve o alinhamento à esquerda.
            .child(div().w_full().flex().items_center().justify_start().gap(px(10.))
                .child(chrome::small_icon(icon, 15., theme::muted()))
                .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(label)));
        div().id("side-menu").flex_1().min_h_0().flex().items_center().justify_center().p(px(16.))
            .child(div().w_full().max_w(px(280.)).flex().flex_col().gap(px(8.))
                .when(browser, |el| el.child(row("side-menu-browser", IconName::Globe, tr("browser"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.open_browser(window, cx)))))
                .when(terminal, |el| el.child(row("side-menu-terminal", IconName::SquareTerminal, tr("shortcuts_native_terminal"), cx)
                    .on_click(cx.listener(|this, _, window, cx| this.show_terminal(window, cx)))))
                .when(git, |el| el
                    .child(row("side-menu-diffs", IconName::List, tr("git_changes"), cx)
                        .on_click(cx.listener(|this, _, window, cx| this.choose_side_tab(SideTab::Git, window, cx))))
                    .child(row("side-menu-history", IconName::GitBranch, tr("git_history"), cx)
                        .on_click(cx.listener(|this, _, window, cx| this.open_git_history(window, cx))))))
            .into_any_element()
    }

    /// O painel está à vista: aberto, com sessão e com largura para ele.
    pub(super) fn side_shown(&self, window: &Window) -> bool {
        let sidebar = self.nav_width();
        self.side.open && self.selected.is_some()
            && self.side.fitted(f32::from(window.viewport_size().width), theme::is_floating(), sidebar, self.side_browser()).is_some()
    }

    /// A aba Navegador está no corpo do painel (e ele usa a largura própria dela).
    pub(super) fn side_browser(&self) -> bool {
        !self.side_menu_shown() && !self.subagent_tab_open() && self.side_tab() == SideTab::Browser
    }

    /// Largura do painel aberto nesta janela; `None` quando está fechado ou não cabe.
    pub(super) fn side_width(&self, window: &Window) -> Option<f32> {
        let viewport = f32::from(window.viewport_size().width);
        let sidebar = self.nav_width();
        self.side.fitted(viewport, theme::is_floating(), sidebar, self.side_browser()).filter(|_| self.side.open && self.selected.is_some())
    }

    /// A leitura de custo acompanha o painel visível; roda no desenho da janela, que acontece mesmo com o painel fechado.
    pub(super) fn sync_side_cost(&mut self, window: &Window) {
        let shown = self.side_width(window).is_some();
        self.sync_cost(shown && self.selected.as_ref().is_some_and(|s| s.readable()));
        self.sync_orq_panel(shown);
    }

    pub(super) fn render_side(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let width = self.side_width(window)?;
        let readable = self.selected.as_ref().is_some_and(|s| s.readable());
        let session = self.selected.clone()?;
        let status = self.status();
        let state = if self.chat_online && !self.chat.state.state.is_empty() { self.chat.state.state.clone() } else { session.state.clone() };
        let detail = self.chat.state.label.clone().or(session.label.clone()).filter(|l| !l.trim().is_empty());
        // Cabeçalho do mock: título "Contexto" e o botão de recolher. Nome e estado já estão no cabeçalho da conversa;
        // o detalhe do estado e o loop descem para a primeira seção.
        let _ = state;
        // Aba de subagente aberta vence a aba escolhida; a árvore só vigia o disco com a aba Arquivos à vista.
        let menu = self.side_menu_shown();
        let tab = (!self.subagent_tab_open() && !menu).then(|| self.side_tab());
        self.show_tree(tab == Some(SideTab::Files), None, cx);
        let tab_in = self.side_tab_in(window, cx);
        // Fileira de abas do web: régua de ponta a ponta com o sublinhado da escolhida por cima dela; o rótulo da
        // primeira aba cai na mesma margem de 16 px das seções, e o recolher fica à parte, no canto. A altura casa o centro
        // do recolher com os botões do cabeçalho da conversa.
        let header = div().flex_shrink_0().h(px(44.)).relative().pl_2().pr_2().flex().items_center().gap_2()
            .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(theme::border()))
            .child(self.render_side_title(cx))
            .when(self.side_tools(), |el| el.child(chrome::icon_button("side-tools", IconName::Plus, tr("side_tools"), cx).flex_shrink_0()
                .selected(menu).toggled(menu)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_side_menu(cx)))))
            .child(chrome::icon_button("side-toggle", IconName::PanelRight, tr("side_hide"), cx).flex_shrink_0()
                .on_click(cx.listener(|this, _, _, cx| this.toggle_side(cx))));
        let orq = session.orq();
        let section = |body: AnyElement| div().px_4().py(px(14.)).border_b_1().border_color(theme::border()).child(body);
        let mut content = div().flex().flex_col();
        if detail.is_some() || self.loop_text().is_some() {
            content = content.child(div().px_4().pt_3().pb_2().flex().flex_col().gap(px(2.))
                .when_some(detail, |el, d| el.child(div().truncate().text_xs().text_color(theme::faint()).child(d)))
                .when_some(self.loop_text(), |el, text| el.child(div().truncate().text_xs().text_color(theme::accent()).child(text))));
        }
        if readable {
            // Na sessão da outra pessoa só se lê: recarregar, arquivos e atalhos são recusados pelo servidor dela.
            let read_only = session.read_only();
            let motive = self.chat.state.recarregar_motivo.clone().filter(|_| !read_only && self.provider().1 && self.provider().0 == "claude");
            let mut notices = Vec::new();
            if let Some(motive) = motive {
                let reloading = self.selected_key().is_some_and(|key| self.side.reloading.contains(&key));
                notices.push(notice(tr("reload_hint").replace("{reason}", &tr(if motive == "config" { "reload_reason_config" } else { "reload_reason_other" })), false)
                    .child(div().text_size(px(11.)).text_color(theme::faint()).child(tr(if reloading { "reload_running" } else if self.reload_allowed() { "reload_ready" } else { "reload_wait" })))
                    .child(div().flex().child(notice_button("side-reload", tr("reload"), false, cx).disabled(!self.reload_allowed())
                        .on_click(cx.listener(|this, _, _, cx| { this.confirm = Some(Confirm::Reload); cx.notify(); }))))
                    .into_any_element());
            }
            notices.extend(self.render_ctx_warning(status.as_ref(), cx));
            content = content.child(section(self.render_context(status.as_ref(), width, cx)))
                .when(!notices.is_empty(), |el| el.child(div().px_4().py_3().border_b_1().border_color(theme::border()).flex().flex_col().gap_2().children(notices)));
            if !read_only && let Some(project) = self.render_project(status.as_ref(), cx) { content = content.child(section(project)); }
            if !read_only && let Some(actions) = self.render_shortcuts(readable, width, cx) { content = content.child(div().px(px(SIDE_PAD)).py(px(14.)).child(actions)); }
        }
        let queued = if readable { self.queued_count() } else { 0 };
        let orq_body = if orq && tab == Some(SideTab::Context) { self.render_orq_panel(cx) } else { div().into_any_element() };
        let handle = div().id("side-resize").absolute().left_0().top_0().bottom_0().w(px(6.)).cursor_col_resize()
            .hover(|el| el.bg(theme::accent_dim()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                let room = Side::room(f32::from(window.viewport_size().width), theme::is_floating(), this.nav_width());
                this.side.drag = Some((f32::from(event.position.x), width, room));
                cx.stop_propagation();
                cx.notify();
            }));
        // A linha fala da sessão: a máquina é a dela, não a do servidor ativo.
        let server = self.session_label(cx);
        let floating = theme::is_floating();
        Some(div().w(px(width)).h_full().flex_shrink_0().relative()
            .child(chrome::glass_panel(div().size_full().flex().flex_col().bg(theme::chrome()).overflow_hidden()
                .map(|el| if floating { el.rounded(px(theme::PANEL_RADIUS)).border_1().border_color(theme::border()).shadow(theme::panel_shadow()) }
                    else { el.border_l_1().border_color(theme::border()) })
                .child(header)
                .children(self.render_subagent_tabs(cx))
                .child(div().flex_1().min_h_0().flex().flex_col().opacity(tab_in).child(match tab {
                    None if menu => self.render_side_menu(cx),
                    None => div().flex_1().min_h_0().children(self.subagent_tab_view()).into_any_element(),
                    Some(SideTab::Files) => div().flex_1().min_h_0().child(self.render_tree(cx)).into_any_element(),
                    Some(SideTab::Activity) => div().flex_1().min_h_0().child(self.activity_view()).into_any_element(),
                    Some(SideTab::Git) => div().flex_1().min_h_0().children(self.side_git(window, cx)).into_any_element(),
                    Some(SideTab::Browser) => div().flex_1().min_h_0().children(self.browser_key().and_then(|k| self.side.browsers.get(&k).cloned())).into_any_element(),
                    Some(SideTab::Context) if orq => div().id("side-scroll").flex_1().min_h_0().overflow_y_scroll().child(orq_body).into_any_element(),
                    Some(SideTab::Context) => div().id("side-scroll").flex_1().min_h_0().overflow_y_scroll().child(content).into_any_element(),
                }))
                .child(div().flex_shrink_0().px_4().py_3().flex().items_center().justify_between().gap_2().border_t_1().border_color(theme::border()).text_size(px(11.))
                    .child(div().min_w_0().truncate().text_color(theme::faint()).child(format!("{} · {server}", agent_label(&session.provider))))
                    .when(queued > 0, |el| el.child(div().flex_shrink_0().text_color(theme::muted()).child(tr("side_queued").replace("{n}", &queued.to_string()))))),
                px(if floating { theme::PANEL_RADIUS } else { 0. })))
            .child(handle)
            .into_any_element())
    }
}

/// Recuo lateral das seções do painel (`px_4`) e espaço entre blocos de atalho.
const SIDE_PAD: f32 = 16.;
const SHORTCUT_GAP: f32 = 6.;
const SHORTCUT_MIN: f32 = 76.;

/// Colunas e largura de cada bloco de atalho para a largura útil `inner` e `count` blocos: o máximo de colunas com bloco
/// de pelo menos `SHORTCUT_MIN`, entre 2 e 5 (mais que cinco por linha fica miúdo), e os blocos dividindo a linha
/// inteira. Coluna que ninguém ocupa sai da conta, como o `auto-fit` do web: três atalhos num painel largo dividem a
/// linha em três, em vez de ficarem encostados à esquerda com um vão de bloco no fim.
fn shortcut_grid(inner: f32, count: usize) -> (usize, f32) {
    let inner = inner.max(SHORTCUT_MIN);
    let fitting = (((inner + SHORTCUT_GAP) / (SHORTCUT_MIN + SHORTCUT_GAP)).floor() as usize).clamp(2, 5);
    let columns = fitting.min(count.max(1));
    let tile = ((inner - SHORTCUT_GAP * (columns - 1) as f32) / columns as f32).floor();
    (columns, tile)
}

/// Como cada emulador recebe o comando a rodar; `$TERMINAL` desconhecido segue a convenção do `-e`.
#[cfg(any(target_os = "linux", test))]
const TERMINALS: [(&str, &[&str]); 8] = [("kitty", &[]), ("ghostty", &["-e"]), ("wezterm", &["start", "--"]),
    ("alacritty", &["-e"]), ("foot", &[]), ("konsole", &["-e"]), ("gnome-terminal", &["--"]), ("xterm", &["-e"])];

/// Programa e argumentos que abrem `tmux attach` na sessão: o `$TERMINAL`, senão o primeiro emulador conhecido instalado.
#[cfg(any(target_os = "linux", test))]
fn linux_launch(chosen: Option<&str>, installed: impl Fn(&str) -> bool, name: &str) -> Option<(String, Vec<String>)> {
    let prefix = |bin: &str| -> Vec<String> {
        let base = std::path::Path::new(bin).file_name().and_then(|n| n.to_str()).unwrap_or(bin);
        TERMINALS.iter().find(|(known, _)| *known == base).map_or(vec!["-e".into()], |(_, pre)| pre.iter().map(|p| p.to_string()).collect())
    };
    let bin = chosen.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
        .or_else(|| TERMINALS.iter().map(|(bin, _)| *bin).find(|bin| installed(bin)).map(str::to_owned))?;
    let mut args = prefix(&bin);
    args.extend(["tmux".into(), "attach".into(), "-t".into(), format!("={name}:")]);
    Some((bin, args))
}

#[cfg(target_os = "linux")]
fn external_command(name: &str) -> Option<std::process::Command> {
    let installed = |bin: &str| std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(bin).is_file()));
    let (bin, args) = linux_launch(std::env::var("TERMINAL").ok().as_deref(), installed, name)?;
    let mut command = std::process::Command::new(bin);
    // Aberto de dentro de outro tmux, o attach recusaria o aninhamento.
    command.args(args).env_remove("TMUX");
    Some(command)
}

#[cfg(windows)]
fn external_command(name: &str) -> Option<std::process::Command> {
    use std::os::windows::process::CommandExt;
    let wt = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join("wt.exe")).find(|p| p.is_file()));
    if let Some(wt) = wt {
        let mut command = std::process::Command::new(wt);
        command.args(["psmux", "attach", "-t", name]);
        return Some(command);
    }
    // Sem o Windows Terminal, `start` abre o psmux no console padrão; o `cmd` que o chama não mostra janela.
    let mut command = std::process::Command::new("cmd");
    command.args(["/C", "start", "", "psmux", "attach", "-t", name]).creation_flags(0x0800_0000);
    Some(command)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn external_command(_name: &str) -> Option<std::process::Command> { None }

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{CHAT_MIN, FLOATING_GAPS, MIN_WIDTH, PANEL_EDGE, SHORTCUT_GAP, SIDE_PAD, Shortcut, Side, duration, merged_tiles, parse_shortcuts, shortcut_grid, tokens};
    use crate::appearance;

    #[test]
    fn shortcuts_fall_back_and_drop_bad_items() {
        // Sem marcação, dos internos só o Rodar vira bloco.
        assert_eq!(parse_shortcuts(""), vec![Shortcut::Run]);
        assert_eq!(parse_shortcuts("{quebrado"), vec![Shortcut::Run]);
        let raw = r#"[{"id":"a","type":"send_text","label":"Relatório","text":"/relatorio","send_direct":false,"confirm":true},
            {"id":"a","type":"shell","label":"dup","command":"x"},{"id":"b","type":"shell","label":"Build","command":"make"},
            {"id":"c","type":"send_text","label":"","text":"x"},{"id":"t","type":"internal","action":"terminal","tile":true},
            {"id":"n","type":"internal","action":"navegador"}]"#;
        assert_eq!(parse_shortcuts(raw), vec![
            Shortcut::Send { label: "Relatório".into(), text: "/relatorio".into(), direct: false, confirm: true, icon: None },
            Shortcut::Shell { label: "Build".into(), command: "make".into(), confirm: false, icon: None, pasta: None,
                key: "global:b".into(), hangar: false, home: true, ask: true },
            Shortcut::Terminal,
        ]);
    }

    #[test]
    fn shell_shortcut_carries_where_it_runs_and_its_identity() {
        let raw = r#"[{"id":"vm","type":"shell","label":"VM","command":"rdp","runs_in":"hangar","hangar_home":false,"answer_in_app":false}]"#;
        assert_eq!(parse_shortcuts(raw), vec![Shortcut::Shell { label: "VM".into(), command: "rdp".into(), confirm: false, icon: None, pasta: None,
            key: "global:vm".into(), hangar: true, home: false, ask: false }]);
    }

    #[test]
    fn project_tiles_come_after_the_globals_with_their_own_ids() {
        let globals = parse_shortcuts(r#"[{"id":"x","type":"internal","action":"anexos"},{"id":"d","type":"shell","label":"Global","command":"g"},
            {"id":"r","type":"internal","action":"rodar"}]"#);
        let project = crate::app::shortcuts::project_items(Some(&serde_json::json!([
            {"id":"d","type":"shell","label":"Debug","command":"make debug","pasta":"backend"},
            {"id":"t","type":"internal","action":"rodar"}])));
        let tiles = merged_tiles(&globals, &project, "/repo");
        // Anexos sem marcação e o interno do projeto não entram; o mesmo id "d" vira dois blocos com ids diferentes.
        assert_eq!(tiles.iter().map(|(id, _, own)| (id.as_str(), *own)).collect::<Vec<_>>(),
            [("shortcut-0", false), ("shortcut-1", false), ("shortcut-p-d", true)]);
        // A identidade No Hangar separa o "d" global do "d" do repositório.
        assert_eq!(tiles[2].1, Shortcut::Shell { label: "Debug".into(), command: "make debug".into(), confirm: false, icon: None, pasta: Some("backend".into()),
            key: "project:/repo:d".into(), hangar: false, home: true, ask: true });
        assert_eq!(tiles[1].1, Shortcut::Run);
    }

    #[test]
    fn token_and_duration_formats() {
        assert_eq!((tokens(590.), tokens(40_400.), tokens(1_000_000.), tokens(1_250_000.)), ("590".into(), "40k".into(), "1M".into(), "1.3M".into()));
        assert_eq!((duration(1_500.), duration(42_000.), duration(125_000.)), ("1.5s".into(), "42s".into(), "2m05s".into()));
    }

    #[test]
    fn panel_never_squeezes_the_chat() {
        let mut side = Side::default();
        let sidebar = appearance::Navigation::Sidebar.sidebar_width();
        assert_eq!(side.fitted(1180., false, sidebar, false), Some(300.));
        assert_eq!(side.fitted(sidebar + CHAT_MIN + MIN_WIDTH - 1., false, sidebar, false), None);
        assert_eq!(side.fitted(sidebar + CHAT_MIN + MIN_WIDTH, false, sidebar, false), Some(MIN_WIDTH));
        // O painel cresce além do limite antigo e encolhe junto com a janela.
        side.width = 1000.;
        assert_eq!(side.fitted(1920., false, sidebar, false), Some(1000.));
        assert_eq!(side.fitted(1180., false, sidebar, false), Some(1180. - sidebar - CHAT_MIN));
        assert_eq!(side.fitted(1180., true, sidebar, false), Some(1180. - sidebar - CHAT_MIN - FLOATING_GAPS));
        let tabs = appearance::Navigation::Tabs.sidebar_width();
        assert_eq!(side.fitted(1920., false, tabs, true), Some(806.));
        assert_eq!(side.fitted(1000., false, tabs, true), Some(420.));
        assert_eq!(side.fitted(1100., false, sidebar, true), Some(1100. - sidebar - CHAT_MIN));
    }

    #[test]
    fn shortcut_grid_fills_the_row_and_grows_columns_with_the_panel() {
        for inner in [150., 268., 400., 700., 2000.] {
            let (columns, tile) = shortcut_grid(inner, 9);
            assert!((2..=5).contains(&columns));
            let used = tile * columns as f32 + SHORTCUT_GAP * (columns - 1) as f32;
            assert!(used <= inner && inner - used < columns as f32, "{inner}: {columns}x{tile}");
        }
        assert!(shortcut_grid(268., 9).0 < shortcut_grid(700., 9).0);
    }

    #[test]
    fn shortcut_row_fits_inside_the_card_border() {
        // A conta da grade é a da largura do painel; a borda do cartão come dela. Num painel de 300 as três colunas
        // pediam 267 e só havia 266: a última caía para a linha de baixo e sobrava um vão do tamanho de um bloco.
        for width in [272., 300., 328., 378.74, 480.] {
            for count in [1, 3, 5, 9] {
                let (columns, tile) = shortcut_grid(width - SIDE_PAD * 2. - PANEL_EDGE, count);
                let used = tile * columns as f32 + SHORTCUT_GAP * (columns - 1) as f32;
                let available = width - 2. - SIDE_PAD * 2.;
                assert!(used <= available, "{width}/{count}: {columns}x{tile} passa de {available}");
            }
        }
    }

    #[test]
    fn few_shortcuts_split_the_whole_row() {
        // O `auto-fit` do web derruba a coluna vazia: três atalhos num painel largo viram três blocos que enchem a
        // linha, não três estreitos com um vão de bloco no fim.
        let inner = 378.74 - SIDE_PAD * 2. - PANEL_EDGE;
        for count in [1, 2, 3] {
            let (columns, tile) = shortcut_grid(inner, count);
            assert_eq!(columns, count, "{count} atalhos deveriam ocupar {count} colunas");
            let leftover = inner - (tile * columns as f32 + SHORTCUT_GAP * (columns - 1) as f32);
            assert!(leftover < columns as f32, "{count}: sobra {leftover}");
        }
        assert_eq!(shortcut_grid(inner, 9).0, 4);
    }

    #[test]
    fn external_terminal_picks_the_command_per_emulator() {
        let attach = |pre: &[&str]| pre.iter().map(|s| s.to_string()).chain(["tmux", "attach", "-t", "=pm-1:"].map(String::from)).collect::<Vec<_>>();
        assert_eq!(super::linux_launch(None, |bin| bin == "kitty", "pm-1"), Some(("kitty".into(), attach(&[]))));
        assert_eq!(super::linux_launch(None, |bin| bin == "wezterm", "pm-1"), Some(("wezterm".into(), attach(&["start", "--"]))));
        assert_eq!(super::linux_launch(Some("/usr/bin/foot"), |_| false, "pm-1"), Some(("/usr/bin/foot".into(), attach(&[]))));
        assert_eq!(super::linux_launch(Some("st"), |_| false, "pm-1"), Some(("st".into(), attach(&["-e"]))));
        assert_eq!(super::linux_launch(Some(" "), |_| false, "pm-1"), None);
    }
}
