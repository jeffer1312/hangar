//! Criar sessão (`CreateSessionSheet.svelte` + `FolderScanner.svelte` do web, no desenho de duas colunas do desktop): a pasta à
//! esquerda, o formulário à direita. As escolhas finas moram em `choices`; continuar uma conversa antiga, em `resume`.
pub(super) mod choices;
mod customizations;
mod folder_git;
mod resume;

use super::*;
use choices::{AccountDone, Catalog, Jev, Motor, QuotaLine};
use resume::{ArchiveEntry, PreviewLine};
use super::accounts::ModelChoice;
use super::device::Remote;
use super::machines::{FocusOnClick, enter_to_focused};
use super::settings::Disclosure;
use super::sidebar::{Target, wrap_step};
use gpui_kit::component::{IndexPath, WindowExt, select::{Select, SelectEvent, SelectState}, searchable_list::{SearchableListItem, SearchableVec}};
use super::chrome::Skeleton;
use serde::Deserialize;
use std::{future::Future, pin::Pin, rc::Rc};

/// Anexo da primeira mensagem: nome, bytes e se vai como imagem. Sobe depois que a sessão nasce, que é quem tem pasta.
pub(super) type FirstFile = (String, Arc<Vec<u8>>, bool);

/// Chave dos anexos da tela sem sessão (`composer_key`); nenhuma sessão real tem nome vazio.
pub(super) fn new_chat_key() -> SessionKey { SessionKey { server: String::new(), name: String::new(), jsonl: String::new() } }

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
pub(super) struct Root { pub(super) name: String, pub(super) path: String }

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) path: String,
    #[serde(default)] is_git: bool,
    #[serde(default)] has_claude_md: bool,
    mtime: Option<f64>,
}

/// Uma pasta lida: as subpastas, ou o motivo de não haver lista (código do backend já em texto).
pub(super) struct Scan { pub(super) entries: Vec<Entry>, pub(super) error: Option<String> }

struct RootScan { root: Root, scan: Remote<Scan> }
enum FolderIndex { Current(usize), Root(usize, usize) }

#[derive(Debug, PartialEq, Eq)]
enum EnterAction { Pick, Submit, Nothing }

/// O que o Enter faz no diálogo. `edited`: só na busca, se o texto mudou desde a escolha da pasta (`None` no Nome). A tela
/// sem sessão e a transferência têm o próprio botão de seguir, e ali o Enter só escolhe a pasta.
fn enter_action(locked: bool, picked: bool, edited: Option<bool>) -> EnterAction {
    match edited {
        Some(edited) if locked || !picked || edited => EnterAction::Pick,
        None if locked || !picked => EnterAction::Nothing,
        _ => EnterAction::Submit,
    }
}

/// A busca saiu da pasta escolhida: outro texto, outra pasta listada (entrou numa subpasta) ou o realce noutra linha, pelas
/// setas ou pelo ponteiro. Aí o Enter escolhe a realçada em vez de começar a sessão.
fn search_moved(query: &str, picked_query: &str, dir: &str, picked_dir: &str, active: Option<&str>, picked: Option<&str>) -> bool {
    query != picked_query || dir != picked_dir || active.is_some_and(|active| Some(active) != picked)
}

fn next_root(current: usize, count: usize, reverse: bool) -> Option<usize> {
    if count == 0 { return None; }
    Some(if reverse { (current + count - 1) % count } else { (current + 1) % count })
}

#[derive(Clone, Debug, Deserialize)]
struct Probe { disponivel: bool, #[serde(default)] default: bool }

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Checkout { current: Option<String>, branches: Vec<String>, remotes: Vec<String>, dirty: bool }

fn checkout_of(result: Result<Value, Failure>) -> Result<Option<Checkout>, String> {
    match result {
        Err(error) if error.status == Some(404)
            || (error.status == Some(409) && error.detail.contains("not a git repository")) => Ok(None),
        Err(error) => Err(Hangar::fetch_failure(&error)),
        Ok(value) => serde_json::from_value(value).map(Some).map_err(|_| tr("invalid_response")),
    }
}

fn default_base(c: &Checkout) -> Option<String> { c.current.clone().or_else(|| c.branches.first().cloned()) }

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

fn choose_codex_account(accounts: &[CodexAccount], requested: Option<&str>) -> Option<usize> {
    match requested {
        Some(id) => accounts.iter().position(|a| a.id == id),
        None => accounts.iter().position(|a| a.is_default && a.auth.status == "connected")
            .or_else(|| accounts.iter().position(|a| a.auth.status == "connected"))
            .or_else(|| accounts.iter().position(|a| a.is_default)).or((!accounts.is_empty()).then_some(0)),
    }
}

enum SessionDialogPurpose {
    Create,
    TransferClaudeToCodex { target: Target, source_life: String, source_jsonl: String },
}

#[derive(Clone, Debug)]
pub(super) struct TransferRequest {
    target: Target, source_life: String, source_jsonl: String, credential_id: String,
    model: Option<String>, effort: Option<String>, seq: u64,
}

impl TransferRequest {
    fn body(&self) -> Value {
        json!({"credential_id": self.credential_id, "source_life": self.source_life,
            "source_jsonl": self.source_jsonl, "model": self.model, "effort": self.effort})
    }

    fn source_matches(&self, session: &SessionInfo) -> bool {
        session.name == self.target.name && super::sidebar::transfer_source(session) == Some((self.source_life.as_str(), self.source_jsonl.as_str()))
    }

    fn confirmed_choices<'a>(&self, value: &'a Value) -> Option<(&'a str, Option<&'a str>)> {
        let (model, effort) = effective_transfer_choices(value)?;
        if self.model.as_deref().is_some_and(|chosen| chosen != model) || self.effort.as_deref().is_some_and(|chosen| Some(chosen) != effort) {
            return None;
        }
        Some((model, effort))
    }
}

pub(super) enum TransferReply {
    Canceled,
    Requested(TransferRequest),
    Finished(TransferRequest, Result<SessionInfo, Failure>),
}

fn transfer_dialog_matches(current: Option<EntityId>, reply: EntityId) -> bool { current == Some(reply) }

fn transfer_failure(error: &Failure) -> String {
    let key = ["session_transfer_restore_failed", "session_transfer_source_changed"].into_iter().find(|key| error.detail.starts_with(*key));
    match key {
        Some(key) => match error.detail.strip_prefix(&format!("{key}: ")) {
            Some(reason) => format!("{} {reason}", tr(key)),
            None => tr(key),
        },
        None => Hangar::fetch_failure(error),
    }
}

fn effective_transfer_choices(value: &Value) -> Option<(&str, Option<&str>)> {
    let model = value.get("model")?.as_str()?;
    if model.is_empty() || model.len() > 128 || model.starts_with('-') || model == "default"
        || !model.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:~/[]-".contains(&b)) { return None; }
    // Os níveis variam por modelo; a forma segue a validação do backend, sem lista fechada.
    let effort = match value.get("effort")? {
        Value::Null => None,
        Value::String(effort) if (2..=32).contains(&effort.len()) && effort != "default"
            && effort.bytes().all(|b| b.is_ascii_lowercase()) => Some(effort.as_str()),
        _ => return None,
    };
    Some((model, effort))
}

async fn transferred(api: &Api, request: &TransferRequest) -> Result<SessionInfo, Failure> {
    let value = api.act(&request.target.name, &["conta"], Some(request.body()), false, 300).await?;
    let id = value.get("transfer_id").and_then(Value::as_str).filter(|id| !id.is_empty());
    let choices = request.confirmed_choices(&value);
    if value.get("ok").and_then(Value::as_bool) != Some(true) || value.get("provider").and_then(Value::as_str) != Some("codex")
        || value.get("conta").and_then(Value::as_str) != Some(request.credential_id.as_str()) || id.is_none() || choices.is_none() {
        return Err(Failure::local(tr("invalid_response")));
    }
    let id = id.unwrap();
    let (model, effort) = choices.unwrap();
    let snapshot = api.server_read(&["sessions"], &[], 15).await
        .map_err(|error| Failure::local(format!("{} {}", tr("session_transfer_refresh_failed"), Hangar::fetch_failure(&error))))?;
    let sessions: Vec<SessionInfo> = serde_json::from_value(snapshot.clone()).map_err(|_| Failure::local(tr("invalid_response")))?;
    let Some(rows) = snapshot.as_array() else { return Err(Failure::local(tr("invalid_response"))) };
    sessions.into_iter().zip(rows).find(|(s, row)| s.name == request.target.name && s.provider == "codex" && s.readable()
        && s.conta.as_deref() == Some(request.credential_id.as_str()) && s.transfer_id.as_deref() == Some(id)
        && s.transfer_phase.as_deref() == Some("complete")
        && row.get("model").is_none_or(|value| value.as_str() == Some(model))
        && row.get("effort").is_none_or(|value| match value { Value::Null => effort.is_none(), Value::String(value) => Some(value.as_str()) == effort, _ => false }))
        .map(|(session, _)| session)
        .ok_or_else(|| Failure::local(tr("session_transfer_refresh_failed")))
}

/// A sessão aberta pela resposta do backend e os avisos já em texto (reconciliação da conta, sessão achada pela lista).
/// `warning`: a continuação nasceu com outra coisa que a pedida (o resumo do Hangar no lugar do escrito pelo modelo).
pub(super) struct Opened { pub(super) session: SessionInfo, notes: Vec<String>, warning: Option<String> }

/// A sessão que o diálogo continua (modo bastão): o servidor monta o resumo dela e o manda à sessão nova.
/// `server`: chave da máquina da sessão continuada; o diálogo fica travado nela (o resumo é arquivo de lá).
pub(in crate::app) struct Baton { pub(in crate::app) name: String, pub(in crate::app) cwd: Option<String>, pub(in crate::app) server: String }

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
    RootScan(u64, String, u64, Result<Scan, String>),
    Branches(u64, Result<Option<Checkout>, String>),
    Sessions(u64, Result<Vec<SessionInfo>, Failure>),
    Providers(u64, Result<Value, Failure>),
    Configs(u64, Result<Value, Failure>),
    ConfigSuggestion(u64, u64, Result<Value, Failure>),
    Customizations(u64, customizations::CustomizationContext, Result<Value, Failure>),
    Codex(u64, Result<Value, Failure>),
    /// Passo da criação em voo; `None` é consulta que falhou, e o passo anterior fica.
    Step(u64, Option<String>),
    Created(u64, Result<Opened, String>),
    /// Seleção, o texto do campo, a mensagem que saiu (com os caminhos dos anexos) e a entrega dela.
    CreatedWithInput(u64, u64, String, String, Result<Delivery, Failure>, Result<Opened, String>),
    /// O catálogo de modelos e o último modelo e esforço lembrados para a chave dele.
    /// O catálogo, o modelo e esforço a pré-escolher e o padrão marcado do harness (modelo, esforço, permissão).
    Models(u64, Result<Value, Failure>, (String, String), Option<(String, String, String)>),
    Engines(u64, Result<Value, Failure>),
    /// A configuração do servidor: modo de sessão e Jev.
    Config(u64, (bool, Result<Option<Value>, Failure>)),
    HeadlessSaved(u64, Result<Value, Failure>),
    Quotas(u64, Result<Value, Failure>),
    Context(u64, Result<Value, Failure>),
    Account(u64, AccountDone),
    Archive(u64, Result<Value, Failure>),
    Preview(u64, Result<Value, Failure>),
    /// A amostra do resumo do bastão, em markdown.
    Baton(u64, Result<String, Failure>),
    /// O estado git da pasta escolhida.
    Git(u64, Result<Value, Failure>),
    /// Fetch, pull, troca ou criação de branch: a ação, a branch dela e o estado que voltou.
    GitDone(folder_git::GitAction, String, Result<Value, Failure>),
    /// As worktrees do servidor, para a pílula de branch mostrar as do repositório da pasta.
    Worktrees(u64, Result<Value, Failure>),
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

// A pasta pode ser de um servidor Windows: `\` também separa.
fn basename(path: &str) -> &str { crate::composer::basename(path) }

/// Caminho relativo ao pai da raiz: "pessoal/hangar".
fn rel_path(root: &str, path: &str) -> String {
    let parent = root.rsplit_once('/').map(|(p, _)| p).unwrap_or_default();
    if !parent.is_empty() { path.strip_prefix(&format!("{parent}/")).unwrap_or(path).to_owned() } else { path.to_owned() }
}

/// Migalhas da raiz até a pasta atual: rótulo e caminho de cada nível.
pub(super) fn crumbs(root: &Root, path: &str) -> Vec<(String, String)> {
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
pub(super) fn scan_of(result: Result<Value, Failure>) -> Result<Scan, String> {
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
type Picker<T = ModelChoice> = (Entity<SelectState<SearchableVec<T>>>, Subscription);

type Chosen = fn(&mut NewSession, String, &mut Window, &mut Context<NewSession>);

/// `searchable`: lista longa (catálogo de modelos) ganha busca por digitação no topo do menu.
fn picker<T: SearchableListItem<Value = String> + 'static>(choices: Vec<T>, at: Option<usize>, searchable: bool, chosen: Chosen,
    window: &mut Window, cx: &mut Context<NewSession>) -> Picker<T> {
    let state = cx.new(|cx| SelectState::new(SearchableVec::new(choices), at.map(IndexPath::new), window, cx).searchable(searchable));
    let subscription = cx.subscribe_in(&state, window, move |this, _, event: &SelectEvent<SearchableVec<T>>, window, cx| {
        if let SelectEvent::Confirm(Some(id)) = event { chosen(this, id.clone(), window, cx); cx.notify(); }
    });
    (state, subscription)
}

/// A conexão da abertura. Guardada no diálogo porque as respostas chegam com o `Hangar` em atualização, e o pedido seguinte
/// (a pasta da raiz que chegou) não pode passar por ele. `api` é a máquina onde a sessão vai nascer: começa na ativa e troca
/// no seletor de máquina sem mexer na conexão do app.
pub(in crate::app) struct Link { api: Api, runtime: Arc<Runtime>, tx: async_channel::Sender<Envelope>, connection: u64,
    servers: Vec<ServerChoice>, servers_rev: u64 }

/// Uma máquina da lista do app; `key` é o endereço normalizado. `offline`: a lista dela falhou na abertura do diálogo.
#[derive(Clone)]
pub(super) struct ServerChoice { pub(super) key: String, pub(super) label: String, pub(super) address: String, pub(super) token: String,
    pub(super) offline: bool }

/// Os menus da tela sem sessão, cada um preso à própria pílula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Menu { Machine, Folder, Model, Account, Branch, Git }

impl Menu {
    pub(in crate::app) fn anchor(self) -> &'static str {
        match self {
            Menu::Machine => "new-chat-machine", Menu::Folder => "new-chat-folder", Menu::Model => "new-chat-model",
            Menu::Account => "new-chat-account", Menu::Branch => "new-chat-branch", Menu::Git => "new-chat-git",
        }
    }
    /// Máquina e pasta ficam acima do compositor: o menu delas abre para cima, sem cobrir o campo.
    pub(in crate::app) fn above(self) -> bool { matches!(self, Menu::Machine | Menu::Folder) }
}

/// O diálogo "Nova sessão". Entidade própria: o diálogo é desenhado durante o desenho da janela, quando o `Hangar` não pode ser
/// escrito. Cada resposta volta com o número do pedido; a de um pedido velho (outra pasta, outro provider) cai.
pub(in crate::app) struct NewSession {
    link: Link,
    purpose: SessionDialogPurpose,
    transfer_blocked: bool,
    compact: bool,
    menu: Rc<std::cell::Cell<Option<Menu>>>,
    /// A busca dos menus de lista (máquina, modelo, conta, branch); a pasta usa a `query` da lista de pastas.
    menu_query: Entity<InputState>,
    servers: Remote<Vec<ServerChoice>>,
    roots: Remote<Vec<Root>>,
    root: Option<Root>,
    /// A pasta listada à esquerda.
    dir: String,
    scan: Remote<Scan>,
    /// Pastas da leitura que casam com a busca, na ordem da lista: refeito quando a busca ou a leitura mudam, não a cada quadro.
    folders: Vec<FolderIndex>,
    /// A pasta realçada pelas setas, guardada pelo caminho: a lista refeita (leitura que chega, busca) não a troca de linha.
    folder_active: Option<String>,
    folder_scroll: UniformListScrollHandle,
    root_scans: Vec<RootScan>,
    search_all: bool,
    search_error: Option<String>,
    query: Entity<InputState>,
    /// A pasta escolhida: é ela que o formulário configura.
    picked: Option<String>,
    /// O texto da busca quando a pasta foi escolhida: o Enter com a busca igual começa a sessão, com outra escolhe a pasta.
    picked_query: String,
    /// A pasta listada quando a pasta foi escolhida: entrar numa subpasta troca a lista sem mudar a busca.
    picked_dir: String,
    /// O Enter atual é repetição de tecla segurada: só a primeira pressão escolhe ou começa a sessão.
    enter_repeat: bool,
    /// Enter dado enquanto o nome sugerido ainda chegava: a sessão começa quando ele chegar.
    create_when_ready: bool,
    checkout: Remote<Option<Checkout>>,
    branch: String,
    /// Worktrees do repositório da pasta, para abrir a sessão numa que já existe.
    worktrees: Remote<Vec<super::worktrees::WorktreeStatus>>,
    /// A worktree existente escolhida na pílula de branch: a sessão nasce nela.
    existing: Option<String>,
    /// O menu de branch mostrando a troca da branch da própria pasta.
    switching: bool,
    /// A lista de bases da worktree nova aberta no menu.
    base_open: bool,
    /// Pasta e worktree pedidas de fora (página de worktrees) antes de as raízes chegarem.
    preset: Option<(String, Option<String>)>,
    /// A worktree nasce numa branch nova, criada a partir de `base`; o nome vazio vira o nome da sessão.
    new_branch: bool,
    base: String,
    new_branch_name: Entity<InputState>,
    /// O gerenciador de git da pasta (tela sem sessão) e o nome da branch nova dele.
    git: folder_git::GitPanel,
    git_name: Entity<InputState>,
    sessions: Remote<Vec<SessionInfo>>,
    same_folder: bool,
    name: Entity<InputState>,
    provider: &'static str,
    provider_touched: bool,
    providers: Remote<HashMap<String, Probe>>,
    configs: Remote<Vec<ConfigDir>>,
    config: Option<String>,
    config_pick: Option<Picker<choices::AccountChoice>>,
    customizations: customizations::CustomizationSelection,
    customizations_search: Entity<InputState>,
    customizations_focus: FocusHandle,
    codex: Remote<Vec<CodexAccount>>,
    codex_account: String,
    codex_pick: Option<Picker>,
    headless: bool,
    headless_owner: Option<bool>,
    headless_touched: bool,
    headless_saving: bool,
    difference: bool,
    manual_open: bool,
    manual: Entity<InputState>,
    choosing: bool,
    choose_error: Option<String>,
    create_seq: u64,
    pub(super) creating: bool,
    started: Option<Instant>,
    step: String,
    pub(super) error: Option<String>,
    /// O relógio do "passo · N s": anda sozinho a cada segundo, mesmo sem resposta do backend.
    clock: Option<Task<()>>,
    models: Remote<Catalog>,
    model: String,
    model_choice_touched: bool,
    /// A conta escolhida à mão no menu da tela sem sessão: a troca por cota esgotada não passa por cima dela.
    account_touched: bool,
    effort: String,
    service_tier: Option<String>,
    permission: String,
    /// O padrão marcado do harness, como foi lido na última leitura do catálogo.
    saved_default: Option<(String, String, String)>,
    /// Permissão escolhida à mão: o padrão do harness não a troca quando o catálogo é relido.
    permission_touched: bool,
    subagent: String,
    engine: String,
    engine_account: String,
    engine_account_pick: Option<Picker>,
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
    /// Com a conversa fechada aberta (`Hangar::reopen`), a conta Claude escolhida para retomá-la; o menu de conta passa a
    /// escolher esta, sem mexer na da tela sem sessão.
    pub(super) reopen_config: Option<Option<String>>,
    /// A dona é a conta do próprio servidor fora da lista (`config_dir` nulo): o menu ganha a linha "Padrão" para ela.
    pub(super) reopen_default: bool,
    /// "+ conta": a linha do nome aberta; "Apagar": a confirmação na tela.
    asking: bool,
    confirming: bool,
    /// Na confirmação de apagar: as conversas vão para a conta padrão (marcado ao abrir).
    keep_transcripts: bool,
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
        let menu_query = cx.new(|cx| InputState::new(window, cx).placeholder(tr("ctl_search")));
        let git_name = cx.new(|cx| InputState::new(window, cx).placeholder(tr("folder_git_name")));
        let new_branch_name = cx.new(|cx| InputState::new(window, cx).placeholder(tr_shared("worktree_nome_branch", &[])));
        let customizations_search = cx.new(|cx| InputState::new(window, cx).placeholder(tr("create_customizations_search")));
        // Com a pasta escolhida e o formulário à vista, o Enter na busca e no Nome começa a sessão, como o botão de baixo.
        let subscriptions = vec![
            cx.subscribe_in(&query, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => { this.refilter(cx); cx.notify() }
                InputEvent::PressEnter { .. } => this.enter_in_search(window, cx),
                _ => {}
            }),
            cx.subscribe(&name, |this: &mut Self, _, event: &InputEvent, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.enter_in_name(cx),
                _ => {}
            }),
            // Digitar depois de escolher uma branch existente volta a ser branch nova com o nome digitado.
            cx.subscribe(&new_branch_name, |this: &mut Self, input, event: &InputEvent, cx| if matches!(event, InputEvent::Change) {
                if !this.branch.is_empty() && input.read(cx).value().trim() != this.branch { this.branch.clear(); this.new_branch = true; }
                cx.notify()
            }),
            cx.subscribe(&menu_query, |_, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify() }),
            cx.subscribe(&customizations_search, |this: &mut Self, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) {
                this.refilter_customizations(cx); cx.notify();
            }),
            cx.subscribe(&git_name, |this: &mut Self, _, event: &InputEvent, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.git_create(cx),
                _ => {}
            }),
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
        let (search_all, search_error) = match appearance::search_all_folders() {
            Ok(value) => (value, None),
            Err(error) => (true, Some(format!("{} {error}", tr("create_search_preference_failed")))),
        };
        Self {
            link, purpose: SessionDialogPurpose::Create, transfer_blocked: false, compact: false, menu: Default::default(), menu_query, servers: Remote::default(),
            roots: Remote::default(), root: None, dir: String::new(), scan: Remote::default(), folders: Vec::new(),
            folder_active: None, folder_scroll: UniformListScrollHandle::new(), root_scans: Vec::new(), search_all, search_error, query, picked: None, picked_query: String::new(), picked_dir: String::new(), enter_repeat: false, create_when_ready: false,
            checkout: Remote::default(), branch: String::new(), worktrees: Remote::default(), existing: None, switching: false, base_open: false,
            preset: None, new_branch: false, base: String::new(), new_branch_name,
            git: Default::default(), git_name,
            sessions: Remote::default(), same_folder: false, name, provider: "claude", provider_touched: false, providers: Remote::default(), configs: Remote::default(),
            config: None, config_pick: None, customizations: Default::default(), customizations_search,
            customizations_focus: cx.focus_handle().tab_stop(true), codex: Remote::default(), codex_account: String::new(), codex_pick: None, headless: true, headless_owner: None, headless_touched: false, headless_saving: false,
            difference: false, manual_open: false, manual, choosing: false, choose_error: None, create_seq: 0, creating: false, started: None,
            step: String::new(), error: None, clock: None, models: Remote::default(), model: String::new(), model_choice_touched: false, account_touched: false, effort: String::new(), service_tier: None,
            permission: "bypassPermissions".into(), saved_default: None, permission_touched: false, subagent: String::new(), engine: String::new(), engine_account: String::new(), engine_account_pick: None, model_pick: None, effort_pick: None,
            permission_pick: None, subagent_pick: None, engine_pick: None, engines: Remote::default(), jev: Remote::default(), jev_on: false,
            more: false, omp, quotas: Remote::default(), reopen_config: None, reopen_default: false, asking: false, confirming: false, keep_transcripts: true, account_busy: false, account_seq: 0, account_name,
            notice: None, created_path: None, context_seq: 0, context_busy: false, context_on: None, context_want: None, context_error: None,
            archive: Remote::default(), want_resume: false, conversation: String::new(), before: None, preview: Remote::default(),
            preview_scroll: ScrollHandle::new(), resuming: false, baton, baton_by_model: false, baton_open: false,
            baton_preview: Remote::default(), _subscriptions: subscriptions,
        }
    }

    pub(in crate::app) fn for_transfer(link: Link, target: Target, source_life: String, source_jsonl: String,
        account: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut dialog = Self::new(link, None, window, cx);
        dialog.purpose = SessionDialogPurpose::TransferClaudeToCodex { target, source_life, source_jsonl };
        dialog.provider = "codex";
        dialog.codex_account = account;
        dialog.permission_touched = true;
        dialog.load_codex(cx);
        let seq = dialog.quotas.start();
        dialog.request(cx, move |api, send| Box::pin(async move {
            send(CreateReply::Quotas(seq, api.server_read(&["cotas"], &[], 30).await)).await
        }));
        dialog
    }

    fn is_transfer(&self) -> bool { matches!(self.purpose, SessionDialogPurpose::TransferClaudeToCodex { .. }) }

    pub(super) fn transfer_busy_for(&self, target: Option<&Target>) -> bool {
        self.creating && matches!(&self.purpose, SessionDialogPurpose::TransferClaudeToCodex { target: captured, .. } if Some(captured) == target)
    }

    fn accepts_transfer(&self, request: &TransferRequest) -> bool {
        self.creating && self.create_seq == request.seq && matches!(&self.purpose,
            SessionDialogPurpose::TransferClaudeToCodex { target, source_life, source_jsonl }
                if *target == request.target && *source_life == request.source_life && *source_jsonl == request.source_jsonl)
    }

    fn can_transfer(&self) -> bool {
        self.is_transfer() && !self.creating && !self.transfer_blocked && !self.quotas.loading && self.codex_ready()
            && !self.models.loading && self.has_transfer_models()
            && self.codex.ok().and_then(|l| l.iter().find(|a| a.id == self.codex_account))
                .and_then(|a| a.credential_id.as_deref()).is_some_and(|id| id.starts_with("codex:"))
            && self.transfer_quota_pct().is_none_or(|pct| pct < 99.)
    }

    fn transfer(&mut self, cx: &mut Context<Self>) {
        if !self.can_transfer() { return; }
        let SessionDialogPurpose::TransferClaudeToCodex { target, source_life, source_jsonl } = &self.purpose else { return };
        let Some(credential_id) = self.codex.ok().and_then(|l| l.iter().find(|a| a.id == self.codex_account))
            .and_then(|a| a.credential_id.clone()) else { return };
        self.create_seq += 1;
        let choice = |s: &String| (!s.is_empty()).then(|| s.clone());
        let request = TransferRequest { target: target.clone(), source_life: source_life.clone(), source_jsonl: source_jsonl.clone(),
            credential_id, model: choice(&self.model), effort: choice(&self.effort), seq: self.create_seq };
        self.creating = true;
        self.menu.set(None);
        self.error = None;
        if self.link.tx.try_send(Envelope { connection: self.link.connection, selection: None,
            payload: Payload::Transfer(cx.entity_id(), TransferReply::Requested(request)) }).is_err() {
            self.creating = false;
            self.error = Some(tr("connection_failed"));
        }
        cx.notify();
    }

    fn render_transfer(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let SessionDialogPurpose::TransferClaudeToCodex { target, .. } = &self.purpose else { return div() };
        let account = self.codex.ok().and_then(|l| l.iter().find(|a| a.id == self.codex_account));
        let quota = self.transfer_quota_pct();
        let height = (window.viewport_size().height - px(DIALOG_TOP + 80.)).min(window.rem_size() * 36.).max(window.rem_size() * 12.);
        div().w_full().max_h(height).flex().flex_col().gap_4()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr("session_transfer_title").replace("{name}", &target.name)))
            .child(div().id("session-transfer-fields").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_4()
                .child(muted(tr("session_transfer_help")))
                .child(self.render_codex_account(cx))
                .when_some(account.filter(|a| a.credential_id.as_deref().is_none_or(|id| !id.starts_with("codex:"))),
                    |el, _| el.child(alert("transfer-account-invalid", tr("session_transfer_account_removed"))))
                .when(self.quotas.loading, |el| el.child(muted(tr("loading"))))
                .when_some(self.quotas.value.as_ref().and_then(|q| q.as_ref().err()),
                    |el, error| el.child(muted(tr("accounts_failed").replace("{reason}", error))))
                .when_some(quota.filter(|pct| *pct >= 95.), |el, pct| el.child(muted(tr(if pct >= 99. {
                    "session_transfer_quota_full" } else { "session_transfer_quota_low" }).replace("{pct}", &format!("{pct:.0}")))))
                .child(self.render_transfer_model(cx))
                .when(self.models.loading || self.models.value.is_none(), |el| el.child(popup::skeleton("transfer-models-loading", 2)))
                .when(!self.models.loading && self.models.ok().is_some() && !self.has_transfer_models(),
                    |el| el.child(muted(tr("session_transfer_models_empty"))))
                .when(!self.models.loading && self.models.value.as_ref().is_some_and(|v| v.is_err()), |el|
                    el.child(alert("transfer-models-error", format!("{}: {}", tr("create_models_failed"),
                        self.models.value.as_ref().and_then(|v| v.as_ref().err()).cloned().unwrap_or_default())))
                    .child(Button::new("transfer-models-retry").outline().small().label(tr("create_try_again")).disabled(self.creating)
                        .on_click(cx.listener(|this, _, window, cx| { this.load_models(window, cx); cx.notify(); }))))
                .when_some(self.error.clone(), |el, error| el.child(alert("session-transfer-error", error))))
            .when(self.transfer_blocked, |el| el.child(muted(tr("session_transfer_check_session"))))
            .child(div().flex().items_center().justify_end().gap_2()
                .when(self.creating, |el| el.child(div().id("session-transfer-progress").role(Role::Status)
                    .text_sm().text_color(theme::muted()).child(tr("session_transfer_progress"))))
                .child(Button::new("session-transfer-cancel").outline().label(tr("cancel")).disabled(self.creating)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.creating { return; }
                        if this.link.tx.try_send(Envelope { connection: this.link.connection, selection: None,
                            payload: Payload::Transfer(cx.entity_id(), TransferReply::Canceled) }).is_err() {
                            this.error = Some(tr("connection_failed"));
                            cx.notify();
                        }
                    })))
                .child(Button::new("session-transfer-confirm").primary().label(tr("session_transfer_confirm"))
                    .disabled(!self.can_transfer() || self.quotas.loading).loading(self.creating)
                    .on_click(cx.listener(|this, _, _, cx| this.transfer(cx)))))
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

    fn load_roots(&mut self, cx: &mut Context<Self>) {
        let seq = self.roots.start();
        self.request(cx, move |api, send| Box::pin(async move {
            let last = tokio::task::spawn_blocking(crate::appearance::last_root).await.ok().flatten();
            send(CreateReply::Roots(seq, api.server_read(&["fs", "roots"], &[], 15).await, last)).await
        }));
    }

    /// As máquinas vêm da lista do app, já na mão: nada a buscar.
    fn load_servers(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let seq = self.servers.start();
        self.servers.finish(seq, Ok(self.link.servers.clone()));
        cx.notify();
    }

    /// Escolher outra máquina troca só o destino deste diálogo e relê o que é dela (raízes, pastas, contas, modelos); o app
    /// continua na máquina ativa até a sessão nascer, como o `targetServer` do web.
    fn pick_machine(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        self.menu.set(None);
        if self.creating || self.account_busy || self.baton.is_some() { return; }
        let Some(choice) = self.servers.ok().and_then(|list| list.iter().find(|c| c.key == key)).cloned() else { return };
        match Api::new(&choice.address, &choice.token) {
            Ok(api) => self.link.api = api,
            Err(error) => { self.error = Some(Hangar::failure(&error)); cx.notify(); return; }
        }
        (self.root, self.picked, self.config, self.config_pick) = (None, None, None, None);
        (self.same_folder, self.error, self.notice, self.created_path, self.asking, self.confirming) = (false, None, None, None, false, false);
        self.headless = true;
        self.headless_owner = None;
        self.headless_touched = false;
        self.headless_saving = false;
        self.dir.clear();
        self.folders.clear();
        self.folder_active = None;
        self.branch.clear();
        (self.new_branch, self.base) = (false, String::new());
        (self.existing, self.switching, self.preset) = (None, false, None);
        self.worktrees.reset();
        self.new_branch_name.update(cx, |input, cx| input.set_value("", window, cx));
        self.roots.reset();
        self.root_scans.clear();
        self.scan.reset();
        self.checkout.reset();
        self.sessions.reset();
        self.providers.reset();
        self.provider_touched = false;
        self.configs.reset();
        self.engines.reset();
        self.engine.clear();
        self.model.clear();
        self.effort.clear();
        self.engine_account.clear();
        self.engine_pick = None;
        self.engine_account_pick = None;
        // O catálogo da outra máquina não vale aqui; o novo vem depois das contas.
        self.models.reset();
        self.sync_customizations(window, cx);
        self.service_tier = None;
        self.before = None;
        self.load_target(cx);
        // O arquivo da pasta volta vazio até a pasta nova.
        if !self.compact { self.load_archive(window, cx); }
        cx.notify();
    }

    /// Chips de máquina no topo do diálogo (os do `CreateSessionSheet`): as ligadas e a atual mesmo fora do ar, com uma linha
    /// contando as que sumiram, senão a pessoa não sabe se a máquina foi apagada ou está desligada. No bastão a máquina fica a da
    /// origem: o resumo é arquivo de lá.
    fn render_machines(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let list = self.servers.ok()?;
        let shown: Vec<&ServerChoice> = list.iter().filter(|m| !m.offline || self.current_server(m)).collect();
        let hidden = list.len() - shown.len();
        if shown.len() < 2 && hidden == 0 { return None; }
        let locked = self.creating || self.account_busy || self.baton.is_some();
        let hint = match hidden {
            0 => None,
            1 => Some(tr("create_servers_offline_one")),
            n => Some(tr("create_servers_offline").replace("{n}", &n.to_string())),
        };
        Some(div().flex().flex_col().gap(px(6.))
            .when(shown.len() > 1, |el| el.child(div().id("create-machines").role(Role::Group).aria_label(tr("new_chat_server"))
                .flex().flex_wrap().items_center().gap(px(6.))
                .child(div().text_sm().text_color(theme::muted()).child(tr("new_chat_server")))
                .children(shown.iter().map(|machine| {
                    let on = self.current_server(machine);
                    let key = machine.key.clone();
                    let tip = if machine.offline { tr("create_server_offline").replace("{label}", &machine.label) } else { machine.address.clone() };
                    choice(SharedString::from(format!("create-machine-{}", machine.key)), on, cx).small().rounded_full()
                        .when(machine.offline, |b| b.icon(IconName::TriangleAlert))
                        .label(machine.label.clone()).tooltip(tip).disabled(!on && locked)
                        .when(!on, |b| b.on_click(cx.listener(move |this, _, window, cx| this.pick_machine(key.clone(), window, cx))))
                }))))
            .children(hint.map(muted))
            .when(self.baton.is_some() && shown.len() > 1, |el| el.child(muted(tr("create_baton_server_locked").replace("{s}", &self.server_label()))))
            .into_any_element())
    }

    /// O que é da máquina de destino. Contas Codex e o contexto delas só com o Codex escolhido.
    fn load_target(&mut self, cx: &mut Context<Self>) {
        self.load_roots(cx);
        self.load_providers(cx);
        self.load_configs(cx);
        if self.provider == "codex" { self.load_codex(cx); self.load_context(cx); }
        self.load_extras(cx);
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_servers(window, cx);
        self.load_target(cx);
        if self.compact { return; }
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
        (self.model_choice_touched, self.account_touched) = (false, false);
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Configs(seq, api.server_read(&["claude-configs"], &[], 15).await)).await }));
    }

    fn load_codex(&mut self, cx: &mut Context<Self>) {
        // O número continua do anterior: resposta da ida passada ao Codex não passa por desta.
        let seq = self.codex.start();
        if !self.is_transfer() { self.codex_account.clear(); }
        self.codex_pick = None;
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Codex(seq, api.server_read(&["codex-contas"], &[], 30).await)).await }));
    }

    fn select_root(&mut self, root: Root, window: &mut Window, cx: &mut Context<Self>) {
        let path = root.path.clone();
        // A pasta da criação em voo não muda: a coluna da esquerda fica parada até a resposta.
        if self.creating { return; }
        self.root = Some(root);
        if self.compact { self.picked = Some(path.clone()); self.reset_git(); self.load_branches(window, cx); }
        let remember = path.clone();
        self.link.runtime.spawn_blocking(move || crate::appearance::remember_root(&remember));
        self.scan_dir(path, cx);
    }

    pub(super) fn root_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let key = &event.keystroke;
        self.enter_repeat = event.is_held && key.key == "enter";
        if self.compact || self.is_transfer() || key.key != "tab" || !key.modifiers.control || key.modifiers.alt || key.modifiers.platform { return false; }
        if self.creating || self.headless_saving { return true; }
        let Some(roots) = self.roots.ok() else { return true; };
        let current = roots.iter().position(|root| self.root.as_ref().is_some_and(|r| r.path == root.path)).unwrap_or(0);
        if let Some(index) = next_root(current, roots.len(), key.modifiers.shift) {
            let root = roots[index].clone();
            self.select_root(root, window, cx);
        }
        true
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
        // A escolhida vira a realçada: um realce velho de antes da escolha não rouba o Enter seguinte.
        (self.folder_active, self.picked, self.error, self.same_folder) = (Some(path.clone()), Some(path), None, false);
        (self.picked_query, self.picked_dir, self.create_when_ready) = (self.query.read(cx).value().to_string(), self.dir.clone(), false);
        if self.compact {
            self.menu.set(None);
            self.reset_git();
            self.load_branches(window, cx);
            cx.notify();
            return;
        }
        let seq = self.sessions.start();
        self.request(cx, move |api, send| Box::pin(async move { send(CreateReply::Sessions(seq, api.sessions().await)).await }));
        self.load_archive(window, cx);
        self.sync_customizations(window, cx);
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

    /// Trocar de provider preserva o modo escolhido e relê as opções e permissões dele.
    fn set_provider(&mut self, provider: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_transfer() || self.creating { return; }
        self.provider_touched = true;
        if provider == self.provider { return; }
        (self.provider, self.error) = (provider, None);
        self.model.clear();
        self.effort.clear();
        self.permission = match provider { "codex" => "Full Access".into(), "claude" => "bypassPermissions".into(), _ => String::new() };
        self.permission_touched = false;
        if provider == "codex" { self.load_codex(cx); self.load_context(cx); } else { self.drop_context(); self.drop_codex(); }
        self.build_engine_account_pick(window, cx);
        self.load_models(window, cx);
        // A tela sem sessão não retoma conversa antiga: o arquivo da pasta não serve a ela.
        if !self.compact { self.load_archive(window, cx); }
        cx.notify();
    }

    fn set_headless(&mut self, headless: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.headless_saving || self.creating || (self.jev.loading && self.headless_owner.is_none()) { return; }
        self.headless = headless;
        self.headless_touched = true;
        self.build_permission_pick(window, cx);
        cx.notify();
        if self.headless_owner != Some(true) { return; }
        self.headless_saving = true;
        let seq = self.jev.seq;
        self.error = None;
        self.request(cx, move |api, send| Box::pin(async move {
            send(CreateReply::HeadlessSaved(seq, api.server_send(reqwest::Method::POST, &["config"],
                Some(json!({"headless_default": headless})), 8).await)).await;
        }));
    }

    fn headless_inherited(&self) -> bool { self.headless_owner != Some(true) && !self.headless_touched }

    fn provider_ready(&self) -> Option<bool> {
        if self.providers.loading { return None; }
        Some(self.providers.ok().and_then(|p| p.get(self.provider)).is_none_or(|p| p.disponivel))
    }

    fn codex_ready(&self) -> bool {
        self.provider != "codex" || (!self.codex.loading
            && self.codex.ok().and_then(|list| list.iter().find(|a| a.id == self.codex_account)).is_some_and(|a| a.auth.status == "connected"))
    }

    /// Pasta nova: o nome da branch nova digitado para a anterior não vale aqui.
    fn load_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_branch_name.update(cx, |input, cx| input.set_value("", window, cx));
        self.read_branches(true, cx);
    }

    /// Releitura da mesma pasta (depois de trocar a branch, ao fechar o gerenciador): a lista e a escolha ficam até a
    /// resposta chegar, sem o menu esvaziar no meio.
    pub(super) fn refresh_branches(&mut self, cx: &mut Context<Self>) { self.read_branches(false, cx); }

    fn read_branches(&mut self, fresh: bool, cx: &mut Context<Self>) {
        if self.creating { return; }
        if !self.compact { return; }
        let (Some(root), Some(path)) = (self.root.as_ref(), self.picked.clone()) else { return };
        let root = root.path.clone();
        let seq = self.checkout.start();
        if fresh {
            (self.checkout.value, self.branch, self.new_branch, self.base) = (None, String::new(), false, String::new());
            (self.existing, self.switching, self.base_open) = (None, false, false);
            self.load_folder_worktrees(cx);
        }
        self.request(cx, move |api, send| Box::pin(async move {
            let result = api.server_read(&["fs", "branches"], &[("root", root.as_str()), ("path", path.as_str())], 30).await;
            send(CreateReply::Branches(seq, checkout_of(result))).await;
        }));
        self.load_git(cx);
        cx.notify();
    }

    fn load_folder_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.picked.clone() else { return };
        let seq = self.worktrees.start();
        // Só o repositório da pasta e sem medir o disco: o menu não mostra espaço.
        self.request(cx, move |api, send| Box::pin(async move {
            let result = api.server_read(&["worktrees"], &[("repo", folder.as_str()), ("sizes", "false")], 30).await;
            send(CreateReply::Worktrees(seq, result)).await
        }));
    }

    /// As worktrees do repositório da pasta escolhida (a pasta é o principal, uma subpasta dele ou uma das worktrees).
    fn folder_worktrees(&self, result: Result<Value, Failure>) -> Result<Vec<super::worktrees::WorktreeStatus>, String> {
        let repos = result.map_err(|e| Hangar::fetch_failure(&e))
            .and_then(|v| serde_json::from_value::<Vec<super::worktrees::WorktreeRepo>>(v["repos"].clone()).map_err(|_| tr("invalid_response")))?;
        let picked = self.picked.clone().unwrap_or_default();
        let inside = |root: &str| super::worktrees::inside(&picked, root);
        Ok(repos.into_iter().find(|r| inside(&r.repo) || r.worktrees.iter().any(|w| inside(&w.path)))
            .map(|r| r.worktrees.into_iter().filter(|w| w.exists).collect()).unwrap_or_default())
    }

    /// Identidade da máquina desta tela: a página de worktrees só a usa se for a mesma.
    pub(super) fn machine(&self) -> String { self.link.api.identity() }

    /// Abre a tela já na pasta (e, com `existing`, com essa worktree escolhida na pílula de branch). Sem as raízes ainda,
    /// fica guardado até elas chegarem.
    pub(super) fn preset_folder(&mut self, folder: String, existing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating { return; }
        self.preset = Some((folder, existing));
        self.apply_preset(window, cx);
    }

    fn apply_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(roots) = self.roots.ok() else { return };
        let Some((folder, existing)) = self.preset.take() else { return };
        let inside = |root: &str| super::worktrees::inside(&folder, root);
        let Some(root) = roots.iter().filter(|r| inside(&r.path)).max_by_key(|r| r.path.len()).cloned() else {
            self.error = Some(format!("{} {folder}", tr("create_roots_failed")));
            cx.notify();
            return;
        };
        self.root = Some(root.clone());
        self.scan_dir(root.path, cx);
        self.pick(folder, window, cx);
        self.existing = existing;
        cx.notify();
    }

    pub(super) fn can_create(&self, cx: &App) -> bool {
        !self.is_transfer() && !self.customizations.is_open() && !self.creating && !self.headless_saving && !self.jev.loading && self.picked.is_some() && !self.sessions.loading && (self.compact || !self.name.read(cx).value().trim().is_empty())
            && self.provider_ready() == Some(true) && self.codex_ready() && self.engine_ready() && !(self.provider == "codex" && self.context_busy)
            && (!self.compact || ((self.provider != "claude" || (!self.configs.loading && self.configs.ok().is_some_and(|list| !list.is_empty())))
                && !self.models.loading && self.models.ok().is_some()
                && !self.checkout.loading))
    }

    pub(super) fn create(&mut self, first: Option<(u64, String, Vec<FirstFile>)>, cx: &mut Context<Self>) {
        if !self.can_create(cx) || self.compact != first.is_some() { return; }
        let Some(cwd) = self.picked.clone() else { return };
        // Worktree que já existe: a sessão nasce dentro dela, sem pedir worktree nova.
        let cwd = self.existing.clone().filter(|_| self.compact && !self.new_branch).unwrap_or(cwd);
        let mut name = if self.compact { basename(&cwd).to_owned() } else { self.name.read(cx).value().trim().to_owned() };
        let provider = self.provider;
        let typed = self.new_branch_name.read(cx).value().trim().to_string();
        let typed_empty = typed.is_empty();
        let wanted = if self.new_branch { if typed.is_empty() { name.clone() } else { typed } } else { self.branch.clone() };
        let mut requested_branch = (self.compact && !wanted.is_empty()).then(|| wanted.clone());
        // Sem nome digitado, a branch é o nome final da sessão, que só sai do `unique_name` abaixo.
        let branch_is_name = self.compact && self.new_branch && typed_empty && self.baton.is_none();
        let text = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
        let mut body = json!({"name": name, "cwd": cwd, "provider": provider, "model": text(&self.model), "effort": text(&self.effort)});
        if self.compact && !wanted.is_empty() {
            if let (Some(obj), Value::Object(extra)) = (body.as_object_mut(), worktree_body(&wanted, self.new_branch, &self.base)) {
                obj.extend(extra);
            }
        }
        match provider {
            "claude" => {
                body["config_dir"] = json!(self.config);
                body["engine"] = text(&self.engine);
                if self.proxy_accounts().is_some() { body["engine_account"] = json!(self.engine_account); }
                if !self.permission.is_empty() { body["permission_mode"] = json!(self.permission); }
                // O motor exporta o próprio modelo de subagente: com ele, o campo nem aparece.
                if self.engine.is_empty() && !self.subagent.is_empty() { body["subagent_model"] = json!(self.subagent); }
            }
            "codex" => { body["codex_account"] = json!(self.codex_account); },
            "omp" => { let profile = self.omp.read(cx).value().trim().to_owned(); if !profile.is_empty() { body["omp_profile"] = json!(profile); } }
            _ => {}
        }
        // Escolha incompatível não bloqueia a criação nem vai para outro harness.
        if let Some(tier) = self.service_tier_for_creation() { body["service_tier"] = json!(tier); }
        if matches!(provider, "claude" | "codex") && !self.headless_inherited() {
            body["headless"] = json!(self.headless);
            if provider == "codex" && self.headless { body["permission_mode"] = text(&self.permission); }
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
            if claude && self.proxy_accounts().is_some() { body["engine_account"] = json!(self.engine_account); }
            if self.headless_inherited() { body.as_object_mut().unwrap().remove("headless"); }
        }
        if let Some(customizations) = self.customization_payload() { body["claude_customizations"] = json!(customizations); }
        // Sonda falhada deixa o Claude por omissão, e isso não é escolha a lembrar.
        if self.provider_touched || self.providers.ok().is_some() {
            body["remember_provider"] = json!(true);
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
            if first.is_some() {
                let sessions = match api.sessions().await {
                    Ok(sessions) => sessions,
                    Err(error) => { send(CreateReply::Created(seq, Err(Hangar::fetch_failure(&error)))).await; return; }
                };
                name = unique_name(&name, &sessions.into_iter().map(|session| session.name).collect());
                body["name"] = json!(name);
                if branch_is_name {
                    body["branch"] = json!(name);
                    requested_branch = Some(name.clone());
                }
            }
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
                    opened(&api, result, &name, &cwd, requested_branch.as_deref()).await
                }
            };
            match (first, opened) {
                (Some((selection, text, files)), Ok(opened)) => {
                    // Os anexos sobem pela rota do servidor, na sessão que acabou de nascer; um que falhe segura a mensagem.
                    let mut uploads = Vec::new();
                    let mut failed = None;
                    for (file, bytes, image) in files {
                        match api.upload(&opened.session.name, &file, composer::mime_for(&file), bytes.to_vec()).await {
                            Ok(up) => uploads.push((image, up)),
                            // O aviso diz qual arquivo segurou a mensagem; ela não saiu, então a falha é certa.
                            Err(error) => { failed = Some(Failure::local(format!("{file}: {}", Hangar::failure(&error)))); break; }
                        }
                    }
                    let message = composer::compose_prompt(&text, &uploads, |speech| tr("attach_video_speech").replace("{texto}", speech));
                    let result = match failed { Some(error) => Err(error), None => api.send(&opened.session.name, &message).await };
                    send(CreateReply::CreatedWithInput(seq, selection, text, message, result, Ok(opened))).await;
                }
                (_, opened) => send(CreateReply::Created(seq, opened)).await,
            }
        }));
        cx.notify();
    }

    /// Guarda a resposta que ainda é deste diálogo; a criação que deu certo sai daqui para o `Hangar` abrir a sessão.
    fn receive(&mut self, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) -> Option<Opened> {
        match reply {
            CreateReply::HeadlessSaved(seq, result) => {
                if seq != self.jev.seq { return None; }
                self.headless_saving = false;
                if let Err(error) = result { self.error = Some(format!("{} {}", tr("session_mode_save_failed"), Hangar::fetch_failure(&error))); }
            }
            CreateReply::Roots(seq, result, last) => {
                let roots = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<Root>>(v).map_err(|_| tr("invalid_response")));
                if !self.roots.finish(seq, roots) { return None; }
                let list = self.roots.ok().cloned().unwrap_or_default();
                self.root_scans = list.iter().cloned().map(|root| RootScan { root, scan: Remote::default() }).collect();
                if let Some(root) = list.iter().find(|r| Some(&r.path) == last.as_ref()).or(list.first()).cloned() { self.select_root(root, window, cx); }
                self.apply_preset(window, cx);
                self.refilter(cx);
            }
            CreateReply::Worktrees(seq, result) => {
                if seq != self.worktrees.seq { return None; }
                let list = self.folder_worktrees(result);
                self.worktrees.finish(seq, list);
            }
            CreateReply::Scan(seq, result) => { if self.scan.finish(seq, result) { self.refilter(cx); } }
            CreateReply::RootScan(epoch, path, seq, result) => {
                if epoch != self.roots.seq { return None; }
                if let Some(cached) = self.root_scans.iter_mut().find(|s| s.root.path == path) {
                    if cached.scan.finish(seq, result) { self.refilter(cx); }
                }
            }
            CreateReply::Branches(seq, result) => {
                if !self.compact || !self.checkout.finish(seq, result) { return None; }
                // A base padrão da branch nova é a branch atual da pasta; com HEAD solto, a primeira local.
                if self.base.is_empty() {
                    self.base = self.checkout.ok().and_then(Option::as_ref).and_then(default_base).unwrap_or_default();
                }
            }
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
                if std::mem::take(&mut self.create_when_ready) { self.submit(cx); }
            }
            CreateReply::Providers(seq, result) => {
                let probes = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")));
                if !self.providers.finish(seq, probes) { return None; }
                if !self.provider_touched && !self.model_choice_touched && !self.creating && !self.is_transfer()
                    && self.providers.ok().is_some() {
                    // Sem padrão marcado, volta ao Claude: o provedor do servidor anterior não vale aqui.
                    let provider = self.providers.ok().and_then(|probes| PROVIDERS.into_iter()
                        .find(|provider| probes.get(*provider).is_some_and(|probe| probe.default && probe.disponivel)))
                        .unwrap_or("claude");
                    if provider != self.provider {
                        self.set_provider(provider, window, cx);
                        self.provider_touched = false;
                    }
                }
            }
            CreateReply::Configs(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<ConfigDir>>(v).map_err(|_| tr("invalid_response")));
                if !self.configs.finish(seq, list) { return None; }
                self.config = self.fallback_config();
                self.leave_exhausted_account();
                self.build_config_pick(window, cx);
                // Lista que falhou também pede o catálogo: sem conta, o backend usa a padrão.
                self.load_models(window, cx);
                if self.provider == "claude" && self.engine.is_empty() {
                    let models = self.models.seq;
                    self.request(cx, move |api, send| Box::pin(async move {
                        send(CreateReply::ConfigSuggestion(seq, models, api.server_read(&["cotas", "sugestao"], &[], 15).await)).await
                    }));
                }
            }
            CreateReply::ConfigSuggestion(seq, models, result) => {
                // Trocar conta, provider ou motor relê o catálogo e invalida a sugestão inicial.
                if seq != self.configs.seq || models != self.models.seq || self.model_choice_touched || self.creating || self.account_busy
                    || self.provider != "claude" || !self.engine.is_empty() || self.target().is_some() { return None; }
                let path = result.ok().and_then(|v| v.get("path").and_then(Value::as_str).map(str::to_owned));
                if let Some(path) = path.filter(|p| self.accounts().any(|c| &c.path == p) && self.config.as_ref() != Some(p)) {
                    self.config = Some(path);
                    self.build_config_pick(window, cx);
                    self.load_models(window, cx);
                }
            }
            CreateReply::Customizations(seq, context, result) => self.receive_customizations(seq, context, result, cx),
            CreateReply::Codex(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|v| serde_json::from_value::<Vec<CodexAccount>>(v).map_err(|_| tr("invalid_response")));
                if !self.codex.finish(seq, list) { return None; }
                let list = self.codex.ok().cloned().unwrap_or_default();
                let at = choose_codex_account(&list, self.is_transfer().then_some(self.codex_account.as_str()));
                if !self.is_transfer() { self.codex_account = at.map(|n| list[n].id.clone()).unwrap_or_default(); }
                let choices = list.iter().map(|a| ModelChoice { id: a.id.clone(), label: a.name.clone(), hint: a.hint() }).collect();
                self.codex_pick = Some(picker(choices, at, false, |this, id, window, cx| {
                    if this.creating { return; }
                    this.codex_account = id;
                    this.error = None;
                    this.load_models(window, cx);
                    if !this.is_transfer() { this.load_archive(window, cx); }
                }, window, cx));
                self.load_models(window, cx);
                if !self.is_transfer() { self.load_archive(window, cx); }
            }
            reply @ (CreateReply::Models(..) | CreateReply::Engines(..) | CreateReply::Config(..) | CreateReply::Quotas(..)
                | CreateReply::Context(..) | CreateReply::Account(..)) => self.receive_extra(reply, window, cx),
            reply @ (CreateReply::Archive(..) | CreateReply::Preview(..)) => self.receive_archive(reply, cx),
            reply @ (CreateReply::Git(..) | CreateReply::GitDone(..)) => { if !self.compact { return None; } self.receive_git(reply, window, cx) }
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
            CreateReply::Created(seq, result) | CreateReply::CreatedWithInput(seq, _, _, _, _, result) => {
                if seq != self.create_seq || !self.creating { return None; }
                (self.creating, self.resuming, self.started, self.clock) = (false, false, None, None);
                match result {
                    Ok(opened) => {
                        // A branch nova já existe: a próxima criação não pode repetir o nome.
                        (self.new_branch, self.base, self.existing) = (false, String::new(), None);
                        self.branch.clear();
                        self.new_branch_name.update(cx, |input, cx| input.set_value("", window, cx));
                        return Some(opened);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        self.sync_customizations(window, cx);
        cx.notify();
        None
    }
}

type Sender = Arc<dyn Fn(CreateReply) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A resposta do POST vira a sessão a abrir. Queda depois de mandar, ou resposta boa ilegível, não diz se ela nasceu: a lista
/// responde, pelo nome que o backend dá (a mesma limpeza) e pela pasta, e o aviso diz que foi achada assim.
pub(super) async fn opened(api: &Api, result: Result<Value, Failure>, name: &str, cwd: &str, branch: Option<&str>) -> Result<Opened, String> {
    match result {
        Ok(value) => {
            let accounts: Vec<&str> = value.get("avisos").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let notes = if accounts.is_empty() { Vec::new() } else { vec![tr("create_account_notes").replace("{n}", &accounts.join(" · "))] };
            match serde_json::from_value(value.clone()) {
                Ok(session) => Ok(Opened { session, notes, warning: None }),
                Err(_) => found(api, name, cwd, branch, tr("invalid_response")).await,
            }
        }
        Err(error) if error.uncertain && error.status.is_none() => found(api, name, cwd, branch, tr("connection_failed")).await,
        Err(error) => Err(Hangar::fetch_failure(&error)),
    }
}

/// A resposta do bastão traz só o nome da sessão nova: a sessão vem da lista. Lista que não responde não desfaz a passagem, que
/// já aconteceu: a sessão abre pelo nome e a lista seguinte completa o resto.
async fn handed(api: &Api, result: Result<Value, Failure>, name: &str, cwd: &str) -> Result<Opened, String> {
    let value = match result {
        Ok(value) => value,
        Err(error) if error.uncertain && error.status.is_none() => return found(api, name, cwd, None, tr("connection_failed")).await,
        Err(error) => return Err(Hangar::fetch_failure(&error)),
    };
    let Some(created) = value.get("name").and_then(Value::as_str).map(str::to_owned) else { return found(api, name, cwd, None, tr("invalid_response")).await };
    let warning = value.get("aviso").and_then(Value::as_str).filter(|a| !a.is_empty()).map(|a| tr("create_baton_summary_failed").replace("{motivo}", a));
    let listed = match api.sessions().await {
        Ok(list) => list.into_iter().find(|s| s.name == created),
        Err(error) => { eprintln!("bastao-lista: {}", error.detail); None }
    };
    let (session, mut notes) = match listed {
        Some(session) => (session, Vec::new()),
        None => (SessionInfo { name: created, cwd: Some(cwd.to_owned()), ..SessionInfo::default() }, vec![tr("create_baton_unlisted")]),
    };
    notes.extend(value.get("avisos").and_then(Value::as_array).into_iter().flatten()
        .filter_map(Value::as_str).map(str::to_owned));
    Ok(Opened { session, notes, warning })
}

/// Campos da worktree no corpo do `POST /api/sessions`; sem base, o backend parte da branch atual.
fn worktree_body(branch: &str, new_branch: bool, base: &str) -> Value {
    if new_branch {
        let mut v = json!({"branch": branch, "new_branch": true});
        if !base.is_empty() { v["base"] = json!(base); }
        v
    } else {
        json!({"branch": branch})
    }
}

// A worktree nasce ao lado do repositório, mesmo quando se escolhe uma subpasta dele.
fn created_here(session: &SessionInfo, name: &str, cwd: &str, branch: Option<&str>) -> bool {
    if session.name != name { return false; }
    match branch {
        None => session.cwd.as_deref() == Some(cwd),
        Some(branch) => session.branch.as_deref() == Some(branch) && session.cwd.as_deref().is_some_and(|path| {
            if path == cwd { return true; }
            let path = path.replace('\\', "/");
            let Some(origin) = path.strip_suffix(&format!("-{name}")) else { return false };
            let cwd = cwd.replace('\\', "/");
            cwd == origin || cwd.starts_with(&format!("{origin}/"))
        }),
    }
}

async fn found(api: &Api, name: &str, cwd: &str, branch: Option<&str>, failure: String) -> Result<Opened, String> {
    let clean = sanitize(name);
    let list = api.sessions().await.map_err(|_| failure.clone())?;
    list.into_iter().find(|s| created_here(s, &clean, cwd, branch))
        .map(|session| Opened { session, notes: vec![tr("create_found_in_list")], warning: None }).ok_or(failure)
}

/// Três níveis no formulário: o título do grupo (`group`), o rótulo do campo (`label`) e a ajuda (`muted`).
fn label(text: String) -> Div { div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(text) }

/// Grupo de campos com o título em caixa alta miúda, como os títulos dos menus; sem campo, o grupo não aparece.
fn group(title: &str, fields: Vec<AnyElement>) -> Option<Div> {
    (!fields.is_empty()).then(|| div().flex().flex_col().gap(px(16.))
        .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr(title).to_uppercase()))
        .children(fields))
}

fn muted(text: String) -> Div { div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text) }

fn alert(id: &'static str, text: String) -> Stateful<Div> {
    div().id(id).role(Role::Alert).text_size(px(12.5)).text_color(theme::danger()).whitespace_normal().child(text)
}

/// Opção escolhível (raiz, provider, onde roda): a escolhida com o fundo de destaque suave dos segmentos das Configurações.
pub(super) fn choice(id: impl Into<ElementId>, on: bool, cx: &App) -> Button {
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
        .foreground(if on { theme::accent_text() } else { theme::text() }).hover(theme::hover()).active(theme::hover()))
        .border_1().border_color(if on { theme::accent() } else { theme::border() }).when(on, |b| b.bg(theme::accent_dim()))
}

/// Escolha sem borda do diálogo (abas de raiz, linhas de pasta): realce de passagem neutro; a escolhida com o fundo de
/// destaque suave, que não muda sob o ponteiro, para seleção e passagem não se confundirem.
fn soft_choice(id: impl Into<ElementId>, on: bool, idle: Hsla, cx: &App) -> Button {
    let fill = if on { theme::accent_dim() } else { transparent_black() };
    let hover = if on { fill } else { theme::hover() };
    Button::new(id).custom(ButtonCustomVariant::new(cx).color(fill).foreground(if on { theme::accent_text() } else { idle })
        .hover(hover).active(hover)).selected(on)
}

/// Cartão de uma escolha entre duas (onde roda, quem escreve o resumo): o ponto de rádio, o título e o resumo da escolha.
fn option_card(id: &'static str, on: bool, title: String, beta: bool, summary: String, busy: bool, cx: &App) -> Button {
    choice(id, on, cx).flex_1().min_w_0().h_auto().py(px(10.)).px(px(12.)).rounded(px(10.)).selected(on).disabled(busy)
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
                .style({
                    let (font, size) = theme::original_code_typography(cx);
                    gpui_kit::component::text::TextViewStyle::default().heading_font_size(|_, _| px(14.))
                        .code_block(StyleRefinement::default().font_family(font.clone()).text_size(size))
                        .inline_code_font_family(font)
                })).into_any_element(),
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
            // No diálogo, as raízes são abas numa trilha em pílula; no menu de pasta da tela sem sessão, ficam como estavam.
            _ => div().id("create-roots").role(Role::Group).aria_label(tr("create_roots")).flex().flex_wrap()
                .map(|el| if self.compact { el.gap(px(8.)) } else { el.self_start().gap(px(2.)).p(px(3.)).rounded_full().bg(theme::inset()) })
                .children(self.roots.ok().into_iter().flatten().enumerate().map(|(n, root)| {
                    let on = self.root.as_ref().is_some_and(|r| r.path == root.path);
                    let pick = root.clone();
                    let id = SharedString::from(format!("create-root-{n}"));
                    if self.compact { choice(id, on, cx) } else { soft_choice(id, on, theme::muted(), cx).px(px(14.)) }
                        .small().rounded_full().label(root.name.clone()).disabled(self.creating)
                        .accessibility_label(root.name.clone()).tooltip(root.path.clone())
                        .on_click(cx.listener(move |this, _, window, cx| this.select_root(pick.clone(), window, cx)))
                })).into_any_element(),
        }
    }

    fn render_rows(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.root.is_none() { return div().into_any_element(); }
        let global = self.global_search(cx);
        let loading = if global { self.root_scans.iter().any(|s| s.scan.loading) } else { self.scan.loading || self.scan.value.is_none() };
        if loading && (!global || self.folders.is_empty()) {
            // Esqueletos são marcadores de posição, sem item de domínio: a posição é a identidade deles.
            return div().flex().flex_col().gap(px(6.)).children((0..5usize).map(|i| div().flex().flex_col().gap(px(6.)).px(px(10.)).py(px(8.))
                .child(Skeleton::new(("create-row-skeleton", i)).w(px(160.)).h(px(12.)))
                .child(Skeleton::new(("create-row-skeleton-detail", i)).secondary().w(px(240.)).h(px(10.))))).into_any_element();
        }
        if !global {
            let scan = match self.scan.value.as_ref() {
                Some(Err(error)) => return alert("create-scan-error", error.clone()).into_any_element(),
                Some(Ok(scan)) => scan,
                None => return div().into_any_element(),
            };
            if let Some(error) = scan.error.clone() { return muted(error).into_any_element(); }
        }
        if self.folders.is_empty() {
            let searching = !self.query.read(cx).value().trim().is_empty();
            return muted(tr(if searching { "create_no_results" } else { "create_no_subfolders" })).into_any_element();
        }
        // Só as linhas visíveis são montadas; todas têm a mesma altura, e o espaço entre elas vai dentro de cada uma.
        uniform_list("create-folder-list", self.folders.len(), cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
            range.filter_map(|ix| this.folder_row(ix, cx)).collect::<Vec<_>>()
        })).track_scroll(&self.folder_scroll).size_full().into_any_element()
    }

    fn folder_at(&self, ix: usize) -> Option<(&Root, &Entry)> {
        match self.folders.get(ix)? {
            FolderIndex::Current(ei) => Some((self.root.as_ref()?, self.scan.ok()?.entries.get(*ei)?)),
            FolderIndex::Root(ri, ei) => { let s = self.root_scans.get(*ri)?; Some((&s.root, s.scan.ok()?.entries.get(*ei)?)) }
        }
    }

    fn active_folder_index(&self) -> Option<usize> {
        let path = self.folder_active.as_deref()?;
        (0..self.folders.len()).find(|ix| self.folder_at(*ix).is_some_and(|(_, e)| e.path == path))
    }

    /// ↑/↓ no campo de busca andam o realce pela lista, dando a volta nas pontas como a busca de conversas.
    fn move_folder(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.creating { return; }
        let Some(next) = wrap_step(self.active_folder_index(), self.folders.len(), delta) else { return };
        self.folder_active = self.folder_at(next).map(|(_, e)| e.path.clone());
        self.folder_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Enter na busca: sem pasta escolhida, ou com a busca fora da escolha, escolhe a pasta; com o formulário à vista e a
    /// realçada sendo a escolhida, começa a sessão.
    fn enter_in_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.enter_repeat { return; }
        let moved = search_moved(&self.query.read(cx).value(), &self.picked_query, &self.dir, &self.picked_dir,
            self.folder_active.as_deref(), self.picked.as_deref());
        match enter_action(self.compact || self.is_transfer(), self.picked.is_some(), Some(moved)) {
            EnterAction::Pick => self.pick_active_folder(window, cx),
            EnterAction::Submit => self.submit(cx),
            EnterAction::Nothing => {}
        }
    }

    fn enter_in_name(&mut self, cx: &mut Context<Self>) {
        if self.enter_repeat { return; }
        if enter_action(self.compact || self.is_transfer(), self.picked.is_some(), None) == EnterAction::Submit { self.submit(cx); }
    }

    /// A ação do botão do formulário, também a do Enter: retoma a conversa escolhida ou cria. Com o nome sugerido ainda
    /// chegando, a criação espera por ele em vez de o Enter se perder.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.target().is_some() { return self.resume(cx); }
        if self.can_create(cx) { self.create(None, cx); }
        else if self.sessions.loading && !self.creating { self.create_when_ready = true; }
    }

    /// Enter no campo de busca escolhe a realçada; sem realce, com algo digitado, a primeira que casou.
    fn pick_active_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = !self.query.read(cx).value().trim().is_empty();
        let Some(ix) = self.active_folder_index().or_else(|| typed.then_some(0)) else { return };
        let Some((root, entry)) = self.folder_at(ix).map(|(r, e)| (r.clone(), e.path.clone())) else { return };
        self.pick_folder(root, entry, window, cx);
    }

    /// A linha de uma busca em todas as raízes pode ser de outra raiz: ela passa a ser a navegada.
    fn enter_root(&mut self, root: Root, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.as_ref().is_none_or(|r| r.path != root.path) { self.select_root(root, window, cx); }
    }

    fn pick_folder(&mut self, root: Root, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.enter_root(root, window, cx);
        self.pick(path, window, cx);
    }

    /// Busca, "em todas as pastas" e o aviso dela, iguais no diálogo e no menu de pasta da tela sem sessão; as setas do
    /// campo andam pela lista.
    fn folder_search(&self, input: Input, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            div().w_full()
                .capture_action(cx.listener(|this, _: &MoveUp, _, cx| { this.move_folder(-1, cx); cx.stop_propagation(); }))
                .capture_action(cx.listener(|this, _: &MoveDown, _, cx| { this.move_folder(1, cx); cx.stop_propagation(); }))
                .child(input.disabled(self.creating).aria_label(tr("create_search")))
                .into_any_element(),
            Checkbox::new(if self.compact { "new-chat-search-all" } else { "create-search-all" }).label(tr("create_search_all"))
                .checked(self.search_all).disabled(self.creating)
                .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_search_all(*checked, cx)))
                .into_any_element(),
            self.search_feedback(cx),
        ]
    }

    // A busca atravessa as raízes e sobrevive à troca de raiz, no diálogo e no menu da tela sem sessão.
    fn global_search(&self, cx: &App) -> bool { self.search_all && !self.query.read(cx).value().trim().is_empty() }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        if self.global_search(cx) {
            let epoch = self.roots.seq;
            let pending: Vec<_> = self.root_scans.iter_mut().filter(|s| s.scan.value.is_none() && !s.scan.loading)
                .map(|s| (s.root.path.clone(), s.scan.start())).collect();
            for (path, seq) in pending {
                self.request(cx, move |api, send| Box::pin(async move {
                    let result = scan_of(api.server_read(&["fs", "scan"], &[("root", path.as_str())], 15).await);
                    send(CreateReply::RootScan(epoch, path, seq, result)).await;
                }));
            }
            let mut seen = HashSet::new();
            let mut folders = Vec::new();
            for (root_index, root_scan) in self.root_scans.iter().enumerate() {
                let Some(scan) = root_scan.scan.ok() else { continue; };
                for (entry_index, entry) in scan.entries.iter().enumerate() {
                    if shown(&query, &root_scan.root.path, entry) && seen.insert(entry.path.clone()) {
                        folders.push(FolderIndex::Root(root_index, entry_index));
                    }
                }
            }
            self.folders = folders;
        } else {
            let root = self.root.as_ref().map(|r| r.path.as_str()).unwrap_or("");
            self.folders = self.scan.ok().into_iter().flat_map(|scan| scan.entries.iter().enumerate())
                .filter(|(_, e)| shown(&query, root, e)).map(|(ix, _)| FolderIndex::Current(ix)).collect();
        }
    }

    fn set_search_all(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.creating { return; }
        self.search_all = on;
        self.search_error = appearance::set_search_all_folders(on).err().map(|error| format!("{} {error}", tr("create_search_preference_failed")));
        self.refilter(cx);
        cx.notify();
    }

    fn search_feedback(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut feedback = div().flex().flex_col().gap(px(4.)).children(self.search_error.clone().map(|error| alert("create-search-preference-error", error)));
        if self.global_search(cx) {
            let errors: Vec<_> = self.root_scans.iter().filter_map(|root_scan| {
                let error = match root_scan.scan.value.as_ref()? {
                    Err(error) => error.as_str(),
                    Ok(scan) => scan.error.as_deref()?,
                };
                Some(format!("{}: {error}", root_scan.root.name))
            }).collect();
            if self.root_scans.iter().any(|s| s.scan.loading) { feedback = feedback.child(muted(tr("loading"))); }
            if !errors.is_empty() {
                feedback = feedback.child(alert("create-search-roots-errors", errors.join("\n")))
                    .child(Button::new("create-search-retry").ghost().small().label(tr("create_try_again")).disabled(self.creating)
                        .on_click(cx.listener(|this, _, _, cx| {
                            for s in &mut this.root_scans {
                                if matches!(s.scan.value, Some(Err(_))) || s.scan.ok().is_some_and(|scan| scan.error.is_some()) { s.scan.reset(); }
                            }
                            this.refilter(cx); cx.notify();
                        })));
            }
        }
        feedback.into_any_element()
    }

    fn folder_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (root, entry) = self.folder_at(ix)?;
        let pick_root = root.clone();
        let open_root = root.clone();
        let on = self.picked.as_deref() == Some(entry.path.as_str());
        let active = !on && self.folder_active.as_deref() == Some(entry.path.as_str());
        let hover_path = entry.path.clone();
        let (pick, open) = (entry.path.clone(), entry.path.clone());
        let relative_path = rel_path(&root.path, &entry.path);
        let display_path = if self.global_search(cx) { format!("{} · {relative_path}", root.name) } else { relative_path };
        // No diálogo, linha sem borda com o ícone da pasta à frente e etiquetas apagadas; o menu da tela sem sessão fica como estava.
        let soft = !self.compact;
        let badge = |text: &str| div().px(px(6.)).rounded(px(if soft { 5. } else { 4. })).text_size(px(10.5)).font_family(theme::MONO)
            .map(|el| if soft { el.py(px(1.)).bg(theme::hover()).text_color(theme::faint()) } else { el.border_1().border_color(theme::border()).text_color(theme::muted()) })
            .child(text.to_owned());
        let id = SharedString::from(format!("create-folder-{}", entry.path));
        let row = if soft { soft_choice(id, on, theme::text(), cx).py(px(8.)).rounded(px(10.)) } else { choice(id, on, cx).py(px(6.)).rounded(px(8.)) };
        // A linha da lista virtual não estica sozinha como o filho da coluna esticava.
        // O ponteiro que passa leva o realce: teclado e mouse nunca acendem duas linhas.
        Some(div().w_full().flex().items_center().gap(px(4.)).pb(px(2.))
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.folder_active.as_deref() != Some(hover_path.as_str()) { this.folder_active = Some(hover_path.clone()); cx.notify(); }
            }))
            .child(row.when(active, |el| el.bg(theme::hover())).disabled(self.creating).flex_1().min_w_0().h_auto().px(px(10.)).accessibility_label(entry.name.clone()).selected(on)
                .child(div().w_full().min_w_0().flex().items_center().gap(px(12.))
                .when(soft, |el| el.child(div().size(px(32.)).flex_shrink_0().rounded(px(8.)).bg(if on { theme::accent_dim() } else { theme::hover() })
                    .flex().items_center().justify_center()
                    .child(chrome::small_icon(if on { IconName::FolderOpen } else { IconName::Folder }, 16., if on { theme::accent() } else { theme::muted() }))))
                .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.))
                    .child(div().w_full().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(entry.name.clone()))
                    .child(div().w_full().flex().items_center().gap(px(6.))
                        .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(11.)).text_color(theme::muted())
                            .child(display_path))
                        .when(entry.is_git, |el| el.child(badge("git")))
                        .when(entry.has_claude_md, |el| el.child(badge("CLAUDE.md")))
                        .when_some(entry.mtime, |el, t| el.child(div().flex_shrink_0().text_size(px(11.)).text_color(theme::faint()).child(folder_time(t)))))))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_folder(pick_root.clone(), pick.clone(), window, cx))))
            .child(Button::new(SharedString::from(format!("create-open-{}", entry.path))).ghost().small().flex_shrink_0().disabled(self.creating)
                .icon(IconName::ChevronRight).accessibility_label(tr("create_open").replace("{nome}", &entry.name))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.enter_root(open_root.clone(), window, cx);
                    this.drill(open.clone(), window, cx);
                })))
            .into_any_element())
    }

    fn render_left(&self, cx: &mut Context<Self>) -> Div {
        let drilled = self.root.as_ref().filter(|r| r.path != self.dir && !self.global_search(cx));
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
        let title = div().text_xl().font_weight(FontWeight::SEMIBOLD).child(tr(if self.baton.is_some() { "create_baton_title" } else { "create_title" }));
        let column = div().w(relative(0.45)).flex_shrink_0().h_full().min_h_0().pr(px(20.)).border_r_1().border_color(theme::border()).flex().flex_col()
            .gap(px(14.)).child(match &self.baton {
                Some(b) => div().flex().flex_col().gap(px(4.)).child(title)
                    .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("create_baton_origin").replace("{n}", &b.name))),
                None => title,
            });
        // Com uma conversa escolhida, a coluna larga vira a leitura dela: a pasta já está escolhida.
        if let Some(c) = self.target() { return column.child(self.render_preview(c, cx)); }
        column
            .children(self.render_machines(cx))
            .child(self.render_roots(cx))
            .children(self.folder_search(Input::new(&self.query).rounded(px(10.)).cleanable(true)
                .prefix(chrome::small_icon(IconName::Search, 14., theme::muted())), cx))
            .children(path_row)
            // A lista de pastas rola sozinha; carregando, vazia ou com erro, a caixa é que rola.
            .child(div().id("create-folders").flex_1().min_h_0().flex().flex_col()
                .when(self.folders.is_empty() || self.scan.loading, |el| el.overflow_y_scroll()).child(self.render_rows(cx)))
            .child(footer)
    }

    fn render_codex_account(&self, cx: &mut Context<Self>) -> Div {
        let account = self.codex.ok().and_then(|list| list.iter().find(|a| a.id == self.codex_account)).cloned();
        div().flex().flex_col().gap(px(6.))
            .child(label(tr("create_codex_account")))
            .map(|el| match (&self.codex_pick, self.codex.value.as_ref()) {
                (_, _) if self.codex.loading => el.child(div().id("create-codex-loading").role(Role::Status).child(muted(tr("loading")))),
                (_, Some(Err(error))) => el.child(alert("create-codex-error", error.clone())),
                (Some((pick, _)), _) => el.child(Select::new(pick).disabled(self.creating).accessibility_label(tr("create_codex_account"))),
                _ => el,
            })
            .when(self.is_transfer() && !self.codex.loading && self.codex.ok().is_some_and(|l| l.is_empty()),
                |el| el.child(muted(tr("session_transfer_accounts_empty"))))
            .when(self.is_transfer() && !self.codex.loading && self.codex.ok().is_some_and(|l| !l.is_empty()) && account.is_none(),
                |el| el.child(alert("transfer-account-removed", tr("session_transfer_account_removed"))))
            .when(self.is_transfer() && !self.codex.loading && self.codex.value.as_ref().is_some_and(|v| v.is_err()), |el|
                el.child(Button::new("transfer-accounts-retry").outline().small().label(tr("create_try_again")).disabled(self.creating)
                    .on_click(cx.listener(|this, _, _, cx| { this.load_codex(cx); cx.notify(); }))))
            .when_some(account, |el, a| el.child(muted(a.hint())).children(self.render_codex_quota(a.credential_id.as_deref()))
                .children(a.sync.issues.iter().enumerate().map(|(n, issue)| {
                    let text = crate::i18n::tr_web(&issue.code, &issue.params).or_else(|| crate::i18n::tr_web("codex_account_error_unknown", &HashMap::new()))
                        .unwrap_or_else(|| issue.code.clone());
                    let ready = a.sync.status == "ready";
                    div().id(SharedString::from(format!("create-codex-issue-{n}"))).role(if ready { Role::Status } else { Role::Alert })
                        .text_size(px(12.5)).whitespace_normal().text_color(if ready { theme::muted() } else { theme::danger() }).child(text)
                })))
    }

    fn render_form(&self, path: &str, cx: &mut Context<Self>) -> Div {
        let checking = self.sessions.loading;
        let ready = self.provider_ready();
        let probe_error = self.providers.value.as_ref().and_then(|v| v.as_ref().err()).cloned();
        let missing = ready == Some(false);
        let busy = self.creating;
        // Controle segmentado na trilha das raízes do lado esquerdo: raio 11 por fora, 8 nos segmentos a 3 de distância.
        let providers = div().id("create-providers").role(Role::Group).aria_label(tr("create_provider_aria")).flex().gap(px(2.))
            .p(px(3.)).rounded(px(11.)).bg(theme::inset())
            .children(PROVIDERS.iter().map(|&p| {
                let available = self.providers.ok().and_then(|m| m.get(p)).is_none_or(|probe| probe.disponivel);
                let on = self.provider == p;
                soft_choice(SharedString::from(format!("create-provider-{p}")), on, theme::muted(), cx).flex_1().min_w_0().h(px(32.)).rounded(px(8.))
                    .disabled(!available || busy).accessibility_label(provider_name(p))
                    .child(div().flex().items_center().gap(px(6.)).text_sm().when(on, |el| el.font_weight(FontWeight::MEDIUM))
                        .child(chrome::provider_glyph(p, 16.)).child(provider_name(p)))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_provider(p, window, cx)))
            }));
        let target = self.target().cloned();
        let fresh = target.is_none();
        let claude = (self.provider == "claude").then(|| self.render_claude_account(cx));
        let codex = (self.provider == "codex").then(|| self.render_codex_account(cx));
        let modes = (fresh && matches!(self.provider, "claude" | "codex")).then(|| {
            let codex = self.provider == "codex";
            let inherited = self.headless_inherited();
            let mode = |id: &'static str, on: bool, title: String, beta: bool, summary: String, headless: bool| {
                option_card(id, on, title, beta, summary, busy || self.headless_saving || (self.jev.loading && self.headless_owner.is_none()), cx)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_headless(headless, window, cx)))
            };
            let help = match (codex, self.headless) {
                (false, false) => "create_mode_tmux_help", (false, true) => "create_mode_headless_help",
                (true, false) => "create_mode_tmux_help_codex", (true, true) => "create_mode_headless_help_codex",
            };
            let this = cx.entity().downgrade();
            div().flex().flex_col().gap(px(8.))
                .child(label(tr("create_mode")))
                .when(inherited, |el| el.child(muted(tr("session_mode_server_default"))))
                .child(div().id("create-modes").role(Role::Group).aria_label(tr("create_mode")).flex().gap(px(12.))
                    .child(mode("create-mode-tmux", !self.headless && !inherited, tr("create_mode_tmux"), false,
                        tr(if codex { "create_mode_tmux_summary_codex" } else { "create_mode_tmux_summary" }), false))
                    .child(mode("create-mode-headless", self.headless && !inherited, tr("create_mode_headless"), false,
                        tr(if codex { "create_mode_headless_summary_codex" } else { "create_mode_headless_summary" }), true)))
                .child(div().child(Disclosure::new("create-difference", self.difference, tr("create_mode_difference"), true)
                    .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| { this.difference = open; cx.notify(); }); })))
                .when(self.difference, |el| el.child(muted(tr(help))))
        });
        // O destino no topo, com a mesma pasta em destaque das linhas da lista ao lado.
        let destination = div().flex().items_center().gap(px(12.)).pr(px(28.))
            .child(div().size(px(36.)).flex_shrink_0().rounded(px(10.)).bg(theme::accent_dim()).flex().items_center().justify_center()
                .child(chrome::small_icon(IconName::FolderOpen, 18., theme::accent())))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().truncate().text_base().font_weight(FontWeight::SEMIBOLD).child(basename(path).to_owned()))
                .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).whitespace_normal().child(path.to_owned())));
        let head = div().flex().flex_col().gap(px(8.)).child(destination)
            .when(checking, |el| el.child(div().id("create-checking").role(Role::Status).child(muted(tr("create_checking")))))
            .when(!checking && self.same_folder, |el| el.child(div().id("create-same-folder").role(Role::Status).child(muted(tr("create_same_folder")))));
        let field = |title: String, control: AnyElement| div().flex().flex_col().gap(px(6.)).child(label(title)).child(control).into_any_element();
        // Retomar pelo proxy recebe modelo; nome, modo, esforço e permissão continuam fora desse pedido.
        let session = [
            fresh.then(|| field(tr("create_name"), Input::new(&self.name).disabled(busy).aria_label(tr("create_name")).into_any_element())),
            Some(div().flex().flex_col().gap(px(6.)).child(label(tr("create_provider"))).child(providers)
                .when_some(probe_error, |el, error| el.child(muted(tr("create_probe_failed").replace("{erro}", &error))
                    .id("create-probe-error").role(Role::Alert)))
                .when(missing, |el| el.child(alert("create-provider-missing", tr("create_provider_missing").replace("{p}", self.provider))))
                .into_any_element()),
            codex.map(IntoElement::into_any_element),
            self.render_engine().map(IntoElement::into_any_element),
            claude.map(IntoElement::into_any_element),
        ];
        let run = [
            modes.map(IntoElement::into_any_element),
            self.render_resume(cx).map(IntoElement::into_any_element),
            (fresh && self.provider == "omp").then(|| self.render_omp().into_any_element()),
            self.render_baton(cx).map(IntoElement::into_any_element),
        ];
        let agent = [
            (fresh || self.proxy_accounts().is_some()).then(|| self.render_trio(!fresh)).flatten().map(IntoElement::into_any_element),
            fresh.then(|| self.render_fast_choice(cx)).flatten().map(IntoElement::into_any_element),
            self.render_engine_context(cx).map(IntoElement::into_any_element),
            fresh.then(|| self.render_default_check(cx)).flatten().map(IntoElement::into_any_element),
            (fresh && self.provider == "codex").then(|| self.render_context(cx).into_any_element()),
            self.render_customizations_row(cx).map(IntoElement::into_any_element),
            self.render_more(cx).map(IntoElement::into_any_element),
        ];
        let groups = [("create_group_session", session.into_iter().flatten().collect()), ("create_group_run", run.into_iter().flatten().collect()),
            ("create_group_agent", agent.into_iter().flatten().collect())].into_iter().filter_map(|(title, fields)| group(title, fields));
        let fields = div().flex().flex_col().gap(px(20.)).pb(px(8.))
            .child(head)
            .when(!checking, |el| el.children(groups.map(|g| g.pt(px(20.)).border_t_1().border_color(theme::border()))));
        let can = self.can_create(cx);
        let seconds = self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let step = if self.step.is_empty() { tr("create_creating") } else { self.step.clone() };
        // Uma ação primária só: com uma conversa escolhida, o botão continua aquela conversa em vez de criar.
        let submit = match &target {
            Some(c) => Button::new("create-resume-submit").primary().large().w_full().label(self.resume_label(c)).loading(busy).disabled(busy || !self.resume_ready())
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            None => Button::new("create-submit").primary().large().w_full()
                .label(tr(if busy { "create_creating" } else if self.baton.is_some() { "create_baton_submit" } else { "create_submit" }))
                .loading(busy).disabled(!can && !busy).on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
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

/// Linha dos menus da tela sem sessão: o nome, a dica à direita e o visto na escolhida.
pub(super) fn menu_row(id: impl Into<ElementId>, on: bool, label: String, hint: String) -> Button { menu_row_with(id, on, label, hint, None) }

/// A mesma linha com uma segunda linha sob o nome: a cota da conta.
fn menu_row_with(id: impl Into<ElementId>, on: bool, label: String, hint: String, below: Option<AnyElement>) -> Button {
    popup::row(id, on).accessibility_label(label.clone()).child(div().w_full().flex().items_center().gap_2()
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
            .child(div().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(label))
            .children(below))
        .when(!hint.is_empty(), |el| el.child(div().flex_shrink_0().max_w(px(170.)).truncate().text_xs().text_color(theme::muted()).child(hint)))
        .when(on, |el| el.child(chrome::small_icon(IconName::Check, 16., theme::accent()))))
}

/// A busca dos menus, sem caixa, sobre o nome e a dica; `query` já vem em minúsculas.
pub(super) fn wanted(query: &str, label: &str, hint: &str) -> bool {
    query.is_empty() || label.to_lowercase().contains(query) || hint.to_lowercase().contains(query)
}

/// Pílula quieta acima e abaixo do compositor: texto apagado com ícone, a seta do menu e o realce enquanto aberto.
fn quiet_pill(menu: Menu, open: bool, icon: IconName, text: String, aria: String, disabled: bool, cx: &mut Context<NewSession>) -> Div {
    popup::anchor(div(), menu.anchor()).child(Button::new(menu.anchor())
        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted()).hover(theme::hover()).active(theme::hover()))
        .h(px(26.)).px(px(8.)).rounded(px(6.)).selected(open).disabled(disabled).accessibility_label(format!("{aria}: {text}"))
        .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
            .child(chrome::small_icon(icon, 14., theme::faint()))
            .child(div().max_w(px(220.)).truncate().child(text))
            .child(chrome::small_icon(IconName::ChevronDown, 12., theme::faint())))
        .on_click(cx.listener(move |this, _, window, cx| this.toggle_menu(menu, window, cx))))
}

impl NewSession {
    pub(super) fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.menu.get() != Some(menu) && !self.creating;
        let leaving_account = self.menu.get() == Some(Menu::Account) && !(open && menu == Menu::Account);
        self.menu.set(open.then_some(menu));
        if leaving_account && self.leave_exhausted_account() { self.build_config_pick(window, cx); self.load_models(window, cx); }
        if open && menu == Menu::Folder { self.query.update(cx, |input, cx| input.focus(window, cx)); }
        else if open { self.menu_query.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); }); }
        if open && menu == Menu::Git { self.git_opened(cx); }
        if open && menu == Menu::Branch { (self.switching, self.base_open) = (false, false); }
        cx.notify();
    }

    /// O texto da busca dos menus de lista, já em minúsculas.
    pub(super) fn menu_filter(&self, cx: &App) -> String { self.menu_query.read(cx).value().trim().to_lowercase() }

    pub(super) fn menu_search(&self) -> Div {
        div().px(px(4.)).pb(px(4.)).child(Input::new(&self.menu_query).h(px(32.)).aria_label(tr("ctl_search"))
            .prefix(chrome::small_icon(IconName::Search, 14., theme::faint())))
    }

    pub(super) fn menu_list(id: &'static str, rows: Vec<AnyElement>) -> AnyElement {
        if rows.is_empty() {
            return div().px(px(8.)).py(px(16.)).text_size(px(12.)).text_color(theme::muted()).text_center().child(tr("ctl_no_results")).into_any_element();
        }
        div().id(id).max_h(px(260.)).overflow_y_scroll().flex().flex_col().children(rows).into_any_element()
    }

    pub(super) fn menu_failure(id: &'static str, error: String, retry: fn(&mut Self, &mut Window, &mut Context<Self>), cx: &mut Context<Self>) -> AnyElement {
        div().p(px(8.)).flex().flex_col().items_start().gap(px(6.))
            .child(alert(id, error))
            .child(Button::new(SharedString::from(format!("{id}-retry"))).outline().xsmall().label(tr("retry"))
                .on_click(cx.listener(move |this, _, window, cx| { retry(this, window, cx); cx.notify(); })))
            .into_any_element()
    }

    fn current_server(&self, choice: &ServerChoice) -> bool { choice.key == super::servers::norm(&self.link.api.identity()) }

    fn server_label(&self) -> String {
        self.servers.ok().and_then(|list| list.iter().find(|c| self.current_server(c))).map(|c| c.label.clone())
            .unwrap_or_else(|| self.link.api.identity().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_owned())
    }

    /// Máquina e pasta, acima do compositor e à direita.
    pub(super) fn render_top_pills(&self, cx: &mut Context<Self>) -> Div {
        let open = self.menu.get();
        let machine = self.server_label();
        let folder = self.picked.as_deref().map(basename).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| tr("new_chat_folder"));
        let folders_ready = !self.roots.loading && self.roots.ok().is_some_and(|roots| !roots.is_empty());
        div().flex().justify_end().items_center().gap(px(2.)).pb(px(6.))
            .child(quiet_pill(Menu::Machine, open == Some(Menu::Machine), IconName::Monitor, machine, tr("new_chat_machine"), false, cx))
            .child(quiet_pill(Menu::Folder, open == Some(Menu::Folder), IconName::Folder, folder, tr("new_chat_folder"), !folders_ready, cx))
    }

    /// Conta e branch, abaixo do compositor e à esquerda; cada uma só quando existe para o provider e a pasta.
    pub(super) fn render_bottom_pills(&self, cx: &mut Context<Self>) -> Div {
        let open = self.menu.get();
        let account = match self.provider {
            "claude" if self.proxy_accounts().is_some() => Some((self.proxy_account().map(|a|
                if a.label.is_empty() { a.email.clone() } else { a.label.clone() }).unwrap_or_else(|| tr("create_proxy_choose_account")), tr("create_chatgpt_account"))),
            "claude" => self.configs.ok().filter(|list| !list.is_empty()).map(|list| {
                let label = list.iter().find(|c| Some(&c.path) == self.config.as_ref()).map(|c| c.label.clone()).unwrap_or_else(|| tr("create_default"));
                (label, tr("create_claude_account"))
            }),
            "codex" => self.codex.ok().filter(|list| !list.is_empty()).map(|list| {
                let label = list.iter().find(|a| a.id == self.codex_account).map(|a| a.name.clone()).unwrap_or_else(|| tr("create_default"));
                (label, tr("create_codex_account"))
            }),
            _ => None,
        };
        let branch = match (self.checkout.loading, self.checkout.ok()) {
            (true, _) => Some(tr("create_checkout_loading")),
            (false, Some(Some(_))) if self.new_branch => {
                let typed = self.new_branch_name.read(cx).value().trim().to_string();
                Some(tr_shared("worktree_menu_pill_nova", &[("branch", if typed.is_empty() { "…" } else { &typed })]))
            }
            (false, Some(Some(_))) if !self.branch.is_empty() => Some(tr_shared("worktree_menu_pill_nova", &[("branch", &self.branch)])),
            (false, Some(Some(_))) if self.existing.is_some() =>
                Some(tr_shared("worktree_menu_pill_existente", &[("nome", self.existing.as_deref().map(basename).unwrap_or_default())])),
            (false, Some(Some(checkout))) => Some(checkout.current.clone().unwrap_or_else(|| tr("create_checkout_current"))),
            _ => None,
        };
        div().flex().items_center().gap(px(2.)).pt(px(2.)).pl(px(6.))
            .children(account.map(|(label, aria)| quiet_pill(Menu::Account, open == Some(Menu::Account), IconName::CircleUser, label, aria,
                self.configs.loading || self.codex.loading, cx)))
            .children(branch.map(|label| quiet_pill(Menu::Branch, open == Some(Menu::Branch), IconName::GitBranch, label,
                tr("create_checkout_branch"), self.checkout.loading, cx)))
            .children(self.render_git_pill(open == Some(Menu::Git), cx))
    }

    /// A conta da conversa fechada aberta, abaixo do compositor: Claude troca pelo menu (com a cota de cada conta), Codex só
    /// mostra a de origem (o servidor recusa outra), os demais não têm conta.
    pub(super) fn render_reopen_account(&self, provider: &str, codex_label: String, cx: &mut Context<Self>) -> Option<Div> {
        let row = div().flex().items_center().gap(px(2.)).pt(px(2.)).pl(px(6.));
        match provider {
            "claude" | "" => {
                let list = self.configs.ok().filter(|list| !list.is_empty())?;
                let chosen = self.reopen_config.clone().flatten();
                // Conta fora da lista (pasta antiga) aparece pelo nome da pasta; sem caminho é a do servidor.
                let label = list.iter().find(|c| Some(&c.path) == chosen.as_ref()).map(|c| c.label.clone())
                    .or_else(|| chosen.as_deref().map(|p| basename(p).to_owned())).unwrap_or_else(|| tr("create_default"));
                Some(row.child(quiet_pill(Menu::Account, self.menu.get() == Some(Menu::Account), IconName::CircleUser, label,
                    tr("create_claude_account"), self.configs.loading, cx)))
            }
            "codex" => Some(row.child(div().id("reopen-codex-account").h(px(26.)).px(px(8.)).flex().items_center().gap(px(6.))
                .text_size(px(12.5)).text_color(theme::muted()).aria_label(format!("{}: {codex_label}", tr("create_codex_account")))
                .child(chrome::small_icon(IconName::CircleUser, 14., theme::faint()))
                .child(div().max_w(px(220.)).truncate().child(codex_label)))),
            _ => None,
        }
    }

    /// O que impede ou explica o envio, abaixo das pílulas: a criação em voo, a falha dela, ou a leitura que faltou.
    /// O nome que a sessão da tela sem sessão vai ter (a pasta; o desempate do servidor pode somar um número) e o agente.
    pub(super) fn opening_name(&self) -> String {
        self.existing.as_deref().filter(|_| !self.new_branch).or(self.picked.as_deref()).map(basename).unwrap_or_default().to_owned()
    }
    pub(super) fn provider(&self) -> &'static str { self.provider }
    /// O passo da criação em curso e os segundos desde o pedido.
    pub(super) fn progress(&self) -> (String, u64) { (self.step.clone(), self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0)) }

    pub(super) fn note(&self) -> Option<(String, bool)> {
        if self.creating { return Some((tr("new_chat_sending"), false)); }
        fn failed<T>(remote: &Remote<T>) -> Option<&String> { remote.value.as_ref().filter(|_| !remote.loading)?.as_ref().err() }
        let claude = self.provider == "claude";
        [
            self.error.clone(),
            self.proxy_note(),
            failed(&self.roots).map(|e| format!("{} {e}", tr("create_roots_failed"))),
            failed(&self.providers).cloned(),
            failed(&self.configs).filter(|_| claude).cloned(),
            failed(&self.codex).filter(|_| self.provider == "codex").cloned(),
            failed(&self.models).map(|e| format!("{}: {e}", tr("create_models_failed"))),
            failed(&self.checkout).map(|e| tr("create_checkout_failed").replace("{reason}", e)),
            (!self.roots.loading && self.roots.ok().is_some_and(Vec::is_empty)).then(|| tr("new_chat_no_roots")),
            (claude && !self.configs.loading && self.configs.ok().is_some_and(Vec::is_empty)).then(|| tr("new_chat_no_accounts")),
            (self.provider_ready() == Some(false)).then(|| tr("create_provider_missing").replace("{p}", self.provider)),
        ].into_iter().flatten().next().map(|text| (text, true))
    }

    /// O menu aberto, preso à pílula dele; `room` é a altura livre para a lista de pastas.
    pub(super) fn render_menu(&mut self, menu: Menu, room: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let query = self.menu_filter(cx);
        let body = match menu {
            Menu::Folder => return self.render_compact_folders(cx).p_3().max_h(room).into_any_element(),
            Menu::Model => return self.render_model_menu(cx).into_any_element(),
            Menu::Git => return self.render_git_menu(room, cx),
            Menu::Machine => match (&self.servers.value, self.servers.ok()) {
                _ if self.servers.loading => popup::skeleton("new-chat-machines", 2).into_any_element(),
                (Some(Err(error)), _) => Self::menu_failure("new-chat-machines-error", error.clone(), |this, window, cx| this.load_servers(window, cx), cx),
                (_, Some(list)) => {
                    let rows = list.iter().filter(|c| wanted(&query, &c.label, &c.address)).map(|choice| {
                        let current = self.current_server(choice);
                        let key = choice.key.clone();
                        menu_row(SharedString::from(format!("new-chat-machine-{}", choice.key)), current, choice.label.clone(),
                            if current { tr("create_current") } else { String::new() })
                            .when(!current, |row| row.on_click(cx.listener(move |this, _, window, cx| this.pick_machine(key.clone(), window, cx))))
                            .into_any_element()
                    }).collect();
                    div().child(Self::menu_list("new-chat-machine-list", rows)).into_any_element()
                }
                _ => div().into_any_element(),
            },
            Menu::Account if self.reopen_config.is_some() => {
                let chosen = self.reopen_config.clone().flatten();
                // `config_dir` nulo no resume = a conta dona da conversa, que aqui é a do servidor.
                let default = self.reopen_default.then(|| menu_row(SharedString::from("reopen-account-default"), chosen.is_none(),
                    tr("create_default"), String::new())
                    .on_click(cx.listener(|this, _, _, cx| { this.menu.set(None); this.reopen_config = Some(None); cx.notify(); }))
                    .into_any_element()).filter(|_| wanted(&query, &tr("create_default"), ""));
                let rows = default.into_iter().chain(self.accounts().filter(|c| wanted(&query, &c.label, "")).map(|c| {
                    let path = c.path.clone();
                    let quota = self.quota_line(format!("reopen-account-quota-{path}"), &format!("claude:{path}"));
                    menu_row_with(SharedString::from(format!("reopen-account-{path}")), chosen.as_ref() == Some(&c.path), c.label.clone(),
                        if c.active { tr("create_current") } else { String::new() }, quota)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu.set(None);
                            this.reopen_config = Some(Some(path.clone()));
                            cx.notify();
                        }))
                        .into_any_element()
                })).collect();
                Self::menu_list("reopen-account-list", rows)
            }
            Menu::Account if self.proxy_accounts().is_some() => {
                let rows = self.proxy_accounts().unwrap_or_default().iter().filter(|a| wanted(&query, &a.label, &a.email)).map(|account| {
                    let id = account.account.clone();
                    let quota = self.quota_line(format!("new-chat-engine-quota-{id}"), &account.credential_id);
                    menu_row_with(SharedString::from(format!("new-chat-engine-account-{id}")), id == self.engine_account,
                        if account.label.is_empty() { account.email.clone() } else { account.label.clone() }, account.email.clone(), quota)
                        .disabled(self.creating || self.engines.loading)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.creating { return; }
                            this.menu.set(None);
                            if this.engine_account != id {
                                this.engine_account = id.clone();
                                this.build_engine_account_pick(window, cx);
                                this.load_models(window, cx);
                            }
                            cx.notify();
                        })).into_any_element()
                }).collect();
                div().when_some(self.proxy_note(), |el, note| el.child(alert("new-chat-proxy-error", note)))
                    .child(Self::menu_list("new-chat-engine-accounts", rows)).into_any_element()
            }
            Menu::Account if self.provider == "codex" => {
                let rows = self.codex.ok().into_iter().flatten().filter(|a| wanted(&query, &a.name, &a.hint())).map(|account| {
                    let id = account.id.clone();
                    let quota = account.credential_id.as_deref().and_then(|c| self.quota_line(format!("new-chat-codex-quota-{id}"), c));
                    menu_row_with(SharedString::from(format!("new-chat-codex-{id}")), account.id == self.codex_account, account.name.clone(),
                        account.hint(), quota)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.menu.set(None);
                            if this.codex_account != id { this.codex_account = id.clone(); this.load_models(window, cx); }
                            cx.notify();
                        }))
                        .into_any_element()
                }).collect();
                Self::menu_list("new-chat-account-list", rows)
            }
            Menu::Account => {
                let rows = self.accounts().filter(|c| wanted(&query, &c.label, "")).map(|c| {
                        let path = c.path.clone();
                        let quota = self.quota_line(format!("new-chat-account-quota-{path}"), &format!("claude:{path}"));
                        let on = Some(&c.path) == self.config.as_ref();
                        menu_row_with(SharedString::from(format!("new-chat-account-{path}")), on, c.label.clone(),
                            if c.active { tr("create_current") } else { String::new() }, quota)
                            .disabled(!on && self.account_exhausted(&c.path))
                            .accessibility_label(Some(self.config_hint(c)).filter(|h| !h.is_empty())
                                .map_or_else(|| c.label.clone(), |hint| format!("{}, {hint}", c.label)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.menu.set(None);
                                (this.model_choice_touched, this.account_touched) = (true, true);
                                if this.config.as_ref() != Some(&path) { this.config = Some(path.clone()); this.load_models(window, cx); }
                                cx.notify();
                            }))
                            .into_any_element()
                    }).collect();
                Self::menu_list("new-chat-account-list", rows)
            }
            Menu::Branch => self.render_branch_menu(&query, cx),
        };
        div().p(px(popup::INSET)).flex().flex_col().gap(px(2.)).child(self.menu_search()).child(body).into_any_element()
    }

    /// A pílula de branch: a pasta principal, uma worktree nova, as worktrees que já existem e, num submenu, a troca da
    /// branch da própria pasta.
    fn render_branch_menu(&self, query: &str, cx: &mut Context<Self>) -> AnyElement {
        use super::worktrees::{age_days, stale_days, title};
        let Some(Some(checkout)) = self.checkout.ok() else { return div().into_any_element() };
        if self.switching { return self.render_switch_menu(checkout, query, cx); }
        let current = checkout.current.clone().unwrap_or_else(|| tr("create_checkout_current"));
        // Worktree nova: de uma branch nova (`new_branch`) ou de uma que já existe (`branch`).
        let fresh = self.new_branch || !self.branch.is_empty();
        let main = menu_row("new-chat-branch-main", !fresh && self.existing.is_none(), tr_shared("worktree_menu_pasta_principal", &[]), current)
            .disabled(self.creating)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.creating { return; }
                (this.new_branch, this.existing) = (false, None);
                this.branch.clear();
                this.menu.set(None);
                cx.notify();
            }));
        // O menu fica aberto: o nome e a base da branch nova aparecem logo abaixo.
        let new = menu_row("new-chat-branch-new", fresh, tr_shared("worktree_menu_nova", &[]), tr_shared("worktree_menu_nova_dica", &[]))
            .disabled(self.creating)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.creating { return; }
                if this.new_branch || !this.branch.is_empty() { return; }
                (this.new_branch, this.existing) = (true, None);
                this.new_branch_name.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }));
        let new_fields = fresh.then(|| {
            let bases: Vec<AnyElement> = if !self.base_open { Vec::new() } else {
                checkout.current.iter().chain(checkout.branches.iter().filter(|b| checkout.current.as_ref() != Some(*b))).map(|b| {
                    let base = b.clone();
                    menu_row(SharedString::from(format!("new-chat-base-{b}")), self.base == *b, b.clone(), String::new())
                        .on_click(cx.listener(move |this, _, _, cx| { this.base = base.clone(); this.base_open = false; cx.notify(); }))
                        .into_any_element()
                }).collect()
            };
            // Branches que já existem e casam com o digitado: escolher uma abre a worktree nela, sem criar branch. As abertas
            // em alguma worktree ficam de fora (aparecem como a worktree delas logo abaixo).
            let typed = self.new_branch_name.read(cx).value().trim().to_lowercase();
            let open_in: HashSet<&str> = self.worktrees.ok().into_iter().flatten().filter_map(|w| w.branch.as_deref()).collect();
            let remote: HashSet<&String> = checkout.remotes.iter().collect();
            let matching: Vec<AnyElement> = checkout.branches.iter().chain(&checkout.remotes)
                .filter(|b| checkout.current.as_ref() != Some(*b) && !open_in.contains(b.as_str()))
                .filter(|b| typed.is_empty() || b.to_lowercase().contains(&typed)).map(|b| {
                    let pick = b.clone();
                    let hint = if remote.contains(b) { tr_shared("worktree_nova_so_remoto", &[]) } else { String::new() };
                    menu_row(SharedString::from(format!("new-chat-existing-branch-{b}")), self.branch == *b, b.clone(), hint)
                        .disabled(self.creating)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if this.creating { return; }
                            (this.new_branch, this.existing, this.branch) = (false, None, pick.clone());
                            this.new_branch_name.update(cx, |input, cx| input.set_value(pick.clone(), window, cx));
                            this.menu.set(None);
                            cx.notify();
                        }))
                        .into_any_element()
                }).collect();
            div().px(px(8.)).py(px(4.)).flex().flex_col().gap(px(4.))
                .child(Input::new(&self.new_branch_name).small().aria_label(tr_shared("worktree_nome_branch", &[])))
                .when(!matching.is_empty(), |el| el.child(div().id("new-chat-existing-branches").max_h(px(160.)).overflow_y_scroll()
                    .flex().flex_col().children(matching)))
                .when(self.new_branch, |el| el.child(div().flex().child(Button::new("new-chat-branch-base").ghost().xsmall()
                    .label(format!("{} ⌄", tr_shared("worktree_menu_partir_de", &[("base", &self.base)])))
                    .on_click(cx.listener(|this, _, _, cx| { this.base_open = !this.base_open; cx.notify(); })))))
                .when(self.new_branch && self.base_open, |el| el.child(Self::menu_list("new-chat-base-list", bases)))
                .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("worktree_modo_ajuda", &[])))
                .when(checkout.dirty, |el| el.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("create_checkout_dirty"))))
        });
        let now = super::side::now_seconds();
        let existing = match (&self.worktrees.value, self.worktrees.ok()) {
            _ if self.worktrees.loading => popup::skeleton("new-chat-worktrees", 2).into_any_element(),
            (Some(Err(error)), _) => Self::menu_failure("new-chat-worktrees-error", error.clone(),
                |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| this.load_folder_worktrees(cx), cx),
            (_, Some(list)) => {
                let rows = list.iter().filter(|w| wanted(query, &title(w), w.branch.as_deref().unwrap_or(""))).map(|w| {
                    let on = !self.new_branch && self.existing.as_deref() == Some(w.path.as_str());
                    let stale = stale_days(w, now);
                    let hint = match (w.sessions.first(), stale, age_days(w, now)) {
                        (Some(name), _, _) => tr_shared("worktree_menu_em_uso", &[("nome", name)]),
                        (None, Some(d), _) => tr_shared("worktree_parada_dias", &[("n", &d.to_string())]),
                        (None, None, Some(d)) if d > 0 => tr_shared("worktree_dias_atras", &[("n", &d.to_string())]),
                        _ => String::new(),
                    };
                    let color = if !w.sessions.is_empty() { theme::accent() } else if stale.is_some() { theme::warning_text() } else { theme::faint() };
                    let (path, busy) = (w.path.clone(), !w.sessions.is_empty());
                    popup::row(SharedString::from(format!("new-chat-worktree-{}", w.path)), on).accessibility_label(title(w)).disabled(self.creating)
                        .child(div().w_full().flex().items_center().gap_2()
                            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(color))
                            .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(title(w)))
                            .when(!hint.is_empty(), |el| el.child(div().flex_shrink_0().max_w(px(170.)).truncate().text_xs().text_color(theme::muted()).child(hint)))
                            .when(on, |el| el.child(chrome::small_icon(IconName::Check, 16., theme::accent()))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.creating { return; }
                            (this.new_branch, this.existing) = (false, Some(path.clone()));
                            this.branch.clear();
                            // Ocupada, o menu fica aberto para o aviso de que outra sessão já trabalha nela.
                            if !busy { this.menu.set(None); }
                            cx.notify();
                        }))
                        .into_any_element()
                }).collect();
                Self::menu_list("new-chat-worktree-list", rows)
            }
            _ => div().into_any_element(),
        };
        let occupied = self.existing.as_ref().filter(|_| !self.new_branch)
            .and_then(|p| self.worktrees.ok()?.iter().find(|w| &w.path == p)?.sessions.first().cloned());
        let switch = popup::row("new-chat-branch-switch", false).accessibility_label(tr_shared("worktree_menu_trocar_branch", &[]))
            .child(div().w_full().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().truncate().text_sm().child(tr_shared("worktree_menu_trocar_branch", &[])))
                .child(chrome::small_icon(IconName::ChevronRight, 14., theme::faint())))
            .on_click(cx.listener(|this, _, _, cx| { this.switching = true; cx.notify(); }));
        div().child(main).child(new).children(new_fields)
            .child(popup::separator())
            .child(popup::title(tr_shared("worktree_menu_existentes", &[]), None))
            .child(existing)
            .children(occupied.map(|name| div().px(px(8.)).py(px(4.)).text_xs().text_color(theme::warning_text()).whitespace_normal()
                .child(tr_shared("worktree_menu_ocupada", &[("nome", &name)]))))
            .child(popup::separator())
            .child(switch)
            .into_any_element()
    }

    /// O submenu da troca da branch da própria pasta, com a recusa ou o resultado dela.
    fn render_switch_menu(&self, checkout: &Checkout, query: &str, cx: &mut Context<Self>) -> AnyElement {
        let locked = self.git_locked();
        let rows: Vec<AnyElement> = checkout.branches.iter().chain(&checkout.remotes)
            .filter(|b| checkout.current.as_ref() != Some(*b) && wanted(query, b, "")).map(|b| {
                let id = b.clone();
                menu_row(SharedString::from(format!("new-chat-branch-{id}")), false, b.clone(), String::new()).disabled(locked)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.creating { return; }
                        // A troca é na pasta: o menu fica aberto para mostrar a recusa ou o resultado.
                        (this.new_branch, this.existing) = (false, None);
                        this.branch.clear();
                        this.git_switch(folder_git::GitAction::Switch, id.clone(), cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }).collect();
        let back = popup::row("new-chat-branch-back", false).accessibility_label(tr("costs_back"))
            .child(div().w_full().flex().items_center().gap_2()
                .child(chrome::small_icon(IconName::ChevronLeft, 14., theme::faint()))
                .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(tr_shared("worktree_menu_trocar_branch", &[]))))
            .on_click(cx.listener(|this, _, _, cx| { this.switching = false; cx.notify(); }));
        div().child(back)
            .child(Self::menu_list("new-chat-branch-list", rows))
            .child(div().px(px(8.)).py(px(4.)).text_xs().text_color(theme::muted()).whitespace_normal().child(tr("create_checkout_switch_help")))
            .children(self.git_feedback(cx))
            .into_any_element()
    }

    pub(super) fn render_compact_folders(&self, cx: &mut Context<Self>) -> Div {
        div().w(rems(28.)).max_w_full().flex().flex_col().gap_3()
            .child(self.render_roots(cx))
            .children(self.folder_search(Input::new(&self.query).small().cleanable(true)
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint())), cx))
            // Buscando em todas as raízes, o caminho navegado não diz de onde vêm as linhas.
            .children(self.root.as_ref().filter(|_| !self.global_search(cx)).map(|root| div().flex().flex_wrap().gap_1()
                .children(crumbs(root, &self.dir).into_iter().map(|(text, path)|
                    Button::new(SharedString::from(format!("new-chat-crumb-{path}"))).ghost().small().label(text)
                        .on_click(cx.listener(move |this, _, window, cx| this.drill(path.clone(), window, cx)))))))
            // ponytail: lista mínima de 6rem; janela menor exige seletor em página própria.
            .child(div().flex_basis(rems(12.)).flex_shrink_1().min_h(rems(6.)).relative().flex().flex_col().child(self.render_rows(cx)))
            .child(div().flex().items_center().gap_2()
                .child(Button::new("new-chat-use-folder").outline().small().label(tr("create_use_folder"))
                    .on_click(cx.listener(|this, _, window, cx| this.pick(this.dir.clone(), window, cx))))
                .child(Button::new("new-chat-computer-folder").ghost().small().icon(IconName::FolderOpen).label(tr("create_computer_folder"))
                    .loading(self.choosing).on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx)))))
            .when_some(self.choose_error.clone(), |el, error| el.child(alert("new-chat-choose-error", error)))
    }
}

impl Render for NewSession {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_transfer() { return self.render_transfer(window, cx); }
        // A tela sem sessão é desenhada pelo `Hangar` (`render_new_chat`), em volta do compositor dele.
        if self.compact { return div(); }
        // Topo, margem de baixo do kit e o preenchimento do diálogo: o resto da janela, até a altura do web.
        let height = (window.viewport_size().height - px(DIALOG_TOP + 16. + 40.)).min(px(760.)).max(px(320.));
        let right = match self.picked.clone() {
            _ if self.customizations.is_open() => self.render_customizations(cx),
            Some(path) => self.render_form(&path, cx),
            None => div().flex_1().min_w_0().h_full().pl(px(20.)).flex().flex_col().items_center().justify_center().gap(px(6.))
                .child(div().size(px(88.)).mb(px(12.)).rounded_full().bg(theme::hover()).flex().items_center().justify_center()
                    .child(div().size(px(60.)).rounded_full().bg(theme::accent_dim()).border_1().border_color(theme::accent_focus())
                        .flex().items_center().justify_center().child(chrome::small_icon(IconName::FolderOpen, 26., theme::accent()))))
                .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(tr("create_empty_title")))
                .child(div().max_w(px(320.)).text_center().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("create_empty_sub"))),
        };
        div().h(height).w_full().flex().child(self.render_left(cx)).child(right)
    }
}

impl Hangar {
    pub(super) fn open_transfer(&mut self, target: Target, source_life: String, source_jsonl: String, account: String,
        window: &mut Window, cx: &mut Context<Self>) {
        if self.new_session.as_ref().is_some_and(|d| d.read(cx).creating) { return; }
        let Some(api) = self.machine_api(&target.server) else {
            window.push_notification(Notification::error(self.machine_error(&target.server)), cx);
            return;
        };
        self.focus_origin(&target, window, cx);
        let link = Link { api, runtime: self.runtime.clone(), tx: self.tx.clone(), connection: self.connection,
            servers: Vec::new(), servers_rev: self.servers_rev };
        let dialog = cx.new(|cx| NewSession::for_transfer(link, target.clone(), source_life, source_jsonl, account, window, cx));
        self.new_session = Some(dialog.clone());
        let weak = cx.entity().downgrade();
        let width = (window.viewport_size().width * 0.94).min(px(640.));
        window.open_dialog(cx, move |d, _, cx| {
            let busy = dialog.read(cx).creating;
            let (weak, me, target) = (weak.clone(), dialog.entity_id(), target.clone());
            popup::dialog(d).w(width).margin_top(px(DIALOG_TOP)).child(dialog.clone())
                .keyboard(!busy).overlay_closable(!busy).close_button(!busy).on_ok(enter_to_focused)
                .on_close(move |_, window, cx| { let _ = weak.update(cx, |this, cx| {
                    if transfer_dialog_matches(this.new_session.as_ref().map(Entity::entity_id), me) {
                        this.new_session = None;
                        this.focus_origin(&target, window, cx);
                        cx.notify();
                    }
                }); })
        });
        cx.notify();
    }

    pub(super) fn receive_agent_transfer(&mut self, dialog: EntityId, reply: TransferReply, window: &mut Window, cx: &mut Context<Self>) {
        if !transfer_dialog_matches(self.new_session.as_ref().map(Entity::entity_id), dialog) { return; }
        let Some(entity) = self.new_session.clone() else { return };
        if matches!(&reply, TransferReply::Canceled) {
            if entity.read(cx).creating { return; }
            let SessionDialogPurpose::TransferClaudeToCodex { target, .. } = &entity.read(cx).purpose else { return };
            let target = target.clone();
            self.new_session = None;
            window.close_dialog(cx);
            self.focus_origin(&target, window, cx);
            cx.notify();
            return;
        }
        let request = match &reply { TransferReply::Requested(request) | TransferReply::Finished(request, _) => request, TransferReply::Canceled => return };
        if !entity.read(cx).accepts_transfer(request) { return; }
        match reply {
            TransferReply::Canceled => {}
            TransferReply::Requested(request) => {
                if !self.sessions_of(&request.target.server).iter().any(|s| request.source_matches(s)) {
                    entity.update(cx, |d, cx| {
                        d.creating = false;
                        d.transfer_blocked = true;
                        d.error = Some(tr("session_transfer_source_changed"));
                        cx.notify();
                    });
                    return;
                }
                self.sidebar.moving.insert(request.target.clone(), (Instant::now(), None));
                let api = entity.read(cx).link.api.clone();
                let (tx, connection) = (self.tx.clone(), self.connection);
                self.runtime.spawn(async move {
                    let result = transferred(&api, &request).await;
                    let _ = tx.send(Envelope { connection, selection: None,
                        payload: Payload::Transfer(dialog, TransferReply::Finished(request, result)) }).await;
                });
            }
            TransferReply::Finished(request, result) => {
                self.sidebar.moving.remove(&request.target);
                let result = result.and_then(|session| {
                    // O mesmo nome recriado durante o pedido não recebe a resposta da vida anterior.
                    let current = self.sessions_of(&request.target.server).iter().find(|s| s.name == request.target.name);
                    if current.is_some_and(|s| !(s.lifecycle_id.as_deref() == Some(request.source_life.as_str())
                        && s.jsonl.as_deref() == Some(request.source_jsonl.as_str())) && s.transfer_id != session.transfer_id) {
                        return Err(Failure::local("session_transfer_source_changed"));
                    }
                    Ok(session)
                });
                entity.update(cx, |d, cx| {
                    d.creating = false;
                    if let Err(error) = &result {
                        d.transfer_blocked = error.uncertain || error.detail.contains("session_transfer_source_changed")
                            || error.detail.contains("session_transfer_restore_failed") || error.status.is_none();
                        d.error = Some(transfer_failure(error));
                    }
                    cx.notify();
                });
                match result {
                    Ok(session) => {
                        if self.is_active_key(&request.target.server) {
                            if let Some(row) = self.sessions.iter_mut().find(|s| s.name == request.target.name) { *row = session.clone(); }
                        } else if let Some(list) = self.remote.get_mut(&request.target.server) {
                            if let Some(row) = list.sessions.iter_mut().find(|s| s.name == request.target.name) { *row = session.clone(); }
                        }
                        let open = self.selected_target().as_ref() == Some(&request.target);
                        self.new_session = None;
                        window.close_dialog(cx);
                        if open {
                            self.select_on(&request.target.server, session, window, cx);
                            self.load_session_accounts(cx);
                            self.composer.update(cx, |input, cx| input.focus(window, cx));
                        }
                        self.sidebar_sessions_changed(window, cx);
                    }
                    Err(_) => {
                        if self.selected_target().as_ref() == Some(&request.target) {
                            let list = self.sessions_of(&request.target.server).to_vec();
                            self.follow_open(&list, window, cx);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// Sem sessão escolhida e com servidor: a faixa de baixo vira a tela de nova conversa.
    pub(super) fn new_chat_screen(&self) -> bool { self.selected.is_none() && self.api.is_some() }

    /// Onde o compositor guarda anexos: a sessão aberta ou, na tela sem sessão antes do Enviar, a conversa por nascer.
    /// Com a criação em voo não há onde pôr: o que já foi anexado segue com ela.
    pub(super) fn composer_key(&self) -> Option<SessionKey> {
        // A conversa fechada aberta não anexa: os anexos sobem para uma sessão, e ela só existe depois do Enviar.
        self.selected_key().or_else(|| (self.new_chat_screen() && self.opening.is_none() && self.reopen.is_none()).then(new_chat_key))
    }

    /// A máquina escolhida nos chips da tela sem sessão, que é onde a conversa vai nascer.
    pub(super) fn new_chat_api(&self, cx: &App) -> Option<Api> {
        self.new_chat.as_ref().filter(|_| self.new_chat_screen()).map(|view| view.read(cx).link.api.clone())
    }

    /// O provider que a tela sem sessão vai criar (o texto do campo diz a quem se escreve).
    pub(super) fn new_chat_provider(&self, cx: &App) -> &'static str {
        self.new_chat.as_ref().map(|view| view.read(cx).provider).unwrap_or("claude")
    }

    /// A pílula de modelo e esforço, dentro do compositor da tela sem sessão.
    pub(super) fn new_chat_pills(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.new_chat.clone().map(|view| view.update(cx, |view, cx| view.render_model_pill(cx).into_any_element())).into_iter().collect()
    }

    /// Nova conversa sem sessão, no desenho do Zeron: o fundo da janela, o compositor no meio, máquina e pasta acima dele,
    /// conta e branch abaixo. Enviar cria a sessão com essas escolhas e manda a mensagem (`NewSession::create`).
    pub(super) fn render_new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return div().flex_1().flex().items_center().justify_center().text_color(theme::muted()).child(tr("choose_session")).into_any_element();
        };
        self.home_usage_opened(cx);
        let view = self.ensure_new_chat(api, window, cx);
        view.update(cx, |view, _| view.reopen_config = None);
        if self.opening.is_some() && view.read(cx).creating { return self.render_opening(view, window, cx); }
        let (top, bottom, note) = view.update(cx, |view, cx| (view.render_top_pills(cx), view.render_bottom_pills(cx), view.note()));
        // Anexo recusado (grande demais, ilegível) avisa aqui, onde a tela sem sessão mostra os avisos dela.
        let note = self.action_feedback.get(&new_chat_key()).cloned().or(note);
        let composer = self.render_composer(false, false, false, 0, false, false, window, cx);
        // A tela chega como um objeto só, descendo 10 px até o lugar (o `settle-down` do kit, no tempo do `fade-in`).
        let settle = motion::enter("new-chat-in", motion::FADE_IN, window, cx);
        // O fundo pertence à janela; a tela vazia nunca o cobre com uma superfície opaca. O compositor fica um pouco acima do meio.
        div().id("new-chat").size_full().overflow_y_scroll().flex().flex_col()
            .child(motion::settle_down(div(), settle).my_auto().pb(rems(4.)).w_full().flex_shrink_0().flex().flex_col()
                .child(landing_column(self.render_home_usage(cx)).mb_6())
                .child(landing_column(popup::anchor(top, super::landing::TOP)))
                .child(composer)
                .child(landing_column(div().flex().flex_col().gap_1().child(popup::anchor(bottom, super::landing::BOTTOM))
                    .children(note.map(|(text, warning)| div().id("new-chat-note").role(if warning { Role::Alert } else { Role::Status })
                        .px(px(14.)).text_sm().whitespace_normal().text_color(if warning { theme::warning() } else { theme::muted() }).child(text))))))
            .into_any_element()
    }

    /// A view da tela sem sessão, refeita quando a conexão ou a lista de máquinas mudam. A conversa fechada aberta usa a
    /// mesma, pelas contas e cotas que ela já lê.
    pub(super) fn ensure_new_chat(&mut self, api: Api, window: &mut Window, cx: &mut Context<Self>) -> Entity<NewSession> {
        if self.new_chat.as_ref().is_none_or(|view| { let link = &view.read(cx).link; link.connection != self.connection || link.servers_rev != self.servers_rev }) {
            let link = Link { api, runtime: self.runtime.clone(), tx: self.tx.clone(), connection: self.connection,
                servers: self.server_choices(), servers_rev: self.servers_rev };
            self.new_chat_folders.set(None);
            self.new_chat = Some(cx.new(|cx| {
                let mut view = NewSession::new(link, None, window, cx);
                view.compact = true;
                view.menu = self.new_chat_folders.clone();
                cx.defer_in(window, |view, window, cx| view.load(window, cx));
                view
            }));
            cx.observe(self.new_chat.as_ref().unwrap(), |this, _, cx| {
                this.check_dictation_owner(cx);
                this.redraw(panes::Area::Bottom, cx);
            }).detach();
        }
        self.new_chat.clone().unwrap()
    }

    /// Com `baton`, o mesmo diálogo cria a sessão que continua aquela (o "Continuar em outra conta" do menu da sessão).
    pub(super) fn open_new_session(&mut self, baton: Option<Baton>, window: &mut Window, cx: &mut Context<Self>) {
        let api = match &baton {
            Some(baton) => self.machine_api(&baton.server),
            None => self.api.clone(),
        };
        let Some(api) = api else {
            if let Some(baton) = &baton { window.push_notification(Notification::error(self.machine_error(&baton.server)), cx); }
            return;
        };
        let link = Link { api, runtime: self.runtime.clone(), tx: self.tx.clone(), connection: self.connection,
            servers: self.server_choices(), servers_rev: self.servers_rev };
        let dialog = cx.new(|cx| NewSession::new(link, baton, window, cx));
        let query = dialog.read(cx).query.clone();
        dialog.update(cx, |d, cx| d.load(window, cx));
        self.new_session = Some(dialog.clone());
        let weak = cx.entity().downgrade();
        let width = (window.viewport_size().width * 0.94).min(px(1320.));
        window.open_dialog(cx, move |d, _, cx| {
            // Criando, o diálogo não fecha: ele é o único lugar onde o resultado aparece, como o Adicionar de Máquinas.
            let busy = dialog.read(cx).creating || dialog.read(cx).headless_saving;
            let (weak, me) = (weak.clone(), dialog.entity_id());
            let cancel = dialog.clone();
            popup::dialog(d).w(width).margin_top(px(DIALOG_TOP)).child(dialog.clone()).keyboard(!busy).overlay_closable(!busy).close_button(!busy)
                .on_ok(enter_to_focused)
                .on_cancel(move |_, window, cx| !cancel.update(cx, |dialog, cx| dialog.cancel_customizations(window, cx)))
                .on_close(move |_, _, cx| { let _ = weak.update(cx, |this, _| {
                    if this.new_session.as_ref().is_some_and(|d| d.entity_id() == me) { this.new_session = None; }
                }); })
        });
        query.update(cx, |input, cx| input.focus(window, cx));
    }

    pub(super) fn receive_create(&mut self, dialog: EntityId, reply: CreateReply, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entity) = self.new_session.iter().chain(self.new_chat.iter()).find(|d| d.entity_id() == dialog).cloned() else { return };
        let compact = entity.read(cx).compact;
        let first = match &reply {
            CreateReply::CreatedWithInput(_, selection, text, message, result, _) => Some((*selection, text.clone(), message.clone(), result.clone())),
            _ => None,
        };
        let finished = matches!(&reply, CreateReply::Created(..) | CreateReply::CreatedWithInput(..));
        let Some(Opened { session, notes, warning }) = entity.update(cx, |d, cx| d.receive(reply, window, cx)) else {
            // A tela sem sessão já mostrava a mensagem como enviada: volta com o erro dela e o texto no campo.
            if compact && finished && !entity.read(cx).creating { self.fail_opening(window, cx); cx.notify(); }
            return;
        };
        // A mensagem enviada da tela sem sessão segue na conversa da sessão nova; a chegada já começou no Enviar.
        let opening = if compact { self.opening.take() } else { None };
        // Criar não desfaz a escolha de outra conversa feita enquanto o pedido estava em voo.
        let current = first.as_ref().is_none_or(|(selection, ..)| *selection == self.selection && self.selected.is_none());
        // Nasceu em outra máquina: abre lá sem trocar o servidor ativo. A lista guardada já a inclui, para a leitura
        // que chega primeiro não fechá-la.
        let server = entity.read(cx).link.api.identity();
        let target = super::servers::norm(&server);
        if let Some(list) = self.remote.get_mut(&target).filter(|l| l.loaded && !l.sessions.iter().any(|s| s.name == session.name)) {
            list.sessions.push(session.clone());
        }
        // Sessão ainda sem transcript não tem chave: a primeira mensagem que falhou não tem onde esperar.
        let unkeyed = first.as_ref().filter(|_| SessionKey::new(&server, &session).is_none())
            .and_then(|(_, text, _, result)| result.as_ref().err().map(|error| (text.clone(), Self::failure(error))));
        // Chaves e seleção valem só na máquina onde a sessão nasceu.
        if let Some((_, text, message, result)) = first.as_ref()
            && let Some(key) = SessionKey::new(&server, &session) {
            self.drafts.entry(key.clone()).or_insert_with(|| text.clone());
            if result.is_err() && self.selected_key().as_ref() == Some(&key) && self.composer.read(cx).value().is_empty() {
                self.composer.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
            }
            self.delivery.begin(key.clone(), message.clone(), HashSet::new());
            self.receive_sent(key.clone(), message.clone(), text.clone(), result.clone(), window, cx);
            // Não entregues, os anexos da tela sem sessão ficam no campo da sessão nova, para mandar de novo.
            if result.is_err() && let Some(list) = self.attachments.remove(&new_chat_key()) {
                self.attachments.entry(key).or_default().extend(list);
            }
        }
        if first.is_some() && unkeyed.is_none() && let Some(list) = self.attachments.remove(&new_chat_key()) {
            for image in list.into_iter().filter_map(|a| a.image) { release_image(image, window, cx); }
        }
        let home = if compact { self.new_chat.take() } else {
            self.new_session = None;
            window.close_dialog(cx);
            None
        };
        let readable = session.readable();
        let key = SessionKey::new(&server, &session);
        if current { self.select_on(&target, session, window, cx); }
        // Os anexos ficam na tela de nova conversa; o texto volta ao campo.
        if let Some((text, error)) = unkeyed {
            if current && self.composer.read(cx).value().is_empty() { self.composer.update(cx, |input, cx| input.set_value(text, window, cx)); }
            window.push_notification(Notification::warning(tr("first_message_not_sent").replace("{erro}", &error)).autohide(false), cx);
        }
        let sent =first.as_ref().is_some_and(|(.., result)| result.is_ok());
        match opening.filter(|_| current) {
            // O envio que falhou volta ao campo pelo rascunho, com o aviso da entrega; a bolha não fica.
            Some(opening) => if sent {
                let sent = first.as_ref().map(|(_, _, message, _)| message.clone()).unwrap_or_default();
                self.opening = Some(super::landing::Opening { key, sent, ..opening });
                self.sync_rows(cx);
            },
            None => if let Some(home) = home.filter(|_| current && !cx.reduce_motion()) { self.start_landing(home, window); },
        }
        // O fechar devolveu o foco ao botão que abriu; a sessão nova é onde se escreve em seguida, como no clique na aba.
        if current && readable { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
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
                .icon(chrome::small_icon(IconName::Plus, 16., theme::muted())).size(px(28.)).rounded(px(6.)).flex_shrink_0().tooltip_with_action(tr("create_title"), &super::OpenNewSession, None)
        } else {
            // O `.cta-new` do web: pílula cheia no destaque, com o rótulo curto; o nome inteiro fica no leitor de tela.
            Button::new(id).primary().icon(IconName::Plus).label(super::costs::web("lista_nova_curto")).h(px(36.)).px(px(12.)).rounded_full()
                .font_weight(FontWeight::SEMIBOLD).flex_shrink_0().tooltip_with_action(tr("create_title"), &super::OpenNewSession, None)
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
    fn detached_head_base_is_first_local_branch() {
        let checkout = super::checkout_of(Ok(json!({"current": null, "branches": ["dev", "main"],
            "remotes": ["r"], "dirty": false}))).unwrap().unwrap();
        assert_eq!(super::default_base(&checkout).as_deref(), Some("dev"));
    }

    #[test]
    fn new_session_uses_connected_codex_account_when_default_is_disconnected() {
        let accounts: Vec<super::CodexAccount> = serde_json::from_value(json!([
            {"id":"default", "name":"Default", "is_default":true, "auth":{"status":"disconnected"}},
            {"id":"gpt", "name":"GPT", "is_default":false, "auth":{"status":"connected"}}
        ])).unwrap();
        assert_eq!(super::choose_codex_account(&accounts, None), Some(1));
        assert_eq!(super::choose_codex_account(&accounts, Some("default")), Some(0));
    }

    #[test]
    fn transfer_keeps_clicked_account_and_rejects_removed_account() {
        let accounts: Vec<super::CodexAccount> = serde_json::from_value(json!([
            {"id":"account-a", "name":"A", "is_default":true}, {"id":"account-b", "name":"B", "is_default":false}
        ])).unwrap();
        assert_eq!(super::choose_codex_account(&accounts, Some("account-b")), Some(1));
        assert_eq!(super::choose_codex_account(&accounts, Some("removed")), None);
        assert_eq!(super::choose_codex_account(&accounts, None), Some(0));
        assert_eq!(super::choose_codex_account(&accounts[1..], None), Some(0));
        assert_eq!(super::choose_codex_account(&[], Some("account-b")), None);
    }

    #[test]
    fn enter_starts_the_session_once_the_form_is_shown() {
        use super::{EnterAction::*, enter_action};
        // Sem pasta, o Enter da busca escolhe; o do Nome não faz nada.
        assert_eq!(enter_action(false, false, Some(false)), Pick);
        assert_eq!(enter_action(false, false, None), Nothing);
        // Pasta escolhida e busca igual: começa, na busca e no Nome.
        assert_eq!(enter_action(false, true, Some(false)), Submit);
        assert_eq!(enter_action(false, true, None), Submit);
        // Outra busca digitada depois da escolha: escolhe a pasta nova.
        assert_eq!(enter_action(false, true, Some(true)), Pick);
        // Tela sem sessão e transferência: o Enter só escolhe a pasta.
        assert_eq!(enter_action(true, true, Some(false)), Pick);
        assert_eq!(enter_action(true, true, None), Nothing);
    }

    #[test]
    fn enter_picks_the_highlighted_folder_before_starting() {
        use super::search_moved;
        // Escolhida A pela busca "a": Enter de novo, com o realce em A, começa.
        assert!(!search_moved("a", "a", "/r", "/r", Some("/r/a"), Some("/r/a")));
        // Sem realce (pasta do computador, caminho digitado): começa.
        assert!(!search_moved("", "", "/r", "/r", None, Some("/x")));
        // ↓ ou o ponteiro realçam B: o Enter escolhe B em vez de criar em A.
        assert!(search_moved("a", "a", "/r", "/r", Some("/r/b"), Some("/r/a")));
        // Entrou numa subpasta com a busca vazia: a lista é outra, e o Enter escolhe.
        assert!(search_moved("", "", "/r/a", "/r", Some("/r/a"), Some("/r/a")));
        // Outra busca digitada: escolhe.
        assert!(search_moved("b", "a", "/r", "/r", Some("/r/a"), Some("/r/a")));
    }

    #[test]
    fn root_shortcut_wraps_in_both_directions() {
        assert_eq!(super::next_root(0, 0, false), None);
        assert_eq!(super::next_root(0, 1, true), Some(0));
        assert_eq!(super::next_root(0, 3, false), Some(1));
        assert_eq!(super::next_root(2, 3, false), Some(0));
        assert_eq!(super::next_root(0, 3, true), Some(2));
        assert_eq!(super::next_root(2, 3, true), Some(1));
    }

    #[test]
    fn new_codex_session_prefers_connected_account_over_disconnected_default() {
        let accounts: Vec<super::CodexAccount> = serde_json::from_value(json!([
            {"id":"old", "name":"Old", "is_default":true, "auth":{"status":"disconnected"}},
            {"id":"ready", "name":"Ready", "auth":{"status":"connected"}}
        ])).unwrap();
        assert_eq!(super::choose_codex_account(&accounts, None), Some(1));
        assert_eq!(super::choose_codex_account(&accounts, Some("old")), Some(0));
    }

    fn transfer_request() -> super::TransferRequest {
        super::TransferRequest { target: super::Target::new("server-a", "same-name"), source_life: "k:original".into(),
            source_jsonl: "/original.jsonl".into(), credential_id: "codex:/registered".into(), model: None, effort: None, seq: 1 }
    }

    #[test]
    fn transfer_captures_server_life_history_and_dialog_without_default_overrides() {
        let request = transfer_request();
        let selected_later = super::Target::new("server-b", "same-name");
        assert_ne!(request.target, selected_later);
        assert_eq!(request.body(), json!({"credential_id":"codex:/registered", "source_life":"k:original",
            "source_jsonl":"/original.jsonl", "model":null, "effort":null}));
        let dialog = super::EntityId::from(1);
        let replacement = super::EntityId::from(2);
        assert!(super::transfer_dialog_matches(Some(dialog), dialog));
        assert!(!super::transfer_dialog_matches(Some(replacement), dialog));
        assert!(!super::transfer_dialog_matches(None, dialog));
        let mut source = super::SessionInfo { provider: "claude".into(), name: "same-name".into(), state: "idle".into(),
            conta: Some("claude:/account".into()), lifecycle_id: Some("k:original".into()), jsonl: Some("/original.jsonl".into()),
            ..Default::default() };
        assert!(request.source_matches(&source));
        source.lifecycle_id = Some("k:recreated".into());
        assert!(!request.source_matches(&source));
        source.lifecycle_id = Some("k:original".into());
        source.jsonl = Some("/switched.jsonl".into());
        assert!(!request.source_matches(&source));
    }

    #[test]
    fn transfer_errors_keep_recovery_code_and_backend_reason() {
        for code in ["session_transfer_restore_failed", "session_transfer_source_changed"] {
            let error = Failure { status: Some(409), detail: format!("{code}: backend reason"), retry_after: None, uncertain: false, code: None };
            let text = super::transfer_failure(&error);
            assert!(text.starts_with(&tr(code)));
            assert!(text.ends_with("backend reason"));
            assert!(!text.contains(&format!("{code}:")));
        }
    }

    #[test]
    fn transfer_requires_effective_choices_and_preserves_native_defaults() {
        let mut request = transfer_request();
        let native = json!({"model":"native-resolved-model", "effort":"ultra"});
        assert_eq!(request.confirmed_choices(&native), Some(("native-resolved-model", Some("ultra"))));
        assert!(request.model.is_none() && request.effort.is_none());
        let native_without_effort = json!({"model":"native-resolved-model", "effort":null});
        assert_eq!(request.confirmed_choices(&native_without_effort), Some(("native-resolved-model", None)));
        for invalid in [
            json!({"effort":null}), json!({"model":null,"effort":null}), json!({"model":12,"effort":null}),
            json!({"model":"","effort":null}), json!({"model":"default","effort":null}),
            json!({"model":"model with spaces","effort":null}), json!({"model":"--flag","effort":null}),
            json!({"model":"native-resolved-model"}), json!({"model":"native-resolved-model","effort":12}),
            json!({"model":"native-resolved-model","effort":""}), json!({"model":"native-resolved-model","effort":"default"}),
            json!({"model":"native-resolved-model","effort":"HIGH"}), json!({"model":"native-resolved-model","effort":"high-level"}),
        ] {
            assert!(request.confirmed_choices(&invalid).is_none(), "{invalid}");
        }
        request.model = Some("selected-model".into());
        request.effort = Some("high".into());
        let confirmed = json!({"model":"selected-model","effort":"high"});
        assert_eq!(request.confirmed_choices(&confirmed), Some(("selected-model", Some("high"))));
        for different in [native, native_without_effort, json!({"model":"selected-model","effort":null}),
            json!({"model":"selected-model","effort":"low"}), json!({"model":"another-model","effort":"high"})] {
            assert!(request.confirmed_choices(&different).is_none(), "{different}");
        }
    }

    #[tokio::test]
    async fn transfer_posts_to_captured_server_and_requires_published_session() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for case in [0, 1, 2, 3, 4, 5, 6, 7, 8] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = super::Api::new(&format!("http://{}", listener.local_addr().unwrap()), "test").unwrap();
            let server = tokio::spawn(async move {
                let mut captured = Vec::new();
                let mut replies = vec![json!({"ok":case != 0, "provider":"codex", "conta":"codex:/registered", "transfer_id":"transfer-a",
                    "model":"native-resolved-model", "effort":if matches!(case, 7 | 8) {json!("ultra")} else {serde_json::Value::Null}})];
                if case != 0 {
                    let mut session = json!({"name":"same-name", "provider":"codex", "conta":"codex:/registered",
                    "transfer_id":if case == 2 {"other-transfer"} else {"transfer-a"},
                    "transfer_phase":if case == 3 {"publishing"} else {"complete"},
                    "lifecycle_id":"k:original", "jsonl":"/destination.jsonl"});
                    if case == 4 { session.as_object_mut().unwrap().remove("transfer_phase"); }
                    if case == 5 { session["model"] = json!("other-model"); }
                    if case == 6 { session["effort"] = json!("high"); }
                    if case == 7 { session["model"] = json!("native-resolved-model"); session["effort"] = json!("ultra"); }
                    if case == 8 { session["effort"] = serde_json::Value::Null; }
                    replies.push(json!([session]));
                }
                for reply in replies {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0; 4096];
                    loop {
                        let n = stream.read(&mut chunk).await.unwrap();
                        if n == 0 { break; }
                        bytes.extend_from_slice(&chunk[..n]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:")
                                .and_then(|l| l.trim().parse::<usize>().ok())).unwrap_or(0);
                            if bytes.len() >= end + 4 + length { break; }
                        }
                    }
                    captured.push(String::from_utf8(bytes).unwrap());
                    let body = reply.to_string();
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                captured
            });
            let result = super::transferred(&api, &transfer_request()).await;
            assert_eq!(result.is_ok(), matches!(case, 1 | 7));
            let captured = tokio::time::timeout(std::time::Duration::from_secs(2), server).await.unwrap().unwrap();
            assert!(captured[0].starts_with("POST /api/sessions/same-name/conta "));
            let body = captured[0].split_once("\r\n\r\n").unwrap().1;
            assert_eq!(serde_json::from_str::<serde_json::Value>(body).unwrap(), transfer_request().body());
            assert_eq!(captured.len(), if case != 0 { 2 } else { 1 });
            if case != 0 { assert!(captured[1].starts_with("GET /api/sessions ")); }
        }
    }

    #[test]
    fn checkout_hides_only_unsupported_or_non_git_and_rejects_old_folders() {
        let failure = |status, detail: &str| Err(Failure { status: Some(status), detail: detail.into(), retry_after: None, uncertain: false, code: None });
        assert!(super::checkout_of(failure(404, "Not Found")).unwrap().is_none());
        assert!(super::checkout_of(failure(409, "fatal: not a git repository (or any of the parent directories): .git")).unwrap().is_none());
        assert_eq!(super::checkout_of(failure(409, "fatal: bad config")).unwrap_err(), "fatal: bad config");
        assert!(super::checkout_of(Ok(json!({}))).is_err());
        let checkout = super::checkout_of(Ok(json!({"current": "main", "branches": ["main", "feature"],
            "remotes": ["remote-feature"], "dirty": true}))).unwrap().unwrap();
        assert_eq!(checkout.current.as_deref(), Some("main"));
        assert_eq!(checkout.branches, ["main", "feature"]);
        assert_eq!(checkout.remotes, ["remote-feature"]);
        assert!(checkout.dirty);
        let mut pending = super::Remote::default();
        let old = pending.start();
        let current = pending.start();
        assert!(pending.finish(current, Ok(None)));
        assert!(!pending.finish(old, Ok(Some(checkout))));
        assert!(matches!(pending.ok(), Some(None)));
        let created = super::SessionInfo { name: "api".into(), cwd: Some("/projects/repo-api".into()),
            branch: Some("feature".into()), ..Default::default() };
        assert!(super::created_here(&created, "api", "/projects/repo/src", Some("feature")));
        assert!(!super::created_here(&created, "api", "/projects/repo/src", None));
        assert!(!super::created_here(&created, "api", "/other/repo", Some("feature")));
        assert!(!super::created_here(&created, "api", "/projects/other-repo", Some("feature")));
        assert!(!super::created_here(&created, "api", "/projects/repo", Some("main")));
    }

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
        // Pasta de servidor Windows: o nome da sessão é a última pasta, não "C--Users-Jefferson-Batista".
        assert_eq!(unique_name(basename(r"C:\Users\Jefferson Batista"), &HashSet::new()), "Jefferson-Batista");
    }

    #[test]
    fn scan_refusals_become_the_reason_and_not_a_list() {
        let refused = |status| Err(Failure { status: Some(status), detail: "x".into(), retry_after: None, uncertain: false, code: None });
        for (status, key) in [(400, "create_scan_invalid"), (403, "create_scan_root"), (404, "create_scan_missing"), (500, "create_scan_failed")] {
            let scan = scan_of(refused(status)).ok().unwrap();
            assert!(scan.entries.is_empty());
            assert_eq!(scan.error, Some(tr(key)));
        }
        let ok = scan_of(Ok(json!({"entries": [{"name": "a", "path": "/r/a", "is_git": true, "mtime": 1.0}], "error": null}))).ok().unwrap();
        assert_eq!((ok.entries.len(), ok.error), (1, None));
    }
}

#[cfg(test)]
mod worktree_tests {
    use super::worktree_body;

    #[test]
    fn body_for_new_branch() {
        assert_eq!(worktree_body("nova", true, "develop"),
                   serde_json::json!({"branch": "nova", "new_branch": true, "base": "develop"}));
        assert_eq!(worktree_body("x", false, ""), serde_json::json!({"branch": "x"}));
    }
}
