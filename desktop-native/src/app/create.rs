//! Criar sessão (`CreateSessionSheet.svelte` + `FolderScanner.svelte` do web, no desenho de duas colunas do desktop): a pasta à
//! esquerda, o formulário à direita. As escolhas finas moram em `choices`; continuar uma conversa antiga, em `resume`.
mod choices;
mod resume;

use super::*;
use choices::{AccountDone, Catalog, Jev, Motor, QuotaLine};
use resume::{ArchiveEntry, PreviewLine};
use super::accounts::ModelChoice;
use super::device::Remote;
use super::machines::{FocusOnClick, enter_to_focused};
use super::settings::Disclosure;
use gpui_kit::component::{IndexPath, WindowExt, select::{Select, SelectEvent, SelectState}, searchable_list::SearchableVec};
use super::chrome::Skeleton;
use serde::Deserialize;
use std::{future::Future, pin::Pin, rc::Rc};

/// Os providers do web, na ordem dele.
const PROVIDERS: [&str; 5] = ["claude", "codex", "pi", "kimi", "omp"];
/// O web pergunta o passo da criação a cada 800 ms.
const STEP_POLL: Duration = Duration::from_millis(800);
/// Topo do diálogo: mais alto que o padrão do kit (um décimo da janela), para as duas colunas caberem.
const DIALOG_TOP: f32 = 40.;

fn provider_name(p: &str) -> &'static str {
    match p { "codex" => "Codex", "pi" => "Pi", "kimi" => "Kimi", "omp" => "OMP", _ => "Claude" }
}

#[derive(Clone, Debug, Deserialize)]
struct Root { name: String, path: String }

#[derive(Clone, Debug, Deserialize)]
struct Entry {
    name: String,
    path: String,
    #[serde(default)] is_git: bool,
    #[serde(default)] has_claude_md: bool,
    mtime: Option<f64>,
}

/// Uma pasta lida: as subpastas, ou o motivo de não haver lista (código do backend já em texto).
pub(super) struct Scan { entries: Vec<Entry>, error: Option<String> }

#[derive(Clone, Debug, Deserialize)]
struct Probe { disponivel: bool }

#[derive(Clone, Debug, Deserialize)]
struct ConfigDir { path: String, label: String, #[serde(default)] active: bool }

#[derive(Clone, Debug, Default, Deserialize)]
struct Issue { code: String, #[serde(default)] params: HashMap<String, String> }

#[derive(Clone, Debug, Default, Deserialize)]
struct Auth { #[serde(default)] status: String, email: Option<String>, #[serde(default)] method: String }

#[derive(Clone, Debug, Default, Deserialize)]
struct Inherit { #[serde(default)] status: String, #[serde(default)] issues: Vec<Issue> }

#[derive(Clone, Debug, Deserialize)]
struct CodexAccount { id: String, name: String, credential_id: Option<String>, #[serde(default)] is_default: bool, #[serde(default)] auth: Auth, #[serde(default)] sync: Inherit }

impl CodexAccount {
    fn auth_text(&self) -> String {
        match self.auth.status.as_str() {
            "connected" => self.auth.email.clone().unwrap_or_else(|| tr(if self.auth.method == "oauth" { "create_codex_oauth" } else { "create_codex_key" })),
            "disconnected" => tr("create_codex_disconnected"),
            _ => tr("create_codex_unknown"),
        }
    }
    /// A dica da linha e a do campo: "Padrão · e-mail".
    fn hint(&self) -> String {
        let mut parts = Vec::new();
        if self.is_default { parts.push(tr("create_default")); }
        parts.push(self.auth_text());
        parts.join(" · ")
    }
}

/// A sessão aberta pela resposta do backend e os avisos já em texto (reconciliação da conta, sessão achada pela lista).
/// `warning`: a continuação nasceu com outra coisa que a pedida (o resumo do Hangar no lugar do escrito pelo modelo).
pub(super) struct Opened { session: SessionInfo, notes: Vec<String>, warning: Option<String> }

/// A sessão que o diálogo continua (modo bastão): o servidor monta o resumo dela e o manda à sessão nova.
pub(in crate::app) struct Baton { pub(in crate::app) name: String, pub(in crate::app) cwd: Option<String> }

/// Nome de quem continua: `pm18368-t24` → `pm18368-t24b`, e a próxima letra livre. Letra, não `-2`: o `-2` é o desempate de
/// duas sessões na mesma pasta.
fn successor(origin: &str, taken: &HashSet<String>) -> String {
    let base = sanitize(origin);
    if base.is_empty() { return unique_name("sessao", taken); }
    ('b'..='z').map(|c| format!("{base}{c}")).find(|name| !taken.contains(name)).unwrap_or_else(|| unique_name(&format!("{base}b"), taken))
}

pub(super) enum CreateReply {
    /// Raízes e a última raiz escolhida neste aparelho.
    Roots(u64, Result<Value, Failure>, Option<String>),
    /// A pasta já convertida na tarefa do tokio: a lista grande não é lida na thread da janela.
    Scan(u64, Result<Scan, String>),
    Sessions(u64, Result<Vec<SessionInfo>, Failure>),
    Providers(u64, Result<Value, Failure>),
    Configs(u64, Result<Value, Failure>),
    Codex(u64, Result<Value, Failure>),
    /// Passo da criação em voo; `None` é consulta que falhou, e o passo anterior fica.
    Step(u64, Option<String>),
    Created(u64, Result<Opened, String>),
    /// O catálogo de modelos e o último modelo e esforço lembrados para a chave dele.
    Models(u64, Result<Value, Failure>, (String, String)),
    Engines(u64, Result<Value, Failure>),
    /// A configuração do servidor: só a chave do Jev e o padrão dele interessam aqui.
    Config(u64, Result<Value, Failure>),
    Quotas(u64, Result<Value, Failure>),
    Context(u64, Result<Value, Failure>),
    Account(u64, AccountDone),
    Archive(u64, Result<Value, Failure>),
    Preview(u64, Result<Value, Failure>),
    /// A amostra do resumo do bastão, em markdown.
    Baton(u64, Result<String, Failure>),
}

/// A regra do backend (`names.sanitize_session_name`): acento vira a letra sem ele, o que não for letra, número, `_` ou `-` vira `-`,
/// e as pontas perdem os `-`.
// ponytail: o NFKD do Latin-1 e do Latin Extended-A em tabela; letra fora dela some, como no `encode("ascii", "ignore")` do backend.
pub(super) fn sanitize(name: &str) -> String {
    let folded: String = name.trim().chars().map(|c| if c.is_ascii() { c.to_string() } else { base_letters(c).to_owned() }).collect();
    folded.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect::<String>().trim_matches('-').to_owned()
}

fn base_letters(c: char) -> &'static str {
    const FROM: &str = "ÀÁÂÃÄÅàáâãäåÇçÈÉÊËèéêëÌÍÎÏìíîïÑñÒÓÔÕÖòóôõöÙÚÛÜùúûüÝýÿĀāĂăĄąĆćĈĉĊċČčĎďĒēĔĕĖėĘęĚěĜĝĞğĠġĢģĤĥĨĩĪīĬĭĮįİĴĵĶķĹĺĻļĽľŃńŅņŇňŌōŎŏŐőŔŕŖŗŘřŚśŜŝŞşŠšŢţŤťŨũŪūŬŭŮůŰűŲųŴŵŶŷŸŹźŻżŽžªº¹²³ſ";
    const TO: &str = "AAAAAAaaaaaaCcEEEEeeeeIIIIiiiiNnOOOOOoooooUUUUuuuuYyyAaAaAaCcCcCcCcDdEeEeEeEeEeGgGgGgGgHhIiIiIiIiIJjKkLlLlLlNnNnNnOoOoOoRrRrRrSsSsSsSsTtTtUuUuUuUuUuUuWwYyYZzZzZzao123s";
    match c {
        'Ĳ' => "IJ", 'ĳ' => "ij", 'Ŀ' => "L", 'ŀ' => "l", 'ŉ' => "n", '¼' => "14", '½' => "12", '¾' => "34",
        '\u{a0}' | '¨' | '¯' | '´' | '¸' => " ",
        _ => FROM.chars().position(|f| f == c).map(|n| &TO[n..n + 1]).unwrap_or(""),
    }
}

/// Nome único: limpo como no backend e, se já existir, com `-2`, `-3`…
pub(super) fn unique_name(base: &str, taken: &HashSet<String>) -> String {
    let clean = Some(sanitize(base)).filter(|s| !s.is_empty()).unwrap_or_else(|| "sessao".into());
    if !taken.contains(&clean) { return clean; }
    (2..).map(|n| format!("{clean}-{n}")).find(|name| !taken.contains(name)).unwrap_or(clean)
}

fn basename(path: &str) -> &str { path.trim_end_matches('/').rsplit('/').next().unwrap_or(path) }

/// Caminho relativo ao pai da raiz: "pessoal/hangar".
fn rel_path(root: &str, path: &str) -> String {
    let parent = root.rsplit_once('/').map(|(p, _)| p).unwrap_or_default();
    if !parent.is_empty() { path.strip_prefix(&format!("{parent}/")).unwrap_or(path).to_owned() } else { path.to_owned() }
}

/// Migalhas da raiz até a pasta atual: rótulo e caminho de cada nível.
fn crumbs(root: &Root, path: &str) -> Vec<(String, String)> {
    let mut out = vec![(root.name.clone(), root.path.clone())];
    let mut acc = root.path.clone();
    for part in path.strip_prefix(&root.path).unwrap_or_default().split('/').filter(|s| !s.is_empty()) {
        acc = format!("{acc}/{part}");
        out.push((part.to_owned(), acc.clone()));
    }
    out
}

fn shown(query: &str, root: &str, entry: &Entry) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || entry.name.to_lowercase().contains(&q) || rel_path(root, &entry.path).to_lowercase().contains(&q)
}

/// A leitura de uma pasta: a recusa de fronteira do backend vira o motivo dela, como o `scanDir` do web.
fn scan_of(result: Result<Value, Failure>) -> Result<Scan, String> {
    let code = match result {
        Ok(value) => {
            let entries = serde_json::from_value(value.get("entries").cloned().unwrap_or_default()).map_err(|_| tr("invalid_response"))?;
            return Ok(Scan { entries, error: value.get("error").and_then(Value::as_str).map(scan_error) });
        }
        Err(error) => match error.status {
            Some(400) => "invalid_path", Some(403) => "root_not_allowed", Some(404) => "not_found",
            Some(401) => return Err(tr("auth_error")),
            None => return Err(Hangar::fetch_failure(&error)),
            Some(_) => "unknown",
        },
    };
    Ok(Scan { entries: Vec::new(), error: Some(scan_error(code)) })
}

fn scan_error(code: &str) -> String {
    tr(match code {
        "permission_denied" => "create_scan_permission", "unreadable" => "create_scan_unreadable", "root_not_allowed" => "create_scan_root",
        "invalid_path" => "create_scan_invalid", "not_found" => "create_scan_missing", _ => "create_scan_failed",
    })
}

/// Data da pasta como o `relativeTime` do web: "há N min", "há N h", e a data a partir de um dia.
fn folder_time(mtime: f64) -> String {
    let now = chrono::Local::now().timestamp() as f64;
    if now - mtime < 86_400. { return super::side::ago(now - mtime); }
    chrono::DateTime::from_timestamp(mtime as i64, 0).map(|t| t.with_timezone(&chrono::Local)
        .format(if crate::i18n::english() { "%m/%d/%Y" } else { "%d/%m/%Y" }).to_string()).unwrap_or_default()
}

fn step_text(value: &Value) -> Option<String> {
    let key = match value.get("step").and_then(Value::as_str)? {
        "preparando" => "create_step_checking", "resumo" => "create_step_summary", "resumo_modelo" => "create_step_summary_model",
        "conta" => "create_step_account", "criando" => "create_step_opening", "recado" => "create_step_message", _ => return None,
    };
    let account = value.pointer("/params/conta").and_then(Value::as_str).unwrap_or_default();
    Some(tr(key).replace("{conta}", account))
}

/// O seletor e a assinatura dele: a lista relida troca os dois juntos, e a assinatura velha sai com o seletor.
type Picker = (Entity<SelectState<SearchableVec<ModelChoice>>>, Subscription);

type Chosen = fn(&mut NewSession, String, &mut Window, &mut Context<NewSession>);

fn picker(choices: Vec<ModelChoice>, at: Option<usize>, chosen: Chosen, window: &mut Window, cx: &mut Context<NewSession>) -> Picker {
    let state = cx.new(|cx| SelectState::new(SearchableVec::new(choices), at.map(IndexPath::new), window, cx));
    let subscription = cx.subscribe_in(&state, window, move |this, _, event: &SelectEvent<SearchableVec<ModelChoice>>, window, cx| {
        if let SelectEvent::Confirm(Some(id)) = event { chosen(this, id.clone(), window, cx); cx.notify(); }
    });
    (state, subscription)
}

/// A conexão da abertura. Guardada no diálogo porque as respostas chegam com o `Hangar` em atualização, e o pedido seguinte
/// (a pasta da raiz que chegou) não pode passar por ele.
struct Link { api: Api, runtime: Arc<Runtime>, tx: async_channel::Sender<Envelope>, connection: u64 }

/// O diálogo "Nova sessão". Entidade própria: o diálogo é desenhado durante o desenho da janela, quando o `Hangar` não pode ser
/// escrito. Cada resposta volta com o número do pedido; a de um pedido velho (outra pasta, outro provider) cai.
pub(in crate::app) struct NewSession {
    link: Link,
    roots: Remote<Vec<Root>>,
    root: Option<Root>,
    /// A pasta listada à esquerda.
    dir: String,
    scan: Remote<Scan>,
    /// Pastas da leitura que casam com a busca, na ordem da lista: refeito quando a busca ou a leitura mudam, não a cada quadro.
    folders: Vec<usize>,
    query: Entity<InputState>,
    /// A pasta escolhida: é ela que o formulário configura.
    picked: Option<String>,
    sessions: Remote<Vec<SessionInfo>>,
    same_folder: bool,
    name: Entity<InputState>,
    provider: &'static str,
    providers: Remote<HashMap<String, Probe>>,
    configs: Remote<Vec<ConfigDir>>,
    config: Option<String>,
    config_pick: Option<Picker>,
    codex: Remote<Vec<CodexAccount>>,
    codex_account: String,
    codex_pick: Option<Picker>,
    headless: bool,
    difference: bool,
    manual_open: bool,
    manual: Entity<InputState>,
    choosing: bool,
    choose_error: Option<String>,
    create_seq: u64,
    creating: bool,
    started: Option<Instant>,
    step: String,
    error: Option<String>,
    /// O relógio do "passo · N s": anda sozinho a cada segundo, mesmo sem resposta do backend.
    clock: Option<Task<()>>,
    models: Remote<Catalog>,
    model: String,
    effort: String,
    permission: String,
    subagent: String,
    engine: String,
    model_pick: Option<Picker>,
    effort_pick: Option<Picker>,
    permission_pick: Option<Picker>,
    subagent_pick: Option<Picker>,
    engine_pick: Option<Picker>,
    engines: Remote<Vec<(String, Motor)>>,
    jev: Remote<Jev>,
    jev_on: bool,
    more: bool,
    omp: Entity<InputState>,
    quotas: Remote<Vec<QuotaLine>>,
    /// "+ conta": a linha do nome aberta; "Apagar": a confirmação na tela.
    asking: bool,
    confirming: bool,
    account_busy: bool,
    account_seq: u64,
    account_name: Entity<InputState>,
    /// O resultado da última operação de conta e se é erro.
    notice: Option<(String, bool)>,
    /// A conta criada nesta abertura: o aviso do /login só vale enquanto ela está escolhida.
    created_path: Option<String>,
    context_seq: u64,
    context_busy: bool,
    context_on: Option<bool>,
    context_want: Option<bool>,
    context_error: Option<String>,
    archive: Remote<Vec<ArchiveEntry>>,
    want_resume: bool,
    conversation: String,
    /// A conta de antes de a conversa escolhida puxar o seletor para a dela.
    before: Option<Option<String>>,
    preview: Remote<Vec<PreviewLine>>,
    preview_scroll: ScrollHandle,
    resuming: bool,
    /// Modo bastão: a sessão continuada, quem escreve o resumo, e a amostra recolhida (`None` dentro dela é resumo vazio).
    baton: Option<Baton>,
    baton_by_model: bool,
    baton_open: bool,
    baton_preview: Remote<Option<Entity<TextViewState>>>,
    _subscriptions: Vec<Subscription>,
}

impl NewSession {
    fn new(link: Link, baton: Option<Baton>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_search")));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_name_placeholder")));
        let manual = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_path_placeholder")));
        let omp = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_omp_profile_hint")));
        let account_name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_account_placeholder")));
        // O Enter no Nome não cria: no web o campo não está num formulário.
        let subscriptions = vec![
            cx.subscribe(&query, |this: &mut Self, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { this.refilter(cx); cx.notify() }),
            cx.subscribe(&name, |_, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify() }),
            cx.subscribe_in(&manual, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.use_typed(window, cx),
                _ => {}
            }),
            cx.subscribe(&account_name, |this: &mut Self, _, event: &InputEvent, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.add_account(cx),
                _ => {}
            }),
        ];
        Self {
            link, roots: Remote::default(), root: None, dir: String::new(), scan: Remote::default(), folders: Vec::new(), query, picked: None,
            sessions: Remote::default(), same_folder: false, name, provider: "claude", providers: Remote::default(), configs: Remote::default(),
            config: None, config_pick: None, codex: Remote::default(), codex_account: String::new(), codex_pick: None, headless: false,
            difference: false, manual_open: false, manual, choosing: false, choose_error: None, create_seq: 0, creating: false, started: None,
            step: String::new(), error: None, clock: None, models: Remote::default(), model: String::new(), effort: String::new(),
            permission: String::new(), subagent: String::new(), engine: String::new(), model_pick: None, effort_pick: None,
            permission_pick: None, subagent_pick: None, engine_pick: None, engines: Remote::default(), jev: Remote::default(), jev_on: false,
            more: false, omp, quotas: Remote::default(), asking: false, confirming: false, account_busy: false, account_seq: 0, account_name,
            notice: None, created_path: None, context_seq: 0, context_busy: false, context_on: None, context_want: None, context_error: None,
            archive: Remote::default(), want_resume: false, conversation: String::new(), before: None, preview: Remote::default(),
            preview_scroll: ScrollHandle::new(), resuming: false, baton, baton_by_model: false, baton_open: false,
            baton_preview: Remote::default(), _subscriptions: subscriptions,
        }
    }

    /// Manda o pedido; a resposta volta a este diálogo pelo id dele, e só se a conexão ainda for a da abertura.
    fn request(&self, cx: &mut Context<Self>, work: impl FnOnce(Api, Sender) -> Pin<Box<dyn Future<Output = ()> + Send>>) {
        let (me, tx, connection) = (cx.entity_id(), self.link.tx.clone(), self.link.connection);
        let send: Sender = Arc::new(move |reply| {
            let tx = tx.clone();
            Box::pin(async move { let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Create(me, reply) }).await; })
        });
        self.link.runtime.spawn(work(self.link.api.clone(), send));
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.roots.start();
        self.request(cx, move |api, send| Box::pin(async move {
            let last = tokio::task::spawn_blocking(crate::appearance::last_root).await.ok().flatten();
            send(CreateReply::Roots(seq, api.server_read(&["fs", "roots"], &[], 15).await, last)).await
        }));
        self.load_providers(cx);
        self.load_configs(cx);
        self.load_extras(cx);
        // A continuação trabalha na mesma árvore: a pasta da origem já vem escolhida. Sem pasta conhecida, o nome sai já e a
        // pasta fica por escolher.
        match self.baton.as_ref().map(|b| (b.cwd.clone().filter(|c| !c.is_empty()), b.name.clone())) {
            Some((Some(cwd), _)) => self.pick(cwd, window, cx),
            Some((None, origin)) => self.name.update(cx, |input, cx| input.set_value(successor(&origin, &HashSet::new()), window, cx)),
            None => {}
        }
    }

    /// Abre ou fecha a amostra; lê o resumo só na primeira abertura, ou de novo depois de uma falha.
    fn toggle_baton_preview(&mut self, open: bool, cx: &mut Context<Self>) {
        self.baton_open = open;
        cx.notify();
        let Some(origin) = self.baton.as_ref().map(|b| b.name.clone()) else { return };
        if !open || self.baton_preview.loading || self.baton_preview.ok().is_some() { return; }
        let seq = self.baton_preview.start();
        self.request(cx, move |api, send| Box::pin(async move {
            let text = api.server_bytes(&["sessions", &origin, "bastao"], 30).await.map(|b| String::from_utf8_lossy(&b).into_owned());
            send(CreateReply::Baton(seq, text)).await
        }));
    }

    fn load_providers(&mut self, cx: &mut Context<Self>) {
        let seq = self.providers.start();
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Providers(seq, api.server_read(&["providers"], &[], 30).await)).await }));
    }

    fn load_configs(&mut self, cx: &mut Context<Self>) {
        let seq = self.configs.start();
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Configs(seq, api.server_read(&["claude-configs"], &[], 15).await)).await }));
    }

    fn load_codex(&mut self, cx: &mut Context<Self>) {
        // O número continua do anterior: resposta da ida passada ao Codex não passa por desta.
        let seq = self.codex.start();
        (self.codex_account, self.codex_pick) = (String::new(), None);
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Codex(seq, api.server_read(&["codex-contas"], &[], 30).await)).await }));
    }

    fn select_root(&mut self, root: Root, window: &mut Window, cx: &mut Context<Self>) {
        let path = root.path.clone();
        // A pasta da criação em voo não muda: a coluna da esquerda fica parada até a resposta.
        if self.creating { return; }
        self.root = Some(root);
        let remember = path.clone();
        self.link.runtime.spawn_blocking(move || crate::appearance::remember_root(&remember));
        self.query.update(cx, |input, cx| input.set_value("", window, cx));
        self.scan_dir(path, cx);
    }

    fn scan_dir(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(root) = self.root.as_ref().map(|r| r.path.clone()) else { return };
        self.dir = path.clone();
        let seq = self.scan.start();
        self.request(cx, move |api, send| Box::pin(async move {
            let query: Vec<(&str, &str)> = if path == root { vec![("root", root.as_str())] } else { vec![("root", root.as_str()), ("path", path.as_str())] };
            send(CreateReply::Scan(seq, scan_of(api.server_read(&["fs", "scan"], &query, 15).await))).await
        }));
        cx.notify();
    }

    fn drill(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating { return; }
        self.query.update(cx, |input, cx| input.set_value("", window, cx));
        self.scan_dir(path, cx);
    }

    /// Escolher a pasta lê as sessões: o nome sugerido não pode repetir, e a pasta com sessão ganha o aviso.
    fn pick(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating { return; }
        (self.picked, self.error, self.same_folder) = (Some(path), None, false);
        let seq = self.sessions.start();
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Sessions(seq, api.sessions().await)).await }));
        self.load_archive(window, cx);
        cx.notify();
    }

    fn use_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.manual.read(cx).value().trim().to_owned();
        if !path.is_empty() { self.pick(path, window, cx); }
    }

    /// "Pasta do computador": o seletor do sistema. A pasta é desta máquina; com o backend em outra, o erro vem na criação.
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.choosing || self.creating { return; }
        (self.choosing, self.choose_error) = (true, None);
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.choosing = false;
                match chosen {
                    Ok(Ok(Some(paths))) => if let Some(path) = paths.first() { this.pick(path.to_string_lossy().into_owned(), window, cx); },
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.choose_error = Some(error.to_string()),
                    Err(_) => this.choose_error = Some(tr("picker_failed")),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Trocar de provider zera o modo e a permissão (o Codex nasce em "Full Access", como no web) e relê o que depende dele.
    fn set_provider(&mut self, provider: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if provider == self.provider || self.creating { return; }
        (self.provider, self.headless, self.error) = (provider, false, None);
        self.permission = if provider == "codex" { "Full Access".into() } else { String::new() };
        if provider == "codex" { self.load_codex(cx); self.load_context(cx); } else { self.drop_context(); self.drop_codex(); }
        self.load_models(window, cx);
        self.load_archive(window, cx);
        cx.notify();
    }

    fn set_headless(&mut self, headless: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.headless = headless;
        self.build_permission_pick(window, cx);
        cx.notify();
    }

    fn provider_ready(&self) -> Option<bool> {
        if self.providers.loading { return None; }
        Some(self.providers.ok().and_then(|p| p.get(self.provider)).is_none_or(|p| p.disponivel))
    }

    fn codex_ready(&self) -> bool {
        self.provider != "codex" || (!self.codex.loading
            && self.codex.ok().and_then(|list| list.iter().find(|a| a.id == self.codex_account)).is_some_and(|a| a.auth.status == "connected"))
    }

    fn can_create(&self, cx: &App) -> bool {
        !self.creating && self.picked.is_some() && !self.sessions.loading && !self.name.read(cx).value().trim().is_empty()
            && self.provider_ready() == Some(true) && self.codex_ready() && !(self.provider == "codex" && self.context_busy)
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create(cx) { return; }
        let Some(cwd) = self.picked.clone() else { return };
        let name = self.name.read(cx).value().trim().to_owned();
        let provider = self.provider;
        let text = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
        let mut body = json!({"name": name, "cwd": cwd, "provider": provider, "model": text(&self.model), "effort": text(&self.effort)});
        match provider {
            "claude" => {
                body["config_dir"] = json!(self.config);
                body["engine"] = text(&self.engine);
                if !self.permission.is_empty() { body["permission_mode"] = json!(self.permission); }
                // O motor exporta o próprio modelo de subagente: com ele, o campo nem aparece.
                if self.engine.is_empty() && !self.subagent.is_empty() { body["subagent_model"] = json!(self.subagent); }
            }
            "codex" => body["codex_account"] = json!(self.codex_account),
            "omp" => { let profile = self.omp.read(cx).value().trim().to_owned(); if !profile.is_empty() { body["omp_profile"] = json!(profile); } }
            _ => {}
        }
        if self.headless && matches!(provider, "claude" | "codex") {
            body["headless"] = json!(true);
            if provider == "codex" { body["permission_mode"] = text(&self.permission); }
        }
        // O bastão não leva o Jev (o interruptor nem aparece) e vai por rota própria, que monta, grava e manda o resumo.
        let baton = self.baton.as_ref().map(|b| b.name.clone());
        let jev = self.jev_choice().filter(|_| baton.is_none());
        if let Some((on, _)) = jev { body["jev"] = json!(on); }
        if baton.is_some() {
            let claude = provider == "claude";
            let omp = self.omp.read(cx).value().trim().to_owned();
            body = json!({"name": name, "cwd": cwd, "provider": provider, "model": text(&self.model), "effort": text(&self.effort),
                "config_dir": if claude { json!(self.config) } else { Value::Null },
                "engine": if claude { text(&self.engine) } else { Value::Null },
                "permission_mode": if claude { text(&self.permission) } else { Value::Null },
                "omp_profile": if provider == "omp" { text(&omp) } else { Value::Null },
                "headless": matches!(provider, "claude" | "codex") && self.headless,
                "resumo_por_modelo": self.baton_by_model});
            if provider == "codex" { body["codex_account"] = json!(self.codex_account); }
        }
        // A memória vai antes do POST: a escolha não se perde se a criação falhar.
        let (key, model, effort) = (self.memory_key(), self.model.clone(), self.effort.clone());
        self.link.runtime.spawn_blocking(move || crate::appearance::remember_model(&key, &model, &effort));
        self.create_seq += 1;
        let seq = self.create_seq;
        (self.creating, self.error, self.step, self.started) = (true, None, String::new(), Some(Instant::now()));
        self.clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if !this.update(cx, |this, cx| { cx.notify(); this.creating }).unwrap_or(false) { break; }
        }));
        self.request(cx, move |api, send| Box::pin(async move {
            // O padrão do Jev muda só aqui, ao criar; falhar nele não impede a sessão de nascer com a escolha feita.
            if let Some((on, true)) = jev
                && let Err(error) = api.server_send(reqwest::Method::POST, &["config"], Some(json!({"jev_padrao": on})), 8).await {
                eprintln!("jev-padrao-salvar: {}", error.detail);
            }
            let (poll_api, poll_send, poll_name) = (api.clone(), send.clone(), name.clone());
            let poll = tokio::spawn(async move {
                loop {
                    let step = poll_api.server_read(&["sessions", "creation-progress"], &[("name", poll_name.as_str())], 5).await.ok();
                    poll_send(CreateReply::Step(seq, step.as_ref().and_then(step_text))).await;
                    tokio::time::sleep(STEP_POLL).await;
                }
            });
            let opened = match baton {
                // A reescrita pelo modelo leva até 180 s no servidor.
                Some(origin) => {
                    let result = api.server_send(reqwest::Method::POST, &["sessions", &origin, "bastao"], Some(body), 200).await;
                    poll.abort();
                    handed(&api, result, &name, &cwd).await
                }
                None => {
                    let result = api.server_send(reqwest::Method::POST, &["sessions"], Some(body), 120).await;
                    poll.abort();
                    opened(&api, result, &name, &cwd).await
                }
            };
            send(CreateReply::Created(seq, opened)).await
        }));
        cx.notify();
    }

    /// Guarda a resposta que ainda é deste diálogo; a criação que deu certo sai daqui para o `Hangar` abrir a sessão.
    fn receive(&mut self, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) -> Option<Opened> {
        match reply {
            CreateReply::Roots(seq, result, last) => {
                let roots = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<Root>>(v).map_err(|_| tr("invalid_response")));
                if !self.roots.finish(seq, roots) { return None; }
                let list = self.roots.ok().cloned().unwrap_or_default();
                if let Some(root) = list.iter().find(|r| Some(&r.path) == last.as_ref()).or(list.first()).cloned() { self.select_root(root, window, cx); }
            }
            CreateReply::Scan(seq, result) => { if self.scan.finish(seq, result) { self.refilter(cx); } }
            CreateReply::Sessions(seq, result) => {
                if seq != self.sessions.seq { return None; }
                let Some(path) = self.picked.clone() else { return None };
                // Lista que não veio não trava: o nome vai sem desempate, e o backend recusa se repetir. No bastão o nome vem da
                // origem, não da pasta.
                let origin = self.baton.as_ref().map(|b| b.name.clone());
                let name = match &result {
                    Ok(list) => {
                        self.same_folder = list.iter().any(|s| s.cwd.as_deref() == Some(path.as_str()));
                        let taken = list.iter().map(|s| s.name.clone()).collect();
                        match &origin { Some(origin) => successor(origin, &taken), None => unique_name(basename(&path), &taken) }
                    }
                    Err(_) => match &origin { Some(origin) => successor(origin, &HashSet::new()), None => basename(&path).to_owned() },
                };
                self.sessions.finish(seq, result.map_err(|e| Hangar::fetch_failure(&e)));
                self.name.update(cx, |input, cx| input.set_value(name, window, cx));
            }
            CreateReply::Providers(seq, result) => {
                let probes = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")));
                self.providers.finish(seq, probes);
            }
            CreateReply::Configs(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<ConfigDir>>(v).map_err(|_| tr("invalid_response")));
                if !self.configs.finish(seq, list) { return None; }
                self.config = self.fallback_config();
                self.build_config_pick(window, cx);
                // Lista que falhou também pede o catálogo: sem conta, o backend usa a padrão.
                self.load_models(window, cx);
            }
            CreateReply::Codex(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<CodexAccount>>(v).map_err(|_| tr("invalid_response")));
                if !self.codex.finish(seq, list) { return None; }
                let list = self.codex.ok().cloned().unwrap_or_default();
                let at = list.iter().position(|a| a.is_default).or((!list.is_empty()).then_some(0));
                self.codex_account = at.map(|n| list[n].id.clone()).unwrap_or_default();
                let choices = list.iter().map(|a| ModelChoice { id: a.id.clone(), label: a.name.clone(), hint: a.hint() }).collect();
                self.codex_pick = Some(picker(choices, at, |this, id, window, cx| {
                    this.codex_account = id;
                    this.load_models(window, cx);
                    this.load_archive(window, cx);
                }, window, cx));
                self.load_models(window, cx);
                self.load_archive(window, cx);
            }
            reply @ (CreateReply::Models(..) | CreateReply::Engines(..) | CreateReply::Config(..) | CreateReply::Quotas(..)
                | CreateReply::Context(..) | CreateReply::Account(..)) => self.receive_extra(reply, window, cx),
            reply @ (CreateReply::Archive(..) | CreateReply::Preview(..)) => self.receive_archive(reply, cx),
            CreateReply::Baton(seq, result) => {
                // "Não consegui ler" e "o resumo está vazio" são respostas diferentes: a falha nunca vira caixa vazia.
                let view = result.map_err(|e| Hangar::fetch_failure(&e))
                    .map(|text| (!text.trim().is_empty()).then(|| cx.new(|cx| TextViewState::markdown(&safe_markdown(&text), cx))));
                self.baton_preview.finish(seq, view);
            }
            CreateReply::Step(seq, step) => {
                if seq != self.create_seq || !self.creating { return None; }
                if let Some(step) = step { self.step = step; }
            }
            CreateReply::Created(seq, result) => {
                if seq != self.create_seq || !self.creating { return None; }
                (self.creating, self.resuming, self.started, self.clock) = (false, false, None, None);
                match result { Ok(opened) => return Some(opened), Err(error) => self.error = Some(error) }
            }
        }
        cx.notify();
        None
    }
}

type Sender = Arc<dyn Fn(CreateReply) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A resposta do POST vira a sessão a abrir. Queda depois de mandar, ou resposta boa ilegível, não diz se ela nasceu: a lista
/// responde, pelo nome que o backend dá (a mesma limpeza) e pela pasta, e o aviso diz que foi achada assim.
async fn opened(api: &Api, result: Result<Value, Failure>, name: &str, cwd: &str) -> Result<Opened, String> {
    match result {
        Ok(value) => {
            let accounts: Vec<&str> = value.get("avisos").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let notes = if accounts.is_empty() { Vec::new() } else { vec![tr("create_account_notes").replace("{n}", &accounts.join(" · "))] };
            match serde_json::from_value(value.clone()) {
                Ok(session) => Ok(Opened { session, notes, warning: None }),
                Err(_) => found(api, name, cwd, tr("invalid_response")).await,
            }
        }
        Err(error) if error.uncertain && error.status.is_none() => found(api, name, cwd, tr("connection_failed")).await,
        Err(error) => Err(Hangar::fetch_failure(&error)),
    }
}

/// A resposta do bastão traz só o nome da sessão nova: a sessão vem da lista. Lista que não responde não desfaz a passagem, que
/// já aconteceu: a sessão abre pelo nome e a lista seguinte completa o resto.
async fn handed(api: &Api, result: Result<Value, Failure>, name: &str, cwd: &str) -> Result<Opened, String> {
    let value = match result {
        Ok(value) => value,
        Err(error) if error.uncertain && error.status.is_none() => return found(api, name, cwd, tr("connection_failed")).await,
        Err(error) => return Err(Hangar::fetch_failure(&error)),
    };
    let Some(created) = value.get("name").and_then(Value::as_str).map(str::to_owned) else { return found(api, name, cwd, tr("invalid_response")).await };
    let warning = value.get("aviso").and_then(Value::as_str).filter(|a| !a.is_empty()).map(|a| tr("create_baton_summary_failed").replace("{motivo}", a));
    let listed = match api.sessions().await {
        Ok(list) => list.into_iter().find(|s| s.name == created),
        Err(error) => { eprintln!("bastao-lista: {}", error.detail); None }
    };
    let (session, notes) = match listed {
        Some(session) => (session, Vec::new()),
        None => (SessionInfo { name: created, cwd: Some(cwd.to_owned()), ..SessionInfo::default() }, vec![tr("create_baton_unlisted")]),
    };
    Ok(Opened { session, notes, warning })
}

async fn found(api: &Api, name: &str, cwd: &str, failure: String) -> Result<Opened, String> {
    let clean = sanitize(name);
    let list = api.sessions().await.map_err(|_| failure.clone())?;
    list.into_iter().find(|s| s.name == clean && s.cwd.as_deref() == Some(cwd))
        .map(|session| Opened { session, notes: vec![tr("create_found_in_list")], warning: None }).ok_or(failure)
}

fn label(text: String) -> Div { div().text_size(px(13.)).text_color(theme::muted()).child(text) }

fn muted(text: String) -> Div { div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text) }

fn alert(id: &'static str, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(text)
}

/// Opção escolhível (raiz, provider, onde roda): a escolhida com o fundo de destaque suave dos segmentos das Configurações.
fn choice(id: impl Into<ElementId>, on: bool, cx: &App) -> Button {
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
        .foreground(if on { theme::accent_text() } else { theme::text() }).hover(theme::hover()).active(theme::hover()))
        .border_1().border_color(if on { theme::accent() } else { theme::border() }).when(on, |b| b.bg(theme::accent_dim()))
}

/// Cartão de uma escolha entre duas (onde roda, quem escreve o resumo): o ponto de rádio, o título e o resumo da escolha.
fn option_card(id: &'static str, on: bool, title: String, beta: bool, summary: String, busy: bool, cx: &App) -> Button {
    choice(id, on, cx).flex_1().min_w_0().h_auto().py(px(8.)).px(px(12.)).rounded(px(8.)).selected(on).disabled(busy)
        .accessibility_label(format!("{title}. {summary}"))
        // No topo, não no centro que o botão dá: com resumos de alturas diferentes, os títulos dos dois cartões ficam na mesma linha.
        .child(div().self_start().w_full().flex().items_start().gap(px(10.))
            .child(div().mt(px(3.)).size(px(12.)).flex_shrink_0().rounded_full().border_1()
                .border_color(if on { theme::accent() } else { theme::border_strong() })
                .when(on, |el| el.child(div().m(px(2.)).size(px(6.)).rounded_full().bg(theme::accent()))))
            .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.))
                .child(div().flex().items_center().gap(px(6.)).text_sm().font_weight(FontWeight::MEDIUM).child(title)
                    .when(beta, |el| el.child(super::server_config::chip(tr("create_beta"), theme::accent_text(), theme::accent_dim()))))
                .child(div().w_full().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(summary))))
}

impl NewSession {
    /// Modo bastão: quem escreve o resumo e a amostra dele, recolhida (aberta, empurraria o formulário e ainda mostraria um
    /// texto que não é o que vai: a origem segue trabalhando até o envio).
    fn render_baton(&self, cx: &mut Context<Self>) -> Option<Div> {
        let baton = self.baton.as_ref()?;
        let busy = self.creating;
        let author = |id: &'static str, by_model: bool, title: &str, summary: &str| {
            option_card(id, self.baton_by_model == by_model, tr(title), false, tr(summary), busy, cx)
                .on_click(cx.listener(move |this, _, _, cx| { this.baton_by_model = by_model; cx.notify(); }))
        };
        let preview = match self.baton_preview.value.as_ref().filter(|_| !self.baton_preview.loading) {
            None => div().id("create-baton-loading").role(Role::Status).child(muted(tr("loading"))).into_any_element(),
            Some(Err(error)) => alert("create-baton-error", format!("{} {error}", tr("create_baton_preview_failed"))).into_any_element(),
            Some(Ok(None)) => muted(tr("create_baton_preview_empty")).into_any_element(),
            // Títulos no tamanho do texto, como no web: é um documento lido numa caixa pequena, não uma página.
            Some(Ok(Some(view))) => div().text_sm().text_color(theme::muted()).child(TextView::new(view).selectable(true).scrollable(false)
                .style(gpui_kit::component::text::TextViewStyle::default().heading_font_size(|_, _| px(14.)))).into_any_element(),
        };
        let this = cx.entity().downgrade();
        Some(div().flex().flex_col().gap(px(16.))
            .child(div().flex().flex_col().gap(px(6.))
                .child(label(tr("create_baton_author")))
                .child(div().id("create-baton-author").role(Role::Group).aria_label(tr("create_baton_author")).flex().gap(px(12.))
                    .child(author("create-baton-hangar", false, "create_baton_hangar", "create_baton_hangar_summary"))
                    .child(author("create-baton-model", true, "create_baton_model", "create_baton_model_summary"))))
            .child(div().flex().flex_col().gap(px(8.))
                .child(div().child(Disclosure::new("create-baton-preview", self.baton_open, tr("create_baton_preview"), true)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| this.toggle_baton_preview(open, cx)); })))
                .when(self.baton_open, |el| el
                    .child(muted(tr("create_baton_preview_note").replace("{n}", &baton.name)))
                    .child(div().id("create-baton-sample").max_h(px(220.)).overflow_y_scroll().p(px(12.)).rounded(px(8.))
                        .bg(theme::inset()).child(preview)))))
    }

    fn render_roots(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.roots.loading || self.roots.value.is_none() {
            return div().flex().gap(px(8.)).children((0..2usize).map(|i| Skeleton::new(("create-root-skeleton", i)).w(px(96.)).h(px(32.)).rounded_full())).into_any_element();
        }
        match self.roots.value.as_ref() {
            Some(Err(error)) => alert("create-roots-error", format!("{} {error}", tr("create_roots_failed"))).into_any_element(),
            Some(Ok(list)) if list.is_empty() => muted(tr("create_no_roots")).into_any_element(),
            _ => div().id("create-roots").role(Role::Group).aria_label(tr("create_roots")).flex().flex_wrap().gap(px(8.))
                .children(self.roots.ok().into_iter().flatten().enumerate().map(|(n, root)| {
                    let on = self.root.as_ref().is_some_and(|r| r.path == root.path);
                    let pick = root.clone();
                    choice(SharedString::from(format!("create-root-{n}")), on, cx).small().rounded_full().label(root.name.clone()).disabled(self.creating)
                        .accessibility_label(root.name.clone()).tooltip(root.path.clone())
                        .on_click(cx.listener(move |this, _, window, cx| this.select_root(pick.clone(), window, cx)))
                })).into_any_element(),
        }
    }

    fn render_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.root.is_none() { return div().into_any_element(); }
        if self.scan.loading || self.scan.value.is_none() {
            // Esqueletos são marcadores de posição, sem item de domínio: a posição é a identidade deles.
            return div().flex().flex_col().gap(px(6.)).children((0..5usize).map(|i| div().flex().flex_col().gap(px(6.)).px(px(10.)).py(px(8.))
                .child(Skeleton::new(("create-row-skeleton", i)).w(px(160.)).h(px(12.)))
                .child(Skeleton::new(("create-row-skeleton-detail", i)).secondary().w(px(240.)).h(px(10.))))).into_any_element();
        }
        let scan = match self.scan.value.as_ref() {
            Some(Err(error)) => return alert("create-scan-error", error.clone()).into_any_element(),
            Some(Ok(scan)) => scan,
            None => return div().into_any_element(),
        };
        if let Some(error) = scan.error.clone() { return muted(error).into_any_element(); }
        if self.folders.is_empty() {
            let searching = !self.query.read(cx).value().trim().is_empty();
            return muted(tr(if searching { "create_no_results" } else { "create_no_subfolders" })).into_any_element();
        }
        // Só as linhas visíveis são montadas; todas têm a mesma altura, e o espaço entre elas vai dentro de cada uma.
        uniform_list("create-folder-list", self.folders.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
            range.filter_map(|ix| this.folder_row(ix, cx)).collect::<Vec<_>>()
        })).size_full().into_any_element()
    }

    fn refilter(&mut self, cx: &App) {
        let query = self.query.read(cx).value().to_string();
        let root = self.root.as_ref().map(|r| r.path.as_str()).unwrap_or("");
        self.folders = match self.scan.ok() {
            Some(scan) => scan.entries.iter().enumerate().filter(|(_, e)| shown(&query, root, e)).map(|(ix, _)| ix).collect(),
            None => Vec::new(),
        };
    }

    fn folder_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let root = self.root.as_ref()?;
        let entry = self.scan.ok()?.entries.get(*self.folders.get(ix)?)?;
        let on = self.picked.as_deref() == Some(entry.path.as_str());
        let (pick, open) = (entry.path.clone(), entry.path.clone());
        let badge = |text: &str| div().px(px(6.)).rounded(px(4.)).border_1().border_color(theme::border()).text_size(px(10.5))
            .font_family(theme::MONO).text_color(theme::muted()).child(text.to_owned());
        // A linha da lista virtual não estica sozinha como o filho da coluna esticava.
        Some(div().w_full().flex().items_center().gap(px(4.)).pb(px(2.))
            .child(choice(SharedString::from(format!("create-folder-{}", entry.path)), on, cx).disabled(self.creating).flex_1().min_w_0().h_auto().py(px(6.)).px(px(10.))
                .rounded(px(8.)).accessibility_label(entry.name.clone()).selected(on)
                .child(div().w_full().min_w_0().flex().flex_col().items_start().gap(px(2.))
                    .child(div().w_full().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(entry.name.clone()))
                    .child(div().w_full().flex().items_center().gap(px(6.))
                        .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted())
                            .child(rel_path(&root.path, &entry.path)))
                        .when(entry.is_git, |el| el.child(badge("git")))
                        .when(entry.has_claude_md, |el| el.child(badge("CLAUDE.md")))
                        .when_some(entry.mtime, |el, t| el.child(div().flex_shrink_0().text_size(px(11.)).text_color(theme::faint()).child(folder_time(t))))))
                .on_click(cx.listener(move |this, _, window, cx| this.pick(pick.clone(), window, cx))))
            .child(Button::new(SharedString::from(format!("create-open-{}", entry.path))).ghost().small().flex_shrink_0().disabled(self.creating)
                .icon(IconName::ChevronRight).accessibility_label(tr("create_open").replace("{nome}", &entry.name))
                .on_click(cx.listener(move |this, _, window, cx| this.drill(open.clone(), window, cx))))
            .into_any_element())
    }

    fn render_left(&self, cx: &mut Context<Self>) -> Div {
        let drilled = self.root.as_ref().filter(|r| r.path != self.dir);
        let path_row = drilled.map(|root| div().flex().flex_col().gap(px(8.))
            .child(div().id("create-crumbs").aria_label(tr("create_path")).flex().flex_wrap().items_center().gap(px(2.))
                .children(crumbs(root, &self.dir).into_iter().enumerate().map(|(n, (text, path))| div().flex().items_center().gap(px(2.))
                    .when(n > 0, |el| el.child(div().text_color(theme::faint()).text_size(px(12.)).child("/")))
                    .child(Button::new(SharedString::from(format!("create-crumb-{n}"))).ghost().xsmall().label(text).disabled(self.creating)
                        .on_click(cx.listener(move |this, _, window, cx| this.drill(path.clone(), window, cx)))))))
            .child(div().child(Button::new("create-use-here").outline().small().icon(IconName::FolderOpen).label(tr("create_use_folder")).disabled(self.creating)
                .on_click(cx.listener(|this, _, window, cx| { let dir = this.dir.clone(); this.pick(dir, window, cx); })))));
        let manual_ready = !self.manual.read(cx).value().trim().is_empty();
        let footer = div().flex_shrink_0().pt(px(12.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(8.))
            .child(div().flex().items_center().gap(px(8.))
                .child(Button::new("create-computer-folder").outline().small().icon(IconName::Folder).label(tr("create_computer_folder")).disabled(self.creating)
                    .loading(self.choosing).on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx))))
                .child(div().flex_1())
                .child({
                    let this = cx.entity().downgrade();
                    Disclosure::new("create-advanced", self.manual_open, tr("create_advanced"), true)
                        .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.manual_open = open; cx.notify(); }); })
                }))
            .when_some(self.choose_error.clone(), |el, error| el.child(alert("create-choose-error", error)))
            .when(self.manual_open, |el| el.child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().child(Input::new(&self.manual).small().font_family(theme::MONO).aria_label(tr("create_path_aria"))))
                .child(Button::new("create-use-typed").outline().small().label(tr("create_use")).disabled(!manual_ready || self.creating)
                    .on_click(cx.listener(|this, _, window, cx| this.use_typed(window, cx))))));
        let title = div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr(if self.baton.is_some() { "create_baton_title" } else { "create_title" }));
        let column = div().w(relative(0.45)).flex_shrink_0().h_full().min_h_0().pr(px(20.)).border_r_1().border_color(theme::border()).flex().flex_col()
            .gap(px(12.)).child(match &self.baton {
                Some(b) => div().flex().flex_col().gap(px(4.)).child(title)
                    .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("create_baton_origin").replace("{n}", &b.name))),
                None => title,
            });
        // Com uma conversa escolhida, a coluna larga vira a leitura dela: a pasta já está escolhida.
        if let Some(c) = self.target() { return column.child(self.render_preview(c, cx)); }
        column
            .child(self.render_roots(cx))
            .when(self.root.is_some(), |el| el.child(Input::new(&self.query).small().cleanable(true).prefix(chrome::small_icon(IconName::Search, 14., theme::muted()))
                .aria_label(tr("create_search"))))
            .children(path_row)
            // A lista de pastas rola sozinha; carregando, vazia ou com erro, a caixa é que rola.
            .child(div().id("create-folders").flex_1().min_h_0().flex().flex_col()
                .when(self.folders.is_empty() || self.scan.loading, |el| el.overflow_y_scroll()).child(self.render_rows(cx)))
            .child(footer)
    }

    fn render_form(&self, path: &str, cx: &mut Context<Self>) -> Div {
        let checking = self.sessions.loading;
        let ready = self.provider_ready();
        let probe_error = self.providers.value.as_ref().and_then(|v| v.as_ref().err()).cloned();
        let missing = ready == Some(false);
        let busy = self.creating;
        let providers = div().id("create-providers").role(Role::Group).aria_label(tr("create_provider_aria")).flex().gap(px(8.))
            .children(PROVIDERS.iter().map(|&p| {
                let available = self.providers.ok().and_then(|m| m.get(p)).is_none_or(|probe| probe.disponivel);
                choice(SharedString::from(format!("create-provider-{p}")), self.provider == p, cx).flex_1().min_w_0().h(px(36.)).rounded(px(8.))
                    .selected(self.provider == p).disabled(!available || busy).accessibility_label(provider_name(p))
                    .child(div().flex().items_center().gap(px(6.)).child(chrome::provider_glyph(p, 16.)).child(provider_name(p)))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_provider(p, window, cx)))
            }));
        let target = self.target().cloned();
        let fresh = target.is_none();
        let claude = (self.provider == "claude").then(|| self.render_claude_account(cx));
        let codex = (self.provider == "codex").then(|| {
            let account = self.codex.ok().and_then(|list| list.iter().find(|a| a.id == self.codex_account)).cloned();
            div().flex().flex_col().gap(px(4.))
                .child(label(tr("create_codex_account")))
                .map(|el| match (&self.codex_pick, self.codex.value.as_ref()) {
                    (_, _) if self.codex.loading => el.child(div().id("create-codex-loading").role(Role::Status).child(muted(tr("loading")))),
                    (_, Some(Err(error))) => el.child(alert("create-codex-error", error.clone())),
                    (Some((pick, _)), _) => el.child(Select::new(pick).small().disabled(busy).accessibility_label(tr("create_codex_account"))),
                    _ => el,
                })
                .when_some(account, |el, a| el.child(muted(a.hint())).children(self.render_codex_quota(a.credential_id.as_deref()))
                    .children(a.sync.issues.iter().enumerate().map(|(n, issue)| {
                        let text = crate::i18n::tr_web(&issue.code, &issue.params).or_else(|| crate::i18n::tr_web("codex_account_error_unknown", &HashMap::new()))
                            .unwrap_or_else(|| issue.code.clone());
                        let ready = a.sync.status == "ready";
                        div().id(SharedString::from(format!("create-codex-issue-{n}"))).role(if ready { Role::Status } else { Role::Alert })
                            .text_size(px(12.5)).whitespace_normal().text_color(if ready { theme::muted() } else { theme::danger() }).child(text)
                    })))
        });
        let modes = (fresh && matches!(self.provider, "claude" | "codex")).then(|| {
            let codex = self.provider == "codex";
            let mode = |id: &'static str, on: bool, title: String, beta: bool, summary: String, headless: bool| {
                option_card(id, on, title, beta, summary, busy, cx)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_headless(headless, window, cx)))
            };
            let help = match (codex, self.headless) {
                (false, false) => "create_mode_tmux_help", (false, true) => "create_mode_headless_help",
                (true, false) => "create_mode_tmux_help_codex", (true, true) => "create_mode_headless_help_codex",
            };
            let this = cx.entity().downgrade();
            div().flex().flex_col().gap(px(6.))
                .child(label(tr("create_mode")))
                .child(div().id("create-modes").role(Role::Group).aria_label(tr("create_mode")).flex().gap(px(12.))
                    .child(mode("create-mode-tmux", !self.headless, tr("create_mode_tmux"), false,
                        tr(if codex { "create_mode_tmux_summary_codex" } else { "create_mode_tmux_summary" }), false))
                    .child(mode("create-mode-headless", self.headless, tr("create_mode_headless"), true,
                        tr(if codex { "create_mode_headless_summary_codex" } else { "create_mode_headless_summary" }), true)))
                .child(div().child(Disclosure::new("create-difference", self.difference, tr("create_mode_difference"), true)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.difference = open; cx.notify(); }); })))
                .when(self.difference, |el| el.child(muted(tr(help))))
        });
        let fields = div().flex().flex_col().gap(px(16.))
            .child(div().flex().flex_col().gap(px(4.)).pr(px(28.))
                .child(div().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(path.to_owned()))
                .when(checking, |el| el.child(div().id("create-checking").role(Role::Status).child(muted(tr("create_checking")))))
                .when(!checking && self.same_folder, |el| el.child(div().id("create-same-folder").role(Role::Status).child(muted(tr("create_same_folder"))))))
            // Nome, modo, modelo, esforço e permissão não chegam ao retomar: com uma conversa escolhida, somem.
            .when(!checking, |el| el
                .when(fresh, |el| el.child(div().flex().flex_col().gap(px(4.)).child(label(tr("create_name")))
                    .child(Input::new(&self.name).disabled(busy).aria_label(tr("create_name")))))
                .child(div().flex().flex_col().gap(px(6.)).child(label(tr("create_provider"))).child(providers)
                    .when_some(probe_error, |el, error| el.child(muted(tr("create_probe_failed").replace("{erro}", &error))
                        .id("create-probe-error").role(Role::Alert)))
                    .when(missing, |el| el.child(alert("create-provider-missing", tr("create_provider_missing").replace("{p}", self.provider)))))
                .children(codex)
                .children(claude)
                .children(modes)
                .children(self.render_resume(cx))
                .when(fresh && self.provider == "omp", |el| el.child(self.render_omp()))
                .when(fresh, |el| el.children(self.render_trio()))
                .when(fresh && self.provider == "codex", |el| el.child(self.render_context(cx)))
                .children(self.render_more(cx))
                .children(self.render_baton(cx)));
        let can = self.can_create(cx);
        let seconds = self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let step = if self.step.is_empty() { tr("create_creating") } else { self.step.clone() };
        // Uma ação primária só: com uma conversa escolhida, o botão continua aquela conversa em vez de criar.
        let submit = match &target {
            Some(c) => Button::new("create-resume-submit").primary().w_full().label(self.resume_label(c)).loading(busy).disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.resume(cx))),
            None => Button::new("create-submit").primary().w_full()
                .label(tr(if busy { "create_creating" } else if self.baton.is_some() { "create_baton_submit" } else { "create_submit" }))
                .loading(busy).disabled(!can && !busy).on_click(cx.listener(|this, _, _, cx| this.create(cx))),
        };
        let footer = div().flex_shrink_0().pt(px(12.)).border_t_1().border_color(theme::border()).flex().flex_col().gap(px(8.))
            .when_some(self.error.clone(), |el, error| el.child(alert("create-error", error)))
            .child(submit)
            .when(busy && !self.resuming, |el| el.child(div().id("create-step").role(Role::Status).child(muted(tr("create_step_time")
                .replace("{passo}", &step).replace("{segundos}", &seconds.to_string())))));
        div().flex_1().min_w_0().h_full().min_h_0().pl(px(20.)).flex().flex_col().gap(px(12.))
            .child(div().id("create-form").flex_1().min_h_0().overflow_y_scroll().child(fields))
            .child(footer)
    }
}

impl Render for NewSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Topo, margem de baixo do kit e o preenchimento do diálogo: o resto da janela, até a altura do web.
        let height = (window.viewport_size().height - px(DIALOG_TOP + 16. + 32.)).min(px(760.)).max(px(320.));
        let right = match self.picked.clone() {
            Some(path) => self.render_form(&path, cx),
            None => div().flex_1().min_w_0().h_full().pl(px(20.)).flex().flex_col().items_center().justify_center().gap(px(8.))
                .child(div().size(px(56.)).rounded(px(14.)).bg(theme::inset()).mb(px(8.)).flex().items_center().justify_center()
                    .child(chrome::small_icon(IconName::Folder, 26., theme::muted())))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("create_empty_title")))
                .child(div().max_w(px(300.)).text_center().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("create_empty_sub"))),
        };
        div().h(height).w_full().flex().child(self.render_left(cx)).child(right)
    }
}

impl Hangar {
    /// Com `baton`, o mesmo diálogo cria a sessão que continua aquela (o "Continuar em outra conta" do menu da sessão).
    pub(super) fn open_new_session(&mut self, baton: Option<Baton>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let link = Link { api, runtime: self.runtime.clone(), tx: self.tx.clone(), connection: self.connection };
        let dialog = cx.new(|cx| NewSession::new(link, baton, window, cx));
        dialog.update(cx, |d, cx| d.load(window, cx));
        self.new_session = Some(dialog.clone());
        let weak = cx.entity().downgrade();
        let width = (window.viewport_size().width * 0.94).min(px(1320.));
        window.open_dialog(cx, move |d, _, cx| {
            // Criando, o diálogo não fecha: ele é o único lugar onde o resultado aparece, como o Adicionar de Máquinas.
            let busy = dialog.read(cx).creating;
            let (weak, me) = (weak.clone(), dialog.entity_id());
            d.w(width).margin_top(px(DIALOG_TOP)).child(dialog.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(enter_to_focused)
                .on_close(move |_, _, cx| { let _ = weak.update(cx, |this, _| {
                    if this.new_session.as_ref().is_some_and(|d| d.entity_id() == me) { this.new_session = None; }
                }); })
        });
    }

    pub(super) fn receive_create(&mut self, dialog: EntityId, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entity) = self.new_session.clone().filter(|d| d.entity_id() == dialog) else { return };
        let Some(Opened { session, notes, warning }) = entity.update(cx, |d, cx| d.receive(reply, window, cx)) else { return };
        self.new_session = None;
        window.close_dialog(cx);
        let readable = session.readable();
        self.select(session, window, cx);
        // O fechar devolveu o foco ao botão que abriu; a sessão nova é onde se escreve em seguida, como no clique na aba.
        if readable { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
        for note in notes { window.push_notification(Notification::info(note), cx); }
        // Fica até ser fechado, como o `alert` do web: quem pediu o resumo do modelo tem de saber que recebeu o do Hangar.
        if let Some(warning) = warning { window.push_notification(Notification::warning(warning).autohide(false), cx); }
        cx.notify();
    }

    /// Botão "Nova sessão" da barra lateral e o "+" das abas: o foco fica nele, para o diálogo devolvê-lo ao fechar.
    pub(super) fn new_session_button(&self, tabs: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let id: &'static str = if tabs { "tabs-new-session" } else { "sidebar-new-session" };
        let weak = cx.entity().downgrade();
        let button = if tabs {
            Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted()).hover(theme::hover()).active(theme::hover()))
                .icon(chrome::small_icon(IconName::Plus, 16., theme::muted())).size(px(28.)).rounded(px(6.)).flex_shrink_0().tooltip(tr("create_title"))
        } else {
            Button::new(id).outline().small().w_full().icon(IconName::Plus).label(tr("create_title"))
        };
        FocusOnClick { id: id.into(), button: button.accessibility_label(tr("create_title")).disabled(self.api.is_none()), open: Rc::new(move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.open_new_session(None, window, cx));
        }) }
    }
}

#[cfg(test)]
mod tests {
    use super::{Failure, HashSet, Root, basename, crumbs, json, rel_path, sanitize, scan_of, successor, tr, unique_name};

    #[test]
    fn the_successor_takes_the_next_free_letter() {
        let taken: HashSet<String> = ["pm18368-t24b", "pm18368-t24c"].into_iter().map(String::from).collect();
        assert_eq!(successor("pm18368-t24", &HashSet::new()), "pm18368-t24b");
        assert_eq!(successor("pm18368-t24", &taken), "pm18368-t24d");
        assert_eq!(successor("São Paulo", &HashSet::new()), "Sao-Paulob");
        assert_eq!(successor("日本", &HashSet::new()), "sessao");
    }

    #[test]
    fn names_follow_the_backend_rule_and_never_repeat() {
        assert_eq!(sanitize("Área de trabalho"), "Area-de-trabalho");
        assert_eq!(sanitize("  São Paulo  "), "Sao-Paulo");
        assert_eq!(sanitize("api.v2b"), "api-v2b");
        assert_eq!(sanitize("日本"), "");
        assert_eq!(sanitize("3º Turno ½"), "3o-Turno-12");
        let taken: HashSet<String> = ["api-v2b", "api-v2b-2"].into_iter().map(String::from).collect();
        assert_eq!(unique_name("api.v2b", &taken), "api-v2b-3");
        assert_eq!(unique_name("日本", &HashSet::new()), "sessao");
    }

    #[test]
    fn folder_paths_read_from_the_root() {
        let root = Root { name: "pessoal".into(), path: "/home/x/pessoal".into() };
        assert_eq!(rel_path(&root.path, "/home/x/pessoal/hangar"), "pessoal/hangar");
        assert_eq!(crumbs(&root, "/home/x/pessoal/hangar/app").into_iter().map(|(l, _)| l).collect::<Vec<_>>(), ["pessoal", "hangar", "app"]);
        assert_eq!(basename("/home/x/pessoal/hangar/"), "hangar");
    }

    #[test]
    fn scan_refusals_become_the_reason_and_not_a_list() {
        let refused = |status| Err(Failure { status: Some(status), detail: "x".into(), retry_after: None, uncertain: false });
        for (status, key) in [(400, "create_scan_invalid"), (403, "create_scan_root"), (404, "create_scan_missing"), (500, "create_scan_failed")] {
            let scan = scan_of(refused(status)).ok().unwrap();
            assert!(scan.entries.is_empty());
            assert_eq!(scan.error, Some(tr(key)));
        }
        let ok = scan_of(Ok(json!({"entries": [{"name": "a", "path": "/r/a", "is_git": true, "mtime": 1.0}], "error": null}))).ok().unwrap();
        assert_eq!((ok.entries.len(), ok.error), (1, None));
    }
}
